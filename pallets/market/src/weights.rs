//! Weight interface for `pallet-market`.
//!
//! B5 replaces these conservative generated-style references with measured PoV
//! weights. Every dispatchable plus the internal admin and try-state operations
//! has a distinct entry so runtime wiring cannot silently omit coverage.

use core::marker::PhantomData;
use frame_support::{traits::Get, weights::Weight};

pub trait WeightInfo {
    fn buy() -> Weight;
    fn sell() -> Weight;
    fn crank_observe() -> Weight;
    fn reap() -> Weight;
    fn create_market() -> Weight;
    fn seed() -> Weight;
    fn close() -> Weight;
    fn try_state() -> Weight;
    fn freeze_creation() -> Weight;
    fn set_frozen() -> Weight;
}

const REF_READS: u64 = 16;
const REF_WRITES: u64 = 16;
const REF_COMPUTE: u64 = 50_000_000;

fn reference() -> Weight {
    Weight::from_parts(REF_COMPUTE, 0)
        .saturating_add(Weight::from_parts(25_000 * REF_READS, 0))
        .saturating_add(Weight::from_parts(100_000 * REF_WRITES, 0))
}

pub struct SubstrateWeight<T>(PhantomData<T>);

macro_rules! ref_impl {
    ($($name:ident),+ $(,)?) => {
        impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
            $(fn $name() -> Weight {
                let db = T::DbWeight::get();
                reference()
                    .saturating_add(db.reads(REF_READS))
                    .saturating_add(db.writes(REF_WRITES))
            })+
        }

        impl WeightInfo for () {
            $(fn $name() -> Weight { reference() })+
        }
    };
}

ref_impl!(
    buy,
    sell,
    crank_observe,
    reap,
    create_market,
    seed,
    close,
    try_state,
    freeze_creation,
    set_frozen,
);
