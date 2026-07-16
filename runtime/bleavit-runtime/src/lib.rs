#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![recursion_limit = "512"]

extern crate alloc;

mod apis;
mod classifier;
mod configs;
mod genesis;

#[cfg(test)]
mod tests;

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
pub type AssetId = u32;
pub type Address = MultiAddress<AccountId, ()>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

/// USDC is the local `ForeignAssets` id corresponding to GeneralIndex(1337).
pub const USDC_ASSET_ID: AssetId = 1337;
/// Compact identity commitment retained from the B1 model and pinned by 02 §8.
pub const USDC_LOCATION: [u8; 32] = [
    1, 0, 0, 0, 232, 3, 0, 0, 50, 0, 0, 0, 57, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];
pub const FEE_VIT_USDC_RATE_KEY: futarchy_primitives::ParamKey = *b"fee.vit_usdc\0\0\0\0";

pub const MILLISECS_PER_BLOCK: u64 = kernel::MILLISECS_PER_BLOCK;
pub const SS58_PREFIX: u16 = chain_identity::SS58_PREFIX;
pub const RUNTIME_SPEC_NAME: &[u8] = b"bleavit";
pub const RUNTIME_IMPL_NAME: &[u8] = b"bleavit-runtime";
pub const RUNTIME_SPEC_VERSION: u32 = 1;
pub const TRANSACTION_VERSION: u32 = futarchy_primitives::INTEGRATION_CONTRACT_VERSION;
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
    // `runtime_version` requires a literal; an identity test pins this to the
    // imported `INTEGRATION_CONTRACT_VERSION` (3).
    transaction_version: 3,
    system_version: 1,
};

#[cfg(feature = "std")]
pub fn native_version() -> sp_version::NativeVersion {
    sp_version::NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

/// Stable2603's Cumulus storage proof-size reclaim extension. Its successor
/// pallet wrapper is outside the pinned stable2603 closure available to B1a.
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
        // A8 slot: `Epoch: pallet_epoch = 61` once its production FRAME shell
        // lands (pallet indices are SCALE-frozen — never renumber the above).
        // A11 slot: `ExecutionGuard: pallet_execution_guard = 62` likewise.
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
