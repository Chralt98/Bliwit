//! Shared JSON welfare-pipeline replay for the welfare core (15 §4.4; 05 §4.4).
//!
//! The `welfare_scenarios` corpus family is generated only by
//! `tools/reference-model/generate-vectors.py`. 15 §4.4 makes the welfare
//! pipeline the "two conforming implementations" case: the reference model and
//! this core MUST reproduce the per-step 64.64/1e9 rounding grid-exactly
//! (bit-identical pillar values, `W_e`, `s`). Every assertion below is
//! therefore byte-exact on the 1e9 grid — no tolerances.

use std::{fs, path::PathBuf};

use futarchy_primitives::{FixedU64, MetricId};
use serde_json::Value;
use welfare_core::{
    settlement_score, ComponentValue, MetricSpec, Pillar, Registration, SourceClass, WelfareParams,
    WelfareState, EPSILON_PILLAR, ONE,
};

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference-model/fixtures/vectors.json");
    serde_json::from_str(&fs::read_to_string(path).expect("read shared reference-model vectors"))
        .expect("parse shared reference-model vectors")
}

/// Parse a corpus decimal string onto the 1e9 grid, exactly.
fn exact_1e9(value: &Value, context: &str) -> u64 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("{context} must be a decimal string"));
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    assert!(
        frac.len() <= 9,
        "{context} value {text} is not exactly representable at 1e9"
    );
    let mut raw: u64 = int
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("{context} integer part"))
        * 1_000_000_000;
    let mut digits = frac.to_owned();
    while digits.len() < 9 {
        digits.push('0');
    }
    raw += digits
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("{context} fraction part"));
    raw
}

fn spec(id: MetricId, pillar: Pillar, source: SourceClass, weight_1e9: u64) -> MetricSpec {
    MetricSpec {
        id,
        version: 1,
        pillar,
        weight: FixedU64(weight_1e9),
        epsilon_floor: EPSILON_PILLAR,
        activation_epoch: 1,
        source,
        formula_ref: [0; 32],
        units: [0; 16],
        repr: [0; 16],
        cadence_blocks: 1,
        sanity_min: FixedU64(0),
        sanity_max: FixedU64(ONE),
        has_normalization_rule: true,
        has_missing_data_rule: true,
        has_gaming_vectors: true,
        has_challenge_procedure: true,
        prior_bounds: [FixedU64(0); welfare_core::HISTORY_PRIORS],
    }
}

/// MetricId assignment: within every pillar map the reference model iterates
/// components in ascending lexicographic name order (05 §4.4 determinism
/// discipline); the ids below preserve exactly that order per pillar, so the
/// core's ascending-id iteration replays the same term order.
const ID_C01: MetricId = 1;
const ID_C02: MetricId = 2;
const ID_C03: MetricId = 3;
const ID_P01: MetricId = 4;
const ID_P02: MetricId = 5;
const ID_A01: MetricId = 6;
const ID_A02: MetricId = 7;
const ID_S_U: MetricId = 8;
const ID_S_F: MetricId = 9;
const ID_S_DEFF: MetricId = 10;

struct PipelineInputs {
    u: u64,
    f: u64,
    d_eff: u64,
    incident: u64,
    c_onchain: [(MetricId, u64); 2],
    c_attested: [(MetricId, u64); 1],
    c_weights: [(MetricId, u64); 3],
    c_daily: [(MetricId, u64); 2],
    p: [(MetricId, u64, u64); 2],
    a: [(MetricId, u64, u64); 2],
}

fn map_entry(inputs: &Value, group: &str, key: &str) -> u64 {
    exact_1e9(&inputs[group][key], &format!("{group}.{key}"))
}

fn pipeline_inputs(inputs: &Value) -> PipelineInputs {
    // Fail loudly if the fixture grows components this harness does not map.
    for (group, expected) in [
        ("c_onchain", vec!["C01", "C02"]),
        ("c_attested", vec!["C03"]),
        ("c_weights", vec!["C01", "C02", "C03"]),
        ("c_daily", vec!["C01", "C02"]),
        ("p_components", vec!["P01", "P02"]),
        ("p_weights", vec!["P01", "P02"]),
        ("a_components", vec!["A01", "A02"]),
        ("a_weights", vec!["A01", "A02"]),
    ] {
        let object = inputs[group].as_object().expect("component map");
        let mut names: Vec<&str> = object.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, expected, "unmapped {group} inventory in fixture");
    }

    // D_eff (05 §4.5): min(1, (1 − HHI)/(1 − 1/N_cap)) floored to the 1e9
    // grid, N_cap = 5 for phase ≤ 3. There is no frame-free Rust producer for
    // this derivation yet (the pallet consumes D_eff as an oracle component),
    // so the harness evaluates the spec formula in exact integer arithmetic
    // and pins it against the vector's D_eff below.
    let phase = inputs["phase"].as_u64().expect("phase");
    let n_cap: u64 = match phase {
        0..=3 => 5,
        4 => 6,
        5 => 7,
        _ => 8,
    };
    let hhi = exact_1e9(&inputs["hhi"], "hhi");
    let one = u128::from(ONE);
    let denominator = one - one / u128::from(n_cap);
    let d_eff = u64::try_from((one * (one - u128::from(hhi)) / denominator).min(one))
        .expect("d_eff fits the 1e9 grid");

    PipelineInputs {
        u: exact_1e9(&inputs["u"], "u"),
        f: exact_1e9(&inputs["f"], "f"),
        d_eff,
        incident: exact_1e9(&inputs["incident"], "incident"),
        c_onchain: [
            (ID_C01, map_entry(inputs, "c_onchain", "C01")),
            (ID_C02, map_entry(inputs, "c_onchain", "C02")),
        ],
        c_attested: [(ID_C03, map_entry(inputs, "c_attested", "C03"))],
        c_weights: [
            (ID_C01, map_entry(inputs, "c_weights", "C01")),
            (ID_C02, map_entry(inputs, "c_weights", "C02")),
            (ID_C03, map_entry(inputs, "c_weights", "C03")),
        ],
        c_daily: [
            (ID_C01, map_entry(inputs, "c_daily", "C01")),
            (ID_C02, map_entry(inputs, "c_daily", "C02")),
        ],
        p: [
            (
                ID_P01,
                map_entry(inputs, "p_components", "P01"),
                map_entry(inputs, "p_weights", "P01"),
            ),
            (
                ID_P02,
                map_entry(inputs, "p_components", "P02"),
                map_entry(inputs, "p_weights", "P02"),
            ),
        ],
        a: [
            (
                ID_A01,
                map_entry(inputs, "a_components", "A01"),
                map_entry(inputs, "a_weights", "A01"),
            ),
            (
                ID_A02,
                map_entry(inputs, "a_components", "A02"),
                map_entry(inputs, "a_weights", "A02"),
            ),
        ],
    }
}

fn weight_of(weights: &[(MetricId, u64)], id: MetricId) -> u64 {
    weights
        .iter()
        .find(|(candidate, _)| *candidate == id)
        .expect("weight present")
        .1
}

/// The faithful metric-spec classification of the full-pipeline row: C01/C02
/// on-chain, C03 attested, plus the P/A vectors and the three S components
/// (u, f, D_eff — S is min-aggregated, weights unused).
fn faithful_specs(inputs: &PipelineInputs) -> Vec<MetricSpec> {
    let mut specs = vec![
        spec(ID_S_U, Pillar::S, SourceClass::Onchain, 0),
        spec(ID_S_F, Pillar::S, SourceClass::Onchain, 0),
        spec(ID_S_DEFF, Pillar::S, SourceClass::RelayDerived, 0),
    ];
    for (id, _) in inputs.c_onchain {
        specs.push(spec(
            id,
            Pillar::COnchain,
            SourceClass::Onchain,
            weight_of(&inputs.c_weights, id),
        ));
    }
    for (id, _) in inputs.c_attested {
        specs.push(spec(
            id,
            Pillar::CAttested,
            SourceClass::Attested,
            weight_of(&inputs.c_weights, id),
        ));
    }
    for (id, _, weight) in inputs.p {
        specs.push(spec(id, Pillar::P, SourceClass::Onchain, weight));
    }
    for (id, _, weight) in inputs.a {
        specs.push(spec(id, Pillar::A, SourceClass::Attested, weight));
    }
    specs
}

fn snapshot_components(inputs: &PipelineInputs) -> Vec<ComponentValue> {
    let mut components = vec![
        ComponentValue {
            id: ID_S_U,
            value: FixedU64(inputs.u),
        },
        ComponentValue {
            id: ID_S_F,
            value: FixedU64(inputs.f),
        },
        ComponentValue {
            id: ID_S_DEFF,
            value: FixedU64(inputs.d_eff),
        },
    ];
    for (id, value) in inputs.c_onchain.iter().chain(&inputs.c_attested) {
        components.push(ComponentValue {
            id: *id,
            value: FixedU64(*value),
        });
    }
    for (id, value, _) in inputs.p.iter().chain(&inputs.a) {
        components.push(ComponentValue {
            id: *id,
            value: FixedU64(*value),
        });
    }
    components
}

/// Pin the exact daily gate value through the production breach comparator:
/// `record_daily_gate` computes the daily composite and latches a breach iff
/// it is `< theta_lo`. Probing the same components against two adjacent
/// thresholds pins the composite to one exact grid value without any
/// non-production accessor.
fn daily_c_equals(inputs: &PipelineInputs, expected: u64) {
    for (theta_c_lo, expect_breach) in [(expected, false), (expected + 1, true)] {
        let mut params = WelfareParams::DEFAULT;
        params.theta_c_lo = FixedU64(theta_c_lo);
        assert!(
            theta_c_lo < params.theta_c_hi.0,
            "probe threshold must stay a valid parameterization"
        );
        let mut state = WelfareState::new();
        state
            .register_metric_spec(Registration::Genesis, 1, faithful_specs(inputs))
            .expect("register faithful spec");
        let mut components = vec![
            ComponentValue {
                id: ID_S_U,
                value: FixedU64(inputs.u),
            },
            ComponentValue {
                id: ID_S_F,
                value: FixedU64(inputs.f),
            },
            ComponentValue {
                id: ID_S_DEFF,
                value: FixedU64(inputs.d_eff),
            },
        ];
        for (id, value) in inputs.c_daily {
            components.push(ComponentValue {
                id,
                value: FixedU64(value),
            });
        }
        for (id, value, _) in inputs.p.iter().chain(&inputs.a) {
            components.push(ComponentValue {
                id: *id,
                value: FixedU64(*value),
            });
        }
        let (flags, _changed) = state
            .record_daily_gate(1, 0, 1, components, &params)
            .expect("daily gate records");
        assert_eq!(
            flags.c_breached, expect_breach,
            "C_daily is not exactly {expected} (probe threshold {theta_c_lo})"
        );
    }
}

/// 15 §4.4 / G0 corpus-family attestation: replay every `welfare_scenarios`
/// row grid-exactly against the production welfare core. Unknown or renamed
/// scenarios fail loudly; the executed count is pinned.
#[test]
fn welfare_vectors_match_python_reference_model_grid_exactly() {
    let fixture = fixture();
    let scenarios = fixture["welfare_scenarios"]
        .as_array()
        .expect("welfare_scenarios family present");
    assert_eq!(scenarios.len(), 4, "welfare family cardinality drifted");

    for row in scenarios {
        let name = row["name"].as_str().expect("scenario name");
        let inputs = &row["inputs"];
        match name {
            "equal_horizons" | "mixed_horizons" => {
                // 05 §4.4 (4): the two-epoch settlement score, bit-identical.
                let score = settlement_score(
                    FixedU64(exact_1e9(&inputs["w_next"], "w_next")),
                    FixedU64(exact_1e9(&inputs["w_next_2"], "w_next_2")),
                )
                .expect("settlement score computes");
                let expected = exact_1e9(&row["s"], "s");
                // 05 §4.4 (4) + 15 §4.4: bit-identical settlement scores
                // across the two conforming implementations. The production
                // core computes the exact grid floor of the geometric mean
                // (integer sqrt), so corpus agreement is exact — including
                // means that land exactly ON the 1e9 grid (0.8 and 0.4 here),
                // which the former approximate exp2/log2 evaluation missed by
                // one grid ulp (the G0-suite-reported defect, fixed 2026-07-18).
                assert_eq!(score, FixedU64(expected), "{name}: settlement score");
            }
            "full_pipeline" | "full_pipeline_non_binary_daily_weights" => {
                let parsed = pipeline_inputs(inputs);
                let outputs = &row["outputs"];
                // Pin the harness-derived D_eff to the vector before it feeds
                // the S pillar (see the derivation note in pipeline_inputs).
                assert_eq!(
                    parsed.d_eff,
                    exact_1e9(&outputs["D_eff"], "D_eff"),
                    "{name} D_eff"
                );

                // Faithful classification: S/P/A pillars and W from one
                // production snapshot.
                let mut state = WelfareState::new();
                state
                    .register_metric_spec(Registration::Genesis, 1, faithful_specs(&parsed))
                    .expect("register faithful spec");
                let welfare = state
                    .record_snapshot(
                        1,
                        1,
                        snapshot_components(&parsed),
                        FixedU64(parsed.incident),
                        &WelfareParams::DEFAULT,
                    )
                    .expect("snapshot records");
                state.try_state().expect("welfare state stays consistent");
                let view = state
                    .current_view(1, 1, false)
                    .expect("snapshot view reads back");
                assert_eq!(
                    view.s_pillar_1e9,
                    FixedU64(exact_1e9(&outputs["S"], "S")),
                    "{name} S pillar"
                );
                assert_eq!(
                    view.p_pillar_1e9,
                    FixedU64(exact_1e9(&outputs["P"], "P")),
                    "{name} P pillar"
                );
                assert_eq!(
                    view.a_pillar_1e9,
                    FixedU64(exact_1e9(&outputs["A"], "A")),
                    "{name} A pillar"
                );
                assert_eq!(
                    view.w_current_1e9,
                    FixedU64(exact_1e9(&outputs["W"], "W")),
                    "{name} W"
                );
                assert_eq!(welfare, view.w_current_1e9, "{name} snapshot/view W");

                // The settlement C_e (incident × joint weighted geometric,
                // 05 §4.4 (2)) is not a stored snapshot field; replay it
                // through the same production arithmetic by registering the
                // identical joint C vector under the attested sub-pillar,
                // whose stored value is exactly `mul_down(incident, geo)`.
                let mut c_specs = vec![
                    spec(ID_S_U, Pillar::S, SourceClass::Onchain, 0),
                    spec(ID_S_F, Pillar::S, SourceClass::Onchain, 0),
                    spec(ID_S_DEFF, Pillar::S, SourceClass::RelayDerived, 0),
                ];
                for (id, _) in parsed.c_onchain.iter().chain(&parsed.c_attested) {
                    c_specs.push(spec(
                        *id,
                        Pillar::CAttested,
                        SourceClass::Attested,
                        weight_of(&parsed.c_weights, *id),
                    ));
                }
                for (id, _, weight) in parsed.p {
                    c_specs.push(spec(id, Pillar::P, SourceClass::Onchain, weight));
                }
                for (id, _, weight) in parsed.a {
                    c_specs.push(spec(id, Pillar::A, SourceClass::Attested, weight));
                }
                let mut c_state = WelfareState::new();
                c_state
                    .register_metric_spec(Registration::Genesis, 1, c_specs)
                    .expect("register joint-C spec");
                c_state
                    .record_snapshot(
                        1,
                        1,
                        snapshot_components(&parsed),
                        FixedU64(parsed.incident),
                        &WelfareParams::DEFAULT,
                    )
                    .expect("joint-C snapshot records");
                let c_view = c_state
                    .current_view(1, 1, false)
                    .expect("joint-C view reads back");
                assert_eq!(
                    c_view.c_attested_1e9,
                    FixedU64(exact_1e9(&outputs["C"], "C")),
                    "{name} settlement C"
                );

                // C_daily (renormalized on-chain composite, 05 §4.4) via the
                // production breach comparator, pinned to one grid value.
                daily_c_equals(&parsed, exact_1e9(&outputs["C_daily"], "C_daily"));

                // S_daily: the vector asserts the daily S equals the snapshot
                // S (same min-fold); the exact value sits below the kernel
                // floor of `welfare.thetaS` (0.90), so the production breach
                // comparator can witness only the breach side. Assert the
                // fixture identity plus the breach.
                assert_eq!(
                    exact_1e9(&outputs["S_daily"], "S_daily"),
                    exact_1e9(&outputs["S"], "S"),
                    "{name} fixture S_daily/S identity"
                );
                let mut daily_state = WelfareState::new();
                daily_state
                    .register_metric_spec(Registration::Genesis, 1, faithful_specs(&parsed))
                    .expect("register daily spec");
                let mut daily_components = snapshot_components(&parsed);
                daily_components.retain(|component| {
                    ![ID_C01, ID_C02, ID_C03, ID_P01, ID_P02, ID_A01, ID_A02]
                        .contains(&component.id)
                });
                for (id, value) in parsed.c_daily {
                    daily_components.push(ComponentValue {
                        id,
                        value: FixedU64(value),
                    });
                }
                let (flags, _changed) = daily_state
                    .record_daily_gate(1, 0, 1, daily_components, &WelfareParams::DEFAULT)
                    .expect("daily gate records");
                assert!(
                    flags.s_breached,
                    "{name} S_daily below welfare.thetaS must latch a breach"
                );

                // settlement_with_self: 05 §4.4 (4) applied to (W, W).
                let self_score =
                    settlement_score(welfare, welfare).expect("self settlement computes");
                assert_eq!(
                    self_score,
                    FixedU64(exact_1e9(
                        &row["settlement_with_self"],
                        "settlement_with_self"
                    )),
                    "{name} settlement_with_self"
                );
            }
            other => panic!("unknown welfare scenario: {other}"),
        }
    }
}
