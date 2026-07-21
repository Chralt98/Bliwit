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


GENERATED_DEV_SPEC = (
    SCRIPT.parents[2] / "deploy" / "chain-specs" / "out" / "bleavit-dev.json"
)
ALLOCATIONS_TEMPLATE = (
    SCRIPT.parents[2] / "deploy" / "genesis" / "allocations.template.json"
)

# Well-known sr25519 dev accounts (subkey //Alice … //Dave), checksummed ss58.
ALICE = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
BOB = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
CHARLIE = "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y"
DAVE = "5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy"
# //Alice's raw sr25519 public key: the genesis patch serialises the treasury's
# Coretime-side renewal account ([u8; 32]) as a byte array, not as SS58.
ALICE_PUBLIC = list(
    bytes.fromhex("d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d")
)
VIT = 10**12


def synthetic_dev_spec() -> dict[str, object]:
    """A self-contained spec mirroring the runtime's development preset.

    Built from the validator's own constants so the suite runs on a fresh
    checkout — the generated artifact under deploy/chain-specs/out/ is
    gitignored and only exists after tools/deploy/generate-chain-specs.sh
    (which validates it directly; see the skip-gated integration test).
    """
    balances = [
        [ALICE, 75_000_000 * VIT],
        [BOB, 75_000_000 * VIT],
        [CHARLIE, 100_000_000 * VIT],
        [DAVE, 100_000_000 * VIT],
    ] + [
        [address, amount]
        for address, amount in validator.PROTOCOL_POTS.values()
    ]
    schedule = list(validator.TEAM_VESTING_SCHEDULE)
    foreign_asset_accounts = [
        [
            copy.deepcopy(validator.USDC_LOCATION),
            address,
            validator.USDC_MIN_BALANCE,
        ]
        for address in validator.USDC_ENDOWED_ACCOUNTS.values()
    ]
    return {
        "para_id": 4242,
        "genesis": {
            "runtimeGenesis": {
                "patch": {
                    "balances": {"balances": balances},
                    "vesting": {"vesting": [[CHARLIE, *schedule], [DAVE, *schedule]]},
                    "foreignAssets": {
                        # Owner is the derived ledger sovereign, never a user
                        # key: `pallet-assets` genesis installs the owner as
                        # issuer, admin and freezer too, so this row carries
                        # mint/burn/freeze authority over protocol collateral.
                        "assets": [
                            [
                                copy.deepcopy(location),
                                validator.USDC_ENDOWED_ACCOUNTS["ledger_sovereign"],
                                True,
                                minimum,
                            ]
                            for location, minimum, _ in validator.DECLARED_ASSETS.values()
                        ],
                        "accounts": foreign_asset_accounts,
                    },
                    "parachainInfo": {"parachainId": 4242},
                }
            }
        },
    }


def synthetic_production_spec() -> dict[str, object]:
    """A production-profile spec: the same allocation shape, plus the 09 §4 seats.

    Both Coretime ops accounts are outputs of the Phase-2/3 ceremony, so a
    paseo/polkadot spec must seat them explicitly rather than inherit the
    runtime's fail-closed `None` default.
    """
    spec = synthetic_dev_spec()
    spec["genesis"]["runtimeGenesis"]["patch"]["futarchyTreasury"] = {
        "coretimeQuoteAuthority": ALICE,
        "coretimeRenewalAccount": ALICE_PUBLIC,
    }
    return spec


class ValidateGenesisTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.valid_spec = synthetic_dev_spec()
        cls.valid_production_spec = synthetic_production_spec()

    def validate(self, spec: dict[str, object], profile: str = "dev") -> list[str]:
        failures: list[str] = []
        validator.validate_genesis(spec, profile, failures)
        return failures

    def test_synthetic_dev_genesis_passes(self) -> None:
        self.assertEqual(self.validate(copy.deepcopy(self.valid_spec)), [])

    def test_pallet_account_constants_match_account_id_conversion(self) -> None:
        ledger_sub_accounts = {
            "ledger_sovereign": None,
            "ledger_insurance": b"INSURANC",
            "ledger_book": b"BOOK____",
            "ledger_pol": b"POL_____",
            "ledger_pol_baseline": b"POL_BASE",
            "ledger_fees": b"FEES____",
            "ledger_treasury": b"TREASRY_",
        }
        treasury_sub_accounts = {
            "treasury_main": None,
            "treasury_keeper": b"KEEPER__",
            "treasury_oracle": b"ORACLE__",
        }
        for label, sub in ledger_sub_accounts.items():
            with self.subTest(label=label):
                self.assertEqual(
                    validator.ss58_account_id(
                        validator.USDC_ENDOWED_ACCOUNTS[label]
                    ),
                    validator.pallet_sub_account(b"bl/ledgr", sub),
                )
        for label, sub in treasury_sub_accounts.items():
            with self.subTest(label=label):
                self.assertEqual(
                    validator.ss58_account_id(
                        validator.USDC_ENDOWED_ACCOUNTS[label]
                    ),
                    validator.pallet_sub_account(b"bl/trsry", sub),
                )

        protocol_pot_controls = {
            "treasury MAIN": None,
            "community": b"communty",
            "incentives": b"incentiv",
        }
        for label, sub in protocol_pot_controls.items():
            with self.subTest(control=label):
                address, _amount = validator.PROTOCOL_POTS[label]
                self.assertEqual(
                    validator.ss58_account_id(address),
                    validator.pallet_sub_account(b"bl/trsry", sub),
                )

    def test_missing_foreign_assets_section_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        del spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]

        failures = self.validate(spec)

        self.assertTrue(
            any("foreignAssets section" in failure for failure in failures), failures
        )

    def test_missing_foreign_assets_accounts_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        del spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]

        failures = self.validate(spec)

        self.assertTrue(
            any("foreignAssets.accounts must be an array" in failure for failure in failures),
            failures,
        )

    def test_missing_required_usdc_endowment_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        accounts = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]
        accounts.pop()

        failures = self.validate(spec)

        self.assertTrue(
            any("endowment is absent" in failure for failure in failures), failures
        )

    def test_usdc_endowment_below_minimum_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        accounts = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]
        accounts[0][2] = validator.USDC_MIN_BALANCE - 1

        failures = self.validate(spec)

        self.assertTrue(
            any("must receive exactly" in failure for failure in failures), failures
        )

    def test_usdc_endowment_above_minimum_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        accounts = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]
        accounts[0][2] = validator.USDC_MIN_BALANCE + 1

        failures = self.validate(spec)

        self.assertTrue(
            any("must receive exactly" in failure for failure in failures), failures
        )

    def test_non_required_usdc_endowment_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        accounts = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]
        accounts.append(
            [copy.deepcopy(validator.USDC_LOCATION), ALICE, validator.USDC_MIN_BALANCE]
        )

        failures = self.validate(spec)

        self.assertTrue(
            any("non-required account" in failure for failure in failures), failures
        )

    def test_duplicate_usdc_endowment_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        accounts = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]
        accounts.append(copy.deepcopy(accounts[0]))

        failures = self.validate(spec)

        self.assertTrue(
            any("duplicate row" in failure for failure in failures), failures
        )

    def test_structurally_wrong_usdc_location_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        accounts = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]
        accounts[0][0] = {"parents": 0, "interior": "Here"}

        failures = self.validate(spec)

        self.assertTrue(
            any("wrong asset Location" in failure for failure in failures), failures
        )

    def test_malformed_foreign_asset_account_row_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        accounts = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["accounts"]
        accounts.append([copy.deepcopy(validator.USDC_LOCATION), ALICE])

        failures = self.validate(spec)

        self.assertTrue(
            any("must be [Location object" in failure for failure in failures), failures
        )

    def test_foreign_assets_must_declare_usdc(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["assets"] = []

        failures = self.validate(spec)

        self.assertTrue(
            any("assets must declare" in failure for failure in failures), failures
        )

    def test_declared_asset_owner_must_be_the_ledger_sovereign(self) -> None:
        # `pallet-assets` genesis installs the owner as issuer, admin and
        # freezer, so an external owner would hold mint/burn/freeze authority
        # over all protocol collateral. Each declared asset is checked.
        for index, label in enumerate(validator.DECLARED_ASSETS):
            with self.subTest(asset=label):
                spec = copy.deepcopy(self.valid_spec)
                assets = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"][
                    "assets"
                ]
                assets[index][1] = ALICE

                failures = self.validate(spec)

                self.assertTrue(
                    any(
                        f"the {label} asset owner must be the derived ledger sovereign"
                        in failure
                        for failure in failures
                    ),
                    failures,
                )

    def test_declared_asset_must_be_sufficient(self) -> None:
        # 03 §7 R-4's opening clause for USDC; clearing it also removes what
        # keeps the endowed protocol accounts alive without provider refs.
        for index, label in enumerate(validator.DECLARED_ASSETS):
            with self.subTest(asset=label):
                spec = copy.deepcopy(self.valid_spec)
                assets = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"][
                    "assets"
                ]
                assets[index][2] = False

                failures = self.validate(spec)

                self.assertTrue(
                    any(
                        f"the {label} asset must be declared sufficient" in failure
                        for failure in failures
                    ),
                    failures,
                )

    def test_declared_asset_min_balance_must_match(self) -> None:
        for index, (label, (_, minimum, _)) in enumerate(
            validator.DECLARED_ASSETS.items()
        ):
            with self.subTest(asset=label):
                spec = copy.deepcopy(self.valid_spec)
                assets = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"][
                    "assets"
                ]
                assets[index][3] = minimum + 1

                failures = self.validate(spec)

                self.assertTrue(
                    any(
                        f"the {label} asset must declare min_balance" in failure
                        for failure in failures
                    ),
                    failures,
                )

    def test_declared_asset_may_not_be_declared_twice(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        assets = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["assets"]
        assets.append(copy.deepcopy(assets[0]))

        failures = self.validate(spec)

        self.assertTrue(
            any("Location more than once" in failure for failure in failures), failures
        )

    def test_foreign_assets_must_declare_dot(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        assets = spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["assets"]
        spec["genesis"]["runtimeGenesis"]["patch"]["foreignAssets"]["assets"] = [
            row for row in assets if row[0] != validator.DOT_LOCATION
        ]

        failures = self.validate(spec)

        self.assertTrue(
            any(
                "must declare the canonical DOT Location" in failure
                for failure in failures
            ),
            failures,
        )

    def test_usdc_endowments_do_not_leak_into_native_balances(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        patch = spec["genesis"]["runtimeGenesis"]["patch"]
        required_accounts = {
            validator.ss58_account_id(address)
            for address in validator.USDC_ENDOWED_ACCOUNTS.values()
        }
        self.assertFalse(
            any(
                validator.ss58_account_id(address) in required_accounts
                and amount == validator.USDC_MIN_BALANCE
                for address, amount in patch["balances"]["balances"]
            )
        )

    def test_usdc_protocol_account_in_native_balances_fails(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        balances = spec["genesis"]["runtimeGenesis"]["patch"]["balances"]["balances"]
        balances[0][1] -= 1
        balances.append(
            [validator.USDC_ENDOWED_ACCOUNTS["ledger_sovereign"], 1]
        )

        failures = self.validate(spec)

        self.assertTrue(
            any("native balances.balances" in failure for failure in failures), failures
        )

    def test_allocations_template_usdc_section_matches_validator(self) -> None:
        template = json.loads(ALLOCATIONS_TEMPLATE.read_text(encoding="utf-8"))
        section = template["usdc_genesis_endowments"]
        self.assertFalse(validator.contains_todo(section))

        rows = section["accounts"]
        self.assertEqual(len(rows), len(validator.USDC_ENDOWED_ACCOUNTS))
        by_label = {row["label"]: row for row in rows}
        self.assertEqual(set(by_label), set(validator.USDC_ENDOWED_ACCOUNTS))
        for label, address in validator.USDC_ENDOWED_ACCOUNTS.items():
            with self.subTest(label=label):
                self.assertEqual(by_label[label]["account"], address)
                self.assertEqual(
                    int(by_label[label]["amount_base_units"]),
                    validator.USDC_MIN_BALANCE,
                )
                self.assertIn("PalletId", by_label[label]["_derivation"])

    @unittest.skipUnless(
        GENERATED_DEV_SPEC.exists(),
        "generated artifact absent (run tools/deploy/generate-chain-specs.sh)",
    )
    def test_generated_dev_genesis_passes(self) -> None:
        spec = json.loads(GENERATED_DEV_SPEC.read_text(encoding="utf-8"))
        self.assertEqual(self.validate(spec), [])

    def test_genesis_para_id_must_match_top_level(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        spec["genesis"]["runtimeGenesis"]["patch"]["parachainInfo"]["parachainId"] = 2000

        failures = self.validate(spec)

        self.assertTrue(
            any("parachainId" in failure for failure in failures), failures
        )

    def test_genesis_para_id_must_be_present(self) -> None:
        spec = copy.deepcopy(self.valid_spec)
        del spec["genesis"]["runtimeGenesis"]["patch"]["parachainInfo"]

        failures = self.validate(spec)

        self.assertTrue(
            any("parachainId" in failure for failure in failures), failures
        )

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
        patch["constitution"] = {"releaseChannel": ["TODO: set before release"]}

        failures = self.validate(spec)

        self.assertTrue(
            any("TODO" in failure for failure in failures), failures
        )

    def test_production_genesis_with_both_coretime_seats_passes(self) -> None:
        for profile in ("paseo", "polkadot"):
            with self.subTest(profile=profile):
                spec = copy.deepcopy(self.valid_production_spec)

                self.assertEqual(self.validate(spec, profile), [])

    def test_missing_coretime_seats_fail_production(self) -> None:
        spec = copy.deepcopy(self.valid_production_spec)
        del spec["genesis"]["runtimeGenesis"]["patch"]["futarchyTreasury"]

        failures = self.validate(spec, "polkadot")

        for key in ("coretimeQuoteAuthority", "coretimeRenewalAccount"):
            with self.subTest(key=key):
                self.assertTrue(
                    any("09 §4" in failure and key in failure for failure in failures),
                    failures,
                )

    def test_each_coretime_seat_is_individually_required_on_production(self) -> None:
        for key in ("coretimeQuoteAuthority", "coretimeRenewalAccount"):
            with self.subTest(key=key):
                spec = copy.deepcopy(self.valid_production_spec)
                spec["genesis"]["runtimeGenesis"]["patch"]["futarchyTreasury"][key] = None

                failures = self.validate(spec, "paseo")

                self.assertTrue(
                    any("09 §4" in failure and key in failure for failure in failures),
                    failures,
                )

    def test_todo_coretime_seat_fails_production(self) -> None:
        for key in ("coretimeQuoteAuthority", "coretimeRenewalAccount"):
            with self.subTest(key=key):
                spec = copy.deepcopy(self.valid_production_spec)
                spec["genesis"]["runtimeGenesis"]["patch"]["futarchyTreasury"][key] = "TODO"

                failures = self.validate(spec, "polkadot")

                self.assertTrue(
                    any(
                        "09 §4" in failure and key in failure and "TODO" in failure
                        for failure in failures
                    ),
                    failures,
                )

    def test_malformed_coretime_seat_fails_production(self) -> None:
        """A seated-but-malformed ceremony output must not clear the release gate.

        Presence alone is not enough: the runtime decodes the quote authority as
        an SS58 `AccountId` and the renewal account as `[u8; 32]`, so a wrong
        shape would otherwise pass validation and fail only at genesis build.
        """
        cases = [
            ("coretimeQuoteAuthority", "not an account"),
            ("coretimeQuoteAuthority", ALICE[:-1] + ("X" if ALICE[-1] != "X" else "Y")),
            ("coretimeQuoteAuthority", list(ALICE_PUBLIC)),
            ("coretimeRenewalAccount", [1, 2, 3]),
            ("coretimeRenewalAccount", ALICE),
            ("coretimeRenewalAccount", [256] + list(ALICE_PUBLIC[1:])),
        ]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                spec = copy.deepcopy(self.valid_production_spec)
                spec["genesis"]["runtimeGenesis"]["patch"]["futarchyTreasury"][key] = value

                failures = self.validate(spec, "polkadot")

                self.assertTrue(
                    any("09 §4" in failure and key in failure for failure in failures),
                    failures,
                )

    def test_dev_and_local_presets_are_exempt_from_coretime_seats(self) -> None:
        # Development/test presets seat stand-ins (09 §4), and the fast-timing
        # drill specs are validated with --profile local without seating them at
        # all — the gate must never fire outside paseo/polkadot.
        for profile in ("dev", "local"):
            with self.subTest(profile=profile):
                spec = copy.deepcopy(self.valid_spec)

                self.assertEqual(self.validate(spec, profile), [])

    def test_production_template_carries_both_coretime_seats_as_todo(self) -> None:
        # 09 §4 binds the template to the validator: the seats an operator must
        # fill are exactly the ones the validator refuses to let them skip.
        template = json.loads(ALLOCATIONS_TEMPLATE.read_text(encoding="utf-8"))
        seats = template["futarchy_treasury"]

        self.assertEqual(seats["coretime_quote_authority"], "TODO")
        self.assertEqual(seats["coretime_renewal_account"], "TODO")

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
