#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![recursion_limit = "512"]

extern crate alloc;

/// B5 (15 §4.5): the runtime-level benchmark registry — every runtime pallet
/// with a `frame-benchmarking` harness, iterated by `list_benchmarks!` /
/// `add_benchmarks!` in `apis.rs`. Deliberately absent: `pallet_xcm`,
/// `cumulus_pallet_xcm` and the XCM executor legs (production XCM config and
/// its weights are B4 — the surface is fail-closed today), `pallet_aura` /
/// `pallet_authorship` / `cumulus_pallet_aura_ext` / `staging_parachain_info`
/// (no dispatchables and no benchmark harness upstream), and
/// `pallet_transaction_payment` / `pallet_asset_tx_payment` (no benchmarkable
/// calls; their tx-extension costs are carried by the extrinsic base weight).
#[cfg(feature = "runtime-benchmarks")]
mod benches {
    frame_benchmarking::define_benchmarks!(
        // Substrate/system set.
        [frame_system, SystemBench::<Runtime>]
        [pallet_balances, Balances]
        [pallet_timestamp, Timestamp]
        [pallet_assets, ForeignAssets]
        [pallet_utility, Utility]
        [pallet_proxy, Proxy]
        [pallet_multisig, Multisig]
        [pallet_preimage, Preimage]
        [pallet_scheduler, Scheduler]
        [pallet_referenda, Referenda]
        [pallet_conviction_voting, ConvictionVoting]
        [pallet_sudo, Sudo]
        [pallet_migrations, Migrations]
        // Parachain set.
        [pallet_session, SessionBench::<Runtime>]
        [pallet_collator_selection, CollatorSelection]
        [pallet_message_queue, MessageQueue]
        [cumulus_pallet_parachain_system, ParachainSystem]
        [cumulus_pallet_xcmp_queue, XcmpQueue]
        // Futarchy pallets (Track A).
        [pallet_origins, Origins]
        [pallet_constitution, Constitution]
        [pallet_conditional_ledger, ConditionalLedger]
        [pallet_market, Market]
        [pallet_welfare, Welfare]
        [pallet_oracle, Oracle]
        [pallet_registry, IncidentRegistry]
        [pallet_registry, MilestoneRegistry]
        [pallet_futarchy_treasury, FutarchyTreasury]
        [pallet_guardian, Guardian]
        [pallet_attestor, Attestor]
        [pallet_epoch, Epoch]
        [pallet_execution_guard, ExecutionGuard]
    );
}

mod apis;
mod classifier;
mod configs;
mod genesis;
pub mod telemetry;
pub mod track_origins;
pub mod views;
pub mod weights;

#[cfg(test)]
mod pov_budgets;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_s5;
#[cfg(test)]
mod tests_s5_behavior;
#[cfg(test)]
mod tests_telemetry;
#[cfg(test)]
mod tests_welfare_inputs;

use alloc::borrow::Cow;
// `impl_opaque_keys!` references a bare `Vec` — resolved by the std prelude in
// native builds, but the no_std Wasm build needs the alloc import in scope.
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use frame_support::construct_runtime;
use futarchy_primitives::{chain_identity, currency, kernel};
use sp_runtime::{
    generic,
    traits::{BlakeTwo256, IdentifyAccount, Verify},
    MultiAddress, MultiSignature,
};
use sp_version::RuntimeVersion;

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

pub type Signature = MultiSignature;
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;
pub type Balance = futarchy_primitives::Balance;
pub type Nonce = u32;
pub type Hash = sp_core::H256;
pub type BlockNumber = futarchy_primitives::BlockNumber;
pub type AssetId = staging_xcm::latest::Location;
pub type Address = MultiAddress<AccountId, ()>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

pub use bleavit_xcm::identity::usdc_location;
/// The 02 §8 SCALE encoding pinned by the release surface manifest.
pub const USDC_LOCATION_ENCODED: [u8; 10] =
    [0x01, 0x03, 0x00, 0xa1, 0x0f, 0x04, 0x32, 0x05, 0xe5, 0x14];
pub const FEE_VIT_USDC_RATE_KEY: futarchy_primitives::ParamKey = *b"fee.vit_usdc\0\0\0\0";

pub const MILLISECS_PER_BLOCK: u64 = kernel::MILLISECS_PER_BLOCK;
pub const SS58_PREFIX: u16 = chain_identity::SS58_PREFIX;
pub const RUNTIME_SPEC_NAME: &[u8] = b"bleavit";
pub const RUNTIME_IMPL_NAME: &[u8] = b"bleavit-runtime";
pub const RUNTIME_SPEC_VERSION: u32 = 1;
/// SDK dispatchable-compatibility counter, deliberately **independent** of
/// `INTEGRATION_CONTRACT_VERSION` (02 §13; SQ-102, contract v6). It denotes
/// compatibility of existing dispatchables as embedded in signed-transaction
/// validity, so an additive contract bump MUST NOT move it. Re-baselined to 1
/// pre-genesis; the SDK forbids this counter ever decreasing after genesis.
pub const TRANSACTION_VERSION: u32 = 1;
pub const VIT_DECIMALS: u8 = currency::VIT_DECIMALS;
pub const USDC_DECIMALS: u8 = currency::USDC_DECIMALS;

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: Cow::Borrowed("bleavit"),
    impl_name: Cow::Borrowed("bleavit-runtime"),
    authoring_version: 1,
    spec_version: 1,
    impl_version: 0,
    apis: apis::RUNTIME_API_VERSIONS,
    // `runtime_version` requires a literal; an identity test pins this to
    // `TRANSACTION_VERSION` and asserts it is independent of the contract
    // version (02 §13; SQ-102).
    transaction_version: 1,
    system_version: 1,
};

#[cfg(feature = "std")]
pub fn native_version() -> sp_version::NativeVersion {
    sp_version::NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

/// The deprecated Cumulus storage proof-size reclaim extension. Its successor
/// (`frame_system::WeightReclaim`, in-closure since the D-19 stable2606 move)
/// changes the TxExtension stack — a transaction-format/metadata change that is
/// deliberately NOT part of a maintenance train bump (SQ-228).
#[allow(deprecated)]
pub type StorageWeightReclaim =
    cumulus_primitives_storage_weight_reclaim::StorageWeightReclaim<Runtime>;

pub type TxExtension = (
    frame_system::AuthorizeCall<Runtime>,
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_asset_tx_payment::ChargeAssetTxPayment<Runtime>,
    (
        frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
        StorageWeightReclaim,
    ),
);

pub type UncheckedExtrinsic =
    generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, TxExtension>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

pub mod opaque {
    use super::{BlockNumber, Header};
    use sp_runtime::{generic, traits::BlakeTwo256, OpaqueExtrinsic};

    pub type UncheckedExtrinsic = OpaqueExtrinsic;
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
    pub type BlockId = generic::BlockId<Block>;
    pub type Hash = <BlakeTwo256 as sp_runtime::traits::Hash>::Output;
    pub type Number = BlockNumber;
}

sp_runtime::impl_opaque_keys! {
    pub struct SessionKeys {
        pub aura: Aura,
    }
}

construct_runtime!(
    pub enum Runtime {
        System: frame_system = 0,
        Timestamp: pallet_timestamp = 1,
        ParachainSystem: cumulus_pallet_parachain_system = 2,
        ParachainInfo: staging_parachain_info = 3,

        Balances: pallet_balances = 10,
        ForeignAssets: pallet_assets::<Instance1> = 11,
        TransactionPayment: pallet_transaction_payment = 12,
        AssetTxPayment: pallet_asset_tx_payment = 13,
        Vesting: pallet_vesting = 14,

        Referenda: pallet_referenda = 20,
        ConvictionVoting: pallet_conviction_voting = 21,
        Preimage: pallet_preimage = 22,
        Scheduler: pallet_scheduler = 23,
        Utility: pallet_utility = 24,
        Proxy: pallet_proxy = 25,
        Multisig: pallet_multisig = 26,
        Migrations: pallet_migrations = 27,
        Sudo: pallet_sudo = 28,

        XcmpQueue: cumulus_pallet_xcmp_queue = 30,
        MessageQueue: pallet_message_queue = 31,
        CumulusXcm: cumulus_pallet_xcm = 32,
        PolkadotXcm: pallet_xcm = 33,

        Authorship: pallet_authorship = 40,
        CollatorSelection: pallet_collator_selection = 41,
        Session: pallet_session = 42,
        Aura: pallet_aura = 43,
        AuraExt: cumulus_pallet_aura_ext = 44,

        Origins: pallet_origins = 50,
        Constitution: pallet_constitution = 51,
        ConditionalLedger: pallet_conditional_ledger = 52,
        Market: pallet_market = 53,
        Welfare: pallet_welfare = 54,
        Oracle: pallet_oracle = 55,
        IncidentRegistry: pallet_registry = 56,
        MilestoneRegistry: pallet_registry::<Instance1> = 57,
        FutarchyTreasury: pallet_futarchy_treasury = 58,
        Guardian: pallet_guardian = 59,
        Attestor: pallet_attestor = 60,
        // Frozen custom-pallet slots (02 §7; never renumber).
        Epoch: pallet_epoch = 61,
        ExecutionGuard: pallet_execution_guard = 62,
        InflowCaps: pallet_inflow_caps = 63,
        // Runtime-internal origin-only shim for the five scoped values tracks.
        TrackOrigins: track_origins = 64,
    }
);

pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
>;

pub(crate) const UNINCLUDED_SEGMENT_CAPACITY: u32 = 3;
pub(crate) const BLOCK_PROCESSING_VELOCITY: u32 = 1;
pub(crate) const RELAY_CHAIN_SLOT_DURATION_MILLIS: u32 = 6_000;

pub(crate) type ConsensusHook = cumulus_pallet_aura_ext::FixedVelocityConsensusHook<
    Runtime,
    RELAY_CHAIN_SLOT_DURATION_MILLIS,
    BLOCK_PROCESSING_VELOCITY,
    UNINCLUDED_SEGMENT_CAPACITY,
>;

cumulus_pallet_parachain_system::register_validate_block! {
    Runtime = Runtime,
    BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
}
