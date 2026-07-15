//! Weights for `pallet-origins`.
//!
//! The shim declares no extrinsics and no weight-bearing hooks (`try_state` is
//! try-runtime-only). The one runtime-relevant cost it owns is evaluating the
//! base call filter, whose recursion the spec bounds (06 §3.3: "filter
//! evaluation weight is bounded" — ≤ `MAX_NESTED_LEVELS` levels /
//! `MAX_NESTED_CALLS` calls). `WeightInfo::safety_filter` is that worst-case
//! bound; it touches no storage, so its proof size is zero. B5 (15 §4.5)
//! replaces the placeholder with PoV-calibrated output from `benchmarking.rs`,
//! re-run over the concrete `RuntimeCall` at B1a.

use core::marker::PhantomData;
use frame_support::weights::Weight;

/// Weight functions needed for `pallet-origins`.
pub trait WeightInfo {
    /// Worst-case base-call-filter evaluation over a maximally nested/wide
    /// wrapper (06 §3.3). Pure compute — no storage reads or writes.
    fn safety_filter() -> Weight;
}

/// Weights expressed through the runtime's configured environment.
pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn safety_filter() -> Weight {
        // Bounded recursion, no DB access; placeholder ref-time until B5.
        Weight::from_parts(15_000_000, 0)
    }
}

// For tests and mocks.
impl WeightInfo for () {
    fn safety_filter() -> Weight {
        Weight::from_parts(15_000_000, 0)
    }
}
