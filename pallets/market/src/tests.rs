//! Initial FRAME-shell conformance suite for `pallet-market` (04, 15 §1).
//!
//! The frame-free core owns the exhaustive LMSR vectors. These tests exercise
//! the production shell's real-ledger custody, frozen event/storage surface,
//! origin gates, rollback-safe error paths, and shell≡core differential.

use crate::{mock::*, BaselineMarketOf, ClosedAt, Error, Event, Markets};
use frame_support::{assert_err, assert_noop, assert_ok, traits::fungibles::Inspect};
use frame_system::RawOrigin;
use futarchy_primitives::{
    keeper::CrankClass, Balance, Branch, FixedU64, MarketId, PositionId, PositionKind, ScalarSide,
    TradeSide,
};
use market_core::{BookKind, MarketBook, MarketPhase, MarketState, MIN_TRADE};
use pallet_conditional_ledger::core_ledger::{baseline, position, LedgerState};
use sp_runtime::traits::Dispatchable;

type E = Error<Test>;

const MARKET_ID: MarketId = 7;
const BASELINE_ID: MarketId = 11;
const PROPOSAL: u64 = 1;
const EPOCH: u32 = 3;
const B: Balance = 10_000 * UNIT;
const TRADE: Balance = 1_000 * UNIT;

fn signed(who: AccountId) -> RuntimeOrigin {
    RawOrigin::Signed(who).into()
}

fn create_decision() {
    assert_ok!(Market::create_market(
        signed(MARKET_ADMIN),
        MARKET_ID,
        BookKind::Decision {
            proposal: PROPOSAL,
            branch: Branch::Accept,
        },
        BOOK,
        FEES,
        B,
    ));
}

fn create_baseline() {
    assert_ok!(Market::create_market(
        signed(MARKET_ADMIN),
        BASELINE_ID,
        BookKind::Baseline { epoch: EPOCH },
        BOOK,
        FEES,
        B,
    ));
}

fn seed(id: MarketId) {
    assert_ok!(Market::seed(signed(MARKET_ADMIN), id, TREASURY));
}

fn position_balance(id: PositionId, who: AccountId) -> Balance {
    pallet_conditional_ledger::Positions::<Test>::get(id, who)
}

fn core_position_balance(
    ledger: &LedgerState<AccountId>,
    id: PositionId,
    who: AccountId,
) -> Balance {
    ledger
        .positions
        .iter()
        .find(|record| record.id == id && record.owner == who)
        .map_or(0, |record| record.balance)
}

fn usdc(who: AccountId) -> Balance {
    <Assets as Inspect<AccountId>>::balance(USDC, &who)
}

#[test]
fn baseline_seed_is_funded_by_treasury_not_the_book() {
    // PR #53 Codex P1a: the Baseline seed must be funded from the POL_BASELINE
    // `treasury` line (04 §8.3/§10), not the book account — the FRAME ledger charges
    // the split payer's USDC, and the book is not the treasury, so charging it would
    // fail (book unfunded) or drain the wrong account.
    new_test_ext().execute_with(|| {
        create_baseline();
        let treasury_before = usdc(TREASURY);
        let book_before = usdc(BOOK);
        seed(BASELINE_ID);
        assert!(
            usdc(TREASURY) < treasury_before,
            "treasury (POL_BASELINE) must fund the baseline seed"
        );
        assert_eq!(
            usdc(BOOK),
            book_before,
            "the book must not be charged for its own seed"
        );
        // The book still ends holding the headroom complete set.
        let long = position_balance(baseline(EPOCH, ScalarSide::Long), BOOK);
        let short = position_balance(baseline(EPOCH, ScalarSide::Short), BOOK);
        assert_eq!(long, short);
        assert!(long > 0);
        assert_try_state();
    });
}

#[test]
fn accept_reject_pair_uses_one_dual_mint_split() {
    new_test_ext().execute_with(|| {
        create_decision();
        assert_ok!(Market::create_market(
            signed(MARKET_ADMIN),
            MARKET_ID + 1,
            BookKind::Decision {
                proposal: PROPOSAL,
                branch: Branch::Reject,
            },
            POL,
            INSURANCE,
            B,
        ));
        assert_ok!(Market::seed_branch_pair(
            signed(MARKET_ADMIN),
            MARKET_ID,
            MARKET_ID + 1,
            TREASURY,
        ));
        let headroom =
            position_balance(position(PROPOSAL, Branch::Accept, PositionKind::Long), BOOK);
        assert!(headroom > 0);
        assert_eq!(
            position_balance(position(PROPOSAL, Branch::Reject, PositionKind::Long), POL,),
            headroom,
        );
        assert_eq!(
            pallet_conditional_ledger::Vaults::<Test>::get(PROPOSAL).map(|vault| vault.escrowed),
            Some(headroom),
            "one split funds both branch books; a per-book split doubles escrow",
        );
        assert_try_state();
    });
}

#[test]
fn seed_failure_rolls_back_atomically_and_sets_no_flag() {
    // PR #53 Codex P1b: `seed` is an internal (non-`#[pallet::call]`) path with no
    // FRAME storage layer of its own, so a mid-sequence ledger failure could strand
    // partial state (and leave `SeededMarkets` unwritten → a retry double-seeds). A
    // book whose headroom `b·ln2` exceeds the treasury's USDC fails at the first split's
    // collateral settlement; the `with_storage_layer` wrap must roll everything back.
    new_test_ext().execute_with(|| {
        let big_b = 200_000 * UNIT; // headroom ~138,600 USDC > treasury's 100,000
        assert_ok!(Market::create_market(
            signed(MARKET_ADMIN),
            30,
            BookKind::Baseline { epoch: 9 },
            BOOK,
            FEES,
            big_b,
        ));
        assert!(Market::seed(signed(MARKET_ADMIN), 30, TREASURY).is_err());
        // No seeded flag, and no stranded baseline positions for treasury or the book.
        assert!(!crate::SeededMarkets::<Test>::contains_key(30));
        assert_eq!(position_balance(baseline(9, ScalarSide::Long), TREASURY), 0);
        assert_eq!(position_balance(baseline(9, ScalarSide::Long), BOOK), 0);
        assert_ok!(Ledger::do_try_state());
    });
}

#[test]
fn r7_baseline_sell_does_not_charge_the_seller() {
    // 04 §6.1: a sell pays the seller net-of-fee value; it must NEVER debit them.
    // In the unbranched Baseline book the seller is paid in a net complete set the
    // BOOK funds from its merge proceeds. Codex A3 review: the earlier
    // `do_split_baseline(who, net)` made the *seller* fund that split, so against the
    // real ledger a Baseline sell debited the seller ~net USDC (the in-memory oracle,
    // which models no real custody, masked it). Regression for that solvency bug.
    new_test_ext().execute_with(|| {
        create_baseline();
        seed(BASELINE_ID);
        System::set_block_number(10);
        assert_ok!(Market::buy(
            signed(ALICE),
            BASELINE_ID,
            ScalarSide::Long,
            TRADE,
            600 * UNIT,
        ));
        let usdc_before = usdc(ALICE);
        System::set_block_number(20);
        assert_ok!(Market::sell(
            signed(ALICE),
            BASELINE_ID,
            ScalarSide::Long,
            TRADE,
            1
        ));
        let paid = usdc_before.saturating_sub(usdc(ALICE));
        // The seller may forfeit at most a couple of position deposits (< 1 USDC) on
        // the two fresh legs — never ~net (~0.5·TRADE = 500 USDC here).
        assert!(
            paid < UNIT,
            "Baseline sell overcharged the seller by {paid}"
        );
        // And they still receive a net complete set (redeemable at par).
        let long = position_balance(baseline(EPOCH, ScalarSide::Long), ALICE);
        let short = position_balance(baseline(EPOCH, ScalarSide::Short), ALICE);
        assert_eq!(long, short);
        assert!(long > 0);
    });
}

#[test]
fn multi_book_proposal_shares_one_ledger_vault() {
    // A TREASURY>1%NAV / CODE / META proposal fields 2 decision + 4 gate books, and
    // even a PARAM proposal has a 2-book decision *pair* — all sharing ONE ledger
    // vault (03 §2.1 / 04 §1.1). `create_market` must create the vault once and reuse
    // it; the ledger rejects a duplicate `create_vault`, so a naive per-book create
    // fails on book 2. Regression for that bug.
    use futarchy_primitives::GateType;
    new_test_ext().execute_with(|| {
        let books: [(MarketId, BookKind); 6] = [
            (
                20,
                BookKind::Decision {
                    proposal: PROPOSAL,
                    branch: Branch::Accept,
                },
            ),
            (
                21,
                BookKind::Decision {
                    proposal: PROPOSAL,
                    branch: Branch::Reject,
                },
            ),
            (
                22,
                BookKind::Gate {
                    proposal: PROPOSAL,
                    branch: Branch::Accept,
                    gate: GateType::Survival,
                },
            ),
            (
                23,
                BookKind::Gate {
                    proposal: PROPOSAL,
                    branch: Branch::Reject,
                    gate: GateType::Survival,
                },
            ),
            (
                24,
                BookKind::Gate {
                    proposal: PROPOSAL,
                    branch: Branch::Accept,
                    gate: GateType::Security,
                },
            ),
            (
                25,
                BookKind::Gate {
                    proposal: PROPOSAL,
                    branch: Branch::Reject,
                    gate: GateType::Security,
                },
            ),
        ];
        for (id, kind) in books {
            assert_ok!(Market::create_market(
                signed(MARKET_ADMIN),
                id,
                kind,
                BOOK,
                FEES,
                B
            ));
        }
        // Exactly one vault backs all six books, and every book is live.
        assert!(pallet_conditional_ledger::Vaults::<Test>::contains_key(
            PROPOSAL
        ));
        for (id, _) in books {
            assert!(Markets::<Test>::contains_key(id));
        }
    });
}

fn market_events() -> Vec<Event<Test>> {
    System::events()
        .into_iter()
        .filter_map(|record| match record.event {
            RuntimeEvent::Market(event) => Some(event),
            _ => None,
        })
        .collect()
}

fn assert_try_state() {
    assert_ok!(Market::do_try_state());
    assert_ok!(Ledger::do_try_state());
}

#[test]
fn buy_collects_complete_pair_fee_and_records_twap() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(10);

        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            600 * UNIT,
        ));

        let book = Markets::<Test>::get(MARKET_ID).expect("created market exists");
        assert_eq!(book.q_long, TRADE);
        assert_eq!(book.q_short, 0);
        assert!(book.fees_accrued > 0);
        assert_eq!(book.last_observed_block, 10);
        assert_eq!(book.last_observation_1e9.0, 500_000_000);
        assert_eq!(book.cumulative_price_blocks, 5_000_000_000);
        assert_eq!(
            position_balance(
                position(PROPOSAL, Branch::Accept, PositionKind::Long),
                ALICE,
            ),
            TRADE,
        );
        // Decision fees are a complete branch pair, so they remain fully backed.
        assert_eq!(
            position_balance(
                position(PROPOSAL, Branch::Accept, PositionKind::BranchUsdc),
                FEES,
            ),
            book.fees_accrued,
        );
        assert_eq!(
            position_balance(
                position(PROPOSAL, Branch::Reject, PositionKind::BranchUsdc),
                FEES,
            ),
            book.fees_accrued,
        );
        assert!(market_events().iter().any(|event| matches!(
            event,
            Event::Observed { market, o_t }
                if *market == MARKET_ID && o_t.0 == 500_000_000
        )));
        assert!(market_events().iter().any(|event| matches!(
            event,
            Event::Traded {
                market,
                who,
                side: TradeSide::BuyLong,
                amount,
                ..
            } if *market == MARKET_ID && *who == ALICE && *amount == TRADE
        )));
        assert_try_state();
    });
}

#[test]
fn decision_sell_round_trip_releases_usdc_via_mirror_merge() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(10);
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            600 * UNIT,
        ));

        let mirror = position(PROPOSAL, Branch::Reject, PositionKind::BranchUsdc);
        let target = position(PROPOSAL, Branch::Accept, PositionKind::BranchUsdc);
        let mirror_before = position_balance(mirror, ALICE);
        let escrow_before = pallet_conditional_ledger::Vaults::<Test>::get(PROPOSAL)
            .expect("market creation creates its vault")
            .escrowed;
        let usdc_before = usdc(ALICE);
        let buy_fee = Markets::<Test>::get(MARKET_ID)
            .expect("created market exists")
            .fees_accrued;

        System::set_block_number(20);
        assert_ok!(Market::sell(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            1,
        ));

        let escrow_after = pallet_conditional_ledger::Vaults::<Test>::get(PROPOSAL)
            .expect("vault remains open")
            .escrowed;
        let released = escrow_before - escrow_after;
        assert!(released > 0);
        // Selling deletes the trader's LONG entry, so the ledger also refunds
        // its separately-accounted position-storage deposit.
        assert_eq!(usdc(ALICE) - usdc_before, released + PositionDeposit::get());
        assert_eq!(position_balance(target, ALICE), 0);
        assert_eq!(mirror_before - position_balance(mirror, ALICE), released);
        let book = Markets::<Test>::get(MARKET_ID).expect("created market exists");
        assert_eq!(book.q_long, 0);
        assert!(book.fees_accrued > buy_fee);
        assert_try_state();
    });
}

#[test]
fn baseline_fees_are_withheld_on_buy_and_sell() {
    new_test_ext().execute_with(|| {
        create_baseline();
        seed(BASELINE_ID);
        System::set_block_number(10);
        assert_ok!(Market::buy(
            signed(ALICE),
            BASELINE_ID,
            ScalarSide::Long,
            TRADE,
            600 * UNIT,
        ));

        assert_eq!(
            position_balance(baseline(EPOCH, ScalarSide::Long), ALICE),
            TRADE
        );
        assert_eq!(
            position_balance(baseline(EPOCH, ScalarSide::Short), ALICE),
            0
        );
        let buy_fee = Markets::<Test>::get(BASELINE_ID)
            .expect("created baseline exists")
            .fees_accrued;
        assert!(buy_fee > 0);

        System::set_block_number(20);
        assert_ok!(Market::sell(
            signed(ALICE),
            BASELINE_ID,
            ScalarSide::Long,
            TRADE,
            1,
        ));

        let book = Markets::<Test>::get(BASELINE_ID).expect("created baseline exists");
        assert!(book.fees_accrued > buy_fee);
        let payout_long = position_balance(baseline(EPOCH, ScalarSide::Long), ALICE);
        let payout_short = position_balance(baseline(EPOCH, ScalarSide::Short), ALICE);
        assert_eq!(payout_long, payout_short);
        assert!(payout_long > 0);
        let sell_event = market_events()
            .into_iter()
            .rev()
            .find_map(|event| match event {
                Event::Traded {
                    side: TradeSide::SellLong,
                    cost,
                    ..
                } => Some(cost),
                _ => None,
            })
            .expect("sell emits Traded");
        // The event reports gross proceeds, while the seller receives a pair
        // net of the second fee.
        assert!(payout_long < sell_event);
        assert_eq!(sell_event - payout_long, book.fees_accrued - buy_fee);
        assert_try_state();
    });
}

#[test]
fn slippage_phase_and_trade_bounds_are_enforced() {
    // limit-coverage: MinTrade, MaxTrade
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);

        assert_noop!(
            Market::buy(signed(ALICE), MARKET_ID, ScalarSide::Long, TRADE, 1,),
            E::SlippageExceeded
        );
        assert_noop!(
            Market::buy(
                signed(ALICE),
                MARKET_ID,
                ScalarSide::Long,
                MIN_TRADE - 1,
                Balance::MAX,
            ),
            E::AmountTooSmall
        );
        assert_noop!(
            Market::buy(
                signed(ALICE),
                MARKET_ID,
                ScalarSide::Long,
                market_core::max_trade_amount(B).saturating_add(1),
                Balance::MAX,
            ),
            E::AmountTooLarge
        );
        assert_ok!(Market::close(signed(MARKET_ADMIN), MARKET_ID));
        assert_eq!(
            Markets::<Test>::get(MARKET_ID)
                .expect("closed market remains archived")
                .phase,
            MarketPhase::Closed,
        );
        assert_noop!(
            Market::buy(
                signed(ALICE),
                MARKET_ID,
                ScalarSide::Long,
                MIN_TRADE,
                Balance::MAX,
            ),
            E::NotTrading
        );
        assert_try_state();
    });
}

#[test]
fn v6_domain_edge_is_rejected_without_mutation() {
    // limit-coverage: lmsr-domain-bound
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        Markets::<Test>::mutate(MARKET_ID, |maybe_book| {
            if let Some(book) = maybe_book {
                book.q_long = 48 * book.b;
            }
        });
        let before = Markets::<Test>::get(MARKET_ID).expect("created market exists");

        assert_noop!(
            Market::buy(
                signed(ALICE),
                MARKET_ID,
                ScalarSide::Long,
                MIN_TRADE,
                Balance::MAX,
            ),
            E::PriceBoundExceeded
        );
        assert_eq!(Markets::<Test>::get(MARKET_ID), Some(before));
        assert_try_state();
    });
}

#[test]
fn live_market_bound_rejects_the_197th_book() {
    // limit-coverage: MaxLiveMarkets
    new_test_ext().execute_with(|| {
        create_decision();
        let template = Markets::<Test>::get(MARKET_ID).expect("created market exists");
        let mut id = 0u64;
        while Markets::<Test>::count() < futarchy_primitives::bounds::MAX_LIVE_MARKETS {
            if !Markets::<Test>::contains_key(id) {
                let mut book = template;
                book.id = id;
                Markets::<Test>::insert(id, book);
            }
            id = id.saturating_add(1);
        }

        assert_noop!(
            Market::create_market(
                signed(MARKET_ADMIN),
                10_000,
                BookKind::Decision {
                    proposal: 10_000,
                    branch: Branch::Accept,
                },
                BOOK,
                FEES,
                B,
            ),
            E::TooManyMarkets
        );
        assert_eq!(
            Markets::<Test>::count(),
            futarchy_primitives::bounds::MAX_LIVE_MARKETS
        );
    });
}

#[test]
fn origins_are_narrow_for_trading_and_admin_operations() {
    new_test_ext().execute_with(|| {
        create_decision();

        assert_noop!(
            Market::buy(
                RawOrigin::None.into(),
                MARKET_ID,
                ScalarSide::Long,
                MIN_TRADE,
                Balance::MAX,
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Market::create_market(
                signed(ALICE),
                99,
                BookKind::Baseline { epoch: 99 },
                BOOK,
                FEES,
                B,
            ),
            E::BadOrigin
        );
        assert_noop!(
            Market::seed(signed(ALICE), MARKET_ID, TREASURY),
            E::BadOrigin
        );
        assert_noop!(Market::close(signed(ALICE), MARKET_ID), E::BadOrigin);
        assert_try_state();
    });
}

#[test]
fn baseline_mapping_is_written_and_duplicate_epoch_is_rejected() {
    new_test_ext().execute_with(|| {
        create_baseline();
        assert_eq!(BaselineMarketOf::<Test>::get(EPOCH), Some(BASELINE_ID));
        assert_noop!(
            Market::create_market(
                signed(MARKET_ADMIN),
                BASELINE_ID + 1,
                BookKind::Baseline { epoch: EPOCH },
                POL,
                FEES,
                B,
            ),
            E::DuplicateBaselineMarket
        );
        assert!(!Markets::<Test>::contains_key(BASELINE_ID + 1));
        assert_try_state();
    });
}

#[test]
fn frame_shell_matches_core_for_create_seed_buy_sell() {
    new_test_ext().execute_with(|| {
        let kind = BookKind::Decision {
            proposal: PROPOSAL,
            branch: Branch::Accept,
        };

        // Production shell over pallet-conditional-ledger.
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(10);
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            600 * UNIT,
        ));
        System::set_block_number(20);
        assert_ok!(Market::sell(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            1,
        ));

        // Frame-free oracle with identical accounts, parameters, and blocks.
        let mut core_ledger = LedgerState::new();
        assert_ok!(core_ledger.create_vault(PROPOSAL, 0));
        let mut core_market = MarketState::new();
        assert_ok!(core_market.create_market(MARKET_ID, kind, BOOK, FEES, B));
        assert_ok!(core_market.seed(&mut core_ledger, MARKET_ID, &TREASURY));
        assert_ok!(core_market.buy(
            &mut core_ledger,
            MARKET_ID,
            &ALICE,
            ScalarSide::Long,
            TRADE,
            600 * UNIT,
            10,
        ));
        assert_ok!(core_market.sell(
            &mut core_ledger,
            MARKET_ID,
            &ALICE,
            ScalarSide::Long,
            TRADE,
            1,
            20,
        ));

        let shell = Markets::<Test>::get(MARKET_ID).expect("created market exists");
        let oracle = core_market
            .markets
            .iter()
            .find(|book| book.id == MARKET_ID)
            .expect("core market exists");
        assert_eq!(shell.q_long, oracle.q_long);
        assert_eq!(shell.q_short, oracle.q_short);
        assert_eq!(shell.fees_accrued, oracle.fees_accrued);
        assert_eq!(shell.last_observation_1e9, oracle.last_observation_1e9);
        assert_eq!(
            shell.cumulative_price_blocks,
            oracle.cumulative_price_blocks
        );

        let keys = [
            (
                position(PROPOSAL, Branch::Accept, PositionKind::Long),
                ALICE,
            ),
            (
                position(PROPOSAL, Branch::Accept, PositionKind::Short),
                ALICE,
            ),
            (
                position(PROPOSAL, Branch::Accept, PositionKind::BranchUsdc),
                ALICE,
            ),
            (
                position(PROPOSAL, Branch::Reject, PositionKind::BranchUsdc),
                ALICE,
            ),
            (position(PROPOSAL, Branch::Accept, PositionKind::Long), BOOK),
            (
                position(PROPOSAL, Branch::Accept, PositionKind::Short),
                BOOK,
            ),
            (
                position(PROPOSAL, Branch::Accept, PositionKind::BranchUsdc),
                FEES,
            ),
            (
                position(PROPOSAL, Branch::Reject, PositionKind::BranchUsdc),
                FEES,
            ),
        ];
        for (id, who) in keys {
            assert_eq!(
                position_balance(id, who),
                core_position_balance(&core_ledger, id, who),
                "position mismatch for {id:?} at account {who}",
            );
        }
        assert_ok!(core_market.try_state());
        assert_ok!(core_ledger.try_state());
        assert_try_state();
    });
}

// ------------------------------------------------------------------ lifecycle: close/reap (04 §2)

#[test]
fn close_freezes_book_writes_closedat_and_blocks_trading() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(42);
        assert_ok!(Market::close(signed(MARKET_ADMIN), MARKET_ID));

        let book = Markets::<Test>::get(MARKET_ID).expect("closed book stays archived");
        assert_eq!(book.phase, MarketPhase::Closed);
        // `close` records the archive-delay start block and emits the frozen event.
        assert_eq!(ClosedAt::<Test>::get(MARKET_ID), Some(42));
        assert!(market_events()
            .iter()
            .any(|event| matches!(event, Event::MarketClosed { market } if *market == MARKET_ID)));
        // 04 §2 / D-8: a book never reopens — buy and sell both refuse once Closed.
        assert_noop!(
            Market::buy(
                signed(ALICE),
                MARKET_ID,
                ScalarSide::Long,
                TRADE,
                Balance::MAX,
            ),
            E::NotTrading
        );
        assert_noop!(
            Market::sell(signed(ALICE), MARKET_ID, ScalarSide::Long, TRADE, 1),
            E::NotTrading
        );
        assert_try_state();
    });
}

#[test]
fn reap_respects_archive_delay_then_removes_decision_book() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(5);
        assert_ok!(Market::close(signed(MARKET_ADMIN), MARKET_ID));
        let delay = MarketArchiveDelay::get();

        // One block short of `ClosedAt + ArchiveDelay`: reap must refuse and mutate nothing.
        System::set_block_number(5 + delay - 1);
        assert_noop!(Market::reap(signed(BOB), MARKET_ID), E::NotReapable);
        assert!(Markets::<Test>::contains_key(MARKET_ID));
        assert!(ClosedAt::<Test>::contains_key(MARKET_ID));

        // Exactly at the boundary: the permissionless reap removes book and ClosedAt.
        System::set_block_number(5 + delay);
        assert_ok!(Market::reap(signed(BOB), MARKET_ID));
        assert!(!Markets::<Test>::contains_key(MARKET_ID));
        assert!(!ClosedAt::<Test>::contains_key(MARKET_ID));
        assert!(market_events()
            .iter()
            .any(|event| matches!(event, Event::MarketReaped { market } if *market == MARKET_ID)));
        assert_try_state();
    });
}

#[test]
fn reap_of_baseline_book_prunes_baseline_mapping() {
    new_test_ext().execute_with(|| {
        create_baseline();
        seed(BASELINE_ID);
        assert_eq!(BaselineMarketOf::<Test>::get(EPOCH), Some(BASELINE_ID));

        System::set_block_number(5);
        assert_ok!(Market::close(signed(MARKET_ADMIN), BASELINE_ID));
        System::set_block_number(5 + MarketArchiveDelay::get());
        assert_ok!(Market::reap(signed(CHARLIE), BASELINE_ID));

        assert!(!Markets::<Test>::contains_key(BASELINE_ID));
        assert!(!ClosedAt::<Test>::contains_key(BASELINE_ID));
        // 04 §8.3: the epoch→market lookup is reaped with the book.
        assert_eq!(BaselineMarketOf::<Test>::get(EPOCH), None);
        assert_try_state();
    });
}

#[test]
fn reap_of_trading_book_is_rejected_not_reapable() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        // Still Trading and never closed: no archive delay can make it reapable.
        System::set_block_number(1 + MarketArchiveDelay::get() * 10);
        assert_noop!(Market::reap(signed(BOB), MARKET_ID), E::NotReapable);
        assert!(Markets::<Test>::contains_key(MARKET_ID));
        assert_try_state();
    });
}

// --------------------------------------------------------------- crank_observe keeper (04 §7)

#[test]
fn crank_observe_records_on_grid_then_noops_within_interval() {
    // limit-coverage: mkt.obs_interval
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);

        // A standalone permissionless crank on the observation grid records the
        // neutral 0.5 quote and advances the accumulator.
        System::set_block_number(ObsInterval::get());
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        let book = Markets::<Test>::get(MARKET_ID).expect("book exists");
        assert_eq!(book.last_observed_block, ObsInterval::get());
        assert_eq!(book.last_observation_1e9.0, 500_000_000);
        // A(end) grows by o_t · Δblocks with Δblocks = the elapsed interval (04 §7).
        assert_eq!(
            book.cumulative_price_blocks,
            500_000_000u128 * u128::from(ObsInterval::get()),
        );
        let observed = |events: &[Event<Test>]| {
            events
                .iter()
                .filter(|event| matches!(event, Event::Observed { .. }))
                .count()
        };
        assert_eq!(observed(&market_events()), 1);

        // A second crank before `obs_interval` elapses is a pure no-op: no new
        // Observed event, and the stored book is byte-identical.
        System::set_block_number(ObsInterval::get() + ObsInterval::get() / 2);
        let before = Markets::<Test>::get(MARKET_ID).expect("book exists");
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        assert_eq!(Markets::<Test>::get(MARKET_ID), Some(before));
        assert_eq!(observed(&market_events()), 1);
        assert_try_state();
    });
}

#[test]
fn crank_observe_caps_a_one_interval_price_jump_at_kappa() {
    // limit-coverage: mkt.kappa
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        Markets::<Test>::mutate(MARKET_ID, |maybe_book| {
            let book = maybe_book.as_mut().expect("book exists");
            book.q_long = 48 * book.b;
            book.last_quote_1e9 = futarchy_primitives::FixedU64(999_000_000);
        });

        System::set_block_number(ObsInterval::get());
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        let book = Markets::<Test>::get(MARKET_ID).expect("book exists");
        let one_interval_cap =
            500_000_000 + 500_000_000u64.saturating_mul(Kappa1e9::get()) / 1_000_000_000;
        assert_eq!(book.last_observation_1e9.0, one_interval_cap);
        assert_try_state();
    });
}

#[test]
fn crank_observe_rebates_once_per_recorded_observation_with_the_window_class() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        RecordKeeperRebates::set(true);

        DecisionWindowMarkets::set(vec![MARKET_ID]);
        System::set_block_number(ObsInterval::get());
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        assert_eq!(
            KeeperRebates::get(),
            vec![(BOB, CrankClass::DecisionCritical)]
        );

        // A too-early retry is a successful pure no-op and must not drain the
        // keeper budget. An error path likewise earns nothing.
        System::set_block_number(ObsInterval::get() + ObsInterval::get() / 2);
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        assert_noop!(Market::crank_observe(signed(BOB), 999), E::UnknownMarket);
        assert_eq!(
            KeeperRebates::get(),
            vec![(BOB, CrankClass::DecisionCritical)]
        );

        DecisionWindowMarkets::set(Vec::new());
        System::set_block_number(ObsInterval::get() * 2);
        assert_ok!(Market::crank_observe(signed(CHARLIE), MARKET_ID));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (BOB, CrankClass::DecisionCritical),
                (CHARLIE, CrankClass::General),
            ]
        );
        assert_try_state();
    });
}

#[test]
fn reap_rebates_only_after_a_book_is_actually_reaped() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(5);
        assert_ok!(Market::close(signed(MARKET_ADMIN), MARKET_ID));
        RecordKeeperRebates::set(true);

        System::set_block_number(5 + MarketArchiveDelay::get() - 1);
        assert_noop!(Market::reap(signed(BOB), MARKET_ID), E::NotReapable);
        assert!(KeeperRebates::get().is_empty());

        System::set_block_number(5 + MarketArchiveDelay::get());
        assert_ok!(Market::reap(signed(BOB), MARKET_ID));
        assert_eq!(KeeperRebates::get(), vec![(BOB, CrankClass::General)]);
        assert_try_state();
    });
}

// ------------------------------------------------- atomic rollback of a failing trade (R-7, 04 §6.4)

#[test]
fn r7_sell_ledger_leg_failure_rolls_back_every_move_atomically() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(10);

        // ALICE buys 2,000 LONG: she holds Accept-Long plus the mirror
        // Reject-branch-USDC — two `Positions` entries so far.
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            2 * TRADE,
            Balance::MAX,
        ));
        // Fill ALICE's remaining 62 position slots with unrelated vaults, so the
        // next mint for her would exceed MaxPositionsPerAccount.
        for i in 0..31u64 {
            let pid = 1_000 + i;
            assert_ok!(Ledger::create_vault(
                RawOrigin::Signed(market_account()).into(),
                pid,
                0,
            ));
            assert_ok!(Ledger::split(signed(ALICE), pid, MinSplit::get()));
        }
        assert_eq!(
            pallet_conditional_ledger::PositionCount::<Test>::get(ALICE),
            MaxPositionsPerAccount::get(),
        );

        let long_id = position(PROPOSAL, Branch::Accept, PositionKind::Long);
        let mirror_id = position(PROPOSAL, Branch::Reject, PositionKind::BranchUsdc);
        let fees_id = position(PROPOSAL, Branch::Accept, PositionKind::BranchUsdc);
        let book_before = Markets::<Test>::get(MARKET_ID).expect("book exists");
        let escrow_before = pallet_conditional_ledger::Vaults::<Test>::get(PROPOSAL)
            .expect("vault exists")
            .escrowed;
        let long_before = position_balance(long_id, ALICE);
        let mirror_before = position_balance(mirror_id, ALICE);
        let fees_before = position_balance(fees_id, FEES);
        let root_before = sp_io::storage::root(sp_runtime::StateVersion::V1);
        assert_eq!(long_before, 2 * TRADE);

        // Selling 1,000 LONG needs a fresh Accept-branch-USDC entry to receive the
        // net payout — the final wrapper leg — which trips the position cap. The
        // production shell has no snapshot of its own; atomicity is entirely the
        // dispatchable storage layer's job (04 §6.4, R-7). Dispatch the call the way
        // the runtime does so the transactional wrapper actually runs.
        let sell = RuntimeCall::Market(crate::Call::sell {
            market: MARKET_ID,
            side: ScalarSide::Long,
            amount: TRADE,
            min_proceeds: 1,
        });
        assert_eq!(
            sell.dispatch(signed(ALICE)).map_err(|e| e.error),
            Err(sp_runtime::DispatchError::from(E::Ledger)),
        );

        // No partial state and no stranded leg: the book, the vault escrow, and
        // every touched position are exactly as before — and so is the global root.
        assert_eq!(Markets::<Test>::get(MARKET_ID), Some(book_before));
        assert_eq!(
            pallet_conditional_ledger::Vaults::<Test>::get(PROPOSAL)
                .expect("vault exists")
                .escrowed,
            escrow_before,
        );
        assert_eq!(position_balance(long_id, ALICE), long_before);
        assert_eq!(position_balance(mirror_id, ALICE), mirror_before);
        assert_eq!(position_balance(fees_id, FEES), fees_before);
        assert_eq!(
            sp_io::storage::root(sp_runtime::StateVersion::V1),
            root_before,
        );
        assert_try_state();
    });
}

#[test]
fn r7_failed_second_trade_leaves_the_first_trade_untouched() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(10);
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        ));
        let settled = Markets::<Test>::get(MARKET_ID).expect("book exists");

        // A second buy with an unsatisfiable slippage bound fails before any ledger
        // move; `assert_noop!` proves the entire storage root — book and ledger — is
        // untouched by the rejected trade.
        assert_noop!(
            Market::buy(signed(BOB), MARKET_ID, ScalarSide::Long, TRADE, 1),
            E::SlippageExceeded
        );
        assert_eq!(Markets::<Test>::get(MARKET_ID), Some(settled));
        assert_try_state();
    });
}

// ----------------------------------------------------- authoritative vectors V1/V2 (04 §5)

#[test]
fn v1_buy_cost_and_v2_price_match_the_spec_vectors() {
    new_test_ext().execute_with(|| {
        // `create_decision` opens a symmetric book at b = 10,000 USDC (= B) — the
        // exact configuration of the 04 §5 vector table.
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(10);
        let escrow_before = pallet_conditional_ledger::Vaults::<Test>::get(PROPOSAL)
            .expect("vault exists")
            .escrowed;

        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE, // 1,000 LONG
            Balance::MAX,
        ));

        let (cost, p_after) = market_events()
            .into_iter()
            .rev()
            .find_map(|event| match event {
                Event::Traded {
                    side: TradeSide::BuyLong,
                    cost,
                    p_after,
                    ..
                } => Some((cost, p_after)),
                _ => None,
            })
            .expect("buy emits Traded");

        // V1: exact cost of buying 1,000 LONG = 512.494795136… USDC; charges round up
        // (04 §4), so the on-chain magnitude must sit within the §4 bound + 1 base unit
        // of the ceil. A regression to the retired 512.925… value is caught here.
        const V1_COST_FLOOR: Balance = 512_494_795; // floor(512.494795136 * 1e6)
        assert!(
            (V1_COST_FLOOR..=V1_COST_FLOOR + 2).contains(&cost),
            "V1 cost {cost} outside the §5 band"
        );
        // V2: p_L after V1 = 0.524979187479…, on the frozen 1e9 price grid.
        const V2_PRICE_1E9: u64 = 524_979_187; // round(0.524979187479 * 1e9)
        assert!(
            p_after.0.abs_diff(V2_PRICE_1E9) <= 2,
            "V2 price {} outside the §5 band",
            p_after.0
        );
        // The wrapper charges cost + the 30 bps fee into vault escrow (04 §6.1); the
        // fee is derived with the production fee function on the mock's `Fee` param.
        let fee = market_core::fee_up(cost, Fee::get()).expect("fee computes");
        let escrow_after = pallet_conditional_ledger::Vaults::<Test>::get(PROPOSAL)
            .expect("vault exists")
            .escrowed;
        assert_eq!(escrow_after - escrow_before, cost + fee);
        assert_try_state();
    });
}

// -------------------------------------------------- POL seed pre-collateralization (04 §10, I-12)

/// V4 worst-case maker loss = 10,000·ln 2 = 6931.47180560… USDC; the seed mints
/// exactly this much headroom of complete sets into the book (04 §5/§10).
const V4_HEADROOM_FLOOR: Balance = 6_931_471_805; // floor(10_000 * ln2 * 1e6)

fn seeded_headroom(id: MarketId) -> Balance {
    market_events()
        .into_iter()
        .rev()
        .find_map(|event| match event {
            Event::Seeded { market, headroom } if market == id => Some(headroom),
            _ => None,
        })
        .expect("seed emits Seeded")
}

#[test]
fn i12_seed_decision_book_precollateralizes_complete_sets() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);

        let headroom = seeded_headroom(MARKET_ID);
        assert!(
            (V4_HEADROOM_FLOOR..=V4_HEADROOM_FLOOR + 2).contains(&headroom),
            "headroom {headroom} outside the b·ln2 band"
        );
        // The book holds `headroom` LONG and `headroom` SHORT — complete sets, each
        // worth 1 branch-USDC at any settlement, so revenue never carries price risk.
        assert_eq!(
            position_balance(position(PROPOSAL, Branch::Accept, PositionKind::Long), BOOK),
            headroom,
        );
        assert_eq!(
            position_balance(
                position(PROPOSAL, Branch::Accept, PositionKind::Short),
                BOOK,
            ),
            headroom,
        );
        assert_try_state();
    });
}

#[test]
fn i12_seed_baseline_book_precollateralizes_complete_sets() {
    new_test_ext().execute_with(|| {
        create_baseline();
        seed(BASELINE_ID);

        let headroom = seeded_headroom(BASELINE_ID);
        assert!(
            (V4_HEADROOM_FLOOR..=V4_HEADROOM_FLOOR + 2).contains(&headroom),
            "headroom {headroom} outside the b·ln2 band"
        );
        // Baseline is unconditional: both epoch legs are minted directly to the book.
        assert_eq!(
            position_balance(baseline(EPOCH, ScalarSide::Long), BOOK),
            headroom,
        );
        assert_eq!(
            position_balance(baseline(EPOCH, ScalarSide::Short), BOOK),
            headroom,
        );
        assert_try_state();
    });
}

// ----------------------------------------------------------- broader origin misuse (rule 6)

#[test]
fn unsigned_origin_is_rejected_for_signed_calls() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        // Every Signed extrinsic checks the origin before touching state: an
        // unsigned origin is rejected with `BadOrigin` and mutates nothing.
        assert_noop!(
            Market::buy(
                RawOrigin::None.into(),
                MARKET_ID,
                ScalarSide::Long,
                TRADE,
                Balance::MAX,
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Market::sell(
                RawOrigin::None.into(),
                MARKET_ID,
                ScalarSide::Long,
                TRADE,
                1
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Market::crank_observe(RawOrigin::None.into(), MARKET_ID),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Market::reap(RawOrigin::None.into(), MARKET_ID),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_try_state();
    });
}

#[test]
fn permissionless_crank_and_reap_accept_any_signed_user() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        // crank_observe is permissionless — a non-admin signed user records an obs.
        System::set_block_number(ObsInterval::get());
        assert_ok!(Market::crank_observe(signed(CHARLIE), MARKET_ID));
        assert!(market_events().iter().any(|event| matches!(
            event,
            Event::Observed { market, .. } if *market == MARKET_ID
        )));

        // reap is permissionless once the book is closed and aged.
        assert_ok!(Market::close(signed(MARKET_ADMIN), MARKET_ID));
        System::set_block_number(ObsInterval::get() + MarketArchiveDelay::get());
        assert_ok!(Market::reap(signed(BOB), MARKET_ID));
        assert!(!Markets::<Test>::contains_key(MARKET_ID));
        assert_try_state();
    });
}

#[test]
fn registered_window_reads_exact_twap_close_spot_and_time_averaged_contest() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 0);
        let trailing = interval.saturating_mul(2);
        let end = interval.saturating_mul(3);
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            0,
            trailing,
            end,
        ));

        System::set_block_number(u64::from(interval));
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        ));
        System::set_block_number(u64::from(trailing));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        System::set_block_number(u64::from(end));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        let observed_close = Market::spot_at(MARKET_ID, end);
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        ));
        assert_ok!(Market::buy(
            signed(BOB),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        ));
        assert_ok!(Market::seal_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            end,
        ));

        assert!(Market::twap_at(MARKET_ID, end, end).is_some());
        assert!(Market::twap_at(MARKET_ID, end, interval).is_some());
        assert_ne!(Market::spot_at(MARKET_ID, end), observed_close);
        assert_eq!(
            Market::spot_at(MARKET_ID, end),
            Markets::<Test>::get(MARKET_ID).map(|book| book.last_quote_1e9),
        );
        assert_eq!(
            Market::average_contest_at(MARKET_ID, end, end),
            TRADE
                .checked_mul(Balance::from(trailing))
                .and_then(|value| value.checked_div(Balance::from(end)))
        );
        assert!(Market::decision_grade_at(
            MARKET_ID,
            end,
            end,
            100,
            FixedU64(1_000_000_000),
            1,
            B,
            true,
        ));

        System::set_block_number(u64::from(end.saturating_add(1)));
        assert_noop!(
            Market::buy(
                signed(ALICE),
                MARKET_ID,
                ScalarSide::Long,
                TRADE,
                Balance::MAX,
            ),
            E::NotTrading
        );
        assert_try_state();
    });
}

#[test]
fn contest_depth_accrues_forward_across_a_pre_observation_round_trip() {
    // 05 §5.2/§5.6 and 08 §5.2: contest depth is a time integral, so a
    // position opened one block before the observation boundary may contribute
    // for that one block only.  The former observation-only accounting charged
    // the pre-sell quantity backward over the entire ten-block interval.
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 1);
        let end = interval.saturating_mul(2);
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            0,
            interval,
            end,
        ));

        System::set_block_number(u64::from(interval));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));

        // Hold TRADE for exactly one block, then unwind in the trade that also
        // records the end observation.  This is the observation-boundary flash
        // that must not receive backward credit for the preceding interval.
        System::set_block_number(u64::from(end.saturating_sub(1)));
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        ));
        System::set_block_number(u64::from(end));
        assert_ok!(Market::sell(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            1,
        ));

        assert_eq!(
            Market::average_contest_at(MARKET_ID, end, end),
            TRADE.checked_div(Balance::from(end)),
            "one-block contest must receive one block of credit, never a full interval",
        );
        assert_try_state();
    });
}

#[test]
fn contest_depth_same_block_round_trip_before_observation_matches_empty_book() {
    // A zero-duration buy+unwind immediately before the observation boundary
    // has the same time integral as holding nothing.  This guards the exact
    // flash path called out by the A8 remediation review.
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 1);
        let end = interval.saturating_mul(2);
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            0,
            interval,
            end,
        ));
        System::set_block_number(u64::from(interval));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));

        System::set_block_number(u64::from(end.saturating_sub(1)));
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        ));
        assert_ok!(Market::sell(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            1,
        ));
        System::set_block_number(u64::from(end));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));

        assert_eq!(Market::average_contest_at(MARKET_ID, end, end), Some(0));
        assert_try_state();
    });
}

#[test]
fn contest_depth_held_for_the_whole_window_counts_fully() {
    // The forward-accrual fix must not undercount genuine persistent contest.
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 0);
        let start = 1;
        let trailing = start + interval;
        let end = trailing + interval;
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            start,
            trailing,
            end,
        ));

        System::set_block_number(u64::from(start));
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        ));
        System::set_block_number(u64::from(trailing));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        System::set_block_number(u64::from(end));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));

        assert_eq!(
            Market::average_contest_at(MARKET_ID, end, end - start),
            Some(TRADE),
            "contest held for every block of the window must count at full notional",
        );
        assert_try_state();
    });
}

#[test]
fn close_seals_end_checkpoint_after_an_end_minus_one_dust_trade() {
    // 04 §7: the end checkpoint is derivable solely from pre-close state.
    // Missing the exact end-grid observation must not let an end-1 dust trade
    // deny an otherwise sufficiently covered decision window.
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 0);
        // Twenty of twenty-one scheduled observations is still >=95% coverage,
        // so the deliberately absent observation at `end` is not itself fatal.
        let scheduled_intervals = 21;
        let end = interval.saturating_mul(scheduled_intervals);
        let trailing = interval.saturating_mul(11);
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            0,
            trailing,
            end,
        ));

        for step in 1..scheduled_intervals {
            System::set_block_number(u64::from(interval.saturating_mul(step)));
            assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        }
        let last_observed = end.saturating_sub(interval);
        assert_eq!(
            Markets::<Test>::get(MARKET_ID).map(|book| book.last_observed_block),
            Some(u64::from(last_observed)),
        );

        System::set_block_number(u64::from(end.saturating_sub(1)));
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            MIN_TRADE,
            Balance::MAX,
        ));
        let close_spot = Markets::<Test>::get(MARKET_ID).map(|book| book.last_quote_1e9);
        assert!(
            close_spot.is_some(),
            "the live market must have a stored pre-close quote",
        );
        let Some(close_spot) = close_spot else {
            return;
        };
        assert!(close_spot.0 > 500_000_000);

        System::set_block_number(u64::from(end));
        assert_ok!(Market::close(signed(MARKET_ADMIN), MARKET_ID));

        // The missing end-grid observation is sealed by carrying the last
        // recorded observation to the boundary.  The dust trade affects the
        // close spot, but cannot rewrite the already-recorded TWAP series.
        assert_eq!(
            Market::twap_at(MARKET_ID, end, end),
            Some(FixedU64(500_000_000)),
        );
        assert_eq!(Market::spot_at(MARKET_ID, end), Some(close_spot));
        assert!(Market::decision_grade_at(
            MARKET_ID,
            end,
            end,
            95,
            FixedU64(1_000_000_000),
            0,
            B,
            true,
        ));
        assert_try_state();
    });
}

#[test]
fn sealed_decision_window_ignores_a_later_trade_in_the_same_end_block() {
    // 04 §2/§7: sealing is the decision boundary. A trade ordered later in
    // the same block remains a valid market trade, but it may not rewrite any
    // checkpoint or the close spot already consumed by the decision engine.
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 1);
        let trailing = interval;
        let end = interval.saturating_mul(2);
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            0,
            trailing,
            end,
        ));
        System::set_block_number(u64::from(trailing));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));

        // Establish a non-neutral pre-close quote without recording another
        // observation, then seal from precisely that pre-close information.
        System::set_block_number(u64::from(end.saturating_sub(1)));
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            MIN_TRADE,
            Balance::MAX,
        ));
        System::set_block_number(u64::from(end));
        assert_ok!(Market::seal_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            end,
        ));

        let sealed_checkpoints = crate::WindowCheckpoints::<Test>::get(MARKET_ID);
        let sealed_full_twap = Market::twap_at(MARKET_ID, end, end);
        let sealed_trailing_twap = Market::twap_at(MARKET_ID, end, interval);
        let sealed_spot = Market::spot_at(MARKET_ID, end);
        let quote_at_seal = Markets::<Test>::get(MARKET_ID).map(|book| book.last_quote_1e9);
        assert!(sealed_full_twap.is_some());
        assert!(sealed_trailing_twap.is_some());
        assert_eq!(sealed_spot, quote_at_seal);

        // This trade is ordered strictly after the seal but still in `end`.
        assert_ok!(Market::buy(
            signed(ALICE),
            MARKET_ID,
            ScalarSide::Long,
            MIN_TRADE,
            Balance::MAX,
        ));
        let quote_after_trade = Markets::<Test>::get(MARKET_ID).map(|book| book.last_quote_1e9);
        assert_ne!(
            quote_after_trade, quote_at_seal,
            "the interleaved trade must execute"
        );

        assert_eq!(
            crate::WindowCheckpoints::<Test>::get(MARKET_ID),
            sealed_checkpoints,
            "a post-seal end-block observation must not rewrite checkpoints",
        );
        assert_eq!(Market::twap_at(MARKET_ID, end, end), sealed_full_twap);
        assert_eq!(
            Market::twap_at(MARKET_ID, end, interval),
            sealed_trailing_twap,
        );
        assert_eq!(
            Market::spot_at(MARKET_ID, end),
            sealed_spot,
            "the post-seal quote is not the decision close spot",
        );
        assert_try_state();
    });
}

#[test]
fn shared_baseline_decisions_read_one_sealed_value_across_an_interleaved_trade() {
    // Two proposal decisions can consume the same Baseline book and end
    // boundary (04 §8.4). The first decision seals the boundary; a Baseline
    // trade before the second decision in the same block must not create a
    // different Baseline fact for that second consumer.
    new_test_ext().execute_with(|| {
        create_baseline();
        seed(BASELINE_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 1);
        let trailing = interval;
        let end = interval.saturating_mul(2);
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            BASELINE_ID,
            0,
            trailing,
            end,
        ));
        System::set_block_number(u64::from(trailing));
        assert_ok!(Market::crank_observe(signed(BOB), BASELINE_ID));
        System::set_block_number(u64::from(end.saturating_sub(1)));
        assert_ok!(Market::buy(
            signed(ALICE),
            BASELINE_ID,
            ScalarSide::Long,
            MIN_TRADE,
            Balance::MAX,
        ));

        System::set_block_number(u64::from(end));
        // Logical consumer 1 seals and reads the shared Baseline boundary.
        assert_ok!(Market::seal_decision_window(
            signed(MARKET_ADMIN),
            BASELINE_ID,
            end,
        ));
        let first_checkpoints = crate::WindowCheckpoints::<Test>::get(BASELINE_ID);
        let first_full_twap = Market::twap_at(BASELINE_ID, end, end);
        let first_trailing_twap = Market::twap_at(BASELINE_ID, end, interval);
        let first_spot = Market::spot_at(BASELINE_ID, end);
        assert!(first_full_twap.is_some());
        assert!(first_trailing_twap.is_some());
        assert!(first_spot.is_some());

        assert_ok!(Market::buy(
            signed(BOB),
            BASELINE_ID,
            ScalarSide::Long,
            MIN_TRADE,
            Balance::MAX,
        ));

        // Logical consumer 2 reaches the same seal API after the interleaved
        // trade. Resealing is idempotent and must return the first decision's
        // exact shared Baseline values.
        assert_ok!(Market::seal_decision_window(
            signed(MARKET_ADMIN),
            BASELINE_ID,
            end,
        ));
        assert_eq!(
            crate::WindowCheckpoints::<Test>::get(BASELINE_ID),
            first_checkpoints,
        );
        assert_eq!(Market::twap_at(BASELINE_ID, end, end), first_full_twap);
        assert_eq!(
            Market::twap_at(BASELINE_ID, end, interval),
            first_trailing_twap,
        );
        assert_eq!(Market::spot_at(BASELINE_ID, end), first_spot);
        assert_try_state();
    });
}

#[test]
fn observation_after_window_end_cannot_backfill_close_data() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        let interval = u32::try_from(ObsInterval::get()).unwrap_or_default();
        assert!(interval > 0);
        let trailing = interval.saturating_mul(2);
        let end = interval.saturating_mul(3);
        assert_ok!(Market::register_decision_window(
            signed(MARKET_ADMIN),
            MARKET_ID,
            0,
            trailing,
            end,
        ));
        System::set_block_number(u64::from(interval));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        System::set_block_number(u64::from(trailing));
        assert_ok!(Market::crank_observe(signed(BOB), MARKET_ID));
        System::set_block_number(u64::from(end.saturating_add(1)));
        assert_noop!(Market::crank_observe(signed(BOB), MARKET_ID), E::NotTrading);
        assert_eq!(Market::twap_at(MARKET_ID, end, end), None);
        assert_eq!(Market::spot_at(MARKET_ID, end), None);
        assert!(!Market::decision_grade_at(
            MARKET_ID,
            end,
            end,
            100,
            FixedU64(1_000_000_000),
            0,
            B,
            true,
        ));
        assert_try_state();
    });
}

#[test]
fn delay_rerun_adds_one_original_seed_and_doubles_lmsr_depth_once() {
    new_test_ext().execute_with(|| {
        create_decision();
        seed(MARKET_ID);
        System::set_block_number(ObsInterval::get());
        assert_ok!(Market::reopen_for_rerun(signed(MARKET_ADMIN), MARKET_ID));
        assert_ok!(Market::seed_rerun(
            signed(MARKET_ADMIN),
            MARKET_ID,
            TREASURY,
        ));
        assert_eq!(
            Markets::<Test>::get(MARKET_ID).map(|book| book.b),
            B.checked_mul(2)
        );
        assert_noop!(
            Market::seed_rerun(signed(MARKET_ADMIN), MARKET_ID, TREASURY),
            E::AlreadySeeded
        );
        assert_try_state();
    });
}

// ---------------------------------------------------- try_state actually detects drift (15 §1)

#[test]
fn do_try_state_rejects_zero_b_book() {
    new_test_ext().execute_with(|| {
        // A book with b == 0 has no valid LMSR domain (04 §4). Injecting one directly
        // through the storage API must make the hook error — proving it catches drift
        // rather than merely passing on the happy path.
        let bad = MarketBook::open(
            MARKET_ID,
            BookKind::Decision {
                proposal: PROPOSAL,
                branch: Branch::Accept,
            },
            BOOK,
            FEES,
            0,
        );
        Markets::<Test>::insert(MARKET_ID, bad);
        assert_err!(Market::do_try_state(), E::TryStateViolation);
    });
}

#[test]
fn do_try_state_rejects_dangling_baseline_mapping() {
    new_test_ext().execute_with(|| {
        // 04 §8.3: `BaselineMarketOf` must resolve to a live Baseline book. A mapping
        // to a non-existent market is drift the hook must reject.
        BaselineMarketOf::<Test>::insert(EPOCH, 4242);
        assert_err!(Market::do_try_state(), E::TryStateViolation);
    });
}
