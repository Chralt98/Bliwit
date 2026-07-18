from dataclasses import replace
from decimal import Decimal
import unittest

from bleavit_reference_model.decision import Outcome
from bleavit_reference_model.treasury import in_cap_prize
from bleavit_simulation.config import DEFAULT_SEED, SimulationConfig
from bleavit_simulation.engine import (
    _extend_gate_books,
    _gate_books,
    _signed_manip_floor,
    _stale_decision,
    simulate_proposal,
)
from bleavit_simulation.proposals import generate_proposal_with_config


class ExecutedEngineTests(unittest.TestCase):
    def test_known_wrong_pass_replay_preserves_thin_market_capture_seam(self):
        config = SimulationConfig(proposal_count=200)
        proposal = generate_proposal_with_config(DEFAULT_SEED, 57, config)
        zero = simulate_proposal(
            proposal, seed=DEFAULT_SEED, config=config, budget_multiple=Decimal(0)
        )
        attacked = simulate_proposal(
            proposal, seed=DEFAULT_SEED, config=config, budget_multiple=Decimal(3)
        )

        self.assertTrue(proposal.harmful)
        self.assertEqual(zero.outcome, "Reject")
        self.assertEqual(zero.reason, "NotDecisionGrade")
        self.assertLess(min(zero.contest_accept, zero.contest_reject), zero.v_min)
        self.assertEqual(attacked.outcome, "Adopt")
        self.assertGreaterEqual(
            min(attacked.contest_accept, attacked.contest_reject), attacked.v_min
        )
        self.assertGreater(attacked.manipulator_flow, 0)
        self.assertGreater(attacked.arbitrage_flow, 0)
        self.assertGreater(attacked.attack_cost, zero.attack_cost)

    def test_real_gate_books_reach_both_ordered_vetoes(self):
        config = SimulationConfig(proposal_count=1)
        survival = simulate_proposal(
            generate_proposal_with_config(3, 0, config),
            seed=3,
            config=config,
            budget_multiple=Decimal(0),
        )
        security = simulate_proposal(
            generate_proposal_with_config(20, 0, config),
            seed=20,
            config=config,
            budget_multiple=Decimal(0),
        )
        self.assertEqual(len(survival.gate_books), 4)
        self.assertEqual(survival.reason, "GateVetoSurvival")
        self.assertEqual(security.reason, "GateVetoSecurity")
        self.assertTrue(all(row.contest > 0 for row in survival.gate_books))

    def test_upgrade_payload_scope_propagates_without_decide_signature_change(self):
        config = SimulationConfig(proposal_count=1)
        proposal = generate_proposal_with_config(1, 0, config)
        self.assertIn(proposal.proposal_class, ("code", "meta"))
        upgrade = replace(proposal, upgrade_payload=True)
        ordinary = replace(proposal, upgrade_payload=False)
        upgrade_result = simulate_proposal(
            upgrade, seed=1, config=config, budget_multiple=Decimal(0)
        )
        ordinary_result = simulate_proposal(
            ordinary, seed=1, config=config, budget_multiple=Decimal(0)
        )
        self.assertEqual(
            upgrade_result.prize,
            in_cap_prize(
                proposal.proposal_class,
                ask=proposal.ask,
                envelope=proposal.envelope,
                spendable_nav=proposal.nav,
                upgrade_payload=True,
            ),
        )
        self.assertEqual(
            ordinary_result.prize,
            max(proposal.ask, proposal.envelope or Decimal(0)),
        )
        self.assertGreaterEqual(upgrade_result.prize, ordinary_result.prize)

    def test_noise_share_changes_a_marginal_decision(self):
        quiet = SimulationConfig(proposal_count=1, noise_flow_share="0.00")
        noisy = replace(quiet, noise_flow_share="0.99")
        p_quiet = generate_proposal_with_config(31, 0, quiet)
        p_noisy = generate_proposal_with_config(31, 0, noisy)
        quiet_result = simulate_proposal(
            p_quiet, seed=31, config=quiet, budget_multiple=Decimal(0)
        )
        noisy_result = simulate_proposal(
            p_noisy, seed=31, config=noisy, budget_multiple=Decimal(0)
        )
        self.assertNotEqual(quiet_result.outcome, noisy_result.outcome)
        self.assertEqual(quiet_result.noise_flow, Decimal(0))
        self.assertGreater(noisy_result.noise_flow, 0)
        self.assertEqual(
            noisy_result.evidence(),
            simulate_proposal(
                p_noisy, seed=31, config=noisy, budget_multiple=Decimal(0)
            ).evidence(),
        )

    def test_stale_events_use_the_shared_extension_budget(self):
        first = _stale_decision(1, extended=False)
        second = _stale_decision(2, extended=False)
        after_other_extension = _stale_decision(1, extended=True)
        self.assertEqual(first.outcome, Outcome.EXTEND)
        self.assertEqual(second.outcome, Outcome.REJECT)
        self.assertEqual(after_other_extension.outcome, Outcome.REJECT)

    def test_undefined_prize_proxy_is_explicit_and_never_adopts(self):
        config = SimulationConfig(proposal_count=1)
        proposal = generate_proposal_with_config(1, 0, config)
        undefined = replace(proposal, envelope=None, upgrade_payload=False)
        result = simulate_proposal(
            undefined, seed=1, config=config, budget_multiple=Decimal(0)
        )
        self.assertIsNone(result.prize)
        self.assertIsNone(result.evidence()["prize"])
        self.assertEqual(result.outcome, "Reject")
        self.assertEqual(result.reason, "SecuritySizing")

    def test_signed_manip_floor_uses_opposite_book_directions(self):
        value, components = _signed_manip_floor(
            b=Decimal("25000"),
            accept_price=Decimal("0.45"),
            reject_price=Decimal("0.58"),
            delta=Decimal("0.025"),
            contest_notional=Decimal("500000"),
            flow_cap=Decimal(20),
        )
        self.assertGreater(value, 0)
        self.assertGreater(components[0], 0)
        self.assertGreater(components[1], 0)
        self.assertNotEqual(components[0], components[1])

    def test_epsilon_budget_without_fill_is_state_identical(self):
        config = SimulationConfig(proposal_count=10_000)
        proposal = generate_proposal_with_config(DEFAULT_SEED, 6536, config)
        zero = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal(0),
            flow_cap=Decimal(23),
        )
        epsilon = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal("4.470348358154296875e-8"),
            flow_cap=Decimal(23),
        )
        self.assertEqual(epsilon.manipulator_flow, 0)
        self.assertEqual(epsilon.arbitrage_flow, 0)
        self.assertEqual(epsilon.outcome, zero.outcome)
        self.assertEqual(epsilon.reason, zero.reason)
        self.assertEqual(epsilon.accept, zero.accept)
        self.assertEqual(epsilon.reject, zero.reject)

    def test_gate_extension_preserves_ledger_identity_and_positions(self):
        config = SimulationConfig(proposal_count=1)
        proposal = generate_proposal_with_config(3, 0, config)
        books, evidence = _gate_books(
            proposal,
            seed=3,
            config=config,
            v_min=Decimal("100000"),
            extension=False,
        )
        identities = [id(book) for book in books]
        positions = [(book.q_long, book.q_short) for book in books]
        extended, extended_evidence = _extend_gate_books(
            proposal,
            books,
            evidence,
            seed=3,
            config=config,
            v_min=Decimal("100000"),
        )
        self.assertEqual([id(book) for book in extended], identities)
        self.assertTrue(
            any(
                (book.q_long, book.q_short) != before
                for book, before in zip(extended, positions)
            )
        )
        self.assertEqual(len(extended_evidence), 4)


if __name__ == "__main__":
    unittest.main()
