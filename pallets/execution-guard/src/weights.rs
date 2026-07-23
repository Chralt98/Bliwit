//! Weights for `pallet-execution-guard`.
//!
//! Hand-seeded worst-case shapes include the 32-entry queue, 256-record ring,
//! 16-call SafetyFilter traversal and sibling callbacks. B5 recalibrates these
//! with assembled-runtime execution time and PoV measurements. In particular,
//! B5 MUST size `execute`/`apply_authorized_upgrade` for the O(queue)
//! load→clear→reinsert persist path (up to roughly 64 queue writes at 32 live
//! entries), and the outer `execute` weight MUST include the maximum dispatched
//! batch weight (`prop.max_weight`, 25% of the block). FRAME post-dispatch info
//! can reduce but cannot increase declared weight, so omitting the inner batch
//! would let a producer over-admit work. These provisional values are not the
//! final PoV-calibrated B5 weights (D-2).

use core::marker::PhantomData;
use frame_support::traits::Get;
use frame_support::weights::{constants::RocksDbWeight, Weight};

pub trait WeightInfo {
    fn execute(calls: u32) -> Weight;
    fn apply_authorized_upgrade(bytes: u32) -> Weight;
    fn expire_failed_execution() -> Weight;
    fn ratify() -> Weight;
    fn reject_stale() -> Weight;
    fn commit_recovery_image() -> Weight;
    fn authorize_phase_four() -> Weight;
    fn qualify_recovery_image(bytes: u32) -> Weight;
}

const STATE_POV: u64 = 96_000;

pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn execute(calls: u32) -> Weight {
        base::<T>(180_000_000, 30, 18)
            .saturating_add(Weight::from_parts(35_000_000, 5_000).saturating_mul(calls.into()))
    }
    fn apply_authorized_upgrade(bytes: u32) -> Weight {
        base::<T>(160_000_000, 12, 10)
            .saturating_add(Weight::from_parts(2_000, 1).saturating_mul(bytes.into()))
    }
    fn expire_failed_execution() -> Weight {
        base::<T>(100_000_000, 24, 16)
    }
    fn ratify() -> Weight {
        base::<T>(75_000_000, 20, 14)
    }
    fn reject_stale() -> Weight {
        base::<T>(105_000_000, 26, 18)
    }
    fn commit_recovery_image() -> Weight {
        base::<T>(90_000_000, 8, 4)
    }
    fn authorize_phase_four() -> Weight {
        base::<T>(20_000_000, 3, 2)
    }
    fn qualify_recovery_image(bytes: u32) -> Weight {
        base::<T>(180_000_000, 16, 3)
            .saturating_add(Weight::from_parts(2_000, 1).saturating_mul(bytes.into()))
    }
}

fn base<T: frame_system::Config>(time: u64, reads: u64, writes: u64) -> Weight {
    Weight::from_parts(time, STATE_POV)
        .saturating_add(T::DbWeight::get().reads(reads))
        .saturating_add(T::DbWeight::get().writes(writes))
}

impl WeightInfo for () {
    fn execute(calls: u32) -> Weight {
        rocks(180_000_000, 30, 18)
            .saturating_add(Weight::from_parts(35_000_000, 5_000).saturating_mul(calls.into()))
    }
    fn apply_authorized_upgrade(bytes: u32) -> Weight {
        rocks(160_000_000, 12, 10)
            .saturating_add(Weight::from_parts(2_000, 1).saturating_mul(bytes.into()))
    }
    fn expire_failed_execution() -> Weight {
        rocks(100_000_000, 24, 16)
    }
    fn ratify() -> Weight {
        rocks(75_000_000, 20, 14)
    }
    fn reject_stale() -> Weight {
        rocks(105_000_000, 26, 18)
    }
    fn commit_recovery_image() -> Weight {
        rocks(90_000_000, 8, 4)
    }
    fn authorize_phase_four() -> Weight {
        rocks(20_000_000, 3, 2)
    }
    fn qualify_recovery_image(bytes: u32) -> Weight {
        rocks(180_000_000, 16, 3)
            .saturating_add(Weight::from_parts(2_000, 1).saturating_mul(bytes.into()))
    }
}

fn rocks(time: u64, reads: u64, writes: u64) -> Weight {
    Weight::from_parts(time, STATE_POV)
        .saturating_add(RocksDbWeight::get().reads(reads))
        .saturating_add(RocksDbWeight::get().writes(writes))
}
