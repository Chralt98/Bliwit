//! Runtime-level welfare-input regressions (milestone A12): SQ-79, SQ-82, SQ-201.
//!
//! These exercise the runtime *composition* — the `WelfareSettlement` seam and
//! the real `pallet-welfare` storage — rather than the pallet in its mock. The
//! pallet-level rules themselves are pinned in `pallets/welfare/src/tests.rs`.

use alloc::{vec, vec::Vec};

use frame_support::traits::Get;

use crate::{configs::RuntimeEpochWelfare, Runtime};

use pallet_epoch::WelfareSettlement;

/// A single-component `C_onchain` spec active from `activation_epoch`, installed
/// directly so these tests do not depend on the (still absent) production
/// genesis MetricSpec set — see the SQ-181 note in PLAN.md.
fn install_spec(
    version: futarchy_primitives::MetricSpecVersion,
    activation_epoch: futarchy_primitives::EpochId,
) {
    for (stored, _) in pallet_welfare::MetricSpecs::<Runtime>::iter() {
        pallet_welfare::MetricSpecs::<Runtime>::remove(stored);
    }
    pallet_welfare::SnapshotDeadline::<Runtime>::kill();
    let spec = pallet_welfare::MetricSpec {
        id: futarchy_primitives::metric_ids::X,
        version,
        pillar: pallet_welfare::Pillar::COnchain,
        weight: futarchy_primitives::FixedU64(pallet_welfare::ONE),
        epsilon_floor: pallet_welfare::EPSILON_PILLAR,
        activation_epoch,
        source: pallet_welfare::SourceClass::Onchain,
        formula_ref: [1; 32],
        units: [2; 16],
        repr: [3; 16],
        cadence_blocks: 1,
        sanity_min: futarchy_primitives::FixedU64(0),
        sanity_max: futarchy_primitives::FixedU64(pallet_welfare::ONE),
        has_normalization_rule: true,
        has_missing_data_rule: true,
        has_gaming_vectors: true,
        has_challenge_procedure: true,
        prior_bounds: [futarchy_primitives::FixedU64(pallet_welfare::ONE);
            pallet_welfare::HISTORY_PRIORS],
    };
    let specs = pallet_welfare::BoundedSpecSet::try_from(vec![spec]).expect("one spec is bounded");
    pallet_welfare::MetricSpecs::<Runtime>::insert(version, specs);
}

/// Seed `count` consecutive epochs of welfare history directly, bypassing the
/// keeper cranks: the point of these tests is the *retirement* path, and the
/// recording path already has pallet-level coverage.
fn seed_history(first: futarchy_primitives::EpochId, count: u32) {
    for offset in 0..count {
        let epoch = first + offset;
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: false,
                c_breached: false,
                day_bitmap: [0; 2],
            },
        );
        pallet_welfare::SampledGateDays::<Runtime>::insert(epoch, [1u32, 0]);
    }
}

// --------------------------------------------------------------- SQ-201
//
// 05 §3.3 made cohort reap the only prune trigger, and `pallet-epoch` calls
// `WelfareSettlement::prune` only from `settle_cohort`. An epoch that never
// forms a cohort is therefore unreachable by cohort-keyed cleanup, and welfare
// history accumulates until `record_snapshot`/`record_daily_gate` jam at their
// hard bounds — a chain wedge, not idle storage. The epoch-roll seam runs on
// every successful tick instead. The bound is what matters here: no number of
// cohortless rolls may grow welfare state without limit.

#[test]
fn sq201_cohortless_rolls_keep_welfare_history_inside_its_bound() {
    crate::tests::development_ext().execute_with(|| {
        install_spec(1, 1);
        // Far more cohortless epochs than the retained window is deep.
        let rolls = 6 * pallet_welfare::MAX_SNAPSHOTS_BOUND;
        let mut observed_peak = 0usize;
        for current in 1..=rolls {
            // One epoch of history arrives per roll and no cohort ever settles,
            // so `WelfareSettlement::prune` is never reached.
            seed_history(current, 1);
            RuntimeEpochWelfare::prune_xcm_traffic(current).expect("roll maintenance is infallible");
            observed_peak = observed_peak.max(pallet_welfare::GateBreachFlags::<Runtime>::iter().count());
        }
        assert!(
            observed_peak <= pallet_welfare::MAX_GATE_FLAGS,
            "welfare gate history grew past its bound over {rolls} cohortless rolls (peak {observed_peak})",
        );
        assert!(
            pallet_welfare::SampledGateDays::<Runtime>::iter().count()
                <= pallet_welfare::MAX_GATE_FLAGS,
            "sampled-day markers grew past their bound",
        );
    });
}

#[test]
fn sq201_the_roll_seam_retires_nothing_inside_the_retained_window() {
    // The roll prune takes the same 05 §3.3 `current - 19` cutoff as the
    // reap-driven prune, so it can never remove state reap would have kept.
    crate::tests::development_ext().execute_with(|| {
        install_spec(1, 1);
        let current = pallet_welfare::MAX_SNAPSHOTS_BOUND;
        let first = current.saturating_sub(pallet_welfare::MAX_SNAPSHOTS_BOUND - 1);
        seed_history(first, pallet_welfare::MAX_SNAPSHOTS_BOUND);
        let before = pallet_welfare::GateBreachFlags::<Runtime>::iter().count();
        RuntimeEpochWelfare::prune_xcm_traffic(current).expect("roll maintenance is infallible");
        assert_eq!(
            pallet_welfare::GateBreachFlags::<Runtime>::iter().count(),
            before,
            "the roll prune retired state inside the retained window",
        );
    });
}

#[test]
fn sq201_a_backlog_is_drained_across_rolls_not_in_one_call() {
    // I-20: the catch-up is cursor-bounded, so a pathological historical
    // backlog is spread over successive ticks instead of one unbounded call.
    crate::tests::development_ext().execute_with(|| {
        install_spec(1, 1);
        seed_history(1, pallet_welfare::MAX_SNAPSHOTS_BOUND);
        let seeded = pallet_welfare::GateBreachFlags::<Runtime>::iter().count();
        // A clock far past the whole seeded history: every epoch is retirable.
        let current = 10 * pallet_welfare::MAX_SNAPSHOTS_BOUND;
        RuntimeEpochWelfare::prune_xcm_traffic(current).expect("roll maintenance is infallible");
        let after_one = pallet_welfare::GateBreachFlags::<Runtime>::iter().count();
        assert_eq!(
            seeded - after_one,
            pallet_welfare::EPOCH_ROLL_PRUNE_MAX_EPOCHS,
            "one roll retired more than its bounded batch",
        );
        // Repeated rolls do drain it.
        for _ in 0..seeded {
            RuntimeEpochWelfare::prune_xcm_traffic(current)
                .expect("roll maintenance is infallible");
        }
        assert_eq!(
            pallet_welfare::GateBreachFlags::<Runtime>::iter().count(),
            0
        );
    });
}

// ---------------------------------------------------------------- SQ-79
//
// The runtime binds `RuntimeEpochWelfare::compute_settlement` to the welfare
// pallet's single settlement path (05 §6). A cohort whose e+1…e+2 gate window
// carries no observation at all must fail there, holding the ledger at the
// status quo, rather than settling gate books at "no breach".

#[test]
fn sq79_the_runtime_settlement_seam_refuses_an_unsampled_gate_window() {
    crate::tests::development_ext().execute_with(|| {
        install_spec(1, 1);
        let result = RuntimeEpochWelfare::compute_settlement(
            10,
            1,
            pallet_epoch::SettlementTarget::Proposal {
                pid: 42,
                has_gate_books: true,
            },
        );
        assert!(
            result.is_err(),
            "settlement over a wholly unobserved gate window must not succeed",
        );
    });
}

// ---------------------------------------------------------------- SQ-82
//
// `register_spec` is a live dispatch and must never reach the genesis
// activation relaxation, whatever the epoch clock reads. The runtime binds
// `CurrentEpoch = pallet_epoch::CurrentEpoch`, so this pins the composed
// behavior rather than the mock's.

#[test]
fn sq82_the_runtime_register_spec_origin_set_is_closed_and_lead_bound() {
    crate::tests::development_ext().execute_with(|| {
        let current = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let specs: Vec<pallet_welfare::MetricSpec> = vec![];
        // Negative origin: a signed account is never the metric-track authority
        // (06 §3.2), so the call is refused before any activation check.
        let refused = pallet_welfare::Pallet::<Runtime>::register_spec(
            crate::RuntimeOrigin::signed(crate::tests::account(1)),
            9,
            pallet_welfare::BoundedSpecSet::try_from(specs).expect("empty set is bounded"),
        );
        assert!(refused.is_err(), "a signed origin must not register specs");
        // The clock the live path reads is the real epoch clock, never a
        // genesis sentinel supplied by the caller.
        assert_eq!(
            current,
            <Runtime as pallet_welfare::Config>::CurrentEpoch::get(),
        );
    });
}
