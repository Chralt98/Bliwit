use crate::mock::*;
use crate::*;
use epoch_core::{CohortInfo as CoreCohort, EpochParams, Origin as CoreOrigin};
use frame_support::{assert_noop, assert_ok, BoundedVec};
use futarchy_primitives::{
    keeper::CrankClass, phase_offsets, Branch, CohortSummary, DecisionOutcome, EpochPhase,
    ProposalState,
};
use parity_scale_codec::{Compact, Decode, Encode};
use sp_runtime::DispatchError;

fn phase_block(epoch: EpochId, numerator: BlockNumber) -> BlockNumber {
    let length = ParamsValue::get().epoch_length;
    epoch
        .saturating_mul(length)
        .saturating_add(length.saturating_mul(numerator) / phase_offsets::DENOMINATOR)
}

fn tick_batch(pids: Vec<ProposalId>) -> TickBatch {
    BoundedVec::try_from(pids).expect("test tick batch is bounded")
}

fn sync_at(block: BlockNumber) {
    set_block(block);
    assert_ok!(Epoch::tick(
        RuntimeOrigin::signed(keeper()),
        tick_batch(Vec::new()),
    ));
}

fn seed_idle_clock(epoch: EpochId) {
    let mut state = EpochState::new();
    let start = phase_block(epoch, phase_offsets::INTAKE_NUM);
    state.epoch.index = epoch;
    state.epoch.phase = EpochPhase::Intake;
    state.epoch.epoch_start_block = start;
    state.epoch.phase_start_block = start;
    assert_ok!(Epoch::seed(state));
    set_block(start);
}

#[test]
fn tick_drains_xcm_traffic_backlog_without_a_clock_crossing_or_settlement_cohort() {
    new_test_ext().execute_with(|| {
        seed_idle_clock(21);
        WelfareTrafficBacklog::set(vec![0]);
        assert!(Epoch::epoch_state().cohorts.is_empty());
        assert!(WelfareTrafficPrunes::get().is_empty());

        sync_at(phase_block(21, phase_offsets::INTAKE_NUM));

        assert_eq!(EpochOf::<Test>::get().index, 21);
        assert_eq!(WelfareTrafficPrunes::get(), vec![21]);
        assert!(WelfareTrafficBacklog::get().is_empty());
        assert!(SeamCalls::get().is_empty());
    });
}

#[test]
fn tick_fully_drains_a_twenty_one_epoch_backlog_in_eleven_calls() {
    new_test_ext().execute_with(|| {
        seed_idle_clock(41);
        WelfareTrafficBacklog::set((0..21).collect());

        for _ in 0..10 {
            sync_at(phase_block(41, phase_offsets::INTAKE_NUM));
        }
        assert_eq!(WelfareTrafficBacklog::get(), vec![20]);

        sync_at(phase_block(41, phase_offsets::INTAKE_NUM));
        assert!(WelfareTrafficBacklog::get().is_empty());
        assert_eq!(WelfareTrafficPrunes::get().len(), 11);
        assert!(WelfareTrafficPrunes::get().iter().all(|epoch| *epoch == 41));
    });
}

fn decision_state(
    pid: ProposalId,
    class: ProposalClass,
) -> EpochState<sp_core::crypto::AccountId32> {
    let mut state = EpochState::new();
    let mut proposal = live_proposal(pid, ProposalState::Trading, 0);
    proposal.proposer = keeper();
    proposal.class = class;
    proposal.markets = Some(markets(pid, 0, epoch_core::requires_gate_markets(class)));
    proposal.decide_at = 1;
    state.resource_locks = proposal
        .resources
        .iter()
        .copied()
        .map(|resource| (resource, pid))
        .collect();
    state.proposals.push(proposal);
    state
}

fn cohort_state(
    pid: ProposalId,
    epoch: EpochId,
    status: CohortStatus,
) -> EpochState<sp_core::crypto::AccountId32> {
    let mut state = EpochState::new();
    let mut proposal = live_proposal(pid, ProposalState::Measuring, epoch);
    proposal.decision = Some(DecisionOutcome::Adopt);
    state.proposals.push(proposal);
    state.cohorts.push(CoreCohort {
        epoch,
        proposals: vec![pid],
        status,
    });
    state
}

fn callback_state(
    pid: ProposalId,
    proposal_state: ProposalState,
) -> EpochState<sp_core::crypto::AccountId32> {
    let mut state = EpochState::new();
    let mut proposal = live_proposal(pid, proposal_state, 0);
    proposal.proposer = keeper();
    if matches!(
        proposal_state,
        ProposalState::Queued | ProposalState::Suspended
    ) {
        proposal.maturity = Some(1);
        proposal.grace_end = Some(ParamsValue::get().grace[0].saturating_add(1));
        proposal.decision = Some(DecisionOutcome::Adopt);
    }
    state.proposals.push(proposal);
    state
}

fn qualification_owned_state() -> (
    EpochState<sp_core::crypto::AccountId32>,
    Vec<(ProposalId, H256)>,
) {
    let mut state = EpochState::new();
    let mut requests = Vec::new();
    for (pid, proposal_state) in [
        (1, ProposalState::Qualified),
        (2, ProposalState::Trading),
        (3, ProposalState::Extended),
    ] {
        let mut proposal = live_proposal(pid, proposal_state, 0);
        proposal.extended = proposal_state == ProposalState::Extended;
        let hash = proposal.payload_hash;
        requests.push((pid, hash));
        state.proposals.push(proposal);
    }
    state.proposal_id_high_water = 3;
    (state, requests)
}

fn install_qualification_requests(requests: &[(ProposalId, H256)]) {
    for (pid, hash) in requests {
        assert_ok!(<TestPreimage as PreimageAccess>::request(*hash));
        QualificationPreimageRequests::<Test>::insert(pid, *hash);
        assert_eq!(TestPreimageRequests::count(*hash), 1);
    }
}

fn assert_qualification_requests_released(requests: &[(ProposalId, H256)]) {
    for (pid, hash) in requests {
        assert!(!QualificationPreimageRequests::<Test>::contains_key(pid));
        assert_eq!(TestPreimageRequests::count(*hash), 0);
    }
}

#[derive(Default)]
struct DifferentialLedger {
    calls: Vec<SeamCall>,
}

impl CoreLedgerOps<sp_core::crypto::AccountId32> for DifferentialLedger {
    fn create_vault(&mut self, pid: ProposalId, spec: MetricSpecVersion) -> Result<(), CoreError> {
        self.calls.push(SeamCall::CreateVault(pid, spec));
        Ok(())
    }

    fn resolve(&mut self, pid: ProposalId, branch: Branch) -> Result<(), CoreError> {
        self.calls.push(SeamCall::Resolve(pid, branch));
        Ok(())
    }

    fn void(&mut self, pid: ProposalId) -> Result<(), CoreError> {
        self.calls.push(SeamCall::Void(pid));
        Ok(())
    }
}

#[derive(Default)]
struct DifferentialWelfare {
    calls: Vec<SeamCall>,
}

impl CoreWelfareOps for DifferentialWelfare {
    fn compute_settlement(
        &mut self,
        cohort_epoch: EpochId,
        spec: MetricSpecVersion,
        target: SettlementTarget,
    ) -> Result<FixedU64, CoreError> {
        self.calls
            .push(SeamCall::Welfare(cohort_epoch, spec, target));
        Ok(WelfareScore::get())
    }

    fn settle_baseline_void(&mut self, cohort_epoch: EpochId) -> Result<(), CoreError> {
        self.calls.push(SeamCall::WelfareVoidBaseline(cohort_epoch));
        Ok(())
    }
}

fn map_core_events(events: &[CoreEvent]) -> Vec<Event<Test>> {
    events
        .iter()
        .filter_map(|event| match event {
            CoreEvent::ProposalSubmitted(pid) => Some(Event::ProposalSubmitted(*pid)),
            CoreEvent::ProposalWithdrawn(pid) => Some(Event::ProposalWithdrawn(*pid)),
            CoreEvent::ScreeningStarted(pid) => Some(Event::ScreeningStarted(*pid)),
            CoreEvent::ProposalCancelled { pid, reason } => Some(Event::ProposalCancelled {
                pid: *pid,
                reason: *reason,
            }),
            CoreEvent::ProposalQualified(pid) => Some(Event::ProposalQualified(*pid)),
            CoreEvent::ProposalDeferred(pid) => Some(Event::ProposalDeferred(*pid)),
            CoreEvent::SlotsShrunk {
                epoch,
                requested,
                funded,
                dropped,
            } => Some(Event::SlotsShrunk {
                epoch: *epoch,
                requested: *requested,
                funded: *funded,
                dropped: dropped.clone(),
            }),
            CoreEvent::MarketsOpened(pid) => Some(Event::MarketsOpened(*pid)),
            CoreEvent::DecisionExtended(pid) => Some(Event::DecisionExtended(*pid)),
            CoreEvent::ProposalQueued {
                pid,
                payload_hash,
                maturity,
            } => Some(Event::ProposalQueued {
                pid: *pid,
                payload_hash: *payload_hash,
                maturity: *maturity,
            }),
            CoreEvent::ProposalRejected { pid, reason } => Some(Event::ProposalRejected {
                pid: *pid,
                reason: *reason,
            }),
            CoreEvent::ProposalDelayed {
                pid,
                justification_hash,
            } => Some(Event::ProposalDelayed {
                pid: *pid,
                justification_hash: *justification_hash,
            }),
            CoreEvent::RerunScheduled(pid) => Some(Event::RerunScheduled(*pid)),
            CoreEvent::RerunOpened(pid) => Some(Event::RerunOpened(*pid)),
            CoreEvent::MandateExpired(pid) => Some(Event::MandateExpired(*pid)),
            CoreEvent::MeasurementStarted { cohort } => {
                Some(Event::MeasurementStarted { cohort: *cohort })
            }
            CoreEvent::CohortSettled { epoch, s } => Some(Event::CohortSettled {
                epoch: *epoch,
                s: *s,
            }),
            CoreEvent::CohortVoided { epoch } => Some(Event::CohortVoided { epoch: *epoch }),
            CoreEvent::BaselineCarried { pid, epoch } => Some(Event::BaselineCarried {
                pid: *pid,
                epoch: *epoch,
            }),
            CoreEvent::ProposalForceRejected { pid, reason } => {
                Some(Event::ProposalForceRejected {
                    pid: *pid,
                    reason: *reason,
                })
            }
            CoreEvent::IntakeSlashed {
                pid,
                reason,
                amount,
            } => Some(Event::IntakeSlashed {
                pid: *pid,
                reason: *reason,
                amount: *amount,
            }),
            CoreEvent::ExecutionFailed { .. } | CoreEvent::NoOp => None,
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum DifferentialDecisionCase {
    Adopt,
    Extend,
    GateVeto,
    SecuritySizing,
    AttestationMissing,
    RateLimited,
    SecondExtensionFailed,
    ConvergenceFailed,
}

fn run_decision_seam_differential(case: DifferentialDecisionCase) {
    new_test_ext().execute_with(|| {
        let (class, expected) = match case {
            DifferentialDecisionCase::Adopt => (ProposalClass::Param, DecisionOutcome::Adopt),
            DifferentialDecisionCase::Extend => (ProposalClass::Param, DecisionOutcome::Extend),
            DifferentialDecisionCase::GateVeto => (
                ProposalClass::Code,
                DecisionOutcome::Reject(RejectReason::GateVetoSurvival),
            ),
            DifferentialDecisionCase::SecuritySizing => (
                ProposalClass::Param,
                DecisionOutcome::Reject(RejectReason::SecuritySizing),
            ),
            DifferentialDecisionCase::AttestationMissing => (
                ProposalClass::Code,
                DecisionOutcome::Reject(RejectReason::AttestationMissing),
            ),
            DifferentialDecisionCase::RateLimited => (
                ProposalClass::Param,
                DecisionOutcome::Reject(RejectReason::RateLimited),
            ),
            DifferentialDecisionCase::SecondExtensionFailed => (
                ProposalClass::Param,
                DecisionOutcome::Reject(RejectReason::SecondExtensionFailed),
            ),
            DifferentialDecisionCase::ConvergenceFailed => (
                ProposalClass::Param,
                DecisionOutcome::Reject(RejectReason::ConvergenceFailed),
            ),
        };
        let mut oracle = decision_state(1, class);
        let books = oracle.proposals[0].markets.expect("decision books exist");
        let mut input = DecisionInputs {
            accept_full: FixedU64(600_000_000),
            reject_full: FixedU64(500_000_000),
            baseline_full: FixedU64(500_000_000),
            accept_trailing: FixedU64(600_000_000),
            reject_trailing: FixedU64(500_000_000),
            baseline_trailing: FixedU64(500_000_000),
            accept_spot: FixedU64(600_000_000),
            reject_spot: FixedU64(500_000_000),
            welfare_grade: WelfareGrade::Ok,
            baseline_grade_ok: true,
            previous_settled_baseline_twap: None,
            survival_grade_ok: true,
            security_grade_ok: true,
            gate_twaps: books.gates.map(|_| [FixedU64(0); 4]),
            measured_depth: MeasuredDepth::get(),
            published_flow_per_day: PublishedFlow::get(),
            in_cap_prize: InCapPrize::get(),
            attestation_quorate: true,
            constitution_queue_ok: true,
        };
        match case {
            DifferentialDecisionCase::Adopt => {}
            DifferentialDecisionCase::Extend => {
                UngradedMarkets::set(vec![books.accept, books.reject]);
                input.welfare_grade = WelfareGrade::Insufficient;
                input.baseline_grade_ok = false;
            }
            DifferentialDecisionCase::GateVeto => {
                let gates = books.gates.expect("gate-veto case has gates");
                TwapOverrides::set(vec![
                    (gates[0], FixedU64(100_000_000)),
                    (gates[1], FixedU64(100_000_000)),
                ]);
                input.gate_twaps = Some([
                    FixedU64(100_000_000),
                    FixedU64(100_000_000),
                    FixedU64(0),
                    FixedU64(0),
                ]);
            }
            DifferentialDecisionCase::SecuritySizing => {
                MeasuredDepth::set(600);
                InCapPrize::set(Some(301));
                input.measured_depth = 600;
                input.in_cap_prize = Some(301);
            }
            DifferentialDecisionCase::AttestationMissing => {
                AttestationQuorate::set(false);
                input.attestation_quorate = false;
            }
            DifferentialDecisionCase::RateLimited => {
                QueueTimeCheck::set(false);
                input.constitution_queue_ok = false;
            }
            DifferentialDecisionCase::SecondExtensionFailed => {
                oracle.proposals[0].state = ProposalState::Extended;
                oracle.proposals[0].extended = true;
                TrailingOverrides::set(vec![(books.accept, FixedU64(500_000_000))]);
                input.accept_trailing = FixedU64(500_000_000);
            }
            DifferentialDecisionCase::ConvergenceFailed => {
                SpotOverrides::set(vec![(books.accept, FixedU64(700_000_000))]);
                input.accept_spot = FixedU64(700_000_000);
            }
        }
        assert_ok!(Epoch::seed(oracle.clone()));
        let mut ledger = DifferentialLedger::default();
        let outcome = oracle
            .decide_with(
                CoreOrigin::Keeper,
                &mut ledger,
                1,
                1,
                input,
                DecisionGuards {
                    preimage_ok: true,
                    resource_locks_held: true,
                    process_hold: false,
                },
                &EpochParams::DEFAULT,
            )
            .expect("core decision scenario is accepted");
        assert_eq!(outcome, expected, "unexpected core outcome for {case:?}");
        if outcome == DecisionOutcome::Extend {
            ledger.calls.push(SeamCall::ExtendMarkets(1));
        } else {
            ledger.calls.push(SeamCall::CloseMarkets(1));
        }
        if outcome == DecisionOutcome::Adopt {
            let queued = oracle
                .proposals
                .iter()
                .find(|proposal| proposal.id == 1)
                .unwrap();
            let maturity = queued.maturity.unwrap();
            ledger.calls.push(SeamCall::Enqueue {
                pid: 1,
                payload_hash: queued.payload_hash,
                maturity,
                grace: queued.grace_end.unwrap().saturating_sub(maturity),
                requires_ratification: matches!(
                    queued.class,
                    ProposalClass::Code | ProposalClass::Meta
                ),
            });
        } else if matches!(outcome, DecisionOutcome::Reject(_)) {
            ledger.calls.push(SeamCall::DequeueTerminal(1));
        }
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        let shell_events = System::events()
            .into_iter()
            .filter_map(|record| match record.event {
                RuntimeEvent::Epoch(event) => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(shell_events, map_core_events(&oracle.events));
        assert_eq!(SeamCalls::get(), ledger.calls);
        oracle.events.clear();
        let shell = Epoch::epoch_state();
        assert_eq!(
            oracle.encode(),
            shell.encode(),
            "state mismatch for {case:?}"
        );
    });
}

fn run_settlement_seam_differential() {
    new_test_ext().execute_with(|| {
        let now = phase_block(3, phase_offsets::HOUSEKEEPING_NUM);
        let initial = cohort_state(1, 0, CohortStatus::Measuring { until_epoch: 2 });
        assert_ok!(Epoch::seed(initial.clone()));
        set_block(now);
        let mut oracle = initial;
        oracle.sync_phase(now);
        let mut welfare = DifferentialWelfare::default();
        for batch in [1, 1] {
            let prior_events = System::events().len();
            oracle.events.clear();
            oracle
                .settle_cohort(
                    CoreOrigin::Keeper,
                    &mut welfare,
                    0,
                    batch,
                    FixedU64(500_000_000),
                    now,
                )
                .expect("core settlement scenario is accepted");
            if !oracle.cohorts.iter().any(|cohort| cohort.epoch == 0) {
                welfare
                    .calls
                    .push(SeamCall::WelfarePrune(oracle.epoch.index));
            }
            assert_ok!(Epoch::settle_cohort(
                RuntimeOrigin::signed(keeper()),
                0,
                batch,
            ));
            let shell_events = System::events()
                .into_iter()
                .skip(prior_events)
                .filter_map(|record| match record.event {
                    RuntimeEvent::Epoch(event) => Some(event),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(shell_events, map_core_events(&oracle.events));
            assert_eq!(SeamCalls::get(), welfare.calls);
            oracle.events.clear();
            assert_eq!(oracle.encode(), Epoch::epoch_state().encode());
        }
    });
}

fn run_t20_void_seam_differential() {
    new_test_ext().execute_with(|| {
        let mut oracle = callback_state(1, ProposalState::Trading);
        assert_ok!(Epoch::seed(oracle.clone()));
        let mut ledger = DifferentialLedger::default();
        oracle
            .force_reject_process_hold(CoreOrigin::GuardianHold, &mut ledger, 1)
            .expect("core T20 scenario is accepted");
        // The pallet wrapper releases any A11 queue entry after the core T20
        // transition (idempotent A8→A11 cleanup). The frame-free oracle has no
        // guard seam, so mirror the expected pallet-level call here — exactly as
        // the qualify/seed differential mirrors `OpenMarkets`.
        ledger.calls.push(SeamCall::DequeueTerminal(1));
        assert_ok!(Epoch::force_reject_process_hold(
            RuntimeOrigin::signed(guardian()),
            1,
        ));
        let shell_events = System::events()
            .into_iter()
            .filter_map(|record| match record.event {
                RuntimeEvent::Epoch(event) => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(shell_events, map_core_events(&oracle.events));
        assert_eq!(SeamCalls::get(), ledger.calls);
        oracle.events.clear();
        // The FRAME persist adapter reaps terminal T20 records immediately.
        oracle
            .proposals
            .retain(|proposal| !matches!(proposal.state, ProposalState::Rejected(_)));
        assert_eq!(oracle.encode(), Epoch::epoch_state().encode());
    });
}

#[test]
fn genesis_uses_the_frozen_three_field_epoch_shape() {
    new_test_ext().execute_with(|| {
        assert_eq!(EpochOf::<Test>::get(), EpochInfo::default());
        assert_eq!(
            Schedule::<Test>::get().length,
            CoreEpochParams::DEFAULT.epoch_length
        );
        assert_eq!(Epoch::current_epoch(), 0);
        assert_eq!(
            <CurrentEpoch<Test> as frame_support::traits::Get<EpochId>>::get(),
            0
        );
        assert_eq!(futarchy_primitives::INTEGRATION_CONTRACT_VERSION, 9);
        assert_ok!(Epoch::do_try_state());
    });
}

/// A chain spec that omits the `epoch` patch section falls back to
/// `GenesisConfig::default()`. Epoch 0 is the reserved pre-launch sentinel that
/// `welfare-core` reads to grant the genesis activation relaxation, so the
/// default must seat the clock on the first *live* epoch instead — otherwise a
/// live `register_spec` would skip the two-epoch activation lead (I-16). The
/// mock deliberately boots at the sentinel to exercise the genesis path, so the
/// default is pinned directly rather than through `new_test_ext`. SQ-82; 05 §4.6.
#[test]
fn default_genesis_config_seats_the_clock_on_the_first_live_epoch() {
    assert_eq!(GenesisConfig::<Test>::default().index, 1);
}

#[test]
fn submit_and_withdraw_cover_happy_and_shape_error_paths() {
    new_test_ext().execute_with(|| {
        let submission = proposal(1, keeper(), ProposalState::Submitted, 0, 1);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            submission.clone()
        ));
        assert_eq!(IntakeQueue::<Test>::get().as_slice(), &[1]);
        assert_eq!(last_epoch_event(), Some(Event::ProposalSubmitted(1)));
        assert_noop!(
            Epoch::withdraw(RuntimeOrigin::signed(nobody()), 1),
            Error::<Test>::BadState
        );
        assert_ok!(Epoch::withdraw(RuntimeOrigin::signed(keeper()), 1));
        assert!(IntakeQueue::<Test>::get().is_empty());
        assert_eq!(
            IntakeProposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Cancelled)
        );
        assert_eq!(last_epoch_event(), Some(Event::ProposalWithdrawn(1)));

        let mut bad = proposal(2, keeper(), ProposalState::Submitted, 0, 1);
        bad.markets = Some(markets(2, 0, false));
        assert_noop!(
            Epoch::submit(RuntimeOrigin::signed(keeper()), bad),
            Error::<Test>::BadProposalShape
        );

        let mut oversized = proposal(2, keeper(), ProposalState::Submitted, 0, 1);
        oversized.payload_len = futarchy_primitives::kernel::MAX_BYTES.saturating_add(1);
        assert_noop!(
            Epoch::submit(RuntimeOrigin::signed(keeper()), oversized),
            Error::<Test>::BadProposalShape
        );
    });
}

#[test]
fn intake_pause_is_origin_gated_bounded_and_lazily_expires() {
    new_test_ext().execute_with(|| {
        set_block(10);
        let until = 20;
        assert_ok!(Epoch::set_intake_paused(
            RuntimeOrigin::signed(void_authority()),
            true,
            until,
        ));
        assert_eq!(IntakePausedUntil::<Test>::get(), Some(until));
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(keeper()),
                proposal(1, keeper(), ProposalState::Submitted, 0, 10),
            ),
            Error::<Test>::IntakePaused
        );

        set_block(until);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, until),
        ));

        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(
                Epoch::set_intake_paused(origin, false, 0),
                DispatchError::BadOrigin
            );
        }
        assert_noop!(
            Epoch::set_intake_paused(
                RuntimeOrigin::signed(void_authority()),
                true,
                until
                    .saturating_add(futarchy_primitives::kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS)
                    .saturating_add(1),
            ),
            Error::<Test>::IntakePauseOutOfBounds
        );
    });
}

#[test]
fn signed_keeper_calls_reject_root_and_none() {
    new_test_ext().execute_with(|| {
        let proposal = proposal(1, keeper(), ProposalState::Submitted, 0, 1);
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(
                Epoch::submit(origin, proposal.clone()),
                DispatchError::BadOrigin
            );
        }
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(Epoch::withdraw(origin, 1), DispatchError::BadOrigin);
        }
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(
                Epoch::tick(origin, tick_batch(Vec::new())),
                DispatchError::BadOrigin
            );
        }
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(Epoch::decide(origin, 1), DispatchError::BadOrigin);
        }
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(Epoch::settle_cohort(origin, 0, 1), DispatchError::BadOrigin);
        }
    });
}

#[test]
fn authority_calls_reject_the_closed_origin_misuse_set() {
    new_test_ext().execute_with(|| {
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(
                Epoch::set_next_epoch_length(origin),
                DispatchError::BadOrigin
            );
        }
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(
                Epoch::delay_once(origin, 1, [1; 32]),
                DispatchError::BadOrigin
            );
        }
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(
                Epoch::force_reject_process_hold(origin, 1),
                DispatchError::BadOrigin
            );
        }
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(Epoch::mark_executed(origin, 1), DispatchError::BadOrigin);
        }
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(
                Epoch::mark_failed_executed(origin, 1),
                DispatchError::BadOrigin
            );
        }
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(
                Epoch::retry_exhausted_to_measurement(origin, 1),
                DispatchError::BadOrigin
            );
        }
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(
                Epoch::expire_or_stale_queue(origin, 1, None),
                DispatchError::BadOrigin
            );
        }
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(Epoch::void_cohort(origin, 0), DispatchError::BadOrigin);
        }
    });
}

#[test]
fn healthy_proposal_is_not_force_rejected_before_frozen_decide_at() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(999, keeper(), ProposalState::Submitted, 0, 1),
        ));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        let decide_at = Proposals::<Test>::get(1)
            .expect("healthy proposal is trading")
            .decide_at;
        for block in [
            1u32.saturating_add(epoch_core::STALE_EPOCH_BOUND),
            decide_at.saturating_sub(1),
        ] {
            set_block(block);
            assert_ok!(Epoch::tick(
                RuntimeOrigin::signed(keeper()),
                tick_batch(vec![1]),
            ));
            assert_eq!(
                Proposals::<Test>::get(1).map(|proposal| proposal.state),
                Some(ProposalState::Trading)
            );
        }
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Void(1))));
    });
}

#[test]
fn stalled_epoch_latches_and_force_rejects_every_affected_proposal() {
    new_test_ext().execute_with(|| {
        for id in 1..=2 {
            assert_ok!(Epoch::submit(
                RuntimeOrigin::signed(account(id as u8)),
                proposal(900 + id, account(id as u8), ProposalState::Submitted, 0, 1,),
            ));
        }
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1, 2]),
        ));
        let first_hash = Proposals::<Test>::get(1)
            .expect("first qualified")
            .payload_hash;
        let second_hash = Proposals::<Test>::get(2)
            .expect("second qualified")
            .payload_hash;
        assert_eq!(preimage_request_count(first_hash), 1);
        assert_eq!(preimage_request_count(second_hash), 1);
        let stale = phase_block(0, phase_offsets::SEED_NUM)
            .saturating_add(epoch_core::STALE_EPOCH_BOUND)
            .saturating_add(1);
        set_block(stale);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(StaleEpochCutoff::<Test>::get(), Some(2));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![2]),
        ));
        assert!(!Proposals::<Test>::contains_key(1));
        assert!(!Proposals::<Test>::contains_key(2));
        assert_eq!(preimage_request_count(first_hash), 0);
        assert_eq!(preimage_request_count(second_hash), 0);
        assert!(!QualificationPreimageRequests::<Test>::contains_key(1));
        assert!(!QualificationPreimageRequests::<Test>::contains_key(2));
        assert_eq!(StaleEpochCutoff::<Test>::get(), None);
        let force_events = System::events()
            .iter()
            .filter(|record| {
                matches!(
                    record.event,
                    RuntimeEvent::Epoch(Event::ProposalForceRejected { .. })
                )
            })
            .count();
        assert_eq!(force_events, 2);
    });
}

#[test]
fn every_prequeue_terminal_path_releases_each_qualification_preimage_request() {
    new_test_ext().execute_with(|| {
        let (state, requests) = qualification_owned_state();
        assert_ok!(Epoch::seed(state));
        install_qualification_requests(&requests);

        let stale = phase_block(0, phase_offsets::SEED_NUM)
            .saturating_add(epoch_core::STALE_EPOCH_BOUND)
            .saturating_add(1);
        set_block(stale);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1, 2, 3]),
        ));

        assert_qualification_requests_released(&requests);
    });

    for state in [
        ProposalState::Qualified,
        ProposalState::Trading,
        ProposalState::Extended,
    ] {
        new_test_ext().execute_with(|| {
            let mut proposal = live_proposal(1, state, 0);
            proposal.extended = state == ProposalState::Extended;
            let hash = proposal.payload_hash;
            let mut epoch_state = EpochState::new();
            epoch_state.proposals.push(proposal);
            epoch_state.proposal_id_high_water = 1;
            assert_ok!(Epoch::seed(epoch_state));
            install_qualification_requests(&[(1, hash)]);

            assert_ok!(Epoch::force_reject_process_hold(
                RuntimeOrigin::signed(guardian()),
                1,
            ));

            assert_qualification_requests_released(&[(1, hash)]);
        });
    }

    new_test_ext().execute_with(|| {
        let (mut state, requests) = qualification_owned_state();
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1, 2, 3],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        assert_ok!(Epoch::seed(state));
        install_qualification_requests(&requests);

        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));

        assert_qualification_requests_released(&requests);
    });
}

#[test]
fn bad_preimage_precedes_unavailable_baseline_input() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
        PreimageLen::set(None);
        UnavailableMarkets::set(vec![baseline(0)]);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::ConstitutionViolation))
        );
    });
}

#[test]
fn gate_veto_precedes_missing_welfare_twap() {
    new_test_ext().execute_with(|| {
        let state = decision_state(1, ProposalClass::Code);
        let books = state.proposals[0].markets.expect("code books exist");
        UnavailableMarkets::set(vec![books.accept]);
        let gates = books.gates.expect("code gates exist");
        TwapOverrides::set(vec![
            (gates[0], FixedU64(100_000_000)),
            (gates[1], FixedU64(100_000_000)),
        ]);
        assert_ok!(Epoch::seed(state));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::GateVetoSurvival))
        );
    });
}

#[test]
fn low_ask_treasury_seeds_four_gate_books_and_survival_vetoes() {
    new_test_ext().execute_with(|| {
        let mut candidate = proposal(999, keeper(), ProposalState::Submitted, 0, 1);
        candidate.class = ProposalClass::Treasury;
        assert_ok!(Epoch::submit(RuntimeOrigin::signed(keeper()), candidate,));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert!(SeamCalls::get().iter().any(|call| matches!(
            call,
            SeamCall::OpenMarkets(1, false, Some(plan)) if plan.gate_b.is_some()
        )));
        let gates = Proposals::<Test>::get(1)
            .and_then(|proposal| proposal.markets)
            .and_then(|books| books.gates)
            .expect("treasury gates were physically deployed at Seed");
        TwapOverrides::set(vec![
            (gates[0], FixedU64(100_000_000)),
            (gates[1], FixedU64(100_000_000)),
        ]);
        sync_at(phase_block(0, phase_offsets::TRADE_NUM));
        sync_at(phase_block(0, phase_offsets::DECIDE_NUM));

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));

        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::GateVetoSurvival))
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { pid: 1, .. })));
    });
}

#[test]
fn low_ask_treasury_security_gate_can_veto() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Treasury);
        state.proposals[0].ask = 1;
        let gates = state.proposals[0]
            .markets
            .and_then(|books| books.gates)
            .expect("Treasury decision fixture has four gate books");
        TwapOverrides::set(vec![
            (gates[0], FixedU64(0)),
            (gates[1], FixedU64(0)),
            (gates[2], FixedU64(100_000_000)),
            (gates[3], FixedU64(100_000_000)),
        ]);
        assert_ok!(Epoch::seed(state));

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));

        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.decision),
            Some(Some(DecisionOutcome::Reject(
                RejectReason::GateVetoSecurity
            )))
        );
    });
}

#[test]
fn param_seeds_four_gate_books_and_both_vetoes_are_reachable() {
    new_test_ext().execute_with(|| {
        let candidate = proposal(999, keeper(), ProposalState::Submitted, 0, 1);
        assert_ok!(Epoch::submit(RuntimeOrigin::signed(keeper()), candidate));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert!(SeamCalls::get().iter().any(|call| matches!(
            call,
            SeamCall::OpenMarkets(1, false, Some(plan)) if plan.gate_b.is_some()
        )));
        let gates = Proposals::<Test>::get(1)
            .and_then(|proposal| proposal.markets)
            .and_then(|books| books.gates)
            .expect("PARAM gates were physically deployed at Seed");
        assert_eq!(gates.len(), 4);
    });

    for (twaps, expected) in [
        (
            [
                FixedU64(100_000_000),
                FixedU64(100_000_000),
                FixedU64(0),
                FixedU64(0),
            ],
            RejectReason::GateVetoSurvival,
        ),
        (
            [
                FixedU64(0),
                FixedU64(0),
                FixedU64(100_000_000),
                FixedU64(100_000_000),
            ],
            RejectReason::GateVetoSecurity,
        ),
    ] {
        new_test_ext().execute_with(|| {
            let state = decision_state(1, ProposalClass::Param);
            let gates = state.proposals[0]
                .markets
                .and_then(|books| books.gates)
                .expect("PARAM decision fixture has four gate books");
            TwapOverrides::set(gates.into_iter().zip(twaps).collect());
            assert_ok!(Epoch::seed(state));

            assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));

            assert_eq!(
                Proposals::<Test>::get(1).and_then(|proposal| proposal.decision),
                Some(DecisionOutcome::Reject(expected)),
            );
        });
    }
}

#[test]
fn r2_2_decide_first_latches_stale_epoch_and_force_rejects() {
    // limit-coverage: StaleEpochBound
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Param);
        state.epoch.phase = EpochPhase::Trade;
        state.epoch.phase_start_block = phase_block(0, phase_offsets::TRADE_NUM);
        state.proposals[0].decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
        state.proposal_id_high_water = 1;
        assert_ok!(Epoch::seed(state));
        let stale = phase_block(0, phase_offsets::DECIDE_NUM)
            .saturating_add(epoch_core::STALE_EPOCH_BOUND)
            .saturating_add(1);
        set_block(stale);

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));

        assert!(!Proposals::<Test>::contains_key(1));
        assert_eq!(StaleEpochCutoff::<Test>::get(), None);
        assert!(SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Void(1))));
        assert_eq!(
            last_epoch_event(),
            Some(Event::ProposalForceRejected {
                pid: 1,
                reason: RejectReason::ProcessHold,
            })
        );
    });
}

#[test]
fn stale_decide_noop_on_already_decided_proposal_never_rebates() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(2, ProposalClass::Param);
        let mut already_decided = live_proposal(1, ProposalState::Measuring, 0);
        already_decided.proposer = keeper();
        already_decided.decision = Some(DecisionOutcome::Adopt);
        state.proposals.insert(0, already_decided);
        state.epoch.phase = EpochPhase::Trade;
        state.epoch.phase_start_block = phase_block(0, phase_offsets::TRADE_NUM);
        state.proposals[1].decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
        state.proposal_id_high_water = 2;
        assert_ok!(Epoch::seed(state));
        RecordKeeperRebates::set(true);
        let stale = phase_block(0, phase_offsets::DECIDE_NUM)
            .saturating_add(epoch_core::STALE_EPOCH_BOUND)
            .saturating_add(1);
        set_block(stale);

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));

        assert_eq!(
            Proposals::<Test>::get(1).and_then(|proposal| proposal.decision),
            Some(DecisionOutcome::Adopt)
        );
        assert!(KeeperRebates::get().is_empty());
    });
}

#[test]
fn frozen_seed_force_reject_does_not_deploy_markets() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        let mut candidate = proposal(1, keeper(), ProposalState::Qualified, 0, 1);
        candidate.decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
        state.proposals.push(candidate);
        assert_ok!(Epoch::seed(state));
        LedgerFrozen::set(true);
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert!(!SeamCalls::get().iter().any(|call| {
            matches!(
                call,
                SeamCall::OpenMarkets(1, _, _) | SeamCall::CreateVault(1, _)
            )
        }));
        assert_eq!(
            last_epoch_event(),
            Some(Event::ProposalForceRejected {
                pid: 1,
                reason: RejectReason::ProcessHold,
            })
        );
    });
}

#[test]
fn proposal_ids_are_monotone_and_never_reused_after_reap() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(999, keeper(), ProposalState::Submitted, 0, 1),
        ));
        assert!(IntakeProposals::<Test>::contains_key(1));
        assert_eq!(NextProposalId::<Test>::get(), 2);
        assert_ok!(Epoch::withdraw(RuntimeOrigin::signed(keeper()), 1));

        set_block(ParamsValue::get().epoch_length);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert!(!IntakeProposals::<Test>::contains_key(1));
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(
                1,
                keeper(),
                ProposalState::Submitted,
                1,
                ParamsValue::get().epoch_length,
            ),
        ));
        assert!(IntakeProposals::<Test>::contains_key(2));
        assert!(!IntakeProposals::<Test>::contains_key(1));
        assert_eq!(NextProposalId::<Test>::get(), 3);
    });
}

#[test]
fn arithmetic_epoch_catchup_archives_every_retained_intermediate_timing() {
    new_test_ext().execute_with(|| {
        let schedule = Schedule::<Test>::get();
        let skipped = RECENT_COHORTS_BOUND.saturating_add(2);
        set_block(schedule.length.saturating_mul(skipped));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert_eq!(EpochOf::<Test>::get().index, skipped);
        let timings = EpochTimings::<Test>::get();
        assert_eq!(timings.len(), RECENT_COHORTS_BOUND as usize);
        let first = skipped.saturating_sub(RECENT_COHORTS_BOUND);
        for (offset, timing) in timings.iter().enumerate() {
            let index = first.saturating_add(offset as EpochId);
            assert_eq!(timing.index, index);
            assert_eq!(timing.start, schedule.length.saturating_mul(index));
            assert_eq!(timing.length, schedule.next_length);
        }
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn qualification_binds_chain_active_metric_spec_version() {
    new_test_ext().execute_with(|| {
        let mut candidate = proposal(999, keeper(), ProposalState::Submitted, 0, 1);
        candidate.metric_spec = 999;
        ActiveMetricSpecVersion::set(7);
        assert_ok!(Epoch::submit(RuntimeOrigin::signed(keeper()), candidate));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.metric_spec),
            Some(7)
        );
        assert_eq!(
            ProposalSchedules::<Test>::get(1).map(|schedule| schedule.metric_spec),
            Some(7)
        );
    });
}

#[test]
fn void_cohort_voids_every_vault_and_reaps_the_terminal_working_set() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        for pid in 1..=2 {
            state
                .proposals
                .push(live_proposal(pid, ProposalState::Measuring, 0));
        }
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1, 2],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        assert_ok!(Epoch::seed(state));
        for pid in 1..=2 {
            let hash = [pid as u8; 32];
            assert_ok!(<TestPreimage as PreimageAccess>::request(hash));
            QualificationPreimageRequests::<Test>::insert(pid, hash);
        }
        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));
        assert!(!Cohorts::<Test>::contains_key(0));
        assert!(!Proposals::<Test>::contains_key(1));
        assert!(!Proposals::<Test>::contains_key(2));
        assert!(RecentCohortSummaries::<Test>::get()
            .iter()
            .any(|summary| summary.epoch == 0 && summary.voided));
        assert_eq!(
            SeamCalls::get()
                .iter()
                .filter(|call| matches!(call, SeamCall::Void(_)))
                .cloned()
                .collect::<Vec<_>>(),
            vec![SeamCall::Void(1), SeamCall::Void(2)]
        );
        assert_eq!(last_epoch_event(), Some(Event::CohortVoided { epoch: 0 }));
        for pid in 1..=2 {
            assert_eq!(preimage_request_count([pid as u8; 32]), 0);
            assert!(!QualificationPreimageRequests::<Test>::contains_key(pid));
        }
    });
}

#[test]
fn sq314_void_cohort_preserves_decided_outcomes_and_rejects_only_undecided() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        let mut adopted = live_proposal(1, ProposalState::Measuring, 0);
        adopted.decision = Some(DecisionOutcome::Adopt);
        let mut rejected = live_proposal(2, ProposalState::Measuring, 0);
        rejected.decision = Some(DecisionOutcome::Reject(RejectReason::HurdleNotMet));
        let undecided = live_proposal(3, ProposalState::Trading, 0);
        state.proposals.extend([adopted, rejected, undecided]);
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1, 2],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        state.proposal_id_high_water = 3;
        assert_ok!(Epoch::seed(state));

        let event_start = System::events().len();
        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));

        assert!(!Cohorts::<Test>::contains_key(0));
        assert!(!Proposals::<Test>::contains_key(1));
        assert!(!Proposals::<Test>::contains_key(2));
        assert!(!Proposals::<Test>::contains_key(3));
        let summary = RecentCohortSummaries::<Test>::get()
            .into_iter()
            .find(|summary| summary.epoch == 0)
            .expect("voided cohort summary");
        assert!(summary.voided);
        assert_eq!(
            summary
                .proposals
                .iter()
                .map(|(pid, _, decision)| (*pid, *decision))
                .collect::<Vec<_>>(),
            vec![
                (1, DecisionOutcome::Adopt),
                (2, DecisionOutcome::Reject(RejectReason::HurdleNotMet)),
                (3, DecisionOutcome::Reject(RejectReason::ProcessHold)),
            ]
        );
        let force_rejections = System::events()
            .into_iter()
            .skip(event_start)
            .filter_map(|record| match record.event {
                RuntimeEvent::Epoch(Event::ProposalForceRejected { pid, reason }) => {
                    Some((pid, reason))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            force_rejections,
            vec![(3, RejectReason::ProcessHold)],
            "already-decided cohort members must emit no force rejection"
        );
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn sq40_undefined_prize_proxy_takes_t10_and_refunds_the_full_bond() {
    new_test_ext().execute_with(|| {
        let state = decision_state(1, ProposalClass::Param);
        let proposer = state.proposals[0].proposer.clone();
        let bond = state.proposals[0].bond;
        assert_ok!(Epoch::seed(state));
        ProposalBonds::<Test>::insert(
            1,
            ProposalBond {
                proposer: proposer.clone(),
                held: bond,
            },
        );
        InCapPrize::set(None);

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));

        let decided = Proposals::<Test>::get(1).expect("rejection enters measurement");
        assert_eq!(decided.state, ProposalState::Measuring);
        assert_eq!(
            decided.decision,
            Some(DecisionOutcome::Reject(RejectReason::SecuritySizing))
        );
        assert!(SeamCalls::get().contains(&SeamCall::Resolve(1, Branch::Reject)));
        assert!(SeamCalls::get().contains(&SeamCall::CloseMarkets(1)));
        assert!(SeamCalls::get().contains(&SeamCall::DequeueTerminal(1)));
        assert!(ResourceLocks::<Test>::get()
            .iter()
            .any(|(_, owner)| *owner == 1));
        assert!(!ProposalBonds::<Test>::contains_key(1));
        assert_eq!(BondReleases::get(), vec![(proposer, bond)]);
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::Epoch(Event::IntakeSlashed { pid: 1, .. })
        )));
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn void_cohort_rejects_non_authority_origin() {
    new_test_ext().execute_with(|| {
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            RuntimeOrigin::signed(nobody()),
        ] {
            assert_noop!(Epoch::void_cohort(origin, 0), DispatchError::BadOrigin);
        }
    });
}

// --------------------------- 03 §2.3/§5.2 · 05 §7(5) epoch-VOID Baseline leg
//
// 05 §7(5): "The one settlement a VOID still performs … Owning transition: the
// cohort-VOID (`void_cohort`) path, **not** T21 and not per-proposal
// `void(pid)` — per-proposal vault voiding is a different VOID and settles no
// Baseline." 03 §5.2 makes the same scoping normative and mandatory. SQ-92.

#[test]
fn sq92_void_cohort_settles_the_voided_epochs_baseline_exactly_once() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        for pid in 1..=2 {
            state
                .proposals
                .push(live_proposal(pid, ProposalState::Measuring, 0));
        }
        // A second, untouched cohort: the Baseline vault is keyed per *epoch*
        // (03 §2.2), so voiding epoch 0 must not settle epoch 1's.
        state
            .proposals
            .push(live_proposal(3, ProposalState::Measuring, 1));
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1, 2],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        state.cohorts.push(CoreCohort {
            epoch: 1,
            proposals: vec![3],
            status: CohortStatus::Measuring { until_epoch: 3 },
        });
        state.proposal_id_high_water = 3;
        assert_ok!(Epoch::seed(state));

        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));

        // Mandatory, exactly once, for exactly the voided epoch — and after the
        // per-proposal vault voids, in the same transaction that sets the
        // cohort status to Void (03 §5.2). Guard-cleanup seam calls are
        // deliberately not constrained here.
        assert_eq!(
            SeamCalls::get()
                .into_iter()
                .filter(|call| matches!(call, SeamCall::Void(_) | SeamCall::WelfareVoidBaseline(_)))
                .collect::<Vec<_>>(),
            vec![
                SeamCall::Void(1),
                SeamCall::Void(2),
                SeamCall::WelfareVoidBaseline(0),
            ]
        );
        assert_eq!(
            Cohorts::<Test>::get(1).map(|cohort| cohort.status),
            Some(CohortStatus::Measuring { until_epoch: 3 })
        );
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn sq92_t20_per_proposal_void_does_not_settle_the_epoch_baseline() {
    // The key scoping guard against over-firing: T20 on a single vault
    // (`void(pid)`) is a *different* VOID and settles no Baseline (03 §5.2,
    // 05 §7(5)). Over-firing here would settle an epoch whose cohort is still
    // measuring and freeze `split_baseline`/`merge_baseline` for everyone else.
    for state in [ProposalState::Trading, ProposalState::Queued] {
        new_test_ext().execute_with(|| {
            assert_ok!(Epoch::seed(callback_state(1, state)));

            assert_ok!(Epoch::force_reject_process_hold(
                RuntimeOrigin::signed(guardian()),
                1,
            ));

            let calls = SeamCalls::get();
            assert!(calls.contains(&SeamCall::Void(1)));
            assert!(!calls
                .iter()
                .any(|call| matches!(call, SeamCall::WelfareVoidBaseline(_))));
            assert_ok!(Epoch::do_try_state());
        });
    }
}

#[test]
fn sq92_void_cohort_fails_closed_when_the_baseline_settlement_fails() {
    // 03 §5.2 enumerates the tolerated no-ops exhaustively (no vault / already
    // `Settled`); a genuine settlement failure is not one of them, so G-1 makes
    // the whole VOID revert rather than record `CohortInfo.status = Void` over
    // an `Open` Baseline vault — the state that strands single-sided holders.
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state
            .proposals
            .push(live_proposal(1, ProposalState::Measuring, 0));
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        state.proposal_id_high_water = 1;
        assert_ok!(Epoch::seed(state));
        SeamFailure::set(Some(SeamCall::WelfareVoidBaseline(0)));
        let before_state = Epoch::epoch_state().encode();
        let before_events = System::events();
        let before_calls = SeamCalls::get();

        assert_noop!(
            Epoch::void_cohort(RuntimeOrigin::signed(void_authority()), 0),
            Error::<Test>::Welfare
        );

        assert_eq!(Epoch::epoch_state().encode(), before_state);
        assert_eq!(System::events(), before_events);
        assert_eq!(SeamCalls::get(), before_calls);
        assert_eq!(
            Cohorts::<Test>::get(0).map(|cohort| cohort.status),
            Some(CohortStatus::Measuring { until_epoch: 2 })
        );
    });
}

/// SQ-320 / 05 §7(6): the orphan the T20 guardian path creates, and the crank
/// that repairs it. `force_reject_process_hold` against a one-proposal epoch is
/// the shortest trigger — it terminates the proposal *before* `Measuring`, so
/// `CohortInfo` is never written and §7(5)'s VOID has nothing to fire on.
#[test]
fn sq320_orphaned_epoch_baseline_is_reachable_and_the_crank_settles_it() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 3;
        state
            .proposals
            .push(live_proposal(1, ProposalState::Trading, 3));
        state.proposal_id_high_water = 1;
        assert_ok!(Epoch::seed(state));

        assert_ok!(Epoch::force_reject_process_hold(
            RuntimeOrigin::signed(guardian()),
            1,
        ));
        // The orphan: the proposal is terminal, yet no cohort was ever created,
        // so neither `settle_cohort` nor `void_cohort` can reach the Baseline.
        assert!(!Cohorts::<Test>::contains_key(3));
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::WelfareVoidBaseline(_))));

        // Roll past the epoch (§7(6) condition 1) and crank permissionlessly.
        EpochOf::<Test>::mutate(|clock| clock.index = 4);
        assert_ok!(Epoch::finalize_epoch_baseline(
            RuntimeOrigin::signed(nobody()),
            3,
        ));
        assert!(SeamCalls::get().contains(&SeamCall::WelfareVoidBaseline(3)));
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn sq320_finalize_epoch_baseline_emits_no_epoch_event() {
    // 02 §6 freezes the `pallet-epoch` event schema (X-11d, "full canonical
    // set"). The crank's canonical signal is the ledger's `BaselineSettled`, so
    // adding an epoch event here would force a contract bump.
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 4;
        assert_ok!(Epoch::seed(state));
        let before = System::events().len();

        assert_ok!(Epoch::finalize_epoch_baseline(
            RuntimeOrigin::signed(nobody()),
            3,
        ));

        assert!(System::events()
            .into_iter()
            .skip(before)
            .all(|record| !matches!(record.event, RuntimeEvent::Epoch(_))));
    });
}

#[test]
fn sq320_finalize_epoch_baseline_rebates_general_once_only_for_useful_work() {
    // 08 §6.3: this permissionless crank is outside the closed
    // decision-critical list, so useful work draws from the general tranche.
    // The §7(6) absent/already-settled no-ops stay callable but must never be a
    // rebate-drain surface, however often an adversarial caller repeats them.
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 4;
        assert_ok!(Epoch::seed(state));
        RecordKeeperRebates::set(true);

        for _ in 0..3 {
            assert_ok!(Epoch::finalize_epoch_baseline(
                RuntimeOrigin::signed(nobody()),
                2,
            ));
        }
        assert!(KeeperRebates::get().is_empty());

        OpenBaselineVaults::set(vec![3]);
        assert_ok!(Epoch::finalize_epoch_baseline(
            RuntimeOrigin::signed(nobody()),
            3,
        ));
        assert_eq!(KeeperRebates::get(), vec![(nobody(), CrankClass::General)]);

        for _ in 0..3 {
            assert_ok!(Epoch::finalize_epoch_baseline(
                RuntimeOrigin::signed(nobody()),
                3,
            ));
        }
        assert_eq!(KeeperRebates::get(), vec![(nobody(), CrankClass::General)]);

        OpenBaselineVaults::set(vec![1]);
        SeamFailure::set(Some(SeamCall::WelfareVoidBaseline(1)));
        assert_noop!(
            Epoch::finalize_epoch_baseline(RuntimeOrigin::signed(nobody()), 1),
            Error::<Test>::Welfare
        );
        assert_eq!(KeeperRebates::get(), vec![(nobody(), CrankClass::General)]);
    });
}

#[test]
fn sq320_finalize_epoch_baseline_survives_a_ledger_freeze() {
    // 06 §6.3 exempts settlement calls from PB-LEDGER-FREEZE, and it must: the
    // freeze's own T20 sweep is one broad way an epoch can be orphaned, so a
    // freeze that blocked the repair would guarantee the stranding it exists to
    // contain (05 §7(6)).
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 4;
        assert_ok!(Epoch::seed(state));
        LedgerFrozen::set(true);

        assert_ok!(Epoch::finalize_epoch_baseline(
            RuntimeOrigin::signed(nobody()),
            3,
        ));
        assert!(SeamCalls::get().contains(&SeamCall::WelfareVoidBaseline(3)));
    });
}

#[test]
fn sq320_finalize_epoch_baseline_rejects_root_and_unsigned_origins() {
    // 06 §3.2 authority matrix: permissionless Signed row. `ensure_signed`
    // is the whole gate — no Root and no unsigned surface (G-5, I-10).
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 4;
        assert_ok!(Epoch::seed(state));
        RecordKeeperRebates::set(true);

        assert_noop!(
            Epoch::finalize_epoch_baseline(RuntimeOrigin::root(), 3),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Epoch::finalize_epoch_baseline(RuntimeOrigin::none(), 3),
            DispatchError::BadOrigin
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::WelfareVoidBaseline(_))));
        assert!(KeeperRebates::get().is_empty());
    });
}

#[test]
fn sq320_finalize_epoch_baseline_refuses_a_live_or_cohorted_epoch() {
    // §7(6) conditions 1 and 2, at the dispatch boundary: a premature
    // finalization would settle a Baseline against an epoch whose cohort is
    // still reachable — the one way this path could destroy information the
    // market is still producing.
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 3;
        assert_ok!(Epoch::seed(state));
        RecordKeeperRebates::set(true);
        // Condition 1: the epoch is still live.
        assert_noop!(
            Epoch::finalize_epoch_baseline(RuntimeOrigin::signed(nobody()), 3),
            Error::<Test>::BadState
        );

        // Condition 2: a cohort exists, so §7(5)/T19 owns this Baseline.
        let mut state = EpochState::new();
        state.epoch.index = 4;
        state
            .proposals
            .push(live_proposal(1, ProposalState::Measuring, 3));
        state.cohorts.push(CoreCohort {
            epoch: 3,
            proposals: vec![1],
            status: CohortStatus::Measuring { until_epoch: 5 },
        });
        state.proposal_id_high_water = 1;
        assert_ok!(Epoch::seed(state));
        assert_noop!(
            Epoch::finalize_epoch_baseline(RuntimeOrigin::signed(nobody()), 3),
            Error::<Test>::BadState
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::WelfareVoidBaseline(_))));
        assert!(KeeperRebates::get().is_empty());
    });
}

#[test]
fn t20_emits_exactly_one_epoch_terminal_event() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Trading)));
        let before = System::events().len();
        assert_ok!(Epoch::force_reject_process_hold(
            RuntimeOrigin::signed(guardian()),
            1,
        ));
        let epoch_events = System::events()
            .into_iter()
            .skip(before)
            .filter_map(|record| match record.event {
                RuntimeEvent::Epoch(event) => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            epoch_events,
            vec![Event::ProposalForceRejected {
                pid: 1,
                reason: RejectReason::ProcessHold,
            }]
        );
    });
}

#[test]
fn corrupted_rerun_reopened_deadline_fails_try_state() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(Epoch::delay_once(
            RuntimeOrigin::signed(guardian()),
            1,
            [7; 32],
        ));
        ReviewClosed::set(true);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        let frozen = ProposalSchedules::<Test>::get(1).expect("rerun deadline frozen");
        let reopened = Proposals::<Test>::get(1).expect("rerun opened");
        assert_eq!(frozen.decide_at, reopened.decide_at);
        Proposals::<Test>::mutate(1, |proposal| {
            if let Some(proposal) = proposal {
                proposal.decide_at = proposal.decide_at.saturating_add(1);
            }
        });
        assert!(Epoch::do_try_state().is_err());
    });
}

#[test]
fn low_ask_treasury_without_gate_books_rejects_not_decision_grade() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Treasury);
        state.proposals[0].ask = 1;
        state.proposals[0].markets = Some(markets(1, 0, false));
        assert_ok!(Epoch::seed(state));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.decision),
            Some(Some(DecisionOutcome::Reject(
                RejectReason::NotDecisionGrade
            )))
        );
    });
}

#[test]
fn first_baseline_carry_uses_previous_twap_and_emits_event() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Code);
        state.proposals[0].epoch = 1;
        state.proposals[0].markets = Some(markets(1, 1, true));
        UngradedMarkets::set(vec![baseline(1)]);
        PreviousBaselineTwap::set(Some(FixedU64(500_000_000)));
        assert_ok!(Epoch::seed(state));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(BaselineCarry::<Test>::get(), Some((1, 1)));
        assert!(System::events().iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::Epoch(Event::BaselineCarried { pid: 1, epoch: 1 })
            )
        }));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.decision),
            Some(Some(DecisionOutcome::Adopt))
        );
    });
}

#[test]
fn second_consecutive_baseline_carry_rejects_gate_bearing_class() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Code);
        state.proposals[0].epoch = 2;
        state.proposals[0].markets = Some(markets(1, 2, true));
        state.baseline_carry = Some((1, 1));
        UngradedMarkets::set(vec![baseline(2)]);
        PreviousBaselineTwap::set(Some(FixedU64(500_000_000)));
        assert_ok!(Epoch::seed(state));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(BaselineCarry::<Test>::get(), Some((2, 2)));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
    });
}

#[test]
fn r2_4_older_rerun_cannot_rewind_baseline_carry_streak() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Code);
        state.baseline_carry = Some((6, 1));
        state.proposals[0].epoch = 5;
        state.proposals[0].state = ProposalState::Extended;
        state.proposals[0].rerun = true;
        state.proposals[0].extended = true;
        state.proposals[0].markets = Some(markets(1, 5, true));
        let mut next = state.proposals[0].clone();
        next.id = 2;
        next.epoch = 7;
        next.state = ProposalState::Trading;
        next.rerun = false;
        next.extended = false;
        next.markets = Some(markets(2, 7, true));
        next.resources = futarchy_primitives::BoundedVec::try_from(vec![[2; 8]])
            .expect("one resource is bounded");
        state.resource_locks.push(([2; 8], 2));
        state.proposals.push(next);
        state.proposal_id_high_water = 2;
        UngradedMarkets::set(vec![baseline(5), baseline(7)]);
        PreviousBaselineTwap::set(Some(FixedU64(500_000_000)));
        assert_ok!(Epoch::seed(state));

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(BaselineCarry::<Test>::get(), Some((6, 1)));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 2));

        assert_eq!(BaselineCarry::<Test>::get(), Some((7, 2)));
        assert_eq!(
            Epoch::epoch_state()
                .proposals
                .iter()
                .find(|proposal| proposal.id == 2)
                .and_then(|proposal| proposal.decision),
            Some(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
    });
}

#[test]
fn dead_man_pauses_phase_and_rejects_submission() {
    new_test_ext().execute_with(|| {
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        let frozen = EpochOf::<Test>::get();
        DeadManEngaged::set(true);
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert_eq!(EpochOf::<Test>::get(), frozen);
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(keeper()),
                proposal(
                    999,
                    keeper(),
                    ProposalState::Submitted,
                    0,
                    phase_block(0, phase_offsets::SEED_NUM),
                ),
            ),
            Error::<Test>::BadPhase
        );
    });
}

#[test]
fn recovery_is_exactly_one_proposal_free_epoch() {
    new_test_ext().execute_with(|| {
        let pause_at = phase_block(0, phase_offsets::QUALIFY_NUM);
        set_block(pause_at);
        assert_ok!(Epoch::observe_dead_man(1, false));
        assert_ok!(Epoch::observe_dead_man(
            1_u32.saturating_add(futarchy_primitives::kernel::DEAD_MAN_RELAY_BLOCKS),
            false,
        ));
        assert!(DeadManEngaged::get());
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        let recovery_start = pause_at.saturating_add(100);
        set_block(recovery_start);
        assert_ok!(Epoch::observe_dead_man(
            2_u32.saturating_add(futarchy_primitives::kernel::DEAD_MAN_RELAY_BLOCKS),
            false,
        ));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert!(DeadManEngaged::get());
        assert_eq!(DeadMan::<Test>::get().recovery_epoch, Some(1));
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(keeper()),
                proposal(999, keeper(), ProposalState::Submitted, 1, recovery_start,),
            ),
            Error::<Test>::BadPhase
        );
        let normal_start = recovery_start.saturating_add(ParamsValue::get().epoch_length);
        set_block(normal_start);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert!(!DeadManEngaged::get());
        assert_eq!(DeadMan::<Test>::get().recovery_epoch, None);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(999, keeper(), ProposalState::Submitted, 2, normal_start,),
        ));
    });
}

#[test]
fn fresh_trigger_discards_partial_recovery_and_requires_a_new_full_epoch() {
    new_test_ext().execute_with(|| {
        let pause_at = phase_block(0, phase_offsets::QUALIFY_NUM);
        set_block(pause_at);
        assert_ok!(Epoch::observe_dead_man(1, false));
        let first_stalled_parent =
            1_u32.saturating_add(futarchy_primitives::kernel::DEAD_MAN_RELAY_BLOCKS);
        assert_ok!(Epoch::observe_dead_man(first_stalled_parent, false));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));

        let first_resume = pause_at.saturating_add(100);
        set_block(first_resume);
        assert_ok!(Epoch::observe_dead_man(
            first_stalled_parent.saturating_add(1),
            false,
        ));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        let first_recovery_epoch = DeadMan::<Test>::get().recovery_epoch;
        assert!(first_recovery_epoch.is_some());
        let recovery_length = Schedule::<Test>::get().length;
        let first_recovery_end = first_resume.saturating_add(recovery_length);

        let restall_at = first_resume.saturating_add(10);
        set_block(restall_at);
        let second_stalled_parent = first_stalled_parent
            .saturating_add(1)
            .saturating_add(futarchy_primitives::kernel::DEAD_MAN_RELAY_BLOCKS);
        assert_ok!(Epoch::observe_dead_man(second_stalled_parent, false));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert_eq!(DeadMan::<Test>::get().recovery_epoch, None);
        assert_eq!(DeadMan::<Test>::get().paused_at, Some(restall_at));
        assert!(DeadManEngaged::get());

        let second_resume = restall_at.saturating_add(1);
        set_block(second_resume);
        assert_ok!(Epoch::observe_dead_man(
            second_stalled_parent.saturating_add(1),
            false,
        ));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert_ne!(DeadMan::<Test>::get().recovery_epoch, first_recovery_epoch);
        assert_eq!(Schedule::<Test>::get().epoch_start_block, second_resume);

        set_block(first_recovery_end);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert!(DeadManEngaged::get());

        set_block(second_resume.saturating_add(recovery_length));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert!(!DeadManEngaged::get());
        assert_eq!(DeadMan::<Test>::get().recovery_epoch, None);
    });
}

#[test]
fn r2_3_recovery_epoch_does_not_open_qualified_markets() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 1;
        state.epoch.phase = EpochPhase::Seed;
        state.epoch.phase_start_block = phase_block(0, phase_offsets::SEED_NUM);
        state.recovery_epoch = Some(1);
        state.proposal_id_high_water = 1;
        let mut qualified = proposal(1, keeper(), ProposalState::Qualified, 1, 1);
        qualified.decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
        state.proposals.push(qualified);
        assert_ok!(Epoch::seed(state));
        set_block(phase_block(0, phase_offsets::SEED_NUM));

        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));

        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Qualified)
        );
        assert!(!SeamCalls::get().iter().any(|call| matches!(
            call,
            SeamCall::OpenMarkets(1, _, _) | SeamCall::CreateVault(1, _)
        )));
    });
}

#[test]
fn r2_3_full_prepause_intake_is_not_permanently_stranded_after_recovery() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        state.epoch.index = 1;
        state.recovery_epoch = Some(1);
        state.proposal_id_high_water = MAX_INTAKE_QUEUE as u64;
        for pid in 1..=MAX_INTAKE_QUEUE as u64 {
            state.proposals.push(proposal(
                pid,
                account(pid as u8),
                ProposalState::Submitted,
                0,
                1,
            ));
            state.intake_queue.push(pid);
        }
        assert_ok!(Epoch::seed(state));

        let length = ParamsValue::get().epoch_length;
        sync_at(length);
        assert_eq!(DeadMan::<Test>::get().recovery_epoch, None);
        assert!(Epoch::epoch_state()
            .proposals
            .iter()
            .filter(|proposal| proposal.state == ProposalState::Submitted)
            .all(|proposal| proposal.epoch == 2));

        sync_at(phase_block(1, phase_offsets::QUALIFY_NUM));
        let all = (1..=MAX_INTAKE_QUEUE as u64).collect::<Vec<_>>();
        for chunk in all.chunks(TICK_BATCH_BOUND as usize) {
            assert_ok!(Epoch::tick(
                RuntimeOrigin::signed(keeper()),
                tick_batch(chunk.to_vec()),
            ));
        }

        for boundary in [
            phase_block(1, phase_offsets::SEED_NUM),
            phase_block(1, phase_offsets::TRADE_NUM),
            phase_block(1, phase_offsets::DECIDE_NUM),
            phase_block(1, phase_offsets::HOUSEKEEPING_NUM),
            phase_block(2, phase_offsets::INTAKE_NUM),
            phase_block(2, phase_offsets::QUALIFY_NUM),
        ] {
            sync_at(boundary);
        }
        let rolled = IntakeQueue::<Test>::get().into_inner();
        for chunk in rolled.chunks(TICK_BATCH_BOUND as usize) {
            assert_ok!(Epoch::tick(
                RuntimeOrigin::signed(keeper()),
                tick_batch(chunk.to_vec()),
            ));
        }
        assert!(IntakeQueue::<Test>::get().is_empty());

        for boundary in [
            phase_block(2, phase_offsets::SEED_NUM),
            phase_block(2, phase_offsets::TRADE_NUM),
            phase_block(2, phase_offsets::DECIDE_NUM),
            phase_block(2, phase_offsets::HOUSEKEEPING_NUM),
            phase_block(3, phase_offsets::INTAKE_NUM),
        ] {
            sync_at(boundary);
        }
        let now = phase_block(3, phase_offsets::INTAKE_NUM);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(nobody()),
            proposal(999, nobody(), ProposalState::Submitted, 4, now),
        ));
    });
}

#[test]
fn highest_bond_wins_the_last_qualification_slot() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        for pid in 1..=4 {
            let mut active = proposal(pid, account(pid as u8), ProposalState::Qualified, 0, 1);
            active.decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
            state.proposals.push(active);
        }
        let mut low = proposal(5, account(50), ProposalState::Submitted, 0, 1);
        low.bond = 10;
        let mut high = proposal(6, account(60), ProposalState::Submitted, 0, 1);
        high.bond = 20;
        state.proposals.extend([low, high]);
        state.intake_queue.extend([5, 6]);
        assert_ok!(Epoch::seed(state));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            // Caller order is deliberately low-first; the pallet screens the
            // canonical descending-bond order before assigning the last slot.
            tick_batch(vec![5, 6]),
        ));
        assert_eq!(
            IntakeProposals::<Test>::get(5).map(|proposal| (proposal.state, proposal.epoch)),
            Some((ProposalState::Submitted, 1))
        );
        assert_eq!(
            Proposals::<Test>::get(6).map(|proposal| proposal.state),
            Some(ProposalState::Qualified)
        );
    });
}

fn qualified_seed_state(
    bonds: &[(ProposalId, Balance)],
) -> EpochState<sp_core::crypto::AccountId32> {
    let mut state = EpochState::new();
    state.epoch.phase = EpochPhase::Qualify;
    state.epoch.phase_start_block = phase_block(0, phase_offsets::QUALIFY_NUM);
    for (pid, bond) in bonds {
        let mut qualified = proposal(
            *pid,
            account((*pid % 200) as u8),
            ProposalState::Qualified,
            0,
            1,
        );
        qualified.bond = *bond;
        qualified.decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
        state.resource_locks.extend(
            qualified
                .resources
                .iter()
                .copied()
                .map(|resource| (resource, *pid)),
        );
        state.proposals.push(qualified);
        state.proposal_id_high_water = state.proposal_id_high_water.max(*pid);
    }
    state
}

#[test]
fn pol_budget_shrinks_in_reverse_bond_priority_and_defers_dropped_slots() {
    new_test_ext().execute_with(|| {
        let bonds = [(1, 40), (2, 30), (3, 20), (4, 10)];
        assert_ok!(Epoch::seed(qualified_seed_state(&bonds)));
        let commitment = 11;
        PolCommitments::set(bonds.iter().map(|(pid, _)| (*pid, commitment)).collect());
        PolEpochBudget::set(commitment.saturating_mul(2));
        for (pid, _) in bonds {
            let hash = Proposals::<Test>::get(pid)
                .expect("qualified proposal exists")
                .payload_hash;
            assert_ok!(<TestPreimage as PreimageAccess>::request(hash));
            QualificationPreimageRequests::<Test>::insert(pid, hash);
        }
        set_block(phase_block(0, phase_offsets::SEED_NUM));

        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));

        assert_eq!(
            IntakeProposals::<Test>::get(3).map(|proposal| (proposal.state, proposal.epoch)),
            Some((ProposalState::Submitted, 1))
        );
        assert_eq!(
            IntakeProposals::<Test>::get(4).map(|proposal| (proposal.state, proposal.epoch)),
            Some((ProposalState::Submitted, 1))
        );
        assert_eq!(RolloverCounts::<Test>::get().as_slice(), &[(4, 1), (3, 1)]);
        assert_eq!(IntakeQueue::<Test>::get().as_slice(), &[4, 3]);
        for pid in [3, 4] {
            assert!(!QualificationPreimageRequests::<Test>::contains_key(pid));
        }
        for pid in [1, 2] {
            assert!(QualificationPreimageRequests::<Test>::contains_key(pid));
        }
        assert!(System::events().iter().any(|record| {
            matches!(
                &record.event,
                RuntimeEvent::Epoch(Event::SlotsShrunk {
                    epoch: 0,
                    requested: 4,
                    funded: 2,
                    dropped,
                }) if dropped.as_slice() == [4, 3]
            )
        }));
        assert!(System::events().iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::Epoch(Event::ProposalDeferred(4))
            )
        }));
        assert!(System::events().iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::Epoch(Event::ProposalDeferred(3))
            )
        }));
    });
}

#[test]
fn fitting_pol_slate_is_not_shrunk() {
    new_test_ext().execute_with(|| {
        let bonds = [(1, 20), (2, 10)];
        assert_ok!(Epoch::seed(qualified_seed_state(&bonds)));
        PolCommitments::set(vec![(1, 7), (2, 5)]);
        PolEpochBudget::set(12);
        set_block(phase_block(0, phase_offsets::SEED_NUM));

        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));

        assert!(bonds.iter().all(|(pid, _)| {
            Proposals::<Test>::get(pid)
                .is_some_and(|proposal| proposal.state == ProposalState::Qualified)
        }));
        assert!(!System::events().iter().any(|record| {
            matches!(record.event, RuntimeEvent::Epoch(Event::SlotsShrunk { .. }))
        }));
    });
}

#[test]
fn funded_pol_seed_plan_is_frozen_at_seed_entry() {
    new_test_ext().execute_with(|| {
        let bonds = [(1, 20)];
        let mut state = qualified_seed_state(&bonds);
        state.proposals[0].class = ProposalClass::Treasury;
        assert_ok!(Epoch::seed(state));
        PolCommitments::set(vec![(1, 1)]);
        PolEpochBudget::set(1);
        set_block(phase_block(0, phase_offsets::SEED_NUM));

        // The transition fixes both the funded slot and its predicted gate shape.
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        assert_eq!(
            FundedPolSlots::<Test>::get().as_slice(),
            &[(
                1,
                PolSeedPlan {
                    commitment: 1,
                    decision_b: 1,
                    gate_b: Some(1),
                }
            )]
        );

        // Later NAV movement can change both live projections, but must neither
        // double-charge the slate nor change the books that seeding will create.
        PolEpochBudget::set(0);
        PolCommitments::set(vec![(1, 99)]);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Trading)
        );
        assert!(SeamCalls::get().contains(&SeamCall::OpenMarkets(
            1,
            false,
            Some(PolSeedPlan {
                commitment: 1,
                decision_b: 1,
                gate_b: Some(1),
            }),
        )));
        assert!(!System::events().iter().any(|record| {
            matches!(record.event, RuntimeEvent::Epoch(Event::SlotsShrunk { .. }))
        }));
    });
}

#[test]
fn zero_spendable_nav_fails_static_and_funds_no_pol_slots() {
    new_test_ext().execute_with(|| {
        let bonds = [(1, 20), (2, 10)];
        assert_ok!(Epoch::seed(qualified_seed_state(&bonds)));
        PolCommitments::set(vec![(1, 1), (2, 1)]);
        // The production provider returns zero when reserve health makes
        // spendable NAV zero; the mock injects that conservative result.
        PolEpochBudget::set(0);
        set_block(phase_block(0, phase_offsets::SEED_NUM));

        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1, 2]),
        ));

        assert!(bonds.iter().all(|(pid, _)| {
            IntakeProposals::<Test>::get(pid).is_some_and(|proposal| {
                proposal.state == ProposalState::Submitted && proposal.epoch == 1
            })
        }));
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::OpenMarkets(_, _, _))));
        assert!(System::events().iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::Epoch(Event::SlotsShrunk {
                    requested: 2,
                    funded: 0,
                    ..
                })
            )
        }));
    });
}

#[test]
fn lock_conflict_rolls_once_then_refunds() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        let candidate = proposal(1, keeper(), ProposalState::Submitted, 0, 1);
        let mut owner = proposal(2, account(2), ProposalState::Qualified, 0, 1);
        owner.resources = candidate.resources.clone();
        owner.decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
        state
            .resource_locks
            .push((candidate.resources.as_slice()[0], 2));
        state.proposals.extend([candidate, owner]);
        state.intake_queue.push(1);
        assert_ok!(Epoch::seed(state));

        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(RolloverCounts::<Test>::get().as_slice(), &[(1, 1)]);
        for block in [
            phase_block(0, phase_offsets::SEED_NUM),
            phase_block(0, phase_offsets::TRADE_NUM),
            phase_block(0, phase_offsets::DECIDE_NUM),
            phase_block(0, phase_offsets::HOUSEKEEPING_NUM),
            ParamsValue::get().epoch_length,
            phase_block(1, phase_offsets::QUALIFY_NUM),
        ] {
            set_block(block);
            assert_ok!(Epoch::tick(
                RuntimeOrigin::signed(keeper()),
                tick_batch(Vec::new()),
            ));
        }
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(
            IntakeProposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Cancelled)
        );
        assert!(!IntakeQueue::<Test>::get().contains(&1));
        assert!(RolloverCounts::<Test>::get().is_empty());
        // 05 §2.1 T26 (SQ-166, contract v6): the second deferral is terminal —
        // it cancels with a full refund — so it MUST report a cancellation.
        // Emitting `ProposalDeferred` made event-derived history claim the
        // proposal was still live.
        assert_eq!(
            last_epoch_event(),
            Some(Event::ProposalCancelled {
                pid: 1,
                reason: RejectReason::RolloverExhausted,
            })
        );
    });
}

#[test]
fn r2_5_stale_force_reject_cleans_rollover_and_drains_cutoff() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        let candidate = proposal(1, keeper(), ProposalState::Submitted, 0, 1);
        let mut owner = proposal(2, account(2), ProposalState::Qualified, 0, 1);
        owner.resources = candidate.resources.clone();
        owner.decide_at = phase_block(0, phase_offsets::DECIDE_NUM);
        state
            .resource_locks
            .push((candidate.resources.as_slice()[0], 2));
        state.proposals.extend([candidate, owner]);
        state.intake_queue.push(1);
        state.proposal_id_high_water = 2;
        assert_ok!(Epoch::seed(state));

        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(RolloverCounts::<Test>::get().as_slice(), &[(1, 1)]);

        let stale = phase_block(0, phase_offsets::SEED_NUM)
            .saturating_add(epoch_core::STALE_EPOCH_BOUND)
            .saturating_add(1);
        set_block(stale);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1, 2]),
        ));

        assert!(RolloverCounts::<Test>::get().is_empty());
        assert_eq!(StaleEpochCutoff::<Test>::get(), None);
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn tick_drives_qualify_and_seed_with_bounded_idempotent_items() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, 1),
        ));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1])
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Qualified)
        );
        assert_eq!(last_epoch_event(), Some(Event::ProposalQualified(1)));

        // A second crank in the same state is a benign no-op and emits nothing.
        let events = System::events().len();
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1])
        ));
        assert_eq!(System::events().len(), events);

        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1])
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Trading)
        );
        assert!(SeamCalls::get().contains(&SeamCall::CreateVault(1, 1)));
        assert_eq!(last_epoch_event(), Some(Event::MarketsOpened(1)));
        assert_noop!(
            Epoch::tick(RuntimeOrigin::signed(keeper()), tick_batch(vec![99])),
            Error::<Test>::UnknownProposal
        );
    });
}

#[test]
fn qualification_pins_preimage_and_unnote_is_impossible_until_t9_handoff() {
    new_test_ext().execute_with(|| {
        let hash = [1; 32];
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, 1),
        ));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(QualificationPreimageRequests::<Test>::get(1), Some(hash));
        assert_eq!(preimage_request_count(hash), 1);
        assert!(!try_unnote_preimage(hash));
        assert_eq!(<TestPreimage as PreimageAccess>::len(hash), Some(32));

        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(preimage_request_count(hash), 1);

        let decide_at = phase_block(0, phase_offsets::SEED_NUM);
        Proposals::<Test>::mutate(1, |proposal| {
            proposal.as_mut().expect("trading proposal").decide_at = decide_at;
        });
        ProposalSchedules::<Test>::mutate(1, |schedule| {
            schedule
                .as_mut()
                .expect("frozen proposal schedule")
                .decide_at = decide_at;
        });
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Queued)
        );
        assert_eq!(QualificationPreimageRequests::<Test>::get(1), None);
        assert_eq!(preimage_request_count(hash), 0);
        assert!(GuardStateModel::get().pinned_preimages.contains(&(1, hash)));
    });
}

#[test]
fn qualification_pin_failure_rolls_back_and_remains_keeper_retriable() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, 1),
        ));
        PreimageRequestFails::set(true);
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_noop!(
            Epoch::tick(RuntimeOrigin::signed(keeper()), tick_batch(vec![1])),
            Error::<Test>::BadDecisionInput
        );
        assert_eq!(
            IntakeProposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Submitted)
        );
        assert_eq!(QualificationPreimageRequests::<Test>::get(1), None);
        assert_eq!(preimage_request_count([1; 32]), 0);

        PreimageRequestFails::set(false);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(QualificationPreimageRequests::<Test>::get(1), Some([1; 32]));
    });
}

#[test]
fn prequeue_decision_rejection_releases_qualification_pin() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, 1),
        ));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        QueueTimeCheck::set(false);
        let decide_at = Proposals::<Test>::get(1)
            .expect("trading proposal")
            .decide_at;
        set_block(decide_at);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(preimage_request_count([1; 32]), 0);
        assert_eq!(QualificationPreimageRequests::<Test>::get(1), None);
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });
}

#[test]
fn keeper_rebate_is_exactly_once_for_useful_tick_and_zero_for_noop_or_error() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, 1),
        ));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        RecordKeeperRebates::set(true);

        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(
            KeeperRebates::get(),
            vec![(keeper(), CrankClass::DecisionCritical)]
        );

        // Already qualified: a repeated crank performs no useful work.
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_noop!(
            Epoch::tick(RuntimeOrigin::signed(keeper()), tick_batch(vec![99])),
            Error::<Test>::UnknownProposal
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![(keeper(), CrankClass::DecisionCritical)]
        );
    });
}

#[test]
fn tick_cancels_failed_static_checks_and_slashes_intake() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, 1),
        ));
        StaticChecks::set(false);
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1])
        ));
        assert_eq!(
            IntakeProposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Cancelled)
        );
        assert_eq!(
            last_epoch_event(),
            Some(Event::IntakeSlashed {
                pid: 1,
                reason: RejectReason::ConstitutionViolation,
                amount: 10,
            })
        );
    });
}

#[test]
fn tick_rejects_a_legacy_oversized_record_before_qualification() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(1, keeper(), ProposalState::Submitted, 0, 1),
        ));
        let oversized = futarchy_primitives::kernel::MAX_BYTES.saturating_add(1);
        IntakeProposals::<Test>::mutate(1, |proposal| {
            proposal.as_mut().expect("submitted proposal").payload_len = oversized;
        });
        // Even a matching length report cannot make a legacy oversized record
        // admissible: qualification applies the kernel cap before static checks.
        PreimageLen::set(Some(oversized));
        set_block(phase_block(0, phase_offsets::QUALIFY_NUM));

        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1])
        ));
        assert_eq!(
            IntakeProposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Cancelled)
        );
    });
}

#[test]
fn decide_adopts_and_only_then_enqueues() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        let queued = Proposals::<Test>::get(1).expect("adopted proposal remains live");
        assert_eq!(queued.state, ProposalState::Queued);
        assert_eq!(queued.decision, Some(DecisionOutcome::Adopt));
        let calls = SeamCalls::get();
        let close = calls
            .iter()
            .position(|call| matches!(call, SeamCall::CloseMarkets(1)));
        let enqueue = calls
            .iter()
            .position(|call| matches!(call, SeamCall::Enqueue { pid: 1, .. }));
        assert!(close.is_some_and(|close| enqueue.is_some_and(|enqueue| close < enqueue)));
        assert_eq!(
            last_epoch_event(),
            Some(Event::ProposalQueued {
                pid: 1,
                payload_hash: [1; 32],
                maturity: 1u32.saturating_add(ParamsValue::get().timelock[0]),
            })
        );
    });
}

#[test]
fn decide_step_one_and_two_fail_closed_without_enqueue() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
        PreimageLen::set(None);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::ConstitutionViolation))
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
        OpenDispute::set(true);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::ProcessHold))
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });
}

#[test]
fn gate_veto_precedes_a_passing_welfare_margin_i14() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Code)));
        TwapOverrides::set(vec![
            (
                markets(1, 0, true).gates.expect("code proposal has gates")[0],
                FixedU64(100_000_000),
            ),
            (
                markets(1, 0, true).gates.expect("code proposal has gates")[1],
                FixedU64(100_000_000),
            ),
        ]);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::GateVetoSurvival))
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });
}

#[test]
fn survival_veto_precedes_security_gate_invalidity_and_keeps_the_intake_bond() {
    // 05 §5.4 steps 3-4 run per gate: Survival's validity, then Survival's
    // veto, then Security's validity — so a Survival veto is reported even
    // when the Security gate books are invalid. The distinction is economic:
    // NotDecisionGrade slashes 10% of the intake bond (06 §4); a gate veto
    // never does.
    new_test_ext().execute_with(|| {
        let books = markets(1, 0, true);
        let gates = books.gates.expect("code proposal has gates");
        UngradedMarkets::set(vec![gates[2], gates[3]]);
        TwapOverrides::set(vec![
            (gates[0], FixedU64(100_000_000)),
            (gates[1], FixedU64(100_000_000)),
        ]);
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Code)));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::GateVetoSurvival))
        );
        assert!(
            !System::events().iter().any(|record| matches!(
                record.event,
                RuntimeEvent::Epoch(Event::IntakeSlashed { .. })
            )),
            "a gate veto must not slash the intake bond"
        );
    });
}

#[test]
fn survival_gate_invalidity_rejects_not_decision_grade_and_slashes_the_bond() {
    // The step-3 counterpart: an invalid gate book with no preceding veto is
    // Reject(NotDecisionGrade), which slashes 10% of the intake bond (06 §4).
    new_test_ext().execute_with(|| {
        let books = markets(1, 0, true);
        let gates = books.gates.expect("code proposal has gates");
        UngradedMarkets::set(vec![gates[0]]);
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Code)));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::Epoch(Event::IntakeSlashed {
                pid: 1,
                reason: RejectReason::NotDecisionGrade,
                amount: 1,
            })
        )));
    });
}

#[test]
fn first_pass_invalid_welfare_book_rejects_instead_of_extending() {
    // 05 §5.4 step 5: only Grade::Insufficient may spend the single shared
    // extension budget. A first-pass Invalid welfare book (sanity band, POL
    // floor/undisturbed, second stale event, non-convergence) rejects with
    // NotDecisionGrade immediately — and therefore slashes (06 §4) — instead
    // of extending.
    new_test_ext().execute_with(|| {
        let books = markets(1, 0, false);
        WelfareInvalidMarkets::set(vec![books.accept]);
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        let proposal = &Epoch::epoch_state().proposals[0];
        assert_eq!(
            proposal.decision,
            Some(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
        assert!(!proposal.extended, "an Invalid grade must never extend");
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::Epoch(Event::IntakeSlashed {
                pid: 1,
                reason: RejectReason::NotDecisionGrade,
                amount: 1,
            })
        )));
    });
}

#[test]
fn decision_extension_is_once_only_and_keeps_creation_schedule_frozen() {
    // limit-coverage: dec.extension
    new_test_ext().execute_with(|| {
        let state = decision_state(1, ProposalClass::Param);
        let books = state.proposals[0].markets.expect("PARAM books exist");
        UngradedMarkets::set(vec![books.accept, books.reject]);
        assert_ok!(Epoch::seed(state));
        RecordKeeperRebates::set(true);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        let extended = Proposals::<Test>::get(1).expect("extended proposal remains live");
        assert_eq!(extended.state, ProposalState::Extended);
        assert!(extended.extended);
        assert!(SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::ExtendMarkets(1))));
        let frozen = ProposalSchedules::<Test>::get(1).expect("schedule was frozen at creation");
        assert_eq!(frozen.decide_at, 1);
        assert_eq!(
            extended.decide_at,
            1u32.saturating_add(epoch_core::DECISION_EXTENSION)
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![(keeper(), CrankClass::DecisionCritical)]
        );

        // The same window cannot be extended again, and the too-early retry does
        // not earn another rebate.
        assert_noop!(
            Epoch::decide(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::BadPhase
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![(keeper(), CrankClass::DecisionCritical)]
        );

        set_block(extended.decide_at);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
        assert!(SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::CloseMarkets(1))));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (keeper(), CrankClass::DecisionCritical),
                (keeper(), CrankClass::DecisionCritical),
            ]
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });
}

#[test]
fn live_params_flip_changes_the_decision_hurdle() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Param);
        let markets = state.proposals[0]
            .markets
            .expect("decision fixture has markets");
        TwapOverrides::set(vec![(markets.accept, FixedU64(517_000_000))]);
        SpotOverrides::set(vec![(markets.accept, FixedU64(517_000_000))]);
        let mut params = EpochParams::DEFAULT;
        params.delta[0] = FixedU64(20_000_000);
        ParamsValue::set(params);
        state.horizon_k = params.horizon_k;
        assert_ok!(Epoch::seed(state));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Epoch::epoch_state().proposals[0].decision,
            Some(DecisionOutcome::Reject(RejectReason::HurdleNotMet))
        );
    });
}

#[test]
fn market_extension_and_close_registration_failures_are_atomic_g1() {
    new_test_ext().execute_with(|| {
        let state = decision_state(1, ProposalClass::Param);
        let books = state.proposals[0].markets.expect("PARAM books exist");
        UngradedMarkets::set(vec![books.accept, books.reject]);
        assert_ok!(Epoch::seed(state));
        SeamFailure::set(Some(SeamCall::ExtendMarkets(1)));
        let before_state = Epoch::epoch_state().encode();
        let before_events = System::events();
        let before_calls = SeamCalls::get();
        assert_noop!(
            Epoch::decide(RuntimeOrigin::signed(keeper()), 1),
            DispatchError::Other("injected epoch seam failure")
        );
        assert_eq!(Epoch::epoch_state().encode(), before_state);
        assert_eq!(System::events(), before_events);
        assert_eq!(SeamCalls::get(), before_calls);
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
        SeamFailure::set(Some(SeamCall::CloseMarkets(1)));
        let before_state = Epoch::epoch_state().encode();
        let before_events = System::events();
        let before_calls = SeamCalls::get();
        assert_noop!(
            Epoch::decide(RuntimeOrigin::signed(keeper()), 1),
            DispatchError::Other("injected epoch seam failure")
        );
        assert_eq!(Epoch::epoch_state().encode(), before_state);
        assert_eq!(System::events(), before_events);
        assert_eq!(SeamCalls::get(), before_calls);
    });
}

#[test]
fn guardian_delay_rerun_and_t24_veto_paths_work_without_enqueue() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
        assert_ok!(Epoch::delay_once(
            RuntimeOrigin::signed(guardian()),
            1,
            [7; 32],
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Suspended)
        );
        assert!(!GuardStateModel::get().queue.is_empty());
        assert_noop!(
            Epoch::delay_once(RuntimeOrigin::signed(guardian()), 1, [8; 32]),
            Error::<Test>::BadState
        );
        ReviewClosed::set(true);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1])
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Rerun)
        );
        assert_eq!(
            SeamCalls::get()
                .iter()
                .filter(|call| **call == SeamCall::DequeueForRerun(1))
                .count(),
            1,
        );
        let guard = GuardStateModel::get();
        assert!(guard.queue.is_empty());
        assert!(guard.held_resources.is_empty());
        assert!(guard.expedited.is_empty());
        assert_eq!(guard.attestation_bindings.len(), 1);
        assert_eq!(guard.ratifications, vec![1]);
        assert_eq!(guard.pinned_preimages, vec![(1, [1; 32])]);
        set_block(phase_block(0, phase_offsets::SEED_NUM));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1])
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Extended)
        );
        assert_eq!(last_epoch_event(), Some(Event::RerunOpened(1)));
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Suspended)));
        assert_ok!(Epoch::veto_upheld_from_review(1));
        let proposal = Proposals::<Test>::get(1).expect("vetoed proposal enters measurement");
        assert_eq!(proposal.state, ProposalState::Measuring);
        assert_eq!(
            proposal.decision,
            Some(DecisionOutcome::Reject(RejectReason::VetoUpheldByReview))
        );
        assert!(SeamCalls::get().contains(&SeamCall::Resolve(1, Branch::Reject)));
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });
}

#[test]
fn execution_callbacks_cover_t21_t22_and_t23() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(Epoch::expire_or_stale_queue(
            RuntimeOrigin::signed(execution_guard()),
            1,
            Some(RejectReason::StaleQueue),
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        assert!(SeamCalls::get().contains(&SeamCall::Resolve(1, Branch::Reject)));
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(
            1,
            ProposalState::FailedExecuted
        )));
        assert_ok!(Epoch::retry_exhausted_to_measurement(
            RuntimeOrigin::signed(execution_guard()),
            1,
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        assert!(SeamCalls::get().contains(&SeamCall::Resolve(1, Branch::Accept)));
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(
            1,
            ProposalState::FailedExecuted
        )));
        assert_ok!(Epoch::mark_executed(
            RuntimeOrigin::signed(execution_guard()),
            1
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        assert!(SeamCalls::get().contains(&SeamCall::Resolve(1, Branch::Accept)));
    });
}

#[test]
fn dead_man_freezes_live_proposals_instead_of_force_rejecting() {
    // 05 §4.8: an engaged dead-man switch freezes the execution queue and pauses
    // the clock. `tick` is permissionless, so it must never convert a live
    // proposal into a rejection/void — the T20 tick force-reject is the
    // VOID/stale-epoch path only (05 §2.1), never a dead-man liveness outage.
    new_test_ext().execute_with(|| {
        // A queued proposal owns its A11 queue entry and carries a live vault.
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
        DeadManEngaged::set(true);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        // Frozen, not rejected: state unchanged, vault not voided, guard entry
        // retained (the queue freeze), never dequeued.
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Queued)
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Void(_) | SeamCall::DequeueTerminal(_))));
        assert!(!GuardStateModel::get().queue.is_empty());
    });
}

#[test]
fn veto_upheld_releases_the_queued_guard_entry() {
    // A proposal queued (A11 owns its entry), delayed to `Suspended` by the
    // guardian, then vetoed at T24 must release the guard queue entry — otherwise
    // A11 leaks capacity and trips its terminal-entry try-state check.
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
        // T-cycle: the guardian delay is not terminal, so the guard entry
        // deliberately persists through `Suspended`.
        assert_ok!(Epoch::delay_once(
            RuntimeOrigin::signed(guardian()),
            1,
            [7; 32],
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Suspended)
        );
        assert!(!GuardStateModel::get().queue.is_empty());
        // T24: upholding the veto drives it terminal and must dequeue A11.
        assert_ok!(Epoch::veto_upheld_from_review(1));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Measuring)
        );
        assert!(SeamCalls::get().contains(&SeamCall::DequeueTerminal(1)));
        let guard = GuardStateModel::get();
        assert!(guard.queue.is_empty());
        assert!(guard.held_resources.is_empty());
        assert!(guard.expedited.is_empty());
        assert!(guard.attestation_bindings.is_empty());
        assert!(guard.ratifications.is_empty());
        assert!(guard.pinned_preimages.is_empty());
    });
}

#[test]
fn direct_force_reject_process_hold_releases_the_guard_entry() {
    // 05 T20 direct guardian path: force-rejecting a `Queued`/`FailedExecuted`
    // proposal is terminal and must dequeue A11. Unlike the tick/expire wrappers
    // this path previously left the guard entry, preimage pin and locks live.
    for state in [ProposalState::Queued, ProposalState::FailedExecuted] {
        new_test_ext().execute_with(|| {
            assert_ok!(Epoch::seed(callback_state(1, state)));
            assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
            assert_ok!(Epoch::force_reject_process_hold(
                RuntimeOrigin::signed(guardian()),
                1,
            ));
            // Terminal T20 record: force-rejected (ProcessHold), then reaped by the
            // persist adapter — with the A11 entry released in lockstep.
            assert_eq!(
                last_epoch_event(),
                Some(Event::ProposalForceRejected {
                    pid: 1,
                    reason: RejectReason::ProcessHold,
                })
            );
            assert!(Proposals::<Test>::get(1).is_none());
            assert!(SeamCalls::get().contains(&SeamCall::DequeueTerminal(1)));
            let guard = GuardStateModel::get();
            assert!(guard.queue.is_empty());
            assert!(guard.held_resources.is_empty());
            assert!(guard.expedited.is_empty());
            assert!(guard.attestation_bindings.is_empty());
            assert!(guard.ratifications.is_empty());
            assert!(guard.pinned_preimages.is_empty());
        });
    }
}

#[test]
fn r2_tick_t15_t16_and_t22_call_guard_terminal_cleanup() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
        let grace_end = Proposals::<Test>::get(1)
            .and_then(|proposal| proposal.grace_end)
            .expect("queued grace");
        set_block(grace_end.saturating_add(1));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Measuring)
        );
        assert!(SeamCalls::get().contains(&SeamCall::DequeueTerminal(1)));
        let guard = GuardStateModel::get();
        assert!(guard.queue.is_empty());
        assert!(guard.held_resources.is_empty());
        assert!(guard.expedited.is_empty());
        assert!(guard.attestation_bindings.is_empty());
        assert!(guard.ratifications.is_empty());
        assert!(guard.pinned_preimages.is_empty());
        assert_eq!(guard.unpinned_preimages, vec![[1; 32]]);
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
        QueueReject::set(Some(RejectReason::StaleQueue));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Measuring)
        );
        assert!(SeamCalls::get().contains(&SeamCall::DequeueTerminal(1)));
        assert!(GuardStateModel::get().queue.is_empty());
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(
            1,
            ProposalState::FailedExecuted,
        )));
        assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
        RetryExhausted::set(true);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Measuring)
        );
        assert!(SeamCalls::get().contains(&SeamCall::DequeueTerminal(1)));
        assert!(GuardStateModel::get().queue.is_empty());
    });
}

#[test]
fn r2_forty_sequential_t15_expiries_do_not_exhaust_guard_queue() {
    new_test_ext().execute_with(|| {
        for pid in 1..=40 {
            assert_ok!(Epoch::seed(callback_state(pid, ProposalState::Queued)));
            assert_ok!(GuardStateModel::insert(pid, [pid as u8; 32]));
            let grace_end = Proposals::<Test>::get(pid)
                .and_then(|proposal| proposal.grace_end)
                .expect("queued grace");
            set_block(grace_end.saturating_add(1));
            assert_ok!(Epoch::tick(
                RuntimeOrigin::signed(keeper()),
                tick_batch(vec![pid]),
            ));
            assert!(GuardStateModel::get().queue.is_empty());
        }
        assert_eq!(
            SeamCalls::get()
                .iter()
                .filter(|call| matches!(call, SeamCall::DequeueTerminal(_)))
                .count(),
            40
        );
    });
}

#[test]
fn r2_terminal_cleanup_failure_rolls_back_epoch_and_ledger() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(GuardStateModel::prime_full(1, [1; 32]));
        let guard_before = GuardStateModel::get();
        SeamFailure::set(Some(SeamCall::DequeueTerminal(1)));
        assert_noop!(
            Epoch::expire_or_stale_queue(
                RuntimeOrigin::signed(execution_guard()),
                1,
                Some(RejectReason::StaleQueue),
            ),
            Error::<Test>::ExecutionGuard
        );
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.state),
            Some(ProposalState::Queued)
        );
        assert!(SeamCalls::get().is_empty());
        assert_eq!(GuardStateModel::get(), guard_before);
    });
}

#[test]
fn r2_terminal_callbacks_are_benign_after_epoch_already_advanced() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(Epoch::expire_or_stale_queue(
            RuntimeOrigin::signed(execution_guard()),
            1,
            Some(RejectReason::StaleQueue),
        ));
        assert_ok!(Epoch::expire_or_stale_queue(
            RuntimeOrigin::signed(execution_guard()),
            1,
            Some(RejectReason::StaleQueue),
        ));
        assert_eq!(
            SeamCalls::get()
                .iter()
                .filter(|call| **call == SeamCall::DequeueTerminal(1))
                .count(),
            2
        );
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(
            1,
            ProposalState::FailedExecuted,
        )));
        assert_ok!(Epoch::retry_exhausted_to_measurement(
            RuntimeOrigin::signed(execution_guard()),
            1,
        ));
        assert_ok!(Epoch::retry_exhausted_to_measurement(
            RuntimeOrigin::signed(execution_guard()),
            1,
        ));
        assert_eq!(
            SeamCalls::get()
                .iter()
                .filter(|call| **call == SeamCall::DequeueTerminal(1))
                .count(),
            2
        );
    });
}

#[test]
fn failed_execution_event_is_not_surfaced_by_epoch() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        let events = System::events().len();
        assert_ok!(Epoch::mark_failed_executed(
            RuntimeOrigin::signed(execution_guard()),
            1,
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::FailedExecuted)
        );
        assert_eq!(System::events().len(), events);
    });
}

#[test]
fn expiry_and_force_reject_never_enqueue_i15() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(Epoch::expire_or_stale_queue(
            RuntimeOrigin::signed(execution_guard()),
            1,
            None,
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
    });

    new_test_ext().execute_with(|| {
        let mut state = callback_state(1, ProposalState::Trading);
        state.resource_locks.push(([1; 8], 1));
        assert_ok!(Epoch::seed(state));
        assert_ok!(Epoch::force_reject_process_hold(
            RuntimeOrigin::signed(guardian()),
            1,
        ));
        assert!(!Proposals::<Test>::contains_key(1));
        assert!(ResourceLocks::<Test>::get().is_empty());
        assert!(SeamCalls::get().contains(&SeamCall::Void(1)));
        assert_eq!(
            last_epoch_event(),
            Some(Event::ProposalForceRejected {
                pid: 1,
                reason: RejectReason::ProcessHold,
            })
        );
        assert!(!SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { .. })));
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn tick_batch_bound_rejects_the_eleventh_pid_at_scale_call_admission() {
    // limit-coverage: TickBatch
    let mut encoded_call = vec![2u8]; // `tick` call index.
    encoded_call.extend(Compact(futarchy_primitives::kernel::TICK_BATCH + 1).encode());

    let error = crate::Call::<Test>::decode(&mut encoded_call.as_slice())
        .expect_err("a max+1 tick batch must fail SCALE call admission");
    assert_eq!(
        error.to_string(),
        "Could not decode `Call::tick::pids`:\n\tBoundedVec exceeds its limit\n"
    );
}

#[test]
fn settlement_is_cursor_resumable_and_welfare_is_the_only_settlement_seam() {
    // limit-coverage: settle_cohort
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(cohort_state(
            1,
            0,
            CohortStatus::Measuring { until_epoch: 2 },
        )));
        assert_ok!(<TestPreimage as PreimageAccess>::request([1; 32]));
        QualificationPreimageRequests::<Test>::insert(1, [1; 32]);
        set_block(phase_block(3, phase_offsets::HOUSEKEEPING_NUM));
        RecordKeeperRebates::set(true);
        assert_ok!(Epoch::settle_cohort(RuntimeOrigin::signed(keeper()), 0, 1));
        assert_eq!(preimage_request_count([1; 32]), 1);
        assert_eq!(
            Cohorts::<Test>::get(0).map(|c| c.status),
            Some(CohortStatus::Settling { cursor: 1 })
        );
        assert_eq!(
            SeamCalls::get(),
            vec![SeamCall::Welfare(
                0,
                1,
                SettlementTarget::Proposal {
                    pid: 1,
                    has_gate_books: false,
                },
            )]
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![(keeper(), CrankClass::DecisionCritical)]
        );
        assert_ok!(Epoch::settle_cohort(RuntimeOrigin::signed(keeper()), 0, 1));
        assert!(SeamCalls::get().ends_with(&[
            SeamCall::Welfare(0, 1, SettlementTarget::Baseline),
            SeamCall::WelfarePrune(3),
        ]));
        assert!(!Cohorts::<Test>::contains_key(0));
        assert!(!Proposals::<Test>::contains_key(1));
        assert_eq!(preimage_request_count([1; 32]), 0);
        assert!(!QualificationPreimageRequests::<Test>::contains_key(1));
        assert_eq!(RecentCohortSummaries::<Test>::get().len(), 1);
        assert_eq!(
            last_epoch_event(),
            Some(Event::CohortSettled {
                epoch: 0,
                s: FixedU64(500_000_000)
            })
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (keeper(), CrankClass::DecisionCritical),
                (keeper(), CrankClass::DecisionCritical),
            ]
        );
        assert_noop!(
            Epoch::settle_cohort(
                RuntimeOrigin::signed(keeper()),
                0,
                futarchy_primitives::kernel::SETTLE_COHORT_MAX_ITEMS + 1,
            ),
            Error::<Test>::BatchTooLarge
        );
        assert_noop!(
            Epoch::settle_cohort(RuntimeOrigin::signed(keeper()), 0, 0),
            Error::<Test>::BatchTooLarge
        );
        assert_noop!(
            Epoch::settle_cohort(RuntimeOrigin::signed(keeper()), 0, 1),
            Error::<Test>::BadState
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (keeper(), CrankClass::DecisionCritical),
                (keeper(), CrankClass::DecisionCritical),
            ]
        );
    });
}

#[test]
fn next_epoch_length_uses_values_origin_and_live_params() {
    new_test_ext().execute_with(|| {
        let mut params = ParamsValue::get();
        params.epoch_length = epoch_core::MIN_EPOCH_LENGTH;
        ParamsValue::set(params);
        assert_ok!(Epoch::set_next_epoch_length(RuntimeOrigin::signed(
            constitutional_values(),
        )));
        assert_eq!(
            Schedule::<Test>::get().next_length,
            epoch_core::MIN_EPOCH_LENGTH
        );
    });
}

#[test]
fn intake_and_live_proposal_caps_are_enforced() {
    // limit-coverage: IntakeQueue, MaxLiveProposals
    new_test_ext().execute_with(|| {
        for id in 0..MAX_INTAKE_QUEUE as u64 {
            let proposer = account(10u8.saturating_add((id / 4) as u8));
            assert_ok!(Epoch::submit(
                RuntimeOrigin::signed(proposer.clone()),
                proposal(id, proposer, ProposalState::Submitted, 0, 1),
            ));
        }
        let proposer = account(80);
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(proposer.clone()),
                proposal(100, proposer, ProposalState::Submitted, 0, 1),
            ),
            Error::<Test>::IntakeFull
        );
        assert_eq!(IntakeQueue::<Test>::get().len(), MAX_INTAKE_QUEUE);
    });

    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        for id in 0..MAX_LIVE_PROPOSALS as u64 {
            let mut proposal = proposal(id, account(id as u8), ProposalState::Qualified, 0, 1);
            proposal.decide_at = 10;
            state.proposals.push(proposal);
        }
        assert_ok!(Epoch::seed(state));
        let proposer = account(90);
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(proposer.clone()),
                proposal(100, proposer, ProposalState::Submitted, 0, 1),
            ),
            Error::<Test>::TooManyLiveProposals
        );
        assert_eq!(Proposals::<Test>::count(), MAX_LIVE_PROPOSALS as u32);
    });
}

#[test]
fn per_account_intake_cap_survives_withdrawals_and_resets_next_epoch() {
    // limit-coverage: intake.max_acct
    new_test_ext().execute_with(|| {
        for id in 1..=ParamsValue::get().intake_max_per_account as u64 {
            assert_ok!(Epoch::submit(
                RuntimeOrigin::signed(keeper()),
                proposal(id, keeper(), ProposalState::Submitted, 0, 1),
            ));
            assert_ok!(Epoch::withdraw(RuntimeOrigin::signed(keeper()), id));
        }
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(keeper()),
                proposal(99, keeper(), ProposalState::Submitted, 0, 1),
            ),
            Error::<Test>::IntakeFull
        );

        let next_epoch = ParamsValue::get().epoch_length;
        set_block(next_epoch);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new())
        ));
        assert_eq!(EpochOf::<Test>::get().phase_start_block, next_epoch);
        assert_eq!(IntakeProposals::<Test>::count(), 0);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(100, keeper(), ProposalState::Submitted, 1, next_epoch),
        ));
    });
}

#[test]
fn four_non_terminal_cohort_cap_rolls_back_the_fifth_transition() {
    // limit-coverage: MaxSettlingCohorts
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        for epoch in 0..MAX_NON_TERMINAL_COHORTS as u32 {
            let pid = epoch as u64;
            state
                .proposals
                .push(live_proposal(pid, ProposalState::Measuring, epoch));
            state.cohorts.push(CoreCohort {
                epoch,
                proposals: vec![pid],
                status: CohortStatus::Measuring {
                    until_epoch: epoch.saturating_add(2),
                },
            });
        }
        state
            .proposals
            .push(live_proposal(99, ProposalState::Queued, 99));
        assert_ok!(Epoch::seed(state));
        let before = Epoch::epoch_state().encode();
        let calls = SeamCalls::get();
        assert_noop!(
            Epoch::mark_executed(RuntimeOrigin::signed(execution_guard()), 99),
            Error::<Test>::TooManyCohorts
        );
        assert_eq!(Epoch::epoch_state().encode(), before);
        assert_eq!(SeamCalls::get(), calls);
    });
}

#[test]
fn r2_6_four_void_cohorts_do_not_block_a_fifth_non_terminal_cohort() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        for pid in 1..=5 {
            let mut proposal = live_proposal(pid, ProposalState::Queued, pid as u32 - 1);
            proposal.decision = Some(DecisionOutcome::Adopt);
            state.proposals.push(proposal);
        }
        state.proposal_id_high_water = 5;
        assert_ok!(Epoch::seed(state));

        for pid in 1..=4 {
            let epoch = pid as u32 - 1;
            assert_ok!(Epoch::mark_executed(
                RuntimeOrigin::signed(execution_guard()),
                pid,
            ));
            assert_ok!(Epoch::void_cohort(
                RuntimeOrigin::signed(void_authority()),
                epoch,
            ));
        }

        assert_ok!(Epoch::mark_executed(
            RuntimeOrigin::signed(execution_guard()),
            5,
        ));
        assert_eq!(Cohorts::<Test>::count(), 1);
        assert_eq!(
            Cohorts::<Test>::get(4).map(|cohort| cohort.status),
            Some(CohortStatus::Measuring { until_epoch: 6 })
        );
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn forty_void_cohorts_reap_working_storage_and_bound_the_archive_ring() {
    new_test_ext().execute_with(|| {
        for epoch in 0..40 {
            let pid = epoch as u64 + 1;
            let mut state = Epoch::epoch_state();
            let mut proposal = live_proposal(pid, ProposalState::Measuring, epoch);
            proposal.decision = Some(DecisionOutcome::Adopt);
            state.proposals.push(proposal);
            state.cohorts.push(CoreCohort {
                epoch,
                proposals: vec![pid],
                status: CohortStatus::Measuring {
                    until_epoch: epoch.saturating_add(2),
                },
            });
            state.proposal_id_high_water = pid;
            assert_ok!(Epoch::seed(state));
            assert_ok!(Epoch::void_cohort(
                RuntimeOrigin::signed(void_authority()),
                epoch,
            ));
            assert_eq!(Cohorts::<Test>::count(), 0);
            assert_eq!(Proposals::<Test>::count(), 0);
            assert_ok!(Epoch::do_try_state());
        }

        let recent = RecentCohortSummaries::<Test>::get();
        assert_eq!(recent.len(), RECENT_COHORTS);
        assert_eq!(recent.first().map(|summary| summary.epoch), Some(8));
        assert_eq!(recent.last().map(|summary| summary.epoch), Some(39));
        assert!(recent.iter().all(|summary| summary.voided));
    });
}

#[test]
/// 05 §7(4): the preserved population is the cohort's own members. A same-epoch
/// proposal that is decided but still pre-Executed (`Queued`, `FailedExecuted`)
/// is **not** a member and takes T20 — `decision.is_some()` is not the
/// discriminator, because T9 records `Some(Adopt)` on entry to `Queued`.
/// Whether T20 is the right record for that population is SQ-319.
fn sq314_void_cohort_preserves_only_cohort_members_and_t20s_the_rest() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        let mut measuring = live_proposal(1, ProposalState::Measuring, 0);
        measuring.decision = Some(DecisionOutcome::Adopt);
        let mut queued = live_proposal(2, ProposalState::Queued, 0);
        queued.decision = Some(DecisionOutcome::Adopt);
        let mut failed = live_proposal(3, ProposalState::FailedExecuted, 0);
        failed.decision = Some(DecisionOutcome::Adopt);
        state.proposals.extend([measuring, queued, failed]);
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        state.proposal_id_high_water = 3;
        assert_ok!(Epoch::seed(state));
        assert_ok!(GuardStateModel::insert(2, [2; 32]));
        assert_ok!(GuardStateModel::insert(3, [3; 32]));

        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));

        assert!(!Proposals::<Test>::contains_key(2));
        assert!(!Proposals::<Test>::contains_key(3));
        assert!(!ProposalSchedules::<Test>::contains_key(2));
        assert!(!ProposalSchedules::<Test>::contains_key(3));
        assert!(!GuardStateModel::get()
            .queue
            .iter()
            .any(|(pid, _)| matches!(*pid, 2 | 3)));
        assert!(!Cohorts::<Test>::contains_key(0));
        let summary = RecentCohortSummaries::<Test>::get()
            .into_iter()
            .find(|summary| summary.epoch == 0);
        assert!(summary.as_ref().is_some_and(|summary| summary.voided));
        assert_eq!(
            summary.map(|summary| summary.proposals.into_inner()),
            Some(vec![
                // pid 1 is the cohort member: its recorded Adopt survives.
                (1, ProposalClass::Param, DecisionOutcome::Adopt,),
                // pids 2 and 3 are decided but pre-Executed and outside the
                // cohort, so they take T20 rather than carrying their vacated
                // Adopt into the archive.
                (
                    2,
                    ProposalClass::Param,
                    DecisionOutcome::Reject(RejectReason::ProcessHold),
                ),
                (
                    3,
                    ProposalClass::Param,
                    DecisionOutcome::Reject(RejectReason::ProcessHold),
                ),
            ])
        );
        assert!(RecentCohortSummaries::<Test>::get()
            .iter()
            .any(|summary| summary.epoch == 0 && summary.voided));
        assert_eq!(last_epoch_event(), Some(Event::CohortVoided { epoch: 0 }));
        assert_eq!(
            SeamCalls::get()
                .into_iter()
                .filter(|call| matches!(call, SeamCall::Void(_)))
                .collect::<Vec<_>>(),
            vec![SeamCall::Void(1), SeamCall::Void(2), SeamCall::Void(3)]
        );
        assert_eq!(
            SeamCalls::get()
                .into_iter()
                .filter(|call| matches!(call, SeamCall::DequeueTerminal(_)))
                .collect::<Vec<_>>(),
            vec![
                SeamCall::DequeueTerminal(1),
                SeamCall::DequeueTerminal(2),
                SeamCall::DequeueTerminal(3),
            ]
        );
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn void_cohort_does_not_block_on_stale_intake_and_ticks_it_to_t20() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        let mut measuring = live_proposal(1, ProposalState::Measuring, 0);
        measuring.decision = Some(DecisionOutcome::Adopt);
        state.proposals.push(measuring);
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        for pid in 2..=8 {
            state
                .proposals
                .push(live_proposal(pid, ProposalState::Submitted, 0));
            state.intake_queue.push(pid);
        }
        state.proposal_id_high_water = 8;
        assert_ok!(Epoch::seed(state));

        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));
        assert!(!Cohorts::<Test>::contains_key(0));
        assert_eq!(IntakeProposals::<Test>::count(), 7);

        let batch = TickBatch::try_from((2..=8).collect::<Vec<_>>());
        assert!(batch.is_ok());
        if let Ok(batch) = batch {
            assert_ok!(Epoch::tick(RuntimeOrigin::signed(keeper()), batch));
        }
        assert_eq!(IntakeProposals::<Test>::count(), 0);
        assert!(IntakeQueue::<Test>::get().is_empty());
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn recent_summary_ring_evicts_fifo_at_32() {
    // limit-coverage: RecentCohortSummaries ring
    new_test_ext().execute_with(|| {
        let mut state = cohort_state(100, 100, CohortStatus::Measuring { until_epoch: 102 });
        for epoch in 0..RECENT_COHORTS as u32 {
            state.recent.push(CohortSummary {
                epoch,
                s_1e9: FixedU64(epoch.into()),
                baseline_twap_1e9: FixedU64(0),
                proposals: futarchy_primitives::BoundedVec::new(),
                voided: false,
                settled_at: epoch,
            });
        }
        assert_ok!(Epoch::seed(state));
        set_block(phase_block(103, phase_offsets::HOUSEKEEPING_NUM));
        assert_ok!(Epoch::settle_cohort(
            RuntimeOrigin::signed(keeper()),
            100,
            2
        ));
        let recent = RecentCohortSummaries::<Test>::get();
        assert_eq!(recent.len(), RECENT_COHORTS);
        assert_eq!(recent[0].epoch, 1);
        assert_eq!(recent[RECENT_COHORTS - 1].epoch, 100);
    });
}

#[test]
fn try_state_covers_positive_and_corrupted_i16_paths() {
    new_test_ext().execute_with(|| {
        let state = decision_state(1, ProposalClass::Param);
        let request = [(1, state.proposals[0].payload_hash)];
        assert_ok!(Epoch::seed(state));
        install_qualification_requests(&request);
        assert_ok!(Epoch::do_try_state());
        ProposalSchedules::<Test>::mutate(1, |schedule| {
            if let Some(schedule) = schedule {
                schedule.metric_spec = schedule.metric_spec.saturating_add(1);
            }
        });
        assert!(Epoch::do_try_state().is_err());
    });

    new_test_ext().execute_with(|| {
        let value = proposal(1, keeper(), ProposalState::Submitted, 0, 1);
        IntakeProposals::<Test>::insert(2, value);
        assert!(Epoch::do_try_state().is_err());
    });
}

#[test]
fn ledger_and_welfare_failures_are_atomic_g1() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        SeamFailure::set(Some(SeamCall::Resolve(1, Branch::Accept)));
        let before_state = Epoch::epoch_state().encode();
        let before_events = System::events();
        let before_calls = SeamCalls::get();
        assert_noop!(
            Epoch::mark_executed(RuntimeOrigin::signed(execution_guard()), 1),
            Error::<Test>::Ledger
        );
        assert_eq!(Epoch::epoch_state().encode(), before_state);
        assert_eq!(System::events(), before_events);
        assert_eq!(SeamCalls::get(), before_calls);
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(cohort_state(
            1,
            0,
            CohortStatus::Measuring { until_epoch: 2 },
        )));
        set_block(phase_block(3, phase_offsets::HOUSEKEEPING_NUM));
        SeamFailure::set(Some(SeamCall::Welfare(0, 1, SettlementTarget::Baseline)));
        let before_state = Epoch::epoch_state().encode();
        let before_events = System::events();
        let before_calls = SeamCalls::get();
        assert_noop!(
            Epoch::settle_cohort(RuntimeOrigin::signed(keeper()), 0, 2),
            Error::<Test>::Welfare
        );
        assert_eq!(Epoch::epoch_state().encode(), before_state);
        assert_eq!(System::events(), before_events);
        assert_eq!(SeamCalls::get(), before_calls);
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(cohort_state(
            1,
            0,
            CohortStatus::Measuring { until_epoch: 2 },
        )));
        set_block(phase_block(3, phase_offsets::HOUSEKEEPING_NUM));
        SeamFailure::set(Some(SeamCall::WelfarePrune(3)));
        let before_state = Epoch::epoch_state().encode();
        let before_events = System::events();
        let before_calls = SeamCalls::get();
        assert_noop!(
            Epoch::settle_cohort(RuntimeOrigin::signed(keeper()), 0, 2),
            Error::<Test>::Welfare
        );
        assert_eq!(Epoch::epoch_state().encode(), before_state);
        assert_eq!(System::events(), before_events);
        assert_eq!(SeamCalls::get(), before_calls);
    });

    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        for pid in 1..=2 {
            state
                .proposals
                .push(live_proposal(pid, ProposalState::Measuring, 0));
        }
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1, 2],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        state.proposal_id_high_water = 2;
        assert_ok!(Epoch::seed(state));
        SeamFailure::set(Some(SeamCall::Void(2)));
        let before_state = Epoch::epoch_state().encode();
        let before_events = System::events();
        let before_calls = SeamCalls::get();
        assert_noop!(
            Epoch::void_cohort(RuntimeOrigin::signed(void_authority()), 0),
            Error::<Test>::Ledger
        );
        assert_eq!(Epoch::epoch_state().encode(), before_state);
        assert_eq!(System::events(), before_events);
        assert_eq!(SeamCalls::get(), before_calls);
    });
}

#[test]
fn randomized_512_step_shell_core_differential_covers_refactored_seams() {
    new_test_ext().execute_with(|| {
        let mut oracle = EpochState::<sp_core::crypto::AccountId32>::new();
        let params = EpochParams::DEFAULT;
        let mut random = 0x6d2b_79f5u32;
        let mut oracle_ledger = DifferentialLedger::default();

        for step in 0..512u32 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let prior_events = System::events()
                .iter()
                .filter(|record| matches!(record.event, RuntimeEvent::Epoch(_)))
                .count();
            oracle.events.clear();
            let core_before = oracle.clone();
            let calls_before = oracle_ledger.calls.clone();

            let (core_ok, shell_ok) = if step < 128 && random & 3 != 0 {
                let who = account(10u8.saturating_add((random % 24) as u8));
                let candidate = proposal(
                    oracle.proposal_id_high_water.saturating_add(1),
                    who.clone(),
                    ProposalState::Submitted,
                    0,
                    1,
                );
                let core = oracle
                    .submit(CoreOrigin::Signed, candidate.clone(), &params)
                    .is_ok();
                let shell = Epoch::submit(RuntimeOrigin::signed(who), candidate).is_ok();
                (core, shell)
            } else if step < 128 {
                let selectable = oracle
                    .proposals
                    .iter()
                    .filter(|proposal| proposal.state == ProposalState::Submitted)
                    .cloned()
                    .collect::<Vec<_>>();
                let choice = selectable.get((random as usize) % selectable.len().max(1));
                let (pid, who) = choice
                    .map(|proposal| (proposal.id, proposal.proposer.clone()))
                    .unwrap_or((u64::MAX, nobody()));
                let core = oracle.withdraw(CoreOrigin::Signed, pid, &who).is_ok();
                if core {
                    oracle_ledger.calls.push(SeamCall::DequeueTerminal(pid));
                }
                let shell = Epoch::withdraw(RuntimeOrigin::signed(who), pid).is_ok();
                (core, shell)
            } else if step < 256 {
                set_block(phase_block(0, phase_offsets::QUALIFY_NUM));
                let pid = oracle
                    .intake_queue
                    .get((step as usize) % oracle.intake_queue.len().max(1))
                    .copied()
                    .unwrap_or(u64::MAX);
                let core = oracle
                    .tick(
                        CoreOrigin::Keeper,
                        &mut oracle_ledger,
                        pid,
                        phase_block(0, phase_offsets::QUALIFY_NUM),
                        TickInputs::default(),
                        &params,
                    )
                    .is_ok();
                if core
                    && oracle.proposal_view(pid).is_ok_and(|proposal| {
                        matches!(
                            proposal.state,
                            ProposalState::Cancelled
                                | ProposalState::Settled
                                | ProposalState::Rejected(_)
                                | ProposalState::Expired
                        )
                    })
                {
                    oracle_ledger.calls.push(SeamCall::DequeueTerminal(pid));
                }
                let shell =
                    Epoch::tick(RuntimeOrigin::signed(keeper()), tick_batch(vec![pid])).is_ok();
                (core, shell)
            } else {
                set_block(phase_block(0, phase_offsets::SEED_NUM));
                let pid = oracle
                    .proposals
                    .iter()
                    .find(|proposal| proposal.state == ProposalState::Qualified)
                    .or_else(|| oracle.proposals.first())
                    .map_or(u64::MAX, |proposal| proposal.id);
                let target = oracle.proposals.iter().find(|proposal| proposal.id == pid);
                let opening = target
                    .filter(|proposal| proposal.state == ProposalState::Qualified)
                    .map(|proposal| {
                        markets(
                            pid,
                            proposal.epoch,
                            epoch_core::requires_gate_markets(proposal.class),
                        )
                    });
                let seed_plan = target
                    .filter(|proposal| proposal.state == ProposalState::Qualified)
                    .and_then(TestPolBudget::proposal_seed_plan);
                if opening.is_some() {
                    oracle_ledger
                        .calls
                        .push(SeamCall::OpenMarkets(pid, false, seed_plan));
                }
                let core = oracle
                    .tick(
                        CoreOrigin::Keeper,
                        &mut oracle_ledger,
                        pid,
                        phase_block(0, phase_offsets::SEED_NUM),
                        TickInputs {
                            markets: opening,
                            ..TickInputs::default()
                        },
                        &params,
                    )
                    .is_ok();
                if core
                    && oracle.proposal_view(pid).is_ok_and(|proposal| {
                        matches!(
                            proposal.state,
                            ProposalState::Cancelled
                                | ProposalState::Settled
                                | ProposalState::Rejected(_)
                                | ProposalState::Expired
                        )
                    })
                {
                    oracle_ledger.calls.push(SeamCall::DequeueTerminal(pid));
                }
                let shell =
                    Epoch::tick(RuntimeOrigin::signed(keeper()), tick_batch(vec![pid])).is_ok();
                (core, shell)
            };

            if !core_ok {
                oracle = core_before;
                oracle_ledger.calls = calls_before;
            }

            assert_eq!(
                core_ok, shell_ok,
                "acceptance mismatch at differential step {step}"
            );
            let shell_events = System::events()
                .iter()
                .filter_map(|record| match &record.event {
                    RuntimeEvent::Epoch(event) => Some(event.clone()),
                    _ => None,
                })
                .skip(prior_events)
                .collect::<Vec<_>>();
            let expected_events = map_core_events(&oracle.events);
            assert_eq!(
                shell_events, expected_events,
                "event mismatch at step {step}"
            );
            assert_eq!(
                SeamCalls::get(),
                oracle_ledger.calls,
                "seam mismatch at step {step}"
            );

            oracle.events.clear();
            oracle.proposals.sort_by_key(|proposal| proposal.id);
            let mut shell = Epoch::epoch_state();
            shell.proposals.sort_by_key(|proposal| proposal.id);
            assert_eq!(oracle.epoch, shell.epoch, "epoch mismatch at step {step}");
            if oracle.proposals != shell.proposals {
                let mismatch = oracle
                    .proposals
                    .iter()
                    .zip(&shell.proposals)
                    .find(|(core, pallet)| core != pallet)
                    .map(|(core, pallet)| {
                        (
                            core.id,
                            core.state,
                            core.epoch,
                            pallet.id,
                            pallet.state,
                            pallet.epoch,
                        )
                    });
                panic!("proposal mismatch at step {step}: {mismatch:?}");
            }
            assert_eq!(
                oracle.proposals, shell.proposals,
                "proposal mismatch at step {step}"
            );
            assert_eq!(
                oracle.intake_queue, shell.intake_queue,
                "intake mismatch at step {step}"
            );
            assert_eq!(
                oracle.resource_locks, shell.resource_locks,
                "lock mismatch at step {step}"
            );
            assert_eq!(
                oracle.encode(),
                shell.encode(),
                "state mismatch at step {step}"
            );
        }
        assert!(oracle_ledger
            .calls
            .iter()
            .any(|call| matches!(call, SeamCall::OpenMarkets(_, false, Some(_)))));
        assert!(oracle_ledger
            .calls
            .iter()
            .any(|call| matches!(call, SeamCall::CreateVault(_, _))));
    });
    for case in [
        DifferentialDecisionCase::Adopt,
        DifferentialDecisionCase::Extend,
        DifferentialDecisionCase::GateVeto,
        DifferentialDecisionCase::SecuritySizing,
        DifferentialDecisionCase::AttestationMissing,
        DifferentialDecisionCase::RateLimited,
        DifferentialDecisionCase::SecondExtensionFailed,
        DifferentialDecisionCase::ConvergenceFailed,
    ] {
        run_decision_seam_differential(case);
    }
    run_settlement_seam_differential();
    run_t20_void_seam_differential();
}
