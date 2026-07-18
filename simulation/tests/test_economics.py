from decimal import Decimal
import unittest

from bleavit_reference_model.treasury import (
    B_FLOORS,
    LN2,
    attack_cost_hat,
    dec_v_min,
    p_ref,
    pol_b,
    security_sizing_ok,
)


class EconomicIdentityTests(unittest.TestCase):
    def test_v_min_equals_two_p_and_leaves_seeded_pol_margin(self):
        # 08 §5.3: at P_ref the Ask-scaled b schedule is still exactly at its
        # floor, while the kernel 2P branch of dec.v_min is binding.
        for proposal_class, b_floor in B_FLOORS.items():
            prize = p_ref(proposal_class)
            self.assertEqual(pol_b(proposal_class, prize), b_floor)
            self.assertEqual(dec_v_min(proposal_class, prize), Decimal(2) * prize)

            seeded_depth = Decimal(2) * b_floor * LN2
            measured_liquidity = seeded_depth + dec_v_min(proposal_class, prize)
            attack_cost = attack_cost_hat(measured_liquidity)
            self.assertTrue(security_sizing_ok(prize, attack_cost))

            expected_margin = Decimal(3) * b_floor * LN2
            actual_margin = attack_cost - Decimal(3) * prize
            # attack_cost_hat rounds down to the USDC base-unit grid.
            self.assertGreaterEqual(
                actual_margin,
                expected_margin - Decimal("0.000001"),
            )
            self.assertLessEqual(actual_margin, expected_margin)


if __name__ == "__main__":
    unittest.main()
