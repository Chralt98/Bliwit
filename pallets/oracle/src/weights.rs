//! Weights for `pallet-oracle`.
//!
//! The `WeightInfo` trait is the runtime-facing surface required by the Track-A
//! definition of done; the values below are hand-seeded placeholders in the
//! generated-file shape. B5 (15 §4.5) replaces them with PoV-calibrated output
//! from the `frame-benchmarking` CI run against `benchmarking.rs`. Because each
//! extrinsic hydrates the whole (bounded) aggregate, the real drivers are the
//! registry/round counts — parameterized here on the variable-length calls
//! (`recompute_proof` proof bytes, `crank_round_close` batch).

use core::marker::PhantomData;
use frame_support::traits::Get;
use frame_support::weights::{constants::RocksDbWeight, Weight};

/// Weight functions needed for `pallet-oracle`.
pub trait WeightInfo {
    /// Weight of `register_reporter` (`Reporters` r:all w:1).
    fn register_reporter() -> Weight;
    /// Weight of `deregister_reporter` (`Reporters` r:all w:1).
    fn deregister_reporter() -> Weight;
    /// Weight of `report` (`Rounds`/`Reporters`/`ComponentValues` r:all w:1).
    fn report() -> Weight;
    /// Weight of `challenge` (`Rounds` r:all w:1).
    fn challenge() -> Weight;
    /// Weight of `recompute_proof` over a `bytes`-long proof (07 §9).
    fn recompute_proof(bytes: u32) -> Weight;
    /// Weight of `register_watchtower` (`Watchtowers` r:all w:1).
    fn register_watchtower() -> Weight;
    /// Weight of `ack_observed` (`Rounds`/`AckRecords` r:all w:1).
    fn ack_observed() -> Weight;
    /// Weight of `crank_round_close` over a `batch` of matured rounds.
    fn crank_round_close(batch: u32) -> Weight;
    /// Weight of `crank_reserve_probe` (`ReserveHealth` r:1 w:1).
    fn crank_reserve_probe() -> Weight;
    /// Worst-case authenticated reserve-probe `QueryResponse` callback.
    fn reserve_probe_result() -> Weight;
    /// Weight of `adjudicate` (`Rounds`/`ComponentValues`/`Reporters` r:all w:1).
    fn adjudicate() -> Weight;
}

/// Weights expressed through the runtime's configured `DbWeight`.
pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register_reporter() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn deregister_reporter() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn report() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn challenge() -> Weight {
        Weight::from_parts(35_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn recompute_proof(bytes: u32) -> Weight {
        Weight::from_parts(45_000_000, 8_000)
            .saturating_add(Weight::from_parts(50u64.saturating_mul(bytes as u64), 0))
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(2))
    }
    fn register_watchtower() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn ack_observed() -> Weight {
        Weight::from_parts(35_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn crank_round_close(batch: u32) -> Weight {
        Weight::from_parts(30_000_000, 8_000)
            .saturating_add(Weight::from_parts(
                10_000_000u64.saturating_mul(batch as u64),
                0,
            ))
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(u64::from(batch.saturating_add(1))))
    }
    fn crank_reserve_probe() -> Weight {
        Weight::from_parts(20_000_000, 1_600)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn reserve_probe_result() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(3))
            .saturating_add(T::DbWeight::get().writes(4))
    }
    fn adjudicate() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(2))
    }
}

// For tests and backwards compatibility.
impl WeightInfo for () {
    fn register_reporter() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn deregister_reporter() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn report() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn challenge() -> Weight {
        Weight::from_parts(35_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn recompute_proof(bytes: u32) -> Weight {
        Weight::from_parts(45_000_000, 8_000)
            .saturating_add(Weight::from_parts(50u64.saturating_mul(bytes as u64), 0))
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(2))
    }
    fn register_watchtower() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn ack_observed() -> Weight {
        Weight::from_parts(35_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn crank_round_close(batch: u32) -> Weight {
        Weight::from_parts(30_000_000, 8_000)
            .saturating_add(Weight::from_parts(
                10_000_000u64.saturating_mul(batch as u64),
                0,
            ))
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(u64::from(batch.saturating_add(1))))
    }
    fn crank_reserve_probe() -> Weight {
        Weight::from_parts(20_000_000, 1_600)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn reserve_probe_result() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(3))
            .saturating_add(RocksDbWeight::get().writes(4))
    }
    fn adjudicate() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(2))
    }
}
