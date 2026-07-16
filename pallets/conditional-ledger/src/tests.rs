//! Doc-15 test obligations for `pallet-conditional-ledger` (audit scope A, R-7).
//!
//! The frame-free core (`conditional-ledger-core`) already carries the deep
//! state-machine / conservation suites and is the differential oracle; these
//! tests exercise the FRAME surface the wrapper adds — origin gating, real USDC
//! custody, the scoped load/persist adapter, `sweep_dust` reaping — plus the
//! spec's mandatory named regression vectors (03 §6.3, §6.4) and a pallet≡core
//! differential check.

use crate::{mock::*, DepositsHeld, Error, Event, Positions, VaultTerminalAt, Vaults};
use conditional_ledger_core::{position as pos, LedgerOrigin, LedgerState};
use frame_support::{assert_noop, assert_ok, traits::fungibles::Inspect};
use frame_system::RawOrigin;
use futarchy_primitives::{
    keeper::CrankClass, kernel, Branch, FixedU64, GateType, MetricSpecVersion, PositionKind,
    ProposalId, ScalarSide,
};

type E = Error<Test>;

fn signed(a: AccountId) -> RuntimeOrigin {
    RawOrigin::Signed(a).into()
}
fn create(pid: ProposalId) {
    assert_ok!(Ledger::create_vault(
        signed(MARKET),
        pid,
        0 as MetricSpecVersion
    ));
}
fn create_base(epoch: u32) {
    assert_ok!(Ledger::create_baseline_vault(signed(MARKET), epoch));
}
fn usdc(who: AccountId) -> u128 {
    <Assets as Inspect<AccountId>>::balance(USDC, &who)
}
fn escrow(pid: ProposalId) -> u128 {
    Vaults::<Test>::get(pid).map(|v| v.escrowed).unwrap_or(0)
}
fn ledger_events() -> Vec<Event<Test>> {
    System::events()
        .into_iter()
        .filter_map(|r| match r.event {
            RuntimeEvent::Ledger(e) => Some(e),
            _ => None,
        })
        .collect()
}
fn try_state() {
    assert_ok!(Ledger::do_try_state());
}

// --------------------------------------------------------------- happy paths

#[test]
fn split_moves_usdc_and_takes_deposits() {
    new_test_ext().execute_with(|| {
        create(1);
        let before = usdc(ALICE);
        let sov_before = usdc(ledger_account());
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        // escrow of 10 USDC + 2 position deposits (Accept/Reject branch-USDC) left ALICE.
        let deposit = 2 * kernel::POSITION_DEPOSIT_USDC;
        assert_eq!(usdc(ALICE), before - 10 * UNIT - deposit);
        assert_eq!(usdc(ledger_account()), sov_before + 10 * UNIT + deposit);
        assert_eq!(escrow(1), 10 * UNIT);
        assert_eq!(DepositsHeld::<Test>::get(), deposit);
        assert_eq!(
            Positions::<Test>::get(pos(1, Branch::Accept, PositionKind::BranchUsdc), ALICE),
            10 * UNIT
        );
        try_state();
    });
}

#[test]
fn merge_pays_out_and_refunds_deposits() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        let before = usdc(ALICE);
        assert_ok!(Ledger::merge(signed(ALICE), 1, 10 * UNIT));
        // 10 USDC back + both deposits refunded (positions deleted).
        assert_eq!(
            usdc(ALICE),
            before + 10 * UNIT + 2 * kernel::POSITION_DEPOSIT_USDC
        );
        assert_eq!(escrow(1), 0);
        assert_eq!(DepositsHeld::<Test>::get(), 0);
        try_state();
    });
}

#[test]
fn scalar_and_gate_splits_move_no_escrow() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 4 * UNIT));
        let sov = usdc(ledger_account());
        assert_ok!(Ledger::split_scalar(
            signed(ALICE),
            1,
            Branch::Accept,
            2 * UNIT
        ));
        assert_ok!(Ledger::split_gate(
            signed(ALICE),
            1,
            Branch::Reject,
            GateType::Security,
            UNIT
        ));
        // Only deposit moves (new leg entries); escrow unchanged at 4 USDC.
        assert_eq!(escrow(1), 4 * UNIT);
        assert!(usdc(ledger_account()) >= sov); // only deposits added, no escrow change
        try_state();
    });
}

#[test]
fn full_scalar_settlement_lifecycle() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        assert_ok!(Ledger::split_scalar(
            signed(ALICE),
            1,
            Branch::Accept,
            10 * UNIT
        ));
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_ok!(Ledger::settle_scalar(
            signed(SETTLER),
            1,
            FixedU64(600_000_000)
        ));
        let before = usdc(ALICE);
        assert_ok!(Ledger::redeem_scalar(
            signed(ALICE),
            1,
            ScalarSide::Long,
            10 * UNIT
        ));
        assert_ok!(Ledger::redeem_scalar(
            signed(ALICE),
            1,
            ScalarSide::Short,
            10 * UNIT
        ));
        // LONG floor(10·0.6)=6, SHORT floor(10·0.4)=4 → 10 USDC total (exact here).
        assert_eq!(
            usdc(ALICE) - before,
            10 * UNIT + 2 * kernel::POSITION_DEPOSIT_USDC
        );
        assert_eq!(escrow(1), 0);
        try_state();
    });
}

#[test]
fn baseline_lifecycle_pair_exact() {
    new_test_ext().execute_with(|| {
        create_base(9);
        assert_ok!(Ledger::split_baseline(signed(ALICE), 9, 5 * UNIT));
        assert_ok!(Ledger::settle_baseline(
            signed(SETTLER),
            9,
            FixedU64(500_000_000)
        ));
        assert_ok!(Ledger::redeem_baseline_pair(signed(ALICE), 9, 5 * UNIT));
        try_state();
    });
}

// ------------------------------------------------------- 03 §6.3 (B-5) vector

#[test]
fn pt3_scalar_rounding_against_claimant_conserves() {
    // s = 0.70005, E = Q_w = 20_000 base units; one 20_000 LONG holder, two
    // 10_000 SHORT holders. Old rule paid 20_001 (insolvent); new floors pay
    // 14_001 + 2_999 + 2_999 = 19_999 ≤ 20_000 (residue 1).
    new_test_ext().execute_with(|| {
        create(1);
        let amt = 20_000u128;
        assert_ok!(Ledger::split(signed(ALICE), 1, amt));
        assert_ok!(Ledger::split_scalar(signed(ALICE), 1, Branch::Accept, amt));
        let short = pos(1, Branch::Accept, PositionKind::Short);
        assert_ok!(Ledger::transfer(signed(ALICE), short, BOB, 10_000));
        assert_ok!(Ledger::transfer(signed(ALICE), short, CHARLIE, 10_000));
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_ok!(Ledger::settle_scalar(
            signed(SETTLER),
            1,
            FixedU64(700_050_000)
        ));
        assert_eq!(escrow(1), 20_000);

        assert_ok!(Ledger::redeem_scalar(
            signed(ALICE),
            1,
            ScalarSide::Long,
            20_000
        ));
        assert_ok!(Ledger::redeem_scalar(
            signed(BOB),
            1,
            ScalarSide::Short,
            10_000
        ));
        assert_ok!(Ledger::redeem_scalar(
            signed(CHARLIE),
            1,
            ScalarSide::Short,
            10_000
        ));

        // Σ payouts = 19_999, residue 1 remains in escrow (never insolvent).
        assert_eq!(escrow(1), 1);
        let payouts: Vec<u128> = ledger_events()
            .into_iter()
            .filter_map(|e| match e {
                Event::ScalarRedeemed { payout, .. } => Some(payout),
                _ => None,
            })
            .collect();
        assert_eq!(payouts, vec![14_001, 2_999, 2_999]);
        try_state();
    });
}

// ------------------------------------------------------- 03 §6.4 (B-1) vector

#[test]
fn pt2_void_merge_recovers_par_not_double() {
    // split 100_000 → void → merge recovers exactly par (100_000), never 200_000.
    new_test_ext().execute_with(|| {
        create(1);
        let a = 100_000u128;
        assert_ok!(Ledger::split(signed(ALICE), 1, a));
        assert_ok!(Ledger::void(signed(RESOLVER), 1));
        assert_eq!(escrow(1), a);
        let before = usdc(ALICE);
        assert_ok!(Ledger::merge(signed(ALICE), 1, a));
        assert_eq!(usdc(ALICE) - before, a + 2 * kernel::POSITION_DEPOSIT_USDC); // par + deposit refunds
        assert_eq!(escrow(1), 0);
        try_state();
    });
}

#[test]
fn void_redeem_unpaired_halves_and_quarters() {
    new_test_ext().execute_with(|| {
        create(1);
        let a = 100_000u128;
        // ALICE and BOB each hold one branch only (unpaired) so par is unreachable.
        assert_ok!(Ledger::split(signed(ALICE), 1, a));
        let reject = pos(1, Branch::Reject, PositionKind::BranchUsdc);
        assert_ok!(Ledger::transfer(signed(ALICE), reject, BOB, a));
        assert_ok!(Ledger::void(signed(RESOLVER), 1));
        // branch-USDC pays floor(a/2).
        let a_bal = usdc(ALICE);
        assert_ok!(Ledger::redeem_void(
            signed(ALICE),
            1,
            Branch::Accept,
            PositionKind::BranchUsdc,
            a
        ));
        assert_eq!(usdc(ALICE) - a_bal, a / 2 + kernel::POSITION_DEPOSIT_USDC);
        let b_bal = usdc(BOB);
        assert_ok!(Ledger::redeem_void(
            signed(BOB),
            1,
            Branch::Reject,
            PositionKind::BranchUsdc,
            a
        ));
        assert_eq!(usdc(BOB) - b_bal, a / 2 + kernel::POSITION_DEPOSIT_USDC);
        // total paid = a (=100_000), never 2·a.
        assert_eq!(escrow(1), 0);
        try_state();
    });
}

#[test]
fn void_leg_pays_quarter() {
    new_test_ext().execute_with(|| {
        create(1);
        let a = 4 * UNIT;
        assert_ok!(Ledger::split(signed(ALICE), 1, a));
        assert_ok!(Ledger::split_gate(
            signed(ALICE),
            1,
            Branch::Accept,
            GateType::Survival,
            a
        ));
        assert_ok!(Ledger::void(signed(RESOLVER), 1));
        let before = usdc(ALICE);
        assert_ok!(Ledger::redeem_void(
            signed(ALICE),
            1,
            Branch::Accept,
            PositionKind::GateYes(GateType::Survival),
            a
        ));
        // gate leg pays floor(a/4).
        assert_eq!(usdc(ALICE) - before, a / 4 + kernel::POSITION_DEPOSIT_USDC);
        try_state();
    });
}

// --------------------------------------------- state-machine tightenings (§2.3)

#[test]
fn no_unpaired_redeem_before_settlement() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        // redeem requires ScalarSettled, not Open/Resolved (03 §2.3 outflow lockout).
        assert_noop!(
            Ledger::redeem(signed(ALICE), 1, 10 * UNIT),
            E::WrongVaultState
        );
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_noop!(
            Ledger::redeem(signed(ALICE), 1, 10 * UNIT),
            E::WrongVaultState
        );
    });
}

#[test]
fn void_barred_from_scalar_settled() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_ok!(Ledger::settle_scalar(
            signed(SETTLER),
            1,
            FixedU64(500_000_000)
        ));
        assert_noop!(Ledger::void(signed(RESOLVER), 1), E::WrongVaultState);
    });
}

#[test]
fn resolve_once_only() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_noop!(
            Ledger::resolve(signed(RESOLVER), 1, Branch::Reject),
            E::WrongVaultState
        );
    });
}

// -------------------------------------------------------- origin-misuse matrix

#[test]
fn authority_calls_reject_wrong_origin() {
    new_test_ext().execute_with(|| {
        create(1);
        // signed user cannot drive authority transitions
        assert_noop!(
            Ledger::resolve(signed(ALICE), 1, Branch::Accept),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Ledger::void(signed(ALICE), 1),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Ledger::settle_scalar(signed(ALICE), 1, FixedU64(0)),
            sp_runtime::DispatchError::BadOrigin
        );
        // wrong authority (settler cannot resolve; resolver cannot settle)
        assert_noop!(
            Ledger::resolve(signed(SETTLER), 1, Branch::Accept),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_noop!(
            Ledger::settle_scalar(signed(RESOLVER), 1, FixedU64(0)),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn internal_api_reachable_only_by_market_authority() {
    new_test_ext().execute_with(|| {
        // create_vault + do_split require MarketAuthority; a signed user is rejected.
        assert_noop!(
            Ledger::create_vault(signed(ALICE), 1, 0),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(Ledger::create_vault(signed(MARKET), 1, 0));
        assert_noop!(
            Ledger::do_split(signed(ALICE), 1, BOB, UNIT),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(Ledger::do_split(signed(MARKET), 1, BOOK, UNIT));
        try_state();
    });
}

#[test]
fn split_requires_signed() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_noop!(
            Ledger::split(RawOrigin::None.into(), 1, UNIT),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

// ----------------------------------------------------- bounds / cap coverage

#[test]
fn position_cap_enforced_for_non_protocol_atomic() {
    new_test_ext().execute_with(|| {
        // 32 vaults × split = 64 positions for ALICE = the cap.
        for pid in 0..32u64 {
            create(pid);
            assert_ok!(Ledger::split(signed(ALICE), pid, UNIT));
        }
        assert_eq!(crate::PositionCount::<Test>::get(ALICE), 64);
        create(999);
        // 65th/66th entry rejected and the whole split rolls back atomically.
        assert_noop!(Ledger::split(signed(ALICE), 999, UNIT), E::TooManyPositions);
        assert_eq!(escrow(999), 0);
        assert_eq!(crate::PositionCount::<Test>::get(ALICE), 64);
        try_state();
    });
}

#[test]
fn protocol_accounts_exempt_from_cap_and_deposit() {
    new_test_ext().execute_with(|| {
        let deposits_before = DepositsHeld::<Test>::get();
        for pid in 0..40u64 {
            create(pid);
            // book holds branch-USDC well past 64 with no deposit and no cap.
            assert_ok!(Ledger::do_split(signed(MARKET), pid, BOOK, UNIT));
        }
        assert_eq!(crate::PositionCount::<Test>::get(BOOK), 0); // uncounted
        assert_eq!(DepositsHeld::<Test>::get(), deposits_before); // no deposit taken
        try_state();
    });
}

// ------------------------------------------------------------- reaping (§5.4)

#[test]
fn sweep_dust_reaps_after_archive_delay_and_sweeps_residue() {
    new_test_ext().execute_with(|| {
        create(1);
        // ALICE splits, then transfers her whole Reject leg away so an unpaired
        // remainder exists at settlement (leaving residue to sweep).
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        assert_ok!(Ledger::split_scalar(
            signed(ALICE),
            1,
            Branch::Accept,
            10 * UNIT
        ));
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_ok!(Ledger::settle_scalar(
            signed(SETTLER),
            1,
            FixedU64(500_000_000)
        ));
        // Nobody redeems; the vault is terminal at block 1.
        assert!(VaultTerminalAt::<Test>::get(1).is_some());

        // Too early — archive delay not elapsed.
        assert_noop!(Ledger::sweep_dust(signed(ALICE), 1), E::ReapNotDue);

        System::set_block_number(1 + ArchiveDelay::get() + 1);
        let insurance_before = usdc(INSURANCE);
        let alice_deposits = crate::PositionCount::<Test>::get(ALICE);
        assert!(alice_deposits > 0);
        assert_ok!(Ledger::sweep_dust(signed(ALICE), 1));

        // Vault gone, residue swept to INSURANCE, deposits refunded, held zeroed.
        assert!(Vaults::<Test>::get(1).is_none());
        assert!(usdc(INSURANCE) >= insurance_before); // residual escrow → INSURANCE
        assert_eq!(DepositsHeld::<Test>::get(), 0);
        assert!(ledger_events()
            .iter()
            .any(|e| matches!(e, Event::VaultReaped { pid: 1, .. })));
        try_state();
    });
}

#[test]
fn keeper_rebate_is_exactly_once_for_positive_sweep_and_zero_for_error() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_ok!(Ledger::settle_scalar(
            signed(SETTLER),
            1,
            FixedU64(500_000_000),
        ));
        RecordKeeperRebates::set(true);

        assert_noop!(Ledger::sweep_dust(signed(ALICE), 1), E::ReapNotDue);
        assert!(KeeperRebates::get().is_empty());

        System::set_block_number(1 + ArchiveDelay::get() + 1);
        assert_ok!(Ledger::sweep_dust(signed(ALICE), 1));
        assert_eq!(KeeperRebates::get(), vec![(ALICE, CrankClass::General)]);

        assert_noop!(Ledger::sweep_dust(signed(ALICE), 1), E::ReapNotDue);
        assert_eq!(KeeperRebates::get(), vec![(ALICE, CrankClass::General)]);
    });
}

#[test]
fn proposal_sweep_rebates_partial_progress_and_zero_residue_full_reap() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_ok!(Ledger::settle_scalar(
            signed(SETTLER),
            1,
            FixedU64(500_000_000),
        ));
        ReapBatch::set(1);
        RecordKeeperRebates::set(true);
        System::set_block_number(1 + ArchiveDelay::get() + 1);

        assert_ok!(Ledger::sweep_dust(signed(ALICE), 1));
        assert!(Vaults::<Test>::contains_key(1));
        assert_eq!(KeeperRebates::get(), vec![(ALICE, CrankClass::General)]);

        // The second one-entry batch drains the final position and reaps the
        // vault; it is a separate progressing batch and earns one rebate.
        assert_ok!(Ledger::sweep_dust(signed(ALICE), 1));
        assert!(!Vaults::<Test>::contains_key(1));
        assert_eq!(
            KeeperRebates::get(),
            vec![(ALICE, CrankClass::General), (ALICE, CrankClass::General),]
        );

        // Removing an empty terminal vault is real cleanup even with zero
        // escrow residue.
        create(2);
        assert_ok!(Ledger::void(signed(RESOLVER), 2));
        let terminal = System::block_number();
        System::set_block_number(terminal + ArchiveDelay::get() + 1);
        assert_ok!(Ledger::sweep_dust(signed(ALICE), 2));
        assert!(!Vaults::<Test>::contains_key(2));
        assert!(ledger_events()
            .iter()
            .any(|event| matches!(event, Event::VaultReaped { pid: 2, residue: 0 })));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (ALICE, CrankClass::General),
                (ALICE, CrankClass::General),
                (ALICE, CrankClass::General),
            ]
        );
    });
}

#[test]
fn baseline_sweep_has_partial_and_zero_residue_rebate_parity() {
    new_test_ext().execute_with(|| {
        create_base(9);
        assert_ok!(Ledger::split_baseline(signed(ALICE), 9, 5 * UNIT));
        assert_ok!(Ledger::settle_baseline(
            signed(SETTLER),
            9,
            FixedU64(500_000_000),
        ));
        ReapBatch::set(1);
        RecordKeeperRebates::set(true);
        System::set_block_number(1 + ArchiveDelay::get() + 1);

        assert_ok!(Ledger::sweep_dust_baseline(signed(ALICE), 9));
        assert!(crate::BaselineVaults::<Test>::contains_key(9));
        assert_eq!(KeeperRebates::get(), vec![(ALICE, CrankClass::General)]);
        assert_ok!(Ledger::sweep_dust_baseline(signed(ALICE), 9));
        assert!(!crate::BaselineVaults::<Test>::contains_key(9));

        create_base(10);
        assert_ok!(Ledger::settle_baseline(
            signed(SETTLER),
            10,
            FixedU64(500_000_000),
        ));
        let terminal = System::block_number();
        System::set_block_number(terminal + ArchiveDelay::get() + 1);
        assert_ok!(Ledger::sweep_dust_baseline(signed(ALICE), 10));
        assert!(!crate::BaselineVaults::<Test>::contains_key(10));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (ALICE, CrankClass::General),
                (ALICE, CrankClass::General),
                (ALICE, CrankClass::General),
            ]
        );
    });
}

#[test]
fn zero_progress_sweep_and_errors_never_rebate() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_ok!(Ledger::settle_scalar(
            signed(SETTLER),
            1,
            FixedU64(500_000_000),
        ));
        ReapBatch::set(0);
        RecordKeeperRebates::set(true);

        assert_noop!(Ledger::sweep_dust(signed(ALICE), 99), E::ReapNotDue);
        System::set_block_number(1 + ArchiveDelay::get() + 1);
        assert_ok!(Ledger::sweep_dust(signed(ALICE), 1));
        assert!(Vaults::<Test>::contains_key(1));
        assert!(KeeperRebates::get().is_empty());
    });
}

// ------------------------------------------------ differential: pallet ≡ core

#[test]
fn differential_matches_frame_free_core() {
    new_test_ext().execute_with(|| {
        create(1);
        // Drive a sequence through the pallet.
        assert_ok!(Ledger::split(signed(ALICE), 1, 8 * UNIT));
        assert_ok!(Ledger::split_scalar(
            signed(ALICE),
            1,
            Branch::Accept,
            3 * UNIT
        ));
        assert_ok!(Ledger::split_gate(
            signed(ALICE),
            1,
            Branch::Reject,
            GateType::Security,
            2 * UNIT
        ));
        let long = pos(1, Branch::Accept, PositionKind::Long);
        assert_ok!(Ledger::transfer(signed(ALICE), long, BOB, UNIT));
        assert_ok!(Ledger::merge_scalar(signed(ALICE), 1, Branch::Accept, UNIT));

        // Replay the identical sequence on a raw core LedgerState.
        let mut core = LedgerState::<AccountId>::new();
        core.create_vault(1, 0).unwrap();
        core.split(LedgerOrigin::Signed, 1, &ALICE, 8 * UNIT)
            .unwrap();
        core.split_scalar(LedgerOrigin::Signed, 1, Branch::Accept, &ALICE, 3 * UNIT)
            .unwrap();
        core.split_gate(
            LedgerOrigin::Signed,
            1,
            Branch::Reject,
            GateType::Security,
            &ALICE,
            2 * UNIT,
        )
        .unwrap();
        core.transfer(LedgerOrigin::Signed, long, &ALICE, &BOB, UNIT)
            .unwrap();
        core.merge_scalar(LedgerOrigin::Signed, 1, Branch::Accept, &ALICE, UNIT)
            .unwrap();

        // Vault + a representative set of positions must agree exactly.
        let cv = &core.vaults.iter().find(|v| v.proposal == 1).unwrap().info;
        assert_eq!(Vaults::<Test>::get(1).unwrap(), *cv);
        for (who, id) in [
            (ALICE, pos(1, Branch::Accept, PositionKind::BranchUsdc)),
            (ALICE, pos(1, Branch::Accept, PositionKind::Long)),
            (ALICE, pos(1, Branch::Accept, PositionKind::Short)),
            (BOB, pos(1, Branch::Accept, PositionKind::Long)),
            (
                ALICE,
                pos(1, Branch::Reject, PositionKind::GateYes(GateType::Security)),
            ),
        ] {
            let core_bal = core
                .positions
                .iter()
                .find(|p| p.id == id && p.owner == who)
                .map(|p| p.balance)
                .unwrap_or(0);
            assert_eq!(
                Positions::<Test>::get(id, who),
                core_bal,
                "mismatch for {:?}/{:?}",
                who,
                id
            );
        }
        try_state();
    });
}

// ----------------------------------------------------------- error paths

#[test]
fn ops_on_unknown_vault_error() {
    new_test_ext().execute_with(|| {
        assert_noop!(Ledger::split(signed(ALICE), 42, UNIT), E::UnknownVault);
        assert_noop!(
            Ledger::split_baseline(signed(ALICE), 42, UNIT),
            E::UnknownBaselineVault
        );
    });
}

#[test]
fn split_below_min_rejects() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_noop!(
            Ledger::split(signed(ALICE), 1, kernel::MIN_SPLIT_USDC - 1),
            E::BelowMinimum
        );
    });
}

#[test]
fn insufficient_position_rejects() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, UNIT));
        assert_noop!(
            Ledger::merge(signed(ALICE), 1, 2 * UNIT),
            E::InsufficientPosition
        );
    });
}

#[test]
fn settle_scalar_rejects_out_of_range_score() {
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::resolve(signed(RESOLVER), 1, Branch::Accept));
        assert_noop!(
            Ledger::settle_scalar(signed(SETTLER), 1, FixedU64(1_000_000_001)),
            E::InvalidScore
        );
    });
}

// ------------------------------------------------- review regressions (Codex/spec)

#[test]
fn self_transfer_is_a_consistent_noop() {
    // Codex critical: a self-transfer (from == to) must not duplicate the scoped
    // cell or double-refund the deposit.
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 5 * UNIT));
        let id = pos(1, Branch::Accept, PositionKind::BranchUsdc);
        let held = DepositsHeld::<Test>::get();
        let count = crate::PositionCount::<Test>::get(ALICE);
        let usdc_before = usdc(ALICE);
        assert_ok!(Ledger::transfer(signed(ALICE), id, ALICE, 5 * UNIT));
        // Balance, count, held deposits and USDC are all unchanged.
        assert_eq!(Positions::<Test>::get(id, ALICE), 5 * UNIT);
        assert_eq!(crate::PositionCount::<Test>::get(ALICE), count);
        assert_eq!(DepositsHeld::<Test>::get(), held);
        assert_eq!(usdc(ALICE), usdc_before);
        try_state();
    });
}

#[test]
fn min_split_creation_floor_uses_live_value_on_transfer() {
    // spec-reviewer major-1: transfer's creation floor binds the live MinSplit.
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 5 * UNIT));
        let id = pos(1, Branch::Accept, PositionKind::BranchUsdc);
        // A sub-min transfer to a brand-new recipient entry is rejected
        // (mock MinSplit == kernel floor, so use floor − 1).
        assert_noop!(
            Ledger::transfer(signed(ALICE), id, BOB, kernel::MIN_SPLIT_USDC - 1),
            E::BelowMinimum
        );
        // At the floor it is admitted.
        assert_ok!(Ledger::transfer(
            signed(ALICE),
            id,
            BOB,
            kernel::MIN_SPLIT_USDC
        ));
        try_state();
    });
}

#[test]
fn reap_refunds_every_owner_and_keeps_totals_consistent() {
    // Codex #2/#3 + spec blocker: reaping decrements PositionTotals per entry,
    // refunds each owner's deposit, and never strands accounting.
    new_test_ext().execute_with(|| {
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 10 * UNIT));
        // Spread the Accept branch-USDC across three owners.
        let acc = pos(1, Branch::Accept, PositionKind::BranchUsdc);
        assert_ok!(Ledger::transfer(signed(ALICE), acc, BOB, 3 * UNIT));
        assert_ok!(Ledger::transfer(signed(ALICE), acc, CHARLIE, 3 * UNIT));
        assert_ok!(Ledger::void(signed(RESOLVER), 1));
        try_state();

        System::set_block_number(1 + ArchiveDelay::get() + 1);
        let (a0, b0, c0) = (usdc(ALICE), usdc(BOB), usdc(CHARLIE));
        assert_ok!(Ledger::sweep_dust(signed(ALICE), 1));

        // Every owner got their per-entry deposit(s) back; vault + totals gone.
        assert!(usdc(ALICE) > a0 && usdc(BOB) > b0 && usdc(CHARLIE) > c0);
        assert!(Vaults::<Test>::get(1).is_none());
        assert_eq!(Positions::<Test>::get(acc, BOB), 0);
        assert_eq!(crate::PositionTotals::<Test>::get(acc), 0);
        assert_eq!(DepositsHeld::<Test>::get(), 0);
        try_state();
    });
}

#[test]
fn scalar_and_gate_splits_enforce_live_min_split() {
    // Codex P2 (03 §7 R-2): `split_scalar`/`split_gate` mint new LONG/SHORT/gate
    // entries and so are bound by the creation floor at the LIVE `ledger.min_split`
    // — not just the stale kernel floor the core guards. Raise the tunable to its
    // 1-USDC ceiling (13 §1) and confirm sub-floor scalar/gate splits are rejected
    // even though they clear the 0.01-USDC kernel floor.
    new_test_ext().execute_with(|| {
        MinSplit::set(UNIT); // live floor 1 USDC ≫ kernel floor 0.01 USDC
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 5 * UNIT));
        // Above the kernel floor (0.5 USDC > 0.01) but below the live floor: reject.
        assert_noop!(
            Ledger::split_scalar(signed(ALICE), 1, Branch::Accept, UNIT / 2),
            E::BelowMinimum
        );
        assert_noop!(
            Ledger::split_gate(
                signed(ALICE),
                1,
                Branch::Accept,
                GateType::Security,
                UNIT / 2
            ),
            E::BelowMinimum
        );
        // At the live floor both are admitted.
        assert_ok!(Ledger::split_scalar(signed(ALICE), 1, Branch::Accept, UNIT));
        assert_ok!(Ledger::split_gate(
            signed(ALICE),
            1,
            Branch::Accept,
            GateType::Security,
            UNIT
        ));
        try_state();
    });
}

#[test]
fn transfer_remainder_sweep_uses_live_min_split() {
    // Codex P2 (03 §7 R-2): the remainder sweep binds the LIVE `ledger.min_split`,
    // not the stale kernel floor. With the tunable at 1 USDC, a Signed transfer
    // leaving a 0.5-USDC remainder (well above the 0.01-USDC kernel floor, so the
    // core alone would NOT sweep) must still move the whole balance rather than
    // strand sub-floor dust; a remainder exactly at the floor is left in place.
    new_test_ext().execute_with(|| {
        MinSplit::set(UNIT); // 1 USDC live floor; kernel floor is 0.01 USDC
        create(1);
        assert_ok!(Ledger::split(signed(ALICE), 1, 5 * UNIT));
        // Send 4.5 of 5 USDC → remainder 0.5 < live floor → sweep the whole 5.
        let acc = pos(1, Branch::Accept, PositionKind::BranchUsdc);
        assert_ok!(Ledger::transfer(signed(ALICE), acc, BOB, 9 * UNIT / 2));
        assert_eq!(Positions::<Test>::get(acc, ALICE), 0);
        assert_eq!(Positions::<Test>::get(acc, BOB), 5 * UNIT);
        // Send 4 of 5 USDC → remainder 1 == live floor (strict "below") → no sweep.
        let rej = pos(1, Branch::Reject, PositionKind::BranchUsdc);
        assert_ok!(Ledger::transfer(signed(ALICE), rej, CHARLIE, 4 * UNIT));
        assert_eq!(Positions::<Test>::get(rej, ALICE), UNIT);
        assert_eq!(Positions::<Test>::get(rej, CHARLIE), 4 * UNIT);
        try_state();
    });
}
