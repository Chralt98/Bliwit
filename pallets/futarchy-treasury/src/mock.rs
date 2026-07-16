//! Mock runtime for `pallet-futarchy-treasury` (15 §4.1).

use crate as pallet_futarchy_treasury;
use crate::{
    TreasuryParams, ISS_INFLATION_CAP_BPS, TRS_CAP_180D_BPS, TRS_CAP_30D_BPS, TRS_CAP_PROPOSAL_BPS,
    TRS_STREAM_THRESHOLD_BPS,
};
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Treasury: pallet_futarchy_treasury,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    // The treasury core keys accounts as `[u8; 32]` (08 §1); the mock uses the
    // production `AccountId32` (which is `From`/`Into<[u8; 32]>`), so the shell
    // exercises the real conversion path.
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
}

/// The single origin the outflow calls admit (08 §1.1).
pub fn treasury_acc() -> AccountId32 {
    AccountId32::new([2u8; 32])
}
/// Any other account — every outflow call must refuse it.
pub fn nobody() -> AccountId32 {
    AccountId32::new([99u8; 32])
}
pub fn acc(n: u8) -> AccountId32 {
    AccountId32::new([n; 32])
}

parameter_types! {
    pub static CurrentEpochValue: u32 = 0;
    // 13 §1 treasury tunables — defaulting to the core defaults so shell ≡ core,
    // overridable per-test to prove the pallet reads `Params` (rule 4), never a
    // hardcode. The runtime (B1a) reads these from `pallet-constitution::Params`.
    pub static CapProposalBps: u32 = TRS_CAP_PROPOSAL_BPS;
    pub static Cap30dBps: u32 = TRS_CAP_30D_BPS;
    pub static Cap180dBps: u32 = TRS_CAP_180D_BPS;
    pub static StreamThresholdBps: u32 = TRS_STREAM_THRESHOLD_BPS;
    pub static InflationCapBps: u32 = ISS_INFLATION_CAP_BPS;
}

pub struct TestParams;
impl TreasuryParams for TestParams {
    fn cap_proposal_bps() -> u32 {
        CapProposalBps::get()
    }
    fn cap_30d_bps() -> u32 {
        Cap30dBps::get()
    }
    fn cap_180d_bps() -> u32 {
        Cap180dBps::get()
    }
    fn stream_threshold_bps() -> u32 {
        StreamThresholdBps::get()
    }
    fn inflation_cap_bps() -> u32 {
        InflationCapBps::get()
    }
}

/// Test stand-in for the runtime's `pallet-origins`-backed `EnsureFutarchyTreasury`
/// (A4/B1a): admits exactly the `FutarchyTreasury` origin, refuses everything
/// else (Root, other governance accounts, ordinary Signed).
pub struct TestTreasuryOrigin;

impl EnsureOrigin<RuntimeOrigin> for TestTreasuryOrigin {
    type Success = ();

    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        let raw: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> =
            origin.clone().into();
        match raw {
            Ok(frame_system::RawOrigin::Signed(who)) if who == treasury_acc() => Ok(()),
            _ => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(treasury_acc()))
    }
}

impl pallet_futarchy_treasury::Config for Test {
    type TreasuryOrigin = TestTreasuryOrigin;
    type Params = TestParams;
    type CurrentEpoch = CurrentEpochValue;
    type RenewalDispatch = ();
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_futarchy_treasury::BenchmarkHelper<RuntimeOrigin, AccountId32> for TestBenchmarkHelper {
    fn treasury_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(treasury_acc())
    }
    fn account(seed: u8) -> AccountId32 {
        AccountId32::new([seed; 32])
    }
}

/// Externalities with the default (empty, 1e9 VIT supply) treasury genesis.
pub fn new_test_ext() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_futarchy_treasury::GenesisConfig::default())
}

/// Externalities with an explicit treasury genesis.
pub fn new_test_ext_with(
    treasury: pallet_futarchy_treasury::GenesisConfig<Test>,
) -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        treasury,
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Drive the mock epoch clock (`Config::CurrentEpoch`).
pub fn set_epoch(epoch: u32) {
    CurrentEpochValue::set(epoch);
}
