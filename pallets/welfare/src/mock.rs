//! Mock runtime for `pallet-welfare` (15 §4.1).

use crate as pallet_welfare;
use crate::{
    ComponentValue, GateKind, LedgerSettlement, MetricInputs, MetricSpec, Pillar, SourceClass,
    WelfareParamsProvider, EPSILON_PILLAR, HISTORY_PRIORS, ONE, THETA_C_HI as CORE_THETA_C_HI,
    THETA_C_LO as CORE_THETA_C_LO, THETA_S_HI as CORE_THETA_S_HI, THETA_S_LO as CORE_THETA_S_LO,
    W_A as CORE_W_A, W_P as CORE_W_P,
};
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    EpochId, FixedU64, MetricSpecVersion, ProposalId,
};
use parity_scale_codec::{Decode, Encode};
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Welfare: pallet_welfare,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
}

pub fn governance_acc() -> AccountId32 {
    AccountId32::new([2u8; 32])
}

pub fn keeper() -> AccountId32 {
    AccountId32::new([3u8; 32])
}

pub fn nobody() -> AccountId32 {
    AccountId32::new([99u8; 32])
}

pub fn metric_spec(id: u16, pillar: Pillar, weight: u64, version: u16) -> MetricSpec {
    let source = match pillar {
        Pillar::CAttested | Pillar::A => SourceClass::Attested,
        Pillar::S | Pillar::COnchain | Pillar::P => SourceClass::Onchain,
    };
    MetricSpec {
        id,
        version,
        pillar,
        weight: FixedU64(weight),
        epsilon_floor: EPSILON_PILLAR,
        activation_epoch: 2,
        source,
        formula_ref: [1; 32],
        units: [2; 16],
        repr: [3; 16],
        cadence_blocks: 1,
        sanity_min: FixedU64(0),
        sanity_max: FixedU64(ONE),
        has_normalization_rule: true,
        has_missing_data_rule: true,
        has_gaming_vectors: true,
        has_challenge_procedure: true,
        prior_bounds: [FixedU64(ONE); HISTORY_PRIORS],
    }
}

pub fn default_specs(version: u16) -> Vec<MetricSpec> {
    vec![
        metric_spec(1, Pillar::S, ONE, version),
        metric_spec(2, Pillar::COnchain, ONE, version),
        metric_spec(3, Pillar::P, ONE, version),
        metric_spec(4, Pillar::A, ONE, version),
    ]
}

/// `default_specs` with an explicit activation epoch (post-genesis registrations
/// must clear the `current + 2` lead; genesis specs activate at epoch 1).
pub fn specs_activating(version: u16, activation: EpochId) -> Vec<MetricSpec> {
    default_specs(version)
        .into_iter()
        .map(|spec| MetricSpec {
            activation_epoch: activation,
            ..spec
        })
        .collect()
}

/// Genesis MetricSpecs activate at epoch 1 (05 §4.6 cold start): welfare is
/// computable from epoch 1, so the genesis spec carries no lead time.
pub fn genesis_specs(version: u16) -> Vec<MetricSpec> {
    specs_activating(version, 1)
}

/// A finalized-epoch horizon for the ext-builder default: high enough that every
/// epoch the snapshot/gate tests record is already finalized (`epoch < NOW`), so
/// the pallet's `EpochNotFinalized` guard is a no-op unless a test sets the clock
/// itself. Registration tests that assert the two-epoch lead pin the clock lower.
pub const FINALIZED_NOW: EpochId = 1_000;

pub fn healthy_components() -> Vec<ComponentValue> {
    (1..=4)
        .map(|id| ComponentValue {
            id,
            value: FixedU64(ONE),
        })
        .collect()
}

#[derive(Clone, Debug, Decode, Encode, PartialEq, Eq)]
pub enum LedgerCall {
    Scalar(ProposalId, FixedU64),
    Gate(ProposalId, GateKind, bool),
    Baseline(EpochId, FixedU64),
}

parameter_types! {
    pub static CurrentEpochValue: EpochId = 0;
    pub static ThetaSLo: FixedU64 = CORE_THETA_S_LO;
    pub static ThetaSHi: FixedU64 = CORE_THETA_S_HI;
    pub static ThetaCLo: FixedU64 = CORE_THETA_C_LO;
    pub static ThetaCHi: FixedU64 = CORE_THETA_C_HI;
    pub static WP: FixedU64 = CORE_W_P;
    pub static WA: FixedU64 = CORE_W_A;
    pub static OnchainInput: Vec<ComponentValue> = healthy_components();
    pub static OnchainInputsByVersion: Vec<(MetricSpecVersion, Vec<ComponentValue>)> = Vec::new();
    pub static DailyInput: Vec<ComponentValue> = healthy_components();
    pub static DailyInputsByVersion: Vec<(MetricSpecVersion, Vec<ComponentValue>)> = Vec::new();
    pub static IncidentInput: FixedU64 = FixedU64(ONE);
    pub static LedgerFailure: Option<LedgerCall> = None;
    pub static RecordKeeperRebates: bool = false;
}

pub struct KeeperRebates;

impl KeeperRebates {
    const KEY: &'static [u8] = b":test:welfare:keeper-rebates";

    pub fn get() -> Vec<(AccountId32, CrankClass)> {
        sp_io::storage::get(Self::KEY)
            .and_then(|encoded| {
                let mut input: &[u8] = encoded.as_ref();
                Vec::<(AccountId32, CrankClass)>::decode(&mut input).ok()
            })
            .unwrap_or_default()
    }

    fn push(who: AccountId32, class: CrankClass) {
        let mut rebates = Self::get();
        rebates.push((who, class));
        sp_io::storage::set(Self::KEY, &rebates.encode());
    }
}

pub struct TestKeeperRebate;

impl KeeperRebateSink<AccountId32> for TestKeeperRebate {
    fn rebate(who: &AccountId32, class: CrankClass) {
        if RecordKeeperRebates::get() {
            KeeperRebates::push(who.clone(), class);
        }
    }
}

pub struct LedgerCalls;

impl LedgerCalls {
    const KEY: &'static [u8] = b":test:welfare:ledger-calls";

    pub fn get() -> Vec<LedgerCall> {
        sp_io::storage::get(Self::KEY)
            .and_then(|encoded| {
                let mut input: &[u8] = encoded.as_ref();
                Vec::<LedgerCall>::decode(&mut input).ok()
            })
            .unwrap_or_default()
    }

    pub fn set(calls: Vec<LedgerCall>) {
        sp_io::storage::set(Self::KEY, &calls.encode());
    }

    pub fn mutate(op: impl FnOnce(&mut Vec<LedgerCall>)) {
        let mut calls = Self::get();
        op(&mut calls);
        Self::set(calls);
    }
}

pub struct TestParams;

impl WelfareParamsProvider for TestParams {
    fn theta_s_lo() -> FixedU64 {
        ThetaSLo::get()
    }
    fn theta_s_hi() -> FixedU64 {
        ThetaSHi::get()
    }
    fn theta_c_lo() -> FixedU64 {
        ThetaCLo::get()
    }
    fn theta_c_hi() -> FixedU64 {
        ThetaCHi::get()
    }
    fn w_p() -> FixedU64 {
        WP::get()
    }
    fn w_a() -> FixedU64 {
        WA::get()
    }
}

pub struct TestMetricInputs;

impl MetricInputs for TestMetricInputs {
    fn onchain_components(_epoch: EpochId, spec_version: MetricSpecVersion) -> Vec<ComponentValue> {
        OnchainInputsByVersion::get()
            .into_iter()
            .find_map(|(version, components)| (version == spec_version).then_some(components))
            .unwrap_or_else(OnchainInput::get)
    }

    fn incident_multiplier(_epoch: EpochId) -> FixedU64 {
        IncidentInput::get()
    }

    fn daily_components(
        _epoch: EpochId,
        _day: u8,
        spec_version: MetricSpecVersion,
    ) -> Vec<ComponentValue> {
        DailyInputsByVersion::get()
            .into_iter()
            .find_map(|(version, components)| (version == spec_version).then_some(components))
            .unwrap_or_else(DailyInput::get)
    }
}

pub struct TestLedger;

impl LedgerSettlement for TestLedger {
    fn settle_scalar(pid: ProposalId, score: FixedU64) -> frame_support::dispatch::DispatchResult {
        let call = LedgerCall::Scalar(pid, score);
        if LedgerFailure::get().as_ref() == Some(&call) {
            return Err(sp_runtime::DispatchError::Other("injected ledger failure"));
        }
        LedgerCalls::mutate(|calls| calls.push(call));
        Ok(())
    }

    fn settle_gate(
        pid: ProposalId,
        gate: GateKind,
        breached: bool,
    ) -> frame_support::dispatch::DispatchResult {
        let call = LedgerCall::Gate(pid, gate, breached);
        if LedgerFailure::get().as_ref() == Some(&call) {
            return Err(sp_runtime::DispatchError::Other("injected ledger failure"));
        }
        LedgerCalls::mutate(|calls| calls.push(call));
        Ok(())
    }

    fn settle_baseline(epoch: EpochId, score: FixedU64) -> frame_support::dispatch::DispatchResult {
        let call = LedgerCall::Baseline(epoch, score);
        if LedgerFailure::get().as_ref() == Some(&call) {
            return Err(sp_runtime::DispatchError::Other("injected ledger failure"));
        }
        LedgerCalls::mutate(|calls| calls.push(call));
        Ok(())
    }
}

pub struct TestMetricGovernanceOrigin;

impl EnsureOrigin<RuntimeOrigin> for TestMetricGovernanceOrigin {
    type Success = ();

    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        let raw: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> =
            origin.clone().into();
        match raw {
            Ok(frame_system::RawOrigin::Signed(who)) if who == governance_acc() => Ok(()),
            _ => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(governance_acc()))
    }
}

impl pallet_welfare::Config for Test {
    type MetricGovernanceOrigin = TestMetricGovernanceOrigin;
    type Params = TestParams;
    type MetricInputs = TestMetricInputs;
    type Ledger = TestLedger;
    type CurrentEpoch = CurrentEpochValue;
    type KeeperRebate = TestKeeperRebate;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_welfare::BenchmarkHelper<RuntimeOrigin> for TestBenchmarkHelper {
    fn metric_governance_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(governance_acc())
    }
    fn prime_finalized_epoch(epoch: EpochId) {
        CurrentEpochValue::set(epoch.saturating_add(1));
    }
    fn prime_metric_inputs(count: u16) {
        let inputs = (1..=count)
            .map(|id| ComponentValue {
                id,
                value: FixedU64(ONE),
            })
            .collect::<Vec<_>>();
        OnchainInput::set(inputs.clone());
        DailyInput::set(inputs);
    }
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    ThetaSLo::set(CORE_THETA_S_LO);
    ThetaSHi::set(CORE_THETA_S_HI);
    ThetaCLo::set(CORE_THETA_C_LO);
    ThetaCHi::set(CORE_THETA_C_HI);
    WP::set(CORE_W_P);
    WA::set(CORE_W_A);
    CurrentEpochValue::set(FINALIZED_NOW);
    OnchainInput::set(healthy_components());
    OnchainInputsByVersion::set(Vec::new());
    DailyInput::set(healthy_components());
    DailyInputsByVersion::set(Vec::new());
    IncidentInput::set(FixedU64(ONE));
    LedgerFailure::set(None);
    RecordKeeperRebates::set(false);

    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        welfare: pallet_welfare::GenesisConfig {
            specs: vec![(1, genesis_specs(1))],
            _config: core::marker::PhantomData,
        },
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
