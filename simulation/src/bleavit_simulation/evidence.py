from __future__ import annotations

from decimal import Decimal, InvalidOperation
import json
from pathlib import Path

from .calibration import (
    _simulate_population,
    _subsample_ids,
    normative_violations,
    outcome_digest,
    outcome_merkle_root,
)
from .config import CLASSES, DEFAULT_SEED, SimulationConfig
from .proposals import generate_proposal_with_config


def canonical_bytes(payload: dict) -> bytes:
    return (json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode("utf-8")


def write_artifact(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(canonical_bytes(payload))


def load_artifact(path: Path) -> dict:
    raw = path.read_bytes()
    payload = json.loads(raw)
    if not isinstance(payload, dict):
        raise ValueError("artifact root must be a JSON object")
    if raw != canonical_bytes(payload):
        raise ValueError("artifact is not canonical byte-stable JSON")
    return payload


def _decimal(value, label: str, errors: list[str]) -> Decimal | None:
    try:
        result = Decimal(str(value))
    except (InvalidOperation, TypeError, ValueError):
        errors.append(f"{label} is not decimal")
        return None
    if not result.is_finite():
        errors.append(f"{label} is not finite")
        return None
    return result


def _count(value, label: str, errors: list[str]) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        errors.append(f"{label} is not a non-negative integer")
        return None
    return value


def _rate(numerator: int, denominator: int) -> str:
    if denominator == 0:
        return "0.000000"
    return format(Decimal(numerator) / Decimal(denominator), ".6f")


def _check_metric_row(row: dict, label: str, errors: list[str]) -> None:
    total = _count(row.get("total"), f"{label}.total", errors)
    harmful = _count(row.get("harmful"), f"{label}.harmful", errors)
    beneficial = _count(row.get("beneficial"), f"{label}.beneficial", errors)
    decidable = _count(row.get("decidable_harm"), f"{label}.decidable_harm", errors)
    false_pass = _count(row.get("false_pass_count"), f"{label}.false_pass", errors)
    decidable_false = _count(row.get("decidable_harm_false_pass_count"), f"{label}.decidable_false_pass", errors)
    false_reject = _count(row.get("false_reject_count"), f"{label}.false_reject", errors)
    if None in (total, harmful, beneficial, decidable, false_pass, decidable_false, false_reject):
        return
    assert harmful is not None and beneficial is not None and decidable is not None
    assert false_pass is not None and decidable_false is not None and false_reject is not None
    if decidable > harmful or false_pass > harmful or decidable_false > decidable or false_reject > beneficial:
        errors.append(f"{label} metric count bounds disagree")
    checks = (
        ("false_pass_rate", false_pass, harmful),
        ("decidable_harm_false_pass_rate", decidable_false, decidable),
        ("false_reject_rate", false_reject, beneficial),
    )
    for field, numerator, denominator in checks:
        if row.get(field) != _rate(numerator, denominator):
            errors.append(f"{label}.{field} disagrees with counts")


def _check_metrics(payload: dict, config: SimulationConfig, errors: list[str]) -> None:
    metrics = payload.get("metrics")
    if not isinstance(metrics, dict) or set(metrics) != set(CLASSES):
        errors.append("metric classes are incomplete")
        return
    expected_axes = {"effect_delta_band", "attack_strategy", "gate_exposure", "market_formation"}
    for name in CLASSES:
        row = metrics[name]
        _check_metric_row(row, name, errors)
        strata = row.get("strata", {})
        if set(strata) != expected_axes:
            errors.append(f"{name} strata axes are incomplete")
            continue
        for axis, groups in strata.items():
            if not isinstance(groups, dict):
                errors.append(f"{name}/{axis} strata are not an object")
                continue
            for key, group in groups.items():
                _check_metric_row(group, f"{name}/{axis}/{key}", errors)
            if sum(group.get("total", 0) for group in groups.values()) != row.get("total"):
                errors.append(f"{name}/{axis} strata do not partition the class")
        effect = strata.get("effect_delta_band", {})
        weights = {band: Decimal(weight) for band, _, _, weight in config.effect_strata}
        recorded = row.get("distribution_weighted_aggregate", {})
        for field in (
            "false_pass_rate",
            "decidable_harm_false_pass_rate",
            "false_reject_rate",
        ):
            eligible = {
                key: group
                for key, group in effect.items()
                if field != "decidable_harm_false_pass_rate"
                or group.get("decidable_harm", 0) > 0
            }
            weight_total = sum(
                (weights.get(key, Decimal(0)) for key in eligible), Decimal(0)
            )
            expected = format(
                sum(
                    weights.get(key, Decimal(0))
                    * Decimal(group[field])
                    / weight_total
                    for key, group in eligible.items()
                )
                if weight_total
                else Decimal(0),
                ".6f",
            )
            if recorded.get(field) != expected:
                errors.append(f"{name}.distribution_weighted.{field} disagrees")


def _check_digests(payload: dict, errors: list[str]) -> None:
    rows = payload.get("outcome_digests")
    count = payload.get("proposal_count")
    if not isinstance(rows, list) or len(rows) != count:
        errors.append("outcome digest list length disagrees")
        return
    expected_ids = list(range(count)) if isinstance(count, int) else []
    if [row.get("proposal_id") for row in rows] != expected_ids:
        errors.append("outcome digest IDs are not complete and sorted")
        return
    try:
        root = outcome_merkle_root(rows)
    except (ValueError, TypeError, KeyError) as error:
        errors.append(f"outcome digest list invalid: {error}")
        return
    if payload.get("outcome_digest_root") != root:
        errors.append("outcome digest root mismatch")


def _check_attack_evidence(payload: dict, errors: list[str]) -> dict:
    attack = payload.get("attack_cost_validation")
    if not isinstance(attack, dict):
        errors.append("attack validation is not an object")
        return {}
    brackets = attack.get("brackets")
    griefing = attack.get("griefing_cost_diagnostics")
    noncausal = attack.get("noncausal_wrong_pass_dispositions")
    if not all(isinstance(rows, list) for rows in (brackets, griefing, noncausal)):
        errors.append("attack validation detail lists are incomplete")
        return attack
    tolerance = _decimal(
        attack.get("relative_tolerance"), "attack.relative_tolerance", errors
    )
    derived_violations = []
    derived_inconclusive = []
    proposal_ids = []
    for index, row in enumerate(brackets):
        if not isinstance(row, dict):
            errors.append(f"attack bracket {index} is not an object")
            continue
        proposal_ids.append(row.get("proposal_id"))
        width = _decimal(
            row.get("relative_width"), f"attack.brackets[{index}].relative_width", errors
        )
        if tolerance is not None and width is not None and width > tolerance:
            errors.append(f"attack bracket {index} exceeds relative tolerance")
        if row.get("direction") != "wrong_pass":
            errors.append(f"attack bracket {index} is not a wrong-PASS flip")
        status = row.get("envelope_status")
        if status == "violation":
            derived_violations.append(row)
        elif status == "inconclusive":
            derived_inconclusive.append(row)
        elif status != "clean":
            errors.append(f"attack bracket {index} has unknown envelope status")
        if row.get("sub_3p_status") not in ("clean", "violation", "inconclusive"):
            errors.append(f"attack bracket {index} has unknown sub-3P status")
    for rows in (griefing, noncausal):
        for row in rows:
            if isinstance(row, dict):
                proposal_ids.append(row.get("proposal_id"))
    if len(proposal_ids) != len(set(proposal_ids)):
        errors.append("attack validation proposal dispositions overlap")
    wrong_pass_accounted = len(brackets) + sum(
        row.get("direction") == "wrong_pass"
        for row in griefing
        if isinstance(row, dict)
    ) + len(noncausal)
    if attack.get("wrong_pass_candidates") != wrong_pass_accounted:
        errors.append("wrong-PASS candidates are not fully dispositioned")
    if any(
        row.get("disposition")
        not in ("noncausal_zero_budget", "primary_wrong_pass_not_reproduced")
        for row in noncausal
        if isinstance(row, dict)
    ):
        errors.append("unknown noncausal wrong-PASS disposition")
    per_class = {}
    for name in CLASSES:
        rows = [row for row in brackets if row.get("class") == name]
        per_class[name] = {
            "bracket_count": len(rows),
            "envelope_clean": all(
                row.get("envelope_status") == "clean" for row in rows
            ),
            "status": "measured" if rows else "no_causal_wrong_pass_observed",
            "sub_3p_clean": all(
                row.get("sub_3p_status") == "clean" for row in rows
            ),
        }
    derived_sub_3p = all(row["sub_3p_clean"] for row in per_class.values())
    if attack.get("envelope_violations") != derived_violations:
        errors.append("envelope violation summary disagrees with brackets")
    if attack.get("envelope_inconclusive") != derived_inconclusive:
        errors.append("envelope inconclusive summary disagrees with brackets")
    if attack.get("per_class") != per_class:
        errors.append("per-class attack summary disagrees with brackets")
    if attack.get("sub_3p_every_class_clean") != derived_sub_3p:
        errors.append("sub-3P attack summary disagrees with brackets")
    return {
        **attack,
        "envelope_inconclusive": derived_inconclusive,
        "envelope_violations": derived_violations,
        "per_class": per_class,
        "sub_3p_every_class_clean": derived_sub_3p,
    }


def _check_subsample(payload: dict, config: SimulationConfig, errors: list[str]) -> int:
    subsample = payload.get("subsample", {})
    ids = subsample.get("proposal_ids", [])
    expected_ids = _subsample_ids(DEFAULT_SEED, config.proposal_count, config.subsample_size)
    if ids != expected_ids:
        errors.append("pinned subsample IDs drifted")
        return 0
    epoch_ids = sorted({proposal_id // config.epoch_slate_size for proposal_id in ids})
    proposal_ids = [
        proposal_id
        for epoch in epoch_ids
        for proposal_id in range(epoch * config.epoch_slate_size, min((epoch + 1) * config.epoch_slate_size, config.proposal_count))
    ]
    proposals = [generate_proposal_with_config(DEFAULT_SEED, proposal_id, config) for proposal_id in proposal_ids]
    reproduced = _simulate_population(
        proposals=proposals,
        seed=DEFAULT_SEED,
        config=config,
        budget_multiple=Decimal(config.primary_manipulator_budget_multiple),
        flow_cap=Decimal(config.diagnostic_probe_flow_cap),
    )
    by_id = {row.proposal.proposal_id: row.evidence() for row in reproduced}
    expected = [by_id[proposal_id] for proposal_id in ids]
    if subsample.get("results") != expected:
        errors.append("pinned subsample does not reproduce byte-exactly")
        return 0
    digest_rows = {row["proposal_id"]: row["digest"] for row in payload.get("outcome_digests", []) if isinstance(row, dict) and "proposal_id" in row and "digest" in row}
    for proposal_id, evidence in zip(ids, expected):
        if digest_rows.get(proposal_id) != outcome_digest(proposal_id, evidence):
            errors.append(f"subsample outcome digest mismatch for {proposal_id}")
    return len(ids)


def check_artifact(path: Path) -> dict:
    payload = load_artifact(path)
    errors: list[str] = []
    config = SimulationConfig()
    if payload.get("schema") != "bleavit.phase0-calibration.v3":
        errors.append("wrong evidence schema")
    if payload.get("seed") != DEFAULT_SEED:
        errors.append("default seed drift")
    # Generating-interpreter provenance is informational: validated for shape,
    # never compared against the running interpreter — cross-version
    # reproducibility is enforced empirically by the byte-exact pinned
    # subsample replay and the outcome-digest Merkle root below.
    recorded_python = payload.get("provenance", {}).get("python_version")
    if (
        not isinstance(recorded_python, list)
        or len(recorded_python) != 5
        or not all(isinstance(part, int) for part in recorded_python[:3])
    ):
        errors.append("missing or malformed python-version provenance")
    if payload.get("config") != config.canonical():
        errors.append("recorded config differs from executable defaults")
    if payload.get("config_digest") != config.digest():
        errors.append("config digest mismatch")
    count = _count(payload.get("proposal_count"), "proposal_count", errors) or 0
    if count != config.proposal_count or count < 10_000:
        errors.append("proposal count differs from the >=10^4 default")
    by_class = payload.get("proposal_counts", {}).get("by_class", {})
    if set(by_class) != set(CLASSES) or sum(by_class.values()) != count:
        errors.append("class counts are incomplete or do not sum")
    _check_metrics(payload, config, errors)
    _check_digests(payload, errors)
    derived_attack = _check_attack_evidence(payload, errors)
    if errors:
        raise ValueError("calibration check failed:\n" + "\n".join(f"- {error}" for error in errors))
    reproduced = _check_subsample(payload, config, errors)
    publication = payload.get("published", {})
    eligibility = publication.get("eligibility", {})
    metrics = payload.get("metrics", {})
    attack = derived_attack
    try:
        derived_violations = normative_violations(metrics, attack)
        class_gate = {
            name: Decimal(metrics[name]["decidable_harm_false_pass_rate"])
            < Decimal("0.01")
            for name in CLASSES
        }
        sub_3p_clean = attack["sub_3p_every_class_clean"] is True
    except (KeyError, TypeError, InvalidOperation):
        errors.append("economic gates cannot be derived from committed evidence")
        derived_violations = []
        class_gate = {}
        sub_3p_clean = False
    if eligibility.get("by_class") != class_gate:
        errors.append("publication class eligibility disagrees with measured rates")
    if eligibility.get("all_decidable_harm_false_pass_lt_1pct") != all(
        class_gate.values()
    ):
        errors.append("publication aggregate eligibility disagrees with measured rates")
    if eligibility.get("sub_3p_brackets_clean") != sub_3p_clean:
        errors.append("publication sub-3P eligibility disagrees with attack brackets")
    eligible = all(class_gate.values()) and sub_3p_clean
    expected_designation = "published" if eligible else "candidates-only"
    if publication.get("designation") != expected_designation:
        errors.append("publication designation disagrees with gates")
    thin = payload.get("thin_market_capture", {})
    if "Attack-generated gross attacker+arbitrage flow" not in thin.get("confirmed_spec_seam", ""):
        errors.append("confirmed thin-market-capture seam is missing")
    violations = payload.get("violations")
    if not isinstance(violations, list):
        errors.append("violations must be a list")
        violations = []
    elif violations != derived_violations:
        errors.append("committed violations disagree with measured evidence")
    if errors:
        raise ValueError("calibration check failed:\n" + "\n".join(f"- {error}" for error in errors))
    return {
        "checked_proposals": count,
        "reproduced_subsample": reproduced,
        "violations": derived_violations,
    }
