//! Pallet-level sweep differential (S1, 15 §4.4; 03 §5.4).
//!
//! The frame-free core has no archive clock, bounded storage-prefix scan,
//! collateral custody, or deposit refund surface. This module therefore reads
//! the single-generator `ledger_sweep_scenarios` from `vectors.json` and drives
//! the real FRAME calls across the archive-delay boundary. The core's own
//! operation replay remains in `crates/conditional-ledger-core/tests`.

use std::{collections::BTreeMap, fs, path::PathBuf};

use crate::{
    mock::*, BaselineTerminalAt, BaselineVaults, DepositsHeld, Error, Event, PositionCount,
    PositionTotals, Positions, VaultTerminalAt, Vaults,
};
use conditional_ledger_core::{baseline, position};
use frame_support::{assert_noop, assert_ok, traits::fungibles::Inspect};
use frame_system::RawOrigin;
use futarchy_primitives::{Branch, FixedU64, GateType, PositionId, PositionKind, ScalarSide};

type E = Error<Test>;

#[derive(Debug)]
enum Json {
    Object(BTreeMap<String, Json>),
    Array(Vec<Json>),
    String(String),
    Number(u128),
}

impl Json {
    fn object(&self) -> &BTreeMap<String, Json> {
        let Self::Object(value) = self else {
            panic!("expected JSON object, got {self:?}")
        };
        value
    }

    fn array(&self) -> &[Json] {
        let Self::Array(value) = self else {
            panic!("expected JSON array, got {self:?}")
        };
        value
    }

    fn string(&self) -> &str {
        let Self::String(value) = self else {
            panic!("expected JSON string, got {self:?}")
        };
        value
    }

    fn number(&self) -> u128 {
        let Self::Number(value) = self else {
            panic!("expected JSON number, got {self:?}")
        };
        *value
    }
}

fn field<'a>(object: &'a BTreeMap<String, Json>, key: &str) -> &'a Json {
    object
        .get(key)
        .unwrap_or_else(|| panic!("missing JSON field {key}"))
}

/// Narrow dependency-free JSON parser for the generated sweep section, so
/// this test adds no JSON dev-dependency to the pallet.
struct Parser<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            cursor: 0,
        }
    }

    fn parse(mut self) -> Json {
        let value = self.value();
        self.whitespace();
        value
    }

    fn value(&mut self) -> Json {
        self.whitespace();
        match self.peek() {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Json::String(self.string()),
            b'0'..=b'9' => Json::Number(self.number()),
            other => panic!("unsupported JSON byte {other:?} at {}", self.cursor),
        }
    }

    fn object(&mut self) -> Json {
        self.expect(b'{');
        let mut object = BTreeMap::new();
        self.whitespace();
        if self.peek() == b'}' {
            self.cursor += 1;
            return Json::Object(object);
        }
        loop {
            self.whitespace();
            let key = self.string();
            self.whitespace();
            self.expect(b':');
            object.insert(key, self.value());
            self.whitespace();
            match self.peek() {
                b',' => self.cursor += 1,
                b'}' => {
                    self.cursor += 1;
                    break;
                }
                other => panic!("unexpected object byte {other:?} at {}", self.cursor),
            }
        }
        Json::Object(object)
    }

    fn array(&mut self) -> Json {
        self.expect(b'[');
        let mut array = Vec::new();
        self.whitespace();
        if self.peek() == b']' {
            self.cursor += 1;
            return Json::Array(array);
        }
        loop {
            array.push(self.value());
            self.whitespace();
            match self.peek() {
                b',' => self.cursor += 1,
                b']' => {
                    self.cursor += 1;
                    break;
                }
                other => panic!("unexpected array byte {other:?} at {}", self.cursor),
            }
        }
        Json::Array(array)
    }

    fn string(&mut self) -> String {
        self.expect(b'"');
        let mut value = String::new();
        loop {
            let byte = self.peek();
            self.cursor += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let escaped = self.peek();
                    self.cursor += 1;
                    value.push(match escaped {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{0008}',
                        b'f' => '\u{000c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        other => panic!("unsupported JSON escape {other:?}"),
                    });
                }
                _ => value.push(char::from(byte)),
            }
        }
        value
    }

    fn number(&mut self) -> u128 {
        let start = self.cursor;
        while self.peek().is_ascii_digit() {
            self.cursor += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.cursor])
            .expect("generated JSON number is UTF-8")
            .parse()
            .expect("generated JSON number fits u128")
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), b' ' | b'\n' | b'\r' | b'\t') {
            self.cursor += 1;
        }
    }

    fn expect(&mut self, expected: u8) {
        assert_eq!(self.peek(), expected, "unexpected generated JSON token");
        self.cursor += 1;
    }

    fn peek(&self) -> u8 {
        *self
            .bytes
            .get(self.cursor)
            .unwrap_or_else(|| panic!("unexpected end of generated JSON"))
    }
}

fn vector_path() -> PathBuf {
    std::env::var_os("BLEAVIT_LEDGER_VECTOR_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../reference-model/fixtures/vectors.json")
        })
}

fn sweep_scenarios() -> Json {
    let text = fs::read_to_string(vector_path()).expect("read shared reference-model vectors");
    let marker = "\"ledger_sweep_scenarios\":";
    let start = text
        .find(marker)
        .unwrap_or_else(|| panic!("vectors omit ledger_sweep_scenarios"))
        + marker.len();
    Parser::new(&text[start..]).parse()
}

fn signed(who: AccountId) -> RuntimeOrigin {
    RawOrigin::Signed(who).into()
}

fn account(name: &str) -> AccountId {
    match name {
        "alice" => ALICE,
        "bob" => BOB,
        "carol" => CHARLIE,
        other => panic!("unknown sweep-vector holder {other}"),
    }
}

fn branch(name: &str) -> Branch {
    match name {
        "Accept" => Branch::Accept,
        "Reject" => Branch::Reject,
        other => panic!("unknown sweep-vector branch {other}"),
    }
}

fn gate(name: &str) -> GateType {
    match name {
        "Survival" => GateType::Survival,
        "Security" => GateType::Security,
        other => panic!("unknown sweep-vector gate {other}"),
    }
}

fn scalar_side(name: &str) -> ScalarSide {
    match name {
        "Long" => ScalarSide::Long,
        "Short" => ScalarSide::Short,
        other => panic!("unknown sweep-vector scalar side {other}"),
    }
}

fn position_id(args: &BTreeMap<String, Json>, pid: u64, epoch: u32) -> PositionId {
    let coordinates = field(args, "position").object();
    match field(coordinates, "family").string() {
        "proposal" => {
            let branch = branch(field(coordinates, "branch").string());
            let kind = match field(coordinates, "kind").string() {
                "BranchUsdc" => PositionKind::BranchUsdc,
                "Long" => PositionKind::Long,
                "Short" => PositionKind::Short,
                "GateYes" => PositionKind::GateYes(gate(field(coordinates, "gate").string())),
                "GateNo" => PositionKind::GateNo(gate(field(coordinates, "gate").string())),
                other => panic!("unknown sweep-vector position kind {other}"),
            };
            position(pid, branch, kind)
        }
        "baseline" => baseline(epoch, scalar_side(field(coordinates, "side").string())),
        other => panic!("unknown sweep-vector position family {other}"),
    }
}

fn apply_setup(operation: &Json, pid: u64, epoch: u32) {
    let operation = operation.object();
    let name = field(operation, "op").string();
    let args = field(operation, "args").object();
    let amount = || field(args, "amount").number();
    match name {
        "split" => {
            assert_ok!(Ledger::split(
                signed(account(field(args, "account").string())),
                pid,
                amount()
            ));
        }
        "split_scalar" => {
            assert_ok!(Ledger::split_scalar(
                signed(account(field(args, "account").string())),
                pid,
                branch(field(args, "branch").string()),
                amount()
            ));
        }
        "split_gate" => {
            assert_ok!(Ledger::split_gate(
                signed(account(field(args, "account").string())),
                pid,
                branch(field(args, "branch").string()),
                gate(field(args, "gate").string()),
                amount()
            ));
        }
        "transfer" => {
            assert_ok!(Ledger::transfer(
                signed(account(field(args, "from").string())),
                position_id(args, pid, epoch),
                account(field(args, "to").string()),
                amount()
            ));
        }
        "void" => {
            assert_ok!(Ledger::void(signed(RESOLVER), pid));
        }
        "split_baseline" => {
            assert_ok!(Ledger::split_baseline(
                signed(account(field(args, "account").string())),
                epoch,
                amount()
            ));
        }
        "settle_baseline" => {
            assert_ok!(Ledger::settle_baseline(
                signed(SETTLER),
                epoch,
                FixedU64(u64::try_from(field(args, "s").number()).expect("score fits u64"))
            ));
        }
        other => panic!("unsupported generated sweep setup operation {other}"),
    }
}

fn belongs_to_family(id: PositionId, family: &str, target: u64) -> bool {
    match (family, id) {
        ("proposal", PositionId::Proposal { proposal, .. }) => proposal == target,
        ("baseline", PositionId::Baseline { epoch, .. }) => u64::from(epoch) == target,
        _ => false,
    }
}

fn position_entries(family: &str, target: u64) -> usize {
    Positions::<Test>::iter()
        .filter(|(id, _, _)| belongs_to_family(*id, family, target))
        .count()
}

fn position_totals(family: &str, target: u64) -> usize {
    PositionTotals::<Test>::iter()
        .filter(|(id, _)| belongs_to_family(*id, family, target))
        .count()
}

fn usdc(who: AccountId) -> u128 {
    <Assets as Inspect<AccountId>>::balance(USDC, &who)
}

#[test]
fn generated_sweep_vectors_match_real_pallet_housekeeping() {
    let scenarios = sweep_scenarios();
    assert_eq!(scenarios.array().len(), 2);

    for scenario in scenarios.array() {
        let scenario = scenario.object();
        let name = field(scenario, "name").string();
        let family = field(scenario, "family").string();
        let target = field(scenario, "id").number();
        let pid = target as u64;
        let epoch = target as u32;
        let reap_batch =
            u32::try_from(field(scenario, "reap_batch").number()).expect("ReapBatch fits u32");
        let expected_entries = usize::try_from(field(scenario, "expected_entries").number())
            .expect("entry count fits usize");
        let expected_batches = usize::try_from(field(scenario, "expected_batches").number())
            .expect("batch count fits usize");
        let expected_residue = field(scenario, "expected_residue").number();
        let expected_refunds = field(scenario, "expected_refunds").object();

        new_test_ext().execute_with(|| {
            match family {
                "proposal" => {
                    assert_ok!(Ledger::create_vault(signed(MARKET), pid, 0));
                }
                "baseline" => {
                    assert_ok!(Ledger::create_baseline_vault(signed(MARKET), epoch));
                }
                other => panic!("unknown sweep scenario family {other}"),
            }
            for operation in field(scenario, "setup_ops").array() {
                apply_setup(operation, pid, epoch);
            }
            assert_eq!(
                position_entries(family, target as u64),
                expected_entries,
                "{name}"
            );
            assert_eq!(
                DepositsHeld::<Test>::get(),
                expected_entries as u128 * futarchy_primitives::kernel::POSITION_DEPOSIT_USDC,
                "{name} initial deposits"
            );

            // I-27 / 03 §5.4: terminal Voided and Settled vaults remain
            // unsweepable until the archive boundary is reached.
            match family {
                "proposal" => {
                    assert_noop!(Ledger::sweep_dust(signed(ALICE), pid), E::ReapNotDue);
                }
                "baseline" => {
                    assert_noop!(
                        Ledger::sweep_dust_baseline(signed(ALICE), epoch),
                        E::ReapNotDue
                    );
                }
                _ => unreachable!(),
            }

            let terminal_at = match family {
                "proposal" => VaultTerminalAt::<Test>::get(pid).expect("proposal terminal block"),
                "baseline" => {
                    BaselineTerminalAt::<Test>::get(epoch).expect("Baseline terminal block")
                }
                _ => unreachable!(),
            };
            ReapBatch::set(reap_batch);
            System::set_block_number(terminal_at + ArchiveDelay::get());

            let insurance_before = usdc(INSURANCE);
            let owner_balances = [
                ("alice", ALICE, usdc(ALICE)),
                ("bob", BOB, usdc(BOB)),
                ("carol", CHARLIE, usdc(CHARLIE)),
            ];
            for batch in 0..expected_batches {
                let before = position_entries(family, target as u64);
                match family {
                    "proposal" => {
                        assert_ok!(Ledger::sweep_dust(signed(ALICE), pid));
                    }
                    "baseline" => {
                        assert_ok!(Ledger::sweep_dust_baseline(signed(ALICE), epoch));
                    }
                    _ => unreachable!(),
                }
                let after = position_entries(family, target as u64);
                assert_eq!(
                    before - after,
                    before.min(reap_batch as usize),
                    "{name} batch {batch} did not honor ReapBatch"
                );
                if batch + 1 < expected_batches {
                    match family {
                        "proposal" => assert!(Vaults::<Test>::contains_key(pid), "{name}"),
                        "baseline" => {
                            assert!(BaselineVaults::<Test>::contains_key(epoch), "{name}")
                        }
                        _ => unreachable!(),
                    }
                }
            }

            assert_eq!(position_entries(family, target as u64), 0, "{name}");
            assert_eq!(position_totals(family, target as u64), 0, "{name}");
            assert_eq!(DepositsHeld::<Test>::get(), 0, "{name}");
            for (owner_name, owner, before) in owner_balances {
                let refund = field(expected_refunds, owner_name).number();
                assert_eq!(
                    usdc(owner) - before,
                    refund,
                    "{name} refund for {owner_name}"
                );
                assert_eq!(PositionCount::<Test>::get(owner), 0, "{name} owner count");
            }
            assert_eq!(
                usdc(INSURANCE) - insurance_before,
                expected_residue,
                "{name} Python residue"
            );

            let last_ledger_event = System::events()
                .into_iter()
                .rev()
                .find_map(|record| match record.event {
                    RuntimeEvent::Ledger(event) => Some(event),
                    _ => None,
                })
                .expect("sweep emits ledger event");
            match (family, last_ledger_event) {
                (
                    "proposal",
                    Event::VaultReaped {
                        pid: event_pid,
                        residue,
                    },
                ) => {
                    assert_eq!(event_pid, pid, "{name}");
                    assert_eq!(residue, expected_residue, "{name}");
                    assert!(!Vaults::<Test>::contains_key(pid), "{name}");
                    assert!(!VaultTerminalAt::<Test>::contains_key(pid), "{name}");
                }
                (
                    "baseline",
                    Event::BaselineVaultReaped {
                        epoch: event_epoch,
                        residue,
                    },
                ) => {
                    assert_eq!(event_epoch, epoch, "{name}");
                    assert_eq!(residue, expected_residue, "{name}");
                    assert!(!BaselineVaults::<Test>::contains_key(epoch), "{name}");
                    assert!(!BaselineTerminalAt::<Test>::contains_key(epoch), "{name}");
                }
                (_, event) => panic!("{name} emitted unexpected final event {event:?}"),
            }
            assert_ok!(Ledger::do_try_state());
        });
    }
}
