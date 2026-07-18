from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import replace
from decimal import Decimal, ROUND_CEILING
import hashlib
import json
import random

from bleavit_reference_model.treasury import (
    BASELINE_B,
    B_FLOORS,
    DELTA_FLOORS,
    LN2,
    V_MIN_FLOORS,
    attack_cost_hat,
    dec_v_min,
    decision_delta,
    p_ref,
    pol_b,
    security_sizing_ok,
)

from .config import CLASSES, DEFAULT_SEED, SimulationConfig, python_version_tuple
from .engine import SimulationResult, _strategy_for, simulate_proposal
from .proposals import Proposal, generate_proposal_with_config


def _rate(numerator: int, denominator: int) -> str:
    if denominator == 0:
        return "0.000000"
    return format(Decimal(numerator) / Decimal(denominator), ".6f")


def _gate_hazard(row: SimulationResult) -> bool:
    p = row.proposal
    return p.gate_exposure == "gate" and (
        p.survival_risk_adopt > Decimal("0.05")
        or p.survival_risk_adopt > p.survival_risk_reject + Decimal("0.02")
        or p.security_risk_adopt > Decimal("0.05")
        or p.security_risk_adopt > p.security_risk_reject + Decimal("0.02")
    )


def _subset_metric(rows: list[SimulationResult]) -> dict:
    harmful = [row for row in rows if row.proposal.harmful]
    decidable = [row for row in harmful if row.decidable_harm]
    beneficial = [row for row in rows if not row.proposal.harmful and not _gate_hazard(row)]
    false_pass = sum(row.outcome == "Adopt" for row in harmful)
    decidable_false_pass = sum(row.outcome == "Adopt" for row in decidable)
    false_reject = sum(row.outcome != "Adopt" for row in beneficial)
    return {
        "beneficial": len(beneficial),
        "decidable_harm": len(decidable),
        "decidable_harm_false_pass_count": decidable_false_pass,
        "decidable_harm_false_pass_rate": _rate(decidable_false_pass, len(decidable)),
        "false_pass_count": false_pass,
        "false_pass_rate": _rate(false_pass, len(harmful)),
        "false_reject_count": false_reject,
        "false_reject_rate": _rate(false_reject, len(beneficial)),
        "gate_hazard_beneficial_excluded": sum(
            not row.proposal.harmful and _gate_hazard(row) for row in rows
        ),
        "harmful": len(harmful),
        "total": len(rows),
    }


def _aggregate(results: list[SimulationResult], config: SimulationConfig | None = None) -> dict:
    config = SimulationConfig(proposal_count=max(1, len(results))) if config is None else config
    output = {}
    axes = {
        "effect_delta_band": lambda row: row.proposal.effect_stratum,
        "attack_strategy": lambda row: row.strategy,
        "gate_exposure": lambda row: row.proposal.gate_exposure,
        "market_formation": lambda row: row.proposal.formation_regime,
    }
    for name in CLASSES:
        rows = [row for row in results if row.proposal.proposal_class == name]
        metric = _subset_metric(rows)
        outcomes = Counter(row.outcome for row in rows)
        reasons = Counter(row.reason for row in rows if row.reason is not None)
        metric.update(
            {
                "adopted": outcomes["Adopt"],
                "decision_grade_formation_rate": _rate(
                    sum(row.welfare_grade == "Ok" for row in rows), len(rows)
                ),
                "extension_rate": _rate(sum(row.extended for row in rows), len(rows)),
                "outcomes": dict(sorted(outcomes.items())),
                "reasons": dict(sorted(reasons.items())),
                "strata": {},
            }
        )
        for axis, getter in axes.items():
            grouped: dict[str, list[SimulationResult]] = defaultdict(list)
            for row in rows:
                grouped[getter(row)].append(row)
            metric["strata"][axis] = {
                key: _subset_metric(group) for key, group in sorted(grouped.items())
            }
        effect_rows = metric["strata"]["effect_delta_band"]
        weights = {row[0]: Decimal(row[3]) for row in config.effect_strata}
        weighted = {}
        for field in (
            "false_pass_rate",
            "decidable_harm_false_pass_rate",
            "false_reject_rate",
        ):
            eligible = {
                key: group
                for key, group in effect_rows.items()
                if field != "decidable_harm_false_pass_rate"
                or group["decidable_harm"] > 0
            }
            weight_total = sum(
                (weights.get(key, Decimal(0)) for key in eligible), Decimal(0)
            )
            weighted[field] = format(
                sum(
                    (
                        weights.get(key, Decimal(0))
                        * Decimal(group[field])
                        / weight_total
                    )
                    for key, group in eligible.items()
                )
                if weight_total
                else Decimal(0),
                ".6f",
            )
        metric["distribution_weighted_aggregate"] = weighted
        output[name] = metric
    return output


def _prize(proposal: Proposal) -> Decimal:
    from bleavit_reference_model.treasury import in_cap_prize

    if proposal.envelope is None:
        return Decimal(0)
    return in_cap_prize(
        proposal.proposal_class,
        ask=proposal.ask,
        envelope=proposal.envelope,
        spendable_nav=proposal.nav,
        upgrade_payload=proposal.upgrade_payload,
    )


def _epoch_groups(proposals: list[Proposal], config: SimulationConfig) -> dict[int, list[Proposal]]:
    groups: dict[int, list[Proposal]] = defaultdict(list)
    for proposal in proposals:
        groups[proposal.proposal_id // config.epoch_slate_size].append(proposal)
    return dict(sorted(groups.items()))


def _simulate_population(
    *,
    proposals: list[Proposal],
    seed: int,
    config: SimulationConfig,
    budget_multiple: Decimal,
    flow_cap: Decimal,
    delta_multiplier: Decimal = Decimal(1),
    flat_floor_delta: bool = False,
    b_multiplier: Decimal = Decimal(1),
    baseline_b_multiplier: Decimal = Decimal(1),
) -> list[SimulationResult]:
    results = []
    for _, slate in _epoch_groups(proposals, config).items():
        pooled = budget_multiple * Decimal(3) * sum((_prize(row) for row in slate), Decimal(0))
        for proposal in slate:
            own = budget_multiple * Decimal(3) * _prize(proposal)
            strategy = _strategy_for(proposal.proposal_id, config)
            delta_override = (
                min(DELTA_FLOORS[proposal.proposal_class] * delta_multiplier, Decimal("0.10"))
                if flat_floor_delta
                else None
            )
            results.append(
                simulate_proposal(
                    proposal,
                    seed=seed,
                    config=config,
                    budget_multiple=budget_multiple,
                    absolute_direct_budget=Decimal(0) if strategy == "th7_baseline_suppression" else own,
                    absolute_baseline_budget=pooled if strategy == "th7_baseline_suppression" else Decimal(0),
                    delta_multiplier=delta_multiplier,
                    delta_override=delta_override,
                    b_multiplier=b_multiplier,
                    baseline_b_multiplier=baseline_b_multiplier,
                    flow_cap=flow_cap,
                )
            )
    return sorted(results, key=lambda row: row.proposal.proposal_id)


def run_batch(*, seed: int = DEFAULT_SEED, proposal_count: int = 128, config: SimulationConfig | None = None) -> dict:
    config = SimulationConfig(proposal_count=proposal_count) if config is None else config
    config.validate()
    proposals = [generate_proposal_with_config(seed, index, config) for index in range(proposal_count)]
    results = _simulate_population(
        proposals=proposals,
        seed=seed,
        config=config,
        budget_multiple=Decimal(config.primary_manipulator_budget_multiple),
        flow_cap=Decimal(config.diagnostic_probe_flow_cap),
    )
    counts = Counter(proposal.proposal_class for proposal in proposals)
    return {
        "class_counts": {name: counts[name] for name in CLASSES},
        "metrics": _aggregate(results, config),
        "proposal_count": proposal_count,
        "results": [row.evidence() for row in results],
        "seed": seed,
    }


def _stratified_sample(proposals: list[Proposal], per_class: int, seed: int) -> list[Proposal]:
    selected = []
    for index, name in enumerate(CLASSES):
        rows = [row for row in proposals if row.proposal_class == name]
        rng = random.Random(seed ^ (index << 32) ^ 0x53414D50)
        selected.extend(rng.sample(rows, min(per_class, len(rows))))
    return sorted(selected, key=lambda row: row.proposal_id)


def _is_wrong_flip(proposal: Proposal, zero: SimulationResult, attacked: SimulationResult) -> tuple[bool, str]:
    if proposal.harmful:
        return zero.outcome != "Adopt" and attacked.outcome == "Adopt", "wrong_pass"
    return zero.outcome == "Adopt" and attacked.outcome != "Adopt", "wrong_reject"


def _threshold_brackets(
    proposals: list[Proposal],
    seed: int,
    config: SimulationConfig,
    flow_cap: Decimal,
    primary_results: list[SimulationResult] | None = None,
) -> dict:
    sample = _stratified_sample(
        proposals, config.threshold_sample_per_class, seed ^ 0x42524143
    )
    if primary_results is not None:
        # Every observed attacked wrong-PASS candidate gets a causal threshold.
        # A small pre-registered beneficial sample remains for the separately
        # labelled wrong-REJECT/griefing diagnostic.
        wrong_pass_ids = {
            row.proposal.proposal_id
            for row in primary_results
            if row.proposal.harmful and row.outcome == "Adopt"
        }
        grief_ids = {
            proposal.proposal_id for proposal in sample if not proposal.harmful
        }
        selected_ids = wrong_pass_ids | grief_ids
        sample = [
            proposal
            for proposal in proposals
            if proposal.proposal_id in selected_ids
        ]
    else:
        wrong_pass_ids = set()
        grief_ids = set()
    tolerance = Decimal(config.threshold_relative_tolerance)
    maximum = Decimal(config.threshold_max_budget_multiple)
    brackets = []
    griefing = []
    envelope_violations = []
    inconclusive = []
    noncausal = []
    for proposal in sample:
        zero = simulate_proposal(proposal, seed=seed, config=config, budget_multiple=Decimal(0), flow_cap=flow_cap)
        if proposal.proposal_id in wrong_pass_ids and zero.outcome == "Adopt":
            noncausal.append(
                {
                    "class": proposal.proposal_class,
                    "disposition": "noncausal_zero_budget",
                    "proposal_id": proposal.proposal_id,
                    "strategy": _strategy_for(proposal.proposal_id, config),
                    "zero_budget_outcome": zero.outcome,
                    "zero_budget_reason": zero.reason,
                }
            )
            continue
        high_limit = (
            Decimal(config.primary_manipulator_budget_multiple)
            if proposal.proposal_id in wrong_pass_ids
            else maximum
        )
        high = simulate_proposal(proposal, seed=seed, config=config, budget_multiple=high_limit, flow_cap=flow_cap)
        flips, direction = _is_wrong_flip(proposal, zero, high)
        if not flips and high_limit != maximum:
            high_limit = maximum
            high = simulate_proposal(proposal, seed=seed, config=config, budget_multiple=high_limit, flow_cap=flow_cap)
            flips, direction = _is_wrong_flip(proposal, zero, high)
        if not flips:
            if proposal.proposal_id in wrong_pass_ids:
                noncausal.append(
                    {
                        "class": proposal.proposal_class,
                        "disposition": "primary_wrong_pass_not_reproduced",
                        "proposal_id": proposal.proposal_id,
                        "strategy": _strategy_for(proposal.proposal_id, config),
                        "zero_budget_outcome": zero.outcome,
                        "zero_budget_reason": zero.reason,
                    }
                )
            continue
        lo, hi = Decimal(0), high_limit
        lo_result, hi_result = zero, high
        probes: dict[Decimal, tuple[bool, SimulationResult]] = {
            Decimal(0): (False, zero),
            high_limit: (True, high),
        }
        while (hi - lo) / max(hi, Decimal("0.000001")) > tolerance:
            mid = (lo + hi) / Decimal(2)
            result = simulate_proposal(proposal, seed=seed, config=config, budget_multiple=mid, flow_cap=flow_cap)
            mid_flips, _ = _is_wrong_flip(proposal, zero, result)
            probes[mid] = (mid_flips, result)
            if mid_flips:
                hi, hi_result = mid, result
            else:
                lo, lo_result = mid, result
        ordered_probes = sorted(probes.items())
        seen_flip = False
        monotone = True
        for _, (did_flip, _) in ordered_probes:
            if did_flip:
                seen_flip = True
            elif seen_flip:
                monotone = False
        flip_results = [
            result for _, (did_flip, result) in ordered_probes if did_flip
        ]
        flip_losses = [row.realized_manipulation_spend for row in flip_results]
        loss_monotone = all(
            left <= right for left, right in zip(flip_losses, flip_losses[1:])
        )
        row = {
            "attack_cost_hat": str(hi_result.attack_cost),
            "budget_bracket_3p_multiple": [str(lo), str(hi)],
            "budget_bracket_usdc": [str(lo * Decimal(3) * (hi_result.prize or Decimal(0))), str(hi * Decimal(3) * (hi_result.prize or Decimal(0)))],
            "class": proposal.proposal_class,
            "direction": direction,
            "loss_monotone": loss_monotone,
            "monotone": monotone,
            "probes": [
                [
                    str(value),
                    did_flip,
                    str(result.realized_manipulation_spend),
                ]
                for value, (did_flip, result) in ordered_probes
            ],
            "proposal_id": proposal.proposal_id,
            "realized_loss_observed_range": [
                str(min(flip_losses)),
                str(max(flip_losses)),
            ],
            "relative_width": str((hi - lo) / hi),
            "strategy": hi_result.strategy,
        }
        if direction == "wrong_reject":
            row["diagnostic"] = "griefing_cost_only"
            griefing.append(row)
            continue
        if hi_result.strategy in ("th2_late_spike", "th7_baseline_suppression"):
            row["diagnostic"] = "non_displacement_or_convergence_flip"
            griefing.append(row)
            continue
        bounds = [
            (
                result.manip_floor or Decimal(0),
                result.realized_manipulation_spend,
                result.attack_cost,
            )
            for result in flip_results
        ]
        observed_outside = any(
            loss < floor or loss > ceiling for floor, loss, ceiling in bounds
        )
        if not monotone:
            status = "inconclusive"
        elif observed_outside:
            status = "violation"
        elif not loss_monotone:
            status = "inconclusive"
        else:
            status = "clean"
        row.update(
            {
                "envelope_status": status,
                "manip_floor_hat_range": [
                    str(min(floor for floor, _, _ in bounds)),
                    str(max(floor for floor, _, _ in bounds)),
                ],
                "attack_cost_hat_range": [
                    str(min(ceiling for _, _, ceiling in bounds)),
                    str(max(ceiling for _, _, ceiling in bounds)),
                ],
                "sub_3p_status": (
                    "inconclusive"
                    if not monotone
                    else ("clean" if lo >= 1 else ("violation" if hi < 1 else "inconclusive"))
                ),
            }
        )
        brackets.append(row)
        if status == "violation":
            envelope_violations.append(row)
        elif status == "inconclusive":
            inconclusive.append(row)
    per_class = {}
    for name in CLASSES:
        rows = [row for row in brackets if row["class"] == name]
        per_class[name] = {
            "bracket_count": len(rows),
            "envelope_clean": all(row["envelope_status"] == "clean" for row in rows),
            "status": "measured" if rows else "no_causal_wrong_pass_observed",
            "sub_3p_clean": all(row["sub_3p_status"] == "clean" for row in rows),
        }
    return {
        "brackets": brackets,
        "envelope_inconclusive": inconclusive,
        "envelope_violations": envelope_violations,
        "griefing_cost_diagnostics": griefing,
        "noncausal_wrong_pass_dispositions": noncausal,
        "per_class": per_class,
        "relative_tolerance": str(tolerance),
        "wrong_pass_candidates": len(wrong_pass_ids),
        "griefing_sample_candidates": len(grief_ids),
        "sub_3p_every_class_clean": all(row["sub_3p_clean"] for row in per_class.values()),
    }


def _thin_market_capture(results: list[SimulationResult]) -> dict:
    promoted = [
        row for row in results
        if min(row.initial_contest_accept, row.initial_contest_reject) < row.v_min
        and min(row.contest_accept, row.contest_reject) >= row.v_min
        and row.manipulator_flow + row.arbitrage_flow > 0
    ]
    false_pass = [row for row in promoted if row.proposal.harmful and row.outcome == "Adopt"]
    def bounds(rows: list[SimulationResult], initial: bool) -> list[str] | None:
        values = [
            min(
                row.initial_contest_accept if initial else row.contest_accept,
                row.initial_contest_reject if initial else row.contest_reject,
            ) / row.v_min
            for row in rows
        ]
        return None if not values else [str(min(values)), str(max(values))]
    return {
        "confirmed_spec_seam": (
            "Attack-generated gross attacker+arbitrage flow can promote a below-v_min "
            "decision book and simultaneously inflate step-9 L_hat/AttackCost_hat; "
            "05 §5.6 caps wash flow only inside ManipFloor_hat.C_hold."
        ),
        "false_pass_promoted_count": len(false_pass),
        "initial_min_book_vmin_ratio_range": bounds(promoted, True),
        "promoted_count": len(promoted),
        "promoted_min_book_vmin_ratio_range": bounds(promoted, False),
    }


def _flow_cap(results: list[SimulationResult], config: SimulationConfig) -> tuple[Decimal, dict]:
    values = sorted(
        (row.contest_accept + row.contest_reject) / (Decimal(2) * row.b)
        for row in results
        if row.welfare_grade == "Ok"
    )
    if not values:
        return Decimal(config.diagnostic_probe_flow_cap), {"sample_count": 0, "calibrated_multiplier": config.diagnostic_probe_flow_cap}
    index = min(len(values) - 1, int(Decimal(config.flow_cap_quantile) * len(values)))
    raw = values[index]
    cap = raw.to_integral_value(rounding=ROUND_CEILING)
    return cap, {
        "calibrated_multiplier": str(cap),
        "quantile": config.flow_cap_quantile,
        "quantile_value": str(raw),
        "sample_count": len(values),
    }


def _publication(metrics: dict, attack: dict, flow_cap: Decimal, config: SimulationConfig) -> dict:
    candidates = {}
    margin = Decimal(config.publication_margin)
    for name in ("param", "code", "meta"):
        cost = attack_cost_hat(Decimal(2) * B_FLOORS[name] * LN2 + V_MIN_FLOORS[name])
        values = [
            value for value in range(0, int(cost / Decimal(3)) + config.publication_round_usdc, config.publication_round_usdc)
            if security_sizing_ok(Decimal(value), cost) and Decimal(3 * value) <= cost * (Decimal(1) - margin)
        ]
        candidates[f"sec.prize.{name}"] = max(values, default=0)
    candidates["sec.flow_cap"] = str(flow_cap)
    class_gate = {
        name: Decimal(metrics[name]["decidable_harm_false_pass_rate"]) < Decimal("0.01")
        for name in CLASSES
    }
    eligible = all(class_gate.values()) and attack["sub_3p_every_class_clean"]
    return {
        "candidates": candidates,
        "designation": "published" if eligible else "candidates-only",
        "eligibility": {
            "all_decidable_harm_false_pass_lt_1pct": all(class_gate.values()),
            "by_class": class_gate,
            "sub_3p_brackets_clean": attack["sub_3p_every_class_clean"],
        },
        "tested_floor_inputs": {
            "dec.delta": {name: str(DELTA_FLOORS[name]) for name in CLASSES},
            "pol.b": {name: str(B_FLOORS[name]) for name in CLASSES},
            "pol.b_baseline": int(BASELINE_B),
        },
    }


def normative_violations(metrics: dict, attack: dict) -> list[str]:
    """Derive normative failures from measured evidence, never committed labels."""
    violations = []
    for name in CLASSES:
        if Decimal(metrics[name]["decidable_harm_false_pass_rate"]) >= Decimal(
            "0.01"
        ):
            violations.append(
                f"{name}: decidable-harm false-pass rate is not below 1%"
            )
    if not attack["sub_3p_every_class_clean"]:
        violations.append("sub-3P threshold brackets are not clean in every class")
    if attack["envelope_violations"]:
        violations.append(
            "wrong-PASS displacement bracket violates ManipFloor/AttackCost envelope"
        )
    if attack["envelope_inconclusive"]:
        violations.append(
            "wrong-PASS displacement envelope validation is inconclusive"
        )
    return violations


def _pol_sizing(results: list[SimulationResult]) -> dict:
    identity = {}
    for name in CLASSES:
        prize = p_ref(name)
        depth = Decimal(2) * B_FLOORS[name] * LN2
        cost = attack_cost_hat(depth + dec_v_min(name, prize))
        expected = Decimal(3) * B_FLOORS[name] * LN2
        actual = cost - Decimal(3) * prize
        identity[name] = {
            "attack_cost_hat": str(cost),
            "identity_clean": expected - Decimal("0.000001") <= actual <= expected,
            "p_ref": str(prize),
            "v_min_equals_2p": dec_v_min(name, prize) == Decimal(2) * prize,
        }
    formation = {
        name: {
            regime: _rate(
                sum(row.welfare_grade == "Ok" for row in results if row.proposal.proposal_class == name and row.proposal.formation_regime == regime),
                sum(row.proposal.proposal_class == name and row.proposal.formation_regime == regime for row in results),
            )
            for regime in ("thin", "marginal", "deep")
        }
        for name in CLASSES
    }
    return {"decision_grade_by_formation_regime": formation, "v_min_identity": identity}


def outcome_digest(proposal_id: int, evidence: dict) -> str:
    body = json.dumps(evidence, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    return hashlib.sha256(b"bleavit.outcome.leaf.v1\0" + proposal_id.to_bytes(8, "big") + body).hexdigest()


def outcome_merkle_root(rows: list[dict]) -> str:
    if [row.get("proposal_id") for row in rows] != sorted(row.get("proposal_id") for row in rows):
        raise ValueError("outcome digests must be sorted by proposal_id")
    if len({row.get("proposal_id") for row in rows}) != len(rows):
        raise ValueError("duplicate proposal_id in outcome digests")
    level = [bytes.fromhex(row["digest"]) for row in rows]
    if not level:
        return hashlib.sha256(b"bleavit.outcome.empty.v1").hexdigest()
    while len(level) > 1:
        if len(level) % 2:
            level.append(level[-1])
        level = [
            hashlib.sha256(b"bleavit.outcome.node.v1\0" + level[i] + level[i + 1]).digest()
            for i in range(0, len(level), 2)
        ]
    return level[0].hex()


def _subsample_ids(seed: int, count: int, size: int) -> list[int]:
    rng = random.Random(seed ^ 0x53554253414D504C)
    return sorted(rng.sample(range(count), min(size, count)))


def run_full_calibration(*, seed: int = DEFAULT_SEED, config: SimulationConfig | None = None) -> dict:
    config = SimulationConfig() if config is None else config
    config.validate()
    if config.proposal_count < 10_000:
        raise ValueError("doc 15 §4.9 full calibration requires >= 10,000 proposals")
    proposals = [generate_proposal_with_config(seed, index, config) for index in range(config.proposal_count)]
    primary = _simulate_population(
        proposals=proposals,
        seed=seed,
        config=config,
        budget_multiple=Decimal(config.primary_manipulator_budget_multiple),
        flow_cap=Decimal(config.diagnostic_probe_flow_cap),
    )
    flow_cap, flow_evidence = _flow_cap(primary, config)
    metrics = _aggregate(primary, config)
    attack = _threshold_brackets(
        proposals,
        seed,
        config,
        flow_cap,
        primary_results=primary,
    )
    thin = _thin_market_capture(primary)
    publication = _publication(metrics, attack, flow_cap, config)
    violations = normative_violations(metrics, attack)
    counts = Counter(row.proposal_class for row in proposals)
    gate_veto_counts = {
        reason: sum(row.reason == reason for row in primary)
        for reason in ("GateVetoSurvival", "GateVetoSecurity")
    }
    evidence_rows = [row.evidence() for row in primary]
    digests = [
        {"digest": outcome_digest(row.proposal.proposal_id, evidence), "proposal_id": row.proposal.proposal_id}
        for row, evidence in zip(primary, evidence_rows)
    ]
    ids = _subsample_ids(seed, config.proposal_count, config.subsample_size)
    by_id = {row.proposal.proposal_id: evidence for row, evidence in zip(primary, evidence_rows)}
    return {
        "assumptions": {
            "a2_arbitrage": "A-2 corrective capacity is L/2 per day at elasticity 1; it is empirical and phase-revalidated.",
            "baseline_contest_floor": "Synthetic 250,000-USDC Baseline contest floor, a TREASURY-tier analogy to 08 §4.3; pending a specification question.",
            "coverage_leg": "Scheduled observations provide an always-clean coverage leg in Phase-0 synthetic runs.",
            "pol_leg": "POL is assumed seeded at the class schedule and undisturbed for step-5 grading.",
            "pre_registered_strata": {
                "attack_strategy_mix": [list(row) for row in config.attack_strategy_mix],
                "effect_delta_bands": [list(row) for row in config.effect_strata],
                "gate_exposure": ["gate", "no_gate"],
                "market_formation": [list(row) for row in config.formation_strata],
            },
            "reporting_gate": "Per 15 §4.9 and 08 §5.2/§5.5, publication gates on per-class |true_effect|>=delta harmful proposals and sub-3P brackets; the distribution-weighted aggregate remains reported.",
            "threshold_search": "Every observed attacked wrong-PASS candidate receives a state-identical zero-budget counterfactual and a binary budget bracket at the configured 5% relative tolerance; a pre-registered beneficial sample supplies griefing diagnostics.",
            "envelope_validation": "05 §5.6/08 §5.5 applies only to causal wrong-PASS displacement flips; observed loss non-monotonicity is reported fail-closed as inconclusive rather than inferred from endpoints.",
            "undefined_prize": "An explicit undefined envelope proxy is represented as null and rejects SecuritySizing.",
        },
        "attack_cost_validation": attack,
        "config": config.canonical(),
        "config_digest": config.digest(),
        "gate_veto_counts": gate_veto_counts,
        "metrics": metrics,
        "outcome_digest_root": outcome_merkle_root(digests),
        "outcome_digests": digests,
        "pol_sizing": _pol_sizing(primary),
        "proposal_count": config.proposal_count,
        "proposal_counts": {"by_class": {name: counts[name] for name in CLASSES}},
        "provenance": {"python_version": list(python_version_tuple())},
        "publication_evidence": {"flow_cap": flow_evidence},
        "published": publication,
        "schema": "bleavit.phase0-calibration.v3",
        "seed": seed,
        "subsample": {"proposal_ids": ids, "results": [by_id[proposal_id] for proposal_id in ids]},
        "thin_market_capture": thin,
        "violations": violations,
    }
