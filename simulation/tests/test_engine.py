from dataclasses import replace
from decimal import Decimal
import unittest

from bleavit_reference_model.decision import Outcome
from bleavit_reference_model.treasury import LN2, in_cap_prize, l_hat
from bleavit_simulation.config import (
    DEFAULT_SEED,
    GATE_EPS,
    GATE_P_MAX,
    SimulationConfig,
)
from bleavit_simulation.engine import (
    _extend_gate_books,
    _gate_books,
    _signed_manip_floor,
    _stale_decision,
    simulate_proposal,
)
from bleavit_simulation.market import (
    ExecutedBook,
    contest_capital,
    execute_turnover,
)
from bleavit_simulation.proposals import generate_proposal_with_config


class ExecutedEngineTests(unittest.TestCase):
    def test_known_thin_book_cannot_use_balanced_inventory_as_depth(self):
        """At-risk regression on the pre-amendment thin-capture fixture.

        Proposal 57 was the committed wrong-PASS replay: attack-generated
        gross flow promoted a below-v_min pair and self-funded the step-9
        certificate. Even at 3×3P the at-risk measure excludes the organic
        balanced complete-set inventory, so the remaining directional maker
        state cannot clear the floor and the harmful proposal stays rejected.
        """
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
        self.assertEqual(attacked.outcome, "Reject")
        self.assertEqual(attacked.reason, "NotDecisionGrade")
        self.assertLess(
            min(attacked.contest_accept, attacked.contest_reject), attacked.v_min
        )
        self.assertGreater(attacked.manipulator_flow, 0)
        self.assertGreater(attacked.arbitrage_flow, 0)
        self.assertGreater(
            attacked.realized_manipulation_spend,
            Decimal(3) * attacked.prize,
        )
        # L-hat is capped: POL depth + sec.flow_cap * (b_acc + b_rej).
        flow_cap = Decimal(config.diagnostic_probe_flow_cap)
        self.assertLessEqual(
            attacked.measured_liquidity,
            Decimal(2) * attacked.b * LN2 + flow_cap * Decimal(2) * attacked.b,
        )

    def test_wash_churn_no_longer_buys_the_certificate(self):
        """A pure-churn manipulator adds gross flow but zero contest capital."""
        config = SimulationConfig(proposal_count=1)
        book = ExecutedBook("churn", Decimal("10000"))
        book.account("manipulator", Decimal("500000"))
        for block in (1, 7_201, 14_401):
            execute_turnover(
                book,
                "manipulator",
                gross_notional=Decimal("120000"),
                block=block,
                role="manipulator",
                first_side="long",
            )
        self.assertGreater(book.contest_notional({"manipulator"}), Decimal("100000"))
        self.assertEqual(
            contest_capital(book, decision_window=config.decision_window),
            Decimal(0),
        )
        self.assertEqual(
            l_hat(
                Decimal(2) * book.b * LN2,
                contest_capital(book, decision_window=config.decision_window),
                Decimal(config.diagnostic_probe_flow_cap),
                book.b,
                book.b,
            ),
            Decimal(2) * book.b * LN2,
        )

    def test_gate_price_signals_survive_but_riskless_depth_is_not_grade(self):
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
        self.assertEqual(survival.initial_gate_vetoes[0], "survival")
        self.assertIn("security", security.initial_gate_vetoes)
        self.assertEqual(survival.reason, "NotDecisionGrade")
        self.assertEqual(security.reason, "NotDecisionGrade")
        self.assertTrue(all(row.contest > 0 for row in survival.gate_books))
        self.assertTrue(any(not row.valid for row in survival.gate_books))

    def test_gated_attacker_suppresses_veto_from_one_shared_budget(self):
        """Seeded META/TH-4 demo: real gate suppression still executes."""
        config = SimulationConfig(proposal_count=400)
        proposal = generate_proposal_with_config(DEFAULT_SEED, 5, config)
        organic = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal(0),
        )
        suppressed = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal("0.5"),
        )
        adopted = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal(3),
        )

        self.assertEqual(organic.strategy, "th4_thin_capture")
        self.assertEqual(organic.reason, "NotDecisionGrade")
        self.assertEqual(organic.initial_gate_vetoes, ("security",))
        self.assertEqual(suppressed.initial_gate_vetoes, ())
        organic_adopt = next(
            row
            for row in organic.gate_books
            if row.gate == "security" and row.branch == "adopt"
        )
        suppressed_adopt = next(
            row
            for row in suppressed.gate_books
            if row.gate == "security" and row.branch == "adopt"
        )
        suppressed_reject = next(
            row
            for row in suppressed.gate_books
            if row.gate == "security" and row.branch == "reject"
        )
        self.assertGreater(organic_adopt.summary.full, GATE_P_MAX)
        self.assertGreater(suppressed.gate_manipulator_flow, 0)
        self.assertLess(suppressed_adopt.summary.full, organic_adopt.summary.full)
        self.assertLessEqual(suppressed_adopt.summary.full, GATE_P_MAX)
        self.assertLessEqual(
            suppressed_adopt.summary.full,
            suppressed_reject.summary.full + GATE_EPS,
        )
        self.assertEqual(suppressed.reason, "NotDecisionGrade")
        self.assertEqual(adopted.outcome, "Reject")
        self.assertLessEqual(
            abs(
                adopted.decision_attack_budget
                + adopted.gate_attack_budget
                - adopted.attacker_budget
            ),
            Decimal("0.000001"),
        )
        self.assertGreater(adopted.gate_attack_budget, 0)
        self.assertLessEqual(
            adopted.realized_manipulation_spend, adopted.attacker_budget
        )
        self.assertEqual(
            suppressed.evidence(),
            simulate_proposal(
                proposal,
                seed=DEFAULT_SEED,
                config=config,
                budget_multiple=Decimal("0.5"),
            ).evidence(),
        )

    def test_th6_belief_capture_also_funds_gate_suppression(self):
        config = SimulationConfig(proposal_count=400)
        proposal = generate_proposal_with_config(DEFAULT_SEED, 34, config)
        attacked = simulate_proposal(
            proposal,
            seed=DEFAULT_SEED,
            config=config,
            budget_multiple=Decimal("0.5"),
        )
        self.assertEqual(attacked.strategy, "th6_belief_capture")
        self.assertGreater(attacked.gate_attack_budget, 0)
        self.assertGreater(attacked.gate_manipulator_flow, 0)
        self.assertLessEqual(
            abs(
                attacked.decision_attack_budget
                + attacked.gate_attack_budget
                - attacked.attacker_budget
            ),
            Decimal("0.000001"),
        )

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
        self.assertEqual(quiet_result.outcome, "Reject")
        self.assertEqual(noisy_result.outcome, "Reject")
        self.assertEqual(quiet_result.noise_flow, Decimal(0))
        self.assertGreater(noisy_result.noise_flow, 0)
        self.assertNotEqual(
            (quiet_result.contest_accept, quiet_result.contest_reject),
            (noisy_result.contest_accept, noisy_result.contest_reject),
        )
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
            contest_capital=Decimal("500000"),
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
