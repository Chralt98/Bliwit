//! Weights for `pallet-guardian`.
//!
//! The `WeightInfo` trait is the runtime-facing surface required by the Track-A
//! definition of done; the values below are hand-seeded placeholders in the
//! generated-file shape. B5 (15 §4.5) replaces them with PoV-calibrated output
//! from the `frame-benchmarking` CI run against `benchmarking.rs`.

use core::marker::PhantomData;
use frame_support::traits::Get;
use frame_support::weights::{constants::RocksDbWeight, Weight};

/// Weight functions needed for `pallet-guardian`.
pub trait WeightInfo {
    /// Weight of `set_members`.
    fn set_members() -> Weight;
    /// Weight of `propose_action`.
    fn propose_action() -> Weight;
    /// Weight of `approve_action` (worst case: the fifth approval dispatches +
    /// schedules a review).
    fn approve_action() -> Weight;
    /// Weight of `ratify_action`.
    fn ratify_action() -> Weight;
    /// Weight of `renew_playbook`.
    fn renew_playbook() -> Weight;
    /// Weight of the ratify-track T24 verdict.
    fn uphold_veto() -> Weight;
    /// Weight of a guardian-track recall enactment.
    fn recall() -> Weight;
    /// Weight of a guardian-track playbook availability toggle.
    fn set_playbook_registered() -> Weight;
    /// Weight of the per-block maintenance hook (expire playbooks + enforce
    /// review deadlines over the bounded sets).
    fn on_initialize() -> Weight;
}

/// Weights expressed through the runtime's configured `DbWeight`.
pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn set_members() -> Weight {
        Weight::from_parts(30_000_000, 4_000)
            .saturating_add(T::DbWeight::get().reads(2))
            .saturating_add(T::DbWeight::get().writes(3))
    }
    fn propose_action() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(4))
    }
    fn approve_action() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(T::DbWeight::get().reads(220))
            .saturating_add(T::DbWeight::get().writes(150))
    }
    fn ratify_action() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(T::DbWeight::get().reads(220))
            .saturating_add(T::DbWeight::get().writes(150))
    }
    fn renew_playbook() -> Weight {
        Weight::from_parts(30_000_000, 4_000)
            .saturating_add(T::DbWeight::get().reads(8))
            .saturating_add(T::DbWeight::get().writes(8))
    }
    fn uphold_veto() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(T::DbWeight::get().reads(220))
            .saturating_add(T::DbWeight::get().writes(150))
    }
    fn recall() -> Weight {
        Weight::from_parts(75_000_000, 9_000)
            .saturating_add(T::DbWeight::get().reads(14))
            .saturating_add(T::DbWeight::get().writes(14))
    }
    fn set_playbook_registered() -> Weight {
        Weight::from_parts(15_000_000, 2_000)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn on_initialize() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(T::DbWeight::get().reads(220))
            .saturating_add(T::DbWeight::get().writes(150))
    }
}

// For tests and backwards compatibility.
impl WeightInfo for () {
    fn set_members() -> Weight {
        Weight::from_parts(30_000_000, 4_000)
            .saturating_add(RocksDbWeight::get().reads(2))
            .saturating_add(RocksDbWeight::get().writes(3))
    }
    fn propose_action() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(4))
    }
    fn approve_action() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(RocksDbWeight::get().reads(220))
            .saturating_add(RocksDbWeight::get().writes(150))
    }
    fn ratify_action() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(RocksDbWeight::get().reads(220))
            .saturating_add(RocksDbWeight::get().writes(150))
    }
    fn renew_playbook() -> Weight {
        Weight::from_parts(30_000_000, 4_000)
            .saturating_add(RocksDbWeight::get().reads(8))
            .saturating_add(RocksDbWeight::get().writes(8))
    }
    fn uphold_veto() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(RocksDbWeight::get().reads(220))
            .saturating_add(RocksDbWeight::get().writes(150))
    }
    fn recall() -> Weight {
        Weight::from_parts(75_000_000, 9_000)
            .saturating_add(RocksDbWeight::get().reads(14))
            .saturating_add(RocksDbWeight::get().writes(14))
    }
    fn set_playbook_registered() -> Weight {
        Weight::from_parts(15_000_000, 2_000)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn on_initialize() -> Weight {
        Weight::from_parts(1_950_000_000, 183_055)
            .saturating_add(RocksDbWeight::get().reads(220))
            .saturating_add(RocksDbWeight::get().writes(150))
    }
}
