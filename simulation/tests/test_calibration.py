from dataclasses import replace
from decimal import Decimal
import unittest
from unittest import mock

from bleavit_reference_model.decision import decide as reference_decide
from bleavit_simulation.calibration import (
    _threshold_brackets,
    run_batch,
)
from bleavit_simulation.config import DEFAULT_SEED, SimulationConfig
from bleavit_simulation.engine import simulate_proposal
from bleavit_simulation.proposals import generate_proposal_with_config


class CalibrationBatchTests(unittest.TestCase):
    def test_informed_population_usually_aligns_with_planted_truth(self):
        seed = 0x15_04_09_01
        config = SimulationConfig(proposal_count=24)
        aligned = []
        for proposal_id in range(24):
            proposal = generate_proposal_with_config(seed, proposal_id, config)
            result = simulate_proposal(
                proposal, seed=seed, config=config, budget_multiple=Decimal(0)
            )
            aligned.append(
                (result.accept.full - result.reject.full) * proposal.true_effect > 0
            )
        self.assertGreaterEqual(sum(aligned) / len(aligned), 0.60)

    def test_same_seed_produces_identical_batch(self):
        first = run_batch(seed=0x5_4_0001, proposal_count=8)
        second = run_batch(seed=0x5_4_0001, proposal_count=8)
        self.assertEqual(first, second)

    def test_small_batch_reaches_reference_decision_engine(self):
        with mock.patch(
            "bleavit_simulation.engine.decide", wraps=reference_decide
        ) as decision_oracle:
            result = run_batch(seed=0x15_04_0009, proposal_count=8)
        self.assertGreaterEqual(decision_oracle.call_count, 8)
        self.assertEqual(result["proposal_count"], 8)

    def test_effect_reweighting_changes_aggregate_not_conditional_rates(self):
        conditional = {
            "sub_half_delta": Decimal("0.20"),
            "half_to_one_delta": Decimal("0.10"),
            "one_to_two_delta": Decimal("0.02"),
            "two_to_three_delta": Decimal("0.00"),
        }
        weights_a = {
            "sub_half_delta": Decimal("0.15"),
            "half_to_one_delta": Decimal("0.15"),
            "one_to_two_delta": Decimal("0.40"),
            "two_to_three_delta": Decimal("0.30"),
        }
        weights_b = {
            "sub_half_delta": Decimal("0.70"),
            "half_to_one_delta": Decimal("0.10"),
            "one_to_two_delta": Decimal("0.10"),
            "two_to_three_delta": Decimal("0.10"),
        }
        aggregate_a = sum(weights_a[key] * value for key, value in conditional.items())
        aggregate_b = sum(weights_b[key] * value for key, value in conditional.items())
        self.assertNotEqual(aggregate_a, aggregate_b)
        self.assertEqual(conditional["one_to_two_delta"], Decimal("0.02"))

    def test_at_risk_measure_removes_the_seeded_thin_flip_through_search_cap(self):
        config = SimulationConfig(proposal_count=200, threshold_sample_per_class=8)
        proposal = generate_proposal_with_config(DEFAULT_SEED, 57, config)
        result = _threshold_brackets(
            [proposal], DEFAULT_SEED, config, Decimal(20)
        )
        self.assertEqual(result["brackets"], [])
        high = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal(config.threshold_max_budget_multiple),
        )
        self.assertEqual(high.outcome, "Reject")
        self.assertEqual(high.reason, "NotDecisionGrade")
        self.assertLess(min(high.contest_accept, high.contest_reject), high.v_min)

    def test_gate_suppression_budget_executes_even_when_at_risk_grade_rejects(self):
        config = replace(
            SimulationConfig(proposal_count=400),
            threshold_relative_tolerance="0.01",
        )
        proposal = generate_proposal_with_config(DEFAULT_SEED, 5, config)
        high = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal(3),
        )
        result = _threshold_brackets(
            [proposal],
            DEFAULT_SEED,
            config,
            Decimal(config.diagnostic_probe_flow_cap),
            [high],
        )
        self.assertEqual(result["brackets"], [])
        self.assertEqual(high.outcome, "Reject")
        self.assertEqual(high.reason, "NotDecisionGrade")
        self.assertGreater(high.gate_attack_budget, 0)
        self.assertGreater(high.gate_manipulator_flow, 0)
        self.assertLessEqual(
            abs(
                high.decision_attack_budget
                + high.gate_attack_budget
                - high.attacker_budget
            ),
            Decimal("0.000001"),
        )


if __name__ == "__main__":
    unittest.main()
