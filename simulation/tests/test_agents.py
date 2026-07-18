from decimal import Decimal
import unittest

from bleavit_simulation.agents import arbitrage_flow, budgeted_move_price


class AgentBehaviorTests(unittest.TestCase):
    def test_informed_capital_moves_price_toward_planted_truth(self):
        b = Decimal("25000")
        start = Decimal("0.50")

        for truth in (Decimal("0.32"), Decimal("0.71")):
            moved = budgeted_move_price(
                b=b,
                current_price=start,
                target_price=truth,
                budget=Decimal("5000"),
                liquidity=Decimal(0),
                days=Decimal(0),
                elasticity=Decimal(1),
            )
            self.assertLess(abs(moved.price - truth), abs(start - truth))
            self.assertLessEqual(moved.spend, Decimal("5000"))
            self.assertGreaterEqual(moved.spend, Decimal(0))

    def test_arbitrage_flow_is_a2_half_depth_per_day(self):
        liquidity = Decimal("434657.359028")
        self.assertEqual(
            arbitrage_flow(liquidity, days=Decimal(1), elasticity=Decimal(1)),
            liquidity / Decimal(2),
        )
        self.assertEqual(
            arbitrage_flow(
                liquidity,
                days=Decimal("0.25"),
                elasticity=Decimal("0.40"),
            ),
            liquidity / Decimal(2) * Decimal("0.25") * Decimal("0.40"),
        )
        self.assertEqual(
            arbitrage_flow(liquidity, days=Decimal(0), elasticity=Decimal(1)),
            Decimal(0),
        )

    def test_manipulator_budget_monotonically_increases_displacement(self):
        b = Decimal("60000")
        start = Decimal("0.50")
        budgets = (
            Decimal("1000"),
            Decimal("5000"),
            Decimal("25000"),
            Decimal("100000"),
        )
        results = [
            budgeted_move_price(
                b=b,
                current_price=start,
                target_price=Decimal("0.95"),
                budget=budget,
                liquidity=Decimal(0),
                days=Decimal(0),
                elasticity=Decimal(1),
            )
            for budget in budgets
        ]
        displacements = [result.price - start for result in results]
        self.assertEqual(displacements, sorted(displacements))
        for result, budget in zip(results, budgets):
            self.assertLessEqual(result.spend, budget)


if __name__ == "__main__":
    unittest.main()
