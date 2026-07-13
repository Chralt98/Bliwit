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

    def test_ledger_void_neutral_flooring(self):
        v = Vault()
        v.split(Decimal("10.000000"))
        v.void()
        self.assertEqual(v.state, VaultState.VOIDED)
        self.assertEqual(v.redeem_void_branch_usdc(Decimal("10.000001")), Decimal("5.000000"))
        self.assertEqual(v.redeem_void_scalar_leg(Decimal("10.000003")), Decimal("2.500000"))

    def test_decision_and_treasury_reason_codes(self):
        self.assertEqual(decide(Decimal("0.04"), Decimal("0.05")).reason, RejectReason.HURDLE_NOT_MET)
        self.assertEqual(decide(Decimal("0.06"), Decimal("0.05")).outcome, Outcome.ADOPT)
        self.assertFalse(security_sizing_ok(Decimal("10"), Decimal("20")))


if __name__ == "__main__":
    unittest.main()
