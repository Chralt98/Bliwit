//! Shared JSON ledger-sequence replay for the frame-free core (15 §4.4;
//! 03 §11).
//!
//! The fixture is generated only by `tools/reference-model/generate-vectors.py`.
//! Every failed operation is also checked for the 03 §5 atomic/status-quo
//! guarantee; every successful operation and final digest must agree exactly.
//! This file replays only operations implemented by `LedgerState`; 03 §5.4
//! archive-delay/batched sweeps are FRAME storage/custody behavior and are
//! replayed from the same fixture by the pallet's `differential_sweep` module.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use conditional_ledger_core::{
    baseline, position, BaselineState, BranchSupply, Error, Event, LedgerOrigin, LedgerState,
};
use futarchy_primitives::{
    kernel, Branch, FixedU64, GateType, PositionId, PositionKind, ScalarSide, VaultState,
};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Deserialize)]
struct Fixture {
    ledger_sequence_scenarios: Vec<Scenario>,
    ledger_score_scenarios: Vec<ScoreScenario>,
    ledger_error_scenarios: Vec<ErrorScenario>,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    initial_state: InitialState,
    ops: Vec<Step>,
    final_state: Value,
}

#[derive(Deserialize)]
struct InitialState {
    proposal_id: u64,
    baseline_epoch: u32,
    digest: Value,
}

#[derive(Deserialize)]
struct Step {
    op: String,
    args: Value,
    outcome: Value,
}

#[derive(Deserialize)]
struct ScoreScenario {
    name: String,
    score: u64,
    amount: u64,
    long_payout: u64,
    short_payout: u64,
    pair_payout: u64,
}

#[derive(Deserialize)]
struct ErrorScenario {
    name: String,
    op: ErrorOperation,
    outcome: Value,
}

#[derive(Deserialize)]
struct ErrorOperation {
    name: String,
}

fn vector_path() -> PathBuf {
    std::env::var_os("BLEAVIT_LEDGER_VECTOR_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../reference-model/fixtures/vectors.json")
        })
}

fn fixture() -> Fixture {
    serde_json::from_str(
        &fs::read_to_string(vector_path()).expect("read shared reference-model vectors"),
    )
    .expect("parse shared reference-model vectors")
}

fn holder(name: &str) -> u8 {
    match name {
        "alice" => 1,
        "bob" => 2,
        "carol" => 3,
        other => panic!("unknown vector holder class: {other}"),
    }
}

fn holder_label(who: u8) -> &'static str {
    match who {
        1 => "alice",
        2 => "bob",
        3 => "carol",
        // The index is deliberately not interpolated: CodeQL's cleartext-logging
        // name heuristic taints anything reaching here from the `owner` field
        // (public-fixture u8 test index, alert #12 on PR #89). Valid indices are
        // 1..=3; a violating scenario is pinpointed by the replay's own
        // scenario/op context in the surrounding assertions.
        _ => panic!("replay holder index outside the fixture's 1..=3 range"),
    }
}

fn branch(value: &str) -> Branch {
    match value {
        "Accept" => Branch::Accept,
        "Reject" => Branch::Reject,
        other => panic!("unknown branch: {other}"),
    }
}

fn gate(value: &str) -> GateType {
    match value {
        "Survival" => GateType::Survival,
        "Security" => GateType::Security,
        other => panic!("unknown gate: {other}"),
    }
}

fn scalar_side(value: &str) -> ScalarSide {
    match value {
        "Long" => ScalarSide::Long,
        "Short" => ScalarSide::Short,
        other => panic!("unknown scalar side: {other}"),
    }
}

fn position_kind(value: &str, gate_name: Option<&str>) -> PositionKind {
    match value {
        "BranchUsdc" => PositionKind::BranchUsdc,
        "Long" => PositionKind::Long,
        "Short" => PositionKind::Short,
        "GateYes" => PositionKind::GateYes(gate(gate_name.expect("GateYes needs gate"))),
        "GateNo" => PositionKind::GateNo(gate(gate_name.expect("GateNo needs gate"))),
        other => panic!("unknown position kind: {other}"),
    }
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string arg {key}"))
}

fn amount(value: &Value) -> u128 {
    u128::from(
        value["amount"]
            .as_u64()
            .expect("amount must fit the generated u64 corpus"),
    )
}

fn position_arg(value: &Value, pid: u64, epoch: u32) -> PositionId {
    let coordinates = &value["position"];
    match string(coordinates, "family") {
        "proposal" => position(
            pid,
            branch(string(coordinates, "branch")),
            position_kind(
                string(coordinates, "kind"),
                coordinates.get("gate").and_then(Value::as_str),
            ),
        ),
        "baseline" => baseline(epoch, scalar_side(string(coordinates, "side"))),
        other => panic!("unknown position family: {other}"),
    }
}

fn object(entries: &[(&str, Value)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    )
}

fn number(value: u128) -> Value {
    Value::from(u64::try_from(value).expect("generated balances fit u64"))
}

fn empty() -> Value {
    Value::Object(Map::new())
}

fn last_event(state: &LedgerState<u8>) -> &Event {
    state
        .events
        .last()
        .expect("successful operation emits event")
}

fn apply_step(
    state: &mut LedgerState<u8>,
    pid: u64,
    epoch: u32,
    step: &Step,
) -> Result<Value, Error> {
    let args = &step.args;
    let who = || holder(string(args, "account"));
    let a = || amount(args);
    match step.op.as_str() {
        "split" => {
            let a = a();
            state.split(LedgerOrigin::Signed, pid, &who(), a)?;
            Ok(object(&[("minted", number(a))]))
        }
        "merge" => {
            let a = a();
            state.merge(LedgerOrigin::Signed, pid, &who(), a)?;
            Ok(object(&[("burned", number(a)), ("payout", number(a))]))
        }
        "split_scalar" => {
            let a = a();
            state.split_scalar(
                LedgerOrigin::Signed,
                pid,
                branch(string(args, "branch")),
                &who(),
                a,
            )?;
            Ok(object(&[("burned", number(a)), ("minted", number(a))]))
        }
        "merge_scalar" => {
            let a = a();
            state.merge_scalar(
                LedgerOrigin::Signed,
                pid,
                branch(string(args, "branch")),
                &who(),
                a,
            )?;
            Ok(object(&[("burned", number(a)), ("minted", number(a))]))
        }
        "split_gate" => {
            let a = a();
            state.split_gate(
                LedgerOrigin::Signed,
                pid,
                branch(string(args, "branch")),
                gate(string(args, "gate")),
                &who(),
                a,
            )?;
            Ok(object(&[("burned", number(a)), ("minted", number(a))]))
        }
        "merge_gate" => {
            let a = a();
            state.merge_gate(
                LedgerOrigin::Signed,
                pid,
                branch(string(args, "branch")),
                gate(string(args, "gate")),
                &who(),
                a,
            )?;
            Ok(object(&[("burned", number(a)), ("minted", number(a))]))
        }
        "transfer" => {
            state.transfer(
                LedgerOrigin::Signed,
                position_arg(args, pid, epoch),
                &holder(string(args, "from")),
                &holder(string(args, "to")),
                a(),
            )?;
            let Event::PositionTransferred(_, moved) = last_event(state) else {
                panic!("transfer emitted wrong event")
            };
            Ok(object(&[("moved", number(*moved))]))
        }
        "resolve" => {
            state.resolve(
                LedgerOrigin::ResolveAuthority,
                pid,
                branch(string(args, "winner")),
            )?;
            Ok(empty())
        }
        "void" => {
            state.void(LedgerOrigin::ResolveAuthority, pid)?;
            Ok(empty())
        }
        "settle_scalar" => {
            let score = args["s"].as_u64().expect("score is u64");
            state.settle_scalar(LedgerOrigin::SettleAuthority, pid, FixedU64(score))?;
            Ok(object(&[("s", Value::from(score))]))
        }
        "settle_gate" => {
            let outcome = args["outcome"].as_bool().expect("gate outcome is bool");
            state.settle_gate(
                LedgerOrigin::SettleAuthority,
                pid,
                gate(string(args, "gate")),
                outcome,
            )?;
            Ok(object(&[("outcome", Value::from(outcome))]))
        }
        "redeem" => {
            let a = a();
            state.redeem(pid, &who(), a)?;
            let Event::Redeemed(_, payout) = last_event(state) else {
                panic!("redeem emitted wrong event")
            };
            Ok(object(&[
                ("burned", number(a)),
                ("payout", number(*payout)),
            ]))
        }
        "redeem_scalar" => {
            let a = a();
            state.redeem_scalar(pid, scalar_side(string(args, "side")), &who(), a)?;
            let Event::ScalarRedeemed(_, _, payout) = last_event(state) else {
                panic!("scalar redemption emitted wrong event")
            };
            Ok(object(&[
                ("burned", number(a)),
                ("payout", number(*payout)),
            ]))
        }
        "redeem_scalar_pair" => {
            let a = a();
            state.redeem_scalar_pair(pid, &who(), a)?;
            let Event::ScalarPairRedeemed(_, payout) = last_event(state) else {
                panic!("scalar pair redemption emitted wrong event")
            };
            Ok(object(&[
                ("burned", number(a)),
                ("payout", number(*payout)),
            ]))
        }
        "redeem_gate" => {
            let a = a();
            state.redeem_gate(pid, gate(string(args, "gate")), &who(), a)?;
            let Event::GateRedeemed(_, _, payout) = last_event(state) else {
                panic!("gate redemption emitted wrong event")
            };
            Ok(object(&[
                ("burned", number(a)),
                ("payout", number(*payout)),
            ]))
        }
        "redeem_void" => {
            let a = a();
            state.redeem_void(
                pid,
                branch(string(args, "branch")),
                position_kind(
                    string(args, "kind"),
                    args.get("gate").and_then(Value::as_str),
                ),
                &who(),
                a,
            )?;
            let Event::VoidRedeemed(_, _, burned, payout) = last_event(state) else {
                panic!("void redemption emitted wrong event")
            };
            Ok(object(&[
                ("burned", number(*burned)),
                ("payout", number(*payout)),
            ]))
        }
        "split_baseline" => {
            let a = a();
            state.split_baseline(LedgerOrigin::Signed, epoch, &who(), a)?;
            Ok(object(&[("minted", number(a))]))
        }
        "merge_baseline" => {
            let a = a();
            state.merge_baseline(LedgerOrigin::Signed, epoch, &who(), a)?;
            Ok(object(&[("burned", number(a)), ("payout", number(a))]))
        }
        "settle_baseline" => {
            let score = args["s"].as_u64().expect("score is u64");
            state.settle_baseline(LedgerOrigin::SettleAuthority, epoch, FixedU64(score))?;
            Ok(object(&[("s", Value::from(score))]))
        }
        "redeem_baseline" => {
            let a = a();
            state.redeem_baseline(epoch, scalar_side(string(args, "side")), &who(), a)?;
            let Event::BaselineRedeemed(_, _, payout) = last_event(state) else {
                panic!("baseline redemption emitted wrong event")
            };
            Ok(object(&[
                ("burned", number(a)),
                ("payout", number(*payout)),
            ]))
        }
        "redeem_baseline_pair" => {
            let a = a();
            state.redeem_baseline_pair(epoch, &who(), a)?;
            let Event::BaselineRedeemed(_, _, payout) = last_event(state) else {
                panic!("baseline pair redemption emitted wrong event")
            };
            Ok(object(&[
                ("burned", number(a)),
                ("payout", number(*payout)),
            ]))
        }
        other => panic!("unhandled vector operation: {other}"),
    }
}

const ERROR_CLASSES: [&str; 10] = [
    "UnknownVault",
    "UnknownBaselineVault",
    "WrongVaultState",
    "AmountTooSmall",
    "ArithmeticOverflow",
    "InsufficientPosition",
    "PositionCapExceeded",
    "InvalidScore",
    "GateAlreadySettled",
    "GateNotSettled",
];

fn expected_error(class: &str) -> Error {
    // Completeness is intentional: a new Python class cannot silently fall
    // through to a generic Rust error (15 §4.4 status-quo differential).
    match class {
        "UnknownVault" => Error::UnknownVault,
        "UnknownBaselineVault" => Error::UnknownBaselineVault,
        "WrongVaultState" => Error::WrongVaultState,
        "AmountTooSmall" => Error::AmountTooSmall,
        "ArithmeticOverflow" => Error::ArithmeticOverflow,
        "InsufficientPosition" => Error::InsufficientPosition,
        "PositionCapExceeded" => Error::PositionCapExceeded,
        "InvalidScore" => Error::InvalidScore,
        "GateAlreadySettled" => Error::GateAlreadySettled,
        "GateNotSettled" => Error::GateNotSettled,
        // `Error::WrongBranch` is deliberately absent: the core API derives the
        // winner and no core path can produce it (dead variant — SQ-170), so a
        // differential witness would have to be fabricated (S1 re-pass finding).
        other => panic!("unknown Python ledger error class: {other}"),
    }
}

fn proposal_state(state: VaultState) -> Value {
    match state {
        VaultState::Open => object(&[("kind", Value::from("Open"))]),
        VaultState::Resolved(winner) => object(&[
            ("kind", Value::from("Resolved")),
            ("winner", Value::from(branch_name(winner))),
        ]),
        VaultState::ScalarSettled { winner, s } => object(&[
            ("kind", Value::from("ScalarSettled")),
            ("winner", Value::from(branch_name(winner))),
            ("s", Value::from(s.0)),
        ]),
        VaultState::Voided => object(&[("kind", Value::from("Voided"))]),
    }
}

fn baseline_state(state: BaselineState) -> Value {
    match state {
        BaselineState::Open => object(&[("kind", Value::from("Open"))]),
        BaselineState::Settled(s) => {
            object(&[("kind", Value::from("Settled")), ("s", Value::from(s.0))])
        }
    }
}

fn branch_name(value: Branch) -> &'static str {
    match value {
        Branch::Accept => "Accept",
        Branch::Reject => "Reject",
    }
}

fn branch_digest(supply: BranchSupply) -> Value {
    object(&[
        ("usdc", number(supply.usdc)),
        ("scalar_sets", number(supply.scalar_sets)),
        (
            "gate_sets",
            object(&[
                ("Survival", number(supply.gate_sets[0])),
                ("Security", number(supply.gate_sets[1])),
            ]),
        ),
    ])
}

fn position_name(id: PositionId) -> String {
    match id {
        PositionId::Proposal { branch, kind, .. } => {
            let prefix = format!("proposal/{}", branch_name(branch));
            match kind {
                PositionKind::BranchUsdc => format!("{prefix}/BranchUsdc"),
                PositionKind::Long => format!("{prefix}/Long"),
                PositionKind::Short => format!("{prefix}/Short"),
                PositionKind::GateYes(g) => {
                    format!("{prefix}/GateYes/{}", gate_name(g))
                }
                PositionKind::GateNo(g) => format!("{prefix}/GateNo/{}", gate_name(g)),
            }
        }
        PositionId::Baseline { side, .. } => {
            format!("baseline/{}", scalar_side_name(side))
        }
    }
}

fn gate_name(value: GateType) -> &'static str {
    match value {
        GateType::Survival => "Survival",
        GateType::Security => "Security",
    }
}

fn scalar_side_name(value: ScalarSide) -> &'static str {
    match value {
        ScalarSide::Long => "Long",
        ScalarSide::Short => "Short",
    }
}

fn event_position_kind_name(kind: PositionKind) -> String {
    match kind {
        PositionKind::BranchUsdc => "BranchUsdc".to_owned(),
        PositionKind::Long => "Long".to_owned(),
        PositionKind::Short => "Short".to_owned(),
        PositionKind::GateYes(gate) => format!("GateYes/{}", gate_name(gate)),
        PositionKind::GateNo(gate) => format!("GateNo/{}", gate_name(gate)),
    }
}

fn event_value(event: &Event) -> Value {
    let (kind, fields) = match *event {
        Event::Split(pid, amount) => ("Split", vec![Value::from(pid), number(amount)]),
        Event::Merged(pid, amount) => ("Merged", vec![Value::from(pid), number(amount)]),
        Event::ScalarSplit(pid, branch, amount) => (
            "ScalarSplit",
            vec![
                Value::from(pid),
                Value::from(branch_name(branch)),
                number(amount),
            ],
        ),
        Event::ScalarMerged(pid, branch, amount) => (
            "ScalarMerged",
            vec![
                Value::from(pid),
                Value::from(branch_name(branch)),
                number(amount),
            ],
        ),
        Event::GateSplit(pid, branch, gate, amount) => (
            "GateSplit",
            vec![
                Value::from(pid),
                Value::from(branch_name(branch)),
                Value::from(gate_name(gate)),
                number(amount),
            ],
        ),
        Event::GateMerged(pid, branch, gate, amount) => (
            "GateMerged",
            vec![
                Value::from(pid),
                Value::from(branch_name(branch)),
                Value::from(gate_name(gate)),
                number(amount),
            ],
        ),
        Event::PositionTransferred(id, amount) => (
            "PositionTransferred",
            vec![Value::from(position_name(id)), number(amount)],
        ),
        Event::BaselineSplit(epoch, amount) => {
            ("BaselineSplit", vec![Value::from(epoch), number(amount)])
        }
        Event::BaselineMerged(epoch, amount) => {
            ("BaselineMerged", vec![Value::from(epoch), number(amount)])
        }
        Event::VaultResolved(pid, winner) => (
            "VaultResolved",
            vec![Value::from(pid), Value::from(branch_name(winner))],
        ),
        Event::VaultVoided(pid) => ("VaultVoided", vec![Value::from(pid)]),
        Event::ScalarSettlementSet(pid, winner, score) => (
            "ScalarSettlementSet",
            vec![
                Value::from(pid),
                Value::from(branch_name(winner)),
                Value::from(score.0),
            ],
        ),
        Event::GateSettled(pid, winner, gate, outcome) => (
            "GateSettled",
            vec![
                Value::from(pid),
                Value::from(branch_name(winner)),
                Value::from(gate_name(gate)),
                Value::from(outcome),
            ],
        ),
        Event::BaselineSettled(epoch, score) => (
            "BaselineSettled",
            vec![Value::from(epoch), Value::from(score.0)],
        ),
        Event::Redeemed(pid, payout) => ("Redeemed", vec![Value::from(pid), number(payout)]),
        Event::ScalarRedeemed(pid, side, payout) => (
            "ScalarRedeemed",
            vec![
                Value::from(pid),
                Value::from(scalar_side_name(side)),
                number(payout),
            ],
        ),
        Event::ScalarPairRedeemed(pid, payout) => {
            ("ScalarPairRedeemed", vec![Value::from(pid), number(payout)])
        }
        Event::GateRedeemed(pid, gate, payout) => (
            "GateRedeemed",
            vec![
                Value::from(pid),
                Value::from(gate_name(gate)),
                number(payout),
            ],
        ),
        Event::VoidRedeemed(pid, position_kind, amount, payout) => (
            "VoidRedeemed",
            vec![
                Value::from(pid),
                Value::from(event_position_kind_name(position_kind)),
                number(amount),
                number(payout),
            ],
        ),
        Event::BaselineRedeemed(epoch, side, payout) => (
            "BaselineRedeemed",
            vec![
                Value::from(epoch),
                Value::from(scalar_side_name(side)),
                number(payout),
            ],
        ),
        Event::VaultReaped(pid, residue) => {
            ("VaultReaped", vec![Value::from(pid), number(residue)])
        }
        Event::BaselineVaultReaped(epoch, residue) => (
            "BaselineVaultReaped",
            vec![Value::from(epoch), number(residue)],
        ),
    };
    object(&[
        ("kind", Value::from(kind)),
        ("fields", Value::Array(fields)),
    ])
}

fn state_digest(state: &LedgerState<u8>, pid: u64, epoch: u32) -> Value {
    let proposal = &state
        .vaults
        .iter()
        .find(|vault| vault.proposal == pid)
        .expect("proposal vault exists")
        .info;
    let baseline = &state
        .baseline_vaults
        .iter()
        .find(|vault| vault.epoch == epoch)
        .expect("baseline vault exists")
        .info;
    let mut balances: BTreeMap<String, BTreeMap<String, u128>> = ["alice", "bob", "carol"]
        .into_iter()
        .map(|name| (name.to_owned(), BTreeMap::new()))
        .collect();
    for record in &state.positions {
        balances
            .get_mut(holder_label(record.owner))
            .expect("known holder")
            .insert(position_name(record.id), record.balance);
    }
    let mut positions = state
        .positions
        .iter()
        .map(|record| {
            (
                (holder_label(record.owner), position_name(record.id)),
                object(&[
                    ("position", Value::from(position_name(record.id))),
                    ("owner", Value::from(holder_label(record.owner))),
                    ("balance", number(record.balance)),
                    ("deposit", number(record.deposit)),
                ]),
            )
        })
        .collect::<Vec<_>>();
    positions.sort_by(|left, right| left.0.cmp(&right.0));
    let mut totals = state
        .position_totals
        .iter()
        .map(|record| {
            (
                position_name(record.id),
                object(&[
                    ("position", Value::from(position_name(record.id))),
                    ("total", number(record.total)),
                ]),
            )
        })
        .collect::<Vec<_>>();
    totals.sort_by(|left, right| left.0.cmp(&right.0));
    object(&[
        (
            "proposal",
            object(&[
                ("proposal_id", Value::from(pid)),
                ("escrowed", number(proposal.escrowed)),
                ("spec", Value::from(proposal.spec)),
                ("state", proposal_state(proposal.state)),
                (
                    "gate_outcomes",
                    object(&[
                        (
                            "Survival",
                            proposal.gate_outcomes[0].map_or(Value::Null, Value::from),
                        ),
                        (
                            "Security",
                            proposal.gate_outcomes[1].map_or(Value::Null, Value::from),
                        ),
                    ]),
                ),
                (
                    "branches",
                    object(&[
                        ("Accept", branch_digest(proposal.branches[0])),
                        ("Reject", branch_digest(proposal.branches[1])),
                    ]),
                ),
            ]),
        ),
        (
            "baseline",
            object(&[
                ("epoch", Value::from(epoch)),
                ("escrowed", number(baseline.escrowed)),
                ("sets", number(baseline.sets)),
                ("state", baseline_state(baseline.state)),
            ]),
        ),
        (
            "balances",
            serde_json::to_value(balances).expect("balances serialize"),
        ),
        (
            "positions",
            Value::Array(positions.into_iter().map(|(_, value)| value).collect()),
        ),
        (
            "position_counts",
            Value::Array(
                state
                    .position_counts
                    .iter()
                    .map(|record| {
                        object(&[
                            ("owner", Value::from(holder_label(record.owner))),
                            ("count", Value::from(record.count)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "position_totals",
            Value::Array(totals.into_iter().map(|(_, value)| value).collect()),
        ),
        ("deposits_held", number(state.deposits_held)),
        (
            "events",
            Value::Array(state.events.iter().map(event_value).collect()),
        ),
        (
            "protocol_accounts",
            Value::Array(
                state
                    .protocol_accounts
                    .iter()
                    .map(|who| Value::from(holder_label(*who)))
                    .collect(),
            ),
        ),
    ])
}

#[test]
fn ledger_sequence_vectors_match_python_reference_model() {
    let fixture = fixture();
    assert_eq!(fixture.ledger_sequence_scenarios.len(), 64);

    for scenario in fixture.ledger_sequence_scenarios {
        let pid = scenario.initial_state.proposal_id;
        let epoch = scenario.initial_state.baseline_epoch;
        let mut state = LedgerState::<u8>::new();
        state.create_vault(pid, 0).expect("create proposal vault");
        state
            .create_baseline_vault(epoch)
            .expect("create baseline vault");
        assert_eq!(
            state_digest(&state, pid, epoch),
            scenario.initial_state.digest,
            "{} initial-state mismatch",
            scenario.name
        );

        for (index, step) in scenario.ops.iter().enumerate() {
            let before = state.clone();
            let actual = apply_step(&mut state, pid, epoch, step);
            if let Some(expected) = step.outcome.get("ok") {
                let actual = actual.unwrap_or_else(|error| {
                    panic!(
                        "{} op {index} ({}) expected ok {expected}, got err {error:?}",
                        scenario.name, step.op
                    )
                });
                assert_eq!(
                    actual, *expected,
                    "{} op {index} ({}) result mismatch: Rust={actual}, Python={expected}",
                    scenario.name, step.op
                );
                state.try_state().unwrap_or_else(|error| {
                    panic!(
                        "{} op {index} ({}) violated Rust try-state: {error:?}",
                        scenario.name, step.op
                    )
                });
            } else if let Some(class) = step.outcome.get("err").and_then(Value::as_str) {
                let expected = expected_error(class);
                assert_eq!(
                    actual,
                    Err(expected),
                    "{} op {index} ({}) error mismatch: Rust={actual:?}, Python={class}",
                    scenario.name,
                    step.op
                );
                assert_eq!(
                    state, before,
                    "{} op {index} ({}) mutated state on expected {class}",
                    scenario.name, step.op
                );
            } else {
                panic!(
                    "{} op {index} ({}) has malformed outcome {}",
                    scenario.name, step.op, step.outcome
                );
            }
        }

        let actual = state_digest(&state, pid, epoch);
        assert_eq!(
            actual, scenario.final_state,
            "{} final-state mismatch: Rust={actual}, Python={}",
            scenario.name, scenario.final_state
        );
    }
}

fn settled_scalar(score: u64, amount: u128) -> LedgerState<u8> {
    let mut state = LedgerState::new();
    state.create_vault(1, 0).expect("create score vault");
    state
        .split(LedgerOrigin::Signed, 1, &1, amount)
        .expect("split score collateral");
    state
        .split_scalar(LedgerOrigin::Signed, 1, Branch::Accept, &1, amount)
        .expect("split score scalar set");
    state
        .resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
        .expect("resolve score vault");
    state
        .settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(score))
        .expect("settle score vault");
    state
}

#[test]
fn ledger_score_vectors_cover_endpoints_and_rounding_boundaries() {
    let rows = fixture().ledger_score_scenarios;
    let scores = rows.iter().map(|row| row.score).collect::<BTreeSet<_>>();
    for required in [0, 1_000_000_000, 700_049_999, 700_050_000, 700_050_001] {
        assert!(
            scores.contains(&required),
            "missing required score {required}"
        );
    }

    for row in rows {
        let amount = u128::from(row.amount);

        let mut long = settled_scalar(row.score, amount);
        long.redeem_scalar(1, ScalarSide::Long, &1, amount)
            .unwrap_or_else(|error| panic!("{} LONG failed: {error:?}", row.name));
        let Event::ScalarRedeemed(_, _, long_payout) = long.events.last().unwrap() else {
            panic!("{} LONG emitted wrong event", row.name)
        };
        assert_eq!(
            *long_payout,
            u128::from(row.long_payout),
            "{} LONG",
            row.name
        );
        long.try_state().expect("LONG score state remains solvent");

        let mut short = settled_scalar(row.score, amount);
        short
            .redeem_scalar(1, ScalarSide::Short, &1, amount)
            .unwrap_or_else(|error| panic!("{} SHORT failed: {error:?}", row.name));
        let Event::ScalarRedeemed(_, _, short_payout) = short.events.last().unwrap() else {
            panic!("{} SHORT emitted wrong event", row.name)
        };
        assert_eq!(
            *short_payout,
            u128::from(row.short_payout),
            "{} SHORT",
            row.name
        );
        short
            .try_state()
            .expect("SHORT score state remains solvent");

        let mut pair = settled_scalar(row.score, amount);
        pair.redeem_scalar_pair(1, &1, amount)
            .unwrap_or_else(|error| panic!("{} pair failed: {error:?}", row.name));
        let Event::ScalarPairRedeemed(_, pair_payout) = pair.events.last().unwrap() else {
            panic!("{} pair emitted wrong event", row.name)
        };
        assert_eq!(
            *pair_payout,
            u128::from(row.pair_payout),
            "{} pair",
            row.name
        );
        pair.try_state().expect("pair score state remains solvent");
    }
}

fn error_witness(operation: &str) -> Error {
    let mut state = LedgerState::<u8>::new();
    match operation {
        "split" => state
            .split(LedgerOrigin::Signed, 404, &1, kernel::MIN_SPLIT_USDC)
            .unwrap_err(),
        "split_baseline" => state
            .split_baseline(LedgerOrigin::Signed, 404, &1, kernel::MIN_SPLIT_USDC)
            .unwrap_err(),
        "split_after_resolve" => {
            state.create_vault(1, 0).unwrap();
            state
                .resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
                .unwrap();
            let before = state.clone();
            let error = state
                .split(LedgerOrigin::Signed, 1, &1, kernel::MIN_SPLIT_USDC)
                .unwrap_err();
            assert_eq!(state, before);
            error
        }
        "split_below_minimum" => {
            state.create_vault(1, 0).unwrap();
            state
                .split(LedgerOrigin::Signed, 1, &1, kernel::MIN_SPLIT_USDC - 1)
                .unwrap_err()
        }
        "split_overflow" => {
            state.create_vault(1, 0).unwrap();
            state
                .split(
                    LedgerOrigin::Signed,
                    1,
                    &1,
                    u128::MAX - (kernel::MIN_SPLIT_USDC - 1),
                )
                .unwrap();
            let before = state.clone();
            let error = state
                .split(LedgerOrigin::Signed, 1, &1, kernel::MIN_SPLIT_USDC)
                .unwrap_err();
            assert_eq!(state, before);
            error
        }
        "merge_without_positions" => {
            state.create_vault(1, 0).unwrap();
            state
                .merge(LedgerOrigin::Signed, 1, &1, kernel::MIN_SPLIT_USDC)
                .unwrap_err()
        }
        "split_at_position_cap" => {
            for index in 0..32u64 {
                let pid = 1_000 + index;
                state.create_vault(pid, 0).unwrap();
                state
                    .split(LedgerOrigin::Signed, pid, &1, kernel::MIN_SPLIT_USDC)
                    .unwrap();
            }
            state.create_vault(1, 0).unwrap();
            let before = state.clone();
            let error = state
                .split(LedgerOrigin::Signed, 1, &1, kernel::MIN_SPLIT_USDC)
                .unwrap_err();
            assert_eq!(state, before);
            error
        }
        "settle_invalid_score" => {
            state.create_vault(1, 0).unwrap();
            state
                .resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
                .unwrap();
            state
                .settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(1_000_000_001))
                .unwrap_err()
        }
        "settle_gate_twice" => {
            state.create_vault(1, 0).unwrap();
            state
                .resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
                .unwrap();
            state
                .settle_gate(LedgerOrigin::SettleAuthority, 1, GateType::Survival, true)
                .unwrap();
            state
                .settle_gate(LedgerOrigin::SettleAuthority, 1, GateType::Survival, false)
                .unwrap_err()
        }
        "redeem_unsettled_gate" => {
            state = settled_scalar(500_000_000, kernel::MIN_SPLIT_USDC);
            state
                .redeem_gate(1, GateType::Survival, &1, kernel::MIN_SPLIT_USDC)
                .unwrap_err()
        }
        other => panic!("unknown generated error witness operation: {other}"),
    }
}

#[test]
fn ledger_error_vectors_equal_the_mapping_table() {
    let scenarios = fixture().ledger_error_scenarios;
    let exercised = scenarios
        .iter()
        .map(|scenario| {
            scenario.outcome["err"]
                .as_str()
                .unwrap_or_else(|| panic!("{} has no error outcome", scenario.name))
        })
        .collect::<BTreeSet<_>>();
    let mapped = ERROR_CLASSES.into_iter().collect::<BTreeSet<_>>();
    assert_eq!(exercised, mapped, "fixture/mapping error-class drift");

    for scenario in scenarios {
        let class = scenario.outcome["err"].as_str().unwrap();
        let actual = error_witness(&scenario.op.name);
        assert_eq!(
            actual,
            expected_error(class),
            "{} ({})",
            scenario.name,
            scenario.op.name
        );
    }
}
