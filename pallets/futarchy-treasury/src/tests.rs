//! 15 §4.1 suite for `pallet-futarchy-treasury`: per-extrinsic × error-path ×
//! origin-misuse × limit coverage, NAV/reserve-haircut fail-static, the rule-4
//! Params-injection proof, a `try_state` assertion, and a seeded shell-vs-core
//! differential (Python M3 ≡ Rust core ≡ this pallet at default parameters).

use crate::mock::*;
use crate::{Error, Event, PayoutLine};
use frame_support::{assert_err, assert_noop, assert_ok};
use futarchy_primitives::keeper::CrankClass;
use futarchy_treasury_core::{
    AssetKind, BudgetLine, Stream, Treasury as CoreTreasury, DAYS_365_BLOCKS, DAY_BLOCKS,
    MAX_STREAMS, TRS_CAP_PROPOSAL_BPS, TRS_STREAM_THRESHOLD_BPS, USDC, VIT,
};

const MAIN0: u128 = 25_000_000 * USDC;

fn to() -> RuntimeOrigin {
    RuntimeOrigin::signed(treasury_acc())
}

fn note_quote(period_index: u32, price: u128) -> frame_support::dispatch::DispatchResult {
    Treasury::note_coretime_quote(
        RuntimeOrigin::signed(coretime_quote_authority()),
        period_index,
        price,
    )
}

/// Genesis-funded `MAIN` (25M USDC) with three lines pre-funded via the
/// extrinsic (the realistic post-XCM funding path, 08 §2.5).
fn funded_ext() -> sp_io::TestExternalities {
    let mut ext = new_test_ext_with(crate::GenesisConfig::<Test> {
        main_usdc: MAIN0,
        coretime_quote_authority: Some(coretime_quote_authority()),
        coretime_renewal_account: Some([44; 32]),
        ..Default::default()
    });
    ext.execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::OpsCollators,
            5_000_000 * USDC
        ));
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Rewards,
            2_000_000 * USDC
        ));
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::OpsCoretime,
            1_000_000 * USDC
        ));
    });
    ext
}

// ---- genesis (08 §2.1) ------------------------------------------------------

#[test]
fn default_genesis_is_empty_and_solvent() {
    new_test_ext().execute_with(|| {
        let t = crate::Pallet::<Test>::treasury();
        assert_eq!(t.main_usdc, 0);
        assert_eq!(t.vit_supply, futarchy_treasury_core::DEFAULT_VIT_SUPPLY);
        assert!(t.lines.is_empty());
        assert_eq!(t.next_stream_id, 0);
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

// ---- origins (08 §1.1, rule 6) -----------------------------------------------

#[test]
fn outflow_calls_admit_only_the_treasury_origin() {
    funded_ext().execute_with(|| {
        for bad in [RuntimeOrigin::signed(nobody()), RuntimeOrigin::root()] {
            assert_noop!(
                Treasury::spend(bad.clone(), BudgetLine::OpsCollators, acc(1), 1),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_noop!(
                Treasury::fund_budget_line(bad.clone(), BudgetLine::Keeper, 1),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_noop!(
                Treasury::open_stream(bad.clone(), BudgetLine::Rewards, acc(1), 1, 0, 1),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_noop!(
                Treasury::cancel_stream(bad.clone(), 0),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_noop!(
                Treasury::issue_vit(bad.clone(), 1, BudgetLine::Rewards),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_noop!(
                Treasury::recover_foreign(bad, AssetKind::Foreign([1u8; 32]), acc(1), 1),
                sp_runtime::DispatchError::BadOrigin
            );
        }
    });
}

#[test]
fn claim_and_renewal_are_signed_permissionless_not_treasury_gated() {
    funded_ext().execute_with(|| {
        // Both are Signed calls: an unknown period / stream errors on state, not
        // on origin — proving they are permissionless, not FutarchyTreasury-only.
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(nobody()), 7),
            Error::<Test>::RenewalWindowClosed
        );
        assert_noop!(
            Treasury::claim_stream(RuntimeOrigin::signed(nobody()), 0),
            Error::<Test>::StreamNotFound
        );
        // Root is not a signed origin.
        assert_noop!(
            Treasury::claim_stream(RuntimeOrigin::root(), 0),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

// ---- fund_budget_line / spend (08 §1.1/§1.3) --------------------------------

#[test]
fn fund_budget_line_moves_main_into_the_line() {
    funded_ext().execute_with(|| {
        let before = Treasury::line_balance(BudgetLine::Keeper);
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            100_000 * USDC
        ));
        assert_eq!(
            Treasury::line_balance(BudgetLine::Keeper),
            before + 100_000 * USDC
        );
        System::assert_last_event(RuntimeEvent::Treasury(Event::BudgetLineFunded {
            line: BudgetLine::Keeper,
            amount: 100_000 * USDC,
        }));
        // NAV is invariant under funding (main − x, line + x).
        assert_eq!(Treasury::nav().nav, MAIN0);
    });
}

#[test]
fn pot_backed_budget_lines_sync_exact_funding_to_custody() {
    funded_ext().execute_with(|| {
        let keeper_before = Treasury::line_balance(BudgetLine::Keeper);
        let oracle_before = Treasury::line_balance(BudgetLine::Oracle);

        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            50 * USDC
        ));
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Oracle,
            30 * USDC
        ));

        assert_eq!(
            pot_funding_calls(),
            vec![
                (PayoutLine::Keeper, 50 * USDC),
                (PayoutLine::Oracle, 30 * USDC),
            ]
        );
        assert_eq!(
            Treasury::line_balance(BudgetLine::Keeper),
            keeper_before + 50 * USDC
        );
        assert_eq!(
            Treasury::line_balance(BudgetLine::Oracle),
            oracle_before + 30 * USDC
        );
        assert_eq!(KeeperRebatePotBalance::get(), 50 * USDC);
        assert_eq!(OracleRebatePotBalance::get(), 30 * USDC);
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

#[test]
fn pot_funding_failure_rolls_back_internal_credit_and_event() {
    funded_ext().execute_with(|| {
        let main_before = crate::Pallet::<Test>::treasury().main_usdc;
        let line_before = Treasury::line_balance(BudgetLine::Keeper);
        set_pot_funding_failure(true);
        System::reset_events();

        assert_noop!(
            Treasury::fund_budget_line(to(), BudgetLine::Keeper, 50 * USDC),
            sp_runtime::DispatchError::Other("pot funding failed")
        );

        assert_eq!(crate::Pallet::<Test>::treasury().main_usdc, main_before);
        assert_eq!(Treasury::line_balance(BudgetLine::Keeper), line_before);
        assert!(System::events().is_empty());
        assert_eq!(pot_funding_calls(), vec![(PayoutLine::Keeper, 50 * USDC)]);
    });
}

#[test]
fn non_pot_budget_lines_credit_without_custody_calls() {
    funded_ext().execute_with(|| {
        let cases = [
            (BudgetLine::Pol, 11 * USDC),
            (BudgetLine::OpsCoretime, 12 * USDC),
            (BudgetLine::Rewards, 13 * USDC),
        ];
        let before = cases
            .iter()
            .map(|(line, _)| (*line, Treasury::line_balance(*line)))
            .collect::<Vec<_>>();

        for (line, amount) in cases {
            assert_ok!(Treasury::fund_budget_line(to(), line, amount));
        }

        assert!(pot_funding_calls().is_empty());
        for ((line, amount), (_, old_balance)) in cases.into_iter().zip(before) {
            assert_eq!(Treasury::line_balance(line), old_balance + amount);
        }
    });
}

#[test]
fn zero_funding_keeps_core_bookkeeping_without_custody_movement() {
    funded_ext().execute_with(|| {
        let main_before = crate::Pallet::<Test>::treasury().main_usdc;
        System::reset_events();

        assert_ok!(Treasury::fund_budget_line(to(), BudgetLine::Keeper, 0));

        assert_eq!(crate::Pallet::<Test>::treasury().main_usdc, main_before);
        assert_eq!(Treasury::line_balance(BudgetLine::Keeper), 0);
        assert!(crate::Pallet::<Test>::treasury()
            .lines
            .contains(&(BudgetLine::Keeper, 0)));
        assert!(pot_funding_calls().is_empty());
        assert_eq!(KeeperRebatePotBalance::get(), 0);
        System::assert_last_event(RuntimeEvent::Treasury(Event::BudgetLineFunded {
            line: BudgetLine::Keeper,
            amount: 0,
        }));
    });
}

#[test]
fn spend_enforces_stream_threshold_cap_and_line_balance() {
    // limit-coverage: trs.stream_thr
    funded_ext().execute_with(|| {
        // > 1% NAV (250k) must stream, not spend.
        assert_noop!(
            Treasury::spend(to(), BudgetLine::OpsCollators, acc(1), 300_000 * USDC),
            Error::<Test>::StreamRequired
        );
        // Unknown line.
        assert_noop!(
            Treasury::spend(to(), BudgetLine::Oracle, acc(1), 1),
            Error::<Test>::UnknownBudgetLine
        );
        // A valid in-cap grant pays out and debits the line.
        let before = Treasury::line_balance(BudgetLine::OpsCollators);
        assert_ok!(Treasury::spend(
            to(),
            BudgetLine::OpsCollators,
            acc(1),
            100_000 * USDC
        ));
        assert_eq!(
            Treasury::line_balance(BudgetLine::OpsCollators),
            before - 100_000 * USDC
        );
        System::assert_last_event(RuntimeEvent::Treasury(Event::Spent {
            line: BudgetLine::OpsCollators,
            dest: acc(1),
            amount: 100_000 * USDC,
        }));
    });
}

// ---- reserve haircut fail-static (08 §1.2) ----------------------------------

#[test]
fn reserve_haircut_zeroes_spendable_nav_and_blocks_new_commitments() {
    funded_ext().execute_with(|| {
        // The haircut event is stamped with the live epoch from Config::CurrentEpoch.
        set_epoch(5);
        crate::Pallet::<Test>::set_reserve_impaired(true);
        System::assert_last_event(RuntimeEvent::Treasury(Event::NavHaircutFlagged {
            epoch: 5,
            flag: true,
        }));
        let nav = Treasury::nav();
        assert!(nav.reserve_impaired);
        assert_eq!(nav.spendable_nav, 0);

        // No new spends / streams / issuance-independent outflows.
        assert_noop!(
            Treasury::spend(to(), BudgetLine::OpsCollators, acc(1), 1),
            Error::<Test>::ReserveImpaired
        );
        assert_noop!(
            Treasury::open_stream(
                to(),
                BudgetLine::OpsCollators,
                acc(1),
                300_000 * USDC,
                0,
                100
            ),
            Error::<Test>::ReserveImpaired
        );
        // Every arming floor fails static (loud event ⇒ `assert_err`).
        assert_err!(
            crate::Pallet::<Test>::ensure_nav_floor(futarchy_primitives::ProposalClass::Param),
            Error::<Test>::NavFloorUnmet
        );

        // The full Coretime liveness sequence stays alive (D-9 freeze-exempt).
        assert_ok!(note_quote(1, 100_000 * USDC));
        assert_ok!(Treasury::prune_coretime_quote(
            RuntimeOrigin::signed(coretime_quote_authority()),
            1,
        ));
        assert_ok!(note_quote(1, 100_000 * USDC));
        assert_ok!(Treasury::execute_coretime_renewal(
            RuntimeOrigin::signed(acc(8)),
            1
        ));
    });
}

// ---- streams (08 §1.3) ------------------------------------------------------

#[test]
fn streams_are_mandatory_claimable_and_cancellable() {
    funded_ext().execute_with(|| {
        // Below threshold ⇒ a stream is not allowed (use spend).
        assert_noop!(
            Treasury::open_stream(
                to(),
                BudgetLine::OpsCollators,
                acc(2),
                10_000 * USDC,
                0,
                100
            ),
            Error::<Test>::StreamRequired
        );
        // A valid mandatory stream.
        assert_ok!(Treasury::open_stream(
            to(),
            BudgetLine::OpsCollators,
            acc(2),
            300_000 * USDC,
            10,
            100
        ));
        // id 0, half vested at block 60 (start 10, duration 100).
        System::set_block_number(60);
        assert_noop!(
            Treasury::claim_stream(RuntimeOrigin::signed(acc(9)), 0),
            Error::<Test>::NotRecipient
        );
        assert_ok!(Treasury::claim_stream(RuntimeOrigin::signed(acc(2)), 0));
        System::assert_last_event(RuntimeEvent::Treasury(Event::StreamClaimed {
            id: 0,
            recipient: acc(2),
            amount: 150_000 * USDC,
        }));
        // Cancel reverts the undisbursed remainder to MAIN.
        assert_ok!(Treasury::cancel_stream(to(), 0));
        assert_noop!(
            Treasury::claim_stream(RuntimeOrigin::signed(acc(2)), 0),
            Error::<Test>::AlreadyCancelled
        );
    });
}

// ---- issuance meter (08 §2.3) -----------------------------------------------

#[test]
fn issuance_is_line_scoped_and_capped_at_two_percent() {
    // limit-coverage: iss.inflation
    funded_ext().execute_with(|| {
        assert_noop!(
            Treasury::issue_vit(to(), 1, BudgetLine::Pol),
            Error::<Test>::IssuanceLineNotAllowed
        );
        let cap = 20_000_000 * VIT; // 2% of 1e9 VIT
        assert_ok!(Treasury::issue_vit(to(), cap, BudgetLine::Rewards));
        assert_eq!(Treasury::vit_line_balance(BudgetLine::Rewards), cap);
        assert_noop!(
            Treasury::issue_vit(to(), 1, BudgetLine::Rewards),
            Error::<Test>::IssuanceCapExceeded
        );
        // Rolling window: at the 365-day seam the day-0 mint is STILL counted,
        // so a fresh full mint is refused (fixed-window doubling closed).
        System::set_block_number(u64::from(DAYS_365_BLOCKS));
        assert_noop!(
            Treasury::issue_vit(to(), 1, BudgetLine::OpsArweave),
            Error::<Test>::IssuanceCapExceeded
        );
        // One day later the day-0 mint has rolled off; capacity returns.
        System::set_block_number(u64::from(DAYS_365_BLOCKS) + u64::from(DAY_BLOCKS));
        assert_ok!(Treasury::issue_vit(to(), 1, BudgetLine::OpsArweave));
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

// ---- coretime renewal (09 §4) -----------------------------------------------

#[test]
fn absent_keeper_rebate_param_is_a_structural_noop() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            100 * USDC
        ));
        System::reset_events();
        let before = Treasury::treasury();

        // The mock holds `keeper.rebate` at 0 to exercise the fail-soft
        // no-payout path; SQ-117 seeds a positive value in the runtime, but a
        // zero rebate must still be a safe no-op (no outflow, no payout event).
        assert_eq!(KeeperRebate::get(), 0);
        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);

        assert_eq!(Treasury::treasury(), before);
        assert!(rebate_payouts().is_empty());
        assert!(System::events().is_empty());
    });
}

#[test]
fn keeper_and_oracle_rebates_pay_from_the_selected_lines() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            100 * USDC
        ));
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Oracle,
            100 * USDC
        ));
        KeeperBudgetEpoch::set(100 * USDC);
        KeeperRebate::set(10 * USDC);
        reset_rebate_payout();
        set_rebate_pot_balance(PayoutLine::Keeper, 100 * USDC);
        set_rebate_pot_balance(PayoutLine::Oracle, 100 * USDC);

        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::General);
        let metered = Treasury::treasury().keeper_meter;
        assert_eq!(metered.spent, 10 * USDC);
        assert_eq!(metered.general_spent, 10 * USDC);

        crate::Pallet::<Test>::do_keeper_rebate(&acc(8), CrankClass::OracleLine);
        assert_eq!(Treasury::treasury().keeper_meter, metered);
        assert_eq!(
            rebate_payouts(),
            vec![
                (acc(7), 10 * USDC, PayoutLine::Keeper),
                (acc(8), 10 * USDC, PayoutLine::Oracle),
            ]
        );
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

#[test]
fn payout_failure_drops_line_meter_and_events() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            100 * USDC
        ));
        KeeperBudgetEpoch::set(100 * USDC);
        KeeperRebate::set(80 * USDC);
        System::reset_events();
        reset_rebate_payout();
        set_rebate_pot_balance(PayoutLine::Keeper, 100 * USDC);
        set_rebate_payout_failure(true);
        let before = Treasury::treasury();

        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);

        assert_eq!(Treasury::treasury(), before);
        assert_eq!(
            rebate_payouts(),
            vec![(acc(7), 80 * USDC, PayoutLine::Keeper)]
        );
        assert!(System::events().is_empty());
    });
}

#[test]
fn threshold_events_map_and_zero_pay_exhaustion_flag_persists_once() {
    // limit-coverage: keeper.budget
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            200 * USDC
        ));
        KeeperBudgetEpoch::set(100 * USDC);
        KeeperRebate::set(20 * USDC);
        System::reset_events();
        reset_rebate_payout();
        set_rebate_pot_balance(PayoutLine::Keeper, 200 * USDC);

        for _ in 0..4 {
            crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);
        }
        System::assert_last_event(RuntimeEvent::Treasury(Event::KeeperBudgetLow {
            remaining: 20 * USDC,
        }));
        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);
        System::assert_last_event(RuntimeEvent::Treasury(Event::KeeperBudgetExhausted {
            epoch: 0,
            spent: 100 * USDC,
        }));
        let event_count = System::events().len();
        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);
        assert_eq!(System::events().len(), event_count);
        assert_eq!(rebate_payouts().len(), 5);
    });
}

#[test]
fn shrunken_budget_alarms_low_then_exhausted_and_latches_rebates() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            100 * USDC
        ));
        KeeperBudgetEpoch::set(100 * USDC);
        KeeperRebate::set(20 * USDC);
        reset_rebate_payout();
        set_rebate_pot_balance(PayoutLine::Keeper, 100 * USDC);

        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);
        System::reset_events();

        // A governance shrink makes already-spent capacity effectively
        // exhausted. The mandatory 80% alarm is emitted first.
        KeeperBudgetEpoch::set(10 * USDC);
        KeeperRebate::set(USDC);
        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);
        assert_eq!(
            System::events()
                .into_iter()
                .map(|record| record.event)
                .collect::<Vec<_>>(),
            vec![
                RuntimeEvent::Treasury(Event::KeeperBudgetLow { remaining: 0 }),
                RuntimeEvent::Treasury(Event::KeeperBudgetExhausted {
                    epoch: 0,
                    spent: 20 * USDC,
                }),
            ]
        );

        // Restoring budget headroom and retaining the smaller rebate parameter
        // cannot reopen the meter after its per-epoch exhaustion latch fired.
        KeeperBudgetEpoch::set(100 * USDC);
        crate::Pallet::<Test>::do_keeper_rebate(&acc(7), CrankClass::DecisionCritical);
        assert_eq!(rebate_payouts().len(), 1);
        assert_eq!(Treasury::treasury().keeper_meter.spent, 20 * USDC);
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

#[test]
fn successful_coretime_renewal_self_rebates_the_keeper_once() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            100 * USDC
        ));
        KeeperBudgetEpoch::set(100 * USDC);
        KeeperRebate::set(10 * USDC);
        reset_rebate_payout();
        set_rebate_pot_balance(PayoutLine::Keeper, 100 * USDC);
        assert_ok!(note_quote(77, 100_000 * USDC));

        assert_ok!(Treasury::execute_coretime_renewal(
            RuntimeOrigin::signed(acc(7)),
            77
        ));

        assert_eq!(
            rebate_payouts(),
            vec![(acc(7), 10 * USDC, PayoutLine::Keeper)]
        );
        assert_eq!(Treasury::treasury().keeper_meter.general_spent, 10 * USDC);
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

mod renewal_dispatch_seam {
    use super::*;
    use crate as pallet_futarchy_treasury;
    use frame_support::{derive_impl, parameter_types};
    use sp_core::crypto::AccountId32;
    use sp_runtime::{traits::IdentityLookup, BuildStorage, DispatchError};
    use std::cell::{Cell, RefCell};

    type Block = frame_system::mocking::MockBlock<DispatchTest>;

    frame_support::construct_runtime!(
        pub enum DispatchTest {
            System: frame_system,
            Treasury: pallet_futarchy_treasury,
        }
    );

    #[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
    impl frame_system::Config for DispatchTest {
        type Block = Block;
        type AccountId = AccountId32;
        type Lookup = IdentityLookup<AccountId32>;
    }

    pub struct DispatchParams;

    impl pallet_futarchy_treasury::TreasuryParams for DispatchParams {
        fn cap_proposal_bps() -> u32 {
            TRS_CAP_PROPOSAL_BPS
        }

        fn cap_30d_bps() -> u32 {
            futarchy_treasury_core::TRS_CAP_30D_BPS
        }

        fn cap_180d_bps() -> u32 {
            futarchy_treasury_core::TRS_CAP_180D_BPS
        }

        fn stream_threshold_bps() -> u32 {
            TRS_STREAM_THRESHOLD_BPS
        }

        fn inflation_cap_bps() -> u32 {
            futarchy_treasury_core::ISS_INFLATION_CAP_BPS
        }

        fn keeper_budget_epoch() -> u128 {
            futarchy_treasury_core::KEEPER_BUDGET_EPOCH
        }

        fn keeper_rebate() -> u128 {
            0
        }

        fn coretime_dot_rate() -> u128 {
            10_000_000_000
        }

        fn coretime_fee_dot() -> u128 {
            100
        }

        fn coretime_quote_ttl() -> u32 {
            100
        }
    }

    std::thread_local! {
        static DISPATCHED: RefCell<Vec<(u32, u128)>> = const { RefCell::new(Vec::new()) };
        static FAIL_DISPATCH: Cell<bool> = const { Cell::new(false) };
    }

    pub struct RecordingRenewalDispatch;

    impl pallet_futarchy_treasury::RenewalDispatch for RecordingRenewalDispatch {
        fn dispatch_renewal(
            period_index: u32,
            amount: u128,
        ) -> frame_support::dispatch::DispatchResult {
            DISPATCHED.with(|calls| calls.borrow_mut().push((period_index, amount)));
            if FAIL_DISPATCH.with(Cell::get) {
                Err(DispatchError::Other("renewal dispatch failed"))
            } else {
                Ok(())
            }
        }
    }

    parameter_types! {
        pub const CurrentEpoch: u32 = 0;
    }

    impl pallet_futarchy_treasury::Config for DispatchTest {
        type TreasuryOrigin = frame_system::EnsureRoot<AccountId32>;
        type Params = DispatchParams;
        type CurrentEpoch = CurrentEpoch;
        type RenewalDispatch = RecordingRenewalDispatch;
        type RebatePayout = ();
        type PotFunding = ();
        type WeightInfo = ();
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper = DispatchBenchmarkHelper;
    }

    #[cfg(feature = "runtime-benchmarks")]
    pub struct DispatchBenchmarkHelper;

    #[cfg(feature = "runtime-benchmarks")]
    impl pallet_futarchy_treasury::BenchmarkHelper<RuntimeOrigin, AccountId32>
        for DispatchBenchmarkHelper
    {
        fn treasury_origin() -> RuntimeOrigin {
            RuntimeOrigin::root()
        }

        fn account(seed: u8) -> AccountId32 {
            AccountId32::new([seed; 32])
        }
    }

    fn new_ext() -> sp_io::TestExternalities {
        let storage = RuntimeGenesisConfig {
            system: Default::default(),
            treasury: pallet_futarchy_treasury::GenesisConfig {
                main_usdc: MAIN0,
                coretime_quote_authority: Some(AccountId32::new([42; 32])),
                coretime_renewal_account: Some([44; 32]),
                ..Default::default()
            },
        }
        .build_storage()
        .expect("renewal-dispatch test genesis must build");
        let mut ext = sp_io::TestExternalities::new(storage);
        ext.execute_with(|| {
            System::set_block_number(1);
            assert_ok!(Treasury::fund_budget_line(
                RuntimeOrigin::root(),
                BudgetLine::OpsCoretime,
                1_000_000 * USDC,
            ));
            DISPATCHED.with(|calls| calls.borrow_mut().clear());
            FAIL_DISPATCH.with(|fail| fail.set(false));
        });
        ext
    }

    #[test]
    fn renewal_dispatch_receives_the_committed_period_and_quote() {
        new_ext().execute_with(|| {
            let price = 100_000 * USDC;
            assert_ok!(Treasury::note_coretime_quote(
                RuntimeOrigin::signed(AccountId32::new([42; 32])),
                42,
                price,
            ));

            assert_ok!(Treasury::execute_coretime_renewal(
                RuntimeOrigin::signed(AccountId32::new([7; 32])),
                42,
            ));

            DISPATCHED.with(|calls| assert_eq!(&*calls.borrow(), &[(42, price)]));
            let state = Treasury::treasury();
            assert!(state.funded_coretime_periods.contains(&42));
            assert!(!state
                .coretime_quotes
                .iter()
                .any(|quote| quote.period_index == 42));
        });
    }

    #[test]
    fn renewal_dispatch_error_rolls_back_accounting_for_retry() {
        new_ext().execute_with(|| {
            let price = 100_000 * USDC;
            assert_ok!(Treasury::note_coretime_quote(
                RuntimeOrigin::signed(AccountId32::new([42; 32])),
                42,
                price,
            ));
            let line_before = Treasury::line_balance(BudgetLine::OpsCoretime);
            System::reset_events();
            FAIL_DISPATCH.with(|fail| fail.set(true));

            assert_err!(
                Treasury::execute_coretime_renewal(
                    RuntimeOrigin::signed(AccountId32::new([7; 32])),
                    42,
                ),
                DispatchError::Other("renewal dispatch failed")
            );

            DISPATCHED.with(|calls| assert_eq!(&*calls.borrow(), &[(42, price)]));
            let state = Treasury::treasury();
            assert_eq!(Treasury::line_balance(BudgetLine::OpsCoretime), line_before);
            assert!(state.coretime_quotes.iter().any(|quote| {
                quote.period_index == 42 && quote.price == price && quote.noted_at == 1
            }));
            assert!(!state.funded_coretime_periods.contains(&42));
            assert!(!System::events().iter().any(|record| {
                matches!(
                    record.event,
                    RuntimeEvent::Treasury(Event::CoretimeRenewalCalled { .. })
                )
            }));
        });
    }
}

#[test]
fn coretime_renewal_is_permissionless_quote_priced_and_idempotent() {
    funded_ext().execute_with(|| {
        // No quote ⇒ window closed.
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 42),
            Error::<Test>::RenewalWindowClosed
        );
        // The XCM dispatcher receives the authority-noted DOT quote, while the
        // USDC budget line pays ceil((quote + fee) * rate / 1 DOT).
        let price = 100_000 * USDC;
        assert_ok!(note_quote(42, price));
        let before = Treasury::line_balance(BudgetLine::OpsCoretime);
        assert_ok!(Treasury::execute_coretime_renewal(
            RuntimeOrigin::signed(acc(7)),
            42
        ));
        assert_eq!(
            Treasury::line_balance(BudgetLine::OpsCoretime),
            before - price - CoretimeFeeDot::get()
        );
        System::assert_last_event(RuntimeEvent::Treasury(Event::CoretimeRenewalCalled {
            line: BudgetLine::OpsCoretime,
            amount: price + CoretimeFeeDot::get(),
        }));
        // Idempotent per period, even against a re-noted quote.
        assert_noop!(note_quote(42, 1), Error::<Test>::PeriodAlreadyFunded);
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(8)), 42),
            Error::<Test>::PeriodAlreadyFunded
        );
        // Bounded by the pre-authorized line balance.
        assert_ok!(note_quote(43, 5_000_000 * USDC));
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(8)), 43),
            Error::<Test>::InsufficientFunds
        );
    });
}

#[test]
fn coretime_quote_authority_can_note_supersede_and_rotate() {
    funded_ext().execute_with(|| {
        assert_noop!(
            Treasury::note_coretime_quote(RuntimeOrigin::signed(acc(7)), 50, 10),
            Error::<Test>::NotQuoteAuthority
        );
        assert_noop!(note_quote(50, 0), Error::<Test>::ZeroQuote);
        assert_ok!(note_quote(50, 10));
        System::set_block_number(9);
        assert_ok!(note_quote(50, 20));
        let quotes = Treasury::treasury().coretime_quotes;
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].period_index, 50);
        assert_eq!(quotes[0].price, 20);
        assert_eq!(quotes[0].noted_at, 9);

        assert_noop!(
            Treasury::set_coretime_authority(RuntimeOrigin::root(), acc(8), [8; 32]),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(Treasury::set_coretime_authority(to(), acc(8), [8; 32]));
        assert_eq!(crate::CoretimeQuoteAuthority::<Test>::get(), Some(acc(8)));
        assert_eq!(crate::CoretimeRenewalAccount::<Test>::get(), Some([8; 32]));
        assert_noop!(note_quote(51, 10), Error::<Test>::NotQuoteAuthority);
        assert_ok!(Treasury::note_coretime_quote(
            RuntimeOrigin::signed(acc(8)),
            51,
            10,
        ));
        crate::CoretimeQuoteAuthority::<Test>::kill();
        let before = Treasury::treasury();
        assert_noop!(
            Treasury::note_coretime_quote(RuntimeOrigin::signed(acc(8)), 52, 10),
            Error::<Test>::NotQuoteAuthority
        );
        assert_eq!(Treasury::treasury(), before);
    });
}

#[test]
fn coretime_prune_enforces_strict_ttl_but_authority_may_prune_early() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            100 * USDC
        ));
        KeeperBudgetEpoch::set(100 * USDC);
        KeeperRebate::set(10 * USDC);
        reset_rebate_payout();
        set_rebate_pot_balance(PayoutLine::Keeper, 100 * USDC);

        assert_ok!(note_quote(50, 10));
        System::set_block_number(101); // age == ttl is still fresh.
        assert_noop!(
            Treasury::prune_coretime_quote(RuntimeOrigin::signed(acc(7)), 50),
            Error::<Test>::QuoteNotExpired
        );
        System::set_block_number(102); // permissionless only when age > ttl.
        assert_ok!(Treasury::prune_coretime_quote(
            RuntimeOrigin::signed(acc(7)),
            50
        ));
        assert_eq!(
            rebate_payouts(),
            vec![(acc(7), 10 * USDC, PayoutLine::Keeper)]
        );
        assert_eq!(Treasury::treasury().keeper_meter.general_spent, 10 * USDC);
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 50),
            Error::<Test>::RenewalWindowClosed
        );

        System::set_block_number(103);
        assert_ok!(note_quote(51, 10));
        assert_ok!(Treasury::prune_coretime_quote(
            RuntimeOrigin::signed(coretime_quote_authority()),
            51,
        ));
        assert_eq!(
            rebate_payouts(),
            vec![(acc(7), 10 * USDC, PayoutLine::Keeper)],
            "the quote authority's anytime prune is not keeper-cranked"
        );
        assert_eq!(Treasury::treasury().keeper_meter.general_spent, 10 * USDC);

        assert_ok!(note_quote(52, 10));
        CoretimeQuoteTtl::set(0);
        assert_noop!(
            Treasury::prune_coretime_quote(RuntimeOrigin::signed(acc(7)), 52),
            Error::<Test>::QuoteTtlUnset
        );
        assert_ok!(Treasury::prune_coretime_quote(
            RuntimeOrigin::signed(coretime_quote_authority()),
            52,
        ));
        assert_eq!(
            rebate_payouts(),
            vec![(acc(7), 10 * USDC, PayoutLine::Keeper)]
        );
    });
}

#[test]
fn coretime_execute_uses_live_params_ceil_and_freshness_without_migration() {
    funded_ext().execute_with(|| {
        let line_before = Treasury::line_balance(BudgetLine::OpsCoretime);
        assert_ok!(note_quote(60, 1));
        let quote_before = Treasury::treasury().coretime_quotes[0];

        // Change both live Params after the quote was stored. No storage
        // migration rewrites the quote; execution consumes the new values.
        CoretimeDotRate::set(5_000_000);
        CoretimeFeeDot::set(100);
        assert_eq!(Treasury::treasury().coretime_quotes[0], quote_before);
        System::set_block_number(101); // age == ttl remains executable.
        assert_ok!(Treasury::execute_coretime_renewal(
            RuntimeOrigin::signed(acc(7)),
            60,
        ));
        // ceil(101 * 5_000_000 / 10_000_000_000) == 1 USDC planck.
        assert_eq!(
            Treasury::line_balance(BudgetLine::OpsCoretime),
            line_before - 1
        );

        System::set_block_number(102);
        assert_ok!(note_quote(61, 1));
        // Move past the new quote's TTL and prove the rejection is fail-static.
        System::set_block_number(203);
        let before = Treasury::treasury();
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 61),
            Error::<Test>::QuoteExpired
        );
        assert_eq!(Treasury::treasury(), before);
    });
}

#[test]
fn coretime_execute_without_destination_is_typed_and_fail_static() {
    funded_ext().execute_with(|| {
        assert_ok!(note_quote(61, 1_000));
        let before = Treasury::treasury();
        crate::CoretimeRenewalAccount::<Test>::kill();

        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 61),
            Error::<Test>::RenewalAccountUnset
        );
        assert_eq!(Treasury::treasury(), before);
    });
}

#[test]
fn coretime_execute_fails_static_on_unset_params_and_future_timestamp() {
    funded_ext().execute_with(|| {
        assert_ok!(note_quote(70, 10));
        let before = Treasury::treasury();
        CoretimeDotRate::set(0);
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 70),
            Error::<Test>::RateUnset
        );
        assert_eq!(Treasury::treasury(), before);

        CoretimeDotRate::set(10_000_000_000);
        CoretimeFeeDot::set(0);
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 70),
            Error::<Test>::FeeBudgetUnset
        );
        assert_eq!(Treasury::treasury(), before);

        CoretimeFeeDot::set(100);
        CoretimeQuoteTtl::set(0);
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 70),
            Error::<Test>::QuoteTtlUnset
        );
        assert_eq!(Treasury::treasury(), before);

        CoretimeQuoteTtl::set(100);
        let mut future = before.clone();
        future.coretime_quotes[0].noted_at = 2;
        crate::Pallet::<Test>::seed(&future);
        System::set_block_number(1);
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 70),
            Error::<Test>::QuoteTimestampInFuture
        );
        assert_eq!(Treasury::treasury(), future);
    });
}

#[test]
fn coretime_obligation_pair_enforces_open_quote_and_funded_history_bounds() {
    funded_ext().execute_with(|| {
        // First prove the funded-period history remains a rolling bound.
        for period in 0..=futarchy_treasury_core::MAX_FUNDED_CORETIME_PERIODS as u32 {
            assert_ok!(note_quote(period, 1));
            assert_ok!(Treasury::execute_coretime_renewal(
                RuntimeOrigin::signed(acc(7)),
                period,
            ));
        }
        let funded = crate::Pallet::<Test>::treasury().funded_coretime_periods;
        assert_eq!(
            funded.len(),
            futarchy_treasury_core::MAX_FUNDED_CORETIME_PERIODS
        );
        assert!(!funded.contains(&0));
        assert!(funded.contains(&(futarchy_treasury_core::MAX_FUNDED_CORETIME_PERIODS as u32)));

        let first_open = 100_u32;
        for offset in 0..futarchy_treasury_core::MAX_FUNDED_CORETIME_PERIODS as u32 {
            assert_ok!(note_quote(first_open.saturating_add(offset), 1));
        }
        // limit-coverage: Treasury coretime obligations
        assert_noop!(
            Treasury::note_coretime_quote(
                RuntimeOrigin::signed(coretime_quote_authority()),
                first_open
                    .saturating_add(futarchy_treasury_core::MAX_FUNDED_CORETIME_PERIODS as u32),
                1,
            ),
            Error::<Test>::TooManyObligations
        );
    });
}

// ---- fund_budget_line atomicity (G-1) ---------------------------------------

#[test]
fn fund_budget_line_is_atomic_on_credit_overflow() {
    funded_ext().execute_with(|| {
        // A line balance near u128::MAX makes the credit overflow; MAIN must not
        // be debited when the credit fails (Codex review).
        let mut t = crate::Pallet::<Test>::treasury();
        t.lines.push((BudgetLine::Oracle, u128::MAX));
        crate::Pallet::<Test>::seed(&t);
        let main_before = crate::Pallet::<Test>::treasury().main_usdc;
        assert_noop!(
            Treasury::fund_budget_line(to(), BudgetLine::Oracle, 1),
            Error::<Test>::Overflow
        );
        assert_eq!(crate::Pallet::<Test>::treasury().main_usdc, main_before);
    });
}

// ---- recover_foreign (08 §1.3) ----------------------------------------------

#[test]
fn recover_foreign_refuses_protocol_assets() {
    funded_ext().execute_with(|| {
        assert_noop!(
            Treasury::recover_foreign(to(), AssetKind::Usdc, acc(1), 1),
            Error::<Test>::UnknownForeignAsset
        );
        assert_noop!(
            Treasury::recover_foreign(to(), AssetKind::Vit, acc(1), 1),
            Error::<Test>::UnknownForeignAsset
        );
        assert_ok!(Treasury::recover_foreign(
            to(),
            AssetKind::Foreign([9u8; 32]),
            acc(3),
            777
        ));
        System::assert_last_event(RuntimeEvent::Treasury(Event::ForeignRecovered {
            asset: AssetKind::Foreign([9u8; 32]),
            dest: acc(3),
            amount: 777,
        }));
    });
}

// ---- minimum-viable-NAV arming gate (08 §4.1/§4.2, loud) --------------------

#[test]
fn nav_floor_gate_is_loud() {
    use futarchy_primitives::ProposalClass;
    // Fund below the CODE floor (~13.9M) but above the gated PARAM floor (~4.62M).
    let mut ext = new_test_ext_with(crate::GenesisConfig::<Test> {
        main_usdc: 5_000_000 * USDC,
        ..Default::default()
    });
    ext.execute_with(|| {
        // Hard gate: above the PARAM floor ⇒ Ok (no event); below the CODE floor
        // ⇒ Err with NO event (a doomed event would roll back with the caller's
        // failed dispatch — Codex review).
        assert_ok!(crate::Pallet::<Test>::ensure_nav_floor(
            ProposalClass::Param
        ));
        assert_err!(
            crate::Pallet::<Test>::ensure_nav_floor(ProposalClass::Code),
            Error::<Test>::NavFloorUnmet
        );
        // Loud variant: below the floor ⇒ deposits the DURABLE NavFloorUnmet
        // (08 §4.2/§4.4 "reject as deferred") and returns true; A8's arming
        // crank calls this on its Ok path so the event survives.
        assert!(crate::Pallet::<Test>::flag_nav_floor(ProposalClass::Code));
        System::assert_last_event(RuntimeEvent::Treasury(Event::NavFloorUnmet {
            class: ProposalClass::Code,
            nav: 5_000_000 * USDC,
            floor: CoreTreasury::floor(ProposalClass::Code),
        }));
        // Above the floor ⇒ returns false, no event.
        assert!(!crate::Pallet::<Test>::flag_nav_floor(ProposalClass::Param));
    });
}

// ---- rolling meters (08 §1.3, I-7) ------------------------------------------

#[test]
fn rolling_30d_meter_binds_spending() {
    // limit-coverage: trs.cap_30d, trs.cap_180d
    // NAV 25M ⇒ trailing-30d ceiling = 10% = 2.5M. Pre-load the meter to just
    // under it so a within-threshold, within-per-proposal-cap spend still trips.
    funded_ext().execute_with(|| {
        let mut t = crate::Pallet::<Test>::treasury();
        t.meter_30d.buckets[0] = 2_400_000 * USDC;
        crate::Pallet::<Test>::seed(&t);
        assert_noop!(
            Treasury::spend(to(), BudgetLine::OpsCollators, acc(1), 200_000 * USDC),
            Error::<Test>::MeterExhausted
        );
        // Within the remaining 100k headroom it is admitted (meter unchanged by
        // the rejected spend above — G-1).
        assert_ok!(Treasury::spend(
            to(),
            BudgetLine::OpsCollators,
            acc(1),
            50_000 * USDC
        ));
    });

    funded_ext().execute_with(|| {
        let mut t = crate::Pallet::<Test>::treasury();
        let nav = t.nav().nav;
        t.meter_180d.buckets[0] =
            nav * u128::from(futarchy_treasury_core::TRS_CAP_180D_BPS) / 10_000;
        crate::Pallet::<Test>::seed(&t);
        assert_noop!(
            Treasury::spend(to(), BudgetLine::OpsCollators, acc(1), USDC),
            Error::<Test>::MeterExhausted
        );
    });
}

// ---- rule 4: caps are read from Params, not hardcoded -----------------------

#[test]
fn caps_track_params_not_a_hardcode() {
    // limit-coverage: trs.cap_proposal
    funded_ext().execute_with(|| {
        // A 300k grant is a valid stream at defaults (> 1% NAV threshold, ≤ 5%
        // NAV cap). Tighten the per-proposal cap to 0.2% via Params ⇒ the same
        // 300k stream is refused, proving the cap is read, not hardcoded.
        CapProposalBps::set(20);
        assert_noop!(
            Treasury::open_stream(
                to(),
                BudgetLine::OpsCollators,
                acc(1),
                300_000 * USDC,
                0,
                100
            ),
            Error::<Test>::ProposalCapExceeded
        );
        CapProposalBps::set(TRS_CAP_PROPOSAL_BPS);
        assert_ok!(Treasury::open_stream(
            to(),
            BudgetLine::OpsCollators,
            acc(1),
            300_000 * USDC,
            0,
            100
        ));

        // Raise the stream threshold to 50% NAV ⇒ 300k now falls below it, so it
        // must be a spend, not a stream (the threshold is read from Params too).
        StreamThresholdBps::set(5_000);
        assert_noop!(
            Treasury::open_stream(
                to(),
                BudgetLine::OpsCollators,
                acc(1),
                300_000 * USDC,
                0,
                100
            ),
            Error::<Test>::StreamRequired
        );
        StreamThresholdBps::set(TRS_STREAM_THRESHOLD_BPS);

        // The issuance cap likewise tracks Params.
        InflationCapBps::set(0);
        assert_noop!(
            Treasury::issue_vit(to(), 1, BudgetLine::Rewards),
            Error::<Test>::IssuanceCapExceeded
        );
    });
}

// ---- storage bounds (13 §4) -------------------------------------------------

#[test]
fn stream_bound_is_enforced() {
    // limit-coverage: Treasury Streams
    funded_ext().execute_with(|| {
        // Seed the stream table to its 13 §4 bound.
        let mut t = crate::Pallet::<Test>::treasury();
        for i in 0..(MAX_STREAMS as u64) {
            t.streams.push(Stream {
                id: i,
                recipient: [1u8; 32],
                line: BudgetLine::Rewards,
                total: USDC,
                claimed: 0,
                start: 0,
                duration: 100,
                cancelled: false,
            });
        }
        t.next_stream_id = MAX_STREAMS as u64;
        crate::Pallet::<Test>::seed(&t);
        // Every seeded stream is live (non-terminal), so none can be reaped and
        // one more open is refused (concurrent bound reached).
        assert_noop!(
            Treasury::open_stream(
                to(),
                BudgetLine::OpsCollators,
                acc(2),
                300_000 * USDC,
                0,
                100
            ),
            Error::<Test>::TooManyStreams
        );
    });
}

#[test]
fn open_stream_reaps_a_terminal_stream_at_the_bound() {
    // The 13 §4 bound is on CONCURRENT open streams (08 §1.3): at the bound, a
    // terminal (cancelled or fully-claimed) stream is reaped to make room, so
    // the lifetime count is unbounded.
    funded_ext().execute_with(|| {
        let mut t = crate::Pallet::<Test>::treasury();
        for i in 0..(MAX_STREAMS as u64) {
            t.streams.push(Stream {
                id: i,
                recipient: [1u8; 32],
                line: BudgetLine::Rewards,
                total: USDC,
                // Make exactly one stream terminal (fully claimed).
                claimed: if i == 3 { USDC } else { 0 },
                start: 0,
                duration: 100,
                cancelled: false,
            });
        }
        t.next_stream_id = MAX_STREAMS as u64;
        crate::Pallet::<Test>::seed(&t);
        // Reaps the fully-claimed stream (id 3) and opens the new one; the table
        // stays at the bound and try_state still holds.
        assert_ok!(Treasury::open_stream(
            to(),
            BudgetLine::OpsCollators,
            acc(2),
            300_000 * USDC,
            0,
            100
        ));
        let after = crate::Pallet::<Test>::treasury();
        assert_eq!(after.streams.len(), MAX_STREAMS);
        assert!(!after.streams.iter().any(|s| s.id == 3));
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

// ---- NAV obligations (08 §1.2) — B1a-wired sync entry points -----------------

#[test]
fn nav_nets_pol_and_pending_obligations() {
    funded_ext().execute_with(|| {
        assert_eq!(crate::Pallet::<Test>::nav().nav, MAIN0);
        // The POL/market and execution-guard sets NAV nets against (08 §1.2).
        assert_ok!(crate::Pallet::<Test>::set_pol_commitments(vec![
            1_000_000 * USDC,
            500_000 * USDC
        ]));
        assert_ok!(crate::Pallet::<Test>::set_pending_outflows(vec![
            250_000 * USDC
        ]));
        assert_eq!(crate::Pallet::<Test>::nav().nav, MAIN0 - 1_750_000 * USDC);
        // Bounded (13 §4): a POL set over MaxLiveMarkets is refused, no-op.
        assert_noop!(
            crate::Pallet::<Test>::set_pol_commitments(vec![
                1;
                futarchy_treasury_core::MAX_POL_COMMITMENTS
                    + 1
            ]),
            Error::<Test>::TooManyObligations
        );
        assert_noop!(
            crate::Pallet::<Test>::set_pending_outflows(vec![
                1;
                futarchy_treasury_core::MAX_PENDING_OUTFLOWS
                    + 1
            ]),
            Error::<Test>::TooManyObligations
        );
        assert_eq!(crate::Pallet::<Test>::nav().nav, MAIN0 - 1_750_000 * USDC);
    });
}

#[test]
fn nav_moves_by_a_stream_exactly_once() {
    // 08 §1.2: opening a stream reduces NAV by the committed remainder EXACTLY
    // once (the open-time line debit; the escrow asset nets the obligation).
    // The differential shares `nav()` on both sides, so this guards the formula
    // directly against the historical 2× double-count.
    funded_ext().execute_with(|| {
        assert_eq!(crate::Pallet::<Test>::nav().nav, MAIN0);
        assert_ok!(Treasury::open_stream(
            to(),
            BudgetLine::OpsCollators,
            acc(2),
            300_000 * USDC,
            0,
            100
        ));
        // Open ⇒ NAV −remainder (once, not twice).
        assert_eq!(crate::Pallet::<Test>::nav().nav, MAIN0 - 300_000 * USDC);
        // Claim of the vested half ⇒ NAV neutral (paying what was already owed).
        System::set_block_number(50);
        assert_ok!(Treasury::claim_stream(RuntimeOrigin::signed(acc(2)), 0));
        assert_eq!(crate::Pallet::<Test>::nav().nav, MAIN0 - 300_000 * USDC);
        // Cancel ⇒ the undisbursed 150k reverts to MAIN, NAV +remainder.
        assert_ok!(Treasury::cancel_stream(to(), 0));
        assert_eq!(crate::Pallet::<Test>::nav().nav, MAIN0 - 150_000 * USDC);
    });
}

// ---- extra error paths (15 §4.1) --------------------------------------------

#[test]
fn error_paths_bad_duration_and_stream_not_claimable() {
    funded_ext().execute_with(|| {
        // Zero-duration stream is rejected.
        assert_noop!(
            Treasury::open_stream(to(), BudgetLine::OpsCollators, acc(2), 300_000 * USDC, 0, 0),
            Error::<Test>::BadDuration
        );
        // A stream claimed before any vesting (now ≤ start) has nothing claimable.
        assert_ok!(Treasury::open_stream(
            to(),
            BudgetLine::OpsCollators,
            acc(2),
            300_000 * USDC,
            100,
            100
        ));
        System::set_block_number(50); // before start (100)
        assert_noop!(
            Treasury::claim_stream(RuntimeOrigin::signed(acc(2)), 0),
            Error::<Test>::StreamNotClaimable
        );
        // Cancelled stream cannot be cancelled again.
        assert_ok!(Treasury::cancel_stream(to(), 0));
        assert_noop!(
            Treasury::cancel_stream(to(), 0),
            Error::<Test>::AlreadyCancelled
        );
        // A missing stream errors on lookup, not state.
        assert_noop!(
            Treasury::cancel_stream(to(), 999),
            Error::<Test>::StreamNotFound
        );
    });
}

// ---- try_state (15 §1) ------------------------------------------------------

#[test]
fn try_state_reconciles_rebate_lines_against_real_custody_pots() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Keeper,
            50 * USDC
        ));
        assert_ok!(Treasury::fund_budget_line(
            to(),
            BudgetLine::Oracle,
            30 * USDC
        ));
        // The funding seam keeps the internal lines and the real custody pots
        // synchronized atomically (08 §1.4).
        assert_eq!(KeeperRebatePotBalance::get(), 50 * USDC);
        assert_eq!(OracleRebatePotBalance::get(), 30 * USDC);
        assert_ok!(crate::Pallet::<Test>::do_try_state());

        // Direct transfers, recovery, or genesis mistakes can still create
        // drift; the standing alarm remains the backstop for those sources.
        set_rebate_pot_balance(PayoutLine::Keeper, 49 * USDC);
        assert!(matches!(
            crate::Pallet::<Test>::do_try_state(),
            Err(sp_runtime::TryRuntimeError::Other(
                "treasury: KEEPER line exceeds real USDC custody pot"
            ))
        ));

        set_rebate_pot_balance(PayoutLine::Keeper, 50 * USDC);
        set_rebate_pot_balance(PayoutLine::Oracle, 29 * USDC);
        assert!(matches!(
            crate::Pallet::<Test>::do_try_state(),
            Err(sp_runtime::TryRuntimeError::Other(
                "treasury: ORACLE line exceeds real USDC custody pot"
            ))
        ));

        set_rebate_pot_balance(PayoutLine::Oracle, 30 * USDC);
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}

#[test]
fn try_state_holds_after_ops_and_catches_a_broken_stream() {
    funded_ext().execute_with(|| {
        assert_ok!(Treasury::open_stream(
            to(),
            BudgetLine::OpsCollators,
            acc(2),
            300_000 * USDC,
            0,
            100
        ));
        assert_ok!(crate::Pallet::<Test>::do_try_state());

        // Corrupt the keeper meter's tranche relation and confirm its standing
        // invariant is enforced independently of the mutable live budget.
        let mut t = crate::Pallet::<Test>::treasury();
        t.keeper_meter.spent = 1;
        t.keeper_meter.general_spent = 2;
        crate::Pallet::<Test>::seed(&t);
        assert!(crate::Pallet::<Test>::do_try_state().is_err());
        t.keeper_meter.general_spent = 1;
        crate::Pallet::<Test>::seed(&t);
        assert_ok!(crate::Pallet::<Test>::do_try_state());

        // Corrupt a stream (claimed > total) and confirm try_state rejects it.
        let mut t = crate::Pallet::<Test>::treasury();
        t.streams.push(Stream {
            id: 999,
            recipient: [1u8; 32],
            line: BudgetLine::Rewards,
            total: 1,
            claimed: 2,
            start: 0,
            duration: 1,
            cancelled: false,
        });
        crate::Pallet::<Test>::seed(&t);
        assert!(crate::Pallet::<Test>::do_try_state().is_err());
    });
}

// ---- shell ≡ core differential ---------------------------------------------

/// Deterministic xorshift so the sequence is reproducible with no wall-clock /
/// RNG dependency (rule 2).
fn next_rand(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Every op the pallet exposes, applied in lock-step to the FRAME shell (via
/// extrinsics) and a standalone core `Treasury` seeded to the identical initial
/// state, asserting equal acceptance and — after clearing the core's transient
/// event log — byte-identical final aggregates. This is the Python-M3 ≡ Rust
/// differential's Rust half at default parameters.
#[test]
fn shell_matches_core_over_a_randomized_op_stream() {
    use origins_core::Origin as CoreOrigin;
    funded_ext().execute_with(|| {
        // Mirror the shell's post-genesis+funding state into a standalone core.
        let mut core = crate::Pallet::<Test>::treasury();
        let mut rng: u32 = 0x9e37_79b9;
        let lines = [
            BudgetLine::OpsCollators,
            BudgetLine::Rewards,
            BudgetLine::OpsCoretime,
            BudgetLine::Keeper,
        ];

        for step in 0..600u32 {
            System::set_block_number((step as u64) * 7 + 1);
            let now = (step * 7 + 1) as futarchy_primitives::BlockNumber;
            let r = next_rand(&mut rng);
            let line = lines[(r % 4) as usize];
            let amount = ((r >> 4) % 400_000) as u128 * USDC;
            let id = ((r >> 8) as u64) % (core.next_stream_id + 2);

            let shell_res: frame_support::pallet_prelude::DispatchResult = match r % 8 {
                0 => Treasury::fund_budget_line(to(), line, amount / 4),
                1 => Treasury::spend(to(), line, acc((r % 5) as u8), amount),
                2 => Treasury::open_stream(to(), line, acc((r % 5) as u8), amount, now.into(), 100),
                3 => Treasury::claim_stream(RuntimeOrigin::signed(acc((r % 5) as u8)), id),
                4 => Treasury::cancel_stream(to(), id),
                5 => Treasury::issue_vit(to(), amount * 1_000, BudgetLine::Rewards),
                6 => {
                    Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(6)), (r >> 3) % 4)
                }
                _ => Treasury::recover_foreign(to(), AssetKind::Foreign([2u8; 32]), acc(1), amount),
            };

            let core_res = match r % 8 {
                0 => core.fund_budget_line(CoreOrigin::FutarchyTreasury, line, amount / 4),
                1 => core.spend(
                    CoreOrigin::FutarchyTreasury,
                    now,
                    line,
                    acc((r % 5) as u8).into(),
                    amount,
                ),
                2 => core
                    .open_stream(
                        CoreOrigin::FutarchyTreasury,
                        now,
                        futarchy_treasury_core::StreamInput {
                            line,
                            recipient: acc((r % 5) as u8).into(),
                            total: amount,
                            start: now,
                            duration: 100,
                        },
                    )
                    .map(|_| ()),
                3 => core
                    .claim_stream(acc((r % 5) as u8).into(), now, id)
                    .map(|_| ()),
                4 => core
                    .cancel_stream(CoreOrigin::FutarchyTreasury, id)
                    .map(|_| ()),
                5 => core.issue_vit(
                    CoreOrigin::FutarchyTreasury,
                    now,
                    amount * 1_000,
                    BudgetLine::Rewards,
                ),
                6 => core
                    .execute_coretime_renewal(
                        acc(6).into(),
                        (r >> 3) % 4,
                        u64::from(now),
                        u64::from(CoretimeQuoteTtl::get()),
                        CoretimeDotRate::get(),
                        CoretimeFeeDot::get(),
                    )
                    .map(|_| ()),
                _ => core.recover_foreign(
                    CoreOrigin::FutarchyTreasury,
                    AssetKind::Foreign([2u8; 32]),
                    acc(1).into(),
                    amount,
                ),
            };

            assert_eq!(
                shell_res.is_ok(),
                core_res.is_ok(),
                "acceptance diverged at step {step} (op {})",
                r % 8
            );

            // Occasionally note a fresh coretime quote on both sides so op 6 can
            // sometimes succeed rather than always closing the window.
            if r % 8 == 6 && core_res.is_err() {
                let period = (r >> 3) % 4;
                let _ = note_quote(period, 50_000 * USDC);
                let _ = core.note_coretime_renewal_quote(period, 50_000 * USDC, u64::from(now));
            }

            // Clear the core's transient event log (the shell never persists it)
            // and assert full aggregate equality.
            core.events.clear();
            assert_eq!(
                crate::Pallet::<Test>::treasury(),
                core,
                "state diverged at step {step} (op {})",
                r % 8
            );
        }
        assert_ok!(crate::Pallet::<Test>::do_try_state());
    });
}
