//! Mock runtime for `pallet-guardian` (15 §4.1).
//!
//! The runtime account is `AccountId32` (02 §8), so `T::AccountId` satisfies the
//! `Into<[u8; 32]> + From<[u8; 32]>` bridge the pallet requires. The cross-pallet
//! seams (`ProposalStatusProvider`, `TriggerProvider`, `ReviewScheduler`,
//! `ValuesOrigin`) are backed by thread-local statics so tests can drive them.

use crate as pallet_guardian;
use crate::{
    GuardianProposalStatus, GuardianRecallScheduler, GuardianReviewScheduler, GuardianTriggers,
    ProposalStatus, TriggerState,
};
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use futarchy_primitives::ProposalId;
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Guardian: pallet_guardian,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
}

parameter_types! {
    pub static CurrentEpochValue: u32 = 0;
    /// Global proposal-status feed (mock ignores the pid; tests set it).
    pub static StatusFeed: (ProposalStatus, bool) = (ProposalStatus::Queued, false);
    /// Global verified-trigger feed.
    pub static TriggerFeed: TriggerState = TriggerState::none();
    /// Monotonic referendum index handed back by the review scheduler.
    pub static NextReferendum: u32 = 100;
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

pub struct TestScheduler;
impl GuardianReviewScheduler for TestScheduler {
    fn schedule_review(_action_id: crate::ActionId) -> u32 {
        let n = NextReferendum::get();
        NextReferendum::set(n + 1);
        n
    }
}

pub struct TestRecallScheduler;
impl GuardianRecallScheduler for TestRecallScheduler {
    fn schedule_recall(_action_id: crate::ActionId) -> u32 {
        let n = NextReferendum::get();
        NextReferendum::set(n + 1);
        n
    }
}

impl pallet_guardian::Config for Test {
    type ValuesOrigin = TestValuesOrigin;
    type CurrentEpoch = CurrentEpochValue;
    type ProposalStatusProvider = TestStatus;
    type TriggerProvider = TestTriggers;
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
