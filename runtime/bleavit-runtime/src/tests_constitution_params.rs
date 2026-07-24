//! A13 (batch X) constitution/params cluster — runtime-level regressions.
//!
//! Kept in a dedicated module (not `tests.rs`) per the session's file-contention
//! rule. Covers the SQ-158 dispute-merit consumer (`max(live dis.merit_min,
//! frozen B_1)`) and the SQ-117/SQ-158 genesis seeds as observed by the real
//! runtime. The pallet-level policy pins live in `pallet-constitution`'s suite
//! and the SQ-150 origin/classifier pins in `tests_s5_behavior`.

use crate::tests::development_ext;
use crate::{Balance, Runtime, System};
use frame_support::traits::Get;
use pallet_epoch::OracleAccess;

const COMPONENT: futarchy_primitives::MetricId = 51;
const SPEC: futarchy_primitives::MetricSpecVersion = 27;
/// A frozen round-1 bond equal to the `orc.bond_floor` / `dis.merit_min`
/// genesis default (10k USDC), so at launch `max(live, B_1)` reduces to `B_1`.
const FROZEN_B1: Balance = 10_000_000_000;

fn set_balance_param(name: &[u8], value: Balance) {
    let key = pallet_constitution::key16(name);
    pallet_constitution::Params::<Runtime>::mutate(key, |record| {
        let record = record
            .as_mut()
            .unwrap_or_else(|| panic!("missing genesis balance param {name:?}"));
        record.value = pallet_constitution::ParamValue::Balance(value);
    });
}

/// Seed one open, challenged §5 oracle round on a consumed component whose
/// frozen round-1 bond is `FROZEN_B1` and whose posted bond is `posted`.
fn seed_open_challenged_round(posted: Balance) {
    let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
    pallet_oracle::Rounds::<Runtime>::insert(
        (COMPONENT, epoch, SPEC),
        pallet_oracle::RoundState {
            component: COMPONENT,
            epoch,
            round: 1,
            spec_version: SPEC,
            reporter: [51; 32],
            value: futarchy_primitives::FixedU64(500_000_000),
            evidence_hash: [52; 32],
            bond: posted,
            challenge_deadline: System::block_number().saturating_add(1),
            extended: false,
            challenger: Some([53; 32]),
            counter_value: Some(futarchy_primitives::FixedU64(400_000_000)),
            acks: 0,
            report_hash: [54; 32],
            stake_at_risk: 400_000_000_000,
            cumulative_reporter_bond: FROZEN_B1,
            cumulative_challenger_bond: FROZEN_B1,
        },
    );
    pallet_oracle::RoundSchedules::<Runtime>::insert(
        (COMPONENT, epoch, SPEC),
        pallet_oracle::StoredRoundSchedule {
            round_one_bond: FROZEN_B1,
            round_cap: pallet_oracle::ORC_ROUNDS,
        },
    );
}

/// SQ-158: raising the independent `dis.merit_min` META lever raises the
/// ProcessHold bar above the game's own `B_1`, so a dispute bonded exactly at
/// `B_1` stops holding the decision once the lever moves past it.
#[test]
fn sq_158_raised_dis_merit_min_raises_the_processhold_bar() {
    development_ext().execute_with(|| {
        seed_open_challenged_round(FROZEN_B1);

        // Default (`dis.merit_min` == orc.bond_floor == B_1): floor = B_1, the
        // dispute qualifies and holds the decision.
        assert!(
            crate::configs::RuntimeEpochOracle::any_open_dispute_touching(SPEC),
            "at the default merit floor a B_1-bonded dispute must hold the decision"
        );

        // Raise the lever one micro-USDC above the posted bond: floor now
        // exceeds round.bond, so the dispute no longer holds.
        set_balance_param(b"dis.merit_min", FROZEN_B1.saturating_add(1));
        assert!(
            !crate::configs::RuntimeEpochOracle::any_open_dispute_touching(SPEC),
            "a raised dis.merit_min must lift the ProcessHold bar above B_1"
        );
    });
}

/// SQ-158 (R-7): the consumer takes `max(live, B_1)`, so lowering the lever
/// below the game's frozen `B_1` can NEVER make censorship cheaper — the floor
/// stays pinned at `B_1` and a `B_1`-bonded dispute keeps holding.
#[test]
fn sq_158_lowered_dis_merit_min_cannot_drop_below_the_frozen_game_bond() {
    development_ext().execute_with(|| {
        seed_open_challenged_round(FROZEN_B1);

        // Drive the live lever far below B_1.
        set_balance_param(b"dis.merit_min", 1);
        assert!(
            crate::configs::RuntimeEpochOracle::any_open_dispute_touching(SPEC),
            "max(live, B_1) must keep the floor at B_1 despite a lowered lever"
        );

        // A dispute bonded strictly below B_1 still never qualifies, even with
        // the lever floored — the frozen game bond is the true minimum.
        pallet_oracle::Rounds::<Runtime>::remove((
            COMPONENT,
            pallet_epoch::CurrentEpoch::<Runtime>::get(),
            SPEC,
        ));
        seed_open_challenged_round(FROZEN_B1.saturating_sub(1));
        assert!(
            !crate::configs::RuntimeEpochOracle::any_open_dispute_touching(SPEC),
            "a sub-B_1 dispute must not hold even when dis.merit_min is floored"
        );
    });
}

/// A challenger remains recorded for later bond settlement, but only the
/// current round's counter value makes it an active decision-time dispute.
#[test]
fn durable_challenger_without_current_counter_does_not_hold_decision() {
    development_ext().execute_with(|| {
        seed_open_challenged_round(FROZEN_B1);
        pallet_oracle::Rounds::<Runtime>::mutate(
            (
                COMPONENT,
                pallet_epoch::CurrentEpoch::<Runtime>::get(),
                SPEC,
            ),
            |round| {
                let round = round.as_mut().expect("seeded round");
                round.counter_value = None;
            },
        );

        assert!(
            !crate::configs::RuntimeEpochOracle::any_open_dispute_touching(SPEC),
            "a durable challenger without a current counter-report must not hold",
        );
    });
}

/// SQ-117 / SQ-158: the real runtime genesis seeds both formerly-unseeded rows,
/// so B9's rebate pipeline reads a non-zero `keeper.rebate` and the dispute
/// engine reads an independent `dis.merit_min`.
#[test]
fn sq_117_sq_158_runtime_genesis_seeds_the_two_rows() {
    development_ext().execute_with(|| {
        let rebate = pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"keeper.rebate",
        ))
        .expect("keeper.rebate must be genesis-seeded (SQ-117)");
        assert!(
            matches!(rebate.value, pallet_constitution::ParamValue::Balance(v) if v > 0),
            "keeper.rebate seed must be a positive balance, got {:?}",
            rebate.value
        );

        let merit = pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"dis.merit_min",
        ))
        .expect("dis.merit_min must be genesis-seeded (SQ-158)");
        assert_eq!(merit.class, pallet_constitution::ParamClass::Meta);
        assert!(!merit.kernel_bounded);
    });
}
