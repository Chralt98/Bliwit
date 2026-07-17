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

fn decision_state(
    pid: ProposalId,
    class: ProposalClass,
) -> EpochState<sp_core::crypto::AccountId32> {
    let mut state = EpochState::new();
    let mut proposal = live_proposal(pid, ProposalState::Trading, 0);
    proposal.proposer = keeper();
    proposal.class = class;
    proposal.markets = Some(markets(
        pid,
        0,
        matches!(class, ProposalClass::Code | ProposalClass::Meta),
    ));
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
            welfare_grade_ok: true,
            baseline_grade_ok: true,
            previous_settled_baseline_twap: None,
            welfare_second_insufficient: false,
            gate_grade_ok: true,
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
                MarketGrade::set(false);
                input.welfare_grade_ok = false;
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
        assert_eq!(futarchy_primitives::INTEGRATION_CONTRACT_VERSION, 3);
        assert_ok!(Epoch::do_try_state());
    });
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
            assert_noop!(Epoch::veto_upheld(origin, 1), DispatchError::BadOrigin);
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
        assert!(!QualificationPins::<Test>::contains_key(1));
        assert!(!QualificationPins::<Test>::contains_key(2));
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
fn r2_1_seeded_treasury_gate_veto_survives_live_nav_reclassification() {
    new_test_ext().execute_with(|| {
        let mut candidate = proposal(999, keeper(), ProposalState::Submitted, 0, 1);
        candidate.class = ProposalClass::Treasury;
        TreasuryGateRequired::set(true);
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
        assert!(SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::OpenMarkets(1, false, true))));
        let gates = Proposals::<Test>::get(1)
            .and_then(|proposal| proposal.markets)
            .and_then(|books| books.gates)
            .expect("treasury gates were physically deployed at Seed");
        TwapOverrides::set(vec![
            (gates[0], FixedU64(100_000_000)),
            (gates[1], FixedU64(100_000_000)),
        ]);
        TreasuryGateRequired::set(false);
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
fn void_cohort_voids_every_vault_and_marks_the_cohort_void() {
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
            QualificationPins::<Test>::insert(pid, hash);
        }
        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));
        assert_eq!(
            Cohorts::<Test>::get(0).map(|cohort| cohort.status),
            Some(CohortStatus::Void)
        );
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
            assert!(!QualificationPins::<Test>::contains_key(pid));
        }
    });
}

#[test]
fn r2_6_void_cohort_makes_every_proposal_terminal() {
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

        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));

        for pid in 1..=2 {
            assert_eq!(
                Proposals::<Test>::get(pid).map(|proposal| (proposal.state, proposal.decision)),
                Some((
                    ProposalState::Rejected(RejectReason::ProcessHold),
                    Some(DecisionOutcome::Reject(RejectReason::ProcessHold)),
                ))
            );
        }
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
fn small_treasury_without_required_gate_books_decides() {
    new_test_ext().execute_with(|| {
        let mut state = decision_state(1, ProposalClass::Treasury);
        state.proposals[0].ask = 1;
        state.proposals[0].markets = Some(markets(1, 0, false));
        TreasuryGateRequired::set(false);
        assert_ok!(Epoch::seed(state));
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            Proposals::<Test>::get(1).map(|proposal| proposal.decision),
            Some(Some(DecisionOutcome::Adopt))
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
        DeadManEngaged::set(true);
        set_block(pause_at);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
        DeadManEngaged::set(false);
        let recovery_start = pause_at.saturating_add(100);
        set_block(recovery_start);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(Vec::new()),
        ));
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
        assert_eq!(DeadMan::<Test>::get().recovery_epoch, None);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(keeper()),
            proposal(999, keeper(), ProposalState::Submitted, 2, normal_start,),
        ));
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
            tick_batch(vec![5]),
        ));
        assert_eq!(
            IntakeProposals::<Test>::get(5).map(|proposal| (proposal.state, proposal.epoch)),
            Some((ProposalState::Submitted, 1))
        );
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![6]),
        ));
        assert_eq!(
            Proposals::<Test>::get(6).map(|proposal| proposal.state),
            Some(ProposalState::Qualified)
        );
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
        assert_eq!(last_epoch_event(), Some(Event::ProposalDeferred(1)));
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
        assert_eq!(QualificationPins::<Test>::get(1), Some(hash));
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
        assert_eq!(QualificationPins::<Test>::get(1), None);
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
        assert_eq!(QualificationPins::<Test>::get(1), None);
        assert_eq!(preimage_request_count([1; 32]), 0);

        PreimageRequestFails::set(false);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(keeper()),
            tick_batch(vec![1]),
        ));
        assert_eq!(QualificationPins::<Test>::get(1), Some([1; 32]));
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
        assert_eq!(QualificationPins::<Test>::get(1), None);
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
        assert!(SeamCalls::get()
            .iter()
            .any(|call| matches!(call, SeamCall::Enqueue { pid: 1, .. })));
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
fn decision_extension_is_once_only_and_keeps_creation_schedule_frozen() {
    // limit-coverage: dec.extension
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
        MarketGrade::set(false);
        RecordKeeperRebates::set(true);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(keeper()), 1));
        let extended = Proposals::<Test>::get(1).expect("extended proposal remains live");
        assert_eq!(extended.state, ProposalState::Extended);
        assert!(extended.extended);
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
fn guardian_delay_rerun_and_t24_veto_paths_work_without_enqueue() {
    new_test_ext().execute_with(|| {
        assert_ok!(Epoch::seed(callback_state(1, ProposalState::Queued)));
        assert_ok!(Epoch::delay_once(
            RuntimeOrigin::signed(guardian()),
            1,
            [7; 32],
        ));
        assert_eq!(
            Proposals::<Test>::get(1).map(|p| p.state),
            Some(ProposalState::Suspended)
        );
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
        assert_ok!(Epoch::veto_upheld(RuntimeOrigin::signed(guardian()), 1));
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
        assert_ok!(Epoch::veto_upheld(RuntimeOrigin::signed(guardian()), 1));
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
        QualificationPins::<Test>::insert(1, [1; 32]);
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
        assert_eq!(
            SeamCalls::get().last(),
            Some(&SeamCall::Welfare(0, 1, SettlementTarget::Baseline))
        );
        assert!(!Cohorts::<Test>::contains_key(0));
        assert!(!Proposals::<Test>::contains_key(1));
        assert_eq!(preimage_request_count([1; 32]), 0);
        assert!(!QualificationPins::<Test>::contains_key(1));
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
        assert_eq!(Cohorts::<Test>::count(), 5);
        assert_eq!(
            Cohorts::<Test>::get(4).map(|cohort| cohort.status),
            Some(CohortStatus::Measuring { until_epoch: 6 })
        );
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn r2_6_late_same_epoch_proposal_force_rejects_without_mutating_void_cohort() {
    new_test_ext().execute_with(|| {
        let mut state = EpochState::new();
        let mut measuring = live_proposal(1, ProposalState::Measuring, 0);
        measuring.decision = Some(DecisionOutcome::Adopt);
        let mut queued = live_proposal(2, ProposalState::Queued, 0);
        queued.decision = Some(DecisionOutcome::Adopt);
        state.proposals.extend([measuring, queued]);
        state.cohorts.push(CoreCohort {
            epoch: 0,
            proposals: vec![1],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        state.proposal_id_high_water = 2;
        assert_ok!(Epoch::seed(state));

        assert_ok!(Epoch::void_cohort(
            RuntimeOrigin::signed(void_authority()),
            0,
        ));
        assert_ok!(Epoch::mark_executed(
            RuntimeOrigin::signed(execution_guard()),
            2,
        ));

        assert!(!Proposals::<Test>::contains_key(2));
        assert!(!ProposalSchedules::<Test>::contains_key(2));
        assert_eq!(
            Cohorts::<Test>::get(0).map(|cohort| cohort.proposals.into_inner()),
            Some(vec![1])
        );
        assert_eq!(
            last_epoch_event(),
            Some(Event::ProposalForceRejected {
                pid: 2,
                reason: RejectReason::ProcessHold,
            })
        );
        assert_eq!(
            SeamCalls::get()
                .into_iter()
                .filter(|call| matches!(call, SeamCall::Void(_)))
                .collect::<Vec<_>>(),
            vec![SeamCall::Void(1), SeamCall::Void(2)]
        );
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
        assert_ok!(Epoch::seed(decision_state(1, ProposalClass::Param)));
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
                    .map(|proposal| markets(pid, proposal.epoch, false));
                if opening.is_some() {
                    oracle_ledger
                        .calls
                        .push(SeamCall::OpenMarkets(pid, false, false));
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
            .any(|call| matches!(call, SeamCall::OpenMarkets(_, false, false))));
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
