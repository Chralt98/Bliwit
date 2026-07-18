//! Monitoring-only runtime telemetry (12 §6.3, B13).
//!
//! This module is explicitly outside the frozen 02 integration contract. The
//! frontend never consumes it, it carries no integration-contract version, and
//! its shape may change without a 02 §13 bump. Every collection is bounded so
//! an operations scrape remains deterministic and bounded in runtime work.

use futarchy_primitives::{bounds, Balance, BlockNumber, BoundedVec, MarketId};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Storage rows fit the generic bounded-meter registry capacity (13 §4).
pub const MAX_STORAGE_UTILIZATION_ROWS: u32 = bounds::MAX_METERS;
/// Stable snake-case map names are deliberately short and bounded.
pub const MAX_STORAGE_NAME_BYTES: u32 = 48;
/// Every live market may carry eight overlapping registered windows.
pub const MAX_WINDOW_COVERAGE_ROWS: u32 =
    bounds::MAX_LIVE_MARKETS * bounds::MAX_TWAP_WINDOWS_PER_MARKET;
/// The POL telemetry view has one row for each independently funded component.
pub const MAX_POL_TELEMETRY_ROWS: u32 = 2;

/// Audited maker-loss state for one live LMSR book.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct MarketTelemetry {
    pub market: MarketId,
    /// Realized seeded inventory consumed, in USDC base units.
    pub book_loss_usdc: Balance,
    /// `seed_headroom(b) = ceil(b·ln 2)`, in USDC base units.
    pub lmsr_loss_bound_usdc: Balance,
}

/// Live scheduled-observation coverage for one unsealed decision window.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct WindowCoverageTelemetry {
    pub market: MarketId,
    pub start: BlockNumber,
    pub end: BlockNumber,
    pub coverage_percent: u8,
}

/// Independently funded POL components (SQ-266).
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum PolComponent {
    Pol,
    Baseline,
}

/// Effective funding and matching floor for one POL component.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct PolTelemetry {
    pub component: PolComponent,
    pub effective_pol_usdc: Balance,
    pub pol_floor_usdc: Balance,
}

/// The exact custody/liability quantities used by ledger try-state L-2.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct CollateralTelemetry {
    pub custody_usdc: Balance,
    pub liability_usdc: Balance,
    /// Positive unexplained custody residue; zero on exact conservation.
    pub anomalous_rounding_dust_usdc: Balance,
}

/// Occupancy for a bounded storage shape portable metadata cannot pair with a bound.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct StorageUtilizationTelemetry {
    pub map: BoundedVec<u8, MAX_STORAGE_NAME_BYTES>,
    pub entries: u32,
    pub bound: u32,
}

sp_api::decl_runtime_apis! {
    /// Monitoring-only telemetry API owned by 12 §6.3, outside contract 02.
    pub trait TelemetryApi {
        /// Per-live-book realized loss and its identically labeled LMSR bound.
        fn market_books() -> Option<BoundedVec<MarketTelemetry, { bounds::MAX_LIVE_MARKETS }>>;
        /// Every currently active, unsealed decision window.
        fn mid_window_coverage() -> Option<BoundedVec<WindowCoverageTelemetry, MAX_WINDOW_COVERAGE_ROWS>>;
        /// POL and Baseline funding compared independently to their matching requirements.
        fn pol() -> Option<BoundedVec<PolTelemetry, MAX_POL_TELEMETRY_ROWS>>;
        /// Ledger L-2 custody and liability, plus the anomalous positive residue component.
        fn collateral() -> Option<CollateralTelemetry>;
        /// Canonical PB-MIGRATION cursor-stall detector state.
        fn migration_cursor_stalled() -> bool;
        /// Metadata-invisible bounded collection occupancy rows.
        fn storage_utilization() -> Option<BoundedVec<StorageUtilizationTelemetry, MAX_STORAGE_UTILIZATION_ROWS>>;
    }
}
