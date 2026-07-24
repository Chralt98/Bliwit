//! Mock runtime for `pallet-futarchy-treasury` (15 §4.1).

use crate as pallet_futarchy_treasury;
use crate::{
    PayoutLine, PotFunding, RebatePayout, TreasuryParams, ISS_INFLATION_CAP_BPS, TRS_CAP_180D_BPS,
    TRS_CAP_30D_BPS, TRS_CAP_PROPOSAL_BPS, TRS_STREAM_THRESHOLD_BPS,
};
use frame_support::{
    derive_impl, parameter_types,
    traits::{ConstU32, EnsureOrigin},
};
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

pub fn coretime_quote_authority() -> AccountId32 {
    acc(42)
}

parameter_types! {
    pub static CurrentEpochValue: u32 = 0;
    pub static TreasuryArmedValue: bool = false;
    // 13 §1 treasury tunables — defaulting to the core defaults so shell ≡ core,
    // overridable per-test to prove the pallet reads `Params` (rule 4), never a
    // hardcode. The runtime (B1a) reads these from `pallet-constitution::Params`.
    pub static CapProposalBps: u32 = TRS_CAP_PROPOSAL_BPS;
    pub static Cap30dBps: u32 = TRS_CAP_30D_BPS;
    pub static Cap180dBps: u32 = TRS_CAP_180D_BPS;
    pub static StreamThresholdBps: u32 = TRS_STREAM_THRESHOLD_BPS;
    pub static InflationCapBps: u32 = ISS_INFLATION_CAP_BPS;
    pub static KeeperBudgetEpoch: u128 = futarchy_treasury_core::KEEPER_BUDGET_EPOCH;
    // `keeper.rebate` held at 0 here to exercise the fail-soft no-payout path.
    // SQ-117 seeds a positive value in the runtime genesis; the mock keeps zero
    // deliberately, so a zero rebate must remain a safe no-op.
    pub static KeeperRebate: u128 = 0;
    // Test-only injected coretime parameters. They are deliberately simple
    // non-default values so tests prove the pallet consumes the seam.
    pub static CoretimeDotRate: u128 = 10_000_000_000;
    pub static ReserveProbeDotRate: u128 = 10_000_000_000;
    pub static ReserveProbeFeeDot: u128 = 100;
    pub static ReserveProbeFailThreshold: u8 = 2;
    pub static ReserveProbeRecoverThreshold: u8 = 3;
    pub static CoretimeFeeDot: u128 = 100;
    pub static CoretimeQuoteTtl: u32 = 100;
    // Configurable stand-ins for the real KEEPER__/ORACLE__ USDC custody pots.
    pub static KeeperRebatePotBalance: u128 = 0;
    pub static OracleRebatePotBalance: u128 = 0;
    pub static RewardsPayoutPotBalance: u128 = 0;
    pub static OpsCollatorPayoutPotBalance: u128 = 0;
    pub CommunityPot: AccountId32 = AccountId32::new([77u8; 32]);
    pub static CommunityDistributionAmount: u128 = 250_000_000 * futarchy_treasury_core::VIT;
    pub static CommunityVestingDuration: u64 = 100;
    pub static CommunityMinVestedTransfer: u128 = futarchy_treasury_core::VIT;
    pub static MaxCommunitySchedules: u32 = 2;
}

pub struct TestTreasuryPhase;
impl pallet_futarchy_treasury::TreasuryPhase for TestTreasuryPhase {
    fn treasury_armed() -> bool {
        TreasuryArmedValue::get()
    }
}

pub struct TestBootstrapOpsFundingPolicy;
impl pallet_futarchy_treasury::BootstrapOpsFundingPolicy for TestBootstrapOpsFundingPolicy {
    fn reserve_probe_ceiling() -> Option<u128> {
        futarchy_treasury_core::reserve_probe_runway_debit(
            ReserveProbeFeeDot::get(),
            ReserveProbeDotRate::get(),
            ReserveProbeFailThreshold::get(),
            ReserveProbeRecoverThreshold::get(),
        )
        .ok()
    }
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
    fn collator_comp_epoch() -> u128 {
        2_000 * futarchy_treasury_core::USDC
    }
    fn coretime_dot_rate() -> u128 {
        CoretimeDotRate::get()
    }
    fn reserve_probe_dot_rate() -> u128 {
        ReserveProbeDotRate::get()
    }
    fn coretime_fee_dot() -> u128 {
        CoretimeFeeDot::get()
    }
    fn coretime_quote_ttl() -> u32 {
        CoretimeQuoteTtl::get()
    }
}

type CommunityVestingCall = (AccountId32, AccountId32, u128, u128, u64);

std::thread_local! {
    static REBATE_PAYOUTS: RefCell<Vec<(AccountId32, u128, PayoutLine)>> = const { RefCell::new(Vec::new()) };
    static FAIL_REBATE_PAYOUT: Cell<bool> = const { Cell::new(false) };
    static POT_FUNDING_CALLS: RefCell<Vec<(PayoutLine, u128)>> = const { RefCell::new(Vec::new()) };
    static FAIL_POT_FUNDING: Cell<bool> = const { Cell::new(false) };
    static INSURANCE_SWEEPS: RefCell<Vec<u128>> = const { RefCell::new(Vec::new()) };
    static FAIL_INSURANCE_SWEEP: Cell<bool> = const { Cell::new(false) };
    static COMMUNITY_VESTING_CALLS: RefCell<Vec<CommunityVestingCall>> = const { RefCell::new(Vec::new()) };
    static FAIL_COMMUNITY_VESTING: Cell<bool> = const { Cell::new(false) };
}

pub struct RecordingCommunityVesting;

impl pallet_futarchy_treasury::CommunityVesting<AccountId32, u64> for RecordingCommunityVesting {
    fn vested_transfer(
        source: &AccountId32,
        beneficiary: &AccountId32,
        amount: u128,
        per_block: u128,
        starting_block: u64,
    ) -> frame_support::dispatch::DispatchResult {
        if FAIL_COMMUNITY_VESTING.with(Cell::get) {
            return Err(sp_runtime::DispatchError::Other("community vesting failed"));
        }
        COMMUNITY_VESTING_CALLS.with(|calls| {
            calls.borrow_mut().push((
                source.clone(),
                beneficiary.clone(),
                amount,
                per_block,
                starting_block,
            ));
        });
        Ok(())
    }
}

pub fn community_vesting_calls() -> Vec<(AccountId32, AccountId32, u128, u128, u64)> {
    COMMUNITY_VESTING_CALLS.with(|calls| calls.borrow().clone())
}

pub fn set_community_vesting_failure(fail: bool) {
    FAIL_COMMUNITY_VESTING.with(|value| value.set(fail));
}

pub fn reset_community_vesting() {
    COMMUNITY_VESTING_CALLS.with(|calls| calls.borrow_mut().clear());
    set_community_vesting_failure(false);
}

/// Stand-in for the runtime's INSURANCE → `MAIN` USDC custody move (08 §1.4).
/// `set_insurance_sweep_failure` models the `Preservation::Preserve` refusal
/// that an over-large sweep must produce.
pub struct MockInsuranceSweep;

impl crate::InsuranceSweep for MockInsuranceSweep {
    fn sweep(amount: u128) -> frame_support::pallet_prelude::DispatchResult {
        INSURANCE_SWEEPS.with(|calls| calls.borrow_mut().push(amount));
        if FAIL_INSURANCE_SWEEP.with(Cell::get) {
            return Err(sp_runtime::DispatchError::Other(
                "insurance sweep would reap the account",
            ));
        }
        Ok(())
    }
}

pub fn insurance_sweeps() -> Vec<u128> {
    INSURANCE_SWEEPS.with(|calls| calls.borrow().clone())
}

pub fn set_insurance_sweep_failure(fail: bool) {
    FAIL_INSURANCE_SWEEP.with(|value| value.set(fail));
}

pub fn reset_insurance_sweeps() {
    INSURANCE_SWEEPS.with(|calls| calls.borrow_mut().clear());
    set_insurance_sweep_failure(false);
}

pub struct MockPotFunding;

impl PotFunding<AccountId32> for MockPotFunding {
    fn fund(line: PayoutLine, amount: u128) -> frame_support::pallet_prelude::DispatchResult {
        POT_FUNDING_CALLS.with(|calls| calls.borrow_mut().push((line, amount)));
        if FAIL_POT_FUNDING.with(Cell::get) {
            return Err(sp_runtime::DispatchError::Other("pot funding failed"));
        }
        let next = RecordingRebatePayout::pot_balance(line)
            .checked_add(amount)
            .ok_or(sp_runtime::DispatchError::Other("pot funding overflow"))?;
        set_rebate_pot_balance(line, next);
        Ok(())
    }
}

pub fn pot_funding_calls() -> Vec<(PayoutLine, u128)> {
    POT_FUNDING_CALLS.with(|calls| calls.borrow().clone())
}

pub fn set_pot_funding_failure(fail: bool) {
    FAIL_POT_FUNDING.with(|value| value.set(fail));
}

pub fn reset_pot_funding() {
    POT_FUNDING_CALLS.with(|calls| calls.borrow_mut().clear());
    set_pot_funding_failure(false);
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
            PayoutLine::Rewards => RewardsPayoutPotBalance::get(),
            PayoutLine::OpsCollators => OpsCollatorPayoutPotBalance::get(),
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
        PayoutLine::Rewards => RewardsPayoutPotBalance::set(balance),
        PayoutLine::OpsCollators => OpsCollatorPayoutPotBalance::set(balance),
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
    type CommunityDistributionOrigin = TestTreasuryOrigin;
    type CommunityVesting = RecordingCommunityVesting;
    type CommunityPot = CommunityPot;
    type CommunityDistributionAmount = CommunityDistributionAmount;
    type CommunityVestingDuration = CommunityVestingDuration;
    type CommunityMinVestedTransfer = CommunityMinVestedTransfer;
    type MaxCommunitySchedules = MaxCommunitySchedules;
    type MaxCollatorCompensationEntries =
        ConstU32<{ pallet_futarchy_treasury::MAX_COLLATOR_COMPENSATION_ENTRIES_BOUND }>;
    type RegisteredCollatorCount = ConstU32<2>;
    type Params = TestParams;
    type CurrentEpoch = CurrentEpochValue;
    type TreasuryPhase = TestTreasuryPhase;
    type BootstrapOpsFundingPolicy = TestBootstrapOpsFundingPolicy;
    type RenewalDispatch = ();
    type RebatePayout = RecordingRebatePayout;
    type PotFunding = MockPotFunding;
    type InsuranceSweep = MockInsuranceSweep;
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
    fn community_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(treasury_acc())
    }
    fn account(seed: u8) -> AccountId32 {
        AccountId32::new([seed; 32])
    }
}

/// Externalities with the default (empty, 1e9 VIT supply) treasury genesis.
pub fn new_test_ext() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_futarchy_treasury::GenesisConfig {
        coretime_quote_authority: Some(coretime_quote_authority()),
        coretime_renewal_account: Some([44; 32]),
        ..Default::default()
    })
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
        CoretimeDotRate::set(10_000_000_000);
        ReserveProbeDotRate::set(10_000_000_000);
        ReserveProbeFeeDot::set(100);
        ReserveProbeFailThreshold::set(2);
        ReserveProbeRecoverThreshold::set(3);
        CoretimeFeeDot::set(100);
        CoretimeQuoteTtl::set(100);
        TreasuryArmedValue::set(false);
        reset_rebate_payout();
        // `reset_rebate_payout` preserves the funded rewards pot for tests
        // that merely reset payout observations; each fresh externality still
        // starts with an empty custody fixture.
        RewardsPayoutPotBalance::set(0);
        OpsCollatorPayoutPotBalance::set(0);
        reset_pot_funding();
        reset_insurance_sweeps();
        reset_community_vesting();
    });
    ext
}

/// Drive the mock epoch clock (`Config::CurrentEpoch`).
pub fn set_epoch(epoch: u32) {
    CurrentEpochValue::set(epoch);
}
