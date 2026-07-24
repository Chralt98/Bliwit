//! B5 (15 §4.5): machine-generated `frame-benchmarking` weights for every
//! benchmarked runtime pallet, produced by `frame-omni-bencher` against the
//! `runtime-benchmarks` Wasm (see each file's header for the exact command,
//! steps/repeat, and host). Regenerating: build the release Wasm with
//! `--features runtime-benchmarks`, then run the per-pallet command from any
//! file header. The weight-regression CI gate
//! (`tools/ci/check-weight-regression.py`) compares these files against the
//! merge-base and fails on >10 % growth (15 §4.5).
//!
//! `pallet_xcm` is calibrated here; its two protocol-disabled calls retain a
//! fail-closed `Weight::MAX` because no valid benchmark fixture may bypass the
//! production filters. Not present here (deliberate):
//! `pallet_transaction_payment` /
//! `pallet_asset_tx_payment` (no benchmarkable calls in this SDK train), and
//! the hook-only consensus pallets (`pallet_aura`, `pallet_authorship`,
//! `cumulus_pallet_aura_ext`, `staging_parachain_info`) which expose no
//! benchmark harness upstream.

pub mod cumulus_pallet_parachain_system;
pub mod cumulus_pallet_xcmp_queue;
pub mod frame_system;
pub mod pallet_assets;
pub mod pallet_attestor;
pub mod pallet_balances;
pub mod pallet_collator_selection;
pub mod pallet_conditional_ledger;
pub mod pallet_constitution;
pub mod pallet_conviction_voting;
pub mod pallet_epoch;
pub mod pallet_execution_guard;
pub mod pallet_futarchy_treasury;
pub mod pallet_guardian;
pub mod pallet_market;
pub mod pallet_message_queue;
pub mod pallet_migrations;
pub mod pallet_multisig;
pub mod pallet_oracle;
pub mod pallet_origins;
pub mod pallet_preimage;
pub mod pallet_proxy;
pub mod pallet_referenda;
pub mod pallet_registry;
pub mod pallet_scheduler;
pub mod pallet_session;
pub mod pallet_sudo;
pub mod pallet_timestamp;
pub mod pallet_utility;
pub mod pallet_welfare;
pub mod pallet_xcm;
