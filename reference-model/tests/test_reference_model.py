from decimal import Decimal
import unittest

from bleavit_reference_model.lmsr import (
    PriceBoundExceeded,
    buy_delta_cost,
    vectors_v1_v6,
)
from bleavit_reference_model.twap import TwapAccumulator
from bleavit_reference_model.ledger import Vault, VaultState
from bleavit_reference_model.decision import decide, Outcome, RejectReason
from bleavit_reference_model.treasury import security_sizing_ok
from bleavit_reference_model.welfare import gate, settlement_score


class ReferenceModelTests(unittest.TestCase):
    def test_normative_lmsr_vectors(self):
        v = vectors_v1_v6()
        self.assertTrue(v["V1"]["value"].startswith("512.494795136"))
        self.assertTrue(v["V2"]["value"].startswith("0.524979187478"))
        self.assertTrue(v["V3"]["delta"].startswith("4054.65108108"))
        self.assertTrue(v["V4"]["value"].startswith("6931.47180559"))
        self.assertTrue(v["V5"]["net_fees_only"].startswith("-3.074968"))
        with self.assertRaises(PriceBoundExceeded):
            buy_delta_cost(10000, 480000, 0, "long", 1)

    def test_twap_slew_clamps_previous_quote(self):
        t = TwapAccumulator(Decimal("0.500"))
        self.assertEqual(t.observe(10, Decimal("0.900")), Decimal("0.502500"))
        self.assertEqual(t.mean(0, 20), Decimal("0.501250"))

    def test_twap_clamp_widens_over_missed_intervals(self):
        # 04 §7: over k missed observation intervals the clamp is (1±κ)^k.
        t = TwapAccumulator(Decimal("0.500"))
        expected_hi = Decimal("0.500") * Decimal("1.005") ** 10
        self.assertEqual(t.observe(100, Decimal("0.900")), expected_hi)
        t2 = TwapAccumulator(Decimal("0.500"))
        expected_lo = Decimal("0.500") * Decimal("0.995") ** 10
        self.assertEqual(t2.observe(100, Decimal("0.100")), expected_lo)
        # A quote inside the widened band is recorded unclamped.
        t3 = TwapAccumulator(Decimal("0.500"))
        self.assertEqual(t3.observe(100, Decimal("0.510")), Decimal("0.510"))

    def test_ledger_void_neutral_flooring(self):
        v = Vault()
        v.split(Decimal("10.000000"))
        v.void()
        self.assertEqual(v.state, VaultState.VOIDED)
        self.assertEqual(v.redeem_void_branch_usdc(Decimal("10.000001")), Decimal("5.000000"))
        self.assertEqual(v.redeem_void_scalar_leg(Decimal("10.000003")), Decimal("2.500000"))

    def test_decision_and_treasury_reason_codes(self):
        below = decide(Decimal("0.54"), Decimal("0.50"), Decimal("0.05"))
        self.assertEqual(below.reason, RejectReason.HURDLE_NOT_MET)
        adopt = decide(Decimal("0.56"), Decimal("0.50"), Decimal("0.05"))
        self.assertEqual(adopt.outcome, Outcome.ADOPT)
        self.assertFalse(security_sizing_ok(Decimal("10"), Decimal("20")))

    def test_decision_window_and_convergence_semantics(self):
        # 05 §5.4 steps 6-8: a pure convergence failure (windows agree) rejects
        # with ConvergenceFailed on the FIRST decide - it never extends.
        first = decide(Decimal("0.56"), Decimal("0.50"), Decimal("0.05"), converged=False)
        self.assertEqual((first.outcome, first.reason), (Outcome.REJECT, RejectReason.CONVERGENCE_FAILED))
        # Extend fires only on full/trailing disagreement, once.
        split = decide(
            Decimal("0.56"), Decimal("0.50"), Decimal("0.05"),
            accept_trailing=Decimal("0.52"), reject_trailing_effective=Decimal("0.50"),
        )
        self.assertEqual(split.outcome, Outcome.EXTEND)
        recurred = decide(
            Decimal("0.56"), Decimal("0.50"), Decimal("0.05"),
            accept_trailing=Decimal("0.52"), reject_trailing_effective=Decimal("0.50"),
            extended=True,
        )
        self.assertEqual((recurred.outcome, recurred.reason), (Outcome.REJECT, RejectReason.SECOND_EXTENSION_FAILED))
        # Joint window failure while converged is a hurdle failure even if extended.
        joint = decide(Decimal("0.54"), Decimal("0.50"), Decimal("0.05"), extended=True)
        self.assertEqual((joint.outcome, joint.reason), (Outcome.REJECT, RejectReason.HURDLE_NOT_MET))

    def test_settlement_score_is_horizon_geomean(self):
        # 05 §4.4 (4) / 08 §8.1: s = GeoMean(W_{e+1}, W_{e+2}) with the ε_W
        # floor, rounded down to the 1e9 grid.
        self.assertEqual(settlement_score(Decimal("0.8"), Decimal("0.8")), Decimal("0.800000000"))
        self.assertEqual(settlement_score(Decimal("0.64"), Decimal("0.25")), Decimal("0.400000000"))
        zeroed = settlement_score(Decimal("0"), Decimal("0.5"))
        self.assertEqual(zeroed, Decimal("0.000022360"))


    def test_welfare_gate_and_settlement_vectors(self):
        self.assertEqual(gate(Decimal("0.85"), Decimal("0.85"), Decimal("0.95")), Decimal("0"))
        self.assertEqual(gate(Decimal("0.95"), Decimal("0.85"), Decimal("0.95")), Decimal("1"))
        self.assertEqual(settlement_score(Decimal("1"), Decimal("1")), Decimal("1"))
        self.assertTrue(str(settlement_score(Decimal("0"), Decimal("1"))).startswith("0.000031622"))


    def test_welfare_gate_and_settlement_vectors(self):
        self.assertEqual(gate(Decimal("0.85"), Decimal("0.85"), Decimal("0.95")), Decimal("0"))
        self.assertEqual(gate(Decimal("0.95"), Decimal("0.85"), Decimal("0.95")), Decimal("1"))
        self.assertEqual(settlement_score(Decimal("1"), Decimal("1")), Decimal("1"))
        self.assertTrue(str(settlement_score(Decimal("0"), Decimal("1"))).startswith("0.000031622"))


if __name__ == "__main__":
    unittest.main()
