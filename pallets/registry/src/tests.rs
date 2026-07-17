//! Unit / property tests for `pallet-registry` (15 §4.1).
//!
//! Covers, per call: the happy path, the error paths, origin misuse, real USDC
//! bond custody (escrow / refund / 40-60 slash), the 07 §4 quorum + extension
//! lifecycle, claimant-adverse aggregates, the close/reap lifecycle, both
//! instances (Incident + Milestone), the rule-4 `Params` read, and `try_state`.

use crate::mock::*;
use crate::{Aggregates, Error, FilingCount, Filings};
use frame_support::{assert_noop, assert_ok};
use futarchy_primitives::{keeper::CrankClass, FixedU64};
use registry_core::{
    FilingClass, FilingState, RegistryKind, MAX_FILINGS_PER_EPOCH, MAX_LIVE_EPOCHS,
    REG_BOND_INCIDENT, REG_BOND_MILESTONE, REG_CLOSE_BATCH, REG_EXT_WINDOW_BLOCKS,
    REG_WINDOW_BLOCKS,
};

fn signed(n: u8) -> RuntimeOrigin {
    RuntimeOrigin::signed(acct(n))
}
const H: [u8; 32] = [9u8; 32];
const VER: u16 = 3;
/// A block past every filing challenge window and the default filing window.
const CLOSE_BLOCK: u64 = 1_000_002;
/// The mock's `reg.archive_delay` fixture (blocks); see `mock::ArchiveDelay`.
const ARCHIVE_DELAY: u64 = 100;

// ------------------------------------------------------------------- file

#[test]
fn incident_file_escrows_bond_and_records_filing() {
    new_test_ext().execute_with(|| {
        let a0 = usdc(&acct(ALICE));
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        let f = Filings::<Test, IncidentInstance>::get(5, 0).unwrap();
        assert_eq!(f.who, raw(ALICE));
        assert_eq!(f.bond, REG_BOND_INCIDENT);
        assert!(matches!(f.state, FilingState::Filed { .. }));
        assert_eq!(FilingCount::<Test, IncidentInstance>::get(5), 1);
        // Bond escrowed to the sovereign.
        assert_eq!(usdc(&acct(ALICE)), a0 - REG_BOND_INCIDENT);
        assert_eq!(usdc(&incident_account()), REG_BOND_INCIDENT);
        assert_ok!(IncidentRegistry::do_try_state());
    });
}

#[test]
fn file_rejects_closed_window_bad_class_and_spec_mismatch() {
    new_test_ext().execute_with(|| {
        // Window closed: now (1) > filing_window_end.
        set_filing_window_end(0);
        assert_noop!(
            IncidentRegistry::file(signed(ALICE), 5, FilingClass::S2, 0, H, VER),
            Error::<Test, IncidentInstance>::WindowClosed
        );
        set_filing_window_end(1_000_000);
        // Wrong class for the Incident instance.
        assert_noop!(
            IncidentRegistry::file(signed(ALICE), 5, FilingClass::Scope(1), 0, H, VER),
            Error::<Test, IncidentInstance>::InvalidClass
        );
        // Spec version other than the frozen version (I-16); filer cannot forge it.
        assert_noop!(
            IncidentRegistry::file(signed(ALICE), 5, FilingClass::S2, 0, H, VER + 1),
            Error::<Test, IncidentInstance>::SpecVersionMismatch
        );
        // No USDC moved on any rejected file.
        assert_eq!(usdc(&incident_account()), 0);
    });
}

#[test]
fn file_enforces_epoch_and_live_epoch_caps() {
    // limit-coverage: reg.max_filings_epoch, Registry filings (Incident/Milestone)
    new_test_ext().execute_with(|| {
        for i in 0..MAX_FILINGS_PER_EPOCH {
            assert_ok!(IncidentRegistry::file(
                signed(ALICE),
                7,
                FilingClass::S3,
                0,
                H,
                VER
            ));
            assert_eq!(FilingCount::<Test, IncidentInstance>::get(7), i + 1);
        }
        assert_noop!(
            IncidentRegistry::file(signed(ALICE), 7, FilingClass::S3, 0, H, VER),
            Error::<Test, IncidentInstance>::EpochFull
        );
    });
}

#[test]
fn file_rejects_a_fifth_live_epoch() {
    new_test_ext().execute_with(|| {
        for e in 1..=(MAX_LIVE_EPOCHS as u32) {
            assert_ok!(IncidentRegistry::file(
                signed(ALICE),
                e,
                FilingClass::S3,
                0,
                H,
                VER
            ));
        }
        assert_noop!(
            IncidentRegistry::file(signed(ALICE), 99, FilingClass::S3, 0, H, VER),
            Error::<Test, IncidentInstance>::TooManyLiveEpochs
        );
    });
}

#[test]
fn file_reads_bond_from_params_not_a_hardcode() {
    new_test_ext().execute_with(|| {
        // Rule 4: raise the live `reg.bond_incident` and prove the escrow follows.
        let raised = 7_000_000_000; // 7,000 USDC
        BondIncident::set(raised);
        let a0 = usdc(&acct(ALICE));
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_eq!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().bond,
            raised
        );
        assert_eq!(usdc(&acct(ALICE)), a0 - raised);
        assert_eq!(usdc(&incident_account()), raised);
    });
}

// ------------------------------------------------------------- challenge

#[test]
fn challenge_escrows_matching_bond_and_is_single_round() {
    new_test_ext().execute_with(|| {
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::challenge_filing(signed(BOB), 5, 0, H));
        assert!(matches!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().state,
            FilingState::Challenged { .. }
        ));
        // Both bonds now escrowed.
        assert_eq!(usdc(&incident_account()), 2 * REG_BOND_INCIDENT);
        // Registry games do not escalate: a second challenge is refused.
        assert_noop!(
            IncidentRegistry::challenge_filing(signed(CHARLIE), 5, 0, H),
            Error::<Test, IncidentInstance>::AlreadyChallenged
        );
    });
}

#[test]
fn challenge_unknown_filing_fails() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            IncidentRegistry::challenge_filing(signed(BOB), 5, 0, H),
            Error::<Test, IncidentInstance>::FilingNotFound
        );
    });
}

// ------------------------------------------------------------- ack_observed

#[test]
fn ack_requires_registered_watchtower_and_dedups() {
    new_test_ext().execute_with(|| {
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        // Unregistered acker rejected (07 §4).
        assert_noop!(
            IncidentRegistry::ack_observed(signed(WT1), 5, 0),
            Error::<Test, IncidentInstance>::NotRegistered
        );
        register_watchtower(WT1);
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_eq!(
            KeeperRebates::get(),
            vec![(acct(WT1), CrankClass::OracleLine)]
        );
        // One ack per watchtower per filing.
        assert_noop!(
            IncidentRegistry::ack_observed(signed(WT1), 5, 0),
            Error::<Test, IncidentInstance>::DuplicateAck
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![(acct(WT1), CrankClass::OracleLine)]
        );

        // The second registry instance has its own Config binding and receives
        // the same mandated keeper-class ack rebate.
        assert_ok!(MilestoneRegistry::file(
            signed(ALICE),
            6,
            FilingClass::Scope(1),
            10,
            H,
            VER
        ));
        assert_ok!(MilestoneRegistry::ack_observed(signed(WT1), 6, 0));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (acct(WT1), CrankClass::OracleLine),
                (acct(WT1), CrankClass::OracleLine),
            ]
        );
    });
}

// ------------------------------------------------------------- crank_close

#[test]
fn crank_close_upholds_with_quorum_and_refunds_filer() {
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        let after_file = usdc(&acct(ALICE));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT2), 5, 0));
        System::set_block_number(CLOSE_BLOCK);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert!(matches!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().state,
            FilingState::Upheld
        ));
        // Filer bond refunded in full; sovereign flat.
        assert_eq!(usdc(&acct(ALICE)), after_file + REG_BOND_INCIDENT);
        assert_eq!(usdc(&incident_account()), 0);
    });
}

#[test]
fn crank_close_extends_once_then_rejects_and_refunds() {
    new_test_ext().execute_with(|| {
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S3,
            0,
            H,
            VER
        ));
        let after_file = usdc(&acct(ALICE));
        // First crank past the 72 h window: no quorum ⇒ single 48 h extension.
        System::set_block_number(REG_WINDOW_BLOCKS as u64 + 2);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert!(matches!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().state,
            FilingState::Filed { extended: true, .. }
        ));
        assert_eq!(
            KeeperRebates::get(),
            vec![(acct(BOB), CrankClass::OracleLine)]
        );
        // Second crank past the extension: rejected-as-unobservable, bond refunded.
        System::set_block_number((REG_WINDOW_BLOCKS + REG_EXT_WINDOW_BLOCKS) as u64 + 3);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert!(matches!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().state,
            FilingState::Rejected
        ));
        assert_eq!(usdc(&acct(ALICE)), after_file + REG_BOND_INCIDENT);
        assert_eq!(usdc(&incident_account()), 0);
        // The extension cannot latch twice, and the terminal quorum-failure
        // rejection is unpaid hygiene work.
        assert_eq!(
            KeeperRebates::get(),
            vec![(acct(BOB), CrankClass::OracleLine)]
        );
    });
}

#[test]
fn quorum_failure_refund_path_cannot_farm_close_rebates() {
    new_test_ext().execute_with(|| {
        let before = usdc(&acct(ALICE));
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S3,
            0,
            H,
            VER
        ));
        assert_eq!(usdc(&acct(ALICE)), before - REG_BOND_INCIDENT);

        // A keeper first arriving after both windows causes the one-time
        // extension and rejection in one bounded call. No quorum was supplied,
        // the filing bond is refunded, and the close earns no rebate.
        System::set_block_number((REG_WINDOW_BLOCKS + REG_EXT_WINDOW_BLOCKS) as u64 + 3);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert!(matches!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().state,
            FilingState::Rejected
        ));
        assert_eq!(usdc(&acct(ALICE)), before);
        assert!(KeeperRebates::get().is_empty());
    });
}

#[test]
fn crank_close_rejects_oversized_batch() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            IncidentRegistry::crank_close(signed(BOB), 5, REG_CLOSE_BATCH as u32 + 1),
            Error::<Test, IncidentInstance>::BatchTooLarge
        );
    });
}

#[test]
fn keeper_rebate_is_instance_scoped_and_skips_noop_or_error_cranks() {
    new_test_ext().execute_with(|| {
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S3,
            0,
            H,
            VER
        ));

        // An open-window crank succeeds but advances nothing: no drain-vector
        // rebate. A rejected oversized batch likewise never reaches the sink.
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert_noop!(
            IncidentRegistry::crank_close(signed(BOB), 5, REG_CLOSE_BATCH as u32 + 1),
            Error::<Test, IncidentInstance>::BatchTooLarge
        );
        assert!(KeeperRebates::get().is_empty());

        // The due crank advances the filing to its latch-once extension and
        // earns exactly one rebate.
        System::set_block_number(REG_WINDOW_BLOCKS as u64 + 2);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert_eq!(
            KeeperRebates::get(),
            vec![(acct(BOB), CrankClass::OracleLine)]
        );

        // Once the extension expires, the quorum-failure rejection closes and
        // refunds the filing but earns nothing further.
        System::set_block_number((REG_WINDOW_BLOCKS + REG_EXT_WINDOW_BLOCKS) as u64 + 3);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert_eq!(
            KeeperRebates::get(),
            vec![(acct(BOB), CrankClass::OracleLine)]
        );

        // The second instance chooses its own Config seam too. A late
        // extension-plus-rejection there is equally unpaid.
        assert_ok!(MilestoneRegistry::file(
            signed(ALICE),
            6,
            FilingClass::Scope(1),
            10,
            H,
            VER
        ));
        System::set_block_number(
            System::block_number()
                + u64::from(REG_WINDOW_BLOCKS)
                + u64::from(REG_EXT_WINDOW_BLOCKS)
                + 1,
        );
        assert_ok!(MilestoneRegistry::crank_close(
            signed(BOB),
            6,
            REG_CLOSE_BATCH as u32
        ));
        assert_eq!(KeeperRebates::get().len(), 1);
    });
}

// --------------------------------------------------------- resolve_challenge

#[test]
fn resolve_reject_slashes_the_filer_40_60_to_challenger_and_insurance() {
    new_test_ext().execute_with(|| {
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::challenge_filing(signed(BOB), 5, 0, H));
        let alice0 = usdc(&acct(ALICE));
        let bob0 = usdc(&acct(BOB));
        let ins0 = usdc(&acct(INSURANCE));
        // uphold=false ⇒ the filing is rejected; ALICE (filer) is the loser.
        assert_ok!(IncidentRegistry::resolve_challenge(
            signed(RESOLVER),
            5,
            0,
            false
        ));
        assert!(matches!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().state,
            FilingState::Rejected
        ));
        let cs = REG_BOND_INCIDENT * 40 / 100;
        let is = REG_BOND_INCIDENT - cs;
        // Winner BOB: own bond back + 40 %. INSURANCE: 60 %. Loser ALICE: nothing.
        assert_eq!(usdc(&acct(BOB)), bob0 + REG_BOND_INCIDENT + cs);
        assert_eq!(usdc(&acct(INSURANCE)), ins0 + is);
        assert_eq!(usdc(&acct(ALICE)), alice0);
        assert_eq!(usdc(&incident_account()), 0);
    });
}

#[test]
fn resolve_uphold_slashes_the_challenger() {
    new_test_ext().execute_with(|| {
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::challenge_filing(signed(BOB), 5, 0, H));
        let alice0 = usdc(&acct(ALICE));
        let bob0 = usdc(&acct(BOB));
        // uphold=true ⇒ filing upheld; BOB (challenger) is the loser.
        assert_ok!(IncidentRegistry::resolve_challenge(
            signed(RESOLVER),
            5,
            0,
            true
        ));
        assert!(matches!(
            Filings::<Test, IncidentInstance>::get(5, 0).unwrap().state,
            FilingState::Upheld
        ));
        let cs = REG_BOND_INCIDENT * 40 / 100;
        assert_eq!(usdc(&acct(ALICE)), alice0 + REG_BOND_INCIDENT + cs);
        assert_eq!(usdc(&acct(BOB)), bob0);
    });
}

#[test]
fn resolve_challenge_rejects_public_origin() {
    new_test_ext().execute_with(|| {
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::challenge_filing(signed(BOB), 5, 0, H));
        // A signed non-authority cannot resolve a challenge (rule 6).
        assert_noop!(
            IncidentRegistry::resolve_challenge(signed(ALICE), 5, 0, false),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

// --------------------------------------------------------- close / reap

#[test]
fn close_epoch_incident_aggregate_is_claimant_adverse_and_notifies_welfare() {
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT2), 5, 0));
        System::set_block_number(CLOSE_BLOCK);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert_ok!(IncidentRegistry::close_epoch(signed(BOB), 5));
        // One upheld S2 (severity 0.4) ⇒ max(0, 1 − 0.4) = 0.6.
        assert_eq!(
            Aggregates::<Test, IncidentInstance>::get(5),
            Some(FixedU64(600_000_000))
        );
        assert_eq!(
            WelfareLog::get(),
            vec![(RegistryKind::Incident, 5, 600_000_000)]
        );
        // The FilingCount slot is reaped so the live-epoch bound stays concurrent;
        // its absence is also what makes a re-close idempotent (NothingToClose).
        assert_eq!(FilingCount::<Test, IncidentInstance>::get(5), 0);
        assert_noop!(
            IncidentRegistry::close_epoch(signed(BOB), 5),
            Error::<Test, IncidentInstance>::NothingToClose
        );
    });
}

#[test]
fn close_epoch_requires_terminal_filings() {
    new_test_ext().execute_with(|| {
        set_filing_window_end(10);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        System::set_block_number(20); // past the filing window, filing still live
        assert_noop!(
            IncidentRegistry::close_epoch(signed(BOB), 5),
            Error::<Test, IncidentInstance>::WindowOpen
        );
    });
}

#[test]
fn reap_epoch_needs_close_out_then_the_archive_delay() {
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT2), 5, 0));
        System::set_block_number(CLOSE_BLOCK);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        // Cannot reap a not-yet-closed epoch.
        assert_noop!(
            IncidentRegistry::reap_epoch(signed(BOB), 5),
            Error::<Test, IncidentInstance>::ReapNotDue
        );
        assert_ok!(IncidentRegistry::close_epoch(signed(BOB), 5));
        // Still not reapable until the archive delay elapses since close — so
        // welfare has consumed the aggregate before the records are destroyed.
        assert_noop!(
            IncidentRegistry::reap_epoch(signed(BOB), 5),
            Error::<Test, IncidentInstance>::ReapNotDue
        );
        System::set_block_number(CLOSE_BLOCK + ARCHIVE_DELAY + 1);
        assert_ok!(IncidentRegistry::reap_epoch(signed(BOB), 5));
        assert!(Filings::<Test, IncidentInstance>::get(5, 0).is_none());
        assert!(Aggregates::<Test, IncidentInstance>::get(5).is_none());
        assert_ok!(IncidentRegistry::do_try_state());
    });
}

#[test]
fn reap_epoch_rebates_once_only_after_bounded_cleanup_succeeds() {
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT2), 5, 0));
        System::set_block_number(CLOSE_BLOCK);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert_ok!(IncidentRegistry::close_epoch(signed(BOB), 5));
        KeeperRebates::set(Vec::new());

        assert_noop!(
            IncidentRegistry::reap_epoch(signed(BOB), 5),
            Error::<Test, IncidentInstance>::ReapNotDue
        );
        assert!(KeeperRebates::get().is_empty());

        System::set_block_number(CLOSE_BLOCK + ARCHIVE_DELAY + 1);
        assert_ok!(IncidentRegistry::reap_epoch(signed(BOB), 5));
        assert_eq!(
            KeeperRebates::get(),
            vec![(acct(BOB), CrankClass::OracleLine)]
        );
        assert_noop!(
            IncidentRegistry::reap_epoch(signed(BOB), 5),
            Error::<Test, IncidentInstance>::ReapNotDue
        );
        assert_eq!(KeeperRebates::get().len(), 1);
    });
}

#[test]
fn a_reaped_epoch_cannot_be_reclosed_to_the_favorable_value() {
    // Dual-review HIGH: a permissionless reap + reap-of-the-finality-marker let an
    // attacker erase an incident and re-close the empty epoch to the favorable
    // "no filings ⇒ 1" value. The archive-delay gate + the FilingCount-present
    // precondition close it.
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        // An S1 incident (severity 1.0) ⇒ aggregate 0 (maximally adverse).
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S1,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT2), 5, 0));
        System::set_block_number(CLOSE_BLOCK);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        assert_ok!(IncidentRegistry::close_epoch(signed(BOB), 5));
        assert_eq!(
            Aggregates::<Test, IncidentInstance>::get(5),
            Some(FixedU64(0))
        );
        System::set_block_number(CLOSE_BLOCK + ARCHIVE_DELAY + 1);
        assert_ok!(IncidentRegistry::reap_epoch(signed(BOB), 5));
        // The replay: re-close the now-empty epoch → refused (no FilingCount).
        assert_noop!(
            IncidentRegistry::close_epoch(signed(BOB), 5),
            Error::<Test, IncidentInstance>::NothingToClose
        );
        // Welfare was notified exactly once, with the real adverse value.
        assert_eq!(WelfareLog::get(), vec![(RegistryKind::Incident, 5, 0)]);
    });
}

#[test]
fn close_epoch_refuses_an_unfiled_epoch() {
    new_test_ext().execute_with(|| {
        set_filing_window_end(10);
        System::set_block_number(20);
        // Never filed ⇒ nothing to close (welfare's pull-side "no record ⇒ 1"
        // handles genuinely-empty epochs; a permissionless empty close would grief).
        assert_noop!(
            IncidentRegistry::close_epoch(signed(BOB), 9),
            Error::<Test, IncidentInstance>::NothingToClose
        );
    });
}

#[test]
fn close_epoch_rolls_back_when_welfare_refuses() {
    // G-1: a refusing welfare sink must roll the whole close back — no aggregate,
    // no ClosedAt, FilingCount still live, no welfare log entry — so registry and
    // welfare never silently disagree (dual-review re-pass minor).
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT2), 5, 0));
        System::set_block_number(CLOSE_BLOCK);
        assert_ok!(IncidentRegistry::crank_close(
            signed(BOB),
            5,
            REG_CLOSE_BATCH as u32
        ));
        WelfareFails::set(true);
        assert!(IncidentRegistry::close_epoch(signed(BOB), 5).is_err());
        // Fully rolled back.
        assert!(Aggregates::<Test, IncidentInstance>::get(5).is_none());
        assert_eq!(FilingCount::<Test, IncidentInstance>::get(5), 1);
        assert!(WelfareLog::get().is_empty());
        // With welfare healthy the same close succeeds.
        WelfareFails::set(false);
        assert_ok!(IncidentRegistry::close_epoch(signed(BOB), 5));
        assert_eq!(
            Aggregates::<Test, IncidentInstance>::get(5),
            Some(FixedU64(600_000_000))
        );
        assert_ok!(IncidentRegistry::do_try_state());
    });
}

#[test]
fn acks_are_capped_at_quorum() {
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        register_watchtower(WT3);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT1), 5, 0));
        assert_ok!(IncidentRegistry::ack_observed(signed(WT2), 5, 0));
        // Quorum reached — a third distinct watchtower's ack is refused, so the
        // per-filing ack set stays bounded.
        assert_noop!(
            IncidentRegistry::ack_observed(signed(WT3), 5, 0),
            Error::<Test, IncidentInstance>::AlreadyQuorum
        );
    });
}

#[test]
fn ack_on_a_challenged_filing_is_rejected() {
    // PR #54 Codex-bot P2: once a filing is challenged, 07 §4's "a challenge
    // supersedes the quorum requirement" makes acks moot — they must be rejected,
    // not recorded, or the `Challenged` state (which has no ack cap) would grow
    // `AckRecords` past the per-filing `WT_QUORUM` bound `do_try_state` and
    // `reap_epoch`'s bounded reap assume.
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        register_watchtower(WT3);
        assert_ok!(IncidentRegistry::file(
            signed(ALICE),
            5,
            FilingClass::S2,
            0,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::challenge_filing(signed(BOB), 5, 0, H));
        // No registered watchtower can ack a challenged filing — the ack set never
        // grows, however many watchtowers try.
        assert_noop!(
            IncidentRegistry::ack_observed(signed(WT1), 5, 0),
            Error::<Test, IncidentInstance>::AlreadyChallenged
        );
        assert_noop!(
            IncidentRegistry::ack_observed(signed(WT3), 5, 0),
            Error::<Test, IncidentInstance>::AlreadyChallenged
        );
        assert_ok!(IncidentRegistry::do_try_state());
    });
}

// --------------------------------------------------------- milestone instance

#[test]
fn milestone_instance_is_independent_and_scores_points_over_target() {
    new_test_ext().execute_with(|| {
        register_watchtower(WT1);
        register_watchtower(WT2);
        // A milestone filing and an incident filing for the same epoch are
        // separate storage (distinct instances).
        assert_ok!(MilestoneRegistry::file(
            signed(ALICE),
            3,
            FilingClass::Scope(1),
            25,
            H,
            VER
        ));
        assert_ok!(IncidentRegistry::file(
            signed(BOB),
            3,
            FilingClass::S1,
            0,
            H,
            VER
        ));
        assert!(Filings::<Test, MilestoneInstance>::get(3, 0).is_some());
        assert!(Filings::<Test, IncidentInstance>::get(3, 0).is_some());
        // Milestone bond is the smaller reg.bond_milestone.
        assert_eq!(
            Filings::<Test, MilestoneInstance>::get(3, 0).unwrap().bond,
            REG_BOND_MILESTONE
        );
        assert_ok!(MilestoneRegistry::ack_observed(signed(WT1), 3, 0));
        assert_ok!(MilestoneRegistry::ack_observed(signed(WT2), 3, 0));
        System::set_block_number(CLOSE_BLOCK);
        assert_ok!(MilestoneRegistry::crank_close(
            signed(CHARLIE),
            3,
            REG_CLOSE_BATCH as u32
        ));
        assert_ok!(MilestoneRegistry::close_epoch(signed(CHARLIE), 3));
        // points 25 / target 100 = 0.25.
        assert_eq!(
            Aggregates::<Test, MilestoneInstance>::get(3),
            Some(FixedU64(250_000_000))
        );
        assert_eq!(
            WelfareLog::get(),
            vec![(RegistryKind::Milestone, 3, 250_000_000)]
        );
        // The incident instance's epoch 3 is untouched by the milestone close.
        assert!(Aggregates::<Test, IncidentInstance>::get(3).is_none());
    });
}

#[test]
fn milestone_rejects_incident_class() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            MilestoneRegistry::file(signed(ALICE), 3, FilingClass::S1, 5, H, VER),
            Error::<Test, MilestoneInstance>::InvalidClass
        );
    });
}

/// Helper: file `points`, quorum-uphold, and close milestone epoch 3, returning
/// the derived aggregate.
fn milestone_close_with_points(points: u16) -> FixedU64 {
    register_watchtower(WT1);
    register_watchtower(WT2);
    assert_ok!(MilestoneRegistry::file(
        signed(ALICE),
        3,
        FilingClass::Scope(1),
        points,
        H,
        VER
    ));
    assert_ok!(MilestoneRegistry::ack_observed(signed(WT1), 3, 0));
    assert_ok!(MilestoneRegistry::ack_observed(signed(WT2), 3, 0));
    System::set_block_number(CLOSE_BLOCK);
    assert_ok!(MilestoneRegistry::crank_close(
        signed(BOB),
        3,
        REG_CLOSE_BATCH as u32
    ));
    assert_ok!(MilestoneRegistry::close_epoch(signed(BOB), 3));
    Aggregates::<Test, MilestoneInstance>::get(3).unwrap()
}

#[test]
fn milestone_target_is_a_seam_not_a_hardcode() {
    new_test_ext().execute_with(|| {
        // Divisor comes from the frozen-MetricSpec seam (I-16), not the core's
        // default 100: target 50 ⇒ 25 / 50 = 0.5.
        MilestoneTarget::set(50);
        assert_eq!(milestone_close_with_points(25), FixedU64(500_000_000));
    });
}

#[test]
fn milestone_over_ship_clamps_to_one() {
    new_test_ext().execute_with(|| {
        // 65_535 / 100 = 655.35 would violate the welfare component's [0,1] range;
        // the aggregate is clamped to 1.0 (dual-review HIGH).
        assert_eq!(
            milestone_close_with_points(u16::MAX),
            FixedU64(1_000_000_000)
        );
    });
}
