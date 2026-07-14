#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, sys
from decimal import Decimal
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "reference-model" / "src"))
from bleavit_reference_model.lmsr import vectors_v1_v6, cost, marginal_price_long, raw_64x64_nearest, fmt
from bleavit_reference_model.decision import decide
from bleavit_reference_model.twap import TwapAccumulator
from bleavit_reference_model.welfare import settlement_score

# decide() scenario inputs (05 §5.4 steps 6-8); outcomes are computed through
# the model below, never hand-written (reference-model rule 2).
DECISION_SCENARIOS = [
    {"name": "hurdle_met", "accept_full": "0.56", "reject_full_effective": "0.50", "delta": "0.05"},
    {"name": "below_hurdle", "accept_full": "0.54", "reject_full_effective": "0.50", "delta": "0.05"},
    {"name": "convergence_failed_first_decide", "accept_full": "0.56", "reject_full_effective": "0.50", "delta": "0.05", "converged": False},
    {"name": "window_disagreement_extends_once", "accept_full": "0.56", "reject_full_effective": "0.50", "delta": "0.05", "accept_trailing": "0.52", "reject_trailing_effective": "0.50"},
    {"name": "window_disagreement_recurs_rejects", "accept_full": "0.56", "reject_full_effective": "0.50", "delta": "0.05", "accept_trailing": "0.52", "reject_trailing_effective": "0.50", "extended": True},
]

# settlement_score inputs (05 §4.4 (4) / 08 §8.1).
WELFARE_SCENARIOS = [
    {"name": "equal_horizons", "w_next": "0.8", "w_next_2": "0.8"},
    {"name": "mixed_horizons", "w_next": "0.64", "w_next_2": "0.25"},
    {"name": "zeroed_epoch_epsilon_floor", "w_next": "0", "w_next_2": "0.5"},
]

def build():
    samples=[]
    for ql, qs in [(0,0),(1000,0),(2500,0),(0,2500),(12345,6789),(240000,0),(0,240000),(480000,0)]:
        c=cost(10000, ql, qs); p=marginal_price_long(10000, ql, qs)
        samples.append({"q_long": str(ql), "q_short": str(qs), "cost": fmt(c), "cost_raw_64x64_nearest": raw_64x64_nearest(c), "price_long": fmt(p), "price_raw_64x64_nearest": raw_64x64_nearest(p)})
    decision_scenarios=[]
    for scenario in DECISION_SCENARIOS:
        kwargs={k: (Decimal(v) if isinstance(v, str) else v) for k, v in scenario.items() if k != "name"}
        decision=decide(**kwargs)
        row=dict(scenario); row["outcome"]=decision.outcome.value
        if decision.reason is not None: row["reason"]=decision.reason.value
        decision_scenarios.append(row)
    welfare_scenarios=[]
    for scenario in WELFARE_SCENARIOS:
        row=dict(scenario)
        row["s"]=str(settlement_score(Decimal(scenario["w_next"]), Decimal(scenario["w_next_2"])))
        welfare_scenarios.append(row)
    twap=TwapAccumulator(Decimal("0.500"))
    twap_scenarios=[{
        "name": "missed_intervals_widen_clamp",
        "initial": "0.500", "kappa": "0.005", "obs_interval": "10",
        "observe_block": "100", "previous_quote": "0.900",
        "recorded": fmt(twap.observe(100, Decimal("0.900"))),
    }]
    return {"schema":"bleavit.reference-model.v1","precision":"Python Decimal, 90 decimal digits (>=256-bit precision target)","lmsr_vectors":vectors_v1_v6(),"high_precision_corpus":{"b":"10000","samples":samples},"decision_scenarios":decision_scenarios,"welfare_scenarios":welfare_scenarios,"twap_scenarios":twap_scenarios,"ledger_scenarios":[{"name":"void_neutral_branch_usdc","input":"10.000000","payout":"5.000000"}]}

def main():
    p=argparse.ArgumentParser(); p.add_argument("--check", action="store_true"); p.add_argument("--out", default="reference-model/fixtures/vectors.json"); a=p.parse_args()
    text=json.dumps(build(), sort_keys=True, indent=2)+"\n"; path=Path(a.out)
    if a.check:
        if not path.exists() or path.read_text()!=text:
            raise SystemExit(f"{path} is stale; run tools/reference-model/generate-vectors.py")
    else:
        path.parent.mkdir(parents=True, exist_ok=True); path.write_text(text)
if __name__ == "__main__": main()
