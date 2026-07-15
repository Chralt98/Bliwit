//! Weights for `pallet-attestor`.
//!
//! Hand-seeded placeholders in generated-file shape. B5 replaces these with
//! PoV-calibrated `frame-benchmarking` output.

use core::marker::PhantomData;
use frame_support::traits::Get;
use frame_support::weights::{constants::RocksDbWeight, Weight};

/// Weight functions needed for `pallet-attestor`.
pub trait WeightInfo {
    fn set_members() -> Weight;
    fn attest() -> Weight;
    fn challenge_attestation() -> Weight;
    fn resolve_challenge() -> Weight;
}

pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn set_members() -> Weight {
        Weight::from_parts(25_000_000, 4_000)
            .saturating_add(T::DbWeight::get().reads(3))
            .saturating_add(T::DbWeight::get().writes(3))
    }

    fn attest() -> Weight {
        Weight::from_parts(35_000_000, 12_000)
            .saturating_add(T::DbWeight::get().reads(3))
            .saturating_add(T::DbWeight::get().writes(3))
    }

    fn challenge_attestation() -> Weight {
        Weight::from_parts(35_000_000, 12_000)
            .saturating_add(T::DbWeight::get().reads(3))
            .saturating_add(T::DbWeight::get().writes(3))
    }

    fn resolve_challenge() -> Weight {
        Weight::from_parts(40_000_000, 12_000)
            .saturating_add(T::DbWeight::get().reads(3))
            .saturating_add(T::DbWeight::get().writes(3))
    }
}

impl WeightInfo for () {
    fn set_members() -> Weight {
        Weight::from_parts(25_000_000, 4_000)
            .saturating_add(RocksDbWeight::get().reads(3))
            .saturating_add(RocksDbWeight::get().writes(3))
    }

    fn attest() -> Weight {
        Weight::from_parts(35_000_000, 12_000)
            .saturating_add(RocksDbWeight::get().reads(3))
            .saturating_add(RocksDbWeight::get().writes(3))
    }

    fn challenge_attestation() -> Weight {
        Weight::from_parts(35_000_000, 12_000)
            .saturating_add(RocksDbWeight::get().reads(3))
            .saturating_add(RocksDbWeight::get().writes(3))
    }

    fn resolve_challenge() -> Weight {
        Weight::from_parts(40_000_000, 12_000)
            .saturating_add(RocksDbWeight::get().reads(3))
            .saturating_add(RocksDbWeight::get().writes(3))
    }
}
