from decimal import Decimal
import random
import unittest

from bleavit_reference_model.lmsr import marginal_price_long
from bleavit_reference_model.lmsr import FEE_RATE, ceil_base
from bleavit_reference_model.twap import TwapAccumulator
from bleavit_simulation.market import (
    ExecutedBook,
    FastTwapAccumulator,
    execute_toward,
    fast_lmsr_price,
    simulate_book,
)


class FastMarketMathTests(unittest.TestCase):
    CLOSED_FORM_TOLERANCE = Decimal("1e-80")

    def assert_closed_form_equivalent(self, actual, expected):
        # Decimal's 100-digit exponentiation and repeated 100-digit
        # multiplication can differ in their final oracle digit. This bound is
        # still over sixty decimal orders tighter than the 64.64 chain grid.
        self.assertLessEqual(
            abs(actual - expected),
            self.CLOSED_FORM_TOLERANCE,
        )

    def test_segment_twap_matches_reference_on_randomized_paths(self):
        rng = random.Random(0x15_04_09)

        for path_index in range(32):
            initial = Decimal(rng.randrange(200_000, 800_001)) / Decimal(1_000_000)
            kappa = Decimal(rng.randrange(1, 21)) / Decimal(1_000)
            interval = rng.choice((5, 10, 20, 50))
            reference = TwapAccumulator(initial, kappa, interval)
            fast = FastTwapAccumulator(initial, kappa, interval)
            block = 0
            recorded_blocks = [0]

            for _ in range(40):
                # Include cadence-aligned observations, missed intervals, and stale
                # gaps. TwapAccumulator deliberately widens with floor(gap/interval).
                block += interval * rng.randrange(1, 13)
                quote = Decimal(rng.randrange(1_000, 999_001)) / Decimal(1_000_000)
                expected_value = reference.observe(block, quote)
                actual_value = fast.observe(block, quote)
                self.assertEqual(
                    actual_value,
                    expected_value,
                    f"path={path_index}, block={block}",
                )
                self.assertEqual(fast.last, reference.last)
                self.assertEqual(fast.cumulative, reference.points[-1].cumulative)
                self.assertEqual(fast.stale_events, reference.stale_events)
                for _ in range(2):
                    probe = rng.randrange(0, block + 1)
                    self.assertEqual(
                        fast.cumulative_at(probe),
                        reference.cumulative_at(probe),
                    )
                recorded_blocks.append(block)

            for _ in range(12):
                start_index = rng.randrange(0, len(recorded_blocks) - 1)
                end_index = rng.randrange(start_index + 1, len(recorded_blocks))
                start = recorded_blocks[start_index]
                end = recorded_blocks[end_index]
                self.assertEqual(fast.mean(start, end), reference.mean(start, end))

    def test_final_quote_spike_keeps_raw_and_observed_close_separate(self):
        summary = simulate_book(
            initial=Decimal("0.5"),
            kappa=Decimal("0.005"),
            interval=10,
            decision_window=43_200,
            trailing_window=14_400,
            path=[
                (43_190, Decimal("0.5")),
                (43_200, Decimal("0.95")),
            ],
        )

        self.assertEqual(summary.spot, Decimal("0.95"))
        self.assertEqual(summary.observed_close, Decimal("0.5025"))
        self.assertGreater(abs(summary.spot - summary.full), Decimal("0.05"))
        self.assertLess(abs(summary.observed_close - summary.full), Decimal("0.05"))

    def test_lmsr_price_shortcut_matches_reference(self):
        rng = random.Random(0x04_05)
        tolerance = Decimal("1e-90")

        for _ in range(256):
            b = Decimal(rng.randrange(10_000, 1_000_001)) / Decimal(10)
            # Stay far inside doc-04's absolute logit-domain clamp of 48.
            net_q = b * Decimal(rng.randrange(-20_000, 20_001)) / Decimal(1_000)
            expected = marginal_price_long(b, net_q, Decimal(0))
            actual = fast_lmsr_price(b, net_q)
            self.assertLessEqual(abs(actual - expected), tolerance)

    def test_segment_advance_matches_reference_for_arbitrary_gaps(self):
        rng = random.Random(0x15_04_09_07)

        for _ in range(24):
            initial = Decimal(rng.randrange(200_000, 800_001)) / Decimal(1_000_000)
            interval = rng.choice((5, 10, 20, 50))
            kappa = Decimal("0.005")
            reference = TwapAccumulator(initial, kappa, interval)
            fast = FastTwapAccumulator(initial, kappa, interval)
            block = 0

            for _ in range(32):
                # Exercise both exact cadence multiples and arbitrary remainders.
                if rng.randrange(2):
                    gap = interval * rng.randrange(1, 13)
                else:
                    gap = rng.randrange(1, 121)
                target_block = block + gap
                quote = Decimal(rng.randrange(1_000, 999_001)) / Decimal(1_000_000)
                steps, remainder = divmod(gap, interval)
                for _ in range(steps):
                    block += interval
                    expected = reference.observe(block, quote)
                if remainder:
                    block = target_block
                    expected = reference.observe(block, quote)
                actual = fast.advance(target_block, quote)
                self.assert_closed_form_equivalent(actual, expected)
                self.assert_closed_form_equivalent(fast.last, reference.last)
                self.assert_closed_form_equivalent(
                    fast.cumulative,
                    reference.points[-1].cumulative,
                )
                self.assertEqual(fast.stale_events, reference.stale_events)
                for _ in range(2):
                    probe = rng.randrange(0, block + 1)
                    self.assert_closed_form_equivalent(
                        fast.cumulative_at(probe),
                        reference.cumulative_at(probe),
                    )

    def test_event_ledger_is_fee_inclusive_and_cash_conserving(self):
        book = ExecutedBook("fixture", Decimal("25000"))
        book.account("informed", Decimal("20000"))
        book.account("arbitrage", Decimal("12000"))
        book.account("manipulator", Decimal("15000"))
        execute_toward(
            book,
            "informed",
            target=Decimal("0.62"),
            gross_notional=Decimal("8000"),
            block=10,
            role="informed",
        )
        execute_toward(
            book,
            "manipulator",
            target=Decimal("0.80"),
            gross_notional=Decimal("9000"),
            block=20,
            role="manipulator",
        )
        execute_toward(
            book,
            "arbitrage",
            target=Decimal("0.55"),
            gross_notional=Decimal("6000"),
            block=30,
            role="arbitrage",
        )
        for event in book.events:
            self.assertLessEqual(event.amount, book.b / Decimal(4))
            self.assertEqual(event.fee, ceil_base(event.cost * FEE_RATE))
        self.assertEqual(book.cash_conservation_error(), Decimal(0))
        self.assertEqual(book.settlement_conservation_error("long"), Decimal(0))
        self.assertEqual(book.settlement_conservation_error("short"), Decimal(0))
        self.assertEqual(
            book.contest_notional(),
            sum((event.cost for event in book.events), Decimal(0)),
        )
        self.assertGreater(book.contest_notional({"manipulator"}), 0)
        self.assertGreater(book.contest_notional({"arbitrage"}), 0)
        self.assertLess(
            book.liquidation_value("manipulator"),
            book.participants["manipulator"].initial_cash,
        )

    def test_balance_exhaustion_prevents_an_unbacked_fill(self):
        book = ExecutedBook("exhaustion", Decimal("10000"))
        book.account("poor", Decimal("1"))
        executed = execute_toward(
            book,
            "poor",
            target=Decimal("0.90"),
            gross_notional=Decimal("100000"),
            block=1,
            role="noise",
        )
        self.assertLessEqual(executed, Decimal(1))
        self.assertGreaterEqual(book.participants["poor"].cash, Decimal(0))
        self.assertLess(executed, Decimal("100000"))
        self.assertEqual(book.cash_conservation_error(), Decimal(0))
        self.assertEqual(book.settlement_conservation_error("long"), Decimal(0))
        self.assertEqual(book.settlement_conservation_error("short"), Decimal(0))


if __name__ == "__main__":
    unittest.main()
