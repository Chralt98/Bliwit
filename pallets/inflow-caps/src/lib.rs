#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-inflow-caps` — Phase-3 real-USDC exposure meters
//!
//! This pallet owns the shared per-account cumulative USDC inflow meter from
//! `09 §5.2` and exposes the global issuance admission check used at the XCM
//! mint step. It deliberately has no dispatchables, benchmark module, or
//! weights: both functions execute inside their consuming callers' weight and
//! transaction envelopes.

pub use pallet::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

/// Live constitution-backed Phase-3 cap values (`13 §1`).
pub trait InflowCapParams {
    /// Maximum total local USDC issuance. `u128::MAX` is the unbounded sentinel.
    fn tvl_cap_usdc() -> u128;
    /// Maximum cumulative inflow per account. `u128::MAX` is unbounded.
    fn deposit_cap_usdc() -> u128;
}

#[frame_support::pallet]
pub mod pallet {
    use super::InflowCapParams;
    use frame_support::{pallet_prelude::*, traits::Get};
    use frame_system::pallet_prelude::*;
    use sp_runtime::TryRuntimeError;

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Constitution-backed global and per-account cap values.
        type CapParams: InflowCapParams;
        /// Provider for total local `ForeignAssets` USDC issuance.
        type UsdcIssuance: Get<u128>;
    }

    /// Per-account cumulative XCM USDC inflow over Phase 3 (`09 §5.2`).
    ///
    /// The map is Phase-3-scoped: it has at most one entry per depositing
    /// account and is retired when Phase 5 installs the unbounded sentinel.
    #[pallet::storage]
    pub type CumulativeDeposits<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    impl<T: Config> Pallet<T> {
        /// Return whether new conditional-ledger escrow is admissible under the
        /// live Phase-3 caps. This is a pure defense-in-depth read: splits move
        /// already-issued, already-metered USDC, so they neither reserve cap
        /// headroom nor extend [`CumulativeDeposits`]. Exact-cap state remains
        /// admissible; only state already above either cap halts new escrow.
        pub fn escrow_admissible(who: &T::AccountId) -> bool {
            let tvl_cap = T::CapParams::tvl_cap_usdc();
            let global_admissible = tvl_cap == u128::MAX || T::UsdcIssuance::get() <= tvl_cap;
            if !global_admissible {
                return false;
            }

            let deposit_cap = T::CapParams::deposit_cap_usdc();
            deposit_cap == u128::MAX || CumulativeDeposits::<T>::get(who) <= deposit_cap
        }

        /// Return `Ok` iff minting `amount` keeps total local issuance within
        /// the live global cap. This pure admission check performs no writes.
        #[allow(clippy::result_unit_err)]
        pub fn mint_admissible(amount: u128) -> Result<(), ()> {
            if amount == 0 {
                return Ok(());
            }
            let cap = T::CapParams::tvl_cap_usdc();
            if cap == u128::MAX || T::UsdcIssuance::get().saturating_add(amount) <= cap {
                Ok(())
            } else {
                Err(())
            }
        }

        /// Return `Ok` iff crediting `who` with `amount` would keep that
        /// account's cumulative Phase-3 inflow within the live per-account cap.
        ///
        /// This is the pure, write-free companion to [`Self::note_inflow`] and
        /// answers exactly the same question against exactly the same meter, so
        /// the XCM barrier can refuse an over-cap inbound program *before* any
        /// local mint (`09 §5.2`, SQ-129 resolution). The recording write stays
        /// at the deposit leg; this read reserves nothing.
        pub fn inflow_admissible(who: &T::AccountId, amount: u128) -> bool {
            let cap = T::CapParams::deposit_cap_usdc();
            if cap == u128::MAX || amount == 0 {
                return true;
            }
            CumulativeDeposits::<T>::get(who)
                .checked_add(amount)
                .is_some_and(|next| next <= cap)
        }

        /// Check and record one account's cumulative Phase-3 USDC inflow.
        /// Refusal is a strict no-op; callers compose this write with the
        /// beneficiary credit in one storage transaction (`09 §5.2`, G-1).
        /// Once Phase 5 installs the unbounded sentinel, the retired meter is
        /// neither read nor extended.
        #[allow(clippy::result_unit_err)]
        pub fn note_inflow(who: &T::AccountId, amount: u128) -> Result<(), ()> {
            let cap = T::CapParams::deposit_cap_usdc();
            if cap == u128::MAX {
                return Ok(());
            }
            // Do not create zero-value map entries: try-state treats them as
            // non-canonical and a zero inflow changes no meter state.
            if amount == 0 {
                return Ok(());
            }
            let current = CumulativeDeposits::<T>::get(who);
            let next = current.checked_add(amount).ok_or(())?;
            if next > cap {
                return Err(());
            }
            CumulativeDeposits::<T>::insert(who, next);
            Ok(())
        }

        /// Validate both live raise-only caps and every Phase-3 meter entry.
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            let tvl_cap = T::CapParams::tvl_cap_usdc();
            ensure!(
                tvl_cap == u128::MAX || T::UsdcIssuance::get() <= tvl_cap,
                TryRuntimeError::Other("inflow-caps: total USDC issuance exceeds live cap")
            );
            let cap = T::CapParams::deposit_cap_usdc();
            for (_, cumulative) in CumulativeDeposits::<T>::iter() {
                ensure!(
                    cumulative != 0,
                    TryRuntimeError::Other("inflow-caps: zero cumulative entry")
                );
                ensure!(
                    cap == u128::MAX || cumulative <= cap,
                    TryRuntimeError::Other("inflow-caps: cumulative deposit exceeds live cap")
                );
            }
            Ok(())
        }
    }
}
