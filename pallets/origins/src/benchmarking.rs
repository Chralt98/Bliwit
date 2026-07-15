//! `frame-benchmarking` v2 harness for `pallet-origins` (Track-A DoD, 15 §4.5).
//!
//! The shim has no extrinsics and no weight-bearing hooks, so the only
//! runtime-relevant cost to characterize is the base-call-filter evaluation
//! (06 §3.3). The single benchmark drives [`SafetyFilter`] on a call that
//! saturates **both** filter budgets — `MAX_NESTED_LEVELS` levels and
//! `MAX_NESTED_CALLS` total calls — i.e. the worst case the closed wrapper set
//! admits. B5 re-runs this over the concrete `RuntimeCall` (B1a) for the
//! PoV-calibrated `weights.rs`.

use super::*;
use alloc::{vec, vec::Vec};
use frame_benchmarking::v2::*;

/// A wrapper that saturates both filter budgets and still passes: four nested
/// batches (depth = `MAX_NESTED_LEVELS`) whose innermost carries public leaves
/// so the total call count is exactly `MAX_NESTED_CALLS`.
fn worst_case_call() -> FilterCall {
    let depth = MAX_NESTED_LEVELS as usize; // 4 batch calls
    let leaves = MAX_NESTED_CALLS as usize - depth; // 12 leaves ⇒ 16 total
    let mut call = FilterCall::UtilityBatch(
        (0..leaves)
            .map(|_| FilterCall::leaf(CallDomain::Public))
            .collect::<Vec<_>>(),
    );
    for _ in 1..depth {
        call = FilterCall::UtilityBatch(vec![call]);
    }
    call
}

#[benchmarks]
mod benches {
    use super::*;

    #[benchmark]
    fn safety_filter() {
        let call = worst_case_call();

        #[block]
        {
            assert!(SafetyFilter::<ModelClassifier>::contains(&call));
        }
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
