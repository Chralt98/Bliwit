//! Weights for `pallet-epoch`.
//!
//! Hand-seeded generated-shape values; B5 recalibrates execution time and PoV
//! against the assembled runtime. Every method includes the bounded aggregate
//! reads/writes and sibling seam calls exercised by its worst case.

use core::marker::PhantomData;
use frame_support::traits::Get;
use frame_support::weights::{constants::RocksDbWeight, Weight};

pub trait WeightInfo {
    fn submit() -> Weight;
    fn withdraw() -> Weight;
    fn tick(items: u32) -> Weight;
    fn decide() -> Weight;
    fn settle_cohort(items: u32) -> Weight;
    fn set_next_epoch_length() -> Weight;
    fn delay_once() -> Weight;
    fn mark_executed() -> Weight;
    fn mark_failed_executed() -> Weight;
    fn retry_exhausted_to_measurement() -> Weight;
    fn expire_or_stale_queue() -> Weight;
    fn force_reject_process_hold() -> Weight;
    fn void_cohort(items: u32) -> Weight;
    fn set_intake_paused() -> Weight;
    fn finalize_epoch_baseline() -> Weight;
    fn bind_ratification() -> Weight;
}

const STATE_POV: u64 = 48_000;

pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn submit() -> Weight {
        base::<T>(45_000_000, 13, 10)
    }
    fn withdraw() -> Weight {
        base::<T>(40_000_000, 12, 10)
    }
    fn tick(items: u32) -> Weight {
        base::<T>(55_000_000, 12, 10)
            .saturating_add(Weight::from_parts(30_000_000, 4_000).saturating_mul(items.into()))
    }
    fn decide() -> Weight {
        base::<T>(140_000_000, 24, 14)
    }
    fn settle_cohort(items: u32) -> Weight {
        base::<T>(85_000_000, 16, 12)
            .saturating_add(Weight::from_parts(45_000_000, 5_000).saturating_mul(items.into()))
    }
    fn set_next_epoch_length() -> Weight {
        base::<T>(30_000_000, 12, 10)
    }
    fn delay_once() -> Weight {
        base::<T>(40_000_000, 12, 10)
    }
    fn mark_executed() -> Weight {
        base::<T>(70_000_000, 16, 12)
    }
    fn mark_failed_executed() -> Weight {
        base::<T>(40_000_000, 12, 10)
    }
    fn retry_exhausted_to_measurement() -> Weight {
        base::<T>(70_000_000, 16, 12)
    }
    fn expire_or_stale_queue() -> Weight {
        base::<T>(70_000_000, 16, 12)
    }
    fn force_reject_process_hold() -> Weight {
        base::<T>(70_000_000, 16, 12)
    }
    fn void_cohort(items: u32) -> Weight {
        base::<T>(55_000_000, 14, 10)
            .saturating_add(Weight::from_parts(20_000_000, 2_000).saturating_mul(items.into()))
    }
    fn set_intake_paused() -> Weight {
        base::<T>(20_000_000, 2, 1)
    }
    fn finalize_epoch_baseline() -> Weight {
        base::<T>(40_000_000, 12, 10)
    }
    fn bind_ratification() -> Weight {
        // Binding is an epoch aggregate mutation, not a single-key write:
        // worst case it loads and rewrites both bounded proposal halves and
        // cohorts, then validates the referendum/preimage and updates the
        // guard join. Keep the fallback conservative until the next assembled
        // runtime benchmark refresh.
        base::<T>(2_100_000_000, 240, 140)
    }
}

fn base<T: frame_system::Config>(time: u64, reads: u64, writes: u64) -> Weight {
    Weight::from_parts(time, STATE_POV)
        .saturating_add(T::DbWeight::get().reads(reads))
        .saturating_add(T::DbWeight::get().writes(writes))
}

impl WeightInfo for () {
    fn submit() -> Weight {
        rocks(45_000_000, 13, 10)
    }
    fn withdraw() -> Weight {
        rocks(40_000_000, 12, 10)
    }
    fn tick(items: u32) -> Weight {
        rocks(55_000_000, 12, 10)
            .saturating_add(Weight::from_parts(30_000_000, 4_000).saturating_mul(items.into()))
    }
    fn decide() -> Weight {
        rocks(140_000_000, 24, 14)
    }
    fn settle_cohort(items: u32) -> Weight {
        rocks(85_000_000, 16, 12)
            .saturating_add(Weight::from_parts(45_000_000, 5_000).saturating_mul(items.into()))
    }
    fn set_next_epoch_length() -> Weight {
        rocks(30_000_000, 12, 10)
    }
    fn delay_once() -> Weight {
        rocks(40_000_000, 12, 10)
    }
    fn mark_executed() -> Weight {
        rocks(70_000_000, 16, 12)
    }
    fn mark_failed_executed() -> Weight {
        rocks(40_000_000, 12, 10)
    }
    fn retry_exhausted_to_measurement() -> Weight {
        rocks(70_000_000, 16, 12)
    }
    fn expire_or_stale_queue() -> Weight {
        rocks(70_000_000, 16, 12)
    }
    fn force_reject_process_hold() -> Weight {
        rocks(70_000_000, 16, 12)
    }
    fn void_cohort(items: u32) -> Weight {
        rocks(55_000_000, 14, 10)
            .saturating_add(Weight::from_parts(20_000_000, 2_000).saturating_mul(items.into()))
    }
    fn set_intake_paused() -> Weight {
        rocks(20_000_000, 2, 1)
    }
    fn finalize_epoch_baseline() -> Weight {
        rocks(40_000_000, 12, 10)
    }
    fn bind_ratification() -> Weight {
        rocks(2_100_000_000, 240, 140)
    }
}

fn rocks(time: u64, reads: u64, writes: u64) -> Weight {
    Weight::from_parts(time, STATE_POV)
        .saturating_add(RocksDbWeight::get().reads(reads))
        .saturating_add(RocksDbWeight::get().writes(writes))
}
