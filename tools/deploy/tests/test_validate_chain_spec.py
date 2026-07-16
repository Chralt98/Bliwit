import copy
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "validate-chain-spec.py"
MODULE_SPEC = importlib.util.spec_from_file_location("validate_chain_spec", SCRIPT)
if MODULE_SPEC is None or MODULE_SPEC.loader is None:
    raise RuntimeError("validator module must be importable")
validator = importlib.util.module_from_spec(MODULE_SPEC)
MODULE_SPEC.loader.exec_module(validator)


PEER_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def peer_id(index: int) -> str:
    return f"12D3KooW{PEER_ALPHABET[index] * 44}"


def endpoint(index: int, port: int, peer_index: int | None = None) -> str:
    peer = peer_id(index if peer_index is None else peer_index)
    return f"/dns/boot{index}.example.org/tcp/{port}/wss/p2p/{peer}"


class ValidateBootnodesTests(unittest.TestCase):
    def validate(
        self,
        count: int,
        operator_count: int,
        port_443_count: int,
        names: list[str] | None = None,
        peer_indexes: list[int] | None = None,
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            script = repo / "tools" / "deploy" / "validate-chain-spec.py"
            manifest_path = repo / "deploy" / "chain-specs" / "bootnodes.paseo.json"
            manifest_path.parent.mkdir(parents=True)
            endpoints = [
                endpoint(
                    i,
                    443 if i < port_443_count else 30333,
                    peer_indexes[i] if peer_indexes else None,
                )
                for i in range(count)
            ]
            operators = [
                {
                    "name": names[operator] if names else f"operator-{operator}",
                    "multiaddrs": [
                        address
                        for index, address in enumerate(endpoints)
                        if index % operator_count == operator
                    ],
                }
                for operator in range(operator_count)
            ]
            manifest_path.write_text(
                json.dumps({"network": "paseo", "operators": operators}),
                encoding="utf-8",
            )
            original_file = validator.__file__
            validator.__file__ = str(script)
            try:
                failures: list[str] = []
                validator.validate_bootnodes({"bootNodes": endpoints}, "paseo", failures)
                return failures
            finally:
                validator.__file__ = original_file

    def test_exact_thresholds_pass(self) -> None:
        self.assertEqual(self.validate(8, 4, 2), [])

    def test_each_threshold_is_release_blocking(self) -> None:
        for case in ((7, 4, 2), (8, 3, 2), (8, 4, 1)):
            with self.subTest(case=case):
                self.assertTrue(self.validate(*case))

    def test_duplicate_peer_ids_fail_distinct_wss_peer_threshold(self) -> None:
        failures = self.validate(8, 4, 2, peer_indexes=[0] * 8)

        self.assertTrue(
            any(
                "02 §10" in failure
                and "8 distinct" in failure
                and "peer" in failure.casefold()
                for failure in failures
            ),
            failures,
        )

    def test_duplicate_peer_ids_fail_distinct_port_443_threshold(self) -> None:
        # Nine endpoints preserve eight distinct peer IDs overall while the two
        # port-443 endpoints deliberately share one peer ID.
        failures = self.validate(9, 4, 2, peer_indexes=[0, 0, 1, 2, 3, 4, 5, 6, 7])

        self.assertTrue(
            any(
                "02 §10" in failure
                and "443" in failure
                and "distinct" in failure.casefold()
                and "peer" in failure.casefold()
                for failure in failures
            ),
            failures,
        )

    def test_malformed_strings_do_not_count_as_wss_multiaddrs(self) -> None:
        self.assertIsNone(validator.wss_port("/tcp/443/wss"))
        self.assertIsNone(validator.wss_port("/dns/example.org/tcp/443/wss"))
        self.assertIsNone(validator.wss_port("/dns/example.org/tcp/70000/wss/p2p/12D3KooW11111111111111111111111111111111"))

    def test_operator_names_are_normalized_for_independence(self) -> None:
        failures = self.validate(
            8,
            4,
            2,
            ["Operator-A", " operator-a ", "Operator-C", "Operator-D"],
        )
        self.assertTrue(any("duplicated" in failure for failure in failures))


class ValidateGenesisTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        generated_spec = (
            SCRIPT.parents[2]
            / "deploy"
            / "chain-specs"
            / "out"
            / "bleavit-dev.json"
        )
        cls.valid_spec = json.loads(generated_spec.read_text(encoding="utf-8"))

    def validate(self, spec: dict[str, object], profile: str = "dev") -> list[str]:
        failures: list[str] = []
        validator.validate_genesis(spec, profile, failures)
        return failures

    def test_generated_dev_genesis_passes(self) -> None:
        self.assertEqual(self.validate(copy.deepcopy(self.valid_spec)), [])

    def test_missing_patch_on_paseo_fails(self) -> None:
        failures = self.validate({"genesis": {"runtimeGenesis": {}}}, "paseo")

        self.assertTrue(
            any("patch" in failure.casefold() for failure in failures), failures
        )

    def test_wrong_total_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        balances = spec["genesis"]["runtimeGenesis"]["patch"]["balances"]["balances"]
        balances[0][1] += 1

        failures = self.validate(spec)

        self.assertTrue(
            any(
                "1,000,000,000" in failure or "1000000000" in failure
                for failure in failures
            ),
            failures,
        )

    def test_protocol_pot_on_non_derived_account_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        patch = spec["genesis"]["runtimeGenesis"]["patch"]
        treasury = next(row for row in patch["balances"]["balances"] if row[1] == 300_000_000 * 10**12)
        treasury[0] = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"

        failures = self.validate(spec)

        self.assertTrue(
            any(
                "pot" in failure.casefold()
                or "treasury" in failure.casefold()
                or "derived" in failure.casefold()
                for failure in failures
            ),
            failures,
        )

    def test_protocol_pot_accepts_any_valid_encoding_of_the_derived_account(self) -> None:
        # The runtime's genesis serializer emits the default (42) display
        # prefix for the SAME 32-byte account; the validator must accept it —
        # the account bytes and amount are the invariant, the display prefix is
        # enforced via properties.ss58Format instead.
        spec = copy.deepcopy(self.valid_spec)
        patch = spec["genesis"]["runtimeGenesis"]["patch"]
        treasury = next(row for row in patch["balances"]["balances"] if row[1] == 300_000_000 * 10**12)
        treasury[0] = "5EYCAe5fvRYqBSrUR8qygZTQbb9EQbCdU4QmcNJQm8R66Eht"

        failures = self.validate(spec)

        self.assertEqual(failures, [])

    def test_todo_leakage_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        patch = spec["genesis"]["runtimeGenesis"]["patch"]
        patch["constitution"]["releaseChannel"] = ["TODO: set before release"]

        failures = self.validate(spec)

        self.assertTrue(
            any("TODO" in failure for failure in failures), failures
        )

    def test_missing_team_vesting_row_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        patch = spec["genesis"]["runtimeGenesis"]["patch"]
        patch["vesting"]["vesting"].pop()

        failures = self.validate(spec)

        self.assertTrue(
            any("vesting" in failure.casefold() for failure in failures), failures
        )


if __name__ == "__main__":
    unittest.main()
