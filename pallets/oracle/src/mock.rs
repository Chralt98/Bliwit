//! Mock runtime for `pallet-oracle` (15 §4.1).
//!
//! Accounts are `AccountId32` — the real runtime's `[u8; 32]`-convertible
//! account, matching `oracle-core`'s `[u8; 32]` idiom. The `ReportingContext`
//! and `AdjudicationOrigin` stand in for the epoch/welfare reads and the
//! `pallet-origins` `OracleResolution` track the runtime wires at B1a.

use crate as pallet_oracle;
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use futarchy_primitives::{Balance, BlockNumber, EpochId, MetricId, MetricSpecVersion};
use sp_runtime::{traits::IdentityLookup, AccountId32, BuildStorage};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Oracle: pallet_oracle,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
}

parameter_types! {
    /// Report window end returned by the mock [`TestReporting`] (07 §5.1).
    pub static ReportWindowEnd: BlockNumber = 10;
    /// The frozen MetricSpec versions live cohorts consume (07 §2(4)). A set, so
    /// per-version games can be driven concurrently; empty ⇒ "no cohort consumes
    /// it" (every report rejected). Default `[3]`.
    pub static ExpectedSpecs: Vec<MetricSpecVersion> = vec![3];
    /// `StakeAtRisk` returned to the bond formula — 400k scaled ⇒ `B_1 = 10k`
    /// floor, matching the 07 §5 worked example.
    pub static StakeAtRiskValue: Balance = 400_000_000_000;
    /// The `(component, version)` pairs the mock reports as consumed by an
    /// epoch's cohorts (07 §2(4)); [`Pallet::note_settle_deadline`] neutral-settles
    /// any member that produced no report. Default empty ⇒ the money-deadline
    /// crank only neutralizes live rounds (tests that exercise the no-report path
    /// set this explicitly).
    pub static ExpectedComponents: Vec<(MetricId, MetricSpecVersion)> = vec![];
    /// Keeper-batch cap for `crank_round_close`.
    pub const MaxRoundCloseBatch: u32 = 20;
}

/// The account the mock `AdjudicationOrigin` treats as the `OracleResolution`
/// values track (07 §5.4). The runtime wires this over `pallet-origins`.
pub fn oracle_resolution_acc() -> AccountId32 {
    AccountId32::from([200u8; 32])
}

/// Deterministic 32-byte test account.
pub fn acc(n: u8) -> AccountId32 {
    AccountId32::from([n; 32])
}

/// Test stand-in for the `report`-time cross-pallet reads (07 §5.1/§2(4)/§6.1).
pub struct TestReporting;

impl pallet_oracle::ReportingContext for TestReporting {
    fn report_window_end(_measurement_epoch: EpochId) -> BlockNumber {
        ReportWindowEnd::get()
    }
    fn is_expected_spec_version(
        _component: MetricId,
        _epoch: EpochId,
        version: MetricSpecVersion,
    ) -> bool {
        ExpectedSpecs::get().contains(&version)
    }
    fn stake_at_risk(_component: MetricId, _epoch: EpochId) -> Balance {
        StakeAtRiskValue::get()
    }
    fn expected_components(_measurement_epoch: EpochId) -> Vec<(MetricId, MetricSpecVersion)> {
        ExpectedComponents::get()
    }
}

/// Admits only the dedicated `OracleResolution` account (07 §5.4); every other
/// origin — signed or root — is refused, the negative path for `adjudicate`.
pub struct TestAdjudicationOrigin;

impl EnsureOrigin<RuntimeOrigin> for TestAdjudicationOrigin {
    type Success = ();

    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let raw: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> =
            origin.clone().into();
        match raw {
            Ok(frame_system::RawOrigin::Signed(who)) if who == oracle_resolution_acc() => Ok(()),
            _ => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(oracle_resolution_acc()))
    }
}

impl pallet_oracle::Config for Test {
    type AdjudicationOrigin = TestAdjudicationOrigin;
    type Reporting = TestReporting;
    type MaxRoundCloseBatch = MaxRoundCloseBatch;
    type ProbeDispatch = ();
    type KeeperRebate = ();
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_oracle::BenchmarkHelper<RuntimeOrigin> for TestBenchmarkHelper {
    fn adjudication_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(oracle_resolution_acc())
    }
}

/// Externalities with the default (empty registries) genesis.
pub fn new_test_ext() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_oracle::GenesisConfig::default())
}

/// Externalities with an explicit oracle genesis (e.g. seeded recomputable set).
pub fn new_test_ext_with(oracle: pallet_oracle::GenesisConfig<Test>) -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        oracle,
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Set the mock chain block number (drives `Pallet::now()`).
pub fn set_block(n: BlockNumber) {
    System::set_block_number(n.into());
}
