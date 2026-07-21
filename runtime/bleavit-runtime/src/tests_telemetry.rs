//! Runtime-level tests for the monitoring-only `TelemetryApi` (12 §6.3, B13).

use alloc::vec;

use frame_support::{
    traits::{fungibles::Inspect, fungibles::Mutate, Get},
    BoundedVec,
};
use futarchy_primitives::{
    currency, Balance, Branch, PositionId, PositionKind, ScalarSide, TradeSide,
};
use pallet_market::core_market::{BookKind, MarketBook, TwapWindow};

use sp_runtime::traits::AccountIdConversion;

use crate::{tests, ForeignAssets, Market, Runtime, System};

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, &'static str> {
    value.ok_or(context)
}

fn v3_displacement(b: Balance) -> Option<Balance> {
    // 04 §5 V3 fixes q/b = ln(1.5) = 0.4054651081… . These are
    // dimensionless vector digits, not a duplicated 13-owned `b` value.
    b.checked_mul(4_054_651_081)?.checked_div(10_000_000_000)
}

#[test]
fn telemetry_market_books_matches_v1_v3_premium_loss_and_seed_bound() -> Result<(), &'static str> {
    tests::development_ext().execute_with(|| {
        const V1_MARKET: u64 = 41;
        const V3_MARKET: u64 = 43;

        // Read `b` from live Params: the test states are the dimensionless
        // x=0.1 (V1) and x=ln(1.5) (V3) walks from 04 §5, not duplicated
        // 13-owned defaults.
        let b = crate::configs::balance_param(b"pol.b.param");
        assert_ne!(b, 0);
        let v1_quantity = b / 10;
        let v3_quantity = required(v3_displacement(b), "V3 displacement must fit fixed math")?;

        let mut v1 = MarketBook::open(
            V1_MARKET,
            BookKind::Decision {
                proposal: 7,
                branch: Branch::Accept,
            },
            tests::account(31),
            tests::account(32),
            b,
        );
        let v1_open = v1.clone();
        v1.q_long = v1_quantity;

        let mut v3 = MarketBook::open(
            V3_MARKET,
            BookKind::Decision {
                proposal: 8,
                branch: Branch::Reject,
            },
            tests::account(33),
            tests::account(34),
            b,
        );
        let v3_open = v3.clone();
        v3.q_long = v3_quantity;

        let bound = required(
            pallet_market::core_market::seed_headroom(b).ok(),
            "live Params b must admit a seed bound",
        )?;
        let v1_loss = required(
            pallet_market::core_market::maker_loss_at_state(b, v1_quantity, 0).ok(),
            "V1 state must admit a maker-loss reconstruction",
        )?;
        let v3_loss = required(
            pallet_market::core_market::maker_loss_at_state(b, v3_quantity, 0).ok(),
            "V3 state must admit a maker-loss reconstruction",
        )?;
        for (book, loss) in [(&v1, v1_loss), (&v3, v3_loss)] {
            let (proposal, branch) = match book.kind {
                BookKind::Decision { proposal, branch } => (proposal, branch),
                _ => return Err("vector fixture must use decision books"),
            };
            let retained = bound.saturating_sub(loss);
            for kind in [PositionKind::Long, PositionKind::Short] {
                pallet_conditional_ledger::Positions::<Runtime>::insert(
                    PositionId::Proposal {
                        proposal,
                        branch,
                        kind,
                    },
                    &book.account,
                    retained,
                );
            }
            pallet_market::SeededMarkets::<Runtime>::insert(book.id, ());
        }
        pallet_market::Markets::<Runtime>::insert(V1_MARKET, v1);
        pallet_market::Markets::<Runtime>::insert(V3_MARKET, v3);
        pallet_market::LivePolCommitments::<Runtime>::put(BoundedVec::truncate_from(vec![
            (V1_MARKET, bound),
            (V3_MARKET, bound),
        ]));

        let rows = required(
            crate::telemetry::market_books(),
            "valid live books must produce telemetry",
        )?;
        assert_eq!(rows.len(), 2);

        for (market, quantity, open) in [
            (V1_MARKET, v1_quantity, v1_open),
            (V3_MARKET, v3_quantity, v3_open),
        ] {
            let row = required(
                rows.iter().find(|row| row.market == market),
                "inserted live book must be present",
            )?;
            let quote = required(
                pallet_market::core_market::quote(
                    &open,
                    TradeSide::BuyLong,
                    quantity,
                    <Runtime as pallet_market::Config>::Fee::get(),
                )
                .ok(),
                "04 §5 vector state must be inside the LMSR domain",
            )?;
            let premium_loss = quantity.saturating_sub(quote.cost);

            // The independent comparison oracle evaluates C(q)-C(0) directly.
            // Its premium rounding can differ from the custody-backed telemetry
            // result by at most one USDC base unit (04 §4), never enough to
            // exceed headroom.
            assert!(row.book_loss_usdc.abs_diff(premium_loss) <= 1);
            assert_eq!(row.lmsr_loss_bound_usdc, bound);
            assert!(row.book_loss_usdc <= row.lmsr_loss_bound_usdc);
        }
        Ok(())
    })
}

#[test]
fn telemetry_market_book_loss_tracks_real_baseline_buy_and_sell_fee_custody(
) -> Result<(), &'static str> {
    tests::development_ext().execute_with(|| {
        const MARKET: u64 = 47;
        const EPOCH: u32 = 9;

        let trader = tests::account(41);
        let book_account = tests::account(42);
        let fees_account = tests::account(43);
        let treasury = crate::configs::pol_baseline_account();
        let asset = <Runtime as pallet_conditional_ledger::Config>::UsdcAssetId::get();
        let b = crate::configs::balance_param(b"pol.b_baseline");
        let headroom = required(
            pallet_market::core_market::seed_headroom(b).ok(),
            "live Baseline b must admit a seed bound",
        )?;
        let trader_funding = headroom
            .checked_add(currency::USDC.saturating_mul(10_000))
            .ok_or("Baseline trader funding must fit")?;

        assert!(ForeignAssets::mint_into(
            asset.clone(),
            &treasury,
            headroom.saturating_add(currency::USDC),
        )
        .is_ok());
        assert!(ForeignAssets::mint_into(asset.clone(), &trader, trader_funding).is_ok());
        assert!(Market::create_market(
            frame_system::RawOrigin::Signed(crate::configs::epoch_account()).into(),
            MARKET,
            BookKind::Baseline { epoch: EPOCH },
            book_account.clone(),
            fees_account,
            b,
        )
        .is_ok());
        let seed_result = Market::seed(
            frame_system::RawOrigin::Signed(crate::configs::epoch_account()).into(),
            MARKET,
            treasury,
        );
        assert!(seed_result.is_ok(), "Baseline seed failed: {seed_result:?}");

        let open = required(
            crate::telemetry::market_books(),
            "seeded Baseline must produce book telemetry",
        )?;
        let open_row = required(
            open.iter().find(|row| row.market == MARKET),
            "seeded Baseline telemetry row must be present",
        )?;
        assert_eq!(open_row.book_loss_usdc, 0);
        assert_eq!(open_row.lmsr_loss_bound_usdc, headroom);

        let amount = b.checked_div(10).ok_or("Baseline trade amount must fit")?;
        System::set_block_number(10);
        assert!(Market::buy(
            frame_system::RawOrigin::Signed(trader.clone()).into(),
            MARKET,
            ScalarSide::Long,
            amount,
            Balance::MAX,
        )
        .is_ok());
        let after_buy = required(
            pallet_market::Markets::<Runtime>::get(MARKET),
            "bought Baseline must remain live",
        )?;
        let buy_bound = required(
            pallet_market::core_market::maker_loss_at_state(
                after_buy.b,
                after_buy.q_long,
                after_buy.q_short,
            )
            .ok(),
            "post-buy state must admit maker-loss reconstruction",
        )?;
        let buy_rows = required(
            crate::telemetry::market_books(),
            "post-buy Baseline must produce telemetry",
        )?;
        let buy_row = required(
            buy_rows.iter().find(|row| row.market == MARKET),
            "post-buy Baseline telemetry row must be present",
        )?;
        assert!(buy_row.book_loss_usdc.abs_diff(buy_bound) <= 1);

        System::set_block_number(20);
        assert!(Market::sell(
            frame_system::RawOrigin::Signed(trader).into(),
            MARKET,
            ScalarSide::Long,
            amount / 2,
            0,
        )
        .is_ok());
        let after_sell = required(
            pallet_market::Markets::<Runtime>::get(MARKET),
            "sold Baseline must remain live",
        )?;
        assert!(after_sell.fees_accrued > 0);
        assert!(<ForeignAssets as Inspect<crate::AccountId>>::balance(asset, &book_account,) > 0);
        let sell_bound = required(
            pallet_market::core_market::maker_loss_at_state(
                after_sell.b,
                after_sell.q_long,
                after_sell.q_short,
            )
            .ok(),
            "post-sell state must admit maker-loss reconstruction",
        )?;
        let sell_rows = required(
            crate::telemetry::market_books(),
            "post-sell Baseline must produce telemetry",
        )?;
        let sell_row = required(
            sell_rows.iter().find(|row| row.market == MARKET),
            "post-sell Baseline telemetry row must be present",
        )?;
        assert!(sell_row.book_loss_usdc.abs_diff(sell_bound) <= 2);
        assert!(sell_row.book_loss_usdc <= sell_row.lmsr_loss_bound_usdc);
        Ok(())
    })
}

#[test]
fn telemetry_market_book_loss_emits_a_value_above_the_bound() -> Result<(), &'static str> {
    tests::development_ext().execute_with(|| {
        const MARKET: u64 = 49;
        let b = crate::configs::balance_param(b"pol.b_baseline");
        let bound = required(
            pallet_market::core_market::seed_headroom(b).ok(),
            "live Baseline b must admit a seed bound",
        )?;
        let mut book = MarketBook::open(
            MARKET,
            BookKind::Baseline { epoch: 10 },
            tests::account(44),
            tests::account(45),
            b,
        );
        book.fees_accrued = 1;
        pallet_market::Markets::<Runtime>::insert(MARKET, book);
        let impossible_market = MARKET.saturating_sub(1);
        pallet_market::Markets::<Runtime>::insert(
            impossible_market,
            MarketBook::open(
                impossible_market,
                BookKind::Baseline { epoch: 9 },
                tests::account(42),
                tests::account(43),
                Balance::MAX,
            ),
        );
        pallet_market::LivePolCommitments::<Runtime>::put(BoundedVec::truncate_from(vec![
            (impossible_market, Balance::MAX),
            (MARKET, bound),
        ]));

        let rows = required(
            crate::telemetry::market_books(),
            "a computed bound violation must remain observable",
        )?;
        assert_eq!(
            rows.len(),
            1,
            "arithmetic impossibility must exclude only the affected row"
        );
        let row = required(
            rows.iter().find(|row| row.market == MARKET),
            "the violating book row must be present",
        )?;
        assert_eq!(row.book_loss_usdc, bound.saturating_add(1));
        assert_eq!(row.lmsr_loss_bound_usdc, bound);
        assert!(row.book_loss_usdc > row.lmsr_loss_bound_usdc);
        Ok(())
    })
}

#[test]
fn telemetry_mid_window_coverage_projects_only_active_unsealed_windows() -> Result<(), &'static str>
{
    tests::development_ext().execute_with(|| {
        const MARKET: u64 = 51;
        const SEALED_MARKET: u64 = 52;
        const FUTURE_MARKET: u64 = 53;

        let interval = match u32::try_from(<Runtime as pallet_market::Config>::ObsInterval::get()) {
            Ok(interval) if interval > 0 => interval,
            _ => return Err("live observation interval must fit a nonzero block count"),
        };
        let start = interval.saturating_mul(3);
        let now = start.saturating_add(interval.saturating_mul(5));
        let end = start.saturating_add(interval.saturating_mul(10));
        System::set_block_number(now);

        let window = |start, end, observations, sealed| TwapWindow {
            start,
            trailing_start: start,
            end,
            observations,
            stale_events: 0,
            contest_capital_blocks: 0,
            contest_accrued_until: start,
            contest_valid: true,
            close_spot: None,
            sealed,
        };
        pallet_market::DecisionWindows::<Runtime>::insert(
            MARKET,
            BoundedVec::truncate_from(vec![window(start, end, 4, false)]),
        );
        pallet_market::DecisionWindows::<Runtime>::insert(
            SEALED_MARKET,
            BoundedVec::truncate_from(vec![window(start, end, 5, true)]),
        );
        pallet_market::DecisionWindows::<Runtime>::insert(
            FUTURE_MARKET,
            BoundedVec::truncate_from(vec![window(
                now.saturating_add(1),
                end.saturating_add(interval),
                5,
                false,
            )]),
        );

        let rows = required(
            crate::telemetry::mid_window_coverage(),
            "bounded valid windows must produce telemetry",
        )?;
        assert_eq!(rows.len(), 1);
        let row = required(rows.iter().next(), "active window row must be present")?;
        assert_eq!(row.market, MARKET);
        assert_eq!(row.start, start);
        assert_eq!(row.end, end);
        assert_eq!(row.coverage_percent, 80);

        pallet_market::DecisionWindows::<Runtime>::insert(
            MARKET,
            BoundedVec::truncate_from(vec![window(start, end, 6, false)]),
        );
        let capped = required(
            crate::telemetry::mid_window_coverage(),
            "overflow-resistant coverage must remain available",
        )?;
        assert_eq!(capped.len(), 1);
        assert_eq!(
            required(capped.iter().next(), "capped window row must be present")?.coverage_percent,
            100
        );

        pallet_market::DecisionWindows::<Runtime>::insert(
            MARKET,
            BoundedVec::truncate_from(vec![window(start, end, u32::MAX, false)]),
        );
        assert!(
            crate::telemetry::mid_window_coverage().is_none(),
            "counter overflow must fail closed instead of publishing healthy coverage"
        );
        Ok(())
    })
}

#[test]
fn telemetry_pol_compares_each_line_with_only_its_matching_requirement() -> Result<(), &'static str>
{
    use futarchy_primitives::Branch;
    use futarchy_runtime_api::PolComponent;
    use pallet_futarchy_treasury::BudgetLine;
    use pallet_market::core_market::{BookKind, MarketBook};

    tests::development_ext().execute_with(|| {
        let proposal_b = crate::configs::balance_param(b"pol.b.param");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let proposal_headroom = required(
            pallet_market::core_market::seed_headroom(proposal_b).ok(),
            "live proposal b must admit a seed bound",
        )?;
        let baseline_headroom = required(
            pallet_market::core_market::seed_headroom(baseline_b).ok(),
            "live Baseline b must admit a seed bound",
        )?;
        let pol_funding = proposal_headroom.saturating_add(19);
        let baseline_funding = baseline_headroom.saturating_add(23);

        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            let mut lines = state.lines.clone().into_inner();
            lines.retain(|(line, _)| !matches!(line, BudgetLine::Pol | BudgetLine::PolBaseline));
            lines.extend([
                (BudgetLine::Pol, pol_funding),
                (BudgetLine::PolBaseline, baseline_funding),
            ]);
            state.lines = BoundedVec::truncate_from(lines);
        });

        let minimum_balance = ForeignAssets::minimum_balance(crate::usdc_location());
        let standing_rows = required(
            crate::telemetry::pol(),
            "standing Baseline POL accounting must produce telemetry",
        )?;
        let standing_baseline = required(
            standing_rows
                .iter()
                .find(|row| row.component == PolComponent::Baseline),
            "standing Baseline component must be present",
        )?;
        assert_eq!(
            standing_baseline.pol_floor_usdc,
            baseline_headroom.saturating_add(minimum_balance),
            "next-epoch capacity must not report the exact unaffordable cliff",
        );

        // One live proposal book and one live Baseline book: the floors must
        // split by book kind, never pooling Baseline obligations into `POL`.
        let account = |seed: u8| crate::AccountId::from([seed; 32]);
        pallet_market::Markets::<Runtime>::insert(
            1,
            MarketBook::open(
                1,
                BookKind::Decision {
                    proposal: 1,
                    branch: Branch::Accept,
                },
                account(1),
                account(2),
                proposal_b,
            ),
        );
        pallet_market::Markets::<Runtime>::insert(
            2,
            MarketBook::open(
                2,
                BookKind::Baseline { epoch: 1 },
                account(3),
                account(4),
                baseline_b,
            ),
        );
        pallet_market::LivePolCommitments::<Runtime>::put(BoundedVec::truncate_from(vec![
            (1, proposal_headroom),
            (2, baseline_headroom),
        ]));

        let rows = required(
            crate::telemetry::pol(),
            "bounded live POL accounting must produce telemetry",
        )?;
        assert_eq!(rows.len(), 2);
        let pol = required(
            rows.iter().find(|row| row.component == PolComponent::Pol),
            "POL component must be present",
        )?;
        assert_eq!(pol.effective_pol_usdc, pol_funding);
        assert_eq!(pol.pol_floor_usdc, proposal_headroom);
        let baseline = required(
            rows.iter()
                .find(|row| row.component == PolComponent::Baseline),
            "Baseline component must be present",
        )?;
        assert_eq!(baseline.effective_pol_usdc, baseline_funding);
        assert_eq!(
            baseline.pol_floor_usdc,
            baseline_headroom
                .saturating_add(baseline_headroom)
                .saturating_add(minimum_balance.saturating_mul(2)),
            "Baseline floor must carry each book endowment plus next-epoch headroom"
        );

        // Overflowing live obligations must fail closed, not publish a
        // truncated floor — in both the per-kind commitment sum and the
        // Baseline standing-headroom addition.
        pallet_market::Markets::<Runtime>::insert(
            3,
            MarketBook::open(
                3,
                BookKind::Decision {
                    proposal: 2,
                    branch: Branch::Reject,
                },
                account(5),
                account(6),
                proposal_b,
            ),
        );
        pallet_market::LivePolCommitments::<Runtime>::put(BoundedVec::truncate_from(vec![
            (1, Balance::MAX),
            (3, proposal_headroom),
        ]));
        assert!(
            crate::telemetry::pol().is_none(),
            "proposal commitment overflow must fail closed"
        );
        pallet_market::LivePolCommitments::<Runtime>::put(BoundedVec::truncate_from(vec![(
            2,
            Balance::MAX,
        )]));
        assert!(
            crate::telemetry::pol().is_none(),
            "Baseline floor overflow must fail closed"
        );

        // A commitment whose backing book is gone is a collection failure: the
        // family degrades absent per 12 §6.3.
        pallet_market::LivePolCommitments::<Runtime>::put(BoundedVec::truncate_from(vec![(
            7,
            proposal_headroom,
        )]));
        assert!(
            crate::telemetry::pol().is_none(),
            "an unbacked commitment must fail closed"
        );
        Ok(())
    })
}

#[test]
fn telemetry_collateral_reuses_ledger_l2_and_flags_only_positive_residue(
) -> Result<(), &'static str> {
    tests::development_ext().execute_with(|| {
        let before = required(
            crate::telemetry::collateral(),
            "genesis ledger totals must be readable",
        )?;
        let liability_delta = futarchy_primitives::currency::USDC
            .saturating_mul(3)
            .saturating_add(17);
        let residue_delta = 11;

        let mut vault = pallet_conditional_ledger::core_ledger::VaultInfo::open(1);
        vault.escrowed = liability_delta;
        pallet_conditional_ledger::Vaults::<Runtime>::insert(901, vault);

        let ledger_account = <Runtime as pallet_conditional_ledger::Config>::PalletId::get()
            .into_account_truncating();
        let asset = <Runtime as pallet_conditional_ledger::Config>::UsdcAssetId::get();
        assert!(ForeignAssets::mint_into(
            asset,
            &ledger_account,
            liability_delta.saturating_add(residue_delta),
        )
        .is_ok());

        let direct = required(
            pallet_conditional_ledger::Pallet::<Runtime>::collateral_totals().ok(),
            "ledger L-2 checked sums must remain valid",
        )?;
        let telemetry = required(
            crate::telemetry::collateral(),
            "valid ledger totals must produce telemetry",
        )?;
        assert_eq!(direct, (telemetry.custody_usdc, telemetry.liability_usdc));
        assert_eq!(
            telemetry.custody_usdc,
            before
                .custody_usdc
                .saturating_add(liability_delta)
                .saturating_add(residue_delta)
        );
        assert_eq!(
            telemetry.liability_usdc,
            before.liability_usdc.saturating_add(liability_delta)
        );
        assert_eq!(
            telemetry.anomalous_rounding_dust_usdc,
            telemetry
                .custody_usdc
                .saturating_sub(telemetry.liability_usdc)
        );
        Ok(())
    })
}

#[test]
fn telemetry_migration_stall_reports_latched_and_live_detector_states() -> Result<(), &'static str>
{
    tests::development_ext().execute_with(|| {
        assert!(!crate::telemetry::migration_cursor_stalled());

        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Stuck);
        assert!(
            crate::telemetry::migration_cursor_stalled(),
            "a failed/stuck migration cursor is part of the exported stall union"
        );
        pallet_migrations::Cursor::<Runtime>::kill();

        crate::configs::MigrationHaltSources::put(crate::configs::MIGRATION_STALL_HALT);
        assert!(crate::telemetry::migration_cursor_stalled());

        crate::configs::MigrationHaltSources::put(0);
        // The stall predicate now reads the SDK `cursor.started_at` directly
        // (SQ-132(d)(i)); no runtime progress marker is primed. A cursor whose
        // start block is > MIGRATION_STALL_BLOCKS in the past is stalled.
        let since = System::block_number().saturating_add(1);
        let cursor = pallet_migrations::ActiveCursor {
            index: 2,
            inner_cursor: Some(BoundedVec::truncate_from(vec![1, 2, 3])),
            started_at: since,
        };
        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Active(
            cursor,
        ));
        System::set_block_number(
            since
                .saturating_add(futarchy_primitives::kernel::MIGRATION_STALL_BLOCKS)
                .saturating_add(1),
        );
        assert!(crate::telemetry::migration_cursor_stalled());

        System::set_block_number(since);
        assert!(!crate::telemetry::migration_cursor_stalled());
        Ok(())
    })
}

#[test]
fn telemetry_storage_utilization_reports_inner_maxima_and_live_value_lengths(
) -> Result<(), &'static str> {
    tests::development_ext().execute_with(|| {
        const MARKET: u64 = 71;
        const SMALLER_MARKET: u64 = 72;

        pallet_market::TwapCheckpoints::<Runtime>::insert(
            MARKET,
            BoundedVec::truncate_from(vec![
                (1, pallet_market::core_market::TwapCumulative::ZERO),
                (2, pallet_market::core_market::TwapCumulative::ZERO),
            ]),
        );
        pallet_market::TwapCheckpoints::<Runtime>::insert(
            SMALLER_MARKET,
            BoundedVec::truncate_from(vec![(1, pallet_market::core_market::TwapCumulative::ZERO)]),
        );
        pallet_market::DecisionWindows::<Runtime>::insert(
            MARKET,
            BoundedVec::truncate_from(vec![TwapWindow {
                start: 1,
                trailing_start: 1,
                end: 2,
                observations: 0,
                stale_events: 0,
                contest_capital_blocks: 0,
                contest_accrued_until: 1,
                contest_valid: true,
                close_spot: None,
                sealed: false,
            }]),
        );
        pallet_market::DecisionWindowOwners::<Runtime>::insert(
            MARKET,
            BoundedVec::truncate_from(vec![(1, 1, 1, 2), (2, 1, 1, 2), (3, 1, 1, 2)]),
        );

        let rows = required(
            crate::telemetry::storage_utilization(),
            "bounded runtime state must produce utilization rows",
        )?;
        for row in rows.iter() {
            assert!(row.bound > 0);
            assert!(row.entries <= row.bound);
        }

        let find = |name: &[u8]| {
            rows.iter()
                .find(|row| row.map.as_slice() == name)
                .map(|row| (row.entries, row.bound))
        };
        assert_eq!(find(b"market_twap_checkpoints").map(|row| row.0), Some(2));
        assert_eq!(find(b"market_decision_windows").map(|row| row.0), Some(1));
        assert_eq!(
            find(b"market_decision_window_owners").map(|row| row.0),
            Some(3)
        );

        let treasury = crate::FutarchyTreasury::treasury();
        assert_eq!(
            find(b"treasury_budget_lines"),
            Some((
                u32::try_from(treasury.lines.len()).unwrap_or_default(),
                pallet_futarchy_treasury::MAX_BUDGET_LINES_BOUND,
            ))
        );
        assert_eq!(
            find(b"treasury_streams"),
            Some((
                u32::try_from(treasury.streams.len()).unwrap_or_default(),
                pallet_futarchy_treasury::MAX_STREAMS_BOUND,
            ))
        );
        assert_eq!(
            find(b"treasury_pending_outflows"),
            Some((
                u32::try_from(treasury.pending_outflows.len()).unwrap_or_default(),
                pallet_futarchy_treasury::MAX_PENDING_OUTFLOWS_BOUND,
            ))
        );
        assert_eq!(
            find(b"treasury_pol_commitments"),
            Some((
                u32::try_from(treasury.pol_commitments.len()).unwrap_or_default(),
                pallet_futarchy_treasury::MAX_POL_COMMITMENTS_BOUND,
            ))
        );
        assert_eq!(
            find(b"treasury_funded_coretime_periods"),
            Some((
                u32::try_from(treasury.funded_coretime_periods.len()).unwrap_or_default(),
                pallet_futarchy_treasury::MAX_FUNDED_CORETIME_BOUND,
            ))
        );
        assert_eq!(
            find(b"treasury_coretime_quotes"),
            Some((
                u32::try_from(treasury.coretime_quotes.len()).unwrap_or_default(),
                pallet_futarchy_treasury::MAX_FUNDED_CORETIME_BOUND,
            ))
        );
        assert_eq!(
            find(b"epoch_recent_cohort_summaries").map(|row| row.0),
            Some(
                u32::try_from(pallet_epoch::RecentCohortSummaries::<Runtime>::get().len())
                    .unwrap_or_default()
            )
        );
        assert_eq!(
            find(b"epoch_timings").map(|row| row.0),
            Some(
                u32::try_from(pallet_epoch::EpochTimings::<Runtime>::get().len())
                    .unwrap_or_default()
            )
        );
        assert_eq!(
            find(b"oracle_rounds").map(|row| row.0),
            Some(
                u32::try_from(pallet_oracle::Rounds::<Runtime>::iter().count()).unwrap_or_default()
            )
        );
        assert_eq!(
            find(b"oracle_round_schedules").map(|row| row.0),
            Some(
                u32::try_from(pallet_oracle::RoundSchedules::<Runtime>::iter().count())
                    .unwrap_or_default()
            )
        );
        Ok(())
    })
}
