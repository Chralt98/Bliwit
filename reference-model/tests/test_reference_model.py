from decimal import Decimal, ROUND_HALF_UP, getcontext
import json
from pathlib import Path
import unittest

from bleavit_reference_model import lmsr
from bleavit_reference_model.decision import (
    Grade,
    Outcome,
    RejectReason,
    decide,
)
from bleavit_reference_model.ledger import (
    BaselineVault,
    Branch,
    GateSide,
    GateType,
    PositionKind,
    ScalarSide,
    Vault,
    VaultState,
)
from bleavit_reference_model.lmsr import (
    PriceBoundExceeded,
    buy_delta_cost,
    vectors_v1_v6,
    worked_maker_example,
)
from bleavit_reference_model.treasury import (
    attack_cost_hat,
    baseline_commitment,
    dec_v_min,
    decision_delta,
    display_integer,
    in_cap_prize,
    nav,
    nav_floor,
    p_ref,
    pol_b,
    pol_commitment,
    security_sizing_ok,
)
from bleavit_reference_model.twap import TwapAccumulator
from bleavit_reference_model.welfare import (
    collator_d_eff,
    floor_64x64,
    full_pipeline,
    gate,
    normalization_sample,
    normalize_metric,
    percentile,
    settlement_score,
    weighted_geometric,
)


class ReferenceModelTests(unittest.TestCase):
    def test_normative_lmsr_vectors(self):
        v = vectors_v1_v6()
        self.assertTrue(v["V1"]["value"].startswith("512.494795136"))
        self.assertTrue(v["V2"]["value"].startswith("0.524979187478"))
        self.assertTrue(v["V3"]["delta"].startswith("4054.65108108"))
        self.assertTrue(v["V4"]["value"].startswith("6931.47180559"))
        self.assertTrue(v["V5"]["net_fees_only"].startswith("-3.074968"))
        self.assertEqual(
            {
                key: v["V6"][key]
                for key in ("b", "q_long", "q_short", "side", "amount")
            },
            {
                "b": "10000",
                "q_long": "480000",
                "q_short": "0",
                "side": "long",
                "amount": "1",
            },
        )
        with self.assertRaises(PriceBoundExceeded):
            buy_delta_cost(10000, 480000, 0, "long", 1)

    def test_lmsr_maker_loss_worked_example(self):
        row = worked_maker_example()
        self.assertAlmostEqual(float(row["loss"]), 180.4, places=1)
        self.assertAlmostEqual(float(row["delta"]), 6029.05, places=2)
        self.assertAlmostEqual(
            float(row["displacement_revenue"]), 3195.83, places=2
        )
        self.assertAlmostEqual(
            float(row["expected_payout"]), 3376.27, places=2
        )

    def test_twap_backward_accumulator_and_mean(self):
        twap = TwapAccumulator(Decimal("0.500"))
        first = twap.observe(10, Decimal("0.900"))
        second = twap.observe(20, Decimal("0.900"))
        self.assertEqual(first, Decimal("0.502500"))
        self.assertEqual(second, Decimal("0.505012500"))
        self.assertEqual(twap.mean(0, 20), Decimal("0.503756250"))
        self.assertEqual(twap.mean(10, 20), second)

    def test_twap_clamp_widens_and_tracks_staleness(self):
        twap = TwapAccumulator(Decimal("0.500"))
        # Exact finite decimal of 0.5*(1.005^10); ambient-precision arithmetic
        # would round it and diverge from the model (which is exact).
        expected = Decimal("0.525570066020395321298833007812500")
        self.assertEqual(twap.observe(100, Decimal("0.900")), expected)
        self.assertEqual(twap.stale_events, 1)
        fresh = TwapAccumulator(Decimal("0.500"))
        fresh.observe(50, Decimal("0.900"))
        self.assertEqual(fresh.stale_events, 0)

    def test_ledger_full_operation_families_and_void_leg_floors(self):
        vault = Vault()
        vault.split(10_000_003)
        vault.split_scalar(Branch.ACCEPT, 4_000_003)
        vault.merge_scalar(Branch.ACCEPT, 1_000_000)
        vault.split_gate(Branch.REJECT, GateType.SECURITY, 2_000_001)
        vault.transfer(1)
        vault.void()
        self.assertEqual(vault.state, VaultState.VOIDED)
        self.assertEqual(
            vault.redeem_void(
                Branch.ACCEPT,
                PositionKind.BRANCH_USDC,
                7_000_000,
            ),
            3_500_000,
        )
        self.assertEqual(
            vault.redeem_void(
                Branch.ACCEPT,
                PositionKind.LONG,
                3_000_003,
            ),
            750_000,
        )
        self.assertEqual(
            vault.redeem_void(
                Branch.REJECT,
                PositionKind.GATE_YES,
                2_000_001,
                GateType.SECURITY,
            ),
            500_000,
        )
        vault.check_conservation()

    def test_void_is_never_legal_after_scalar_settlement(self):
        vault = Vault()
        vault.split(100)
        vault.resolve(Branch.ACCEPT)
        vault.settle_scalar(Decimal("0.5"))
        with self.assertRaises(ValueError):
            vault.void()

    def test_void_claim_bound_values_pairs_before_unmatched_floors(self):
        # 03 §6.4/§6.5: both scalar and gate pairs can merge into
        # branch-USDC, after which cross-branch pairs redeem at par. Valuing
        # every live leg directly at floor(a/4), as the old oracle did,
        # therefore understates the maximal remaining liability.
        vault = Vault()
        vault.split(20_006)
        for branch in Branch:
            vault.split_scalar(branch, 10_003)
            vault.split_gate(
                branch, GateType.SURVIVAL, 10_003
            )
        vault.void()

        direct_floor_bound = 0
        for supply in vault.branches.values():
            direct_floor_bound += supply.usdc // 2
            direct_floor_bound += supply.long // 4
            direct_floor_bound += supply.short // 4
            for gate in GateType:
                direct_floor_bound += supply.gate_yes[gate] // 4
                direct_floor_bound += supply.gate_no[gate] // 4

        self.assertEqual(direct_floor_bound, 20_000)
        self.assertEqual(vault._claim_bound(), 20_006)
        self.assertGreater(vault._claim_bound(), direct_floor_bound)
        vault.check_conservation()

    def test_b5_scalar_fragmentation_vector(self):
        vault = Vault()
        vault.split(20_000)
        vault.split_scalar(Branch.ACCEPT, 20_000)
        vault.resolve(Branch.ACCEPT)
        vault.settle_scalar(Decimal("0.70005"))
        long_payout = vault.redeem_scalar(
            Branch.ACCEPT, ScalarSide.LONG, 20_000
        )
        short_a = vault.redeem_scalar(
            Branch.ACCEPT, ScalarSide.SHORT, 10_000
        )
        short_b = vault.redeem_scalar(
            Branch.ACCEPT, ScalarSide.SHORT, 10_000
        )
        self.assertEqual(long_payout, 14_001)
        self.assertEqual(short_a, 2_999)
        self.assertEqual(short_b, 2_999)
        self.assertEqual(long_payout + short_a + short_b, 19_999)
        self.assertLessEqual(vault.total_payouts, vault.collateral_in)
        vault.check_conservation()

    def test_scalar_pair_is_exact(self):
        vault = Vault()
        vault.split(20_000)
        vault.split_scalar(Branch.ACCEPT, 20_000)
        vault.resolve(Branch.ACCEPT)
        vault.settle_scalar(Decimal("0.70005"))
        self.assertEqual(
            vault.redeem_scalar_pair(Branch.ACCEPT, 20_000), 20_000
        )
        vault.check_conservation()

    def test_gate_settlement_pays_one_zero(self):
        vault = Vault()
        vault.split(1_000)
        vault.split_gate(Branch.ACCEPT, GateType.SURVIVAL, 1_000)
        vault.resolve(Branch.ACCEPT)
        vault.settle_gate(GateType.SURVIVAL, True)
        vault.settle_scalar(Decimal("0.5"))
        self.assertEqual(
            vault.redeem_gate(
                Branch.ACCEPT,
                GateType.SURVIVAL,
                GateSide.YES,
                400,
            ),
            400,
        )
        self.assertEqual(
            vault.redeem_gate(
                Branch.ACCEPT,
                GateType.SURVIVAL,
                GateSide.NO,
                400,
            ),
            0,
        )
        vault.check_conservation()

    def test_baseline_scalar_and_pair_redemptions(self):
        baseline = BaselineVault(epoch=4)
        baseline.split_baseline(20_000)
        baseline.settle_baseline(Decimal("0.70005"))
        self.assertEqual(
            baseline.redeem_baseline(ScalarSide.LONG, 10_000), 7_000
        )
        self.assertEqual(baseline.redeem_baseline_pair(10_000), 10_000)
        baseline.check_conservation()

    def test_ledger_sequence_fixture_covers_full_operation_alphabet(self):
        # 15 §4.4 / 03 §11: one generated JSON corpus drives the Python↔Rust
        # differential, including gate, Baseline, VOID, and pair paths. The
        # FRAME-only sweep surface has its own generated pallet scenarios.
        fixture = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "fixtures"
                / "vectors.json"
            ).read_text()
        )
        scenarios = fixture["ledger_sequence_scenarios"]
        self.assertEqual(fixture["schema"], "bleavit.reference-model.v4")
        self.assertEqual(len(scenarios), 64)
        self.assertEqual(
            {scenario["coverage_intent"] for scenario in scenarios},
            {
                "void-after-open",
                "void-after-resolved",
                "gate-settle-then-pair-redemption",
                "gate-false-and-unpaired-rounding",
                "baseline-pair-redemption",
                "baseline-unpaired-rounding",
                "terminal-residue-for-pallet-split",
                "illegal-terminal-interleavings",
            },
        )
        operations = {
            row["op"] for scenario in scenarios for row in scenario["ops"]
        }
        self.assertEqual(
            operations,
            {
                "split",
                "merge",
                "split_scalar",
                "merge_scalar",
                "split_gate",
                "merge_gate",
                "transfer",
                "resolve",
                "void",
                "settle_scalar",
                "settle_gate",
                "redeem",
                "redeem_void",
                "redeem_scalar",
                "redeem_scalar_pair",
                "redeem_gate",
                "split_baseline",
                "merge_baseline",
                "settle_baseline",
                "redeem_baseline",
                "redeem_baseline_pair",
            },
        )
        for scenario in scenarios:
            self.assertRegex(scenario["seed"], r"^0x[0-9A-F]{16}$")
            self.assertTrue(scenario["ops"])
            for row in scenario["ops"]:
                self.assertEqual(
                    len({"ok", "err"}.intersection(row["outcome"])), 1
                )

    def test_ledger_sequence_fixture_has_errors_and_flooring_vectors(self):
        fixture = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "fixtures"
                / "vectors.json"
            ).read_text()
        )
        rows = [
            row
            for scenario in fixture["ledger_sequence_scenarios"]
            for row in scenario["ops"]
        ]
        self.assertTrue(any("err" in row["outcome"] for row in rows))
        error_classes = {
            row["outcome"]["err"]
            for row in fixture["ledger_error_scenarios"]
        }
        self.assertEqual(
            error_classes,
            {
                "UnknownVault",
                "UnknownBaselineVault",
                "WrongVaultState",
                "AmountTooSmall",
                "ArithmeticOverflow",
                "InsufficientPosition",
                "PositionCapExceeded",
                "InvalidScore",
                "GateAlreadySettled",
                "GateNotSettled",
                # WrongBranch deliberately absent: dead core variant, no honest
                # differential witness exists (SQ-170).
            },
        )
        successful = [row for row in rows if "ok" in row["outcome"]]
        self.assertTrue(
            any(
                row["op"] == "redeem_void"
                and row["args"]["amount"] % 4 != 0
                and row["outcome"]["ok"]["payout"]
                == row["args"]["amount"] // 4
                for row in successful
            )
        )
        for pair_op in ("redeem_scalar_pair", "redeem_baseline_pair"):
            self.assertTrue(
                any(
                    row["op"] == pair_op
                    and row["outcome"]["ok"]["payout"]
                    == row["args"]["amount"]
                    for row in successful
                )
            )

    def test_ledger_fixture_covers_score_endpoints_and_rounding_boundary(self):
        fixture = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "fixtures"
                / "vectors.json"
            ).read_text()
        )
        scores = {
            row["score"] for row in fixture["ledger_score_scenarios"]
        }
        self.assertTrue(
            {
                0,
                1_000_000_000,
                700_049_999,
                700_050_000,
                700_050_001,
            }.issubset(scores)
        )

    def test_sweep_fixture_is_python_derived_and_batched(self):
        fixture = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "fixtures"
                / "vectors.json"
            ).read_text()
        )
        scenarios = fixture["ledger_sweep_scenarios"]
        self.assertEqual(
            {scenario["family"] for scenario in scenarios},
            {"proposal", "baseline"},
        )
        for scenario in scenarios:
            self.assertGreater(scenario["expected_residue"], 0)
            self.assertGreater(
                scenario["expected_entries"], scenario["reap_batch"]
            )
            self.assertEqual(
                scenario["expected_batches"],
                (
                    scenario["expected_entries"]
                    + scenario["reap_batch"]
                    - 1
                )
                // scenario["reap_batch"],
            )
            self.assertEqual(
                sum(scenario["expected_refunds"].values()),
                scenario["expected_entries"] * 100_000,
            )

    def test_decide_reason_code_matrix(self):
        cases = [
            (
                {"preimage_ok": False},
                RejectReason.CONSTITUTION_VIOLATION,
            ),
            (
                {"resource_locks_held": False},
                RejectReason.RESOURCE_CONFLICT,
            ),
            ({"process_hold": True}, RejectReason.PROCESS_HOLD),
            (
                {
                    "requires_gate_markets": True,
                    "gate_book_valid": False,
                },
                RejectReason.NOT_DECISION_GRADE,
            ),
            (
                {
                    "requires_gate_markets": True,
                    "p_adopt": {"Survival": Decimal("0.06")},
                },
                RejectReason.GATE_VETO_SURVIVAL,
            ),
            (
                {
                    "requires_gate_markets": True,
                    "p_adopt": {"Security": Decimal("0.06")},
                },
                RejectReason.GATE_VETO_SECURITY,
            ),
            (
                {"welfare_grade": Grade.INVALID},
                RejectReason.NOT_DECISION_GRADE,
            ),
            (
                {"accept_full": Decimal("0.54")},
                RejectReason.HURDLE_NOT_MET,
            ),
            ({"converged": False}, RejectReason.CONVERGENCE_FAILED),
            (
                {
                    "accept_trailing": Decimal("0.52"),
                    "extended": True,
                },
                RejectReason.SECOND_EXTENSION_FAILED,
            ),
            (
                {"envelope_value": Decimal("1")},
                RejectReason.SECURITY_SIZING,
            ),
            (
                {
                    "proposal_class": "Code",
                    "attestation_ok": False,
                },
                RejectReason.ATTESTATION_MISSING,
            ),
            ({"queue_time_ok": False}, RejectReason.RATE_LIMITED),
        ]
        base = {
            "accept_full": Decimal("0.56"),
            "reject_full_effective": Decimal("0.50"),
            "delta": Decimal("0.05"),
        }
        observed = set()
        for overrides, expected in cases:
            result = decide(**(base | overrides))
            self.assertEqual(
                (result.outcome, result.reason),
                (Outcome.REJECT, expected),
            )
            observed.add(expected)
        decide_time = {
            RejectReason.NOT_DECISION_GRADE,
            RejectReason.GATE_VETO_SURVIVAL,
            RejectReason.GATE_VETO_SECURITY,
            RejectReason.HURDLE_NOT_MET,
            RejectReason.CONVERGENCE_FAILED,
            RejectReason.SECOND_EXTENSION_FAILED,
            RejectReason.PROCESS_HOLD,
            RejectReason.CONSTITUTION_VIOLATION,
            RejectReason.RESOURCE_CONFLICT,
            RejectReason.RATE_LIMITED,
            RejectReason.SECURITY_SIZING,
            RejectReason.ATTESTATION_MISSING,
        }
        self.assertEqual(observed, decide_time)
        self.assertEqual(len(RejectReason), 16)

    def test_gate_veto_precedes_welfare_grade_failure(self):
        result = decide(
            Decimal("0.56"),
            Decimal("0.50"),
            Decimal("0.05"),
            requires_gate_markets=True,
            p_adopt={"Survival": Decimal("0.06")},
            welfare_grade=Grade.INVALID,
        )
        self.assertEqual(result.reason, RejectReason.GATE_VETO_SURVIVAL)

    def test_first_insufficiency_extends_second_rejects(self):
        first = decide(
            Decimal("0.56"),
            Decimal("0.50"),
            Decimal("0.05"),
            welfare_grade=Grade.INSUFFICIENT,
        )
        second = decide(
            Decimal("0.56"),
            Decimal("0.50"),
            Decimal("0.05"),
            welfare_grade=Grade.INSUFFICIENT,
            extended=True,
        )
        self.assertEqual(first.outcome, Outcome.EXTEND)
        self.assertEqual(
            (second.outcome, second.reason),
            (Outcome.REJECT, RejectReason.NOT_DECISION_GRADE),
        )

    def test_decision_window_match_and_adoption(self):
        split = decide(
            Decimal("0.56"),
            Decimal("0.50"),
            Decimal("0.05"),
            accept_trailing=Decimal("0.52"),
        )
        self.assertEqual(split.outcome, Outcome.EXTEND)
        adopted = decide(
            Decimal("0.56"),
            Decimal("0.50"),
            Decimal("0.05"),
        )
        self.assertEqual(adopted.outcome, Outcome.ADOPT)

    def test_welfare_rounding_and_full_pipeline(self):
        product = Decimal("0.333333333333333333") * Decimal(
            "0.777777777777777777"
        )
        self.assertLessEqual(floor_64x64(product), product)
        geo = weighted_geometric(
            {"b": Decimal("0.7"), "a": Decimal("0.8")},
            {"b": Decimal("0.4"), "a": Decimal("0.6")},
        )
        self.assertTrue(Decimal(0) <= geo <= Decimal(1))
        row = full_pipeline(
            u=Decimal("0.97"),
            f=Decimal("0.96"),
            hhi=Decimal("0.335"),
            phase=2,
            c_onchain={"C01": Decimal("0.94"), "C02": Decimal("0.91")},
            c_attested={"C03": Decimal("0.90")},
            c_weights={
                "C01": Decimal("0.50"),
                "C02": Decimal("0.30"),
                "C03": Decimal("0.20"),
            },
            incident=Decimal("0.98"),
            p_components={"P01": Decimal("0.8"), "P02": Decimal("0.7")},
            p_weights={"P01": Decimal("0.6"), "P02": Decimal("0.4")},
            a_components={"A01": Decimal("0.9"), "A02": Decimal("0.6")},
            a_weights={"A01": Decimal("0.4"), "A02": Decimal("0.6")},
            c_daily={"C01": Decimal("0.93"), "C02": Decimal("0.89")},
        )
        self.assertEqual(row["S"], Decimal("0.831250000"))
        self.assertTrue(Decimal(0) <= row["W"] <= Decimal(1))
        self.assertNotEqual(row["C"], row["C_daily"])
        self.assertEqual(
            settlement_score(row["W"], row["W"]), row["W"]
        )

    def test_welfare_gate_and_settlement_vectors(self):
        self.assertEqual(
            gate(Decimal("0.85"), Decimal("0.85"), Decimal("0.95")),
            Decimal("0E-9"),
        )
        self.assertEqual(
            gate(Decimal("0.95"), Decimal("0.85"), Decimal("0.95")),
            Decimal(1),
        )
        self.assertEqual(
            settlement_score(Decimal("0.8"), Decimal("0.8")),
            Decimal("0.800000000"),
        )
        self.assertEqual(
            settlement_score(Decimal("0.64"), Decimal("0.25")),
            Decimal("0.400000000"),
        )
        self.assertEqual(collator_d_eff(Decimal("0.2"), 2), Decimal("1.000000000"))
        self.assertEqual(
            collator_d_eff(Decimal("0.335"), 2), Decimal("0.831250000")
        )

    def test_normalization_cold_start_and_percentiles(self):
        sample = [Decimal(i) for i in range(12)]
        self.assertEqual(percentile(sample, Decimal("0.05")), Decimal("0.55"))
        self.assertEqual(
            percentile(sample, Decimal("0.95")), Decimal("10.45")
        )
        trailing = normalization_sample(sample, [Decimal(12), Decimal(13)])
        self.assertEqual(trailing, [Decimal(i) for i in range(2, 14)])
        self.assertEqual(
            normalize_metric(Decimal("6"), sample, []),
            Decimal("0.550505050"),
        )
        logged = normalize_metric(Decimal("6"), sample, [], log1p=True)
        self.assertTrue(Decimal(0) < logged < Decimal(1))

    def test_nav_and_security_rounding_directions(self):
        healthy = nav(
            Decimal("100"),
            undisbursed_reversions=Decimal("5"),
            obligations=Decimal("25"),
        )
        impaired = nav(
            Decimal("100"),
            obligations=Decimal("25"),
            reserve_impaired=True,
        )
        self.assertEqual(healthy.nav, Decimal("80"))
        self.assertEqual(healthy.spendable_nav, Decimal("80"))
        self.assertEqual(impaired.nav, Decimal("75"))
        self.assertEqual(impaired.spendable_nav, Decimal("0"))
        self.assertEqual(
            in_cap_prize("Param", envelope=Decimal("1.0000001")),
            Decimal("1.000001"),
        )
        self.assertEqual(
            attack_cost_hat(Decimal("2.000001")),
            Decimal("3.000001"),
        )
        self.assertTrue(
            security_sizing_ok(Decimal("1"), Decimal("3"))
        )
        self.assertFalse(
            security_sizing_ok(Decimal("1.000001"), Decimal("3"))
        )

    def test_code_meta_nav_floor_is_scoped_to_upgrade_payloads(self):
        inputs = {
            "ask": Decimal("100"),
            "envelope": Decimal("200"),
            "spendable_nav": Decimal("10000"),
        }
        for proposal_class in ("Code", "Meta"):
            with self.subTest(proposal_class=proposal_class):
                self.assertEqual(
                    in_cap_prize(proposal_class, **inputs),
                    Decimal("500.000000"),
                )
                self.assertEqual(
                    in_cap_prize(
                        proposal_class,
                        **inputs,
                        upgrade_payload=False,
                    ),
                    Decimal("200.000000"),
                )

    def test_pol_commitments_and_nav_floor_worked_numbers(self):
        commitment_cases = [
            (("Param", False), 13_863),
            (("Treasury", False), 34_657),
            (("Treasury", True), 55_452),
            (("Code", False), 103_972),
            (("Meta", False), 159_424),
        ]
        for (proposal_class, large), displayed in commitment_cases:
            model = display_integer(
                pol_commitment(
                    proposal_class, large_treasury=large
                )
            )
            self.assertEqual(model, displayed)
        self.assertEqual(display_integer(baseline_commitment()), 17_329)

        floor_cases = [
            (("Param", 1, False), 1_848_400),
            (("Treasury", 1, False), 4_620_981),
            (("Treasury", 1, True), 7_393_600),
            (("Code", 1, False), 13_862_944),
            (("Meta", 1, False), 21_256_533),
            (("Param", 5, False), 9_241_960),
            (("Meta", 5, False), 106_282_533),
        ]
        for (proposal_class, slots, large), displayed in floor_cases:
            model = nav_floor(
                proposal_class,
                slots=slots,
                large_treasury=large,
            )
            self.assertLessEqual(abs(model - displayed), Decimal(10))

    def test_security_worked_numbers_and_scaling(self):
        nav_value = Decimal("13862944")
        prize = in_cap_prize("Code", spendable_nav=nav_value)
        volume = dec_v_min("Code", prize)
        depth = Decimal(2) * Decimal("60000") * lmsr.LN2
        liquidity = depth + volume
        attack = attack_cost_hat(liquidity)
        displayed = [
            (prize, 693_147),
            (volume, 1_386_294),
            (depth, 83_178),
            (liquidity, 1_469_472),
            (attack, 2_204_208),
            (Decimal(3) * prize, 2_079_441),
            (attack / Decimal(3), 734_736),
        ]
        for model, expected in displayed:
            self.assertLessEqual(
                abs(
                    int(
                        model.to_integral_value(
                            rounding=ROUND_HALF_UP
                        )
                    )
                    - expected
                ),
                10,
            )
        treasury_liquidity = (
            Decimal(2) * Decimal("25000") * lmsr.LN2
            + Decimal("400000")
        )
        self.assertLessEqual(
            abs(display_integer(attack_cost_hat(treasury_liquidity)) - 651_986),
            10,
        )
        p_refs = {
            "Param": 56_931,  # 08 §5.4 reconciled (SQ-29): pair depth 13,863, not 27,726
            "Treasury": 142_329,
            "Code": 341_589,
            "Meta": 669_315,
        }
        for proposal_class, expected in p_refs.items():
            self.assertEqual(display_integer(p_ref(proposal_class)), expected)
        self.assertEqual(
            dec_v_min("Treasury", Decimal("200000")),
            Decimal("400000"),
        )
        self.assertGreater(pol_b("Code", Decimal("700000")), Decimal("60000"))
        self.assertLessEqual(
            decision_delta("Meta", Decimal("999999999")),
            Decimal("0.10"),
        )

    def test_imports_do_not_mutate_global_decimal_context(self):
        # The package was imported at module load; if any module had set
        # getcontext().prec at import time (the audited defect), the ambient
        # context would no longer be the Python default of 28. Reloading is
        # deliberately avoided: it would recreate exception/enum classes and
        # break identity comparisons for the rest of the suite.
        self.assertEqual(getcontext().prec, 28)


if __name__ == "__main__":
    unittest.main()
