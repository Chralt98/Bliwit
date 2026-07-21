//! Monitoring-only `TelemetryApi` implementation (12 §6.3, B13).
//!
//! Explicitly OUTSIDE the 02 integration contract: the frontend never consumes
//! this surface, it carries no contract version, and its shape may change
//! without a 02 §13 bump. Consumed by the §6.3 chain exporter via `state_call`.
//! Solvency-relevant methods MUST read the same quantities the owning pallet's
//! try-state compares — this module is a window onto audited state, never a
//! second bookkeeping.

use alloc::{vec, vec::Vec};

use frame_support::traits::{fungibles::Inspect, Get};
use futarchy_primitives::{bounds, Balance, BoundedVec, PositionId, PositionKind, ScalarSide};
use futarchy_runtime_api::{
    CollateralTelemetry, MarketTelemetry, PolComponent, PolTelemetry, StorageUtilizationTelemetry,
    WindowCoverageTelemetry, MAX_POL_TELEMETRY_ROWS, MAX_STORAGE_NAME_BYTES,
    MAX_STORAGE_UTILIZATION_ROWS, MAX_WINDOW_COVERAGE_ROWS,
};
use pallet_futarchy_treasury::BudgetLine;
use pallet_market::core_market::{BookKind, MarketBook};

use crate::{
    configs::{
        active_migration_stall_is_live, MarketObsInterval, MigrationHaltSources,
        MIGRATION_STALL_HALT,
    },
    usdc_location, ForeignAssets, FutarchyTreasury, Runtime, System,
};

fn book_positions(book: &MarketBook<crate::AccountId>) -> (PositionId, PositionId) {
    match book.kind {
        BookKind::Decision { proposal, branch } => (
            PositionId::Proposal {
                proposal,
                branch,
                kind: PositionKind::Long,
            },
            PositionId::Proposal {
                proposal,
                branch,
                kind: PositionKind::Short,
            },
        ),
        BookKind::Gate {
            proposal,
            branch,
            gate,
        } => (
            PositionId::Proposal {
                proposal,
                branch,
                kind: PositionKind::GateYes(gate),
            },
            PositionId::Proposal {
                proposal,
                branch,
                kind: PositionKind::GateNo(gate),
            },
        ),
        BookKind::Baseline { epoch } => (
            PositionId::Baseline {
                epoch,
                side: ScalarSide::Long,
            },
            PositionId::Baseline {
                epoch,
                side: ScalarSide::Short,
            },
        ),
    }
}

/// Exact current inventory loss for a seeded book.
///
/// At seeding both outcome inventories contain the exact live POL commitment
/// `H` (normally `seed_headroom(b)`; a rerun commitment preserves the sum of
/// its two separately rounded seeds). At any later LMSR state, path
/// independence gives net curve premium
/// `C(q)-C(0,0)` and the smaller outcome inventory is
/// `H - [max(q)-premium]`; therefore `H - min(inventory)` is the realized
/// worst-world maker loss and tends to `b·ln 2`. It is checked against the
/// canonical `seed_headroom(book.b)` bound. Decision/gate fees live in a
/// separate fee account. Baseline fees are temporarily held in the book as
/// either complete sets (buys) or plain USDC (sells), so both the accumulated
/// fee claim and its plain-USDC realization are removed from the curve P&L.
fn realized_book_loss(
    book: &MarketBook<crate::AccountId>,
    seed_capital: Balance,
) -> Option<Balance> {
    let (long, short) = book_positions(book);
    let retained_sets =
        pallet_conditional_ledger::Positions::<Runtime>::get(long, &book.account).min(
            pallet_conditional_ledger::Positions::<Runtime>::get(short, &book.account),
        );
    let (capital, retained) = if matches!(book.kind, BookKind::Baseline { .. }) {
        let capital = seed_capital.checked_add(book.fees_accrued)?;
        let plain_usdc =
            <ForeignAssets as Inspect<crate::AccountId>>::balance(usdc_location(), &book.account);
        (capital, retained_sets.checked_add(plain_usdc)?)
    } else {
        (seed_capital, retained_sets)
    };
    let loss = capital.checked_sub(retained).unwrap_or_default();
    Some(loss)
}

/// Per-live-book realized maker loss paired with the canonical LMSR bound.
pub fn market_books() -> Option<BoundedVec<MarketTelemetry, { bounds::MAX_LIVE_MARKETS }>> {
    let mut rows = BoundedVec::new();
    // This commitment vector is the market pallet's canonical, sorted index of
    // seeded books whose settlement obligation is still live. Closed-but-not-
    // settled books remain correctly visible; unseeded and terminal books do not.
    for (market, seed_capital) in pallet_market::LivePolCommitments::<Runtime>::get() {
        // A missing backing book makes collection itself impossible, so the
        // family degrades absent per 12 §6.3. Once a book is collected, no
        // independently computable alert condition is allowed to suppress it.
        let book = pallet_market::Markets::<Runtime>::get(market)?;
        let bound = match pallet_market::core_market::seed_headroom(book.b) {
            Ok(bound) => bound,
            Err(_) => continue,
        };
        let Some(book_loss_usdc) = realized_book_loss(&book, seed_capital) else {
            // Arithmetic impossibility excludes only this row. Other live
            // books remain observable; computed alert conditions never do.
            continue;
        };
        rows.try_push(MarketTelemetry {
            market,
            book_loss_usdc,
            lmsr_loss_bound_usdc: bound,
        })
        .ok()?;
    }
    Some(rows)
}

/// Project scheduled-observation coverage for every active unsealed window.
pub fn mid_window_coverage() -> Option<BoundedVec<WindowCoverageTelemetry, MAX_WINDOW_COVERAGE_ROWS>>
{
    let now = System::block_number();
    let interval = u32::try_from(MarketObsInterval::get()).ok()?;
    if interval == 0 {
        return None;
    }
    let mut windows = pallet_market::DecisionWindows::<Runtime>::iter().collect::<Vec<_>>();
    windows.sort_unstable_by_key(|(market, _)| *market);
    let mut rows = Vec::new();
    for (market, registered) in windows {
        for window in registered {
            if window.sealed || now < window.start || now >= window.end {
                continue;
            }
            let elapsed = now.checked_sub(window.start)?;
            let expected = elapsed.checked_div(interval)?;
            let coverage = if expected == 0 {
                100
            } else {
                window
                    .observations
                    .checked_mul(100)?
                    .checked_div(expected)?
                    .min(100)
            };
            rows.push(WindowCoverageTelemetry {
                market,
                start: window.start,
                end: window.end,
                coverage_percent: u8::try_from(coverage).ok()?,
            });
        }
    }
    rows.sort_unstable_by_key(|row| (row.market, row.start, row.end));
    BoundedVec::try_from(rows).ok()
}

/// Independently funded POL components and their matching requirements.
pub fn pol() -> Option<BoundedVec<PolTelemetry, MAX_POL_TELEMETRY_ROWS>> {
    // Live obligations split by book kind at the market pallet's identity-keyed
    // source (the treasury mirror is amount-only): Baseline books are seeded
    // from the dedicated `POL_BASELINE` line (08 §4.3), so their obligations
    // belong to the Baseline component's floor — held against `POL`, a live
    // Baseline book would fire the `pol` alert while its dedicated funding is
    // correct.
    let mut proposal_live: Balance = 0;
    let mut baseline_live: Balance = 0;
    let mut baseline_books: Balance = 0;
    for (market, amount) in pallet_market::LivePolCommitments::<Runtime>::get() {
        // A missing backing book makes collection itself impossible, so the
        // family degrades absent per 12 §6.3.
        let book = pallet_market::Markets::<Runtime>::get(market)?;
        let component = if matches!(book.kind, BookKind::Baseline { .. }) {
            baseline_books = baseline_books.checked_add(1)?;
            &mut baseline_live
        } else {
            &mut proposal_live
        };
        *component = component.checked_add(amount)?;
    }
    // Besides its live books, the dedicated line is standing funding: it must
    // retain the live-Params capacity to seed the next epoch's mandatory
    // Baseline book, even when the current epoch has no proposals (08 §4.3).
    let standing_baseline =
        pallet_market::core_market::seed_headroom(crate::configs::balance_param(b"pol.b_baseline"))
            .ok()?;
    // 03 §7 R-4 / 04 §8.3: each live Baseline book consumed one live
    // min_balance endowment outside pol.budget_epoch, and the standing capacity
    // for the next mandatory book must include its endowment too. Count only
    // identity-checked live commitments plus that one explicit standing book.
    let baseline_books_with_standing = baseline_books.checked_add(1)?;
    let baseline_endowments = ForeignAssets::minimum_balance(usdc_location())
        .checked_mul(baseline_books_with_standing)?;
    BoundedVec::try_from(vec![
        PolTelemetry {
            component: PolComponent::Pol,
            effective_pol_usdc: FutarchyTreasury::line_balance(BudgetLine::Pol),
            pol_floor_usdc: proposal_live,
        },
        PolTelemetry {
            component: PolComponent::Baseline,
            effective_pol_usdc: FutarchyTreasury::line_balance(BudgetLine::PolBaseline),
            pol_floor_usdc: baseline_live
                .checked_add(standing_baseline)?
                .checked_add(baseline_endowments)?,
        },
    ])
    .ok()
}

/// Ledger custody/liability totals from the pallet's L-2 helper.
pub fn collateral() -> Option<CollateralTelemetry> {
    let (custody_usdc, liability_usdc) =
        pallet_conditional_ledger::Pallet::<Runtime>::collateral_totals().ok()?;
    // The ledger has no independent dust accumulator: claimant-adverse residue
    // is transferred out to INSURANCE when a vault is swept. Consequently the
    // only honest anomalous-dust component is unexplained positive custody
    // beyond the exact L-2 liability. Undercollateralization remains visible in
    // the signed drift series itself.
    let anomalous_rounding_dust_usdc = custody_usdc.checked_sub(liability_usdc).unwrap_or_default();
    Some(CollateralTelemetry {
        custody_usdc,
        liability_usdc,
        anomalous_rounding_dust_usdc,
    })
}

/// Canonical migration stall signal used by PB-MIGRATION.
pub fn migration_cursor_stalled() -> bool {
    let halt = MigrationHaltSources::get() & MIGRATION_STALL_HALT != 0;
    let live = match pallet_migrations::Cursor::<Runtime>::get() {
        Some(pallet_migrations::MigrationCursor::Active(cursor)) => {
            active_migration_stall_is_live(&cursor)
        }
        Some(pallet_migrations::MigrationCursor::Stuck) => true,
        None => false,
    };
    halt || live
}

fn max_len<I>(mut lengths: I) -> Option<u32>
where
    I: Iterator<Item = usize>,
{
    lengths.try_fold(0_u32, |largest, length| {
        Some(largest.max(u32::try_from(length).ok()?))
    })
}

fn storage_row(map: &'static str, entries: u32, bound: u32) -> Option<StorageUtilizationTelemetry> {
    if bound == 0 {
        return None;
    }
    Some(StorageUtilizationTelemetry {
        map: BoundedVec::<u8, MAX_STORAGE_NAME_BYTES>::try_from(map.as_bytes().to_vec()).ok()?,
        entries,
        bound,
    })
}

/// Bounded storage shapes whose const-generic limits are absent from portable metadata.
pub fn storage_utilization(
) -> Option<BoundedVec<StorageUtilizationTelemetry, MAX_STORAGE_UTILIZATION_ROWS>> {
    let treasury = FutarchyTreasury::treasury();
    // The runtime-API row bound is derived from the market storage shape in
    // B13's monitoring surface. Reuse it here so the occupancy denominator and
    // the returned-collection bound cannot drift independently.
    let windows_per_market = MAX_WINDOW_COVERAGE_ROWS.checked_div(bounds::MAX_LIVE_MARKETS)?;
    let rows = vec![
        storage_row(
            "market_twap_checkpoints",
            max_len(pallet_market::TwapCheckpoints::<Runtime>::iter_values().map(|v| v.len()))?,
            windows_per_market,
        )?,
        storage_row(
            "market_decision_windows",
            max_len(pallet_market::DecisionWindows::<Runtime>::iter_values().map(|v| v.len()))?,
            windows_per_market,
        )?,
        storage_row(
            "market_decision_window_owners",
            max_len(
                pallet_market::DecisionWindowOwners::<Runtime>::iter_values().map(|v| v.len()),
            )?,
            bounds::MAX_LIVE_PROPOSALS.checked_mul(windows_per_market)?,
        )?,
        storage_row(
            "treasury_budget_lines",
            u32::try_from(treasury.lines.len()).ok()?,
            pallet_futarchy_treasury::MAX_BUDGET_LINES_BOUND,
        )?,
        storage_row(
            "treasury_streams",
            u32::try_from(treasury.streams.len()).ok()?,
            pallet_futarchy_treasury::MAX_STREAMS_BOUND,
        )?,
        storage_row(
            "treasury_pending_outflows",
            u32::try_from(treasury.pending_outflows.len()).ok()?,
            pallet_futarchy_treasury::MAX_PENDING_OUTFLOWS_BOUND,
        )?,
        storage_row(
            "treasury_pol_commitments",
            u32::try_from(treasury.pol_commitments.len()).ok()?,
            pallet_futarchy_treasury::MAX_POL_COMMITMENTS_BOUND,
        )?,
        storage_row(
            "treasury_vit_budget_lines",
            u32::try_from(treasury.vit_lines.len()).ok()?,
            pallet_futarchy_treasury::MAX_BUDGET_LINES_BOUND,
        )?,
        storage_row(
            "treasury_funded_coretime_periods",
            u32::try_from(treasury.funded_coretime_periods.len()).ok()?,
            pallet_futarchy_treasury::MAX_FUNDED_CORETIME_BOUND,
        )?,
        storage_row(
            "treasury_coretime_quotes",
            u32::try_from(treasury.coretime_quotes.len()).ok()?,
            pallet_futarchy_treasury::MAX_FUNDED_CORETIME_BOUND,
        )?,
        storage_row(
            "epoch_recent_cohort_summaries",
            u32::try_from(pallet_epoch::RecentCohortSummaries::<Runtime>::get().len()).ok()?,
            pallet_epoch::RECENT_COHORTS_BOUND,
        )?,
        storage_row(
            "epoch_timings",
            u32::try_from(pallet_epoch::EpochTimings::<Runtime>::get().len()).ok()?,
            pallet_epoch::RECENT_COHORTS_BOUND,
        )?,
        storage_row(
            "oracle_rounds",
            u32::try_from(pallet_oracle::Rounds::<Runtime>::iter().count()).ok()?,
            pallet_oracle::MAX_ROUNDS_BOUND,
        )?,
        storage_row(
            "oracle_round_schedules",
            u32::try_from(pallet_oracle::RoundSchedules::<Runtime>::iter().count()).ok()?,
            pallet_oracle::MAX_ROUNDS_BOUND,
        )?,
    ];
    BoundedVec::try_from(rows).ok()
}
