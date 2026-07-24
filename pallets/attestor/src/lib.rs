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
//! ## Custody
//!
//! Attestor and challenger bonds are real native-asset holds. Slash proceeds
//! are transferred to the configured INSURANCE account, and every custody
//! mutation runs inside the dispatch storage layer so an underfunded seating or
//! failed settlement is a strict no-op (G-1/R-7).
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
    Attestation, AttestationId, AttestationRevocation, AttestorInfo, AttestorLiability,
    AttestorOrigin, AttestorParams, AttestorRegistry, ChallengeStatus, Error as CoreError,
    Event as CoreEvent, ATTESTOR_BOND, CHALLENGE_BOND, CHALLENGE_WINDOW_BLOCKS,
    FALSE_EJECTION_THRESHOLD, MIN_MEMBERS, QUORUM,
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

/// Retained liability rows are bounded by the same maximum as the roster.
pub const MAX_LIABILITIES: u32 = MAX_ATTESTORS;

/// Live attestor tunables sourced from `pallet-constitution::Params`.
pub trait AttestorParamsProvider {
    fn get() -> AttestorParams;
}

/// Terminal/executed proposal feed used by permissionless record reaping and
/// cause-aware revocation. The runtime owns the concrete epoch lookup.
pub trait AttestorProposalStatus {
    fn has_executed(pid: futarchy_primitives::ProposalId) -> bool;
    fn is_terminal(pid: futarchy_primitives::ProposalId) -> bool;
}

/// Maps authority roles to concrete origins for the v2 benchmark harness.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    /// Endow benchmark accounts and install the attestor/challenge holds
    /// required by custody-aware extrinsics. Production genesis already
    /// provisions these holds; the omni-bencher starts from an empty state.
    fn prime_funds() {}
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
    use frame_support::traits::{
        fungible::{Inspect, InspectHold, MutateHold},
        tokens::{Fortitude, Precision, Restriction},
        EnsureOrigin,
    };
    use frame_system::pallet_prelude::*;
    use sp_runtime::{SaturatedConversion, TryRuntimeError};

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    /// Native VIT hold namespaces owned by this pallet.
    #[pallet::composite_enum]
    pub enum HoldReason {
        AttestorBond,
        ChallengeBond,
    }

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

        /// Native VIT custody for attestor/challenger bonds.
        type Currency: Inspect<Self::AccountId, Balance = futarchy_primitives::Balance>
            + InspectHold<Self::AccountId, Reason = Self::RuntimeHoldReason>
            + MutateHold<Self::AccountId>;

        /// Aggregate runtime hold reason.
        type RuntimeHoldReason: From<HoldReason>;

        /// INSURANCE sink for every slash leg.
        type InsuranceAccount: Get<Self::AccountId>;

        /// Proposal status for durable revocation/reap checks.
        type ProposalStatus: AttestorProposalStatus;

        /// Weight information for all six calls.
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

    /// Bond bases independent of the active roster (02 §7.5, v10).
    #[pallet::storage]
    pub type Liabilities<T: Config> =
        StorageValue<_, BoundedVec<AttestorLiability, ConstU32<MAX_LIABILITIES>>, ValueQuery>;

    /// Durable cause markers for records that lost their signer (02 §7.5, v10).
    #[pallet::storage]
    pub type Revocations<T: Config> =
        StorageValue<_, BoundedVec<AttestationRevocation, ConstU32<MAX_ATTESTATIONS>>, ValueQuery>;

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
        /// A values-authorized cause removed an attestor from the active roster.
        AttestorRemovedForCause {
            who: T::AccountId,
            cause_hash: futarchy_primitives::H256,
        },
        /// A record was durably revoked by a cause-aware removal/ejection.
        AttestationRevoked {
            attestation_id: AttestationId,
            pid: futarchy_primitives::ProposalId,
            attestor: T::AccountId,
            cause_hash: futarchy_primitives::H256,
        },
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
        TooManyLiabilities,
        TooManyRevocations,
        LiabilityExists,
        AttestorNotFound,
        LiabilityNotFound,
        ProposalNotTerminal,
        ChallengeOpen,
        ReapNotAllowed,
        BondAccounting,
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
            frame_support::storage::with_storage_layer(|| {
                let params = T::Params::get();
                let raw: Vec<CoreAccountId> = members.iter().map(Self::to_core).collect();
                let mut registry = Self::load().unwrap_or_else(Self::empty_core);
                let old_members = registry.members.clone();
                let attestor_reason = Self::attestor_reason();
                for member in members.iter() {
                    let raw_member = Self::to_core(member);
                    ensure!(
                        !registry
                            .liabilities
                            .iter()
                            .any(|liability| liability.account == raw_member),
                        Error::<T>::LiabilityExists
                    );
                    let old_bond = old_members
                        .iter()
                        .find(|info| info.account == raw_member)
                        .map(|info| info.bond)
                        .unwrap_or(0);
                    if params.bond > old_bond {
                        T::Currency::hold(&attestor_reason, member, params.bond - old_bond)?;
                    } else if old_bond > params.bond {
                        T::Currency::release(
                            &attestor_reason,
                            member,
                            old_bond - params.bond,
                            Precision::Exact,
                        )?;
                    }
                }
                registry
                    .set_members(
                        AttestorOrigin::ConstitutionalValues,
                        raw,
                        Self::now(),
                        params,
                    )
                    .map_err(Self::map_core_error)?;
                for previous in old_members {
                    if members
                        .iter()
                        .any(|member| Self::to_core(member) == previous.account)
                    {
                        continue;
                    }
                    let retained = registry
                        .liabilities
                        .iter()
                        .any(|liability| liability.account == previous.account);
                    if !retained && previous.bond > 0 {
                        T::Currency::release(
                            &attestor_reason,
                            &Self::from_core(previous.account),
                            previous.bond,
                            Precision::Exact,
                        )?;
                    }
                }
                Self::persist(&registry)?;
                Self::drain_events(&mut registry);
                Ok(())
            })
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
            frame_support::storage::with_storage_layer(|| {
                let mut registry = Self::load().ok_or(Error::<T>::NotInitialized)?;
                let reason = Self::challenge_reason();
                T::Currency::hold(&reason, &challenger, bond)?;
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
            })
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
            frame_support::storage::with_storage_layer(|| {
                let mut registry = Self::load().ok_or(Error::<T>::NotInitialized)?;
                let attestation = registry
                    .attestations
                    .iter()
                    .find(|attestation| attestation.id == attestation_id)
                    .copied()
                    .ok_or(Error::<T>::AttestationNotFound)?;
                let (challenger, challenge_bond) = match attestation.challenge {
                    Some(ChallengeStatus::Open {
                        challenger, bond, ..
                    }) => (challenger, bond),
                    _ => return Err(Error::<T>::NoOpenChallenge.into()),
                };
                registry
                    .resolve_challenge_with(
                        AttestorOrigin::RatifyTrack,
                        attestation_id,
                        attestation_upheld,
                        T::ProposalStatus::has_executed,
                    )
                    .map_err(Self::map_core_error)?;
                let slashed = registry
                    .events
                    .iter()
                    .rev()
                    .find_map(|event| match event {
                        CoreEvent::ChallengeResolved {
                            attestation_id: event_id,
                            slashed,
                            ..
                        } if *event_id == attestation_id => Some(*slashed),
                        _ => None,
                    })
                    .unwrap_or_default();
                let challenge_reason = Self::challenge_reason();
                let insurance = T::InsuranceAccount::get();
                if attestation_upheld {
                    let slashed = challenge_bond / 2 + challenge_bond % 2;
                    if slashed > 0 {
                        T::Currency::transfer_on_hold(
                            &challenge_reason,
                            &Self::from_core(challenger),
                            &insurance,
                            slashed,
                            Precision::Exact,
                            Restriction::Free,
                            Fortitude::Force,
                        )?;
                    }
                    let refund = challenge_bond.saturating_sub(slashed);
                    if refund > 0 {
                        T::Currency::release(
                            &challenge_reason,
                            &Self::from_core(challenger),
                            refund,
                            Precision::Exact,
                        )?;
                    }
                } else {
                    T::Currency::release(
                        &challenge_reason,
                        &Self::from_core(challenger),
                        challenge_bond,
                        Precision::Exact,
                    )?;
                    if slashed > 0 {
                        T::Currency::transfer_on_hold(
                            &Self::attestor_reason(),
                            &Self::from_core(attestation.attestor),
                            &insurance,
                            slashed,
                            Precision::Exact,
                            Restriction::Free,
                            Fortitude::Force,
                        )?;
                    }
                }
                Self::persist(&registry)?;
                Self::drain_events(&mut registry);
                Ok(())
            })
        }

        /// Remove an attestor with an explicit cause and revoke every
        /// unexecuted record atomically (06 §7, contract v11).
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::remove_for_cause())]
        pub fn remove_for_cause(
            origin: OriginFor<T>,
            who: T::AccountId,
            cause_hash: futarchy_primitives::H256,
        ) -> DispatchResult {
            T::ValuesOrigin::ensure_origin(origin)?;
            frame_support::storage::with_storage_layer(|| {
                let mut registry = Self::load().ok_or(Error::<T>::NotInitialized)?;
                registry
                    .remove_for_cause(
                        AttestorOrigin::ConstitutionalValues,
                        Self::to_core_authorized(&who)?,
                        cause_hash,
                        T::ProposalStatus::has_executed,
                    )
                    .map_err(Self::map_core_error)?;
                let raw_who = Self::to_core_authorized(&who)?;
                let no_retained_records = !registry
                    .attestations
                    .iter()
                    .any(|record| record.attestor == raw_who)
                    && !registry
                        .revocations
                        .iter()
                        .any(|record| record.attestor == raw_who);
                if no_retained_records {
                    if let Some(index) = registry
                        .liabilities
                        .iter()
                        .position(|liability| liability.account == raw_who)
                    {
                        let liability = registry.liabilities.remove(index);
                        if liability.bond > 0 {
                            T::Currency::release(
                                &Self::attestor_reason(),
                                &who,
                                liability.bond,
                                Precision::Exact,
                            )?;
                        }
                    }
                }
                Self::persist(&registry)?;
                Self::drain_events(&mut registry);
                Ok(())
            })
        }

        /// Permissionlessly reap a terminal, settled record and release the
        /// departing attestor's remaining bond basis when its last record is
        /// gone.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::reap_attestation())]
        pub fn reap_attestation(
            origin: OriginFor<T>,
            attestation_id: AttestationId,
        ) -> DispatchResult {
            ensure_signed(origin)?;
            frame_support::storage::with_storage_layer(|| {
                let mut registry = Self::load().ok_or(Error::<T>::NotInitialized)?;
                let released = registry
                    .reap_attestation(attestation_id, Self::now(), T::ProposalStatus::is_terminal)
                    .map_err(Self::map_core_error)?;
                if let Some(liability) = released {
                    if liability.bond > 0 {
                        T::Currency::release(
                            &Self::attestor_reason(),
                            &Self::from_core(liability.account),
                            liability.bond,
                            Precision::Exact,
                        )?;
                    }
                }
                Self::persist(&registry)?;
                Self::drain_events(&mut registry);
                Ok(())
            })
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
                let reason: T::RuntimeHoldReason = HoldReason::AttestorBond.into();
                for member in &self.members {
                    assert!(
                        T::Currency::hold(&reason, member, T::Params::get().bond).is_ok(),
                        "attestor genesis: each member must fund the attestor bond"
                    );
                }
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
                liabilities: Vec::new(),
                attestations: Vec::new(),
                revocations: Vec::new(),
                next_attestation_id: 0,
                events: Vec::new(),
            }
        }

        /// Rebuild the core aggregate from storage; empty membership means the
        /// registry has not been initialized.
        fn load() -> Option<AttestorRegistry> {
            let members = Members::<T>::get();
            let liabilities = Liabilities::<T>::get();
            let attestations = Attestations::<T>::get();
            let revocations = Revocations::<T>::get();
            if members.is_empty() && liabilities.is_empty() && attestations.is_empty() {
                return None;
            }
            Some(AttestorRegistry {
                members: members.into_inner(),
                liabilities: liabilities.into_inner(),
                attestations: attestations.into_inner(),
                revocations: revocations.into_inner(),
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
            let liabilities = BoundedVec::try_from(registry.liabilities.clone())
                .map_err(|_| Error::<T>::TooManyLiabilities)?;
            let revocations = BoundedVec::try_from(registry.revocations.clone())
                .map_err(|_| Error::<T>::TooManyRevocations)?;

            Members::<T>::put(members);
            Liabilities::<T>::put(liabilities);
            Attestations::<T>::put(attestations);
            Revocations::<T>::put(revocations);
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
                    CoreEvent::AttestorRemovedForCause { who, cause_hash } => {
                        Self::deposit_event(Event::AttestorRemovedForCause {
                            who: Self::from_core(who),
                            cause_hash,
                        });
                    }
                    CoreEvent::AttestationRevoked {
                        attestation_id,
                        pid,
                        attestor,
                        cause_hash,
                    } => Self::deposit_event(Event::AttestationRevoked {
                        attestation_id,
                        pid,
                        attestor: Self::from_core(attestor),
                        cause_hash,
                    }),
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

        /// Execute-time record quorum; unlike queue-time `has_quorum`, this
        /// does not read the current active roster.
        pub fn has_record_quorum(
            pid: futarchy_primitives::ProposalId,
            artifact_hash: futarchy_primitives::H256,
        ) -> bool {
            Self::load()
                .map(|registry| registry.has_record_quorum(pid, artifact_hash, Self::now()))
                .unwrap_or(false)
        }

        pub fn is_revoked(attestation_id: AttestationId) -> bool {
            Revocations::<T>::get()
                .iter()
                .any(|revocation| revocation.attestation_id == attestation_id)
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
            ensure!(
                Liabilities::<T>::get().len() as u32 <= MAX_LIABILITIES,
                TryRuntimeError::Other("attestor: Liabilities over bound")
            );
            ensure!(
                Revocations::<T>::get().len() as u32 <= MAX_ATTESTATIONS,
                TryRuntimeError::Other("attestor: Revocations over bound")
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
                CoreError::LiabilityExists => Error::<T>::LiabilityExists.into(),
                CoreError::LiabilityNotFound => Error::<T>::LiabilityNotFound.into(),
                CoreError::ProposalNotTerminal => Error::<T>::ProposalNotTerminal.into(),
                CoreError::ChallengeOpen => Error::<T>::ChallengeOpen.into(),
                CoreError::ReapNotAllowed => Error::<T>::ReapNotAllowed.into(),
                CoreError::TooManyLiabilities => Error::<T>::TooManyLiabilities.into(),
                CoreError::TooManyRevocations => Error::<T>::TooManyRevocations.into(),
                CoreError::Overflow => Error::<T>::Overflow.into(),
            }
        }

        fn attestor_reason() -> T::RuntimeHoldReason {
            HoldReason::AttestorBond.into()
        }

        fn challenge_reason() -> T::RuntimeHoldReason {
            HoldReason::ChallengeBond.into()
        }
    }

    impl<T: Config> GenesisConfig<T> {
        fn acct_to_core(who: &T::AccountId) -> CoreAccountId {
            who.clone().into()
        }
    }
}
