#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-attestor` — bonded kernel-attestation registry (A10)
//!
//! Production FRAME shell over the frame-free functional core
//! [`attestor_core`]. Every extrinsic resolves its explicit authority, rebuilds
//! an [`AttestorRegistry`] from decomposed bounded storage, calls the core state
//! machine, persists the result, and drains the core event accumulator.
//!
//! Spec: `docs/architecture/06` §7 (bonded registry) and §8 (frontend
//! surface), with call authorities from §3.2; `09 §1.2` consumes the quorum
//! view; `15 §4.1` requires per-call origin/error tests.
//!
//! ## Scope boundary
//!
//! Bonds are represented arithmetically exactly as in the core
//! ([`AttestorInfo::bond`] and [`ChallengeStatus::Open::bond`]). A real
//! `fungible` hold/slash and its destination are B-track runtime-integration
//! work; A10 is not one of the audit-scope-A pallets in R-7.
//!
//! Ratify-track adjudication is implemented here. The permissionless
//! deterministic-recomputation proof path in 06 §7 follows in the B track once
//! reproducible-build verification is wired off chain.

extern crate alloc;

pub use attestor_core;
pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use attestor_core::{
    Attestation, AttestationId, AttestorInfo, AttestorOrigin, AttestorParams, AttestorRegistry,
    ChallengeStatus, Error as CoreError, Event as CoreEvent, ATTESTOR_BOND, CHALLENGE_BOND,
    CHALLENGE_WINDOW_BLOCKS, FALSE_EJECTION_THRESHOLD, MIN_MEMBERS, QUORUM,
};

use futarchy_primitives::AccountId as CoreAccountId;

/// Maximum elected attestors. `13 §4` has no attestor storage-bound row; the
/// pallet-owned value and its `Members` storage shape are frozen in 02 §7.5
/// (contract v4). The membership floor/quorum remain the kernel constants
/// `ATT_MIN_MEMBERS` / `ATT_QUORUM`.
pub const MAX_ATTESTORS: u32 = 16;

/// Maximum flat attestation records retained by this pallet. The frame-free
/// core intentionally keeps the vector unbounded; this cap makes runtime state
/// bounded, and overflow is rejected without writes (G-1). A future map reaped
/// by settled proposal would change the 02 §7.5 contract shape and must follow
/// its versioning/migration discipline.
pub const MAX_ATTESTATIONS: u32 = 256;

/// Live attestor tunables sourced from `pallet-constitution::Params`.
pub trait AttestorParamsProvider {
    fn get() -> AttestorParams;
}

/// Maps authority roles to concrete origins for the v2 benchmark harness.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    /// Construct a signed origin for `who`.
    fn signed(who: CoreAccountId) -> RuntimeOrigin;
    /// Construct a `ConstitutionalValues` origin.
    fn values() -> RuntimeOrigin;
    /// Construct a `ratify`-track origin.
    fn ratify() -> RuntimeOrigin;
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::EnsureOrigin;
    use frame_system::pallet_prelude::*;
    use sp_runtime::{SaturatedConversion, TryRuntimeError};

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    // The canonical account is AccountId32 (02 §8). Bounding the frame-system
    // supertrait's associated type makes the `[u8; 32]` core bridge available
    // to every `impl<T: Config>`.
    #[pallet::config]
    pub trait Config:
        frame_system::Config<
        AccountId: Into<CoreAccountId> + From<CoreAccountId>,
        RuntimeEvent: From<Event<Self>>,
    >
    {
        /// Live constitution values for attestor bonds and challenge windows.
        type Params: AttestorParamsProvider;

        /// `ConstitutionalValues` authority for `attestor.set_members`
        /// (06 §3.2 row 5).
        type ValuesOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// `ratify`-track authority for challenge adjudication (06 §7).
        type RatifyOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// Weight information for all four calls.
        type WeightInfo: WeightInfo;

        /// Origin construction for benchmarking.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin>;
    }

    /// Elected bonded registry members (06 §8: `attestor.Members`). Empty until
    /// genesis or the first `set_members`.
    #[pallet::storage]
    pub type Members<T: Config> =
        StorageValue<_, BoundedVec<AttestorInfo, ConstU32<MAX_ATTESTORS>>, ValueQuery>;

    /// Flat bounded attestation ledger mirroring the core's `Vec<Attestation>`;
    /// this exact shipped value shape is frozen in 02 §7.5. Exceeding the cap is
    /// a rejected no-op (G-1).
    #[pallet::storage]
    pub type Attestations<T: Config> =
        StorageValue<_, BoundedVec<Attestation, ConstU32<MAX_ATTESTATIONS>>, ValueQuery>;

    /// Monotonic attestation id cursor.
    #[pallet::storage]
    pub type NextAttestationId<T: Config> = StorageValue<_, AttestationId, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Registry members were replaced by the values track.
        MembersSet { members: Vec<T::AccountId> },
        /// A bonded member submitted an artifact attestation.
        AttestationSubmitted {
            attestation_id: AttestationId,
            pid: futarchy_primitives::ProposalId,
            artifact_hash: futarchy_primitives::H256,
            attestor: T::AccountId,
        },
        /// Anyone opened a bonded challenge inside the window.
        AttestationChallenged {
            attestation_id: AttestationId,
            challenger: T::AccountId,
            evidence_hash: futarchy_primitives::H256,
        },
        /// The ratify track resolved a challenge and slashed its loser.
        ChallengeResolved {
            attestation_id: AttestationId,
            upheld: bool,
            loser: T::AccountId,
            slashed: futarchy_primitives::Balance,
        },
        /// An attestor reached the second-false-attestation ejection threshold.
        AttestorEjected { who: T::AccountId },
    }

    /// Core errors map 1:1; `CoreError::BadOrigin` becomes
    /// `DispatchError::BadOrigin`. The final three variants are pallet-side
    /// initialization/storage-bound failures.
    #[pallet::error]
    pub enum Error<T> {
        NotMember,
        DuplicateMember,
        TooFewMembers,
        AttestationNotFound,
        DuplicateAttestation,
        ChallengeWindowClosed,
        ChallengeAlreadyOpen,
        ChallengeBondTooSmall,
        ChallengeStillOpen,
        QuorumMissing,
        /// The referenced attestation exists but has no open challenge.
        NoOpenChallenge,
        /// try-state only: a member at or past the ejection threshold is still
        /// active. No dispatch produces this (06 §7; SQ-262).
        EjectedMemberActive,
        Overflow,
        NotInitialized,
        TooManyAttestors,
        TooManyAttestations,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        // 06 §7 defines a thin registry with no time-driven transitions. The
        // challenge deadline is evaluated by calls/views; there is no crank.
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Install the values-elected member set (06 §3.2 row 5, §7).
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::set_members())]
        pub fn set_members(origin: OriginFor<T>, members: Vec<T::AccountId>) -> DispatchResult {
            T::ValuesOrigin::ensure_origin(origin)?;
            ensure!(
                members.len() <= MAX_ATTESTORS as usize,
                Error::<T>::TooManyAttestors
            );
            let raw: Vec<CoreAccountId> = members.iter().map(Self::to_core).collect();
            let mut registry = Self::load().unwrap_or_else(Self::empty_core);
            registry
                .set_members(
                    AttestorOrigin::ConstitutionalValues,
                    raw,
                    Self::now(),
                    T::Params::get(),
                )
                .map_err(Self::map_core_error)?;
            Self::persist(&registry)?;
            Self::drain_events(&mut registry);
            Ok(())
        }

        /// Submit a member's bonded artifact attestation (06 §7). Membership
        /// and duplicate checks are enforced by the core.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::attest())]
        pub fn attest(
            origin: OriginFor<T>,
            pid: futarchy_primitives::ProposalId,
            artifact_hash: futarchy_primitives::H256,
            statement_hash: futarchy_primitives::H256,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut registry = Self::load().ok_or(Error::<T>::NotInitialized)?;
            registry
                .attest(
                    Self::to_core_authorized(&who)?,
                    pid,
                    artifact_hash,
                    statement_hash,
                    Self::now(),
                    T::Params::get(),
                )
                .map_err(Self::map_core_error)?;
            Self::persist(&registry)?;
            Self::drain_events(&mut registry);
            Ok(())
        }

        /// Open a bonded challenge inside an attestation's 72-hour window
        /// (06 §3.2 signed row, §7).
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::challenge_attestation())]
        pub fn challenge_attestation(
            origin: OriginFor<T>,
            attestation_id: AttestationId,
            evidence_hash: futarchy_primitives::H256,
            bond: futarchy_primitives::Balance,
        ) -> DispatchResult {
            let challenger = ensure_signed(origin)?;
            let mut registry = Self::load().ok_or(Error::<T>::NotInitialized)?;
            registry
                .challenge_attestation(
                    Self::to_core_authorized(&challenger)?,
                    attestation_id,
                    evidence_hash,
                    bond,
                    Self::now(),
                )
                .map_err(Self::map_core_error)?;
            Self::persist(&registry)?;
            Self::drain_events(&mut registry);
            Ok(())
        }

        /// Resolve an open challenge through the `ratify` track (06 §7).
        /// Permissionless deterministic-recomputation resolution is deferred
        /// until B-track reproducible-build verification is available.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::resolve_challenge())]
        pub fn resolve_challenge(
            origin: OriginFor<T>,
            attestation_id: AttestationId,
            attestation_upheld: bool,
        ) -> DispatchResult {
            T::RatifyOrigin::ensure_origin(origin)?;
            let mut registry = Self::load().ok_or(Error::<T>::NotInitialized)?;
            registry
                .resolve_challenge(
                    AttestorOrigin::RatifyTrack,
                    attestation_id,
                    attestation_upheld,
                )
                .map_err(Self::map_core_error)?;
            Self::persist(&registry)?;
            Self::drain_events(&mut registry);
            Ok(())
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        /// Kernel minimum registry size (06 §7; 13 §4).
        #[pallet::constant_name(AttMinMembers)]
        fn att_min_members() -> u32 {
            futarchy_primitives::kernel::ATT_MIN_MEMBERS
        }

        /// Kernel quorum (2-of-N; 06 §7; 13 §4).
        #[pallet::constant_name(AttQuorum)]
        fn att_quorum() -> u32 {
            futarchy_primitives::kernel::ATT_QUORUM
        }

        /// Kernel floor envelope for live `att.window`: 43,200 blocks / 72 h
        /// (02 §9(2); 13 rule 7). This is not the live tunable value.
        #[pallet::constant_name(ChallengeWindowBlocks)]
        fn challenge_window_blocks() -> futarchy_primitives::BlockNumber {
            CHALLENGE_WINDOW_BLOCKS
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Optional initial member set. Empty leaves the registry uninitialized;
        /// otherwise the core enforces at least three distinct accounts.
        pub members: Vec<T::AccountId>,
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                members: Vec::new(),
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if self.members.is_empty() {
                return;
            }
            assert!(
                self.members.len() <= MAX_ATTESTORS as usize,
                "attestor genesis: at most {MAX_ATTESTORS} members"
            );
            let raw = self
                .members
                .iter()
                .map(Self::acct_to_core)
                .collect::<Vec<_>>();
            let registry = AttestorRegistry::new(raw, T::Params::get());
            assert!(
                registry.is_ok(),
                "attestor genesis: at least three unique members required (06 §7)"
            );
            if let Ok(registry) = registry {
                Members::<T>::put(BoundedVec::truncate_from(registry.members));
                NextAttestationId::<T>::put(registry.next_attestation_id);
            }
        }
    }

    impl<T: Config> Pallet<T> {
        // ---- account bridge (`T::AccountId` ↔ core `[u8; 32]`) ----
        pub(crate) fn to_core(who: &T::AccountId) -> CoreAccountId {
            who.clone().into()
        }

        /// Round-trip-checked conversion for **authorization** boundaries: a
        /// lossy `Into<[u8; 32]>` could alias two distinct runtime accounts to
        /// the same attestor bytes and let an outsider attest/challenge as a
        /// member (A10 Codex adversarial finding). The canonical runtime account
        /// is `AccountId32` (02 §8), whose conversion is bijective, so this
        /// always passes; it hardens against a misconfigured lossy `AccountId`.
        fn to_core_authorized(who: &T::AccountId) -> Result<CoreAccountId, DispatchError> {
            let raw = who.clone().into();
            ensure!(&T::AccountId::from(raw) == who, DispatchError::BadOrigin);
            Ok(raw)
        }

        pub(crate) fn from_core(raw: CoreAccountId) -> T::AccountId {
            T::AccountId::from(raw)
        }

        /// Current block as the core's u32 block number.
        fn now() -> futarchy_primitives::BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<u32>()
        }

        /// Invalid only as a transient pre-`set_members` aggregate; the core
        /// call validates and replaces the member set before persistence.
        fn empty_core() -> AttestorRegistry {
            AttestorRegistry {
                members: Vec::new(),
                attestations: Vec::new(),
                next_attestation_id: 0,
                events: Vec::new(),
            }
        }

        /// Rebuild the core aggregate from storage; empty membership means the
        /// registry has not been initialized.
        fn load() -> Option<AttestorRegistry> {
            let members = Members::<T>::get();
            if members.is_empty() {
                return None;
            }
            Some(AttestorRegistry {
                members: members.into_inner(),
                attestations: Attestations::<T>::get().into_inner(),
                next_attestation_id: NextAttestationId::<T>::get(),
                events: Vec::new(),
            })
        }

        /// Convert every vector before writing so any bound failure is a clean
        /// rejected no-op (G-1).
        fn persist(registry: &AttestorRegistry) -> DispatchResult {
            let members = BoundedVec::try_from(registry.members.clone())
                .map_err(|_| Error::<T>::TooManyAttestors)?;
            let attestations = BoundedVec::try_from(registry.attestations.clone())
                .map_err(|_| Error::<T>::TooManyAttestations)?;

            Members::<T>::put(members);
            Attestations::<T>::put(attestations);
            NextAttestationId::<T>::put(registry.next_attestation_id);
            Ok(())
        }

        /// Bridge and deposit the core's accumulated events in order.
        fn drain_events(registry: &mut AttestorRegistry) {
            for event in core::mem::take(&mut registry.events) {
                match event {
                    CoreEvent::MembersSet { members } => {
                        Self::deposit_event(Event::MembersSet {
                            members: members.into_iter().map(Self::from_core).collect(),
                        });
                    }
                    CoreEvent::AttestationSubmitted {
                        attestation_id,
                        pid,
                        artifact_hash,
                        attestor,
                    } => Self::deposit_event(Event::AttestationSubmitted {
                        attestation_id,
                        pid,
                        artifact_hash,
                        attestor: Self::from_core(attestor),
                    }),
                    CoreEvent::AttestationChallenged {
                        attestation_id,
                        challenger,
                        evidence_hash,
                    } => Self::deposit_event(Event::AttestationChallenged {
                        attestation_id,
                        challenger: Self::from_core(challenger),
                        evidence_hash,
                    }),
                    CoreEvent::ChallengeResolved {
                        attestation_id,
                        upheld,
                        loser,
                        slashed,
                    } => Self::deposit_event(Event::ChallengeResolved {
                        attestation_id,
                        upheld,
                        loser: Self::from_core(loser),
                        slashed,
                    }),
                    CoreEvent::AttestorEjected { who } => {
                        Self::deposit_event(Event::AttestorEjected {
                            who: Self::from_core(who),
                        });
                    }
                }
            }
        }

        /// FE projection for 06 §8 `attestor.AttestationsFor`, derived from the
        /// flat `Attestations` value frozen in 02 §7.5.
        pub fn attestations_for(pid: futarchy_primitives::ProposalId) -> Vec<Attestation> {
            Attestations::<T>::get()
                .into_iter()
                .filter(|attestation| attestation.pid == pid)
                .collect()
        }

        /// FE projection for 06 §8 `attestor.OpenChallenges`, derived from the
        /// flat `Attestations` value frozen in 02 §7.5.
        pub fn open_challenges() -> Vec<Attestation> {
            Attestations::<T>::get()
                .into_iter()
                .filter(|attestation| {
                    matches!(attestation.challenge, Some(ChallengeStatus::Open { .. }))
                })
                .collect()
        }

        /// Quorum view consumed by the execution guard (09 §1.2).
        pub fn has_quorum(
            pid: futarchy_primitives::ProposalId,
            artifact_hash: futarchy_primitives::H256,
        ) -> bool {
            Self::load()
                .map(|registry| registry.has_quorum(pid, artifact_hash, Self::now()))
                .unwrap_or(false)
        }

        /// Rebuild the aggregate and run the core validator plus defensive
        /// FRAME-side bound assertions (15 §1).
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            ensure!(
                Members::<T>::get().len() as u32 <= MAX_ATTESTORS,
                TryRuntimeError::Other("attestor: Members over bound")
            );
            ensure!(
                Attestations::<T>::get().len() as u32 <= MAX_ATTESTATIONS,
                TryRuntimeError::Other("attestor: Attestations over bound")
            );
            if let Some(registry) = Self::load() {
                registry.try_state().map_err(|_| {
                    TryRuntimeError::Other("attestor core try_state failed (06 §7)")
                })?;
            }
            Ok(())
        }

        pub(crate) fn map_core_error(error: CoreError) -> DispatchError {
            match error {
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::NotMember => Error::<T>::NotMember.into(),
                CoreError::DuplicateMember => Error::<T>::DuplicateMember.into(),
                CoreError::TooFewMembers => Error::<T>::TooFewMembers.into(),
                CoreError::AttestationNotFound => Error::<T>::AttestationNotFound.into(),
                CoreError::DuplicateAttestation => Error::<T>::DuplicateAttestation.into(),
                CoreError::ChallengeWindowClosed => Error::<T>::ChallengeWindowClosed.into(),
                CoreError::ChallengeAlreadyOpen => Error::<T>::ChallengeAlreadyOpen.into(),
                CoreError::ChallengeBondTooSmall => Error::<T>::ChallengeBondTooSmall.into(),
                CoreError::ChallengeStillOpen => Error::<T>::ChallengeStillOpen.into(),
                CoreError::QuorumMissing => Error::<T>::QuorumMissing.into(),
                CoreError::NoOpenChallenge => Error::<T>::NoOpenChallenge.into(),
                CoreError::EjectedMemberActive => Error::<T>::EjectedMemberActive.into(),
                CoreError::Overflow => Error::<T>::Overflow.into(),
            }
        }
    }

    impl<T: Config> GenesisConfig<T> {
        fn acct_to_core(who: &T::AccountId) -> CoreAccountId {
            who.clone().into()
        }
    }
}
