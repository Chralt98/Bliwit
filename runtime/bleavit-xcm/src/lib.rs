//! # `bleavit-xcm` — the B4 XCM layer (09 §6; 07 §8; 09 §4; 01 §4)
//!
//! Runtime-independent XCM configuration + plumbing for the Bleavit parachain.
//! B1a's `construct_runtime!` wires these components; nothing here depends on
//! the assembled runtime (the same seam pattern as the Track-A pallets).

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

extern crate alloc;

pub mod assets;
pub mod barrier;
pub mod caps;
pub mod coretime;
pub mod filter;
pub mod health;
pub mod identity;
pub mod probe;
pub mod trader;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
