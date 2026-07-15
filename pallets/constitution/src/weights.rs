//! Weights for `pallet-constitution`.
//!
//! The `WeightInfo` trait is the runtime-facing surface required by the Track-A
//! definition of done; the values below are hand-seeded placeholders in the
//! generated-file shape. B5 (15 §4.5) replaces them with PoV-calibrated output
//! from the `frame-benchmarking` CI run against `benchmarking.rs`.

use core::marker::PhantomData;
use frame_support::traits::Get;
use frame_support::weights::{constants::RocksDbWeight, Weight};

/// Weight functions needed for `pallet-constitution`.
pub trait WeightInfo {
    /// Weight of `set_param` (`Params` r:1 w:1).
    fn set_param() -> Weight;
    /// Weight of `set_capability` (`Capabilities` r:1 w:1, worst case: full-table upsert).
    fn set_capability() -> Weight;
    /// Weight of `set_phase_flag` (`PhaseFlags` r:1 w:1).
    fn set_phase_flag() -> Weight;
    /// Weight of `set_release_channel` (`ReleaseChannel` r:0 w:1).
    fn set_release_channel() -> Weight;
    /// Weight of `amend_registry` (`Params` r:1 w:1).
    fn amend_registry() -> Weight;
}

/// Weights expressed through the runtime's configured `DbWeight`.
pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn set_param() -> Weight {
        Weight::from_parts(25_000_000, 3_600)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn set_capability() -> Weight {
        Weight::from_parts(25_000_000, 3_600)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn set_phase_flag() -> Weight {
        Weight::from_parts(20_000_000, 1_600)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn set_release_channel() -> Weight {
        Weight::from_parts(20_000_000, 0).saturating_add(T::DbWeight::get().writes(1))
    }
    fn amend_registry() -> Weight {
        Weight::from_parts(25_000_000, 3_600)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
}

// For tests and backwards compatibility.
impl WeightInfo for () {
    fn set_param() -> Weight {
        Weight::from_parts(25_000_000, 3_600)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn set_capability() -> Weight {
        Weight::from_parts(25_000_000, 3_600)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn set_phase_flag() -> Weight {
        Weight::from_parts(20_000_000, 1_600)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn set_release_channel() -> Weight {
        Weight::from_parts(20_000_000, 0).saturating_add(RocksDbWeight::get().writes(1))
    }
    fn amend_registry() -> Weight {
        Weight::from_parts(25_000_000, 3_600)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
}
