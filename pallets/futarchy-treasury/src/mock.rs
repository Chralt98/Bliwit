//! Mock runtime for `pallet-futarchy-treasury` (15 §4.1).

use crate as pallet_futarchy_treasury;
use crate::{
    PayoutLine, RebatePayout, TreasuryParams, ISS_INFLATION_CAP_BPS, TRS_CAP_180D_BPS,
    TRS_CAP_30D_BPS, TRS_CAP_PROPOSAL_BPS, TRS_STREAM_THRESHOLD_BPS,
};
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage};
use std::cell::{Cell, RefCell};

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
    pub static KeeperBudgetEpoch: u128 = futarchy_treasury_core::KEEPER_BUDGET_EPOCH;
    // `keeper.rebate` is intentionally absent from genesis Params until B5.
    pub static KeeperRebate: u128 = 0;
    // Configurable stand-ins for the real KEEPER__/ORACLE__ USDC custody pots.
    pub static KeeperRebatePotBalance: u128 = 0;
    pub static OracleRebatePotBalance: u128 = 0;
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
    fn keeper_budget_epoch() -> u128 {
        KeeperBudgetEpoch::get()
    }
    fn keeper_rebate() -> u128 {
        KeeperRebate::get()
    }
}

std::thread_local! {
    static REBATE_PAYOUTS: RefCell<Vec<(AccountId32, u128, PayoutLine)>> = const { RefCell::new(Vec::new()) };
    static FAIL_REBATE_PAYOUT: Cell<bool> = const { Cell::new(false) };
}

pub struct RecordingRebatePayout;

impl RebatePayout<AccountId32> for RecordingRebatePayout {
    fn pay(
        who: &AccountId32,
        amount: u128,
        line: PayoutLine,
    ) -> frame_support::pallet_prelude::DispatchResult {
        REBATE_PAYOUTS.with(|calls| calls.borrow_mut().push((who.clone(), amount, line)));
        if FAIL_REBATE_PAYOUT.with(Cell::get) {
            return Err(sp_runtime::DispatchError::Other("rebate payout failed"));
        }
        let available = Self::pot_balance(line);
        let Some(remaining) = available.checked_sub(amount) else {
            return Err(sp_runtime::DispatchError::Other(
                "rebate payout pot underfunded",
            ));
        };
        set_rebate_pot_balance(line, remaining);
        Ok(())
    }

    fn pot_balance(line: PayoutLine) -> u128 {
        match line {
            PayoutLine::Keeper => KeeperRebatePotBalance::get(),
            PayoutLine::Oracle => OracleRebatePotBalance::get(),
        }
    }
}

pub fn rebate_payouts() -> Vec<(AccountId32, u128, PayoutLine)> {
    REBATE_PAYOUTS.with(|calls| calls.borrow().clone())
}

pub fn set_rebate_payout_failure(fail: bool) {
    FAIL_REBATE_PAYOUT.with(|value| value.set(fail));
}

pub fn set_rebate_pot_balance(line: PayoutLine, balance: u128) {
    match line {
        PayoutLine::Keeper => KeeperRebatePotBalance::set(balance),
        PayoutLine::Oracle => OracleRebatePotBalance::set(balance),
    }
}

pub fn reset_rebate_payout() {
    REBATE_PAYOUTS.with(|calls| calls.borrow_mut().clear());
    set_rebate_payout_failure(false);
    KeeperRebatePotBalance::set(0);
    OracleRebatePotBalance::set(0);
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
    type RebatePayout = RecordingRebatePayout;
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
    ext.execute_with(|| {
        System::set_block_number(1);
        KeeperBudgetEpoch::set(futarchy_treasury_core::KEEPER_BUDGET_EPOCH);
        KeeperRebate::set(0);
        reset_rebate_payout();
    });
    ext
}

/// Drive the mock epoch clock (`Config::CurrentEpoch`).
pub fn set_epoch(epoch: u32) {
    CurrentEpochValue::set(epoch);
}
