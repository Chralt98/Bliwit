//! 15 §4.1 suite for `pallet-futarchy-treasury`: per-extrinsic × error-path ×
//! origin-misuse × limit coverage, NAV/reserve-haircut fail-static, the rule-4
//! Params-injection proof, a `try_state` assertion, and a seeded shell-vs-core
//! differential (Python M3 ≡ Rust core ≡ this pallet at default parameters).

use crate::mock::*;
use crate::{Error, Event};
use frame_support::{assert_err, assert_noop, assert_ok};
use futarchy_treasury_core::{
    AssetKind, BudgetLine, Stream, Treasury as CoreTreasury, DAYS_365_BLOCKS, DAY_BLOCKS,
    MAX_STREAMS, TRS_CAP_PROPOSAL_BPS, TRS_STREAM_THRESHOLD_BPS, USDC, VIT,
};

const MAIN0: u128 = 25_000_000 * USDC;

fn to() -> RuntimeOrigin {
    RuntimeOrigin::signed(treasury_acc())
}

/// Genesis-funded `MAIN` (25M USDC) with three lines pre-funded via the
/// extrinsic (the realistic post-XCM funding path, 08 §2.5).
fn funded_ext() -> sp_io::TestExternalities {
    let mut ext = new_test_ext_with(crate::GenesisConfig::<Test> {
        main_usdc: MAIN0,
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
fn spend_enforces_stream_threshold_cap_and_line_balance() {
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

        // Coretime renewal stays alive (D-9 freeze-exempt).
        assert_ok!(crate::Pallet::<Test>::note_coretime_renewal_quote(
            1,
            100_000 * USDC
        ));
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
            assert_ok!(Treasury::note_coretime_renewal_quote(42, price));

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
                .any(|(period, _)| *period == 42));
        });
    }

    #[test]
    fn renewal_dispatch_error_rolls_back_accounting_for_retry() {
        new_ext().execute_with(|| {
            let price = 100_000 * USDC;
            assert_ok!(Treasury::note_coretime_renewal_quote(42, price));
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
            assert!(state.coretime_quotes.contains(&(42, price)));
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
        // The paid amount is the runtime-noted quote, not a caller value.
        assert_ok!(crate::Pallet::<Test>::note_coretime_renewal_quote(
            42,
            100_000 * USDC
        ));
        let before = Treasury::line_balance(BudgetLine::OpsCoretime);
        assert_ok!(Treasury::execute_coretime_renewal(
            RuntimeOrigin::signed(acc(7)),
            42
        ));
        assert_eq!(
            Treasury::line_balance(BudgetLine::OpsCoretime),
            before - 100_000 * USDC
        );
        System::assert_last_event(RuntimeEvent::Treasury(Event::CoretimeRenewalCalled {
            line: BudgetLine::OpsCoretime,
            amount: 100_000 * USDC,
        }));
        // Idempotent per period, even against a re-noted quote.
        assert_noop!(
            crate::Pallet::<Test>::note_coretime_renewal_quote(42, 1),
            Error::<Test>::PeriodAlreadyFunded
        );
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(8)), 42),
            Error::<Test>::PeriodAlreadyFunded
        );
        // Bounded by the pre-authorized line balance.
        assert_ok!(crate::Pallet::<Test>::note_coretime_renewal_quote(
            43,
            5_000_000 * USDC
        ));
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(8)), 43),
            Error::<Test>::InsufficientFunds
        );
    });
}

#[test]
fn coretime_quote_rejects_zero_and_can_be_pruned() {
    funded_ext().execute_with(|| {
        // A zero quote is refused (a keeper must not "renew" for free and lock
        // the period against a corrected retry).
        assert_noop!(
            crate::Pallet::<Test>::note_coretime_renewal_quote(50, 0),
            Error::<Test>::ZeroQuote
        );
        // A stale open quote can be pruned so it cannot be executed later.
        assert_ok!(crate::Pallet::<Test>::note_coretime_renewal_quote(
            50,
            10_000 * USDC
        ));
        assert_ok!(crate::Pallet::<Test>::prune_coretime_quote(50));
        assert_noop!(
            Treasury::execute_coretime_renewal(RuntimeOrigin::signed(acc(7)), 50),
            Error::<Test>::RenewalWindowClosed
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
    // Fund below the CODE floor (~13.9M) but above the PARAM floor (~1.85M).
    let mut ext = new_test_ext_with(crate::GenesisConfig::<Test> {
        main_usdc: 2_000_000 * USDC,
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
            nav: 2_000_000 * USDC,
            floor: CoreTreasury::floor(ProposalClass::Code),
        }));
        // Above the floor ⇒ returns false, no event.
        assert!(!crate::Pallet::<Test>::flag_nav_floor(ProposalClass::Param));
    });
}

// ---- rolling meters (08 §1.3, I-7) ------------------------------------------

#[test]
fn rolling_30d_meter_binds_spending() {
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
}

// ---- rule 4: caps are read from Params, not hardcoded -----------------------

#[test]
fn caps_track_params_not_a_hardcode() {
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
                    .execute_coretime_renewal(acc(6).into(), (r >> 3) % 4)
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
                let _ = crate::Pallet::<Test>::note_coretime_renewal_quote(period, 50_000 * USDC);
                let _ = core.note_coretime_renewal_quote(period, 50_000 * USDC);
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
