//! Mock runtime for `pallet-guardian` (15 §4.1).
//!
//! The runtime account is `AccountId32` (02 §8), so `T::AccountId` satisfies the
//! `Into<[u8; 32]> + From<[u8; 32]>` bridge the pallet requires. The cross-pallet
//! seams (`ProposalStatusProvider`, `TriggerProvider`, `ReviewScheduler`,
//! `ValuesOrigin`) are backed by thread-local statics so tests can drive them.

use crate as pallet_guardian;
use crate::{
    GuardianEffectDispatcher, GuardianPower, GuardianProposalStatus, GuardianProposalVeto,
    GuardianRecallScheduler, GuardianReviewScheduler, GuardianTriggers, PlaybookId, ProposalStatus,
    ReviewVerdict, TriggerState,
};
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use futarchy_primitives::ProposalId;
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Guardian: pallet_guardian,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
    type AccountData = pallet_balances::AccountData<futarchy_primitives::Balance>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type AccountStore = System;
    type Balance = futarchy_primitives::Balance;
}

parameter_types! {
    pub static CurrentEpochValue: u32 = 0;
    /// Global proposal-status feed (mock ignores the pid; tests set it).
    pub static StatusFeed: (ProposalStatus, bool) = (ProposalStatus::Queued, false);
    /// Global verified-trigger feed.
    pub static TriggerFeed: TriggerState = TriggerState::none();
    /// Monotonic referendum index handed back by the review scheduler.
    pub static NextReferendum: u32 = 100;
    pub static ReviewSchedulingFails: bool = false;
    pub static ReviewRefundFailsFor: Option<u32> = None;
    pub static ScheduledReviews: Vec<(crate::ActionId, ReviewVerdict, u32)> = Vec::new();
    pub static CancelledReviews: Vec<u32> = Vec::new();
    pub static RefundedReviews: Vec<u32> = Vec::new();
    pub static RecallSchedulingFails: bool = false;
    pub static VetoFails: bool = false;
    pub static ReviewDepositValue: futarchy_primitives::Balance =
        1_001 * futarchy_primitives::currency::VIT;
    pub SovereignAccountValue: AccountId32 = AccountId32::from([210; 32]);
}

/// The account the mock `ValuesOrigin` accepts as `ConstitutionalValues`.
pub const VALUES_ACC: [u8; 32] = [200u8; 32];

/// Test stand-in for the runtime's `pallet-origins`-backed `ConstitutionalValues`
/// resolver (A4/B1a): only `VALUES_ACC` (or Root, for benchmarking) passes.
pub struct TestValuesOrigin;
impl EnsureOrigin<RuntimeOrigin> for TestValuesOrigin {
    type Success = ();
    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let raw: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> =
            origin.clone().into();
        match raw {
            Ok(frame_system::RawOrigin::Signed(who)) if who == AccountId32::from(VALUES_ACC) => {
                Ok(())
            }
            _ => Err(origin),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(AccountId32::from(VALUES_ACC)))
    }
}

pub struct TestStatus;
impl GuardianProposalStatus for TestStatus {
    fn status(_pid: ProposalId) -> (ProposalStatus, bool) {
        StatusFeed::get()
    }
}

pub struct TestTriggers;
impl GuardianTriggers for TestTriggers {
    fn current() -> TriggerState {
        TriggerFeed::get()
    }
}

pub struct TestEffects;
impl GuardianEffectDispatcher for TestEffects {
    fn dispatch(
        _power: GuardianPower,
        _justification_hash: futarchy_primitives::H256,
    ) -> Result<(), sp_runtime::DispatchError> {
        Ok(())
    }

    fn revert_playbook(_id: PlaybookId) -> Result<(), sp_runtime::DispatchError> {
        Ok(())
    }

    fn renew_playbook(_id: PlaybookId) -> Result<(), sp_runtime::DispatchError> {
        Ok(())
    }
}

pub struct TestScheduler;
impl GuardianReviewScheduler for TestScheduler {
    fn review_deposit() -> futarchy_primitives::Balance {
        ReviewDepositValue::get()
    }

    fn schedule_review(
        action_id: crate::ActionId,
        verdict: ReviewVerdict,
    ) -> Result<u32, sp_runtime::DispatchError> {
        if ReviewSchedulingFails::get() {
            return Err(sp_runtime::DispatchError::Other(
                "review scheduler unavailable",
            ));
        }
        let n = NextReferendum::get();
        NextReferendum::set(n + 1);
        ScheduledReviews::mutate(|reviews| reviews.push((action_id, verdict, n)));
        Ok(n)
    }

    fn cancel_review(referendum: u32) -> Result<(), sp_runtime::DispatchError> {
        CancelledReviews::mutate(|reviews| reviews.push(referendum));
        Ok(())
    }

    fn refund_review(referendum: u32) -> Result<(), sp_runtime::DispatchError> {
        if ReviewRefundFailsFor::get() == Some(referendum) {
            return Err(sp_runtime::DispatchError::Other(
                "review refund unavailable",
            ));
        }
        RefundedReviews::mutate(|reviews| reviews.push(referendum));
        Ok(())
    }
}

pub struct TestRecallScheduler;
impl GuardianRecallScheduler for TestRecallScheduler {
    fn schedule_recall(
        _action_id: crate::ActionId,
        _slash_pool: futarchy_primitives::Balance,
    ) -> Result<u32, sp_runtime::DispatchError> {
        if RecallSchedulingFails::get() {
            return Err(sp_runtime::DispatchError::Other(
                "recall scheduler unavailable",
            ));
        }
        let n = NextReferendum::get();
        NextReferendum::set(n + 1);
        Ok(n)
    }

    fn refund_recall(_referendum: u32) -> Result<(), sp_runtime::DispatchError> {
        Ok(())
    }

    fn forward_failed_recall_pool(
        _amount: futarchy_primitives::Balance,
    ) -> Result<(), sp_runtime::DispatchError> {
        Ok(())
    }
}

pub struct TestVeto;
impl GuardianProposalVeto for TestVeto {
    fn uphold(_pid: ProposalId) -> Result<(), sp_runtime::DispatchError> {
        if VetoFails::get() {
            Err(sp_runtime::DispatchError::Other(
                "veto callback unavailable",
            ))
        } else {
            Ok(())
        }
    }
}

impl pallet_guardian::Config for Test {
    type ValuesOrigin = TestValuesOrigin;
    type AdminOrigin = TestValuesOrigin;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type SovereignAccount = SovereignAccountValue;
    type CurrentEpoch = CurrentEpochValue;
    type ProposalStatusProvider = TestStatus;
    type TriggerProvider = TestTriggers;
    type EffectDispatcher = TestEffects;
    type ProposalVeto = TestVeto;
    type ReviewScheduler = TestScheduler;
    type RecallScheduler = TestRecallScheduler;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_guardian::BenchmarkHelper<RuntimeOrigin> for TestBenchmarkHelper {
    fn signed(who: [u8; 32]) -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::from(who))
    }
    fn values() -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::from(VALUES_ACC))
    }
    fn admin() -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::from(VALUES_ACC))
    }
    fn prime_for_worst_case() {
        StatusFeed::set((ProposalStatus::Queued, false));
        TriggerFeed::set(TriggerState {
            depeg: true,
            migration_halt: true,
            oracle_deadlock: true,
            gate_breach: true,
            dead_man: true,
            void_in_flight: true,
            reserve_health: true,
            ledger_drift: true,
        });
    }
    fn prime_review_approved(_action: crate::ActionId) {}
    fn prime_maintenance_epoch(epoch: futarchy_primitives::EpochId) {
        CurrentEpochValue::set(epoch);
    }
    fn close_review(_referendum: u32) -> Result<(), sp_runtime::DispatchError> {
        Ok(())
    }
}

/// Deterministic council of seven distinct members.
pub fn members() -> [AccountId32; 7] {
    core::array::from_fn(|i| AccountId32::from([(i as u8) + 1; 32]))
}

/// Raw 32-byte member (for `RuntimeOrigin::signed`).
pub fn acct(n: u8) -> AccountId32 {
    AccountId32::from([n; 32])
}

/// The `ConstitutionalValues` origin the mock accepts.
pub fn values_origin() -> RuntimeOrigin {
    RuntimeOrigin::signed(AccountId32::from(VALUES_ACC))
}

/// Externalities with a genesis-seeded council.
pub fn new_test_ext() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_guardian::GenesisConfig::<Test> {
        members: members().to_vec(),
        _config: Default::default(),
    })
}

/// Externalities with no council (await `set_members`).
pub fn new_test_ext_empty() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_guardian::GenesisConfig::<Test>::default())
}

/// Externalities with an explicit guardian genesis.
pub fn new_test_ext_with(
    guardian: pallet_guardian::GenesisConfig<Test>,
) -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: members()
                .into_iter()
                .map(|who| (who, 2 * crate::GUARDIAN_BOND))
                .chain(core::iter::once((SovereignAccountValue::get(), 1)))
                .collect(),
            dev_accounts: None,
        },
        guardian,
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Drive the mock epoch clock.
pub fn set_epoch(epoch: u32) {
    CurrentEpochValue::set(epoch);
}

/// Set the global proposal-status feed.
pub fn set_status(status: ProposalStatus, in_rerun: bool) {
    StatusFeed::set((status, in_rerun));
}

/// Set the global trigger feed.
pub fn set_triggers(triggers: TriggerState) {
    TriggerFeed::set(triggers);
}

/// Advance to `block`, running `on_initialize` for each newly-entered block so
/// the maintenance hook fires (playbook expiry, review-deadline enforcement).
pub fn run_to_block(block: u32) {
    use frame_support::traits::{OnFinalize, OnInitialize};
    while System::block_number() < block as u64 {
        let now = System::block_number();
        Guardian::on_finalize(now);
        System::on_finalize(now);
        System::set_block_number(now + 1);
        System::on_initialize(now + 1);
        Guardian::on_initialize(now + 1);
    }
}
