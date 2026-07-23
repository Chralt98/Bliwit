from __future__ import annotations

import copy
import sys
import unittest
from pathlib import Path


TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS))

import runtime_profiles as PROFILES  # noqa: E402


class RuntimeProfileTests(unittest.TestCase):
    def setUp(self) -> None:
        self.document = PROFILES.load_profiles()

    def test_manifest_declares_exact_four_profile_product(self) -> None:
        self.assertEqual(
            set(self.document["profiles"]),
            {
                "bootstrap",
                "phase-four",
                "bootstrap-recovery",
                "phase-four-recovery",
            },
        )
        for name, row in self.document["profiles"].items():
            bases = {"bootstrap", "phase-four"} & set(row["cargo_features"])
            self.assertEqual(len(bases), 1, name)
            self.assertEqual("recovery" in row["cargo_features"], row["recovery"])
            self.assertEqual(row["sudo_in_metadata"], row["base"] == "bootstrap")
            self.assertEqual(
                row["multi_block_migrations"],
                "disabled" if row["recovery"] else "normal",
            )

    def test_ambiguous_or_missing_base_is_rejected(self) -> None:
        for features in (
            ["std", "substrate-wasm-builder"],
            ["std", "substrate-wasm-builder", "bootstrap", "phase-four"],
        ):
            document = copy.deepcopy(self.document)
            document["profiles"]["bootstrap"]["cargo_features"] = features
            with self.assertRaisesRegex(
                PROFILES.ProfileError, "exactly one base feature"
            ):
                PROFILES.validate_profiles(document)

    def test_recovery_feature_and_declared_mode_cannot_diverge(self) -> None:
        document = copy.deepcopy(self.document)
        document["profiles"]["bootstrap-recovery"]["recovery"] = False
        with self.assertRaisesRegex(PROFILES.ProfileError, "recovery"):
            PROFILES.validate_profiles(document)

        document = copy.deepcopy(self.document)
        document["profiles"]["phase-four-recovery"][
            "multi_block_migrations"
        ] = "enabled"
        with self.assertRaisesRegex(PROFILES.ProfileError, "must be disabled"):
            PROFILES.validate_profiles(document)

    def test_all_build_recipes_disable_cargo_defaults(self) -> None:
        for name, row in self.document["profiles"].items():
            recipe = PROFILES.cargo_recipe(row)
            self.assertIn("--no-default-features", recipe, name)
            self.assertEqual(
                recipe[recipe.index("--features") + 1],
                ",".join(row["cargo_features"]),
            )

    def test_recovery_build_info_requires_same_profile_zero_mbm_proof(self) -> None:
        _, profile = PROFILES.select_profile("phase-four-recovery")
        info = {
            "schema": PROFILES.BUILD_SCHEMA,
            "runtime_profile": "phase-four-recovery",
            "base_profile": "phase-four",
            "recovery": True,
            "cargo_default_features": False,
            "cargo_features": profile["cargo_features"],
            "multi_block_migrations": "disabled",
            "recipe": " ".join(PROFILES.cargo_recipe(profile)),
            "profile_verification": {
                "command": " ".join(
                    PROFILES.recovery_test_recipe(profile) or []
                ),
                "result": "passed",
                "test": PROFILES.RECOVERY_TEST,
            },
        }
        self.assertEqual(PROFILES.validate_build_profile(info), [])
        for mutation in (
            {"profile_verification": None},
            {"multi_block_migrations": "enabled"},
            {"cargo_default_features": True},
        ):
            invalid = {**info, **mutation}
            self.assertTrue(PROFILES.validate_build_profile(invalid), mutation)

    def test_phase_four_metadata_must_omit_sudo(self) -> None:
        _, profile = PROFILES.select_profile("phase-four")
        build = {
            "runtime_profile": "phase-four",
            "base_profile": "phase-four",
            "recovery": False,
            "cargo_features": profile["cargo_features"],
        }
        self.assertEqual(
            PROFILES.validate_metadata_profile(
                build,
                {
                    "runtime_profile": "phase-four",
                    "metadata_pallets": ["System", "Timestamp"],
                },
            ),
            [],
        )
        errors = PROFILES.validate_metadata_profile(
            build,
            {
                "runtime_profile": "phase-four",
                "metadata_pallets": ["Sudo", "System"],
            },
        )
        self.assertTrue(any("omit Sudo" in error for error in errors))

    def test_bootstrap_metadata_must_contain_sudo(self) -> None:
        errors = PROFILES.validate_metadata_profile(
            {"runtime_profile": "bootstrap"},
            {
                "runtime_profile": "bootstrap",
                "metadata_pallets": ["System"],
            },
        )
        self.assertTrue(any("contain Sudo" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
