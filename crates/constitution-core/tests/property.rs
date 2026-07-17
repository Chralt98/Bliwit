//! Constitution bound/meter properties (15 §1 I-6/I-7 and §4.3).

use constitution_core::{
    key16, ConstitutionState, MaxDelta, Meter, ParamClass, ParamRecord, ParamValue,
};
use futarchy_primitives::FixedU64;
use proptest::{
    prelude::*,
    test_runner::{Config as ProptestConfig, RngSeed},
};

fn property_config(seed: u64) -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_RNG_SEED").is_none() {
        config.rng_seed = RngSeed::Fixed(seed);
    }
    config.failure_persistence = None;
    config
}

#[derive(Clone, Copy, Debug)]
enum MeterOp {
    Tick(u8),
    Charge { epoch_advance: u8, amount: u64 },
}

fn meter_op_strategy() -> impl Strategy<Value = MeterOp> {
    prop_oneof![
        (1u8..=4).prop_map(MeterOp::Tick),
        (0u8..=4, 0u64..=2_000_000u64).prop_map(|(epoch_advance, amount)| {
            MeterOp::Charge {
                epoch_advance,
                amount,
            }
        }),
    ]
}

fn value(kind: u8, raw: u128) -> ParamValue {
    match kind % 6 {
        0 => ParamValue::U8(raw.min(u128::from(u8::MAX)) as u8),
        1 => ParamValue::U32(raw.min(u128::from(u32::MAX)) as u32),
        2 => ParamValue::Balance(raw),
        3 => ParamValue::Fixed(FixedU64(raw.min(u128::from(u64::MAX)) as u64)),
        4 => ParamValue::Percent(raw.min(u128::from(u8::MAX)) as u8),
        _ => ParamValue::Perbill(raw.min(u128::from(u32::MAX)) as u32),
    }
}

/// Independent interval oracle for I-6 (15 §1).  It constructs the admitted
/// closed interval and intersects it with the registry bounds; it does not
/// mirror `ParamRecord::checked_update`'s candidate-delta predicates.
fn allowed_interval(record: &ParamRecord) -> (u128, u128) {
    let current = record.value.as_u128();
    let (delta_low, delta_high) = match record.max_delta {
        None => (0, u128::MAX),
        Some(MaxDelta::Absolute(bound)) => (
            current.saturating_sub(bound.as_u128()),
            current.saturating_add(bound.as_u128()),
        ),
        Some(MaxDelta::Percent(percent)) => {
            // Exact rational interval followed by the conservative integer
            // grid: allowance = floor(current·percent/100).
            let numerator = current
                .checked_mul(u128::from(percent))
                .expect("generated registry values are bounded");
            let radius = numerator / 100;
            (
                current.saturating_sub(radius),
                current.saturating_add(radius),
            )
        }
        Some(MaxDelta::Factor(factor)) => {
            let factor = u128::from(factor);
            if factor == 0 {
                (1, 0)
            } else {
                let reciprocal_floor = current / factor;
                let reciprocal_ceil = reciprocal_floor + u128::from(current % factor != 0);
                (reciprocal_ceil, current.saturating_mul(factor))
            }
        }
    };
    (
        delta_low.max(record.min.as_u128()),
        delta_high.min(record.max.as_u128()),
    )
}

fn update_allowed(record: &ParamRecord, next: ParamValue, epoch: u32) -> bool {
    if !record.value.same_kind(next)
        || !record.min.same_kind(next)
        || !record.max.same_kind(next)
        || epoch
            < record
                .last_changed_epoch
                .saturating_add(record.cooldown_epochs)
    {
        return false;
    }
    let (low, high) = allowed_interval(record);
    (low..=high).contains(&next.as_u128())
}

proptest! {
    #![proptest_config(property_config(0x1ed6_0007))]

    /// I-7: consumption is monotone within a window, a successful tick resets
    /// exactly, and every refused charge (including a lazy future-window
    /// reset) leaves all meter fields unchanged.
    #[test]
    fn i7_meter_is_monotone_reset_exact_and_refusal_is_noop(
        limit in 0u128..=1_000_000u128,
        ops in prop::collection::vec(meter_op_strategy(), 1..64),
    ) {
        let mut meter = Meter::new(limit, 0);
        let mut epoch = 0u32;

        for op in ops {
            let before = meter;
            match op {
                MeterOp::Tick(advance) => {
                    epoch = epoch.saturating_add(u32::from(advance));
                    meter.charge(0, epoch).unwrap();
                    prop_assert_eq!(meter.reset_epoch, epoch);
                    prop_assert_eq!(meter.spent, 0, "window rollover must reset exactly");
                }
                MeterOp::Charge { epoch_advance, amount } => {
                    epoch = epoch.saturating_add(u32::from(epoch_advance));
                    let result = meter.charge(u128::from(amount), epoch);
                    if result.is_err() {
                        prop_assert_eq!(meter, before, "refused charge mutated the meter");
                    } else if epoch > before.reset_epoch {
                        prop_assert_eq!(meter.reset_epoch, epoch);
                        prop_assert_eq!(meter.spent, u128::from(amount));
                    } else {
                        prop_assert_eq!(meter.reset_epoch, before.reset_epoch);
                        prop_assert!(meter.spent >= before.spent, "in-window meter regressed");
                        prop_assert_eq!(meter.spent, before.spent + u128::from(amount));
                    }
                    prop_assert!(meter.spent <= meter.limit);
                }
            }
        }
    }

    /// I-6: randomly-shaped registry rows and random set_param sequences can
    /// never store a value outside bounds, beyond max-Δ, or before cooldown.
    /// Every rejected update is a strict state no-op.
    #[test]
    fn i6_set_param_sequences_enforce_bounds_delta_cooldown_and_atomicity(
        kind in 0u8..6,
        min_raw in 0u128..=100u128,
        span in 1u128..=100u128,
        offset in 0u128..=100u128,
        delta_kind in 0u8..4,
        delta_raw in 0u128..=100u128,
        cooldown in 0u32..=5u32,
        last_changed in 0u32..=5u32,
        actions in prop::collection::vec((0u32..=6u32, 0u128..=300u128), 1..64),
    ) {
        // Synthetic rows remain type-valid: Percent registry bounds never
        // exceed 100; the other generated kinds use a compact 0..200 domain.
        let ceiling = if kind % 6 == 4 { 100 } else { 200 };
        let min_raw = min_raw.min(ceiling - 1);
        let span = span.min(ceiling - min_raw).max(1);
        let max_raw = min_raw + span;
        let current_raw = min_raw + offset % (span + 1);
        let max_delta = match delta_kind {
            0 => None,
            1 => Some(MaxDelta::Absolute(value(kind, delta_raw))),
            2 => Some(MaxDelta::Percent((delta_raw % 100 + 1) as u8)),
            _ => Some(MaxDelta::Factor((delta_raw % 4 + 1) as u8)),
        };
        let key = key16(b"property.row");
        let row = ParamRecord {
            key,
            value: value(kind, current_raw),
            min: value(kind, min_raw),
            max: value(kind, max_raw),
            max_delta,
            cooldown_epochs: cooldown,
            last_changed_epoch: last_changed,
            class: ParamClass::Param,
            kernel_bounded: false,
        };
        let mut state = ConstitutionState::genesis();
        state.params = vec![row];
        state.try_state().unwrap();
        let mut epoch = last_changed;

        for (advance, candidate_raw) in actions {
            epoch = epoch.saturating_add(advance);
            let next = value(kind, candidate_raw);
            let before = state.clone();
            let previous = before.params[0];
            let expected = update_allowed(&previous, next, epoch);
            let result = state.set_param(key, next, epoch);
            prop_assert_eq!(
                result.is_ok(),
                expected,
                "I-6 independent interval disagrees: {:?}, candidate {}, epoch {}",
                previous.max_delta,
                next.as_u128(),
                epoch,
            );
            if result.is_err() {
                prop_assert_eq!(&state, &before, "rejected set_param mutated state");
            } else {
                let stored = state.params[0];
                prop_assert_eq!(stored.value, next);
                prop_assert!(stored.value.as_u128() >= previous.min.as_u128());
                prop_assert!(stored.value.as_u128() <= previous.max.as_u128());
                prop_assert!(
                    epoch >= previous.last_changed_epoch.saturating_add(previous.cooldown_epochs),
                    "cooldown bypass"
                );
                let (low, high) = allowed_interval(&previous);
                prop_assert!(
                    (low..=high).contains(&next.as_u128()),
                    "max-delta interval bypass"
                );
                prop_assert_eq!(stored.last_changed_epoch, epoch);
            }
            prop_assert_eq!(state.try_state(), Ok(()));
        }
    }
}

fn interval_row(current: u32, min: u32, max: u32, delta: MaxDelta) -> ParamRecord {
    ParamRecord {
        key: key16(b"boundary.row"),
        value: ParamValue::U32(current),
        min: ParamValue::U32(min),
        max: ParamValue::U32(max),
        max_delta: Some(delta),
        cooldown_epochs: 0,
        last_changed_epoch: 0,
        class: ParamClass::Param,
        kernel_bounded: false,
    }
}

fn assert_interval_table(record: ParamRecord, cases: &[(u32, bool)]) {
    for (candidate, expected) in cases {
        let next = ParamValue::U32(*candidate);
        assert_eq!(
            update_allowed(&record, next, 0),
            *expected,
            "oracle table candidate {candidate}"
        );
        assert_eq!(
            record.checked_update(next, 0).is_ok(),
            *expected,
            "implementation table candidate {candidate}"
        );
    }
}

#[test]
fn i6_delta_interval_boundary_tables() {
    // Zero/current-value edge: a percentage of zero has zero radius.
    assert_interval_table(
        interval_row(0, 0, 100, MaxDelta::Percent(25)),
        &[(0, true), (1, false), (100, false)],
    );

    // Exact percentage boundaries around floor(37 * 10% / 100) = 3.
    assert_interval_table(
        interval_row(37, 0, 100, MaxDelta::Percent(10)),
        &[(33, false), (34, true), (37, true), (40, true), (41, false)],
    );

    // Factor reciprocal uses ceil(current/factor): ceil(10/3) = 4.
    assert_interval_table(
        interval_row(10, 0, 40, MaxDelta::Factor(3)),
        &[(3, false), (4, true), (10, true), (30, true), (31, false)],
    );

    // Registry min/max intersect the delta interval at both edges.
    assert_interval_table(
        interval_row(5, 5, 20, MaxDelta::Absolute(ParamValue::U32(10))),
        &[(4, false), (5, true), (15, true), (16, false), (20, false)],
    );
    assert_interval_table(
        interval_row(20, 5, 20, MaxDelta::Absolute(ParamValue::U32(10))),
        &[
            (4, false),
            (5, false),
            (9, false),
            (10, true),
            (20, true),
            (21, false),
        ],
    );
}

#[test]
fn refused_future_window_charge_does_not_persist_lazy_reset() {
    // Historical I-7 regression: the failed charge must not commit the reset.
    let mut meter = Meter::new(10, 7);
    meter.charge(6, 7).unwrap();
    let before = meter;
    assert!(meter.charge(11, 8).is_err());
    assert_eq!(meter, before);
}
