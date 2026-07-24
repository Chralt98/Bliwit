//! Weights for `pallet-futarchy-treasury`.
//!
//! The `WeightInfo` trait is the runtime-facing surface required by the Track-A
//! definition of done; the values below are hand-seeded placeholders in the
//! generated-file shape. B5 (15 §4.5) replaces them with PoV-calibrated output
//! from the `frame-benchmarking` CI run against `benchmarking.rs`. Every call
//! reads and writes the single bounded `State` value (r:1 w:1); B5 may split
//! that aggregate if the PoV of the worst-case (196-POL / 128-stream) encoding
//! demands it.

use core::marker::PhantomData;
use frame_support::traits::Get;
use frame_support::weights::{constants::RocksDbWeight, Weight};

/// Weight functions needed for `pallet-futarchy-treasury`.
pub trait WeightInfo {
    /// Weight of `fund_budget_line` (`State` r:1 w:1).
    fn fund_budget_line() -> Weight;
    /// Weight of `spend` (`State` r:1 w:1; NAV over the full obligation set).
    fn spend() -> Weight;
    /// Weight of `open_stream` (`State` r:1 w:1; NAV + stream push).
    fn open_stream() -> Weight;
    /// Weight of `claim_stream` (`State` r:1 w:1; stream scan + vesting math).
    fn claim_stream() -> Weight;
    /// Weight of `cancel_stream` (`State` r:1 w:1; stream scan).
    fn cancel_stream() -> Weight;
    /// Weight of `issue_vit` (`State` r:1 w:1; issuance meter).
    fn issue_vit() -> Weight;
    /// Weight of `recover_foreign` (`State` r:1 w:1).
    fn recover_foreign() -> Weight;
    /// Weight of `execute_coretime_renewal` (`State` r:1 w:1; quote lookup).
    fn execute_coretime_renewal() -> Weight;
    /// Weight of `note_coretime_quote` (`State` r:1 w:1; bounded quote scan).
    fn note_coretime_quote() -> Weight;
    /// Weight of `prune_coretime_quote` (`State` r:1 w:1; bounded quote scan).
    fn prune_coretime_quote() -> Weight;
    /// Weight of `set_coretime_authority` (two dedicated values w:2).
    fn set_coretime_authority() -> Weight;
    /// Weight of `sweep_insurance` (`State` r:1 w:1 + one USDC custody move).
    fn sweep_insurance() -> Weight;
    /// Weight of `create_community_schedule` (bounded allocation state plus
    /// the SDK vesting/currency adapter).
    fn create_community_schedule() -> Weight;
    /// Weight of the per-authored-block `note_collator_block` authorship
    /// callback (worst case: full bounded accumulator moved aside at an epoch
    /// boundary, then a fresh accumulator started). Registered as Mandatory
    /// extra weight because `pallet_authorship` reserves nothing for its
    /// `EventHandler`.
    fn note_collator_block() -> Weight;
}

/// Placeholder proof size covering the worst-case bounded `State` encoding
/// (streams 128 + POL 196 + …); B5 replaces it with the benchmarked value.
const STATE_POV: u64 = 24_000;

/// Placeholder proof size for the authorship callback: both bounded
/// 120-entry authored/pending accumulators plus the small epoch/count
/// values; B5 replaces it with the benchmarked value.
const COLLATOR_ACCUMULATOR_POV: u64 = 12_000;

/// Weights expressed through the runtime's configured `DbWeight`.
pub struct SubstrateWeight<T>(PhantomData<T>);

impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn fund_budget_line() -> Weight {
        Weight::from_parts(30_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn spend() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn open_stream() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn claim_stream() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn cancel_stream() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn issue_vit() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn recover_foreign() -> Weight {
        Weight::from_parts(30_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn execute_coretime_renewal() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(1))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn note_coretime_quote() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(2))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn prune_coretime_quote() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(3))
            .saturating_add(T::DbWeight::get().writes(1))
    }
    fn set_coretime_authority() -> Weight {
        Weight::from_parts(20_000_000, 4_000).saturating_add(T::DbWeight::get().writes(2))
    }
    fn sweep_insurance() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(3))
            .saturating_add(T::DbWeight::get().writes(3))
    }
    fn create_community_schedule() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(T::DbWeight::get().reads(4))
            .saturating_add(T::DbWeight::get().writes(3))
    }
    fn note_collator_block() -> Weight {
        Weight::from_parts(30_000_000, COLLATOR_ACCUMULATOR_POV)
            .saturating_add(T::DbWeight::get().reads(7))
            .saturating_add(T::DbWeight::get().writes(9))
    }
}

// For tests and backwards compatibility.
impl WeightInfo for () {
    fn fund_budget_line() -> Weight {
        Weight::from_parts(30_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn spend() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn open_stream() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn claim_stream() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn cancel_stream() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn issue_vit() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn recover_foreign() -> Weight {
        Weight::from_parts(30_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn execute_coretime_renewal() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(1))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn note_coretime_quote() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(2))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn prune_coretime_quote() -> Weight {
        Weight::from_parts(35_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(3))
            .saturating_add(RocksDbWeight::get().writes(1))
    }
    fn set_coretime_authority() -> Weight {
        Weight::from_parts(20_000_000, 4_000).saturating_add(RocksDbWeight::get().writes(2))
    }
    fn sweep_insurance() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(3))
            .saturating_add(RocksDbWeight::get().writes(3))
    }
    fn create_community_schedule() -> Weight {
        Weight::from_parts(45_000_000, STATE_POV)
            .saturating_add(RocksDbWeight::get().reads(4))
            .saturating_add(RocksDbWeight::get().writes(3))
    }
    fn note_collator_block() -> Weight {
        Weight::from_parts(30_000_000, COLLATOR_ACCUMULATOR_POV)
            .saturating_add(RocksDbWeight::get().reads(7))
            .saturating_add(RocksDbWeight::get().writes(9))
    }
}
