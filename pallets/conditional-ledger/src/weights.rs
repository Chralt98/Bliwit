//! Weight interface for `pallet-conditional-ledger`.
//!
//! PoV-calibrated weights are generated in B5 (15 §4.5); until then the harness,
//! the `WeightInfo` trait and a conservative `SubstrateWeight<T>` reference impl
//! land with the pallet (Track-A DoD). Every extrinsic and the `sweep_dust`
//! cranks carry a `#[pallet::weight(T::WeightInfo::…)]`.

use core::marker::PhantomData;
use frame_support::{traits::Get, weights::Weight};

/// Weights for every call in the pallet (plus the keeper cranks).
pub trait WeightInfo {
    fn split() -> Weight;
    fn merge() -> Weight;
    fn split_scalar() -> Weight;
    fn merge_scalar() -> Weight;
    fn split_gate() -> Weight;
    fn merge_gate() -> Weight;
    fn transfer() -> Weight;
    fn split_baseline() -> Weight;
    fn merge_baseline() -> Weight;
    fn resolve() -> Weight;
    fn void() -> Weight;
    fn settle_scalar() -> Weight;
    fn settle_gate() -> Weight;
    fn settle_baseline() -> Weight;
    fn redeem() -> Weight;
    fn redeem_scalar() -> Weight;
    fn redeem_scalar_pair() -> Weight;
    fn redeem_gate() -> Weight;
    fn redeem_void() -> Weight;
    fn redeem_baseline() -> Weight;
    fn redeem_baseline_pair() -> Weight;
    fn sweep_dust() -> Weight;
    fn sweep_dust_baseline() -> Weight;
    fn set_split_paused() -> Weight;
    fn set_frozen() -> Weight;
}

/// A conservative reference weight (03 §5 weight drivers: a handful of map
/// reads/writes plus at most two asset transfers). Replaced by generated weights
/// in B5; the magnitudes here are placeholders, not benchmarked figures.
const REF_READS: u64 = 8;
const REF_WRITES: u64 = 8;
const REF_COMPUTE: u64 = 30_000_000;

fn reference() -> Weight {
    Weight::from_parts(REF_COMPUTE, 0)
        .saturating_add(Weight::from_parts(25_000 * REF_READS, 0))
        .saturating_add(Weight::from_parts(100_000 * REF_WRITES, 0))
}

/// Generated-style weights parameterised by the runtime's `DbWeight` — a stub
/// until B5 replaces it with `frame-benchmarking` output.
pub struct SubstrateWeight<T>(PhantomData<T>);

macro_rules! ref_impl {
    ($($name:ident),+ $(,)?) => {
        impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
            $( fn $name() -> Weight {
                let db = T::DbWeight::get();
                reference()
                    .saturating_add(db.reads(REF_READS))
                    .saturating_add(db.writes(REF_WRITES))
            } )+
        }
        impl WeightInfo for () {
            $( fn $name() -> Weight { reference() } )+
        }
    };
}

ref_impl!(
    split,
    merge,
    split_scalar,
    merge_scalar,
    split_gate,
    merge_gate,
    transfer,
    split_baseline,
    merge_baseline,
    resolve,
    void,
    settle_scalar,
    settle_gate,
    settle_baseline,
    redeem,
    redeem_scalar,
    redeem_scalar_pair,
    redeem_gate,
    redeem_void,
    redeem_baseline,
    redeem_baseline_pair,
    sweep_dust,
    sweep_dust_baseline,
    set_split_paused,
    set_frozen,
);
