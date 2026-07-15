//! Weight interface for `pallet-registry`.
//!
//! Zero-cost `()` weights until B5 replaces them with PoV-calibrated generated
//! weights (15 §4.5). Every `#[pallet::call]` extrinsic carries a method here.

use frame_support::weights::Weight;

/// Runtime-provided weights for the registry extrinsics.
pub trait WeightInfo {
    /// `file` — one bond escrow + one filing insert.
    fn file() -> Weight;
    /// `challenge_filing` — one bond escrow + one filing mutate.
    fn challenge_filing() -> Weight;
    /// `ack_observed` — O(1) ack.
    fn ack_observed() -> Weight;
    /// `crank_close` — bounded batch (≤ `REG_CLOSE_BATCH`) close.
    fn crank_close() -> Weight;
    /// `resolve_challenge` — one filing mutate + 40 / 60 custody.
    fn resolve_challenge() -> Weight;
    /// `close_epoch` — aggregate over ≤ 64 filings.
    fn close_epoch() -> Weight;
    /// `reap_epoch` — drain a closed epoch's filings + acks.
    fn reap_epoch() -> Weight;
}

impl WeightInfo for () {
    fn file() -> Weight {
        Weight::zero()
    }
    fn challenge_filing() -> Weight {
        Weight::zero()
    }
    fn ack_observed() -> Weight {
        Weight::zero()
    }
    fn crank_close() -> Weight {
        Weight::zero()
    }
    fn resolve_challenge() -> Weight {
        Weight::zero()
    }
    fn close_epoch() -> Weight {
        Weight::zero()
    }
    fn reap_epoch() -> Weight {
        Weight::zero()
    }
}
