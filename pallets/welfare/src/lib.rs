#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-welfare` — welfare snapshots and cohort settlement (A7)
//!
//! Production FRAME shell over the frame-free [`welfare_core`] functional
//! core. The pallet owns the bounded runtime storage and authority seams while
//! delegating all welfare arithmetic and validation to the core.
//!
//! Spec: `docs/architecture/05` (§4 welfare/gates, §6 the single settlement
//! path, §7 cohorts), `02 §4/§7.4` (view and frozen storage names), `06 §3.2`
//! (metric authority), `13 §1/§4` (live parameters and bounds), and `15 §1/§4`
//! (try-state and differential verification).
//!
//! `Snapshots`, `MetricSpecs`, and `GateBreachFlags` are separate bounded maps
//! because 02 §7.4 freezes those frontend-readable names and key/value shapes.
//! Each transition is load → core operation → checked conversion → replace the
//! bounded pre-image keys with the post-image keys → drain core events.
//!
//! SQ-80: 06 §3.2 names `welfare.activate_spec`, but the functional core has no
//! separate activation state: registration enforces `activation_epoch >= now+2`
//! and activation is implicit. A separate call is deferred until the spec and
//! core define an activation-state transition.

extern crate alloc;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use frame_support::pallet_prelude::DispatchResult;
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    EpochId, FixedU64, MetricSpecVersion, ProposalId,
};

pub use welfare_core::{
    ComponentValue, Error as CoreError, Event as CoreEvent, GateBreachFlags as CoreGateBreachFlags,
    MetricSpec, Pillar, Snapshot as CoreSnapshot, SourceClass, WelfareParams as CoreWelfareParams,
    WelfareState, EPSILON, EPSILON_PILLAR, HISTORY_PRIORS, MAX_COMPONENTS_PER_SPEC,
    MAX_DAILY_GATE_SAMPLES, MAX_GATE_FLAGS, MAX_METRIC_SPECS, MAX_SNAPSHOTS, ONE, THETA_C_HI,
    THETA_C_LO, THETA_S_HI, THETA_S_LO, W_A, W_P,
};

/// Core bounds in the `u32` form required by FRAME's `ConstU32`.
pub const MAX_METRIC_SPECS_BOUND: u32 = MAX_METRIC_SPECS as u32;
pub const MAX_SNAPSHOTS_BOUND: u32 = MAX_SNAPSHOTS as u32;
pub const MAX_GATE_FLAGS_BOUND: u32 = MAX_GATE_FLAGS as u32;
pub const MAX_COMPONENTS_PER_SPEC_BOUND: u32 = MAX_COMPONENTS_PER_SPEC as u32;

/// Live 13 §1 welfare tunables. B1a implements this provider over
/// `pallet-constitution::Params`; tests use overridable parameter statics.
pub trait WelfareParamsProvider {
    fn theta_s_lo() -> FixedU64;
    fn theta_s_hi() -> FixedU64;
    fn theta_c_lo() -> FixedU64;
    fn theta_c_hi() -> FixedU64;
    fn w_p() -> FixedU64;
    fn w_a() -> FixedU64;

    fn welfare_params() -> CoreWelfareParams {
        CoreWelfareParams {
            theta_s_lo: Self::theta_s_lo(),
            theta_s_hi: Self::theta_s_hi(),
            theta_c_lo: Self::theta_c_lo(),
            theta_c_hi: Self::theta_c_hi(),
            w_p: Self::w_p(),
            w_a: Self::w_a(),
        }
    }
}

/// Injected normalized metric source. Normalization, missing-data treatment,
/// raw counter mapping, and attestation plumbing are runtime-composition work;
/// this pallet aggregates only already-normalized `[0, 1]` components.
pub trait MetricInputs {
    fn onchain_components(epoch: EpochId, spec_version: MetricSpecVersion) -> Vec<ComponentValue>;
    fn incident_multiplier(epoch: EpochId) -> FixedU64;
    fn daily_components(
        epoch: EpochId,
        day: u8,
        spec_version: MetricSpecVersion,
    ) -> Vec<ComponentValue>;
}

/// Gate-market dimension settled through the conditional ledger seam.
#[derive(
    Clone, Copy, Debug, parity_scale_codec::Decode, parity_scale_codec::Encode, PartialEq, Eq,
)]
pub enum GateKind {
    Survival,
    Security,
}

/// Runtime-injected conditional-ledger settlement endpoint.
pub trait LedgerSettlement {
    fn settle_scalar(pid: ProposalId, score: FixedU64) -> DispatchResult;
    fn settle_gate(pid: ProposalId, gate: GateKind, breached: bool) -> DispatchResult;
    fn settle_baseline(epoch: EpochId, score: FixedU64) -> DispatchResult;
}

/// The cohort whose computed score is being dispatched to the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleTarget {
    Proposal {
        pid: ProposalId,
        has_gate_books: bool,
    },
    Baseline,
}

/// Maps the benchmark's governance call to an admitted runtime origin.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    fn metric_governance_origin() -> RuntimeOrigin;
    /// Advance the configured clock so `epoch` is finalized before a keeper
    /// crank. Runtime implementations inject the real epoch storage state.
    fn prime_finalized_epoch(epoch: EpochId);
    /// Populate every component the active benchmark MetricSpec reads.
    fn prime_metric_inputs(count: u16);
    fn prime_keeper_rebate() {}
    fn assert_keeper_rebate_paid(_: futarchy_primitives::keeper::CrankClass) {}
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::EnsureOrigin;
    use frame_system::pallet_prelude::*;
    use sp_runtime::TryRuntimeError;

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        /// ConstitutionalValues / metric-track authority (06 §3.2).
        type MetricGovernanceOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// Live welfare values from constitution Params (rule 4).
        type Params: WelfareParamsProvider;
        /// Normalized epoch and daily component inputs.
        type MetricInputs: MetricInputs;
        /// Conditional-ledger settlement seam used only by `compute_settlement`.
        type Ledger: LedgerSettlement;
        /// Current epoch clock used by metric registration.
        type CurrentEpoch: Get<EpochId>;
        /// Fail-soft keeper rebate endpoint (08 §6.3).
        type KeeperRebate: KeeperRebateSink<Self::AccountId>;
        /// Weight information for all extrinsics.
        type WeightInfo: WeightInfo;
        /// Admitted origin construction for benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin>;
    }

    pub type BoundedComponents =
        BoundedVec<ComponentValue, ConstU32<MAX_COMPONENTS_PER_SPEC_BOUND>>;
    pub type BoundedSpecSet = BoundedVec<MetricSpec, ConstU32<MAX_COMPONENTS_PER_SPEC_BOUND>>;
    type CheckedStorage = (
        Vec<(MetricSpecVersion, BoundedSpecSet)>,
        Vec<((EpochId, MetricSpecVersion), StoredSnapshot)>,
        Vec<(EpochId, CoreGateBreachFlags)>,
    );
    /// Bounded mirror of the core snapshot, whose transient component `Vec`
    /// cannot itself implement `MaxEncodedLen`.
    #[derive(
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub struct StoredSnapshot {
        pub epoch: EpochId,
        pub spec_version: MetricSpecVersion,
        pub s_pillar: FixedU64,
        pub c_onchain: FixedU64,
        pub c_attested: FixedU64,
        pub p_pillar: FixedU64,
        pub a_pillar: FixedU64,
        pub gate_s: FixedU64,
        pub gate_c: FixedU64,
        pub welfare: FixedU64,
        pub components: BoundedComponents,
    }

    impl TryFrom<CoreSnapshot> for StoredSnapshot {
        type Error = CoreError;

        fn try_from(s: CoreSnapshot) -> Result<Self, Self::Error> {
            Ok(Self {
                epoch: s.epoch,
                spec_version: s.spec_version,
                s_pillar: s.s_pillar,
                c_onchain: s.c_onchain,
                c_attested: s.c_attested,
                p_pillar: s.p_pillar,
                a_pillar: s.a_pillar,
                gate_s: s.gate_s,
                gate_c: s.gate_c,
                welfare: s.welfare,
                components: BoundedVec::try_from(s.components)
                    .map_err(|_| CoreError::TooManyComponents)?,
            })
        }
    }

    impl From<StoredSnapshot> for CoreSnapshot {
        fn from(s: StoredSnapshot) -> Self {
            Self {
                epoch: s.epoch,
                spec_version: s.spec_version,
                s_pillar: s.s_pillar,
                c_onchain: s.c_onchain,
                c_attested: s.c_attested,
                p_pillar: s.p_pillar,
                a_pillar: s.a_pillar,
                gate_s: s.gate_s,
                gate_c: s.gate_c,
                welfare: s.welfare,
                components: s.components.into_inner(),
            }
        }
    }

    /// Frozen 02 §7.4 frontend surface: versioned metric definitions.
    #[pallet::storage]
    pub type MetricSpecs<T: Config> =
        StorageMap<_, Blake2_128Concat, MetricSpecVersion, BoundedSpecSet, OptionQuery>;

    /// Frozen 02 §7.4 frontend surface: bounded settlement snapshots.
    #[pallet::storage]
    pub type Snapshots<T: Config> =
        StorageMap<_, Blake2_128Concat, (EpochId, MetricSpecVersion), StoredSnapshot, OptionQuery>;

    /// Frozen 02 §7.4 frontend surface: daily breach outcomes by epoch.
    #[pallet::storage]
    pub type GateBreachFlags<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, CoreGateBreachFlags, OptionQuery>;

    /// Pallet-internal marker for successfully sampled daily gates.
    ///
    /// This is deliberately separate from the frozen `GateBreachFlags` surface:
    /// 02 §7.4 names only `Snapshots`, `MetricSpecs`, and `GateBreachFlags`, and
    /// 05 §4.7 requires the latter's bitmap to identify breached days only.
    /// The auxiliary map is bounded and pruned in lockstep with gate history.
    #[pallet::storage]
    pub type SampledGateDays<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, [u32; 2], OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        MetricSpecRegistered {
            version: MetricSpecVersion,
        },
        SnapshotRecorded {
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            welfare: FixedU64,
        },
        GateBreachRecorded {
            epoch: EpochId,
            day: u8,
            s_breached: bool,
            c_breached: bool,
        },
        SettlementComputed {
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            score: FixedU64,
        },
    }

    /// Core errors map 1:1; `BadParams` identifies an invalid live registry
    /// value before the core operation begins.
    #[pallet::error]
    pub enum Error<T> {
        TooManyMetricSpecs,
        TooManySnapshots,
        TooManyComponents,
        TooManyGateFlags,
        DuplicateSpecVersion,
        SpecNotFound,
        BadActivationEpoch,
        SpecNotActive,
        MissingMetricDiscipline,
        BadEpsilonFloor,
        BadSourceClass,
        BadWeightSum,
        ValueOutOfRange,
        MissingComponent,
        DuplicateComponent,
        DuplicateSnapshot,
        ArithmeticOverflow,
        TryStateViolation,
        BadParams,
        /// A snapshot/daily-gate crank named an epoch that has not finalized yet
        /// (`epoch >= CurrentEpoch`). 05 §4.6 winsorizes over *finalized* epoch
        /// values, so a keeper may only record an epoch the clock has passed.
        EpochNotFinalized,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Register a metric-track-approved version. Activation is implicit and
        /// the core enforces the two-epoch lead time.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register_spec())]
        pub fn register_spec(
            origin: OriginFor<T>,
            version: MetricSpecVersion,
            specs: BoundedSpecSet,
        ) -> DispatchResult {
            T::MetricGovernanceOrigin::ensure_origin(origin)?;
            Self::mutate(|state| {
                state.register_metric_spec(T::CurrentEpoch::get(), version, specs.into_inner())
            })
        }

        /// Permissionless signed keeper crank for one **finalized** epoch's
        /// snapshot. The epoch must have closed (`epoch < CurrentEpoch`; 05 §4.6
        /// winsorizes over finalized epoch values), else the crank is rejected —
        /// this stops an early/future call from locking a wrong `W` or consuming
        /// the bounded snapshot window before the real counters exist.
        #[pallet::call_index(1)]
        // B5: recalibrate for the keeper-rebate sink's additional storage path.
        #[pallet::weight(T::WeightInfo::record_snapshot())]
        pub fn record_snapshot(
            origin: OriginFor<T>,
            epoch: EpochId,
            spec_version: MetricSpecVersion,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            frame_support::ensure!(
                epoch < T::CurrentEpoch::get(),
                Error::<T>::EpochNotFinalized
            );
            let components = T::MetricInputs::onchain_components(epoch, spec_version);
            let incident = T::MetricInputs::incident_multiplier(epoch);
            let params = Self::live_params()?;
            Self::mutate(|state| {
                state
                    .record_snapshot(epoch, spec_version, components, incident, &params)
                    .map(|_| ())
            })?;
            T::KeeperRebate::rebate(&who, CrankClass::DecisionCritical);
            Ok(())
        }

        /// Permissionless signed keeper crank for a **finalized** epoch's daily
        /// S/C gate sample. Like `record_snapshot`, the epoch must have closed
        /// (`epoch < CurrentEpoch`) so the day's counters are final (05 §4.7).
        #[pallet::call_index(2)]
        // B5: recalibrate for the keeper-rebate sink's additional storage path.
        #[pallet::weight(T::WeightInfo::record_daily_gate())]
        pub fn record_daily_gate(
            origin: OriginFor<T>,
            epoch: EpochId,
            day: u8,
            spec_version: MetricSpecVersion,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            frame_support::ensure!(
                epoch < T::CurrentEpoch::get(),
                Error::<T>::EpochNotFinalized
            );
            let components = T::MetricInputs::daily_components(epoch, day, spec_version);
            let params = Self::live_params()?;
            frame_support::ensure!(day < MAX_DAILY_GATE_SAMPLES, Error::<T>::ValueOutOfRange);
            let word_index = usize::from(day / 32);
            let bit = 1u32 << (day % 32);
            let mut sampled_days = SampledGateDays::<T>::get(epoch).unwrap_or([0; 2]);
            let sampled_word = sampled_days
                .get_mut(word_index)
                .ok_or(Error::<T>::ValueOutOfRange)?;
            let newly_sampled = *sampled_word & bit == 0;
            *sampled_word |= bit;
            let mut new_breach_flags = false;
            Self::mutate(|state| {
                state
                    .record_daily_gate(epoch, day, spec_version, components, &params)
                    .map(|(_, did_change)| new_breach_flags = did_change)
            })?;
            SampledGateDays::<T>::insert(epoch, sampled_days);
            if newly_sampled || new_breach_flags {
                T::KeeperRebate::rebate(&who, CrankClass::General);
            }
            Ok(())
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        #[pallet::constant_name(INTEGRATION_CONTRACT_VERSION)]
        fn integration_contract_version() -> u32 {
            futarchy_primitives::INTEGRATION_CONTRACT_VERSION
        }

        #[pallet::constant_name(MaxMetricSpecs)]
        fn max_metric_specs() -> u32 {
            MAX_METRIC_SPECS_BOUND
        }

        #[pallet::constant_name(MaxSnapshots)]
        fn max_snapshots() -> u32 {
            MAX_SNAPSHOTS_BOUND
        }

        #[pallet::constant_name(MaxGateFlags)]
        fn max_gate_flags() -> u32 {
            MAX_GATE_FLAGS_BOUND
        }

        #[pallet::constant_name(MaxDailyGateSamples)]
        fn max_daily_gate_samples() -> u8 {
            MAX_DAILY_GATE_SAMPLES
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub specs: Vec<(MetricSpecVersion, Vec<MetricSpec>)>,
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                specs: Vec::new(),
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            let mut state = WelfareState::new();
            for (version, specs) in &self.specs {
                assert!(
                    state
                        .register_metric_spec(0, *version, specs.clone())
                        .is_ok(),
                    "welfare genesis metric specs violate core validation"
                );
            }
            assert!(
                state.try_state().is_ok(),
                "welfare genesis violates bounded core invariants"
            );
            for (version, specs) in state.specs {
                MetricSpecs::<T>::insert(version, BoundedVec::truncate_from(specs));
            }
        }
    }

    impl<T: Config> Pallet<T> {
        /// The one 05 §6 settlement endpoint. It is runtime-internal (not a
        /// call); B1a exposes it only through `pallet-epoch::settle_cohort`.
        // B1a: the SettleAuthority-trusted epoch caller supplies the proposal's
        // creation-time `spec_version` (Proposal.metric_spec, I-16) and whether
        // its class/ask created gate books.
        pub fn compute_settlement(
            cohort_epoch: EpochId,
            spec_version: MetricSpecVersion,
            target: SettleTarget,
        ) -> DispatchResult {
            let mut state = Self::load();
            let score = state
                .compute_settlement(cohort_epoch, spec_version)
                .map_err(Self::map_core_error)?;
            let gate_outcomes = match target {
                SettleTarget::Proposal {
                    has_gate_books: true,
                    ..
                } => Some(Self::gate_outcomes(cohort_epoch)?),
                _ => None,
            };

            frame_support::storage::with_storage_layer(|| {
                match target {
                    SettleTarget::Proposal {
                        pid,
                        has_gate_books,
                    } => {
                        T::Ledger::settle_scalar(pid, score)?;
                        if has_gate_books {
                            let (s_breached, c_breached) = gate_outcomes
                                .ok_or(DispatchError::Other("missing gate outcomes"))?;
                            T::Ledger::settle_gate(pid, GateKind::Survival, s_breached)?;
                            T::Ledger::settle_gate(pid, GateKind::Security, c_breached)?;
                        }
                    }
                    SettleTarget::Baseline => {
                        T::Ledger::settle_baseline(cohort_epoch, score)?;
                    }
                }
                Self::deposit_core_event(CoreEvent::SettlementComputed {
                    epoch: cohort_epoch,
                    spec_version,
                    score,
                });
                Ok(())
            })
        }

        /// Runtime-internal rolling-window maintenance. B1a wires this from
        /// epoch Housekeeping only after the cohort reap precondition in 05 §3.3.
        pub fn prune(cutoff_epoch: EpochId) -> DispatchResult {
            Self::mutate(|state| {
                state.prune_before(cutoff_epoch);
                Ok(())
            })?;
            for epoch in SampledGateDays::<T>::iter_keys()
                .filter(|epoch| *epoch < cutoff_epoch)
                .collect::<Vec<_>>()
            {
                SampledGateDays::<T>::remove(epoch);
            }
            Ok(())
        }

        /// Full core state rebuilt from the three frozen storage mirrors.
        pub fn welfare_state() -> WelfareState {
            Self::load()
        }

        /// Seed a checked core state for tests and worst-case benchmarks.
        #[cfg(any(test, feature = "runtime-benchmarks"))]
        pub fn seed(state: &WelfareState) -> DispatchResult {
            let mut state = state.clone();
            state.events.clear();
            let pre = Self::load();
            Self::persist(&pre, state)
        }

        fn live_params() -> Result<CoreWelfareParams, DispatchError> {
            let params = T::Params::welfare_params();
            params
                .validate()
                .map_err(|_| DispatchError::from(Error::<T>::BadParams))?;
            Ok(params)
        }

        fn gate_outcomes(cohort_epoch: EpochId) -> Result<(bool, bool), DispatchError> {
            let state = Self::load();
            let first_epoch = cohort_epoch
                .checked_add(1)
                .ok_or_else(|| Self::map_core_error(CoreError::ArithmeticOverflow))?;
            let second_epoch = cohort_epoch
                .checked_add(2)
                .ok_or_else(|| Self::map_core_error(CoreError::ArithmeticOverflow))?;
            // SQ-79: absent daily observations deterministically read as no
            // breach; whether G-1 requires a pessimistic default is deferred.
            let first = state.gate_breach(first_epoch);
            let second = state.gate_breach(second_epoch);
            Ok((
                first.s_breached || second.s_breached,
                first.c_breached || second.c_breached,
            ))
        }

        fn load() -> WelfareState {
            let mut specs = MetricSpecs::<T>::iter()
                .map(|(version, specs)| (version, specs.into_inner()))
                .collect::<Vec<_>>();
            specs.sort_by_key(|(version, _)| *version);
            let mut snapshots = Snapshots::<T>::iter()
                .map(|(_, snapshot)| CoreSnapshot::from(snapshot))
                .collect::<Vec<_>>();
            snapshots.sort_by_key(|snapshot| (snapshot.epoch, snapshot.spec_version));
            let mut gate_flags = GateBreachFlags::<T>::iter().collect::<Vec<_>>();
            gate_flags.sort_by_key(|(epoch, _)| *epoch);
            WelfareState {
                specs,
                snapshots,
                gate_flags,
                events: Vec::new(),
            }
        }

        fn mutate(op: impl FnOnce(&mut WelfareState) -> Result<(), CoreError>) -> DispatchResult {
            let pre = Self::load();
            let mut post = pre.clone();
            op(&mut post).map_err(Self::map_core_error)?;
            Self::persist(&pre, post)
        }

        fn persist(pre: &WelfareState, post: WelfareState) -> DispatchResult {
            let (specs, snapshots, gate_flags) = Self::checked_storage(&post)?;

            for (version, _) in &pre.specs {
                MetricSpecs::<T>::remove(version);
            }
            for snapshot in &pre.snapshots {
                Snapshots::<T>::remove((snapshot.epoch, snapshot.spec_version));
            }
            for (epoch, _) in &pre.gate_flags {
                GateBreachFlags::<T>::remove(epoch);
            }
            for (version, spec_set) in specs {
                MetricSpecs::<T>::insert(version, spec_set);
            }
            for (key, snapshot) in snapshots {
                Snapshots::<T>::insert(key, snapshot);
            }
            for (epoch, flags) in gate_flags {
                GateBreachFlags::<T>::insert(epoch, flags);
            }
            for event in post.events {
                Self::deposit_core_event(event);
            }
            Ok(())
        }

        fn checked_storage(state: &WelfareState) -> Result<CheckedStorage, DispatchError> {
            if state.specs.len() > MAX_METRIC_SPECS {
                return Err(Error::<T>::TooManyMetricSpecs.into());
            }
            if state.snapshots.len() > MAX_SNAPSHOTS {
                return Err(Error::<T>::TooManySnapshots.into());
            }
            if state.gate_flags.len() > MAX_GATE_FLAGS {
                return Err(Error::<T>::TooManyGateFlags.into());
            }
            let specs = state
                .specs
                .iter()
                .map(|(version, specs)| {
                    BoundedVec::try_from(specs.clone())
                        .map(|specs| (*version, specs))
                        .map_err(|_| Error::<T>::TooManyComponents.into())
                })
                .collect::<Result<Vec<_>, DispatchError>>()?;
            let snapshots = state
                .snapshots
                .iter()
                .cloned()
                .map(StoredSnapshot::try_from)
                .collect::<Result<Vec<_>, CoreError>>()
                .map_err(Self::map_core_error)?;
            let snapshots = snapshots
                .into_iter()
                .map(|snapshot| ((snapshot.epoch, snapshot.spec_version), snapshot))
                .collect();
            let gate_flags = state.gate_flags.clone();
            Ok((specs, snapshots, gate_flags))
        }

        fn deposit_core_event(event: CoreEvent) {
            let event = match event {
                CoreEvent::MetricSpecRegistered { version } => {
                    Event::MetricSpecRegistered { version }
                }
                CoreEvent::SnapshotRecorded {
                    epoch,
                    spec_version,
                    welfare,
                } => Event::SnapshotRecorded {
                    epoch,
                    spec_version,
                    welfare,
                },
                CoreEvent::GateBreachRecorded {
                    epoch,
                    day,
                    s_breached,
                    c_breached,
                } => Event::GateBreachRecorded {
                    epoch,
                    day,
                    s_breached,
                    c_breached,
                },
                CoreEvent::SettlementComputed {
                    epoch,
                    spec_version,
                    score,
                } => Event::SettlementComputed {
                    epoch,
                    spec_version,
                    score,
                },
            };
            Self::deposit_event(event);
        }

        /// Rebuild and validate the core plus every map key/value invariant.
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            let state = Self::load();
            state.try_state().map_err(|_| {
                TryRuntimeError::Other("welfare core try_state failed (I-16/bounds)")
            })?;
            T::Params::welfare_params().validate().map_err(|_| {
                TryRuntimeError::Other("welfare live Params violate kernel floors or weight sum")
            })?;
            if MetricSpecs::<T>::iter().count() > MAX_METRIC_SPECS
                || Snapshots::<T>::iter().count() > MAX_SNAPSHOTS
                || GateBreachFlags::<T>::iter().count() > MAX_GATE_FLAGS
                || SampledGateDays::<T>::iter().count() > MAX_GATE_FLAGS
            {
                return Err(TryRuntimeError::Other(
                    "welfare map entry count exceeds its core bound",
                ));
            }
            for (version, specs) in MetricSpecs::<T>::iter() {
                if specs.iter().any(|spec| spec.version != version) {
                    return Err(TryRuntimeError::Other(
                        "welfare metric-spec map key does not match its value",
                    ));
                }
            }
            for (key, snapshot) in Snapshots::<T>::iter() {
                if key != (snapshot.epoch, snapshot.spec_version) {
                    return Err(TryRuntimeError::Other(
                        "welfare snapshot map key does not match its value",
                    ));
                }
                StoredSnapshot::try_from(CoreSnapshot::from(snapshot)).map_err(|_| {
                    TryRuntimeError::Other("welfare snapshot violates its component bound")
                })?;
            }
            for epoch in SampledGateDays::<T>::iter_keys() {
                if !GateBreachFlags::<T>::contains_key(epoch) {
                    return Err(TryRuntimeError::Other(
                        "welfare sampled-gate marker has no corresponding gate record",
                    ));
                }
            }
            Ok(())
        }

        pub(crate) fn map_core_error(error: CoreError) -> DispatchError {
            match error {
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::TooManyMetricSpecs => Error::<T>::TooManyMetricSpecs.into(),
                CoreError::TooManySnapshots => Error::<T>::TooManySnapshots.into(),
                CoreError::TooManyComponents => Error::<T>::TooManyComponents.into(),
                CoreError::TooManyGateFlags => Error::<T>::TooManyGateFlags.into(),
                CoreError::DuplicateSpecVersion => Error::<T>::DuplicateSpecVersion.into(),
                CoreError::SpecNotFound => Error::<T>::SpecNotFound.into(),
                CoreError::BadActivationEpoch => Error::<T>::BadActivationEpoch.into(),
                CoreError::SpecNotActive => Error::<T>::SpecNotActive.into(),
                CoreError::MissingMetricDiscipline => Error::<T>::MissingMetricDiscipline.into(),
                CoreError::BadEpsilonFloor => Error::<T>::BadEpsilonFloor.into(),
                CoreError::BadSourceClass => Error::<T>::BadSourceClass.into(),
                CoreError::BadWeightSum => Error::<T>::BadWeightSum.into(),
                CoreError::ValueOutOfRange => Error::<T>::ValueOutOfRange.into(),
                CoreError::MissingComponent => Error::<T>::MissingComponent.into(),
                CoreError::DuplicateComponent => Error::<T>::DuplicateComponent.into(),
                CoreError::DuplicateSnapshot => Error::<T>::DuplicateSnapshot.into(),
                CoreError::ArithmeticOverflow => Error::<T>::ArithmeticOverflow.into(),
                CoreError::TryStateViolation => Error::<T>::TryStateViolation.into(),
            }
        }
    }
}
