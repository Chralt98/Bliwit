//! Mock-runtime tests for `pallet-oracle` (15 §4.1): every extrinsic × every
//! error path × origin misuse, limit/boundary coverage, and `try_state`
//! assertions. Behavior is asserted against the **spec** (07 §3–§13, 02 §7.2)
//! and the reviewed `oracle-core` state machine (the differential oracle — the
//! shell's job is to reproduce it), never against incidental implementation.
//!
//! Numeric bond/stake expectations are derived from the core's `round_bond`
//! (the 07 §6.1 formula) and the 13-default core constants, never hand-computed
//! (15 §4.4). Each test is named after the obligation it discharges so coverage
//! against 15 §4.1 stays auditable.

use crate::mock::*;
use crate::pallet::{
    Recomputable, Reporters, ReserveHealth, Rounds, WatchtowerActive, Watchtowers,
};
use crate::{Error, Event};
use frame_support::traits::ConstU32;
use frame_support::{assert_noop, assert_ok, pallet_prelude::DispatchResult, BoundedVec};
use futarchy_primitives::{Balance, EpochId, FixedU64, MetricId, MetricSpecVersion, H256};
use oracle_core::{
    hash_evidence, hash_report, round_bond, RoundState, SettlePath, COMPONENT_VALUE_MAX,
    ORC_EXT_WINDOW_BLOCKS, ORC_REPORTER_STAKE, ORC_ROUNDS, ORC_WINDOW_BLOCKS, RES_PROBE_INTERVAL,
    RES_PROBE_TIMEOUT, WT_STAKE,
};
use sp_runtime::DispatchError;

// ------------------------------------------------------------- fixtures ----

/// The canonical game key, mirroring the 07 §5 worked example
/// ("integrations value 0.62 for epoch 41") and the core's own tests. `V`
/// is in the mock `ExpectedSpecs` default set so `report` accepts it.
const C: MetricId = 7;
const E: EpochId = 41;
const V: MetricSpecVersion = 3;

/// A 32-byte evidence/report hash.
fn h(n: u8) -> H256 {
    [n; 32]
}

/// The raw `[u8; 32]` a signed account collapses to inside the core (the
/// `RoundState.reporter` field is `oracle_core::AccountId`, not `T::AccountId`).
fn raw(n: u8) -> [u8; 32] {
    [n; 32]
}

/// Reported value 0.62 on the 05 §4.4 1e9 component grid (07 §5 example).
fn reported_value() -> FixedU64 {
    FixedU64(620_000_000)
}

/// Challenger counter-value 0.44 on the grid (07 §5 example).
fn counter_value() -> FixedU64 {
    FixedU64(440_000_000)
}

/// The value-scaled round-`r` bond for the mock's `StakeAtRisk` (07 §6.1), read
/// from the core formula so the expectation tracks 13 rather than a literal.
fn bond(round: u8) -> Balance {
    round_bond(StakeAtRiskValue::get(), round).expect("round in 1..=R_max")
}

/// The `recompute_proof` argument type — its `ConstU32` bound is exactly
/// `orc.max_proof_bytes` (07 §9), so `ProofTooLarge` is unreachable via the
/// extrinsic and the boundary is enforced by SCALE decoding of the arg itself.
type ProofArg = BoundedVec<u8, ConstU32<{ crate::MAX_PROOF_BYTES_BOUND }>>;

fn proof_arg(bytes: Vec<u8>) -> ProofArg {
    ProofArg::try_from(bytes).expect("proof within orc.max_proof_bytes")
}

/// A committed-evidence payload whose first eight LE bytes decode to `value`
/// (the `oracle-core` recompute stand-in, 07 §9).
fn proof_for(value: FixedU64) -> Vec<u8> {
    let mut proof = vec![0u8; 24];
    proof[..8].copy_from_slice(&value.0.to_le_bytes());
    proof
}

/// Oracle events deposited since the last `reset_events`, in order.
fn oracle_events() -> Vec<Event<Test>> {
    System::events()
        .into_iter()
        .filter_map(|record| match record.event {
            RuntimeEvent::Oracle(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn register_reporter(n: u8) {
    assert_ok!(Oracle::register_reporter(RuntimeOrigin::signed(acc(n))));
}

fn register_watchtower(n: u8) {
    assert_ok!(Oracle::register_watchtower(RuntimeOrigin::signed(acc(n))));
}

/// Post a report for `(C, epoch, V)` from account `reporter`.
fn do_report(reporter: u8, epoch: EpochId, value: FixedU64, evidence: H256) -> DispatchResult {
    Oracle::report(
        RuntimeOrigin::signed(acc(reporter)),
        C,
        epoch,
        V,
        value,
        evidence,
    )
}

/// Report `(C, epoch, V)` with recomputable evidence and settle it `Recomputed`
/// at `reported_value()` — the cheapest "just needs to be final" settle (07 §9,
/// no watchtowers/crank). Used where the F10 reconciliation replaces an
/// `adjudicate` that only served as a settlement shortcut.
fn settle_recomputed(reporter: u8, epoch: EpochId) {
    assert_ok!(Oracle::note_recomputable(C, V));
    let proof = proof_for(reported_value());
    assert_ok!(Oracle::report(
        RuntimeOrigin::signed(acc(reporter)),
        C,
        epoch,
        V,
        reported_value(),
        hash_evidence(&proof)
    ));
    assert_ok!(Oracle::recompute_proof(
        RuntimeOrigin::signed(acc(reporter)),
        C,
        epoch,
        V,
        proof_arg(proof)
    ));
}

/// Escalate a fresh game for `(C, epoch, V)` to a terminal round-`R_max` dispute
/// carrying a live challenger, so `adjudicate` is admissible (07 §5.3/§5.4;
/// Codex F10). Windows are half-open: challenge at `< deadline`, crank matures at
/// `>= deadline`. Leaves the block at the terminal round's `deadline - 1`.
fn escalate_to_terminal(reporter: u8, challenger: u8, epoch: EpochId) {
    assert_ok!(Oracle::report(
        RuntimeOrigin::signed(acc(reporter)),
        C,
        epoch,
        V,
        reported_value(),
        h(9)
    ));
    for _ in 1..ORC_ROUNDS {
        let d = Rounds::<Test>::get((C, epoch, V))
            .unwrap()
            .challenge_deadline;
        set_block(d - 1);
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(challenger)),
            C,
            epoch,
            V,
            counter_value(),
            h(10)
        ));
        set_block(d);
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 1));
    }
    // The terminal-round challenge that makes the game adjudicable.
    let d = Rounds::<Test>::get((C, epoch, V))
        .unwrap()
        .challenge_deadline;
    set_block(d - 1);
    assert_ok!(Oracle::challenge(
        RuntimeOrigin::signed(acc(challenger)),
        C,
        epoch,
        V,
        counter_value(),
        h(10)
    ));
}

// =========================================================================
// 1. Registries (07 §3 reporters, §4 watchtowers; 02 §7.2 counted maps)
// =========================================================================

#[test]
fn register_reporter_happy_path_emits_and_counts() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_eq!(Reporters::<Test>::count(), 1);
        assert_eq!(
            Reporters::<Test>::get(acc(1)).unwrap().stake,
            ORC_REPORTER_STAKE
        );
        assert_eq!(Reporters::<Test>::get(acc(1)).unwrap().offenses, 0);
        System::assert_has_event(
            Event::ReporterRegistered {
                who: acc(1),
                stake: ORC_REPORTER_STAKE,
            }
            .into(),
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn register_reporter_duplicate_is_already_registered() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_noop!(
            Oracle::register_reporter(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::AlreadyRegistered
        );
        assert_eq!(Reporters::<Test>::count(), 1);
    });
}

#[test]
fn register_reporter_fills_to_bound_then_rejects() {
    // 02 §7.2 / core `MAX_REPORTERS = 64`: the 64 seats fill, the 65th is
    // rejected (I-21 bounded-by-construction), and `try_state` still holds.
    new_test_ext().execute_with(|| {
        for n in 1..=64u8 {
            register_reporter(n);
        }
        assert_eq!(Reporters::<Test>::count(), 64);
        assert_noop!(
            Oracle::register_reporter(RuntimeOrigin::signed(acc(65))),
            Error::<Test>::TooManyReporters
        );
        assert_eq!(Reporters::<Test>::count(), 64);
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn register_watchtower_happy_path_emits_and_counts() {
    new_test_ext().execute_with(|| {
        register_watchtower(1);
        assert_eq!(Watchtowers::<Test>::count(), 1);
        assert_eq!(Watchtowers::<Test>::get(acc(1)).unwrap().stake, WT_STAKE);
        System::assert_has_event(
            Event::WatchtowerRegistered {
                who: acc(1),
                stake: WT_STAKE,
            }
            .into(),
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn register_watchtower_duplicate_is_already_registered() {
    new_test_ext().execute_with(|| {
        register_watchtower(1);
        assert_noop!(
            Oracle::register_watchtower(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::AlreadyRegistered
        );
    });
}

#[test]
fn register_watchtower_fills_to_bound_then_rejects() {
    // 07 §4 `wt.max = 16` seats: the 16 fill, the 17th is rejected.
    new_test_ext().execute_with(|| {
        for n in 1..=16u8 {
            register_watchtower(n);
        }
        assert_eq!(Watchtowers::<Test>::count(), 16);
        assert_noop!(
            Oracle::register_watchtower(RuntimeOrigin::signed(acc(17))),
            Error::<Test>::TooManyWatchtowers
        );
        assert_eq!(Watchtowers::<Test>::count(), 16);
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn deregister_reporter_happy_path_clears_seat() {
    // 07 §3: exit is permitted once every round the reporter participated in
    // is closed; with no open round the seat clears.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(Oracle::deregister_reporter(RuntimeOrigin::signed(acc(1))));
        assert_eq!(Reporters::<Test>::count(), 0);
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn deregister_reporter_with_open_round_is_window_open() {
    // 07 §3: the stake is only returned after the reporter's rounds close; a
    // live round blocks exit.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_noop!(
            Oracle::deregister_reporter(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::WindowOpen
        );
        assert_eq!(Reporters::<Test>::count(), 1);
    });
}

#[test]
fn deregister_reporter_unknown_is_not_registered() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Oracle::deregister_reporter(RuntimeOrigin::signed(acc(9))),
            Error::<Test>::NotRegistered
        );
    });
}

// =========================================================================
// 2. report (07 §5.1 window, §2(4) frozen version, §6.1 value-scaled bond)
// =========================================================================

#[test]
fn report_happy_path_opens_round_with_scaled_bond() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));

        let round = Rounds::<Test>::get((C, E, V)).expect("round populated at the triple key");
        assert_eq!(round.round, 1);
        assert_eq!(round.spec_version, V);
        assert_eq!(round.reporter, raw(1));
        assert_eq!(round.value, reported_value());
        assert_eq!(round.evidence_hash, h(9));
        // 07 §6.2: `B_1 = max(10k, 250 bps × StakeAtRisk)` — the mock's 400k
        // scaled hits the floor exactly (`= 10_000_000_000`).
        assert_eq!(round.bond, bond(1));
        // 07 §5.2: 72 h (`orc.window`) challenge window from the report block.
        assert_eq!(round.challenge_deadline, 1 + ORC_WINDOW_BLOCKS);
        assert!(!round.extended);
        assert!(round.challenger.is_none());

        System::assert_has_event(
            Event::Reported {
                component: C,
                epoch: E,
                round: 1,
                reporter: acc(1),
                value: reported_value(),
                evidence_hash: h(9),
                bond: bond(1),
            }
            .into(),
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn report_unregistered_reporter_is_not_registered() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            do_report(1, E, reported_value(), h(9)),
            Error::<Test>::NotRegistered
        );
    });
}

#[test]
fn report_after_window_close_is_window_closed() {
    // 07 §5.1: no report by the 2-day window close (mock `ReportWindowEnd = 10`).
    new_test_ext().execute_with(|| {
        register_reporter(1);
        set_block(ReportWindowEnd::get() + 1);
        assert_noop!(
            do_report(1, E, reported_value(), h(9)),
            Error::<Test>::WindowClosed
        );
    });
}

#[test]
fn report_wrong_spec_version_is_mismatch() {
    // 07 §2(4)/I-16: a report naming a version other than the cohort's frozen
    // version is invalid at dispatch (mock `ExpectedSpecs = [3]`).
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_noop!(
            Oracle::report(
                RuntimeOrigin::signed(acc(1)),
                C,
                E,
                4,
                reported_value(),
                h(9)
            ),
            Error::<Test>::SpecVersionMismatch
        );
    });
}

#[test]
fn report_with_no_consuming_cohort_is_spec_mismatch() {
    // 07 §2(4): a report against a `(component, epoch)` no live cohort consumes
    // is invalid (the mock's frozen-version set is empty ⇒ no version accepted).
    new_test_ext().execute_with(|| {
        register_reporter(1);
        ExpectedSpecs::set(vec![]);
        assert_noop!(
            do_report(1, E, reported_value(), h(9)),
            Error::<Test>::SpecVersionMismatch
        );
        ExpectedSpecs::set(vec![3]); // restore default for suite isolation
    });
}

#[test]
fn report_duplicate_key_is_already_final_for_live_and_settled() {
    // I-18: a live round, and a settled `(component, epoch, version)`, are both
    // final — a fresh report may not reopen the game or shadow the value.
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::note_recomputable(C, V));
        register_reporter(1);
        let proof = proof_for(reported_value());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&proof)));
        // Live round for the key.
        assert_noop!(
            do_report(1, E, reported_value(), hash_evidence(&proof)),
            Error::<Test>::AlreadyFinal
        );
        // Settle it via recompute (F10 makes adjudicate terminal-only), then a
        // repeat report still refuses (settled key).
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            V,
            proof_arg(proof)
        ));
        assert_noop!(
            do_report(1, E, FixedU64(1), h(9)),
            Error::<Test>::AlreadyFinal
        );
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 3. challenge (07 §5.2 — bonded challenge supersedes the quorum requirement)
// =========================================================================

#[test]
fn challenge_happy_path_supersedes_quorum_and_escalates_with_doubled_bond() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));

        System::reset_events();
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(4)),
            C,
            E,
            V,
            counter_value(),
            h(10)
        ));
        assert_eq!(
            oracle_events(),
            vec![Event::Challenged {
                component: C,
                epoch: E,
                round: 1,
                challenger: acc(4),
                counter_value: counter_value(),
                evidence_hash: h(10),
                bond: bond(1),
            }]
        );

        // A posted challenge is itself proof of observability: the round
        // escalates on the crank regardless of any acknowledgments (07 §5.2),
        // bonds doubling per round (07 §6.2: `B_2 = 2·B_1`).
        set_block(1 + ORC_WINDOW_BLOCKS);
        System::reset_events();
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        assert_eq!(
            oracle_events(),
            vec![Event::RoundEscalated {
                component: C,
                epoch: E,
                round: 2,
                new_bond: bond(2),
            }]
        );
        let round = Rounds::<Test>::get((C, E, V)).unwrap();
        assert_eq!(round.round, 2);
        assert_eq!(round.bond, bond(2));
        assert!(round.challenger.is_none()); // cleared for the new round
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn challenge_window_is_half_open_at_the_deadline() {
    // Codex F24 / 07 §5.2: the challenge window is `[open, deadline)` — a
    // challenge at `deadline - 1` is the last valid block, and one at the
    // `deadline` block (which the close crank treats as mature) is `WindowClosed`,
    // so a challenge can never race the close.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_ok!(do_report(1, E + 1, reported_value(), h(9)));
        let deadline = Rounds::<Test>::get((C, E, V)).unwrap().challenge_deadline;

        // Last valid block succeeds.
        set_block(deadline - 1);
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(4)),
            C,
            E,
            V,
            counter_value(),
            h(10)
        ));
        // The deadline block itself is closed.
        set_block(deadline);
        assert_noop!(
            Oracle::challenge(
                RuntimeOrigin::signed(acc(4)),
                C,
                E + 1,
                V,
                counter_value(),
                h(10)
            ),
            Error::<Test>::WindowClosed
        );
    });
}

#[test]
fn challenge_second_time_is_already_challenged() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(4)),
            C,
            E,
            V,
            counter_value(),
            h(10)
        ));
        assert_noop!(
            Oracle::challenge(RuntimeOrigin::signed(acc(5)), C, E, V, FixedU64(1), h(11)),
            Error::<Test>::AlreadyChallenged
        );
    });
}

#[test]
fn challenge_unknown_round_is_round_not_found() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Oracle::challenge(
                RuntimeOrigin::signed(acc(4)),
                C,
                E,
                V,
                counter_value(),
                h(10)
            ),
            Error::<Test>::RoundNotFound
        );
    });
}

// =========================================================================
// 4. crank_round_close lifecycle (07 §4 quorum, §5 escalation, §10 neutral)
// =========================================================================

#[test]
fn crank_quorum_and_no_challenge_settles_unchallenged() {
    // 07 §4: an unchallenged round finalizes at window close only if ≥ `wt.quorum
    // = 2` distinct watchtowers acknowledged it.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        register_watchtower(2);
        register_watchtower(3);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        let rh = hash_report(C, E, 1, reported_value(), h(9));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(2)),
            C,
            E,
            V,
            1,
            rh
        ));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(3)),
            C,
            E,
            V,
            1,
            rh
        ));

        set_block(1 + ORC_WINDOW_BLOCKS);
        System::reset_events();
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));

        let settled = Oracle::settled_component(C, E, V).expect("settled");
        assert_eq!(settled.path, SettlePath::Unchallenged);
        assert_eq!(settled.value, reported_value());
        assert!(!settled.flagged);
        assert!(Rounds::<Test>::get((C, E, V)).is_none()); // round reaped
        System::assert_has_event(
            Event::ComponentSettled {
                component: C,
                epoch: E,
                value: reported_value(),
                path: SettlePath::Unchallenged,
            }
            .into(),
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn crank_no_quorum_extends_once_then_settles_neutral() {
    // 07 §4/§10: no quorum and no challenge ⇒ one 48 h (`orc.ext_window`)
    // extension, then — still no quorum — the neutral path, carrying the last
    // valid value (0.5 with no history, 05 §10) with the epoch flagged.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));

        set_block(1 + ORC_WINDOW_BLOCKS);
        System::reset_events();
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        let ext_deadline = (1 + ORC_WINDOW_BLOCKS) + ORC_EXT_WINDOW_BLOCKS;
        assert_eq!(
            oracle_events(),
            vec![Event::WindowExtended {
                component: C,
                epoch: E,
                round: 1,
                new_deadline: ext_deadline,
            }]
        );
        assert!(Rounds::<Test>::get((C, E, V)).unwrap().extended);

        set_block(ext_deadline);
        System::reset_events();
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        let neutral = FixedU64(COMPONENT_VALUE_MAX / 2);
        assert_eq!(
            oracle_events(),
            vec![
                Event::QuorumFailed {
                    component: C,
                    epoch: E,
                    round: 1,
                },
                Event::NeutralSettlement {
                    component: C,
                    epoch: E,
                    carried_value: neutral,
                    flagged_epochs: 1,
                },
                Event::ComponentSettled {
                    component: C,
                    epoch: E,
                    value: neutral,
                    path: SettlePath::Neutral,
                },
            ]
        );
        let settled = Oracle::settled_component(C, E, V).unwrap();
        assert_eq!(settled.path, SettlePath::Neutral);
        assert_eq!(settled.value, neutral);
        assert!(settled.flagged);
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn crank_round_close_batch_cap_is_honored() {
    // 07 §13: the crank is a bounded keeper batch. The shell clamps `batch` to
    // `MaxRoundCloseBatch`; a call asking for more than the cap processes no
    // more than the cap even when more rounds have matured.
    new_test_ext().execute_with(|| {
        let cap = MaxRoundCloseBatch::get();
        register_reporter(1);
        // One matured, unacked, unchallenged round per epoch: each takes the
        // single-extension branch, so "processed" is observable as `extended`.
        for epoch in 1..=(cap + 1) {
            assert_ok!(do_report(1, epoch, reported_value(), h(9)));
        }
        set_block(1 + ORC_WINDOW_BLOCKS + 1);
        assert_ok!(Oracle::crank_round_close(
            RuntimeOrigin::signed(acc(9)),
            cap + 5 // asks for more than the cap
        ));

        let extended = Rounds::<Test>::iter().filter(|(_, r)| r.extended).count();
        assert_eq!(extended as u32, cap); // exactly the cap advanced
        assert_eq!(Rounds::<Test>::iter().count() as u32, cap + 1); // none settled
        assert_eq!(
            oracle_events()
                .iter()
                .filter(|e| matches!(e, Event::WindowExtended { .. }))
                .count() as u32,
            cap
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn crank_round_close_with_nothing_matured_is_noop() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        // Window still open: the crank matures nothing.
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        assert!(Rounds::<Test>::get((C, E, V)).is_some());
        assert!(Oracle::settled_component(C, E, V).is_none());
    });
}

// =========================================================================
// 5. ack_observed (07 §4 — bonded watchtower acknowledgment)
// =========================================================================

#[test]
fn ack_observed_happy_path_increments_and_emits() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        register_watchtower(2);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        let rh = hash_report(C, E, 1, reported_value(), h(9));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(2)),
            C,
            E,
            V,
            1,
            rh
        ));

        assert_eq!(Rounds::<Test>::get((C, E, V)).unwrap().acks, 1);
        System::assert_has_event(
            Event::WindowAcknowledged {
                component: C,
                epoch: E,
                round: 1,
                watchtower: acc(2),
            }
            .into(),
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn ack_observed_non_watchtower_is_not_registered() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        let rh = hash_report(C, E, 1, reported_value(), h(9));
        assert_noop!(
            Oracle::ack_observed(RuntimeOrigin::signed(acc(2)), C, E, V, 1, rh),
            Error::<Test>::NotRegistered
        );
    });
}

#[test]
fn ack_observed_window_is_half_open_at_the_deadline() {
    // Codex F24 / 07 §4: the acknowledgment window is `[open, deadline)` — an ack
    // at `deadline - 1` is valid, and one at the `deadline` block is
    // `WindowClosed` (boundary consistency with the close crank, so a late ack
    // cannot retro-finalize).
    new_test_ext().execute_with(|| {
        register_reporter(1);
        register_watchtower(2);
        register_watchtower(3);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        let rh = hash_report(C, E, 1, reported_value(), h(9));
        let deadline = Rounds::<Test>::get((C, E, V)).unwrap().challenge_deadline;

        set_block(deadline - 1);
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(2)),
            C,
            E,
            V,
            1,
            rh
        ));
        set_block(deadline);
        assert_noop!(
            Oracle::ack_observed(RuntimeOrigin::signed(acc(3)), C, E, V, 1, rh),
            Error::<Test>::WindowClosed
        );
        assert_eq!(Rounds::<Test>::get((C, E, V)).unwrap().acks, 1); // only the valid ack
    });
}

#[test]
fn ack_observed_duplicate_is_duplicate_ack() {
    // 07 §13: acks are per-round, keyed by `report_hash`; a replay is refused.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        register_watchtower(2);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        let rh = hash_report(C, E, 1, reported_value(), h(9));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(2)),
            C,
            E,
            V,
            1,
            rh
        ));
        assert_noop!(
            Oracle::ack_observed(RuntimeOrigin::signed(acc(2)), C, E, V, 1, rh),
            Error::<Test>::DuplicateAck
        );
        assert_eq!(Rounds::<Test>::get((C, E, V)).unwrap().acks, 1);
    });
}

#[test]
fn ack_observed_inside_extension_window_still_counts_toward_quorum() {
    // Mirror of the core's `late_watchtower_acks_cannot_retro_finalize`: acks
    // rejected after the original close, then accepted inside the live 48 h
    // extension, drive an unchallenged settle (07 §4).
    new_test_ext().execute_with(|| {
        register_reporter(1);
        register_watchtower(2);
        register_watchtower(3);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        let rh = hash_report(C, E, 1, reported_value(), h(9));
        let deadline = Rounds::<Test>::get((C, E, V)).unwrap().challenge_deadline;

        // An acknowledgment at the deadline block is already closed (half-open)...
        set_block(deadline);
        assert_noop!(
            Oracle::ack_observed(RuntimeOrigin::signed(acc(2)), C, E, V, 1, rh),
            Error::<Test>::WindowClosed
        );
        // ...the uncranked round then extends rather than finalizing (the close
        // crank treats the deadline block as mature)...
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        let ext_deadline = Rounds::<Test>::get((C, E, V)).unwrap().challenge_deadline;
        assert!(ext_deadline > deadline);

        // ...and acks strictly inside the live extension window count toward quorum.
        set_block(ext_deadline - 1);
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(2)),
            C,
            E,
            V,
            1,
            rh
        ));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(3)),
            C,
            E,
            V,
            1,
            rh
        ));
        set_block(ext_deadline);
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        assert_eq!(
            Oracle::settled_component(C, E, V).unwrap().path,
            SettlePath::Unchallenged
        );
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 6. recompute_proof (07 §9 — permissionless mechanical resolution)
// =========================================================================

#[test]
fn recompute_proof_matching_evidence_settles_recomputed_without_offense() {
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::note_recomputable(C, V));
        register_reporter(1);
        let proof = proof_for(reported_value());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&proof)));

        System::reset_events();
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            V,
            proof_arg(proof)
        ));
        let settled = Oracle::settled_component(C, E, V).unwrap();
        assert_eq!(settled.path, SettlePath::Recomputed);
        assert_eq!(settled.value, reported_value());
        // Reporter's committed data agreed: no offense (07 §9).
        assert_eq!(Reporters::<Test>::get(acc(1)).unwrap().offenses, 0);
        System::assert_has_event(
            Event::RecomputeProven {
                component: C,
                epoch: E,
                value: reported_value(),
                prover: acc(5),
            }
            .into(),
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn recompute_proof_disagreeing_evidence_settles_recomputed_and_records_offense() {
    // 07 §5/§9: committed data that disproves the reported value settles at the
    // recomputed value and forfeits the reporter's stack (records an offense).
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::note_recomputable(C, V));
        register_reporter(1);
        let proof = proof_for(counter_value()); // committed data says 0.44
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&proof))); // reported 0.62

        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            V,
            proof_arg(proof)
        ));
        let settled = Oracle::settled_component(C, E, V).unwrap();
        assert_eq!(settled.path, SettlePath::Recomputed);
        assert_eq!(settled.value, counter_value());
        assert_eq!(Reporters::<Test>::get(acc(1)).unwrap().offenses, 1);
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn recompute_proof_non_recomputable_component_is_rejected() {
    // 07 §9: the flow is admissible only where the frozen spec declares the
    // component deterministically recomputable; otherwise it fails closed.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        let proof = proof_for(reported_value());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&proof)));
        assert_noop!(
            Oracle::recompute_proof(RuntimeOrigin::signed(acc(5)), C, E, V, proof_arg(proof)),
            Error::<Test>::NotRecomputable
        );
    });
}

#[test]
fn recompute_proof_evidence_mismatch_is_rejected() {
    // 07 §9: the proof must reproduce the committed content hash.
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::note_recomputable(C, V));
        register_reporter(1);
        let committed = proof_for(reported_value());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&committed)));
        let other = proof_for(counter_value()); // hashes differently
        assert_noop!(
            Oracle::recompute_proof(RuntimeOrigin::signed(acc(5)), C, E, V, proof_arg(other)),
            Error::<Test>::EvidenceMismatch
        );
    });
}

#[test]
fn recompute_proof_short_committed_payload_is_bad_proof() {
    // 07 §9 / 05 §4.4: a committed payload too short to decode a grid value is
    // a bad proof even when its content hash matches.
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::note_recomputable(C, V));
        register_reporter(1);
        let short = vec![3u8; 4];
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&short)));
        assert_noop!(
            Oracle::recompute_proof(RuntimeOrigin::signed(acc(5)), C, E, V, proof_arg(short)),
            Error::<Test>::BadProof
        );
    });
}

#[test]
fn recompute_proof_off_grid_committed_payload_is_bad_proof() {
    // 05 §4.4 determinism rule 1: a value off the [0, 1] 1e9 grid is a bad proof.
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::note_recomputable(C, V));
        register_reporter(1);
        let mut off_grid = vec![0u8; 24];
        off_grid[..8].copy_from_slice(&(COMPONENT_VALUE_MAX + 1).to_le_bytes());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&off_grid)));
        assert_noop!(
            Oracle::recompute_proof(RuntimeOrigin::signed(acc(5)), C, E, V, proof_arg(off_grid)),
            Error::<Test>::BadProof
        );
    });
}

#[test]
fn recompute_proof_arg_bound_rejects_oversized_payload() {
    // 07 §9: `orc.max_proof_bytes = 256 KiB` is enforced by the SCALE-decoded
    // `BoundedVec` argument itself — a payload one byte over the bound cannot be
    // constructed into the call, so `ProofTooLarge` is unreachable by design.
    new_test_ext().execute_with(|| {
        let oversized = vec![0u8; crate::MAX_PROOF_BYTES_BOUND as usize + 1];
        assert!(ProofArg::try_from(oversized).is_err());
        // The boundary value (exactly the bound) is constructible.
        let at_bound = vec![0u8; crate::MAX_PROOF_BYTES_BOUND as usize];
        assert!(ProofArg::try_from(at_bound).is_ok());
    });
}

// =========================================================================
// 7. adjudicate (07 §5.4 OracleResolution-only origin, §5.5 slashing)
// =========================================================================

#[test]
fn adjudicate_happy_path_settles_adjudicated() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        escalate_to_terminal(1, 4, E); // round `R_max` with a live challenger (F10)

        System::reset_events();
        let verdict = FixedU64(500_000_000);
        assert_ok!(Oracle::adjudicate(
            RuntimeOrigin::signed(oracle_resolution_acc()),
            C,
            E,
            V,
            verdict,
            false
        ));
        let settled = Oracle::settled_component(C, E, V).unwrap();
        assert_eq!(settled.path, SettlePath::Adjudicated);
        assert_eq!(settled.value, verdict);
        assert_eq!(
            oracle_events(),
            vec![
                Event::Adjudicated {
                    component: C,
                    epoch: E,
                    value: verdict,
                },
                Event::ComponentSettled {
                    component: C,
                    epoch: E,
                    value: verdict,
                    path: SettlePath::Adjudicated,
                },
            ]
        );
        // reporter_wrong = false ⇒ no offense.
        assert_eq!(Reporters::<Test>::get(acc(1)).unwrap().offenses, 0);
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn adjudicate_rejects_non_oracle_resolution_origins() {
    // 07 §5.4 / rule 6 (G-5): the sole privileged call admits only the
    // `OracleResolution` track — an ordinary signed account, Root, and an
    // unsigned origin are all `BadOrigin`, and no state moves.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        for bad in [
            RuntimeOrigin::signed(acc(1)),
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
        ] {
            assert_noop!(
                Oracle::adjudicate(bad, C, E, V, counter_value(), true),
                DispatchError::BadOrigin
            );
        }
        assert!(Oracle::settled_component(C, E, V).is_none());
        assert_eq!(Reporters::<Test>::get(acc(1)).unwrap().offenses, 0);
    });
}

#[test]
fn adjudicate_reporter_wrong_records_a_single_offense() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        escalate_to_terminal(1, 4, E);
        assert_ok!(Oracle::adjudicate(
            RuntimeOrigin::signed(oracle_resolution_acc()),
            C,
            E,
            V,
            counter_value(),
            true
        ));
        assert_eq!(Reporters::<Test>::get(acc(1)).unwrap().offenses, 1);
        // First offense does not slash or eject (07 §3).
        assert!(!oracle_events().iter().any(|e| matches!(
            e,
            Event::ReporterSlashed { .. } | Event::ReporterEjected { .. }
        )));
    });
}

#[test]
fn adjudicate_third_offense_slashes_and_ejects_reporter() {
    // 07 §3 / Codex F19: a 50 % slash of `orc.reporter_stake` on the *second*
    // adjudicated-false report, and *ejection-only* on the third (no further
    // slash). Three recompute-disproofs — each records an offense and settles the
    // game directly (07 §9, no crank; F10 makes adjudicate terminal-only) — walk
    // that discipline.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(Oracle::note_recomputable(C, V));
        let disproof = proof_for(counter_value()); // recomputes to 0.44, disproving 0.62
        for epoch in E..(E + 3) {
            assert_ok!(Oracle::report(
                RuntimeOrigin::signed(acc(1)),
                C,
                epoch,
                V,
                reported_value(),
                hash_evidence(&disproof)
            ));
            assert_ok!(Oracle::recompute_proof(
                RuntimeOrigin::signed(acc(5)),
                C,
                epoch,
                V,
                proof_arg(disproof.clone())
            ));
        }
        assert_eq!(Reporters::<Test>::count(), 0); // ejected
        let evs = oracle_events();
        assert!(evs.iter().any(|e| matches!(
            e,
            Event::ReporterSlashed {
                who,
                amount,
                offense: 2
            } if *who == acc(1) && *amount == ORC_REPORTER_STAKE / 2
        )));
        // Codex F19: the third offense ejects only — no further `ReporterSlashed`.
        assert!(!evs
            .iter()
            .any(|e| matches!(e, Event::ReporterSlashed { offense: 3, .. })));
        assert!(evs
            .iter()
            .any(|e| matches!(e, Event::ReporterEjected { who } if *who == acc(1))));
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 8. request_adjudication (07 §5.4 — runtime-internal escalation to the track)
// =========================================================================

#[test]
fn request_adjudication_at_round_three_emits_event() {
    // Drive the game to round 3 with a live challenger, then escalate to the
    // `OracleResolution` referendum (07 §5.3/§5.4).
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(4)),
            C,
            E,
            V,
            counter_value(),
            h(10)
        ));
        set_block(1 + ORC_WINDOW_BLOCKS);
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(4)),
            C,
            E,
            V,
            counter_value(),
            h(11)
        ));
        set_block(1 + 2 * ORC_WINDOW_BLOCKS);
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        let round = Rounds::<Test>::get((C, E, V)).unwrap();
        assert_eq!(round.round, 3);
        assert_eq!(round.bond, bond(3)); // 07 §6.2: `B_3 = 4·B_1`
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(4)),
            C,
            E,
            V,
            counter_value(),
            h(12)
        ));

        System::reset_events();
        assert_ok!(Oracle::request_adjudication(C, E, V, 77));
        assert_eq!(
            oracle_events(),
            vec![Event::AdjudicationRequested {
                component: C,
                epoch: E,
                referendum: 77,
            }]
        );
    });
}

#[test]
fn request_adjudication_before_round_three_is_window_open() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_noop!(
            Oracle::request_adjudication(C, E, V, 1),
            Error::<Test>::WindowOpen
        );
    });
}

// =========================================================================
// 9. Reserve health probe R (07 §8 — deterministic class-3, fail-static)
// =========================================================================

mod probe_dispatch_seam {
    use super::*;
    use crate as pallet_oracle;
    use frame_support::{derive_impl, parameter_types};
    use sp_runtime::{traits::IdentityLookup, AccountId32, BuildStorage};
    use std::cell::RefCell;

    type Block = frame_system::mocking::MockBlock<DispatchTest>;

    frame_support::construct_runtime!(
        pub enum DispatchTest {
            System: frame_system,
            Oracle: pallet_oracle,
        }
    );

    #[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
    impl frame_system::Config for DispatchTest {
        type Block = Block;
        type AccountId = AccountId32;
        type Lookup = IdentityLookup<AccountId32>;
    }

    pub struct DispatchReporting;

    impl pallet_oracle::ReportingContext for DispatchReporting {
        fn report_window_end(_: EpochId) -> futarchy_primitives::BlockNumber {
            10
        }

        fn is_expected_spec_version(_: MetricId, _: EpochId, _: MetricSpecVersion) -> bool {
            true
        }

        fn stake_at_risk(_: MetricId, _: EpochId) -> Balance {
            0
        }

        fn expected_components(_: EpochId) -> Vec<(MetricId, MetricSpecVersion)> {
            Vec::new()
        }
    }

    std::thread_local! {
        static DISPATCHED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    }

    pub struct RecordingProbeDispatch;

    impl pallet_oracle::ProbeDispatch for RecordingProbeDispatch {
        fn probe_due(query_id: u64) {
            DISPATCHED.with(|ids| ids.borrow_mut().push(query_id));
        }
    }

    parameter_types! {
        pub const MaxRoundCloseBatch: u32 = 20;
    }

    impl pallet_oracle::Config for DispatchTest {
        type AdjudicationOrigin = frame_system::EnsureRoot<AccountId32>;
        type Reporting = DispatchReporting;
        type MaxRoundCloseBatch = MaxRoundCloseBatch;
        type ProbeDispatch = RecordingProbeDispatch;
        type WeightInfo = ();
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper = DispatchBenchmarkHelper;
    }

    #[cfg(feature = "runtime-benchmarks")]
    pub struct DispatchBenchmarkHelper;

    #[cfg(feature = "runtime-benchmarks")]
    impl pallet_oracle::BenchmarkHelper<RuntimeOrigin> for DispatchBenchmarkHelper {
        fn adjudication_origin() -> RuntimeOrigin {
            RuntimeOrigin::root()
        }
    }

    fn new_ext() -> sp_io::TestExternalities {
        let storage = RuntimeGenesisConfig {
            system: Default::default(),
            oracle: Default::default(),
        }
        .build_storage()
        .expect("probe-dispatch test genesis must build");
        let mut ext = sp_io::TestExternalities::new(storage);
        ext.execute_with(|| System::set_block_number(1));
        ext
    }

    #[test]
    fn reserve_probe_crank_dispatches_only_a_fresh_pending_query() {
        new_ext().execute_with(|| {
            DISPATCHED.with(|ids| ids.borrow_mut().clear());

            System::set_block_number(u64::from(RES_PROBE_INTERVAL));
            assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                AccountId32::new([9; 32])
            )));
            assert_eq!(
                pallet_oracle::ReserveHealth::<DispatchTest>::get().last_query_id,
                1
            );
            DISPATCHED.with(|ids| assert_eq!(&*ids.borrow(), &[1]));

            // The timeout matures before the next send interval. This crank
            // commits the fail-static fold but creates no pending query, so the
            // runtime dispatcher must not be invoked a second time (07 §8).
            System::set_block_number(u64::from(RES_PROBE_INTERVAL + RES_PROBE_TIMEOUT));
            assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                AccountId32::new([9; 32])
            )));
            let health = pallet_oracle::ReserveHealth::<DispatchTest>::get();
            assert_eq!(health.last_query_id, 1);
            assert_eq!(health.pending_since, None);
            assert_eq!(health.consecutive_fails, 1);
            DISPATCHED.with(|ids| assert_eq!(&*ids.borrow(), &[1]));
        });
    }
}

#[test]
fn reserve_probe_before_interval_is_too_early() {
    new_test_ext().execute_with(|| {
        // Block 1 < `res.probe_interval` (14,400): no probe may be sent yet.
        assert_noop!(
            Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))),
            Error::<Test>::ProbeTooEarly
        );
    });
}

#[test]
fn reserve_probe_first_send_emits_query_one() {
    new_test_ext().execute_with(|| {
        set_block(RES_PROBE_INTERVAL);
        System::reset_events();
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
        assert_eq!(ReserveHealth::<Test>::get().last_query_id, 1);
        System::assert_has_event(Event::ReserveProbeSent { query_id: 1 }.into());
        assert!(!Oracle::reserve_unhealthy());
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn reserve_probe_two_consecutive_fails_go_unhealthy() {
    // 07 §8: `res.fail_threshold = 2` consecutive failed probes ⇒ unhealthy.
    new_test_ext().execute_with(|| {
        set_block(RES_PROBE_INTERVAL);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
        assert_ok!(Oracle::reserve_probe_result(1, false));
        assert!(!Oracle::reserve_unhealthy());
        assert_eq!(ReserveHealth::<Test>::get().consecutive_fails, 1);

        set_block(RES_PROBE_INTERVAL * 2);
        System::reset_events();
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
        assert_ok!(Oracle::reserve_probe_result(2, false));
        assert!(Oracle::reserve_unhealthy());
        System::assert_has_event(Event::ReserveUnhealthy.into());
        System::assert_has_event(
            Event::ReserveProbeResult {
                query_id: 2,
                passed: false,
            }
            .into(),
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn reserve_probe_three_consecutive_passes_recover() {
    // 07 §8: `res.recover_threshold = 3` consecutive passes clears the state.
    new_test_ext().execute_with(|| {
        set_block(RES_PROBE_INTERVAL);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
        assert_ok!(Oracle::reserve_probe_result(1, false));
        set_block(RES_PROBE_INTERVAL * 2);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
        assert_ok!(Oracle::reserve_probe_result(2, false));
        assert!(Oracle::reserve_unhealthy());

        System::reset_events();
        for i in 3..=5u64 {
            set_block(RES_PROBE_INTERVAL * i as u32);
            assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
            assert_ok!(Oracle::reserve_probe_result(i, true));
        }
        assert!(!Oracle::reserve_unhealthy());
        System::assert_has_event(Event::ReserveRecovered.into());
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn reserve_probe_result_for_unknown_or_stale_query_is_rejected() {
    // 07 §8/§13: an unknown or already-consumed `query_id` is dropped and never
    // moves the fail-static state (mirror of `reserve_probe_results_count_once`).
    new_test_ext().execute_with(|| {
        set_block(RES_PROBE_INTERVAL);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
        // Wrong id: no state move.
        assert_noop!(
            Oracle::reserve_probe_result(999, false),
            Error::<Test>::UnknownQuery
        );
        assert_eq!(ReserveHealth::<Test>::get().consecutive_fails, 0);
        // Consume the live probe, then a replay of it is rejected too.
        assert_ok!(Oracle::reserve_probe_result(1, false));
        assert_noop!(
            Oracle::reserve_probe_result(1, false),
            Error::<Test>::UnknownQuery
        );
        assert_eq!(ReserveHealth::<Test>::get().consecutive_fails, 1);
    });
}

#[test]
fn reserve_probe_timeout_fold_counts_an_unanswered_probe_as_a_fail() {
    // 07 §8: a probe still unanswered when the next crank fires past
    // `res.probe_timeout` counts as a fail (absence is never healthy). Two such
    // timeouts, with no `QueryResponse` at all, reach the unhealthy state.
    new_test_ext().execute_with(|| {
        set_block(RES_PROBE_INTERVAL);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9)))); // probe 1 pending

        set_block(RES_PROBE_INTERVAL * 2);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9)))); // probe 1 times out, probe 2 sent
        assert!(!Oracle::reserve_unhealthy());
        assert_eq!(ReserveHealth::<Test>::get().consecutive_fails, 1);

        set_block(RES_PROBE_INTERVAL * 3);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9)))); // probe 2 times out
        assert!(Oracle::reserve_unhealthy());
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 10. Per-version games (SQ-2; 07 §2(4) — one game per frozen spec version)
// =========================================================================

#[test]
fn per_version_games_settle_independently_across_an_activation_boundary() {
    // 07 §2(4): where two live cohorts consume the same `(component, epoch)`
    // under different frozen versions, one game runs per version; settling one
    // must not settle or shadow the other, and each lands its own triple entry.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(Oracle::note_recomputable(C, 3));
        assert_ok!(Oracle::note_recomputable(C, 4));
        ExpectedSpecs::set(vec![3, 4]);
        // Version-3 game.
        let proof3 = proof_for(reported_value());
        assert_ok!(Oracle::report(
            RuntimeOrigin::signed(acc(1)),
            C,
            E,
            3,
            reported_value(),
            hash_evidence(&proof3)
        ));
        // Version-4 game opens independently while v3 is live.
        let v4_value = FixedU64(500_000_000);
        let proof4 = proof_for(v4_value);
        assert_ok!(Oracle::report(
            RuntimeOrigin::signed(acc(1)),
            C,
            E,
            4,
            v4_value,
            hash_evidence(&proof4)
        ));

        assert!(Rounds::<Test>::get((C, E, 3)).is_some());
        assert!(Rounds::<Test>::get((C, E, 4)).is_some());

        // Settle only v3 (recompute — F10 makes adjudicate terminal-only).
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            3,
            proof_arg(proof3)
        ));
        assert!(Oracle::settled_component(C, E, 3).is_some());
        assert!(Oracle::settled_component(C, E, 4).is_none()); // not shadowed
        assert!(Rounds::<Test>::get((C, E, 4)).is_some()); // still live

        // v4 settles on its own track ⇒ two distinct triple entries.
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            4,
            proof_arg(proof4)
        ));
        assert_eq!(
            Oracle::settled_component(C, E, 3).unwrap().value,
            reported_value()
        );
        assert_eq!(Oracle::settled_component(C, E, 4).unwrap().value, v4_value);

        // A repeat report for a settled version stays final.
        ExpectedSpecs::set(vec![3]);
        assert_noop!(
            Oracle::report(RuntimeOrigin::signed(acc(1)), C, E, 3, FixedU64(1), h(9)),
            Error::<Test>::AlreadyFinal
        );
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 11. Origin misuse across the Signed call surface (15 §4.1; rule 6)
// =========================================================================

#[test]
fn signed_calls_reject_unsigned_origin() {
    // Every reporter/watchtower/keeper call is ordinary `Signed` (07 §3/§4/§5;
    // `ValidateUnsigned` is implemented for no call) — an unsigned origin is
    // `BadOrigin` before any state is touched.
    new_test_ext().execute_with(|| {
        assert_noop!(
            Oracle::register_reporter(RuntimeOrigin::none()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::deregister_reporter(RuntimeOrigin::none()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::report(RuntimeOrigin::none(), C, E, V, reported_value(), h(9)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::challenge(RuntimeOrigin::none(), C, E, V, counter_value(), h(10)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::recompute_proof(RuntimeOrigin::none(), C, E, V, ProofArg::default()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::register_watchtower(RuntimeOrigin::none()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::ack_observed(RuntimeOrigin::none(), C, E, V, 1, h(9)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::crank_round_close(RuntimeOrigin::none(), 20),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Oracle::crank_reserve_probe(RuntimeOrigin::none()),
            DispatchError::BadOrigin
        );
    });
}

// =========================================================================
// 12. try_state (15 §1; I-18 — only challenge-closed values settle money)
// =========================================================================

#[test]
fn try_state_holds_across_the_representative_lifecycle() {
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::do_try_state()); // empty genesis
        register_reporter(1);
        assert_ok!(Oracle::note_recomputable(C, V));
        let proof = proof_for(reported_value());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&proof)));
        assert_ok!(Oracle::do_try_state()); // post-report

        // Settle via recompute (F10 makes adjudicate terminal-only).
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            V,
            proof_arg(proof)
        ));
        assert_ok!(Oracle::do_try_state()); // post-settle

        // A second key walks the neutral path.
        assert_ok!(do_report(1, E + 1, reported_value(), h(9)));
        set_block(1 + ORC_WINDOW_BLOCKS);
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20)); // extend
        let ext = Rounds::<Test>::get((C, E + 1, V))
            .unwrap()
            .challenge_deadline;
        set_block(ext);
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20)); // neutral
        assert_ok!(Oracle::do_try_state()); // post-neutral

        set_block(ext + RES_PROBE_INTERVAL);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9))));
        assert_ok!(Oracle::reserve_probe_result(1, true));
        assert_ok!(Oracle::do_try_state()); // post-probe
    });
}

/// Build a well-formed round for corrupt-state injection (valid up to the
/// invariant being violated).
fn valid_round(round: u8) -> RoundState {
    let stake = StakeAtRiskValue::get();
    RoundState {
        component: C,
        epoch: E,
        round,
        spec_version: V,
        reporter: raw(1),
        value: reported_value(),
        evidence_hash: h(9),
        bond: bond(round),
        challenge_deadline: 0,
        extended: false,
        challenger: None,
        counter_value: None,
        acks: 0,
        report_hash: h(0),
        stake_at_risk: stake,
        cumulative_reporter_bond: bond(round),
        cumulative_challenger_bond: 0,
    }
}

#[test]
fn try_state_rejects_an_out_of_range_round() {
    // 07 §13 machine invariant: every live round is in `1..=R_max`.
    new_test_ext().execute_with(|| {
        assert_ok!(Oracle::do_try_state());
        let mut bad = valid_round(1);
        bad.round = 5; // out of `1..=3`
        Rounds::<Test>::insert((C, E, V), bad);
        assert!(Oracle::do_try_state().is_err());
    });
}

#[test]
fn try_state_rejects_a_live_round_for_a_settled_key() {
    // I-18 (07 §13): no live round may survive for an already-settled
    // `(component, epoch, version)`, or a second settlement could shadow the
    // final value.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        settle_recomputed(1, E); // F10: settle without adjudicate
        assert_ok!(Oracle::do_try_state()); // settled, no live round

        // Inject a live round shadowing the settled entry.
        Rounds::<Test>::insert((C, E, V), valid_round(1));
        assert!(Oracle::do_try_state().is_err());
    });
}

// =========================================================================
// 13. Genesis (07 §2(4)/§9 recomputable seed; default emptiness)
// =========================================================================

#[test]
fn genesis_recomputable_seed_enables_recompute_at_dispatch() {
    let genesis = crate::GenesisConfig::<Test> {
        recomputable_components: vec![(C, V)],
        ..Default::default()
    };
    new_test_ext_with(genesis).execute_with(|| {
        register_reporter(1);
        let proof = proof_for(reported_value());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&proof)));
        // Seeded at genesis ⇒ recompute is admissible (contrast:
        // `recompute_proof_non_recomputable_component_is_rejected`).
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            V,
            proof_arg(proof)
        ));
        assert_eq!(
            Oracle::settled_component(C, E, V).unwrap().path,
            SettlePath::Recomputed
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn default_genesis_is_empty_and_reserve_is_healthy() {
    new_test_ext().execute_with(|| {
        assert_eq!(Reporters::<Test>::count(), 0);
        assert_eq!(Watchtowers::<Test>::count(), 0);
        assert!(Recomputable::<Test>::get().is_empty());
        assert!(!Oracle::reserve_unhealthy());
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 14. Read/reap accessors for sibling pallets (07 §13; A8/B1a wiring)
// =========================================================================

#[test]
fn settled_component_reads_and_reap_clears_the_entry() {
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(Oracle::note_recomputable(C, V));
        let proof = proof_for(reported_value());
        assert_ok!(do_report(1, E, reported_value(), hash_evidence(&proof)));
        assert!(Oracle::settled_component(C, E, V).is_none()); // not yet settled
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            V,
            proof_arg(proof)
        ));
        assert!(Oracle::settled_component(C, E, V).is_some());
        Oracle::reap_component(C, E, V);
        assert!(Oracle::settled_component(C, E, V).is_none());
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 15. Watchtower liveness discipline (07 §4 — epoch-boundary sweep)
// =========================================================================
// `note_epoch_boundary(ended_epoch, had_open_round)` is runtime-internal (the
// epoch pallet drives it at B1a); it is called directly here, not as an
// extrinsic. The just-ended epoch's activity set is `WatchtowerActive`.

#[test]
fn watchtower_liveness_registration_grace_charges_nothing() {
    // 07 §4: a watchtower is active for the epoch it registered in — the first
    // sweep does not charge it, and the activity set clears afterward.
    new_test_ext().execute_with(|| {
        register_watchtower(2);
        // Registration marked the seat active for the current epoch.
        assert!(WatchtowerActive::<Test>::get().contains(&raw(2)));

        System::reset_events();
        assert_ok!(Oracle::note_epoch_boundary(1, true));
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 0);
        assert!(!oracle_events()
            .iter()
            .any(|e| matches!(e, Event::WatchtowerInactive { .. })));
        assert!(WatchtowerActive::<Test>::get().is_empty()); // swept clean
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn watchtower_liveness_inactive_epoch_accrues_and_emits() {
    // 07 §4: a watchtower that acknowledges no round in an epoch that carried an
    // open round is marked inactive for that epoch.
    new_test_ext().execute_with(|| {
        register_watchtower(2);
        assert_ok!(Oracle::note_epoch_boundary(1, true)); // grace consumed

        System::reset_events();
        assert_ok!(Oracle::note_epoch_boundary(2, true));
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 1);
        assert_eq!(
            oracle_events(),
            vec![Event::WatchtowerInactive {
                who: acc(2),
                epoch: 2,
            }]
        );
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn watchtower_liveness_second_consecutive_miss_slashes_and_ejects() {
    // 07 §4: two consecutive inactive epochs slash 10 % of `wt.stake` and eject.
    new_test_ext().execute_with(|| {
        register_watchtower(2);
        assert_ok!(Oracle::note_epoch_boundary(1, true)); // grace
        assert_ok!(Oracle::note_epoch_boundary(2, true)); // inactive #1
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 1);

        System::reset_events();
        assert_ok!(Oracle::note_epoch_boundary(3, true)); // inactive #2 ⇒ slash + eject
        assert!(oracle_events().iter().any(|e| matches!(
            e,
            Event::WatchtowerSlashed { who, amount }
                if *who == acc(2) && *amount == WT_STAKE / 10
        )));
        assert_eq!(Watchtowers::<Test>::count(), 0);
        assert!(Watchtowers::<Test>::get(acc(2)).is_none());
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn watchtower_liveness_acknowledgment_resets_the_counter() {
    // 07 §4: a watchtower active this epoch (it acknowledged a round) has its
    // inactivity counter reset — verified by moving a genuinely non-zero counter
    // back to 0 rather than merely holding it there.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        register_watchtower(2);
        assert_ok!(Oracle::note_epoch_boundary(1, true)); // grace
        assert_ok!(Oracle::note_epoch_boundary(2, true)); // inactive #1
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 1);

        // The watchtower does its job next epoch: acknowledge a live round.
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        let rh = hash_report(C, E, 1, reported_value(), h(9));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(2)),
            C,
            E,
            V,
            1,
            rh
        ));

        System::reset_events();
        assert_ok!(Oracle::note_epoch_boundary(3, true));
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 0); // reset
        assert_eq!(Watchtowers::<Test>::count(), 1); // not ejected
        assert!(!oracle_events().iter().any(|e| matches!(
            e,
            Event::WatchtowerInactive { .. } | Event::WatchtowerSlashed { .. }
        )));
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn watchtower_liveness_no_open_round_epoch_charges_nobody() {
    // 07 §4: an epoch with no open round is not a liveness failure — a genuinely
    // idle (non-active) watchtower is exempt when `had_open_round = false`.
    new_test_ext().execute_with(|| {
        register_watchtower(2);
        assert_ok!(Oracle::note_epoch_boundary(1, true)); // grace clears the active set

        System::reset_events();
        assert_ok!(Oracle::note_epoch_boundary(2, false)); // idle, but no open round
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 0);
        assert_eq!(Watchtowers::<Test>::count(), 1);
        assert!(!oracle_events()
            .iter()
            .any(|e| matches!(e, Event::WatchtowerInactive { .. })));
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn watchtower_liveness_no_open_round_breaks_the_inactivity_streak() {
    // Codex F5 / 07 §4: an epoch with no open round resets the inactivity
    // counter, breaking the "two *consecutive*" streak — so a miss on either side
    // of an exempt epoch cannot combine to force a slash.
    new_test_ext().execute_with(|| {
        register_watchtower(2);
        assert_ok!(Oracle::note_epoch_boundary(1, true)); // grace
        assert_ok!(Oracle::note_epoch_boundary(2, true)); // miss #1 ⇒ inactive 1
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 1);

        assert_ok!(Oracle::note_epoch_boundary(3, false)); // exempt ⇒ streak resets
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 0);

        System::reset_events();
        assert_ok!(Oracle::note_epoch_boundary(4, true)); // a fresh miss #1, not #2
        assert_eq!(Watchtowers::<Test>::get(acc(2)).unwrap().inactive_epochs, 1);
        assert_eq!(Watchtowers::<Test>::count(), 1); // not ejected
        assert!(!oracle_events()
            .iter()
            .any(|e| matches!(e, Event::WatchtowerSlashed { .. })));
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 16. Component value range (Codex F15; 05 §4.4 [0,1] 1e9 grid, I-18)
// =========================================================================

#[test]
fn report_value_over_grid_max_is_out_of_bounds() {
    // Codex F15: a value above `COMPONENT_VALUE_MAX` can never be a valid settled
    // value, so `report` rejects it at the door rather than let an out-of-range
    // attestation settle unchallenged (I-18). The grid maximum itself is valid.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_noop!(
            do_report(1, E, FixedU64(COMPONENT_VALUE_MAX + 1), h(9)),
            Error::<Test>::ValueOutOfBounds
        );
        assert!(Rounds::<Test>::get((C, E, V)).is_none()); // nothing opened
        assert_ok!(do_report(1, E, FixedU64(COMPONENT_VALUE_MAX), h(9)));
        assert_ok!(Oracle::do_try_state());
    });
}

#[test]
fn adjudicate_value_over_grid_max_is_out_of_bounds() {
    // Codex F15: the terminal verdict is itself a settled value and must lie on
    // the grid — an out-of-range verdict is rejected and settles nothing.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_noop!(
            Oracle::adjudicate(
                RuntimeOrigin::signed(oracle_resolution_acc()),
                C,
                E,
                V,
                FixedU64(COMPONENT_VALUE_MAX + 1),
                false
            ),
            Error::<Test>::ValueOutOfBounds
        );
        assert!(Oracle::settled_component(C, E, V).is_none());
        assert!(Rounds::<Test>::get((C, E, V)).is_some()); // round untouched
    });
}

// =========================================================================
// 17. Concurrent per-version games (Codex F7; 07 §2(4))
// =========================================================================

#[test]
fn concurrent_per_version_reports_coexist_without_toggling() {
    // Codex F7: with both frozen versions in the live set, a single reporter can
    // post both the version-3 and version-4 reports for one `(component, epoch)`
    // without toggling the provider — the two per-version games coexist.
    new_test_ext().execute_with(|| {
        ExpectedSpecs::set(vec![3, 4]);
        register_reporter(1);
        assert_ok!(Oracle::report(
            RuntimeOrigin::signed(acc(1)),
            C,
            E,
            3,
            reported_value(),
            h(9)
        ));
        assert_ok!(Oracle::report(
            RuntimeOrigin::signed(acc(1)),
            C,
            E,
            4,
            FixedU64(500_000_000),
            h(4)
        ));
        assert!(Rounds::<Test>::get((C, E, 3)).is_some());
        assert!(Rounds::<Test>::get((C, E, 4)).is_some());
        assert_ok!(Oracle::do_try_state());
        ExpectedSpecs::set(vec![3]); // restore default for suite isolation
    });
}

// =========================================================================
// 18. Version-scoped ack pruning (Codex F8; 07 §2(4)/§13)
// =========================================================================

#[test]
fn per_version_acks_survive_a_sibling_version_settlement() {
    // Codex F8: `AckRecords` are keyed by the full game triple, so settling one
    // per-version game must NOT prune a sibling version's acknowledgments — the
    // core bug the fix addresses. Here v4 gathers quorum, v3 is recompute-settled,
    // and v4's acks survive to settle it Unchallenged on its own quorum.
    new_test_ext().execute_with(|| {
        ExpectedSpecs::set(vec![3, 4]);
        register_reporter(1);
        register_watchtower(2);
        register_watchtower(3);
        assert_ok!(Oracle::note_recomputable(C, 3));
        let proof3 = proof_for(reported_value());
        assert_ok!(Oracle::report(
            RuntimeOrigin::signed(acc(1)),
            C,
            E,
            3,
            reported_value(),
            hash_evidence(&proof3)
        ));
        let v4_value = FixedU64(500_000_000);
        assert_ok!(Oracle::report(
            RuntimeOrigin::signed(acc(1)),
            C,
            E,
            4,
            v4_value,
            h(4)
        ));
        let rh4 = hash_report(C, E, 1, v4_value, h(4));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(2)),
            C,
            E,
            4,
            1,
            rh4
        ));
        assert_ok!(Oracle::ack_observed(
            RuntimeOrigin::signed(acc(3)),
            C,
            E,
            4,
            1,
            rh4
        ));
        assert_eq!(Rounds::<Test>::get((C, E, 4)).unwrap().acks, 2);

        // Settle v3 by recompute (F10) — must not touch v4's acks.
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            C,
            E,
            3,
            proof_arg(proof3)
        ));
        assert_eq!(Rounds::<Test>::get((C, E, 4)).unwrap().acks, 2); // survived
        assert_ok!(Oracle::do_try_state());

        // v4 still reaches quorum and settles Unchallenged on its own acks.
        set_block(1 + ORC_WINDOW_BLOCKS);
        assert_ok!(Oracle::crank_round_close(RuntimeOrigin::signed(acc(9)), 20));
        let settled = Oracle::settled_component(C, E, 4).unwrap();
        assert_eq!(settled.path, SettlePath::Unchallenged);
        assert_eq!(settled.value, v4_value);
        assert_ok!(Oracle::do_try_state());
        ExpectedSpecs::set(vec![3]); // restore default for suite isolation
    });
}

// =========================================================================
// 19. Late reserve-probe response (Codex F2; 07 §8 fail-static)
// =========================================================================

#[test]
fn reserve_probe_response_at_or_after_timeout_counts_as_fail() {
    // Codex F2 / 07 §8: a probe response that lands at or after the
    // `res.probe_timeout` deadline is counted as a FAIL regardless of the
    // reported outcome — a late answer is never healthy.
    new_test_ext().execute_with(|| {
        set_block(RES_PROBE_INTERVAL);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(acc(9)))); // probe 1 pending

        // A "passed" response arriving exactly at the timeout deadline is a fail.
        set_block(RES_PROBE_INTERVAL + RES_PROBE_TIMEOUT);
        assert_ok!(Oracle::reserve_probe_result(1, true));
        assert_eq!(ReserveHealth::<Test>::get().consecutive_fails, 1);
        assert_eq!(ReserveHealth::<Test>::get().consecutive_passes, 0);
        assert!(!Oracle::reserve_unhealthy());
        assert_ok!(Oracle::do_try_state());
    });
}

// =========================================================================
// 20. adjudicate is terminal-only (Codex F10; 07 §5.4)
// =========================================================================

#[test]
fn adjudicate_on_a_fresh_round_is_window_open() {
    // Codex F10 / 07 §5.4: adjudication is the TERMINAL step of the game — the
    // `OracleResolution` origin cannot settle a fresh, unchallenged round-1
    // report and thereby bypass the escalation ladder.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_noop!(
            Oracle::adjudicate(
                RuntimeOrigin::signed(oracle_resolution_acc()),
                C,
                E,
                V,
                counter_value(),
                false
            ),
            Error::<Test>::WindowOpen
        );
        assert!(Oracle::settled_component(C, E, V).is_none());
        assert!(Rounds::<Test>::get((C, E, V)).is_some()); // round untouched
    });
}

// =========================================================================
// 21. Money deadline / force-neutralize (Codex F11/F12; 07 §11, I-18)
// =========================================================================

#[test]
fn note_settle_deadline_neutralizes_contested_round_and_blocks_late_verdict() {
    // 07 §11 rule 1: any `(component, m)` not challenge-closed by its
    // `OracleSettleDeadline` settles NEUTRALLY. Once neutral-settled and removed,
    // a late terminal verdict finds no round and cannot overwrite the money (I-18
    // — a late verdict resolves bonds only).
    new_test_ext().execute_with(|| {
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        assert_ok!(Oracle::challenge(
            RuntimeOrigin::signed(acc(4)),
            C,
            E,
            V,
            counter_value(),
            h(10)
        ));

        // The money deadline fires while the round is still contested.
        assert_ok!(Oracle::note_settle_deadline(E));
        let settled = Oracle::settled_component(C, E, V).expect("neutral-settled");
        assert_eq!(settled.path, SettlePath::Neutral);
        assert!(settled.flagged);
        assert_eq!(settled.value, FixedU64(COMPONENT_VALUE_MAX / 2)); // 0.5, no history
        assert!(Rounds::<Test>::get((C, E, V)).is_none()); // no money-bearing round survives
        assert_ok!(Oracle::do_try_state());

        // A late terminal verdict can no longer overwrite the settled money.
        assert_noop!(
            Oracle::adjudicate(
                RuntimeOrigin::signed(oracle_resolution_acc()),
                C,
                E,
                V,
                counter_value(),
                false
            ),
            Error::<Test>::RoundNotFound
        );
    });
}

#[test]
fn note_settle_deadline_neutralizes_no_report_components() {
    // Codex P1 / 07 §11(1): an admitted component that produced NO report has no
    // round, so the live-round sweep never touches it — yet the money-deadline
    // guarantee is that welfare finds a (neutral) value for EVERY expected
    // component. The crank reads the expected `(component, version)` set from the
    // `ReportingContext` provider and neutral-settles the no-report members
    // (07 §10 no-report path).
    new_test_ext().execute_with(|| {
        // Component C reports (opens a round); component 8 is also consumed by the
        // epoch's cohorts but never reports.
        register_reporter(1);
        assert_ok!(do_report(1, E, reported_value(), h(9)));
        ExpectedComponents::set(vec![(C, V), (8, V)]);

        assert_ok!(Oracle::note_settle_deadline(E));

        // C settled neutrally via its round; 8 via the no-report path.
        let reported = Oracle::settled_component(C, E, V).expect("C neutral-settled");
        assert_eq!(reported.path, SettlePath::Neutral);
        assert!(reported.flagged);
        let no_report = Oracle::settled_component(8, E, V).expect("no-report component settled");
        assert_eq!(no_report.path, SettlePath::Neutral);
        assert!(no_report.flagged);
        assert_eq!(no_report.value, FixedU64(COMPONENT_VALUE_MAX / 2)); // 0.5, no history
        assert!(Rounds::<Test>::get((8, E, V)).is_none());
        assert_ok!(Oracle::do_try_state());

        ExpectedComponents::set(vec![]); // restore default for suite isolation
    });
}

// =========================================================================
// 22. Offense against an ejected reporter is a no-op (Codex F17; 07 §3)
// =========================================================================

#[test]
fn offense_against_an_ejected_reporter_does_not_strand_a_valid_recompute() {
    // Codex F17: a reporter ejected on one game can still have another live round
    // settled by `recompute_proof` — the further offense against the (now absent)
    // reporter is a no-op, not a `NotRegistered` error that strands the settle.
    new_test_ext().execute_with(|| {
        register_reporter(1);
        let disproof = proof_for(counter_value()); // recomputes to 0.44, disproving 0.62

        // A pre-existing game on component 30, reported now, recomputed later.
        assert_ok!(Oracle::note_recomputable(30, V));
        assert_ok!(Oracle::report(
            RuntimeOrigin::signed(acc(1)),
            30,
            E,
            V,
            reported_value(),
            hash_evidence(&disproof)
        ));

        // Eject acc(1) via three recompute-disproofs on other components.
        for component in 31..34u16 {
            assert_ok!(Oracle::note_recomputable(component, V));
            assert_ok!(Oracle::report(
                RuntimeOrigin::signed(acc(1)),
                component,
                E,
                V,
                reported_value(),
                hash_evidence(&disproof)
            ));
            assert_ok!(Oracle::recompute_proof(
                RuntimeOrigin::signed(acc(5)),
                component,
                E,
                V,
                proof_arg(disproof.clone())
            ));
        }
        assert_eq!(Reporters::<Test>::count(), 0); // ejected

        // The pre-existing game still settles despite the ejection (no `NotRegistered`).
        assert_ok!(Oracle::recompute_proof(
            RuntimeOrigin::signed(acc(5)),
            30,
            E,
            V,
            proof_arg(disproof)
        ));
        assert!(Oracle::settled_component(30, E, V).is_some());
        assert_ok!(Oracle::do_try_state());
    });
}
