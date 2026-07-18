#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Bleavit runtime-API declarations.
//!
//! The frozen 11-method [`FutarchyApi`] surface is specified by the integration
//! contract (02 §3). The separate [`telemetry`] module is monitoring-only and
//! explicitly outside that contract (12 §6.3). Both are read-only, bounded, and
//! shared with their respective clients.

pub mod telemetry;

#[cfg(not(feature = "std"))]
pub use runtime_decl_for_telemetry_api::TelemetryApi;
pub use telemetry::runtime_decl_for_telemetry_api;
#[cfg(feature = "std")]
pub use telemetry::TelemetryApi;
pub use telemetry::{
    CollateralTelemetry, MarketTelemetry, PolComponent, PolTelemetry, StorageUtilizationTelemetry,
    WindowCoverageTelemetry, MAX_POL_TELEMETRY_ROWS, MAX_STORAGE_NAME_BYTES,
    MAX_STORAGE_UTILIZATION_ROWS, MAX_WINDOW_COVERAGE_ROWS,
};

use futarchy_primitives::{
    bounds, AccountId, Balance, BoundedVec, CohortSummaryView, DecisionStatsView, EpochStatusView,
    MarketId, NavView, OracleRoundView, ParamKey, ParamView, PositionView, ProposalId,
    ProposalSummaryView, QueuedExecutionView, QuoteView, TradeSide, WelfareView,
};

/// Maximum number of queued executions returned by [`FutarchyApi::execution_queue`]
/// (02 §3, `ConstU32<32>`). The queue can never hold more than every live proposal,
/// so the bound is single-homed to `MaxLiveProposals` (rule 4) — exactly how
/// `execution-guard-core` derives its `MAX_QUEUE`.
pub const MAX_QUEUED_EXECUTIONS: u32 = bounds::MAX_LIVE_PROPOSALS;

sp_api::decl_runtime_apis! {
    /// The frozen Bleavit read-only runtime API (02 §3).
    pub trait FutarchyApi {
        /// Epoch clock: index, phase, boundaries, dead-man, freeze and phase flags.
        fn epoch_status() -> EpochStatusView;
        /// All live proposals with market ids, states, decide_at, maturity, ratification.
        fn proposal_summaries() -> BoundedVec<ProposalSummaryView, { bounds::MAX_PROPOSAL_SUMMARIES }>;
        /// Exact quote incl. fee for a hypothetical trade at current book state (USDC-denominated, D-3 wrapper semantics).
        fn quote(market: MarketId, side: TradeSide, amount: Balance) -> QuoteView;
        /// Decision statistics exactly as decide() would read them now (incl. D-4 sizing).
        fn decision_stats(pid: ProposalId) -> Option<DecisionStatsView>;
        /// All positions of an account across proposal, gate and Baseline instruments.
        fn account_positions(who: AccountId) -> BoundedVec<PositionView, { bounds::MAX_ACCOUNT_POSITIONS }>;
        /// Execution queue incl. maturity/grace/version/ratification state.
        fn execution_queue() -> BoundedVec<QueuedExecutionView, { MAX_QUEUED_EXECUTIONS }>;
        /// Current welfare pillars, gates, breach + reserve flags, active MetricSpec.
        fn welfare_current() -> WelfareView;
        /// Typed constitution params (value + bounds + governance metadata) for ≤ 64 keys.
        fn params(keys: BoundedVec<ParamKey, { bounds::MAX_PARAM_KEYS }>) -> BoundedVec<ParamView, { bounds::MAX_PARAM_KEYS }>;
        /// Treasury NAV components (matches the treasury definition in 08), incl. haircut flag.
        fn nav() -> NavView;
        /// Ring of the last 32 cohort settlements (mirrors RecentCohortSummaries, §7.1).
        fn recent_cohorts() -> BoundedVec<CohortSummaryView, { bounds::RECENT_COHORT_SUMMARIES }>;
        /// Oracle rounds currently open.
        fn open_oracle_rounds() -> BoundedVec<OracleRoundView, { bounds::MAX_OPEN_ORACLE_ROUNDS }>;
    }
}

#[cfg(test)]
mod tests;
