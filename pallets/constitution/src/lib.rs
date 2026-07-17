#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-constitution` — governance parameter registry (A1)
//!
//! Production FRAME shell over the frame-free functional core
//! [`constitution-core`], which remains the differential oracle
//! (Python M3 ≡ Rust core ≡ this pallet) and the auditor-consumable port.
//!
//! Spec: `docs/architecture/06` (authority matrix §3), `13 §1/§4`
//! (typed/bounded/rate-limited keys, storage bounds), `02 §7.3/§12`
//! (frozen storage surface, D-14 `ReleaseChannel` raw layout),
//! `15 §1` (I-6 + meter-primitive I-7 try-state coverage; the I-17
//! envelope meters live with treasury/guard).
//!
//! ## Frozen contract surface (02 §7.3/§12 — names byte-for-byte)
//!
//! - `Params: map ParamKey → ParamRecord`
//! - `PhaseFlags: u32` bitset (bit assignments in 02 §7.3, bits 8–31 reserved)
//! - `ReleaseChannel`: 168-byte fixed layout under the raw key
//!   `twox128("Constitution") ++ twox128("ReleaseChannel")`. **The runtime MUST
//!   instance this pallet as `Constitution`** or the D-14 key changes;
//!   `try_state` asserts the real final key against the frozen constant.
//!
//! ## Origins (06 §3)
//!
//! Every extrinsic resolves its origin through
//! [`Config::GovernanceOrigin`] into the core's [`ConstitutionOrigin`] and then
//! enforces the 06 §3.2 authority-matrix predicate for that call — two
//! independent checks once the runtime's `SafetyFilter` (`BaseCallFilter`) is
//! in front (06 §3.3). The runtime wires `GovernanceOrigin` over
//! `pallet-origins` (A4); the mock provides a test resolver.

extern crate alloc;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

// The functional core is the semantic source of truth; re-export its surface
// (named, not glob — the pallet defines its own `Error`/storage aliases).
pub use constitution_core::{
    empty_release_channel, genesis_capabilities, genesis_meters, genesis_params, key16, Capability,
    CapabilityRecord, ConstitutionOrigin, ConstitutionState, Error as CoreError, MaxDelta, Meter,
    ParamClass, ParamRecord, ParamValue, PhaseFlags as PhaseFlagsValue,
    ReleaseChannel as ReleaseChannelValue, CONTRACT_VERSION, MAX_CAPABILITIES, MAX_METERS,
    MAX_PARAMS, RELEASE_CHANNEL_FLAGS, RELEASE_CHANNEL_FLAG_URGENT_UPGRADE, RELEASE_CHANNEL_LEN,
    RELEASE_CHANNEL_PENDING_AUTHORIZED_AT, RELEASE_CHANNEL_SPEC_VERSION,
    RELEASE_CHANNEL_STORAGE_KEY, RELEASE_CHANNEL_UPDATED_AT,
};
pub use futarchy_primitives::kernel;

/// Storage-bound forms of the core limits (13 §4 / core constants).
pub const MAX_PARAMS_BOUND: u32 = MAX_PARAMS as u32;
/// See [`MAX_PARAMS_BOUND`].
pub const MAX_CAPABILITIES_BOUND: u32 = MAX_CAPABILITIES as u32;
/// See [`MAX_PARAMS_BOUND`].
pub const MAX_METERS_BOUND: u32 = MAX_METERS as u32;

/// Maps an authority-matrix origin to a concrete runtime origin so benchmarks
/// can exercise every call with its exact 06 §3.2 authority.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    /// Return a runtime origin that [`Config::GovernanceOrigin`] resolves to
    /// `authority`.
    fn origin(authority: ConstitutionOrigin) -> RuntimeOrigin;
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::EnsureOrigin;
    use frame_system::pallet_prelude::*;
    use sp_runtime::{traits::UniqueSaturatedInto, TryRuntimeError};

    use futarchy_primitives::{EpochId, ParamKey, ProposalClass};

    /// The in-code storage version of this pallet.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        /// Resolves a runtime origin into the 06 §3 authority-matrix origin.
        ///
        /// The runtime implements this over `pallet-origins`' custom origins
        /// (A4/B1a); no signed or unsigned origin may resolve to a governance
        /// variant. Each call then enforces its own matrix row predicate.
        type GovernanceOrigin: EnsureOrigin<Self::RuntimeOrigin, Success = ConstitutionOrigin>;

        /// Current epoch index, for 13 §1 cooldowns and I-7/I-17 meter
        /// windows. Wired to `pallet-epoch`'s clock by the runtime (A8/B1a);
        /// a constant in the mock.
        type CurrentEpoch: Get<EpochId>;

        /// Weight information for extrinsics.
        type WeightInfo: WeightInfo;

        /// Origin construction for benchmarking (see [`BenchmarkHelper`]).
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin>;
    }

    /// 02 §7.3 (frozen): `Params: map ParamKey → ParamRecord`.
    ///
    /// The key set is genesis-fixed at ≤ [`MAX_PARAMS`] entries — no call
    /// inserts new keys (`set_param` updates existing records only), so the
    /// map is bounded by construction (I-21); `try_state` re-asserts it.
    #[pallet::storage]
    pub type Params<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ParamKey, ParamRecord, OptionQuery>;

    /// 02 §7.3 (frozen): `PhaseFlags: u32` bitset.
    ///
    /// Bit assignments (append-only): 0 shadow mode, 1 PARAM armed,
    /// 2 TREASURY armed, 3 CODE/META armed, 4 sudo present, 5 ledger frozen
    /// (PB-LEDGER-FREEZE), 6 dead-man engaged, 7 reserve-health flag;
    /// bits 8–31 reserved. Reserved bits are rejected on every write path.
    #[pallet::storage]
    pub type PhaseFlags<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// 02 §12 (frozen forever, D-14): the 168-byte fixed-layout release
    /// channel. SCALE for the wrapper is exactly the 168 raw bytes (no length
    /// prefix), so a metadata-less reader parses by offset. Writers are
    /// exhaustive: the execution guard via [`Pallet::note_release_channel`]
    /// and the `ConstitutionalValues` origin via
    /// [`Pallet::set_release_channel`].
    #[pallet::storage]
    pub type ReleaseChannel<T: Config> =
        StorageValue<_, ReleaseChannelValue, ValueQuery, DefaultReleaseChannel<T>>;

    /// Schema-1 zeroed default until the first genesis/write.
    #[pallet::type_value]
    pub fn DefaultReleaseChannel<T: Config>() -> ReleaseChannelValue {
        empty_release_channel()
    }

    /// Generic bounded-meter primitive (the constitution's half of I-7):
    /// empty at genesis — the I-17 envelope meters live with their owning
    /// pallets (treasury issuance/outflow, guard upgrade-spacing; 15 §1).
    /// Windows reset lazily per epoch on charge; refusals are strict no-ops.
    #[pallet::storage]
    pub type Meters<T: Config> =
        StorageValue<_, BoundedVec<Meter, ConstU32<MAX_METERS_BOUND>>, ValueQuery>;

    /// Capability table (06 §3.2 rows / §6.2): which proposal class may
    /// exercise which constitution-mediated capability. Consulted by the
    /// execution guard at dispatch (09 §1.2).
    #[pallet::storage]
    pub type Capabilities<T: Config> =
        StorageValue<_, BoundedVec<CapabilityRecord, ConstU32<MAX_CAPABILITIES_BOUND>>, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A 13 §1 key passed its bounds/Δ/cooldown checks and was updated.
        ParamUpdated { key: ParamKey, value: ParamValue },
        /// A capability-table row was inserted or replaced.
        CapabilitySet {
            class: ProposalClass,
            capability: Capability,
            enabled: bool,
        },
        /// A phase-flag bit was set or cleared.
        PhaseFlagSet { flag: u32, enabled: bool, bits: u32 },
        /// The D-14 release channel was rewritten.
        ReleaseChannelSet { spec_version: u32, updated_at: u32 },
        /// A registry row's governance metadata was amended (06 §2.1).
        RegistryAmended { key: ParamKey },
        /// A kernel meter was charged within its envelope.
        MeterCharged {
            index: u32,
            amount: u128,
            spent: u128,
        },
    }

    /// 1:1 with [`CoreError`]; `CoreError::BadOrigin` maps to
    /// `DispatchError::BadOrigin` instead (FRAME convention).
    #[pallet::error]
    pub enum Error<T> {
        /// No record exists under the given `ParamKey`.
        UnknownParam,
        /// No meter exists at the given index.
        UnknownMeter,
        /// Value kind does not match the record's typed kind.
        WrongType,
        /// Proposed value below the record's hard minimum (I-6).
        BelowMin,
        /// Proposed value above the record's hard maximum (I-6).
        AboveMax,
        /// Proposed step exceeds the record's max Δ/decision (I-6).
        DeltaTooLarge,
        /// The record's per-key cooldown has not elapsed (I-6).
        CooldownActive,
        /// Meter arithmetic overflow — rejected, never wrapped (G-1).
        MeterOverflow,
        /// Charge would exceed the meter's kernel envelope (I-7/I-17).
        MeterExhausted,
        /// Write touches a reserved `PhaseFlags` bit (02 §7.3).
        ReservedPhaseFlag,
        /// `set_phase_flag` touches a machinery bit outside the 09 §5.4
        /// sudo-armable set (bits 5–7 are sibling-pallet state).
        FlagNotArmable,
        /// Release-channel bytes violate the frozen schema-1 layout (02 §12).
        BadReleaseSchema,
        /// Params over the 13 §4 bound (genesis validation only).
        TooManyParams,
        /// Meters over the core bound (genesis validation only).
        TooManyMeters,
        /// Capability table full.
        TooManyCapabilities,
        /// `amend_registry` tried to move a kernel-bounded row's bounds
        /// (13 rule 7 — genesis-fixed).
        KernelBoundImmutable,
        /// `amend_registry` violates the compile-time meta-bounds
        /// (13 rule 2/7: `min ≤ value ≤ max`, kind-consistent, cooldown ≤ 8).
        MetaBoundViolation,
        /// Core state validator rejected the aggregate (try-state only).
        TryStateViolation,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        // 15 §1 try-state coverage rule: I-6 (bounds/Δ/cooldown shape),
        // the meter-primitive half of I-7 (spent ≤ limit), storage shape,
        // and the D-14 raw-key identity.
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// `constitution.set_param` — update one typed, bounded, rate-limited
        /// 13 §1 key (I-6).
        ///
        /// Authority matrix (06 §3.2): PARAM-class keys ⇒ `FutarchyParam`;
        /// TREASURY ⇒ `FutarchyTreasury`; META **and META+values** ⇒
        /// `FutarchyMeta` (06 §1 bars values from parameter keys; the values
        /// half of the dual consent is the guard's execute-time ratification,
        /// 06 §2.2 — PLAN SQ-6); CONST/entrenched ⇒ `ConstitutionalValues`.
        /// No Root path — 09 §5.4's bootstrap-sudo scope is exhaustive and
        /// excludes parameter administration (PLAN SQ-11).
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::set_param())]
        pub fn set_param(origin: OriginFor<T>, key: ParamKey, value: ParamValue) -> DispatchResult {
            let authority = T::GovernanceOrigin::ensure_origin(origin)?;
            let record = Params::<T>::get(key).ok_or(Error::<T>::UnknownParam)?;
            ensure!(
                authority.can_set_param(record.class),
                DispatchError::BadOrigin
            );
            let updated = record
                .checked_update(
                    value,
                    T::CurrentEpoch::get(),
                    frame_system::Pallet::<T>::block_number().unique_saturated_into(),
                )
                .map_err(Self::map_core_error)?;
            if let Some(pair) = constitution_core::gate_v_min_pair(key) {
                let paired = Params::<T>::get(pair).ok_or(Error::<T>::TryStateViolation)?;
                let (decision, gate) = if key.as_slice().starts_with(b"dec.") {
                    (updated.value, paired.value)
                } else {
                    (paired.value, updated.value)
                };
                match (decision, gate) {
                    (ParamValue::Balance(decision), ParamValue::Balance(gate)) => ensure!(
                        constitution_core::gate_v_min_coupled(decision, gate),
                        Error::<T>::TryStateViolation
                    ),
                    _ => return Err(Error::<T>::WrongType.into()),
                }
            }
            Params::<T>::insert(key, updated);
            Self::deposit_event(Event::ParamUpdated { key, value });
            Ok(())
        }

        /// `constitution.set_capability` — insert or replace one capability
        /// row (06 §3.2 row 4: a `FutarchyMeta` call; values participates via
        /// the rule-altering ratification of 06 §2.2, never direct dispatch).
        ///
        /// Mirrors `ConstitutionState::set_capability` over the bounded
        /// storage form (upsert by `(class, capability)`, bound
        /// [`MAX_CAPABILITIES`]); the differential test pins equivalence.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::set_capability())]
        pub fn set_capability(origin: OriginFor<T>, record: CapabilityRecord) -> DispatchResult {
            let authority = T::GovernanceOrigin::ensure_origin(origin)?;
            ensure!(authority.can_set_capability(), DispatchError::BadOrigin);
            Capabilities::<T>::try_mutate(|table| -> DispatchResult {
                if let Some(existing) = table
                    .iter_mut()
                    .find(|c| c.class == record.class && c.capability == record.capability)
                {
                    *existing = record;
                    return Ok(());
                }
                table
                    .try_push(record)
                    .map_err(|_| Error::<T>::TooManyCapabilities)?;
                Ok(())
            })?;
            Self::deposit_event(Event::CapabilitySet {
                class: record.class,
                capability: record.capability,
                enabled: record.enabled,
            });
            Ok(())
        }

        /// `constitution.set_phase_flag` — set/clear 02 §7.3 **arming** bits.
        ///
        /// Root-only and bit-scoped: the sole origin-mediated flag writer the
        /// spec names is bootstrap sudo, whose powers include "arming phase
        /// flags on evidence" (09 §5.4, Phases 0–3; the Phase-3→4 upgrade
        /// removes Root, after which arming bits move with phase-advancement
        /// upgrades, 09 §5.2). Only `PhaseFlagsValue::SUDO_ARMABLE_MASK`
        /// (bits 0–4) is writable here; the machinery bits — 5 ledger-frozen,
        /// 6 dead-man, 7 reserve-health — belong to sibling-pallet state and
        /// are reachable only through their dedicated internal setters, so
        /// even sudo cannot fake or clear a freeze/dead-man/reserve signal.
        /// Full per-bit writer map is PLAN SQ-5. Reserved bits 8–31 rejected.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::set_phase_flag())]
        pub fn set_phase_flag(origin: OriginFor<T>, flag: u32, enabled: bool) -> DispatchResult {
            let authority = T::GovernanceOrigin::ensure_origin(origin)?;
            ensure!(authority.can_set_phase_flag(), DispatchError::BadOrigin);
            ensure!(
                flag & !PhaseFlagsValue::SUDO_ARMABLE_MASK == 0,
                Error::<T>::FlagNotArmable
            );
            Self::write_phase_flag(flag, enabled)
        }

        /// `constitution.set_release_channel` — 02 §12 writer (b): the
        /// `ConstitutionalValues` origin rewrites the D-14 fixed layout on a
        /// canonical repoint, `min_supported_version` bump or key revocation.
        /// No other origin — including bootstrap Root — may dispatch this;
        /// writer (a) is the execution guard's [`Pallet::note_release_channel`].
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::set_release_channel())]
        pub fn set_release_channel(
            origin: OriginFor<T>,
            bytes: [u8; RELEASE_CHANNEL_LEN],
        ) -> DispatchResult {
            let authority = T::GovernanceOrigin::ensure_origin(origin)?;
            ensure!(
                authority.can_set_release_channel(),
                DispatchError::BadOrigin
            );
            Self::write_release_channel(bytes)
        }

        /// `constitution.amend_registry` — amend one key's governance
        /// metadata (bounds / max-Δ / cooldown), never its value, class or
        /// key set (06 §2.1 constitution track; 06 §3.2 row 4; 13 rule 2/7).
        ///
        /// Origins: `ConstitutionalValues` (constitution/entrenched tracks)
        /// or `FutarchyMeta` (META-amendable within meta-bounds).
        /// Kernel-bounded rows keep their bounds genesis-fixed; every
        /// amendment must keep `min ≤ value ≤ max`, preserve the value kind,
        /// and keep `cooldown ≤ 8` epochs. Registry rows are never inserted
        /// or removed on-chain — new keys arrive with runtime upgrades
        /// (13 §4: the key set is genesis-fixed).
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::amend_registry())]
        pub fn amend_registry(
            origin: OriginFor<T>,
            key: ParamKey,
            min: ParamValue,
            max: ParamValue,
            max_delta: Option<MaxDelta>,
            cooldown_epochs: u32,
        ) -> DispatchResult {
            let authority = T::GovernanceOrigin::ensure_origin(origin)?;
            ensure!(authority.can_amend_registry(), DispatchError::BadOrigin);
            let record = Params::<T>::get(key).ok_or(Error::<T>::UnknownParam)?;
            let amended = record
                .checked_amend(min, max, max_delta, cooldown_epochs)
                .map_err(Self::map_core_error)?;
            Params::<T>::insert(key, amended);
            Self::deposit_event(Event::RegistryAmended { key });
            Ok(())
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        /// 02 §2/§8: `INTEGRATION_CONTRACT_VERSION`, metadata-readable,
        /// canonical spelling per rule 5 (02 names byte-for-byte).
        #[pallet::constant_name(INTEGRATION_CONTRACT_VERSION)]
        fn integration_contract_version() -> u32 {
            CONTRACT_VERSION
        }
        /// 13 §4 bound on the genesis-fixed key set.
        #[pallet::constant_name(MaxParams)]
        fn max_params() -> u32 {
            MAX_PARAMS_BOUND
        }
        /// Core bound on the capability table.
        #[pallet::constant_name(MaxCapabilities)]
        fn max_capabilities() -> u32 {
            MAX_CAPABILITIES_BOUND
        }
        /// Core bound on the kernel meter set.
        #[pallet::constant_name(MaxMeters)]
        fn max_meters() -> u32 {
            MAX_METERS_BOUND
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Initial 02 §7.3 phase-flags bitset (reserved bits rejected).
        pub phase_flags: u32,
        /// Initial 168-byte D-14 release-channel value; empty ⇒ the schema-1
        /// zeroed default.
        pub release_channel: Vec<u8>,
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                phase_flags: 0,
                release_channel: Vec::new(),
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        // The parameter registry, meters and capability table are seeded from
        // the code-owned 13 §1 registry (`constitution-core` genesis fns), not
        // from the chain spec: 13's reading rules make the registry normative,
        // and a spec-injectable table would be a hardcode bypass (X-11e/h).
        // Only deployment-specific state (phase flags, release channel) is
        // configurable. Genesis-time `assert!` is the FRAME convention for
        // invalid chain specs — it runs before any block, so the G-1
        // status-quo rule for dispatch paths does not apply.
        fn build(&self) {
            for record in genesis_params() {
                Params::<T>::insert(record.key, record);
            }
            assert!(
                Params::<T>::count() as usize <= MAX_PARAMS,
                "constitution genesis: parameter registry over the 13 §4 bound"
            );

            let meters: BoundedVec<Meter, ConstU32<MAX_METERS_BOUND>> =
                BoundedVec::truncate_from(genesis_meters());
            assert!(
                meters.len() == genesis_meters().len(),
                "constitution genesis: meter set over the core bound"
            );
            Meters::<T>::put(meters);

            let capabilities: BoundedVec<CapabilityRecord, ConstU32<MAX_CAPABILITIES_BOUND>> =
                BoundedVec::truncate_from(genesis_capabilities());
            assert!(
                capabilities.len() == genesis_capabilities().len(),
                "constitution genesis: capability table over the core bound"
            );
            Capabilities::<T>::put(capabilities);

            let flags = PhaseFlagsValue::from_bits(self.phase_flags);
            assert!(
                flags.is_ok(),
                "constitution genesis: reserved PhaseFlags bits set (02 §7.3)"
            );
            if let Ok(flags) = flags {
                PhaseFlags::<T>::put(flags.bits());
            }

            if self.release_channel.is_empty() {
                ReleaseChannel::<T>::put(empty_release_channel());
            } else {
                assert!(
                    self.release_channel.len() == RELEASE_CHANNEL_LEN,
                    "constitution genesis: release channel must be exactly 168 bytes (02 §12)"
                );
                let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
                bytes.copy_from_slice(&self.release_channel);
                let channel = ReleaseChannelValue::new(bytes);
                assert!(
                    channel.is_ok(),
                    "constitution genesis: release channel violates the schema-1 layout (02 §12)"
                );
                if let Ok(channel) = channel {
                    ReleaseChannel::<T>::put(channel);
                }
            }

            assert!(
                Pallet::<T>::do_try_state().is_ok(),
                "constitution genesis violates I-6/I-7/I-17"
            );
        }
    }

    impl<T: Config> Pallet<T> {
        /// 02 §12 writer (a): the execution guard's runtime-internal write
        /// path (at `UpgradeAuthorized` and applied-upgrade detection).
        /// Not an extrinsic — reachable only as a Rust call from a sibling
        /// pallet inside the runtime (A11/B6 wire it); still validates the
        /// frozen layout.
        pub fn note_release_channel(bytes: [u8; RELEASE_CHANNEL_LEN]) -> DispatchResult {
            Self::write_release_channel(bytes)
        }

        /// Runtime-internal, bit-specific writers for the machinery-owned
        /// 02 §7.3 bits. Each sets exactly one bit, so no internal caller can
        /// reach the arming bits (0–4) or another pallet's signal — the
        /// per-writer discipline Codex's adversarial review asked for.
        /// Rust cannot authenticate the calling pallet; the runtime-level
        /// negative suite (S5/B1a) proves only the designated wiring calls
        /// each setter. Per-bit writer map: PLAN SQ-5.
        ///
        /// Bit 5 — PB-LEDGER-FREEZE mirror (guardian/ledger wiring, 06 §6.3).
        pub fn note_ledger_frozen(frozen: bool) -> DispatchResult {
            Self::write_phase_flag(PhaseFlagsValue::LEDGER_FROZEN, frozen)
        }

        /// Bit 6 — dead-man switch engaged (epoch/system wiring, 13 §2).
        pub fn note_dead_man_engaged(engaged: bool) -> DispatchResult {
            Self::write_phase_flag(PhaseFlagsValue::DEAD_MAN_ENGAGED, engaged)
        }

        /// Bit 7 — reserve-health flag `R` (oracle probe wiring, 07 §8).
        pub fn note_reserve_health(unhealthy: bool) -> DispatchResult {
            Self::write_phase_flag(PhaseFlagsValue::RESERVE_HEALTH_FLAG, unhealthy)
        }

        /// Runtime-internal I-7/I-17 meter charge for sibling pallets
        /// (treasury outflows, guard execute-path checks — A9/A11 wire it;
        /// meter identity/derivation is PLAN SQ-12). No spec document defines
        /// a `charge_meter` extrinsic, so none exists (06 §3.2 closed matrix).
        /// Windows reset lazily by epoch; spending is monotone in-window and
        /// the envelope is never exceeded (G-1: overflow rejects).
        pub fn charge_meter_internal(index: u32, amount: u128) -> DispatchResult {
            let spent = Meters::<T>::try_mutate(|meters| -> Result<u128, DispatchError> {
                let meter = meters
                    .get_mut(index as usize)
                    .ok_or(Error::<T>::UnknownMeter)?;
                meter
                    .charge(amount, T::CurrentEpoch::get())
                    .map_err(Self::map_core_error)?;
                Ok(meter.spent)
            })?;
            Self::deposit_event(Event::MeterCharged {
                index,
                amount,
                spent,
            });
            Ok(())
        }

        fn write_phase_flag(flag: u32, enabled: bool) -> DispatchResult {
            let mut flags =
                PhaseFlagsValue::from_bits(PhaseFlags::<T>::get()).map_err(Self::map_core_error)?;
            flags.set(flag, enabled).map_err(Self::map_core_error)?;
            PhaseFlags::<T>::put(flags.bits());
            Self::deposit_event(Event::PhaseFlagSet {
                flag,
                enabled,
                bits: flags.bits(),
            });
            Ok(())
        }

        /// Read helper for sibling pallets and views: the raw 02 §7.3 bitset.
        pub fn phase_flags() -> u32 {
            PhaseFlags::<T>::get()
        }

        /// Read helper: one typed parameter record, if the key exists.
        pub fn param(key: &ParamKey) -> Option<ParamRecord> {
            Params::<T>::get(key)
        }

        /// Read helper for the execution guard (09 §1.2): is a capability
        /// enabled for a class? Mirrors `ConstitutionState::capability_enabled`.
        pub fn capability_enabled(class: ProposalClass, capability: Capability) -> bool {
            Capabilities::<T>::get()
                .iter()
                .any(|c| c.class == class && c.capability == capability && c.enabled)
        }

        fn write_release_channel(bytes: [u8; RELEASE_CHANNEL_LEN]) -> DispatchResult {
            let channel = ReleaseChannelValue::new(bytes).map_err(Self::map_core_error)?;
            ReleaseChannel::<T>::put(channel);
            Self::deposit_event(Event::ReleaseChannelSet {
                spec_version: channel.spec_version(),
                updated_at: channel.updated_at(),
            });
            Ok(())
        }

        /// Rebuild the functional-core aggregate from storage and run its
        /// reviewed validator, plus the FRAME-side shape checks (15 §1).
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            let phase_flags = PhaseFlagsValue::from_bits(PhaseFlags::<T>::get())
                .map_err(|_| TryRuntimeError::Other("PhaseFlags: reserved bits set (02 §7.3)"))?;
            let mut params = Vec::new();
            for (key, record) in Params::<T>::iter() {
                if key != record.key {
                    return Err(TryRuntimeError::Other(
                        "Params: map key diverges from record key",
                    ));
                }
                params.push(record);
            }
            if params.len() != Params::<T>::count() as usize {
                return Err(TryRuntimeError::Other(
                    "Params: counter diverges from iterated entries",
                ));
            }
            let state = ConstitutionState {
                params,
                meters: Meters::<T>::get().into_inner(),
                capabilities: Capabilities::<T>::get().into_inner(),
                phase_flags,
                release_channel: ReleaseChannel::<T>::get(),
            };
            state.try_state().map_err(|_| {
                TryRuntimeError::Other("constitution core try_state failed (I-6/I-7/I-17)")
            })?;

            // D-14: the live raw key must equal
            // twox128("Constitution") ++ twox128("ReleaseChannel").
            let final_key =
                <ReleaseChannel<T> as frame_support::storage::generator::StorageValue<
                    ReleaseChannelValue,
                >>::storage_value_final_key();
            if final_key != RELEASE_CHANNEL_STORAGE_KEY {
                return Err(TryRuntimeError::Other(
                    "D-14 raw-key mismatch: the runtime must instance this pallet as `Constitution`",
                ));
            }
            Ok(())
        }

        pub(crate) fn map_core_error(err: CoreError) -> DispatchError {
            match err {
                CoreError::UnknownParam => Error::<T>::UnknownParam.into(),
                CoreError::UnknownMeter => Error::<T>::UnknownMeter.into(),
                CoreError::WrongType => Error::<T>::WrongType.into(),
                CoreError::BelowMin => Error::<T>::BelowMin.into(),
                CoreError::AboveMax => Error::<T>::AboveMax.into(),
                CoreError::DeltaTooLarge => Error::<T>::DeltaTooLarge.into(),
                CoreError::CooldownActive => Error::<T>::CooldownActive.into(),
                CoreError::MeterOverflow => Error::<T>::MeterOverflow.into(),
                CoreError::MeterExhausted => Error::<T>::MeterExhausted.into(),
                CoreError::ReservedPhaseFlag => Error::<T>::ReservedPhaseFlag.into(),
                CoreError::FlagNotArmable => Error::<T>::FlagNotArmable.into(),
                CoreError::KernelBoundImmutable => Error::<T>::KernelBoundImmutable.into(),
                CoreError::MetaBoundViolation => Error::<T>::MetaBoundViolation.into(),
                CoreError::BadReleaseSchema => Error::<T>::BadReleaseSchema.into(),
                CoreError::TooManyParams => Error::<T>::TooManyParams.into(),
                CoreError::TooManyMeters => Error::<T>::TooManyMeters.into(),
                CoreError::TooManyCapabilities => Error::<T>::TooManyCapabilities.into(),
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::TryStateViolation => Error::<T>::TryStateViolation.into(),
            }
        }
    }
}
