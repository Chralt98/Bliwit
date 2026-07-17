//! Mock runtime and transaction-aware sibling-pallet doubles for `pallet-epoch`.

use crate as pallet_epoch;
use crate::*;
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    BoundedVec, Branch, ProposalState, ResourceId,
};
use parity_scale_codec::{Decode, Encode};
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage, DispatchError};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Epoch: pallet_epoch,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
}

pub fn account(seed: u8) -> AccountId32 {
    AccountId32::new([seed; 32])
}

pub fn keeper() -> AccountId32 {
    account(1)
}

pub fn guardian() -> AccountId32 {
    account(2)
}

pub fn execution_guard() -> AccountId32 {
    account(3)
}

pub fn constitutional_values() -> AccountId32 {
    account(4)
}

pub fn void_authority() -> AccountId32 {
    account(5)
}

pub fn nobody() -> AccountId32 {
    account(99)
}

pub fn baseline(epoch: EpochId) -> MarketId {
    9_000u64.saturating_add(epoch.into())
}

pub fn markets(pid: ProposalId, epoch: EpochId, gates: bool) -> MarketSet {
    let first = pid.saturating_mul(10);
    MarketSet {
        accept: first.saturating_add(1),
        reject: first.saturating_add(2),
        gates: gates.then_some([
            first.saturating_add(3),
            first.saturating_add(4),
            first.saturating_add(5),
            first.saturating_add(6),
        ]),
        baseline: baseline(epoch),
    }
}

pub fn proposal(
    id: ProposalId,
    proposer: AccountId32,
    state: ProposalState,
    epoch: EpochId,
    now: BlockNumber,
) -> Proposal<AccountId32> {
    Proposal {
        id,
        proposer,
        class: ProposalClass::Param,
        state,
        epoch,
        submitted_at: now,
        payload_hash: [id as u8; 32],
        payload_len: 32,
        ask: 0,
        bond: 10,
        resources: BoundedVec::try_from(vec![[id as u8; 8]])
            .expect("one mock resource is within the primitive bound"),
        metric_spec: 1,
        decide_at: 0,
        rerun: false,
        extended: false,
        delayed_once: false,
        markets: None,
        maturity: None,
        grace_end: None,
        version_constraint: None,
        decision: None,
    }
}

pub fn live_proposal(
    id: ProposalId,
    state: ProposalState,
    epoch: EpochId,
) -> Proposal<AccountId32> {
    let mut proposal = proposal(id, account((id % 200) as u8), state, epoch, 1);
    proposal.decide_at = 1;
    proposal.markets = Some(markets(id, epoch, false));
    proposal
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub enum SeamCall {
    OpenMarkets(ProposalId, bool, bool),
    Enqueue {
        pid: ProposalId,
        payload_hash: H256,
        maturity: BlockNumber,
        grace: BlockNumber,
        requires_ratification: bool,
    },
    DequeueTerminal(ProposalId),
    Welfare(EpochId, MetricSpecVersion, SettlementTarget),
    CreateVault(ProposalId, MetricSpecVersion),
    Resolve(ProposalId, Branch),
    Void(ProposalId),
}

pub struct SeamCalls;

impl SeamCalls {
    const KEY: &'static [u8] = b":test:epoch:seam-calls";

    pub fn get() -> Vec<SeamCall> {
        sp_io::storage::get(Self::KEY)
            .and_then(|encoded| {
                let mut input: &[u8] = encoded.as_ref();
                Vec::<SeamCall>::decode(&mut input).ok()
            })
            .unwrap_or_default()
    }

    pub fn set(calls: Vec<SeamCall>) {
        sp_io::storage::set(Self::KEY, &calls.encode());
    }

    pub fn push(call: SeamCall) -> Result<(), DispatchError> {
        if SeamFailure::get().as_ref() == Some(&call) {
            return Err(DispatchError::Other("injected epoch seam failure"));
        }
        let mut calls = Self::get();
        calls.push(call);
        Self::set(calls);
        Ok(())
    }
}

/// Transaction-aware A11 storage adapter for A8 handoff regressions. The real
/// guard suite independently tests these exact ownership classes; this model
/// proves `epoch.tick` reaches the full cleanup through the configured seam.
#[derive(Clone, Debug, Decode, Default, Encode, Eq, PartialEq)]
pub struct MockGuardState {
    pub queue: Vec<(ProposalId, H256)>,
    pub held_resources: Vec<(ProposalId, ResourceId)>,
    pub expedited: Vec<ProposalId>,
    pub attestation_bindings: Vec<(ProposalId, u32, H256)>,
    pub ratifications: Vec<ProposalId>,
    pub pinned_preimages: Vec<(ProposalId, H256)>,
    pub unpinned_preimages: Vec<H256>,
}

pub struct GuardStateModel;

impl GuardStateModel {
    const KEY: &'static [u8] = b":test:epoch:guard-state";

    pub fn get() -> MockGuardState {
        sp_io::storage::get(Self::KEY)
            .and_then(|encoded| {
                let mut input: &[u8] = encoded.as_ref();
                MockGuardState::decode(&mut input).ok()
            })
            .unwrap_or_default()
    }

    pub fn set(state: MockGuardState) {
        sp_io::storage::set(Self::KEY, &state.encode());
    }

    pub fn insert(pid: ProposalId, payload_hash: H256) -> frame_support::dispatch::DispatchResult {
        let mut state = Self::get();
        if state.queue.iter().any(|(candidate, _)| *candidate == pid) {
            return Ok(());
        }
        if state.queue.len() >= futarchy_primitives::bounds::MAX_LIVE_PROPOSALS as usize {
            return Err(DispatchError::Other("mock execution guard QueueFull"));
        }
        state.queue.push((pid, payload_hash));
        state.held_resources.push((pid, [pid as u8; 8]));
        state.expedited.push(pid);
        state.pinned_preimages.push((pid, payload_hash));
        Self::set(state);
        Ok(())
    }

    pub fn prime_full(
        pid: ProposalId,
        payload_hash: H256,
    ) -> frame_support::dispatch::DispatchResult {
        Self::insert(pid, payload_hash)?;
        let mut state = Self::get();
        state.attestation_bindings.push((pid, 7, payload_hash));
        state.ratifications.push(pid);
        Self::set(state);
        Ok(())
    }

    pub fn remove(pid: ProposalId) {
        let mut state = Self::get();
        let Some(payload_hash) = state
            .queue
            .iter()
            .find_map(|(candidate, hash)| (*candidate == pid).then_some(*hash))
        else {
            return;
        };
        state.queue.retain(|(candidate, _)| *candidate != pid);
        state
            .held_resources
            .retain(|(candidate, _)| *candidate != pid);
        state.expedited.retain(|candidate| *candidate != pid);
        state
            .attestation_bindings
            .retain(|(candidate, _, _)| *candidate != pid);
        state.ratifications.retain(|candidate| *candidate != pid);
        state
            .pinned_preimages
            .retain(|(candidate, _)| *candidate != pid);
        state.unpinned_preimages.push(payload_hash);
        Self::set(state);
    }
}

parameter_types! {
    pub static ParamsValue: CoreEpochParams = CoreEpochParams::DEFAULT;
    pub static SeamFailure: Option<SeamCall> = None;
    pub static MarketGrade: bool = true;
    pub static UnavailableMarkets: Vec<MarketId> = Vec::new();
    pub static UngradedMarkets: Vec<MarketId> = Vec::new();
    pub static BaselineAvailable: bool = true;
    pub static TwapOverrides: Vec<(MarketId, FixedU64)> = Vec::new();
    pub static TrailingOverrides: Vec<(MarketId, FixedU64)> = Vec::new();
    pub static SpotOverrides: Vec<(MarketId, FixedU64)> = Vec::new();
    pub static MeasuredDepth: Balance = 1_000_000_000_000;
    pub static PublishedFlow: Option<Balance> = None;
    pub static SecondInsufficiency: bool = false;
    pub static OpenDispute: bool = false;
    pub static GuardianHold: bool = false;
    pub static DeadManEngaged: bool = false;
    pub static ReviewClosed: bool = false;
    pub static AttestationQuorate: bool = true;
    pub static StaticChecks: bool = true;
    pub static QueueTimeCheck: bool = true;
    pub static InCapPrize: Option<Balance> = Some(100);
    pub static LedgerFrozen: bool = false;
    pub static PhaseFlagsValue: u32 = 0;
    pub static ActiveMetricSpecVersion: MetricSpecVersion = 1;
    pub static TreasuryGateRequired: bool = false;
    pub static PreimageLen: Option<u32> = Some(32);
    pub static PreimageNoted: bool = true;
    pub static PreimageRequests: Vec<(H256, u32)> = Vec::new();
    pub static PreimageRequestFails: bool = false;
    pub static QueueReject: Option<RejectReason> = None;
    pub static RetryExhausted: bool = false;
    pub static WelfareScore: FixedU64 = FixedU64(500_000_000);
    pub static PreviousBaselineTwap: Option<FixedU64> = None;
    /// Disabled by default, so the mock behaves like the `()` sink unless a
    /// keeper-rebate regression explicitly enables recording.
    pub static RecordKeeperRebates: bool = false;
    pub static KeeperRebates: Vec<(AccountId32, CrankClass)> = Vec::new();
}

pub struct TestKeeperRebate;

impl KeeperRebateSink<AccountId32> for TestKeeperRebate {
    fn rebate(who: &AccountId32, class: CrankClass) {
        if RecordKeeperRebates::get() {
            let mut rebates = KeeperRebates::get();
            rebates.push((who.clone(), class));
            KeeperRebates::set(rebates);
        }
    }
}

pub struct TestParams;

impl EpochParamsProvider for TestParams {
    fn get() -> CoreEpochParams {
        ParamsValue::get()
    }
}

pub struct TestMarket;

impl MarketAccess<AccountId32> for TestMarket {
    fn open_markets(
        proposal: &Proposal<AccountId32>,
        rerun: bool,
        requires_gate_markets: bool,
    ) -> Result<MarketSet, DispatchError> {
        SeamCalls::push(SeamCall::OpenMarkets(
            proposal.id,
            rerun,
            requires_gate_markets,
        ))?;
        Ok(markets(proposal.id, proposal.epoch, requires_gate_markets))
    }

    fn baseline_market(epoch: EpochId) -> Option<MarketId> {
        BaselineAvailable::get().then_some(baseline(epoch))
    }

    fn twap_full(market: MarketId) -> Option<FixedU64> {
        (!UnavailableMarkets::get().contains(&market))
            .then(|| value_for(market, &TwapOverrides::get()))
    }

    fn twap_trailing(market: MarketId, _window: BlockNumber) -> Option<FixedU64> {
        (!UnavailableMarkets::get().contains(&market)).then(|| {
            TrailingOverrides::get()
                .iter()
                .find_map(|(id, value)| (*id == market).then_some(*value))
                .unwrap_or_else(|| value_for(market, &TwapOverrides::get()))
        })
    }

    fn spot(market: MarketId) -> Option<FixedU64> {
        (!UnavailableMarkets::get().contains(&market))
            .then(|| value_for(market, &SpotOverrides::get()))
    }

    fn decision_grade(
        market: MarketId,
        _role: BookRole,
        _class: ProposalClass,
        _params: &CoreEpochParams,
    ) -> bool {
        MarketGrade::get()
            && !UnavailableMarkets::get().contains(&market)
            && !UngradedMarkets::get().contains(&market)
    }

    fn measured_depth(_pid: ProposalId) -> Balance {
        MeasuredDepth::get()
    }

    fn published_flow_per_day(_pid: ProposalId) -> Option<Balance> {
        PublishedFlow::get()
    }

    fn second_insufficiency(_pid: ProposalId) -> bool {
        SecondInsufficiency::get()
    }

    fn previous_settled_baseline_twap(_epoch: EpochId) -> Option<FixedU64> {
        PreviousBaselineTwap::get()
    }
}

fn value_for(market: MarketId, overrides: &[(MarketId, FixedU64)]) -> FixedU64 {
    overrides
        .iter()
        .find_map(|(id, value)| (*id == market).then_some(*value))
        .unwrap_or(match market % 10 {
            1 => FixedU64(600_000_000),
            2 => FixedU64(500_000_000),
            3..=6 => FixedU64(0),
            _ => FixedU64(500_000_000),
        })
}

pub struct TestOracle;
impl OracleAccess for TestOracle {
    fn any_open_dispute_touching(_spec: MetricSpecVersion) -> bool {
        OpenDispute::get()
    }
}

pub struct TestGuardian;
impl GuardianAccess for TestGuardian {
    fn hold_active(_pid: ProposalId) -> bool {
        GuardianHold::get()
    }
    fn dead_man_engaged() -> bool {
        DeadManEngaged::get()
    }
    fn review_window_closed(_pid: ProposalId) -> bool {
        ReviewClosed::get()
    }
}

pub struct TestAttestation;
impl AttestationAccess for TestAttestation {
    fn present_and_quorate(_pid: ProposalId, _artifact_hash: H256) -> bool {
        AttestationQuorate::get()
    }
}

pub struct TestConstitution;
impl ConstitutionAccess<AccountId32> for TestConstitution {
    fn static_checks_pass(_proposal: &Proposal<AccountId32>) -> bool {
        StaticChecks::get()
    }
    fn queue_time_check(_proposal: &Proposal<AccountId32>) -> bool {
        QueueTimeCheck::get()
    }
    fn in_cap_prize(_proposal: &Proposal<AccountId32>) -> Option<Balance> {
        InCapPrize::get()
    }
    fn ledger_frozen() -> bool {
        LedgerFrozen::get()
    }
    fn phase_flags() -> u32 {
        PhaseFlagsValue::get()
    }
    fn active_metric_spec_version() -> MetricSpecVersion {
        ActiveMetricSpecVersion::get()
    }
    fn treasury_gate_required(_proposal: &Proposal<AccountId32>) -> bool {
        TreasuryGateRequired::get()
    }
}

pub struct TestPreimage;
impl PreimageAccess for TestPreimage {
    fn len(hash: H256) -> Option<u32> {
        (PreimageNoted::get()
            || PreimageRequests::get()
                .iter()
                .any(|(candidate, count)| *candidate == hash && *count > 0))
        .then(PreimageLen::get)
        .flatten()
    }
    fn request(hash: H256) -> frame_support::dispatch::DispatchResult {
        if PreimageRequestFails::get() || Self::len(hash).is_none() {
            return Err(DispatchError::Other("mock preimage request failed"));
        }
        PreimageRequests::mutate(|requests| {
            if let Some((_, count)) = requests
                .iter_mut()
                .find(|(candidate, _)| *candidate == hash)
            {
                *count = count.saturating_add(1);
            } else {
                requests.push((hash, 1));
            }
        });
        Ok(())
    }
    fn unrequest(hash: H256) {
        PreimageRequests::mutate(|requests| {
            if let Some((_, count)) = requests
                .iter_mut()
                .find(|(candidate, _)| *candidate == hash)
            {
                *count = count.saturating_sub(1);
            }
            requests.retain(|(_, count)| *count > 0);
        });
    }
}

pub struct TestExecutionGuard;
impl ExecutionGuardAccess for TestExecutionGuard {
    fn enqueue(
        pid: ProposalId,
        payload_hash: H256,
        _version_constraint: Option<RuntimeVersionConstraint>,
        maturity: BlockNumber,
        grace: BlockNumber,
        requires_ratification: bool,
    ) -> frame_support::dispatch::DispatchResult {
        SeamCalls::push(SeamCall::Enqueue {
            pid,
            payload_hash,
            maturity,
            grace,
            requires_ratification,
        })?;
        GuardStateModel::insert(pid, payload_hash)
    }

    fn queue_reject_reason(_pid: ProposalId) -> Option<RejectReason> {
        QueueReject::get()
    }

    fn retry_exhausted(_pid: ProposalId) -> bool {
        RetryExhausted::get()
    }

    fn dequeue_terminal(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        SeamCalls::push(SeamCall::DequeueTerminal(pid))?;
        GuardStateModel::remove(pid);
        Ok(())
    }
}

pub struct TestWelfare;
impl WelfareSettlement for TestWelfare {
    fn compute_settlement(
        cohort_epoch: EpochId,
        spec: MetricSpecVersion,
        target: SettlementTarget,
    ) -> Result<FixedU64, DispatchError> {
        SeamCalls::push(SeamCall::Welfare(cohort_epoch, spec, target))?;
        Ok(WelfareScore::get())
    }
}

pub struct TestLedger;
impl LedgerResolution for TestLedger {
    fn create_vault(
        pid: ProposalId,
        spec: MetricSpecVersion,
    ) -> frame_support::dispatch::DispatchResult {
        SeamCalls::push(SeamCall::CreateVault(pid, spec))
    }
    fn resolve(pid: ProposalId, branch: Branch) -> frame_support::dispatch::DispatchResult {
        SeamCalls::push(SeamCall::Resolve(pid, branch))
    }
    fn void(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        SeamCalls::push(SeamCall::Void(pid))
    }
}

macro_rules! fixed_origin {
    ($name:ident, $account:expr) => {
        pub struct $name;
        impl EnsureOrigin<RuntimeOrigin> for $name {
            type Success = ();

            fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
                let raw: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> =
                    origin.clone().into();
                match raw {
                    Ok(frame_system::RawOrigin::Signed(who)) if who == $account => Ok(()),
                    _ => Err(origin),
                }
            }

            #[cfg(feature = "runtime-benchmarks")]
            fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
                Ok(RuntimeOrigin::signed($account))
            }
        }
    };
}

fixed_origin!(TestGuardianOrigin, guardian());
fixed_origin!(TestExecutionGuardOrigin, execution_guard());
fixed_origin!(TestConstitutionalValuesOrigin, constitutional_values());
fixed_origin!(TestVoidAuthority, void_authority());

impl pallet_epoch::Config for Test {
    type Params = TestParams;
    type Market = TestMarket;
    type Oracle = TestOracle;
    type Guardian = TestGuardian;
    type Attestation = TestAttestation;
    type Constitution = TestConstitution;
    type Preimage = TestPreimage;
    type ExecutionGuard = TestExecutionGuard;
    type Welfare = TestWelfare;
    type Ledger = TestLedger;
    type KeeperRebate = TestKeeperRebate;
    type GuardianOrigin = TestGuardianOrigin;
    type ExecutionGuardOrigin = TestExecutionGuardOrigin;
    type VoidAuthority = TestVoidAuthority;
    type ConstitutionalValuesOrigin = TestConstitutionalValuesOrigin;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl BenchmarkHelper<RuntimeOrigin, AccountId32> for TestBenchmarkHelper {
    fn prime_submit_epoch(_: EpochId) {}
    fn constitutional_values_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(constitutional_values())
    }
    fn guardian_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(guardian())
    }
    fn execution_guard_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(execution_guard())
    }
    fn void_authority_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(void_authority())
    }
    fn account(seed: u8) -> AccountId32 {
        account(seed)
    }
    fn proposal(
        id: ProposalId,
        who: AccountId32,
        now: BlockNumber,
        epoch: EpochId,
    ) -> Proposal<AccountId32> {
        let mut proposal = proposal(id, who, ProposalState::Submitted, epoch, now);
        // The assembled benchmark helper notes the matching distinct 64 KiB
        // preimages. Keep the generic mock benchmark at the same committed
        // length and distinct-per-id hash shape.
        proposal.payload_len = futarchy_primitives::kernel::MAX_BYTES;
        PreimageLen::set(Some(futarchy_primitives::kernel::MAX_BYTES));
        proposal
    }
    fn prime_decision(pid: ProposalId, epoch: EpochId, gates: bool) -> MarketSet {
        MarketGrade::set(true);
        QueueTimeCheck::set(true);
        AttestationQuorate::set(true);
        markets(pid, epoch, gates)
    }
    fn prime_guard_enqueue(_pid: ProposalId) {}
    fn prime_settlement(_epoch: EpochId) {
        WelfareScore::set(FixedU64(500_000_000));
    }
}

pub fn reset_doubles() {
    ParamsValue::set(CoreEpochParams::DEFAULT);
    SeamFailure::set(None);
    MarketGrade::set(true);
    UnavailableMarkets::set(Vec::new());
    UngradedMarkets::set(Vec::new());
    BaselineAvailable::set(true);
    TwapOverrides::set(Vec::new());
    TrailingOverrides::set(Vec::new());
    SpotOverrides::set(Vec::new());
    MeasuredDepth::set(1_000_000_000_000);
    PublishedFlow::set(None);
    SecondInsufficiency::set(false);
    OpenDispute::set(false);
    GuardianHold::set(false);
    DeadManEngaged::set(false);
    ReviewClosed::set(false);
    AttestationQuorate::set(true);
    StaticChecks::set(true);
    QueueTimeCheck::set(true);
    InCapPrize::set(Some(100));
    LedgerFrozen::set(false);
    PhaseFlagsValue::set(0);
    ActiveMetricSpecVersion::set(1);
    TreasuryGateRequired::set(false);
    PreimageLen::set(Some(32));
    PreimageNoted::set(true);
    PreimageRequests::set(Vec::new());
    PreimageRequestFails::set(false);
    QueueReject::set(None);
    RetryExhausted::set(false);
    WelfareScore::set(FixedU64(500_000_000));
    PreviousBaselineTwap::set(None);
    RecordKeeperRebates::set(false);
    KeeperRebates::set(Vec::new());
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    reset_doubles();
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        epoch: pallet_epoch::GenesisConfig {
            index: 0,
            start_block: 0,
            _config: core::marker::PhantomData,
        },
    }
    .build_storage()
    .expect("mock epoch genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| {
        System::set_block_number(1);
        SeamCalls::set(Vec::new());
        GuardStateModel::set(MockGuardState::default());
    });
    ext
}

pub fn set_block(block: BlockNumber) {
    System::set_block_number(block.into());
}

pub fn preimage_request_count(hash: H256) -> u32 {
    PreimageRequests::get()
        .into_iter()
        .find_map(|(candidate, count)| (candidate == hash).then_some(count))
        .unwrap_or(0)
}

/// Model `pallet_preimage::unnote_preimage`: a noted payload cannot be
/// removed while any consumer owns a request reference.
pub fn try_unnote_preimage(hash: H256) -> bool {
    if preimage_request_count(hash) > 0 {
        return false;
    }
    PreimageNoted::set(false);
    true
}

pub fn last_epoch_event() -> Option<Event<Test>> {
    System::events()
        .into_iter()
        .rev()
        .find_map(|record| match record.event {
            RuntimeEvent::Epoch(event) => Some(event),
            _ => None,
        })
}
