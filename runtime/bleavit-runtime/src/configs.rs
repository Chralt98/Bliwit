//! Runtime configuration and the B1a fail-closed cross-pallet adapters.

use alloc::{borrow::Cow, boxed::Box, vec::Vec};

use frame_support::{
    derive_impl,
    dispatch::{DispatchClass, DispatchResult},
    parameter_types,
    traits::{
        fungibles::{Inspect, Mutate},
        tokens::Preservation,
        ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, Contains, EqualPrivilegeOnly, Get,
        InstanceFilter, Nothing, OriginTrait, QueryPreimage, StorageInstance, StorePreimage,
        TransformOrigin, UnfilteredDispatchable, VariantCountOf, WithdrawReasons,
    },
    weights::{
        constants::{
            BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND,
        },
        IdentityFee, Weight,
    },
    PalletId,
};
use frame_system::{
    limits::{BlockLength, BlockWeights},
    EnsureRoot, EnsureSigned,
};
use futarchy_primitives::{bounds, chain_identity, currency, kernel, EpochId, FixedU64, ParamKey};
use parity_scale_codec::{Decode, Encode};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
#[cfg(feature = "runtime-benchmarks")]
use sp_runtime::AccountId32;
use sp_runtime::{
    traits::{AccountIdConversion, AccountIdLookup, Dispatchable},
    DispatchError, Perbill,
};

use crate::{
    AccountId, AssetId, Aura, Balance, Balances, Block, BlockNumber, CollatorSelection,
    ConditionalLedger, ConsensusHook, Epoch, ForeignAssets, FutarchyTreasury, Hash, MessageQueue,
    Migrations, Nonce, PalletInfo, ParachainSystem, PolkadotXcm, Preimage, Referenda, Runtime,
    RuntimeCall, RuntimeEvent, RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin, RuntimeTask,
    Scheduler, Session, SessionKeys, System, XcmpQueue, USDC_ASSET_ID, VERSION,
};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(5);
const MAXIMUM_BLOCK_WEIGHT: Weight = Weight::from_parts(
    WEIGHT_REF_TIME_PER_SECOND.saturating_mul(2),
    cumulus_primitives_core::relay_chain::MAX_POV_SIZE as u64,
);

parameter_types! {
    pub const Version: sp_version::RuntimeVersion = VERSION;
    pub RuntimeBlockLength: BlockLength = BlockLength::builder()
        .max_length(5 * 1024 * 1024)
        .modify_max_length_for_class(DispatchClass::Normal, |m| *m = NORMAL_DISPATCH_RATIO * *m)
        .build();
    pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
        .base_block(BlockExecutionWeight::get())
        .for_class(DispatchClass::all(), |w| w.base_extrinsic = ExtrinsicBaseWeight::get())
        .for_class(DispatchClass::Normal, |w| w.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT))
        .for_class(DispatchClass::Operational, |w| {
            w.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
            w.reserved = Some(MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
        })
        .avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
        .build_or_panic();
    pub const Ss58Prefix: u16 = chain_identity::SS58_PREFIX;
}

type SingleBlockMigrations = ();

#[derive_impl(frame_system::config_preludes::ParaChainDefaultConfig)]
impl frame_system::Config for Runtime {
    type BaseCallFilter = crate::classifier::RuntimeBaseCallFilter;
    type AccountId = AccountId;
    type Lookup = AccountIdLookup<AccountId, ()>;
    type Nonce = Nonce;
    type Hash = Hash;
    type Block = Block;
    type Version = Version;
    type AccountData = pallet_balances::AccountData<Balance>;
    type DbWeight = RocksDbWeight;
    type BlockWeights = RuntimeBlockWeights;
    type BlockLength = RuntimeBlockLength;
    type SS58Prefix = Ss58Prefix;
    type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
    type MaxConsumers = ConstU32<16>;
    type SingleBlockMigrations = SingleBlockMigrations;
    type MultiBlockMigrator = Migrations;
}

parameter_types! {
    pub const MinimumPeriod: u64 = kernel::MILLISECS_PER_BLOCK / 2;
    pub const ExistentialDeposit: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
}

impl pallet_timestamp::Config for Runtime {
    type Moment = u64;
    type OnTimestampSet = Aura;
    type MinimumPeriod = MinimumPeriod;
    type WeightInfo = ();
}

impl pallet_balances::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Balance = Balance;
    type DustRemoval = ();
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ConstU32<50>;
    type ReserveIdentifier = [u8; 8];
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = VariantCountOf<RuntimeFreezeReason>;
    type DoneSlashHandler = ();
}

parameter_types! {
    pub const MinVestedTransfer: Balance = currency::VIT;
    pub UnvestedFundsAllowedWithdrawReasons: WithdrawReasons =
        WithdrawReasons::TRANSACTION_PAYMENT;
}

impl pallet_vesting::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type BlockNumberToBalance = sp_runtime::traits::ConvertInto;
    type MinVestedTransfer = MinVestedTransfer;
    type WeightInfo = pallet_vesting::weights::SubstrateWeight<Runtime>;
    // The pallet applies the complement when installing its legacy balance lock.
    // The fungible fee adapter ignores these lock reasons, so in practice unvested
    // VIT cannot pay fees despite TRANSACTION_PAYMENT being the allowed reason.
    type UnvestedFundsAllowedWithdrawReasons = UnvestedFundsAllowedWithdrawReasons;
    // Schedules use para-blocks at the nominal 6 s cadence. Slower production can
    // therefore unlock later, never earlier, which is conservative under R-7.
    type BlockNumberProvider = frame_system::Pallet<Runtime>;
    const MAX_VESTING_SCHEDULES: u32 = 8;
}

parameter_types! {
    pub const UsdcAssetId: AssetId = USDC_ASSET_ID;
    pub const AssetDeposit: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const AssetAccountDeposit: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const ApprovalDeposit: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const AssetsStringLimit: u32 = 64;
    pub const MetadataDepositBase: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const MetadataDepositPerByte: Balance = 1;
}

impl pallet_assets::Config<pallet_assets::Instance1> for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Balance = Balance;
    type AssetId = AssetId;
    type AssetIdParameter = AssetId;
    type Currency = Balances;
    type CreateOrigin = EnsureConstitutionalAssetCreate;
    type ForceOrigin = frame_system::EnsureNever<AccountId>;
    type AssetDeposit = AssetDeposit;
    type AssetAccountDeposit = AssetAccountDeposit;
    type MetadataDepositBase = MetadataDepositBase;
    type MetadataDepositPerByte = MetadataDepositPerByte;
    type ApprovalDeposit = ApprovalDeposit;
    type StringLimit = AssetsStringLimit;
    type Freezer = ();
    type Holder = ();
    type ReserveData = ();
    type Extra = ();
    type CallbackHandle = ();
    type WeightInfo = pallet_assets::weights::SubstrateWeight<Runtime>;
    type RemoveItemsLimit = ConstU32<1_000>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = AssetBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct AssetBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_assets::BenchmarkHelper<AssetId, ()> for AssetBenchmarkHelper {
    fn create_asset_id_parameter(id: u32) -> AssetId {
        id
    }
    fn create_reserve_id_parameter(_: u32) {}
}

pub struct EnsureConstitutionalAssetCreate;
impl frame_support::traits::EnsureOriginWithArg<RuntimeOrigin, AssetId>
    for EnsureConstitutionalAssetCreate
{
    type Success = AccountId;
    fn try_origin(origin: RuntimeOrigin, _: &AssetId) -> Result<AccountId, RuntimeOrigin> {
        match <pallet_origins::EnsureConstitutionalValues as frame_support::traits::EnsureOrigin<
            RuntimeOrigin,
        >>::try_origin(origin.clone())
        {
            Ok(()) => Ok(LedgerPalletId::get().into_account_truncating()),
            Err(_) => Err(origin),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin(_: &AssetId) -> Result<RuntimeOrigin, ()> {
        Ok(pallet_origins::Origin::ConstitutionalValues.into())
    }
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = pallet_transaction_payment::FungibleAdapter<Balances, ()>;
    type WeightToFee = IdentityFee<Balance>;
    type LengthToFee = IdentityFee<Balance>;
    type FeeMultiplierUpdate = ();
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightInfo = ();
}

/// Live VIT/USDC conversion. Missing or malformed `fee.vit_usdc` rejects the
/// asset-fee path; native VIT fee payment remains available.
pub struct LiveFeeConversion;

impl frame_support::traits::tokens::ConversionToAssetBalance<Balance, AssetId, Balance>
    for LiveFeeConversion
{
    type Error = ();
    fn to_asset_balance(vit: Balance, asset_id: AssetId) -> Result<Balance, ()> {
        if asset_id != USDC_ASSET_ID {
            return Err(());
        }
        let rate = pallet_constitution::Params::<Runtime>::get(crate::FEE_VIT_USDC_RATE_KEY)
            .and_then(|record| match record.value {
                pallet_constitution::ParamValue::Fixed(value) if value.0 > 0 => Some(value.0),
                _ => None,
            })
            .ok_or(())?;
        let numerator = sp_core::U256::from(vit)
            .checked_mul(sp_core::U256::from(rate))
            .and_then(|value| value.checked_mul(sp_core::U256::from(currency::USDC)))
            .ok_or(())?;
        let denominator = sp_core::U256::from(1_000_000_000u64)
            .checked_mul(sp_core::U256::from(currency::VIT))
            .ok_or(())?;
        let rounded = numerator
            .checked_add(denominator.checked_sub(sp_core::U256::one()).ok_or(())?)
            .and_then(|value| value.checked_div(denominator))
            .ok_or(())?;
        let charged = if vit > 0 && rounded.is_zero() {
            sp_core::U256::one()
        } else {
            rounded
        };
        Balance::try_from(charged).map_err(|_| ())
    }
}

impl pallet_asset_tx_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Fungibles = ForeignAssets;
    type OnChargeAssetTransaction =
        pallet_asset_tx_payment::FungiblesAdapter<LiveFeeConversion, ()>;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = AssetTxBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct AssetTxBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_asset_tx_payment::BenchmarkHelperTrait<AccountId, AssetId, AssetId>
    for AssetTxBenchmarkHelper
{
    fn create_asset_id_parameter(id: u32) -> (AssetId, AssetId) {
        (id, id)
    }
    fn setup_balances_and_pool(_: AssetId, _: AccountId) {}
}

parameter_types! {
    pub const PreimageBaseDeposit: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const PreimageByteDeposit: Balance = 1;
    pub const PreimageHoldReason: RuntimeHoldReason = RuntimeHoldReason::Preimage(pallet_preimage::HoldReason::Preimage);
}

impl pallet_preimage::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type ManagerOrigin = pallet_origins::EnsureConstitutionalValues;
    type Consideration = frame_support::traits::fungible::HoldConsideration<
        AccountId,
        Balances,
        PreimageHoldReason,
        frame_support::traits::LinearStoragePrice<
            PreimageBaseDeposit,
            PreimageByteDeposit,
            Balance,
        >,
    >;
}

/// 06 §3.4 admits scheduling only through referenda's internal Scheduler API;
/// no user or privileged origin may submit arbitrary scheduler calls.
pub struct InternalSchedulerOnly;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for InternalSchedulerOnly {
    type Success = ();
    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        Err(origin)
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Err(())
    }
}

parameter_types! {
    pub MaximumSchedulerWeight: Weight = Perbill::from_percent(80) * RuntimeBlockWeights::get().max_block;
}
impl pallet_scheduler::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeOrigin = RuntimeOrigin;
    type PalletsOrigin = <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin;
    type RuntimeCall = RuntimeCall;
    type MaximumWeight = MaximumSchedulerWeight;
    type ScheduleOrigin = InternalSchedulerOnly;
    type MaxScheduledPerBlock = ConstU32<50>;
    type WeightInfo = ();
    type OriginPrivilegeCmp = EqualPrivilegeOnly;
    type Preimages = Preimage;
    type BlockNumberProvider = System;
}

impl pallet_utility::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type PalletsOrigin = <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin;
    type WeightInfo = pallet_utility::weights::SubstrateWeight<Runtime>;
}

#[derive(
    parity_scale_codec::Encode,
    parity_scale_codec::Decode,
    parity_scale_codec::MaxEncodedLen,
    scale_info::TypeInfo,
    Clone,
    Copy,
    Debug,
    Eq,
    Ord,
    PartialOrd,
    PartialEq,
    parity_scale_codec::DecodeWithMemTracking,
)]
pub enum ProxyType {
    Any,
}
impl Default for ProxyType {
    fn default() -> Self {
        Self::Any
    }
}
impl InstanceFilter<RuntimeCall> for ProxyType {
    fn filter(&self, _: &RuntimeCall) -> bool {
        true
    }
    fn is_superset(&self, _: &Self) -> bool {
        true
    }
}
parameter_types! {
    pub const ProxyDepositBase: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const ProxyDepositFactor: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const AnnouncementDepositBase: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
    pub const AnnouncementDepositFactor: Balance = currency::VIT_EXISTENTIAL_DEPOSIT;
}
impl pallet_proxy::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    type ProxyType = ProxyType;
    type ProxyDepositBase = ProxyDepositBase;
    type ProxyDepositFactor = ProxyDepositFactor;
    type MaxProxies = ConstU32<32>;
    type WeightInfo = ();
    type MaxPending = ConstU32<32>;
    type CallHasher = sp_runtime::traits::BlakeTwo256;
    type AnnouncementDepositBase = AnnouncementDepositBase;
    type AnnouncementDepositFactor = AnnouncementDepositFactor;
    type BlockNumberProvider = System;
}
impl pallet_multisig::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    type DepositBase = ConstU128<{ currency::VIT_EXISTENTIAL_DEPOSIT }>;
    type DepositFactor = ConstU128<{ currency::VIT_EXISTENTIAL_DEPOSIT }>;
    type MaxSignatories = ConstU32<100>;
    type WeightInfo = ();
    type BlockNumberProvider = System;
}

parameter_types! {
    pub MigrationMaxServiceWeight: Weight = Perbill::from_percent(bounds::MIGRATION_SERVICE_WEIGHT_PERCENT) * RuntimeBlockWeights::get().max_block;
}

// Runtime-internal PB-MIGRATION observability. These aliases deliberately do
// not join the 02-frozen pallet storage surface; every value is fixed-size and
// bounded. The cursor itself remains single-sourced in `pallet-migrations`.
pub struct MigrationHaltSourcesStorage;
impl StorageInstance for MigrationHaltSourcesStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "HaltSources";
}
pub type MigrationHaltSources = frame_support::storage::types::StorageValue<
    MigrationHaltSourcesStorage,
    u8,
    frame_support::pallet_prelude::ValueQuery,
>;

pub struct MigrationFailedStepStorage;
impl StorageInstance for MigrationFailedStepStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "FailedStep";
}
pub type MigrationFailedStep = frame_support::storage::types::StorageValue<
    MigrationFailedStepStorage,
    u32,
    frame_support::pallet_prelude::OptionQuery,
>;

pub struct MigrationProgressMarkerStorage;
impl StorageInstance for MigrationProgressMarkerStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "ProgressMarker";
}
pub type MigrationProgressMarker = frame_support::storage::types::StorageValue<
    MigrationProgressMarkerStorage,
    ([u8; 32], BlockNumber),
    frame_support::pallet_prelude::OptionQuery,
>;

const MIGRATION_FAILURE_HALT: u8 = 0b001;
const MIGRATION_STALL_HALT: u8 = 0b010;
const APPLIED_DETECTION_HALT: u8 = 0b100;
const UPGRADE_ABORT_TRIGGER: u8 = 0b1000;
const EXECUTION_HALT_SOURCES: u8 =
    MIGRATION_FAILURE_HALT | MIGRATION_STALL_HALT | APPLIED_DETECTION_HALT;

fn sync_execution_migration_halt(sources: u8) {
    pallet_execution_guard::MigrationHalt::<Runtime>::put(sources & EXECUTION_HALT_SOURCES != 0);
}

fn set_migration_halt_source(source: u8) {
    let sources = MigrationHaltSources::mutate(|sources| {
        *sources |= source;
        *sources
    });
    sync_execution_migration_halt(sources);
}

fn clear_migration_halt_sources(mask: u8) {
    let remaining = MigrationHaltSources::mutate(|sources| {
        *sources &= !mask;
        *sources
    });
    sync_execution_migration_halt(remaining);
}

fn active_migration_marker(cursor: &pallet_migrations::ActiveCursorOf<Runtime>) -> [u8; 32] {
    // `started_at` is lifecycle metadata, not cursor progress. Track the MBM
    // index and the migration-owned cursor bytes that `step` actually returns.
    sp_io::hashing::blake2_256(&(cursor.index, &cursor.inner_cursor).encode())
}

fn active_migration_stall_is_live(cursor: &pallet_migrations::ActiveCursorOf<Runtime>) -> bool {
    MigrationProgressMarker::get().is_some_and(|(marker, since)| {
        marker == active_migration_marker(cursor)
            && System::block_number().saturating_sub(since) > kernel::MIGRATION_STALL_BLOCKS
    })
}

fn track_migration_progress() {
    let now = System::block_number();
    match pallet_migrations::Cursor::<Runtime>::get() {
        Some(pallet_migrations::MigrationCursor::Active(cursor)) => {
            let marker = active_migration_marker(&cursor);
            match MigrationProgressMarker::get() {
                Some((previous, since)) if previous == marker => {
                    // A lawful `SteppedMigration::step` may mutate storage yet
                    // return identical cursor bytes. Such work lasting >900
                    // blocks can conservatively false-trigger this bounded,
                    // deterministic tracker. The halt self-clears when
                    // `completed()` fires; the normative semantic definition
                    // of "stalled" remains an open specification question.
                    if now.saturating_sub(since) > kernel::MIGRATION_STALL_BLOCKS {
                        set_migration_halt_source(MIGRATION_STALL_HALT);
                    }
                }
                _ => MigrationProgressMarker::put((marker, now)),
            }
        }
        Some(pallet_migrations::MigrationCursor::Stuck) => {
            // The failure callback normally records this first. Seeing an
            // externally restored stuck cursor is still a machine trigger.
            set_migration_halt_source(MIGRATION_FAILURE_HALT);
            MigrationProgressMarker::kill();
        }
        None => MigrationProgressMarker::kill(),
    }
}

fn migration_validation_hook_weight() -> Weight {
    // `remark_with_event` is the stable2603 benchmarked linear hash-of-bytes
    // path. Charge it at CursorMaxLen plus the hook's bounded worst-case
    // storage/proof work; this remains conservative until B5 benchmarking.
    <<Runtime as frame_system::Config>::SystemWeightInfo as frame_system::WeightInfo>::remark_with_event(
        bounds::MIGRATION_CURSOR_MAX_LEN,
    )
    .saturating_add(
        <Runtime as frame_system::Config>::DbWeight::get().reads_writes(8, 5),
    )
    .saturating_add(Weight::from_parts(
        0,
        u64::from(bounds::MIGRATION_CURSOR_MAX_LEN),
    ))
}

/// PB-MIGRATION signal bridge. A failed step stays stuck (the SDK's
/// fail-closed transaction pause) and makes the guard's machine trigger live.
pub struct MigrationFailureToGuard;
impl frame_support::migrations::FailedMigrationHandler for MigrationFailureToGuard {
    fn failed(failed_step: Option<u32>) -> frame_support::migrations::FailedMigrationHandling {
        match failed_step {
            Some(index) => MigrationFailedStep::put(index),
            None => MigrationFailedStep::kill(),
        }
        set_migration_halt_source(MIGRATION_FAILURE_HALT);
        frame_support::migrations::FailedMigrationHandling::KeepStuck
    }
}

/// A genuinely completed retry is the only SDK status transition that clears
/// the PB-MIGRATION trigger. Starting a migration never clears an earlier halt.
pub struct MigrationStatusToGuard;
impl frame_support::migrations::MigrationStatusHandler for MigrationStatusToGuard {
    fn started() {
        MigrationFailedStep::kill();
        track_migration_progress();
    }

    fn completed() {
        MigrationFailedStep::kill();
        MigrationProgressMarker::kill();
        // MBM completion clears only migration failure/stall sources. An
        // applied-code mismatch remains halted until a later valid applied
        // callback resolves that condition. The additional try-state-before-
        // lift coupling is intentionally still an open specification question.
        clear_migration_halt_sources(MIGRATION_FAILURE_HALT | MIGRATION_STALL_HALT);
    }
}

impl pallet_migrations::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type Migrations = ();
    #[cfg(feature = "runtime-benchmarks")]
    type Migrations = pallet_migrations::mock_helpers::MockedMigrations;
    type CursorMaxLen = ConstU32<{ bounds::MIGRATION_CURSOR_MAX_LEN }>;
    type IdentifierMaxLen = ConstU32<{ bounds::MIGRATION_IDENTIFIER_MAX_LEN }>;
    type MigrationStatusHandler = MigrationStatusToGuard;
    type FailedMigrationHandler = MigrationFailureToGuard;
    type MaxServiceWeight = MigrationMaxServiceWeight;
    type WeightInfo = pallet_migrations::weights::SubstrateWeight<Runtime>;
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = ();
}

parameter_types! {
    pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const RelayOrigin: cumulus_primitives_core::AggregateMessageOrigin = cumulus_primitives_core::AggregateMessageOrigin::Parent;
}
impl cumulus_pallet_parachain_system::Config for Runtime {
    type WeightInfo = ();
    type RuntimeEvent = RuntimeEvent;
    type OnSystemEvent = ExecutionGuardSystemEvent;
    type SelfParaId = staging_parachain_info::Pallet<Runtime>;
    type OutboundXcmpMessageSource = XcmpQueue;
    type DmpQueue = frame_support::traits::EnqueueWithOrigin<MessageQueue, RelayOrigin>;
    type ReservedDmpWeight = ReservedDmpWeight;
    type XcmpMessageHandler = XcmpQueue;
    type ReservedXcmpWeight = ReservedXcmpWeight;
    type CheckAssociatedRelayNumber =
        cumulus_pallet_parachain_system::RelayNumberMonotonicallyIncreases;
    type ConsensusHook = ConsensusHook;
    type RelayParentOffset = ConstU32<0>;
}
impl staging_parachain_info::Config for Runtime {}

mod xcm_config {
    use super::*;
    use staging_xcm::latest::prelude::*;
    use staging_xcm_builder::{FixedWeightBounds, FrameTransactionalProcessor};
    use staging_xcm_executor::XcmExecutor;

    parameter_types! {
        pub UniversalLocation: InteriorLocation = Parachain(staging_parachain_info::Pallet::<Runtime>::parachain_id().into()).into();
        pub UnitWeightCost: Weight = Weight::from_parts(1_000_000_000, 64 * 1024);
        pub const MaxInstructions: u32 = 100;
        pub const MaxAssetsIntoHolding: u32 = 64;
    }
    pub struct XcmConfig;
    impl staging_xcm_executor::Config for XcmConfig {
        type RuntimeCall = RuntimeCall;
        type XcmSender = ();
        type XcmEventEmitter = PolkadotXcm;
        type AssetTransactor = ();
        type OriginConverter = ();
        type IsReserve = ();
        type IsTeleporter = ();
        type UniversalLocation = UniversalLocation;
        type Barrier = ();
        type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
        type Trader = ();
        type ResponseHandler = PolkadotXcm;
        type AssetTrap = PolkadotXcm;
        type SubscriptionService = PolkadotXcm;
        type PalletInstancesInfo = crate::AllPalletsWithSystem;
        type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
        type AssetLocker = ();
        type AssetExchanger = ();
        type FeeManager = ();
        type MessageExporter = ();
        type UniversalAliases = Nothing;
        type CallDispatcher = RuntimeCall;
        type SafeCallFilter = Nothing;
        type Aliasers = Nothing;
        type TransactionalProcessor = FrameTransactionalProcessor;
        type HrmpNewChannelOpenRequestHandler = ();
        type HrmpChannelAcceptedHandler = ();
        type HrmpChannelClosingHandler = ();
        type XcmRecorder = PolkadotXcm;
    }
    pub type Executor = XcmExecutor<XcmConfig>;
}

parameter_types! {
    pub MessageQueueServiceWeight: Weight = Perbill::from_percent(35) * RuntimeBlockWeights::get().max_block;
}
impl pallet_message_queue::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type MessageProcessor = pallet_message_queue::mock_helpers::NoopMessageProcessor<
        cumulus_primitives_core::AggregateMessageOrigin,
    >;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type MessageProcessor = staging_xcm_builder::ProcessXcmMessage<
        cumulus_primitives_core::AggregateMessageOrigin,
        xcm_config::Executor,
        RuntimeCall,
    >;
    type Size = u32;
    type QueueChangeHandler = parachains_common::message_queue::NarrowOriginToSibling<XcmpQueue>;
    type QueuePausedQuery = parachains_common::message_queue::NarrowOriginToSibling<XcmpQueue>;
    type HeapSize = sp_core::ConstU32<{ 103 * 1024 }>;
    type MaxStale = sp_core::ConstU32<8>;
    type ServiceWeight = MessageQueueServiceWeight;
    type IdleMaxServiceWeight = ();
}

pub struct ControllerOriginConverter;
impl staging_xcm_executor::traits::ConvertOrigin<RuntimeOrigin> for ControllerOriginConverter {
    fn convert_origin(
        origin: impl Into<staging_xcm::latest::Location>,
        _: staging_xcm::latest::OriginKind,
    ) -> Result<RuntimeOrigin, staging_xcm::latest::Location> {
        Err(origin.into())
    }
}
impl cumulus_pallet_xcmp_queue::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ChannelInfo = ParachainSystem;
    type VersionWrapper = ();
    type XcmpQueue = TransformOrigin<
        MessageQueue,
        cumulus_primitives_core::AggregateMessageOrigin,
        cumulus_primitives_core::ParaId,
        parachains_common::message_queue::ParaIdToSibling,
    >;
    type MaxInboundSuspended = ConstU32<1_000>;
    type MaxActiveOutboundChannels = ConstU32<128>;
    type MaxPageSize = ConstU32<{ 1 << 16 }>;
    type ControllerOrigin = EnsureRoot<AccountId>;
    type ControllerOriginConverter = ControllerOriginConverter;
    type WeightInfo = ();
    type PriceForSiblingDelivery = polkadot_runtime_common::xcm_sender::NoPriceForMessageDelivery<
        cumulus_primitives_core::ParaId,
    >;
}

impl pallet_xcm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type SendXcmOrigin = staging_xcm_builder::EnsureXcmOrigin<RuntimeOrigin, ()>;
    type XcmRouter = ();
    type ExecuteXcmOrigin = staging_xcm_builder::EnsureXcmOrigin<RuntimeOrigin, ()>;
    type XcmExecuteFilter = Nothing;
    type XcmExecutor = xcm_config::Executor;
    type XcmTeleportFilter = Nothing;
    type XcmReserveTransferFilter = Nothing;
    type Weigher = staging_xcm_builder::FixedWeightBounds<
        xcm_config::UnitWeightCost,
        RuntimeCall,
        xcm_config::MaxInstructions,
    >;
    type UniversalLocation = xcm_config::UniversalLocation;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
    type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
    type Currency = Balances;
    type CurrencyMatcher = ();
    type TrustedLockers = ();
    type SovereignAccountOf = ();
    type MaxLockers = ConstU32<0>;
    type WeightInfo = pallet_xcm::TestWeightInfo;
    type AdminOrigin = EnsureRoot<AccountId>;
    type MaxRemoteLockConsumers = ConstU32<0>;
    type RemoteLockConsumerIdentifier = ();
    type AuthorizedAliasConsideration = ();
}
impl cumulus_pallet_xcm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type XcmExecutor = xcm_config::Executor;
}

impl cumulus_pallet_aura_ext::Config for Runtime {}
impl pallet_authorship::Config for Runtime {
    type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Aura>;
    type EventHandler = (CollatorSelection,);
}
parameter_types! {
    pub const Period: u32 = 6 * (60 * 60 * 1_000 / kernel::MILLISECS_PER_BLOCK as u32);
    pub const Offset: u32 = 0;
    pub const PotId: PalletId = PalletId(*b"PotStake");
}
impl pallet_session::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorId = AccountId;
    type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
    type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
    type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
    type SessionManager = CollatorSelection;
    type SessionHandler = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;
    type Keys = SessionKeys;
    type DisablingStrategy = ();
    type WeightInfo = ();
    type Currency = Balances;
    type KeyDeposit = ();
}
impl pallet_aura::Config for Runtime {
    type AuthorityId = AuraId;
    type DisabledValidators = ();
    type MaxAuthorities = ConstU32<100_000>;
    type AllowMultipleBlocksPerSlot = ConstBool<true>;
    type SlotDuration = ConstU64<{ kernel::MILLISECS_PER_BLOCK }>;
}
impl pallet_collator_selection::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type UpdateOrigin = EnsureRoot<AccountId>;
    type PotId = PotId;
    type MaxCandidates = ConstU32<100>;
    type MinEligibleCollators = ConstU32<1>;
    type MaxInvulnerables = ConstU32<20>;
    type KickThreshold = Period;
    type ValidatorId = AccountId;
    type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
    type ValidatorRegistration = Session;
    type WeightInfo = ();
}

// Custom protocol pallet configurations and their fail-closed A8/A11 seams
// follow below. Keeping these in the same module makes the assembly graph easy
// to audit against docs/architecture/01 §5.

const BLOCKS_PER_DAY: u32 = (24 * 60 * 60 * 1_000) / kernel::MILLISECS_PER_BLOCK as u32;
const BLOCKS_PER_WEEK: u32 = 7 * BLOCKS_PER_DAY;
const fn percent(x: i32) -> sp_runtime::FixedI64 {
    sp_runtime::FixedI64::from_rational(x as u128, 100)
}
// `make_linear(length, period, floor, ceil)` builds a `LinearDecreasing` curve
// that starts at `ceil` (turnout/approval share 0) and decays to `floor` (at
// share 1); it REQUIRES `ceil >= floor` (its `threshold` computes
// `ceil - x·(ceil - floor)` over raw `Perbill`, which underflows/panics if
// `floor > ceil`). 06 §2.1 support curves are written "high→low", i.e.
// ceil=high, floor=low. Passing them floor-first inverts the bound and bricks
// every track (support requirement wraps; the values layer cannot confirm).
// Approval is flat (floor == ceil), so its order is immaterial.
//
// **Values-track collapse (PR #57 Codex-bot P1).** Stock `pallet-referenda`
// selects the track from the proposal origin (`track_for`), so the five 06 §2.1
// tracks that all produce `ConstitutionalValues` (metric/constitution/
// entrenched/guardian/ratify) collapse onto ONE track. To ensure no values
// action enacts below its required bar, the shared track uses the **strongest
// (entrenched) thresholds** — 80% approval, 20%→10% support (06 §2.1 entrenched
// row). Over-strict but G-1-safe; `OracleResolution` keeps its own track. True
// per-track discrimination (distinct enactment origins + per-call track scope)
// is the values-layer milestone (SQ-103). `CV_*` therefore = entrenched values.
pub(crate) const CV_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(80), percent(80));
pub(crate) const CV_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(10), percent(20));
pub(crate) const ORACLE_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(60), percent(60));
pub(crate) const ORACLE_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(3), percent(10));
const TRACKS: [pallet_referenda::Track<u16, Balance, u32>; 2] = [
    pallet_referenda::Track {
        id: 0,
        info: pallet_referenda::TrackInfo {
            // The shared ConstitutionalValues track at entrenched strength (the
            // strongest 06 §2.1 values track): 50,000-VIT deposit, 7 d/28 d/7 d,
            // 4-epoch enactment (approximated at the 21-day default epoch).
            name: sp_runtime::str_array("constitutional_values"),
            max_deciding: 10,
            decision_deposit: 50_000 * currency::VIT,
            prepare_period: 7 * BLOCKS_PER_DAY,
            decision_period: 28 * BLOCKS_PER_DAY,
            confirm_period: 7 * BLOCKS_PER_DAY,
            min_enactment_period: 4 * 21 * BLOCKS_PER_DAY,
            min_approval: CV_APPROVAL,
            min_support: CV_SUPPORT,
        },
    },
    pallet_referenda::Track {
        id: 1,
        info: pallet_referenda::TrackInfo {
            name: sp_runtime::str_array("oracle"),
            max_deciding: 10,
            decision_deposit: 5_000 * currency::VIT,
            prepare_period: 0,
            decision_period: 7 * BLOCKS_PER_DAY,
            confirm_period: BLOCKS_PER_DAY,
            min_enactment_period: 0,
            min_approval: ORACLE_APPROVAL,
            min_support: ORACLE_SUPPORT,
        },
    },
];

pub struct BleavitTracks;
impl pallet_referenda::TracksInfo<Balance, u32> for BleavitTracks {
    type Id = u16;
    type RuntimeOrigin = <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin;
    fn tracks() -> impl Iterator<Item = Cow<'static, pallet_referenda::Track<u16, Balance, u32>>> {
        TRACKS.iter().map(Cow::Borrowed)
    }
    fn track_for(origin: &Self::RuntimeOrigin) -> Result<Self::Id, ()> {
        let candidate: Result<pallet_origins::Origin, _> = origin.clone().try_into();
        match candidate {
            Ok(pallet_origins::Origin::ConstitutionalValues) => Ok(0),
            Ok(pallet_origins::Origin::OracleResolution) => Ok(1),
            _ => Err(()),
        }
    }
}

parameter_types! {
    pub const SubmissionDeposit: Balance = currency::VIT;
    pub const MaxQueued: u32 = 100;
    pub const UndecidingTimeout: u32 = 7 * BLOCKS_PER_DAY;
    pub const AlarmInterval: u32 = 10;
    pub const MaxTurnout: Balance = currency::VIT_TOTAL_SUPPLY;
    pub const VoteLockingPeriod: u32 = 32 * BLOCKS_PER_WEEK;
}
impl pallet_referenda::Config for Runtime {
    type RuntimeCall = RuntimeCall;
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Scheduler = Scheduler;
    type Currency = Balances;
    type SubmitOrigin = frame_system::EnsureSigned<AccountId>;
    type CancelOrigin = pallet_origins::EnsureConstitutionalValues;
    type KillOrigin = pallet_origins::EnsureConstitutionalValues;
    type Slash = ();
    type Votes = Balance;
    type Tally = pallet_conviction_voting::TallyOf<Self>;
    type SubmissionDeposit = SubmissionDeposit;
    type MaxQueued = MaxQueued;
    type UndecidingTimeout = UndecidingTimeout;
    type AlarmInterval = AlarmInterval;
    type Tracks = BleavitTracks;
    type Preimages = Preimage;
    type BlockNumberProvider = System;
}
impl pallet_conviction_voting::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type Polls = Referenda;
    type MaxTurnout = MaxTurnout;
    type MaxVotes = ConstU32<512>;
    type VoteLockingPeriod = VoteLockingPeriod;
    type BlockNumberProvider = System;
    type VotingHooks = ();
}

impl pallet_origins::Config for Runtime {
    type WeightInfo = ();
}

pub struct ConstitutionGovernanceOrigin;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for ConstitutionGovernanceOrigin {
    type Success = pallet_constitution::ConstitutionOrigin;
    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let custom: Result<pallet_origins::Origin, RuntimeOrigin> = origin.clone().into();
        if let Ok(custom) = custom {
            return match custom {
                pallet_origins::Origin::FutarchyParam => Ok(Self::Success::FutarchyParam),
                pallet_origins::Origin::FutarchyTreasury => Ok(Self::Success::FutarchyTreasury),
                pallet_origins::Origin::FutarchyCode => Ok(Self::Success::FutarchyCode),
                pallet_origins::Origin::FutarchyMeta => Ok(Self::Success::FutarchyMeta),
                pallet_origins::Origin::ConstitutionalValues => {
                    Ok(Self::Success::ConstitutionalValues)
                }
                pallet_origins::Origin::OracleResolution => Err(origin),
                pallet_origins::Origin::GuardianHold => Ok(Self::Success::GuardianHold),
                pallet_origins::Origin::EmergencyPlaybook => Ok(Self::Success::EmergencyPlaybook),
            };
        }
        let raw: Result<frame_system::RawOrigin<AccountId>, RuntimeOrigin> = origin.clone().into();
        match raw {
            Ok(frame_system::RawOrigin::Root) => Ok(Self::Success::Root),
            Ok(frame_system::RawOrigin::Signed(_)) => Ok(Self::Success::Signed),
            _ => Err(origin),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(pallet_origins::Origin::FutarchyParam.into())
    }
}
impl pallet_constitution::Config for Runtime {
    type GovernanceOrigin = ConstitutionGovernanceOrigin;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type WeightInfo = pallet_constitution::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

fn default_param(key: ParamKey) -> Option<pallet_constitution::ParamValue> {
    pallet_constitution::genesis_params()
        .into_iter()
        .find(|record| record.key == key)
        .map(|record| record.value)
}
fn live_param(key: ParamKey) -> Option<pallet_constitution::ParamValue> {
    pallet_constitution::Params::<Runtime>::get(key).map(|record| record.value)
}
pub(crate) fn balance_param(name: &[u8]) -> Balance {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Balance(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Balance(value)) => value,
            _ => 0,
        },
    }
}
fn fixed_param(name: &[u8]) -> u64 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Fixed(value)) => value.0,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Fixed(value)) => value.0,
            _ => 0,
        },
    }
}
fn u32_param(name: &[u8]) -> u32 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::U32(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::U32(value)) => value,
            _ => 0,
        },
    }
}
fn perbill_param(name: &[u8]) -> u32 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Perbill(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Perbill(value)) => value,
            _ => 0,
        },
    }
}
fn percent_param(name: &[u8]) -> u8 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Percent(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Percent(value)) => value,
            _ => 0,
        },
    }
}
fn u8_param(name: &[u8]) -> u8 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::U8(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::U8(value)) => value,
            _ => 0,
        },
    }
}

pub struct RuntimeEpochParams;
impl pallet_epoch::EpochParamsProvider for RuntimeEpochParams {
    fn get() -> pallet_epoch::CoreEpochParams {
        let v_min = [
            balance_param(b"dec.v_min.param"),
            balance_param(b"dec.v_min.trs"),
            balance_param(b"dec.v_min.code"),
            balance_param(b"dec.v_min.meta"),
            0,
        ];
        pallet_epoch::CoreEpochParams {
            epoch_length: u32_param(b"epoch.length"),
            epoch_slots: u8_param(b"epoch.slots"),
            horizon_k: u8_param(b"epoch.horizon_k"),
            decision_window: u32_param(b"dec.window"),
            trailing_window: u32_param(b"dec.trailing"),
            delta: [
                FixedU64(fixed_param(b"dec.delta.param")),
                FixedU64(fixed_param(b"dec.delta.trs")),
                FixedU64(fixed_param(b"dec.delta.code")),
                FixedU64(fixed_param(b"dec.delta.meta")),
                FixedU64(pallet_market::core_market::PRICE_ONE_1E9),
            ],
            sigma: [
                FixedU64(fixed_param(b"dec.sigma.param")),
                FixedU64(fixed_param(b"dec.sigma.trs")),
                FixedU64(fixed_param(b"dec.sigma.code")),
                FixedU64(fixed_param(b"dec.sigma.meta")),
                FixedU64(0),
            ],
            delta_max: FixedU64(fixed_param(b"dec.delta_max")),
            coverage_pct: percent_param(b"dec.coverage"),
            v_min,
            gate_v_min: [
                balance_param(b"gate.v_min.param"),
                balance_param(b"gate.v_min.trs"),
                balance_param(b"gate.v_min.code"),
                balance_param(b"gate.v_min.meta"),
                0,
            ],
            gate_p_max: [FixedU64(fixed_param(b"gate.p_max")); 2],
            gate_eps: [FixedU64(fixed_param(b"gate.eps")); 2],
            gate_nb_coverage_pct: percent_param(b"gate.nb_coverage"),
            gate_nb_convergence: FixedU64(fixed_param(b"gate.nb_conv")),
            timelock: [
                u32_param(b"exec.lock.param"),
                u32_param(b"exec.lock.trs"),
                u32_param(b"exec.lock.code"),
                u32_param(b"exec.lock.meta"),
                0,
            ],
            grace: [
                u32_param(b"exec.grace"),
                u32_param(b"exec.grace"),
                u32_param(b"exec.grace"),
                u32_param(b"exec.grace"),
                0,
            ],
            intake_max_per_account: u8_param(b"intake.max_acct"),
            intake_slash_pct: percent_param(b"intake.slash_pct"),
        }
    }
}
pub struct MarketFee;
impl frame_support::traits::Get<u128> for MarketFee {
    fn get() -> u128 {
        u128::from(perbill_param(b"mkt.fee") / 100_000)
    }
}
pub struct MarketObsInterval;
impl frame_support::traits::Get<u64> for MarketObsInterval {
    fn get() -> u64 {
        u64::from(u32_param(b"mkt.obs_interval"))
    }
}
pub struct MarketKappa;
impl frame_support::traits::Get<u64> for MarketKappa {
    fn get() -> u64 {
        fixed_param(b"mkt.kappa")
    }
}
pub struct LedgerMinSplit;
impl frame_support::traits::Get<Balance> for LedgerMinSplit {
    fn get() -> Balance {
        balance_param(b"ledger.min_split")
    }
}
pub struct LedgerPositionDeposit;
impl frame_support::traits::Get<Balance> for LedgerPositionDeposit {
    fn get() -> Balance {
        balance_param(b"ledger.pos_dep")
    }
}
pub struct LedgerArchiveDelay;
impl frame_support::traits::Get<u32> for LedgerArchiveDelay {
    fn get() -> u32 {
        u32_param(b"ledger.archive")
    }
}

parameter_types! {
    pub const LedgerPalletId: PalletId = PalletId(*b"bl/ledgr");
    pub const MarketPalletId: PalletId = PalletId(*b"bl/mrket");
    pub const EpochPalletId: PalletId = PalletId(*b"bl/epoch");
    pub const ExecutionGuardPalletId: PalletId = PalletId(*b"bl/exgrd");
    pub const TreasuryPalletId: PalletId = PalletId(*b"bl/trsry");
    pub const IncidentPalletId: PalletId = PalletId(*b"bl/reg/i");
    pub const MilestonePalletId: PalletId = PalletId(*b"bl/reg/m");
}
pub fn market_account() -> AccountId {
    MarketPalletId::get().into_account_truncating()
}
pub(crate) fn epoch_account() -> AccountId {
    EpochPalletId::get().into_account_truncating()
}
pub(crate) fn execution_guard_account() -> AccountId {
    ExecutionGuardPalletId::get().into_account_truncating()
}
pub fn welfare_settlement_account() -> AccountId {
    PalletId(*b"bl/welfr").into_account_truncating()
}
pub fn insurance_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"INSURANC")
}
pub fn book_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"BOOK____")
}
pub fn pol_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"POL_____")
}
pub fn pol_baseline_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"POL_BASE")
}
pub fn fee_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"FEES____")
}
pub fn treasury_protocol_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"TREASRY_")
}
/// 08 §1.1 KEEPER USDC custody pot, derived under the canonical `bl/trsry`
/// pallet id just like the genesis treasury/community/incentive pots.
pub fn treasury_keeper_account() -> AccountId {
    TreasuryPalletId::get().into_sub_account_truncating(*b"KEEPER__")
}
/// 08 §1.1 ORACLE USDC custody pot.
pub fn treasury_oracle_account() -> AccountId {
    TreasuryPalletId::get().into_sub_account_truncating(*b"ORACLE__")
}

pub struct EnsureMarketAccount;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for EnsureMarketAccount {
    type Success = AccountId;
    fn try_origin(origin: RuntimeOrigin) -> Result<AccountId, RuntimeOrigin> {
        match EnsureSigned::<AccountId>::try_origin(origin.clone()) {
            Ok(who) if who == market_account() => Ok(who),
            _ => Err(origin),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(market_account()))
    }
}
pub struct EnsureWelfareAccount;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for EnsureWelfareAccount {
    type Success = AccountId;
    fn try_origin(origin: RuntimeOrigin) -> Result<AccountId, RuntimeOrigin> {
        match EnsureSigned::<AccountId>::try_origin(origin.clone()) {
            Ok(who) if who == welfare_settlement_account() => Ok(who),
            _ => Err(origin),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(welfare_settlement_account()))
    }
}
pub struct EnsureEpochAccount;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for EnsureEpochAccount {
    type Success = AccountId;
    fn try_origin(origin: RuntimeOrigin) -> Result<AccountId, RuntimeOrigin> {
        match EnsureSigned::<AccountId>::try_origin(origin.clone()) {
            Ok(who) if who == epoch_account() => Ok(who),
            _ => Err(origin),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(epoch_account()))
    }
}
pub struct EnsureExecutionGuardAccount;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for EnsureExecutionGuardAccount {
    type Success = AccountId;
    fn try_origin(origin: RuntimeOrigin) -> Result<AccountId, RuntimeOrigin> {
        match EnsureSigned::<AccountId>::try_origin(origin.clone()) {
            Ok(who) if who == execution_guard_account() => Ok(who),
            _ => Err(origin),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(execution_guard_account()))
    }
}
pub struct ProtocolAccounts;
impl Contains<AccountId> for ProtocolAccounts {
    fn contains(who: &AccountId) -> bool {
        let accounts = [
            LedgerPalletId::get().into_account_truncating(),
            market_account(),
            book_account(),
            pol_account(),
            pol_baseline_account(),
            fee_account(),
            treasury_protocol_account(),
            insurance_account(),
            IncidentPalletId::get().into_account_truncating(),
            MilestonePalletId::get().into_account_truncating(),
            welfare_settlement_account(),
            epoch_account(),
            execution_guard_account(),
        ];
        accounts.contains(who)
    }
}
parameter_types! { pub InsuranceAccount: AccountId = insurance_account(); }
impl pallet_conditional_ledger::Config for Runtime {
    type Collateral = ForeignAssets;
    type UsdcAssetId = UsdcAssetId;
    type MarketAuthority = EnsureMarketAccount;
    type ResolveAuthority = EnsureEpochAccount;
    type SettleAuthority = EnsureWelfareAccount;
    type MinSplit = LedgerMinSplit;
    type PositionDeposit = LedgerPositionDeposit;
    type MaxPositionsPerAccount = ConstU32<{ bounds::MAX_ACCOUNT_POSITIONS }>;
    type ArchiveDelay = LedgerArchiveDelay;
    type ReapBatch = ConstU32<{ kernel::REAP_BATCH }>;
    type ProtocolAccounts = ProtocolAccounts;
    type InsuranceAccount = InsuranceAccount;
    type PalletId = LedgerPalletId;
    type KeeperRebate = FutarchyTreasury;
    type WeightInfo = pallet_conditional_ledger::weights::SubstrateWeight<Runtime>;
}

fn market_book_account(id: futarchy_primitives::MarketId) -> AccountId {
    MarketPalletId::get().into_sub_account_truncating((*b"BOOK", id))
}

fn market_fee_account(id: futarchy_primitives::MarketId) -> AccountId {
    MarketPalletId::get().into_sub_account_truncating((*b"FEES", id))
}

fn epoch_signed_origin() -> RuntimeOrigin {
    RuntimeOrigin::signed(epoch_account())
}

fn market_window_end(id: futarchy_primitives::MarketId) -> Option<BlockNumber> {
    let book = pallet_market::Markets::<Runtime>::get(id)?;
    match book.kind {
        pallet_market::core_market::BookKind::Decision { proposal, .. }
        | pallet_market::core_market::BookKind::Gate { proposal, .. } => {
            pallet_epoch::Proposals::<Runtime>::get(proposal).map(|record| record.decide_at)
        }
        pallet_market::core_market::BookKind::Baseline { .. } => {
            // At most MaxLiveProposals=32 entries. The latest due pair selects
            // the Baseline window; equal closes deduplicate naturally.
            let now = System::block_number();
            pallet_epoch::Proposals::<Runtime>::iter_values()
                .filter(|proposal| {
                    proposal
                        .markets
                        .is_some_and(|markets| markets.baseline == id)
                        && proposal.decide_at <= now
                })
                .map(|proposal| proposal.decide_at)
                .max()
        }
    }
}

pub(crate) fn class_pol_floor(class: futarchy_primitives::ProposalClass) -> Balance {
    match class {
        futarchy_primitives::ProposalClass::Param => balance_param(b"pol.b.param"),
        futarchy_primitives::ProposalClass::Treasury => balance_param(b"pol.b.trs"),
        futarchy_primitives::ProposalClass::Code => balance_param(b"pol.b.code"),
        futarchy_primitives::ProposalClass::Meta => balance_param(b"pol.b.meta"),
        futarchy_primitives::ProposalClass::Constitutional => 0,
    }
}

pub(crate) fn proposal_class_index(class: futarchy_primitives::ProposalClass) -> usize {
    match class {
        futarchy_primitives::ProposalClass::Param => 0,
        futarchy_primitives::ProposalClass::Treasury => 1,
        futarchy_primitives::ProposalClass::Code => 2,
        futarchy_primitives::ProposalClass::Meta => 3,
        futarchy_primitives::ProposalClass::Constitutional => 4,
    }
}

/// Exact Ask-scaled contest floor enforced per decision book (05 §5.2; 08
/// §5.3; 13 `dec.v_min`). Returning `None` is fail-closed for an unavailable
/// prize proxy or checked `2P` overflow; both the grade adapter and
/// `FutarchyApi::decision_stats` call this helper.
pub(crate) fn effective_decision_contest_floor(
    proposal: &futarchy_primitives::Proposal<AccountId>,
    params: &pallet_epoch::CoreEpochParams,
) -> Option<Balance> {
    let base = params.v_min[proposal_class_index(proposal.class)];
    <RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<AccountId>>::in_cap_prize(
        proposal,
    )
    .and_then(|prize| prize.checked_mul(2))
    .map(|scaled| base.max(scaled))
}

fn contest_floor_for_grade(
    market: futarchy_primitives::MarketId,
    end: BlockNumber,
    role: pallet_epoch::BookRole,
    class: futarchy_primitives::ProposalClass,
    params: &pallet_epoch::CoreEpochParams,
) -> Option<Balance> {
    let book = pallet_market::Markets::<Runtime>::get(market)?;
    match book.kind {
        pallet_market::core_market::BookKind::Decision { proposal, .. } => {
            matches!(role, pallet_epoch::BookRole::Decision)
                .then_some(())
                .and_then(|()| pallet_epoch::Proposals::<Runtime>::get(proposal))
                .filter(|proposal| proposal.class == class && proposal.decide_at == end)
                .and_then(|proposal| effective_decision_contest_floor(&proposal, params))
        }
        pallet_market::core_market::BookKind::Gate { proposal, .. } => {
            matches!(role, pallet_epoch::BookRole::Gate)
                .then(|| params.gate_v_min[proposal_class_index(class)])
                .filter(|_| {
                    pallet_epoch::Proposals::<Runtime>::get(proposal).is_some_and(|proposal| {
                        proposal.class == class && proposal.decide_at == end
                    })
                })
        }
        pallet_market::core_market::BookKind::Baseline { .. } => {
            if !matches!(role, pallet_epoch::BookRole::Baseline) {
                return None;
            }
            pallet_epoch::Proposals::<Runtime>::iter_values()
                .filter(|proposal| {
                    proposal.class == class
                        && proposal.decide_at == end
                        && proposal
                            .markets
                            .is_some_and(|markets| markets.baseline == market)
                })
                .filter_map(|proposal| effective_decision_contest_floor(&proposal, params))
                .max()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeDecisionMarketStats {
    pub coverage_pct: u8,
    pub traded_volume: Balance,
    pub v_min_required: Balance,
}

fn decision_book_window_stats(
    market: futarchy_primitives::MarketId,
    end: BlockNumber,
    window: BlockNumber,
) -> Option<(u8, Balance)> {
    let start = end.checked_sub(window)?;
    let stats = pallet_market::DecisionWindows::<Runtime>::get(market)
        .into_iter()
        .find(|record| record.start == start && record.end == end && record.sealed)?;
    if !stats.contest_valid {
        return None;
    }
    let interval = u32::try_from(MarketObsInterval::get()).ok()?;
    let expected = window.checked_div(interval)?;
    if expected == 0 {
        return None;
    }
    // Actual scheduled-interval coverage uses the same observations/window/
    // interval sources as market-core's division-free grade predicate. The
    // display rounds down and caps surplus observations at 100%.
    let coverage = stats
        .observations
        .saturating_mul(100)
        .checked_div(expected)?
        .min(100);
    let coverage_pct = u8::try_from(coverage).ok()?;
    let traded_volume = stats
        .contest_notional_blocks
        .checked_div(Balance::from(window))?;
    Some((coverage_pct, traded_volume))
}

/// Proposal-level projection of the two per-book grade records. 05 §5.2
/// grades Accept and Reject independently, while 02 §4 exposes one coverage
/// and one volume scalar, so the projection takes the conservative minimum:
/// the displayed statistic clears a per-book threshold iff both books do.
pub(crate) fn decision_market_stats_for_view(
    proposal: &futarchy_primitives::Proposal<AccountId>,
    params: &pallet_epoch::CoreEpochParams,
) -> Option<RuntimeDecisionMarketStats> {
    let markets = proposal.markets?;
    let accept =
        decision_book_window_stats(markets.accept, proposal.decide_at, params.decision_window)?;
    let reject =
        decision_book_window_stats(markets.reject, proposal.decide_at, params.decision_window)?;
    let accept_floor = contest_floor_for_grade(
        markets.accept,
        proposal.decide_at,
        pallet_epoch::BookRole::Decision,
        proposal.class,
        params,
    )?;
    let reject_floor = contest_floor_for_grade(
        markets.reject,
        proposal.decide_at,
        pallet_epoch::BookRole::Decision,
        proposal.class,
        params,
    )?;
    if accept_floor != reject_floor {
        return None;
    }
    Some(RuntimeDecisionMarketStats {
        coverage_pct: accept.0.min(reject.0),
        traded_volume: accept.1.min(reject.1),
        v_min_required: accept_floor,
    })
}

pub struct RuntimeMarketAccess;
impl pallet_epoch::MarketAccess<AccountId> for RuntimeMarketAccess {
    fn open_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
        rerun: bool,
        requires_gate_markets: bool,
    ) -> Result<futarchy_primitives::MarketSet, DispatchError> {
        use futarchy_primitives::{Branch, GateType, MarketSet};
        use pallet_market::core_market::BookKind;

        let params = <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let now = System::block_number();
        let end = if rerun {
            now.checked_add(kernel::DEC_EXTENSION_BLOCKS)
                .ok_or(DispatchError::Arithmetic(
                    sp_runtime::ArithmeticError::Overflow,
                ))?
        } else {
            proposal.decide_at
        };
        let start = end
            .checked_sub(params.decision_window)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Underflow,
            ))?;
        let trailing_start =
            end.checked_sub(params.trailing_window)
                .ok_or(DispatchError::Arithmetic(
                    sp_runtime::ArithmeticError::Underflow,
                ))?;

        if rerun {
            let markets = proposal
                .markets
                .ok_or(DispatchError::Other("rerun market set missing"))?;
            if markets.gates.is_some() != requires_gate_markets {
                return Err(DispatchError::Other("rerun gate market invariant"));
            }
            let mut ids = Vec::from([markets.accept, markets.reject]);
            if let Some(gates) = markets.gates {
                ids.extend(gates);
            }
            for id in ids {
                pallet_market::Pallet::<Runtime>::reopen_for_rerun(epoch_signed_origin(), id)?;
                pallet_market::Pallet::<Runtime>::register_decision_window(
                    epoch_signed_origin(),
                    id,
                    start,
                    trailing_start,
                    end,
                )?;
            }
            pallet_market::Pallet::<Runtime>::seed_rerun_branch_pair(
                epoch_signed_origin(),
                markets.accept,
                markets.reject,
                pol_account(),
            )?;
            if let Some(gates) = markets.gates {
                for pair in [[gates[0], gates[1]], [gates[2], gates[3]]] {
                    pallet_market::Pallet::<Runtime>::seed_rerun_branch_pair(
                        epoch_signed_origin(),
                        pair[0],
                        pair[1],
                        pol_account(),
                    )?;
                }
            }
            pallet_market::Pallet::<Runtime>::reopen_baseline_for_rerun(
                epoch_signed_origin(),
                markets.baseline,
            )?;
            pallet_market::Pallet::<Runtime>::register_decision_window(
                epoch_signed_origin(),
                markets.baseline,
                start,
                trailing_start,
                end,
            )?;
            return Ok(markets);
        }

        if let Some(vault) = pallet_conditional_ledger::Vaults::<Runtime>::get(proposal.id) {
            if vault.spec != proposal.metric_spec {
                return Err(DispatchError::Other("proposal metric-spec vault mismatch"));
            }
        } else {
            ConditionalLedger::create_vault(
                epoch_signed_origin(),
                proposal.id,
                proposal.metric_spec,
            )?;
        }

        let create = |kind: BookKind, b: Balance| {
            let id = pallet_market::Pallet::<Runtime>::allocate_market_id(epoch_signed_origin())?;
            pallet_market::Pallet::<Runtime>::create_market(
                epoch_signed_origin(),
                id,
                kind,
                market_book_account(id),
                market_fee_account(id),
                b,
            )?;
            pallet_market::Pallet::<Runtime>::register_decision_window(
                epoch_signed_origin(),
                id,
                start,
                trailing_start,
                end,
            )?;
            Ok::<_, DispatchError>(id)
        };

        let b = class_pol_floor(proposal.class);
        // A8 fail-closed: the simulation-gated P/P_ref slope has no verified
        // on-chain P_ref backing yet. The normative floor is used; effective
        // v_min=2P still prevents under-sized adoption — owner Phase-0/SQ-145.
        let accept = create(
            BookKind::Decision {
                proposal: proposal.id,
                branch: Branch::Accept,
            },
            b,
        )?;
        let reject = create(
            BookKind::Decision {
                proposal: proposal.id,
                branch: Branch::Reject,
            },
            b,
        )?;
        pallet_market::Pallet::<Runtime>::seed_branch_pair(
            epoch_signed_origin(),
            accept,
            reject,
            pol_account(),
        )?;
        let gates = if requires_gate_markets {
            let gate_b = balance_param(b"pol.b_gate");
            let ids = [
                create(
                    BookKind::Gate {
                        proposal: proposal.id,
                        branch: Branch::Accept,
                        gate: GateType::Survival,
                    },
                    gate_b,
                )?,
                create(
                    BookKind::Gate {
                        proposal: proposal.id,
                        branch: Branch::Reject,
                        gate: GateType::Survival,
                    },
                    gate_b,
                )?,
                create(
                    BookKind::Gate {
                        proposal: proposal.id,
                        branch: Branch::Accept,
                        gate: GateType::Security,
                    },
                    gate_b,
                )?,
                create(
                    BookKind::Gate {
                        proposal: proposal.id,
                        branch: Branch::Reject,
                        gate: GateType::Security,
                    },
                    gate_b,
                )?,
            ];
            for pair in [[ids[0], ids[1]], [ids[2], ids[3]]] {
                pallet_market::Pallet::<Runtime>::seed_branch_pair(
                    epoch_signed_origin(),
                    pair[0],
                    pair[1],
                    pol_account(),
                )?;
            }
            Some(ids)
        } else {
            None
        };
        let baseline = match pallet_market::BaselineMarketOf::<Runtime>::get(proposal.epoch) {
            Some(id) => {
                pallet_market::Pallet::<Runtime>::register_decision_window(
                    epoch_signed_origin(),
                    id,
                    start,
                    trailing_start,
                    end,
                )?;
                id
            }
            None => {
                let id = create(
                    BookKind::Baseline {
                        epoch: proposal.epoch,
                    },
                    balance_param(b"pol.b_baseline"),
                )?;
                pallet_market::Pallet::<Runtime>::seed(
                    epoch_signed_origin(),
                    id,
                    pol_baseline_account(),
                )?;
                id
            }
        };
        Ok(MarketSet {
            accept,
            reject,
            gates,
            baseline,
        })
    }

    fn extend_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Result<(), DispatchError> {
        let markets = proposal
            .markets
            .ok_or(DispatchError::Other("extended market set missing"))?;
        let params = <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let start = proposal
            .decide_at
            .checked_sub(params.decision_window)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Underflow,
            ))?;
        let trailing_start = proposal
            .decide_at
            .checked_sub(params.trailing_window)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Underflow,
            ))?;
        let mut proposal_books = Vec::from([markets.accept, markets.reject]);
        if let Some(gates) = markets.gates {
            proposal_books.extend(gates);
        }
        for id in proposal_books {
            pallet_market::Pallet::<Runtime>::mark_extended(epoch_signed_origin(), id)?;
            pallet_market::Pallet::<Runtime>::register_decision_window(
                epoch_signed_origin(),
                id,
                start,
                trailing_start,
                proposal.decide_at,
            )?;
        }
        pallet_market::Pallet::<Runtime>::register_decision_window(
            epoch_signed_origin(),
            markets.baseline,
            start,
            trailing_start,
            proposal.decide_at,
        )
    }

    fn force_rerun_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Result<(), DispatchError> {
        let markets = proposal
            .markets
            .ok_or(DispatchError::Other("force-rerun market set missing"))?;
        let params = <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let start = proposal
            .decide_at
            .checked_sub(params.decision_window)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Underflow,
            ))?;
        let trailing_start = proposal
            .decide_at
            .checked_sub(params.trailing_window)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Underflow,
            ))?;
        let mut books = Vec::from([markets.accept, markets.reject]);
        if let Some(gates) = markets.gates {
            books.extend(gates);
        }
        for id in books {
            pallet_market::Pallet::<Runtime>::reopen_for_rerun(epoch_signed_origin(), id)?;
            pallet_market::Pallet::<Runtime>::register_decision_window(
                epoch_signed_origin(),
                id,
                start,
                trailing_start,
                proposal.decide_at,
            )?;
        }
        pallet_market::Pallet::<Runtime>::reopen_baseline_for_rerun(
            epoch_signed_origin(),
            markets.baseline,
        )?;
        pallet_market::Pallet::<Runtime>::register_decision_window(
            epoch_signed_origin(),
            markets.baseline,
            start,
            trailing_start,
            proposal.decide_at,
        )
    }

    fn close_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Result<(), DispatchError> {
        let markets = proposal
            .markets
            .ok_or(DispatchError::Other("decided market set missing"))?;
        let mut proposal_books = Vec::from([markets.accept, markets.reject]);
        if let Some(gates) = markets.gates {
            proposal_books.extend(gates);
        }
        for id in proposal_books {
            pallet_market::Pallet::<Runtime>::close(epoch_signed_origin(), id)?;
        }
        let baseline_still_live = pallet_epoch::Proposals::<Runtime>::iter_values().any(|other| {
            other.id != proposal.id
                && other
                    .markets
                    .is_some_and(|other_markets| other_markets.baseline == markets.baseline)
                && matches!(
                    other.state,
                    futarchy_primitives::ProposalState::Trading
                        | futarchy_primitives::ProposalState::Extended
                )
        });
        if !baseline_still_live {
            pallet_market::Pallet::<Runtime>::close(epoch_signed_origin(), markets.baseline)?;
        }
        Ok(())
    }

    fn seal_decision_window(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Result<(), DispatchError> {
        let markets = proposal
            .markets
            .ok_or(DispatchError::Other("decision market set missing"))?;
        let mut ids = Vec::from([markets.accept, markets.reject, markets.baseline]);
        if let Some(gates) = markets.gates {
            ids.extend(gates);
        }
        for id in ids {
            pallet_market::Pallet::<Runtime>::seal_decision_window(
                epoch_signed_origin(),
                id,
                proposal.decide_at,
            )?;
        }
        Ok(())
    }

    fn baseline_market(epoch: EpochId) -> Option<futarchy_primitives::MarketId> {
        pallet_market::BaselineMarketOf::<Runtime>::get(epoch)
    }

    fn twap_full(market: futarchy_primitives::MarketId) -> Option<FixedU64> {
        let end = market_window_end(market)?;
        pallet_market::Pallet::<Runtime>::twap_at(market, end, u32_param(b"dec.window"))
    }

    fn twap_full_at(market: futarchy_primitives::MarketId, end: BlockNumber) -> Option<FixedU64> {
        pallet_market::Pallet::<Runtime>::twap_at(market, end, u32_param(b"dec.window"))
    }

    fn twap_trailing_at(
        market: futarchy_primitives::MarketId,
        end: BlockNumber,
        window: BlockNumber,
    ) -> Option<FixedU64> {
        pallet_market::Pallet::<Runtime>::twap_at(market, end, window)
    }

    fn spot_at(market: futarchy_primitives::MarketId, end: BlockNumber) -> Option<FixedU64> {
        pallet_market::Pallet::<Runtime>::spot_at(market, end)
    }

    fn decision_grade(
        market: futarchy_primitives::MarketId,
        end: BlockNumber,
        role: pallet_epoch::BookRole,
        class: futarchy_primitives::ProposalClass,
        params: &pallet_epoch::CoreEpochParams,
    ) -> bool {
        let Some(book) = pallet_market::Markets::<Runtime>::get(market) else {
            return false;
        };
        let role_matches = matches!(
            (role, book.kind),
            (
                pallet_epoch::BookRole::Decision,
                pallet_market::core_market::BookKind::Decision { .. }
            ) | (
                pallet_epoch::BookRole::Baseline,
                pallet_market::core_market::BookKind::Baseline { .. }
            ) | (
                pallet_epoch::BookRole::Gate,
                pallet_market::core_market::BookKind::Gate { .. }
            )
        );
        if !role_matches {
            return false;
        }
        let Some(contest) = contest_floor_for_grade(market, end, role, class, params) else {
            return false;
        };
        let (coverage, convergence, pol_floor, sanity) = match role {
            pallet_epoch::BookRole::Decision => (
                params.coverage_pct,
                params.delta_max,
                class_pol_floor(class),
                true,
            ),
            pallet_epoch::BookRole::Baseline => (
                params.coverage_pct,
                params.delta_max,
                balance_param(b"pol.b_baseline"),
                true,
            ),
            pallet_epoch::BookRole::Gate => {
                let near_boundary =
                    pallet_market::Pallet::<Runtime>::twap_at(market, end, params.decision_window)
                        .is_some_and(|twap| {
                            twap.0 < kernel::DECISION_SANITY_MIN_1E9
                                || twap.0 > kernel::DECISION_SANITY_MAX_1E9
                        });
                (
                    if near_boundary {
                        params.gate_nb_coverage_pct
                    } else {
                        params.coverage_pct
                    },
                    if near_boundary {
                        params.gate_nb_convergence
                    } else {
                        params.delta_max
                    },
                    balance_param(b"pol.b_gate"),
                    false,
                )
            }
        };
        pallet_market::Pallet::<Runtime>::decision_grade_at(
            market,
            end,
            params.decision_window,
            coverage,
            convergence,
            contest,
            pol_floor,
            sanity,
        )
    }

    fn measured_depth(pid: futarchy_primitives::ProposalId) -> Option<Balance> {
        pallet_epoch::Proposals::<Runtime>::get(pid).and_then(|proposal| {
            let markets = proposal.markets?;
            let window = u32_param(b"dec.window");
            let mut total = 0_u128;
            for id in [markets.accept, markets.reject] {
                if !pallet_market::SeededMarkets::<Runtime>::contains_key(id) {
                    return None;
                }
                let book = pallet_market::Markets::<Runtime>::get(id)?;
                let pol = pallet_market::core_market::maker_loss_floor(book.b)?;
                let contest = pallet_market::Pallet::<Runtime>::average_contest_at(
                    id,
                    proposal.decide_at,
                    window,
                )?;
                total = total.checked_add(pol)?.checked_add(contest)?;
            }
            Some(total)
        })
    }

    fn published_flow_per_day(_: futarchy_primitives::ProposalId) -> Option<Balance> {
        // A8 fail-closed telemetry deferral: None makes the decision kernel use
        // its specified L/2 fallback (08 §5.2) — owner Phase-3 calibration.
        None
    }

    fn second_insufficiency(pid: futarchy_primitives::ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid)
            .is_some_and(|proposal| proposal.extended || proposal.rerun)
    }

    fn previous_settled_baseline_twap(epoch: EpochId) -> Option<FixedU64> {
        let previous = epoch.checked_sub(1)?;
        pallet_epoch::RecentCohortSummaries::<Runtime>::get()
            .iter()
            .find(|summary| summary.epoch == previous && !summary.voided)
            .map(|summary| summary.baseline_twap_1e9)
    }
}

pub struct RuntimeInDecisionWindow;
impl Contains<futarchy_primitives::MarketId> for RuntimeInDecisionWindow {
    fn contains(market: &futarchy_primitives::MarketId) -> bool {
        let now = System::block_number();
        pallet_epoch::Proposals::<Runtime>::iter_values().any(|proposal| {
            proposal.markets.is_some_and(|markets| {
                let belongs = markets.accept == *market
                    || markets.reject == *market
                    || markets.baseline == *market
                    || markets.gates.is_some_and(|gates| gates.contains(market));
                belongs
                    && proposal
                        .decide_at
                        .checked_sub(u32_param(b"dec.window"))
                        .is_some_and(|start| now >= start && now <= proposal.decide_at)
            })
        })
    }
}

impl pallet_market::Config for Runtime {
    type WeightInfo = pallet_market::weights::SubstrateWeight<Runtime>;
    type Fee = MarketFee;
    type ObsInterval = MarketObsInterval;
    type Kappa1e9 = MarketKappa;
    type MarketAdmin = EnsureEpochAccount;
    type ArchiveDelay = LedgerArchiveDelay;
    type PalletId = MarketPalletId;
    type KeeperRebate = FutarchyTreasury;
    type InDecisionWindow = RuntimeInDecisionWindow;
}

pub struct RuntimeEpochOracle;
impl pallet_epoch::OracleAccess for RuntimeEpochOracle {
    fn any_open_dispute_touching(spec: futarchy_primitives::MetricSpecVersion) -> bool {
        // Rounds is core-bounded to 128 and try-state-covered. Only a live
        // challenged round at or above the value-scaled round-one merit floor
        // is a decision-time dispute (07 §12). Registry sub-games never
        // enter this storage surface.
        pallet_oracle::Rounds::<Runtime>::iter().any(|(_, round)| {
            round.spec_version == spec
                && round.challenger.is_some()
                && pallet_oracle::round_bond(round.stake_at_risk, 1)
                    .map_or(true, |floor| round.bond >= floor)
        })
    }
}

pub struct RuntimeEpochGuardian;
impl pallet_epoch::GuardianAccess for RuntimeEpochGuardian {
    fn hold_active(pid: futarchy_primitives::ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid).is_some_and(|proposal| {
            matches!(
                proposal.state,
                futarchy_primitives::ProposalState::Suspended
                    | futarchy_primitives::ProposalState::Rerun
            )
        })
    }

    fn dead_man_engaged() -> bool {
        crate::Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED
            != 0
    }

    fn review_window_closed(pid: futarchy_primitives::ProposalId) -> bool {
        let current = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_epoch::GuardianReviewDeadlines::<Runtime>::get(pid)
            .is_some_and(|deadline| current >= deadline)
    }
}

pub struct RuntimeEpochAttestation;
impl pallet_epoch::AttestationAccess for RuntimeEpochAttestation {
    fn present_and_quorate(
        pid: futarchy_primitives::ProposalId,
        artifact_hash: futarchy_primitives::H256,
    ) -> bool {
        pallet_attestor::Pallet::<Runtime>::has_quorum(pid, artifact_hash)
            && pallet_attestor::Attestations::<Runtime>::get()
                .iter()
                .any(|record| {
                    record.pid == pid
                        && record.artifact_hash == artifact_hash
                        && <RuntimeAttestations as pallet_execution_guard::Attestations>::present_unrevoked_unchallenged(record.id)
                })
    }
}

fn proposal_calls(
    proposal: &futarchy_primitives::Proposal<AccountId>,
) -> Option<pallet_execution_guard::pallet::RuntimeBatch<Runtime>> {
    use pallet_execution_guard::Preimages;
    let bytes = RuntimePreimages::fetch(proposal.payload_hash)?;
    if u32::try_from(bytes.len()).ok()? != proposal.payload_len {
        return None;
    }
    let mut input = &bytes[..];
    let calls = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::decode(&mut input).ok()?;
    input.is_empty().then_some(calls)
}

/// Re-derive the committed USDC outflow (`Ask`) from the only Treasury leaves
/// whose outflow is statically knowable. Unknown calls, wrappers, recipient
/// claims and quote-priced renewal calls fail closed instead of trusting the
/// proposer's numeric declaration (05 §1.2/§5.6; 08 §5.2).
fn derived_treasury_ask(
    calls: &pallet_execution_guard::pallet::RuntimeBatch<Runtime>,
) -> Option<Balance> {
    let mut ask = 0_u128;
    for call in calls {
        let addition = match call {
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
                amount, ..
            }) => *amount,
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::open_stream {
                total,
                ..
            }) => *total,
            RuntimeCall::FutarchyTreasury(
                pallet_futarchy_treasury::Call::fund_budget_line { .. }
                | pallet_futarchy_treasury::Call::cancel_stream { .. }
                | pallet_futarchy_treasury::Call::issue_vit { .. }
                | pallet_futarchy_treasury::Call::recover_foreign { .. },
            ) => 0,
            // `claim_stream` is Signed-recipient-only and coretime renewal is
            // priced from live quote storage. Neither can be committed as a
            // statically-sized Treasury proposal outflow.
            RuntimeCall::FutarchyTreasury(
                pallet_futarchy_treasury::Call::claim_stream { .. }
                | pallet_futarchy_treasury::Call::execute_coretime_renewal { .. }
                | pallet_futarchy_treasury::Call::__Ignore(_, _),
            ) => return None,
            _ => return None,
        };
        ask = ask.checked_add(addition)?;
    }
    Some(ask)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeAdmissionMeter {
    TreasuryOutflow,
    VitIssuance,
    CodeSpacing,
}

impl RuntimeAdmissionMeter {
    const fn key(self) -> [u8; 8] {
        match self {
            Self::TreasuryOutflow => *b"trs.outf",
            Self::VitIssuance => *b"vit.issu",
            Self::CodeSpacing => *b"code.spc",
        }
    }
}

fn visit_runtime_leaves(call: &RuntimeCall, visit: &mut impl FnMut(&RuntimeCall) -> bool) -> bool {
    match call {
        RuntimeCall::Utility(
            pallet_utility::Call::batch { calls }
            | pallet_utility::Call::batch_all { calls }
            | pallet_utility::Call::force_batch { calls },
        ) => calls.iter().all(|call| visit_runtime_leaves(call, visit)),
        RuntimeCall::Utility(
            pallet_utility::Call::as_derivative { call, .. }
            | pallet_utility::Call::dispatch_as { call, .. }
            | pallet_utility::Call::with_weight { call, .. },
        )
        | RuntimeCall::Proxy(
            pallet_proxy::Call::proxy { call, .. }
            | pallet_proxy::Call::proxy_announced { call, .. },
        )
        | RuntimeCall::Multisig(
            pallet_multisig::Call::as_multi { call, .. }
            | pallet_multisig::Call::as_multi_threshold_1 { call, .. },
        )
        | RuntimeCall::Sudo(
            pallet_sudo::Call::sudo { call }
            | pallet_sudo::Call::sudo_unchecked_weight { call, .. },
        ) => visit_runtime_leaves(call, visit),
        _ => visit(call),
    }
}

pub(crate) fn derived_execution_meters(
    calls: &pallet_execution_guard::pallet::RuntimeBatch<Runtime>,
) -> Option<pallet_execution_guard::pallet::StoredMeters> {
    let mut meters = Vec::new();
    for call in calls {
        if !visit_runtime_leaves(call, &mut |leaf| {
            let meter = match leaf {
                RuntimeCall::FutarchyTreasury(
                    pallet_futarchy_treasury::Call::spend { .. }
                    | pallet_futarchy_treasury::Call::open_stream { .. },
                ) => Some(RuntimeAdmissionMeter::TreasuryOutflow),
                RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::issue_vit {
                    ..
                }) => Some(RuntimeAdmissionMeter::VitIssuance),
                RuntimeCall::System(frame_system::Call::authorize_upgrade { .. }) => {
                    Some(RuntimeAdmissionMeter::CodeSpacing)
                }
                _ => None,
            };
            if let Some(meter) = meter {
                let key = meter.key();
                if !meters.contains(&key) {
                    meters.push(key);
                }
            }
            true
        }) {
            return None;
        }
    }
    pallet_execution_guard::pallet::StoredMeters::try_from(meters).ok()
}

/// Read-only decision-time preview of every live treasury/issuance/spacing
/// meter touched by the exact recursively decoded batch.
pub(crate) fn preview_batch_admission(
    calls: &pallet_execution_guard::pallet::RuntimeBatch<Runtime>,
) -> bool {
    let mut treasury = crate::FutarchyTreasury::treasury();
    let now = System::block_number();
    let mut ok = true;
    let mut authorize_count = 0_u8;
    for call in calls {
        if !visit_runtime_leaves(call, &mut |leaf| {
            let result = match leaf {
                RuntimeCall::FutarchyTreasury(
                    pallet_futarchy_treasury::Call::fund_budget_line { line, amount },
                ) => treasury.fund_budget_line(
                    pallet_futarchy_treasury::Origin::FutarchyTreasury,
                    *line,
                    *amount,
                ),
                RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
                    line,
                    dest,
                    amount,
                }) => treasury.spend(
                    pallet_futarchy_treasury::Origin::FutarchyTreasury,
                    now,
                    *line,
                    dest.clone().into(),
                    *amount,
                ),
                RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::open_stream {
                    line,
                    recipient,
                    total,
                    start,
                    duration,
                }) => treasury
                    .open_stream(
                        pallet_futarchy_treasury::Origin::FutarchyTreasury,
                        now,
                        pallet_futarchy_treasury::StreamInput {
                            line: *line,
                            recipient: recipient.clone().into(),
                            total: *total,
                            start: *start,
                            duration: *duration,
                        },
                    )
                    .map(|_| ()),
                RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::issue_vit {
                    amount,
                    line,
                }) => treasury.issue_vit(
                    pallet_futarchy_treasury::Origin::FutarchyTreasury,
                    now,
                    *amount,
                    *line,
                ),
                RuntimeCall::System(frame_system::Call::authorize_upgrade { .. }) => {
                    authorize_count = authorize_count.saturating_add(1);
                    let spacing_ok = authorize_count == 1
                        && pallet_execution_guard::LastUpgradeAuthorized::<Runtime>::get()
                            .is_none_or(|last| {
                                now >= last.saturating_add(u32_param(b"code.spacing"))
                            });
                    if spacing_ok {
                        Ok(())
                    } else {
                        Err(pallet_futarchy_treasury::CoreError::MeterExhausted)
                    }
                }
                _ => Ok(()),
            };
            if result.is_err() {
                ok = false;
            }
            ok
        }) {
            return false;
        }
    }
    ok
}

pub struct RuntimeConstitutionAccess;
pub(crate) fn required_proposal_bond(
    proposal: &futarchy_primitives::Proposal<AccountId>,
) -> Option<Balance> {
    match proposal.class {
        futarchy_primitives::ProposalClass::Param => Some(balance_param(b"prop.bond.param")),
        futarchy_primitives::ProposalClass::Treasury => proposal
            .ask
            .checked_mul(kernel::TREASURY_BOND_ASK_BPS)
            .and_then(|value| value.checked_div(kernel::BASIS_POINTS_DENOMINATOR))
            .and_then(|surcharge| balance_param(b"prop.bond.trs").checked_add(surcharge)),
        futarchy_primitives::ProposalClass::Code => Some(balance_param(b"prop.bond.code")),
        futarchy_primitives::ProposalClass::Meta => Some(balance_param(b"prop.bond.meta")),
        futarchy_primitives::ProposalClass::Constitutional => None,
    }
}

impl pallet_epoch::ConstitutionAccess<AccountId> for RuntimeConstitutionAccess {
    fn required_bond(proposal: &futarchy_primitives::Proposal<AccountId>) -> Option<Balance> {
        required_proposal_bond(proposal)
    }

    fn static_check(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> pallet_epoch::StaticCheckDisposition {
        use pallet_epoch::StaticCheckDisposition;
        use pallet_execution_guard::Capabilities;
        let Some(bond_floor) = required_proposal_bond(proposal) else {
            return StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold);
        };
        if proposal.bond < bond_floor {
            // The live floor can rise after submission. That drift is not
            // proposer fraud and therefore cannot confiscate the held bond.
            return StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold);
        }
        let Some(calls) = proposal_calls(proposal) else {
            return StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold);
        };
        if !calls
            .iter()
            .all(|call| RuntimeCapabilities::call_enabled(proposal.class, call))
        {
            return StaticCheckDisposition::SlashAll(
                futarchy_primitives::RejectReason::ConstitutionViolation,
            );
        }
        if matches!(proposal.class, futarchy_primitives::ProposalClass::Treasury)
            && (derived_treasury_ask(&calls) != Some(proposal.ask)
                || Self::in_cap_prize(proposal).is_none())
        {
            return StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold);
        }
        if calls.is_empty() {
            return if proposal.resources.is_empty() {
                StaticCheckDisposition::Eligible
            } else {
                // An empty batch has an exactly-known empty footprint, so a
                // non-empty declaration is a verified false declaration.
                StaticCheckDisposition::SlashAll(
                    futarchy_primitives::RejectReason::ConstitutionViolation,
                )
            };
        }
        // SQ-140: no canonical RuntimeCall→8-byte resource-key mapping exists.
        // A non-empty payload is therefore implementation-unverifiable, never
        // evidence of a false declaration. Cancel status quo and refund.
        StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold)
    }

    fn queue_time_check(proposal: &futarchy_primitives::Proposal<AccountId>) -> bool {
        matches!(
            Self::static_check(proposal),
            pallet_epoch::StaticCheckDisposition::Eligible
        ) && proposal.version_constraint
            == pallet_execution_guard::CurrentSpecName::<Runtime>::get()
            && proposal_calls(proposal).is_some_and(|calls| preview_batch_admission(&calls))
    }

    fn in_cap_prize(proposal: &futarchy_primitives::Proposal<AccountId>) -> Option<Balance> {
        let nav = crate::FutarchyTreasury::nav().spendable_nav;
        let cap = nav
            .checked_mul(Balance::from(percent_param(b"trs.cap_proposal")))?
            .checked_div(100)?;
        match proposal.class {
            futarchy_primitives::ProposalClass::Treasury => {
                let calls = proposal_calls(proposal)?;
                let ask = derived_treasury_ask(&calls)?;
                (ask == proposal.ask && ask <= cap).then_some(ask)
            }
            // A8 fail-closed: PARAM/CODE/META capability-envelope valuation is
            // not recorded on chain. Returning None blocks Adopt at sizing
            // step 9 — owner values/classifier envelope milestone (SQ-141).
            futarchy_primitives::ProposalClass::Param
            | futarchy_primitives::ProposalClass::Code
            | futarchy_primitives::ProposalClass::Meta
            | futarchy_primitives::ProposalClass::Constitutional => None,
        }
    }

    fn ledger_frozen() -> bool {
        Self::phase_flags() & pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN != 0
    }

    fn phase_flags() -> u32 {
        crate::Constitution::phase_flags()
    }

    fn active_metric_spec_version() -> Option<futarchy_primitives::MetricSpecVersion> {
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let candidates = pallet_welfare::MetricSpecs::<Runtime>::iter()
            .filter(|(_, specs)| {
                !specs.is_empty() && specs.iter().all(|spec| spec.activation_epoch <= epoch)
            })
            .filter_map(|(version, specs)| {
                specs
                    .iter()
                    .map(|spec| spec.activation_epoch)
                    .max()
                    .map(|activation| (activation, version))
            })
            .collect::<Vec<_>>();
        let latest = candidates.iter().map(|(activation, _)| *activation).max()?;
        let mut latest_versions = candidates
            .into_iter()
            .filter_map(|(activation, version)| (activation == latest).then_some(version));
        let version = latest_versions.next()?;
        latest_versions.next().is_none().then_some(version)
    }

    fn treasury_gate_required(proposal: &futarchy_primitives::Proposal<AccountId>) -> bool {
        if !matches!(proposal.class, futarchy_primitives::ProposalClass::Treasury) {
            return false;
        }
        let nav = crate::FutarchyTreasury::nav().spendable_nav;
        nav.checked_mul(kernel::TREASURY_GATE_NAV_BPS)
            .and_then(|value| value.checked_div(kernel::BASIS_POINTS_DENOMINATOR))
            .is_none_or(|threshold| proposal.ask > threshold)
    }

    fn attestation_artifact(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Option<futarchy_primitives::H256> {
        use pallet_execution_guard::BatchDispatcher;
        let calls = proposal_calls(proposal)?;
        let mut artifact = None;
        for call in &calls {
            if let Some(hash) = RuntimeBatchDispatcher::authorize_upgrade_hash(call) {
                if artifact.replace(hash).is_some() {
                    return None;
                }
            }
        }
        Some(artifact.map_or(proposal.payload_hash, |hash| hash))
    }
}

pub struct RuntimeEpochPreimages;
impl pallet_epoch::PreimageAccess for RuntimeEpochPreimages {
    fn len(hash: futarchy_primitives::H256) -> Option<u32> {
        <RuntimePreimages as pallet_execution_guard::Preimages>::len(hash)
    }
    fn request(hash: futarchy_primitives::H256) {
        <Preimage as QueryPreimage>::request(&Hash::from(hash));
    }
    fn unrequest(hash: futarchy_primitives::H256) {
        <Preimage as QueryPreimage>::unrequest(&Hash::from(hash));
    }
}

pub struct RuntimeEpochWelfare;
impl pallet_epoch::WelfareSettlement for RuntimeEpochWelfare {
    fn compute_settlement(
        cohort_epoch: EpochId,
        spec: futarchy_primitives::MetricSpecVersion,
        target: pallet_epoch::SettlementTarget,
    ) -> Result<FixedU64, DispatchError> {
        let target = match target {
            pallet_epoch::SettlementTarget::Proposal {
                pid,
                has_gate_books,
            } => pallet_welfare::SettleTarget::Proposal {
                pid,
                has_gate_books,
            },
            pallet_epoch::SettlementTarget::Baseline => pallet_welfare::SettleTarget::Baseline,
        };
        pallet_welfare::Pallet::<Runtime>::compute_settlement(cohort_epoch, spec, target)
    }
}

pub struct RuntimeEpochLedger;
impl pallet_epoch::LedgerResolution for RuntimeEpochLedger {
    fn create_vault(
        pid: futarchy_primitives::ProposalId,
        spec: futarchy_primitives::MetricSpecVersion,
    ) -> DispatchResult {
        match pallet_conditional_ledger::Vaults::<Runtime>::get(pid) {
            Some(vault) if vault.spec == spec => Ok(()),
            Some(_) => Err(DispatchError::Other("epoch vault metric-spec mismatch")),
            None => ConditionalLedger::create_vault(epoch_signed_origin(), pid, spec),
        }
    }

    fn resolve(
        pid: futarchy_primitives::ProposalId,
        branch: futarchy_primitives::Branch,
    ) -> DispatchResult {
        ConditionalLedger::resolve(epoch_signed_origin(), pid, branch)
    }

    fn void(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        ConditionalLedger::void(epoch_signed_origin(), pid)
    }
}

pub struct RuntimeProposalBond;
impl pallet_epoch::ProposalBondCurrency<AccountId> for RuntimeProposalBond {
    fn hold(who: &AccountId, amount: Balance) -> DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::transfer(
            USDC_ASSET_ID,
            who,
            &epoch_account(),
            amount,
            Preservation::Expendable,
        )
        .map(|_| ())
    }

    fn release(who: &AccountId, amount: Balance) -> DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::transfer(
            USDC_ASSET_ID,
            &epoch_account(),
            who,
            amount,
            Preservation::Expendable,
        )
        .map(|_| ())
    }

    fn slash_to_insurance(amount: Balance) -> DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::transfer(
            USDC_ASSET_ID,
            &epoch_account(),
            &insurance_account(),
            amount,
            Preservation::Expendable,
        )
        .map(|_| ())
    }

    fn escrow_balance() -> Balance {
        <ForeignAssets as Inspect<AccountId>>::balance(USDC_ASSET_ID, &epoch_account())
    }
}

impl pallet_epoch::Config for Runtime {
    type Params = RuntimeEpochParams;
    type Market = RuntimeMarketAccess;
    type Oracle = RuntimeEpochOracle;
    type Guardian = RuntimeEpochGuardian;
    type Attestation = RuntimeEpochAttestation;
    type Constitution = RuntimeConstitutionAccess;
    type ProposalBond = RuntimeProposalBond;
    type Preimage = RuntimeEpochPreimages;
    type ExecutionGuard = RuntimeEpochExecutionGuard;
    type Welfare = RuntimeEpochWelfare;
    type Ledger = RuntimeEpochLedger;
    type KeeperRebate = FutarchyTreasury;
    type GuardianOrigin = pallet_origins::EnsureGuardianHold;
    type ExecutionGuardOrigin = EnsureExecutionGuardAccount;
    type VoidAuthority = pallet_origins::EnsureEmergencyPlaybook;
    type ConstitutionalValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    type WeightInfo = pallet_epoch::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

pub struct WelfareParams;
impl pallet_welfare::WelfareParamsProvider for WelfareParams {
    fn theta_s_lo() -> FixedU64 {
        FixedU64(fixed_param(b"welfare.thS_lo"))
    }
    fn theta_s_hi() -> FixedU64 {
        FixedU64(fixed_param(b"welfare.thS_hi"))
    }
    fn theta_c_lo() -> FixedU64 {
        FixedU64(fixed_param(b"welfare.thC_lo"))
    }
    fn theta_c_hi() -> FixedU64 {
        FixedU64(fixed_param(b"welfare.thC_hi"))
    }
    fn w_p() -> FixedU64 {
        FixedU64(fixed_param(b"welfare.wP"))
    }
    fn w_a() -> FixedU64 {
        FixedU64(fixed_param(b"welfare.wA"))
    }
}
/// Runtime source aggregation is not part of the epoch wire-up. Empty vectors
/// make snapshot cranks reject instead of persisting a partial financial input
/// (G-1); the complete source adapters remain owned by the B1 follow-up.
pub struct RuntimeMetricInputs;
impl pallet_welfare::MetricInputs for RuntimeMetricInputs {
    fn onchain_components(_: EpochId, _: u16) -> Vec<pallet_welfare::ComponentValue> {
        Vec::new()
    }
    fn incident_multiplier(epoch: EpochId) -> FixedU64 {
        // The IncidentRegistry aggregate IS the C_attested multiplier
        // (registry-core: an empty closed epoch records exactly 1.0). An
        // absent entry means the epoch is not closed yet; the neutral 1.0 is
        // returned because this seam is unreachable while
        // `onchain_components` is empty (snapshot cranks reject first) — the
        // real MetricInputs must instead gate snapshots on registry close-out
        // rather than fabricate a multiplier (returning 0 here would zero
        // C_attested outright, which is fail-destructive, not fail-safe).
        match pallet_registry::Aggregates::<Runtime>::get(epoch) {
            Some(value) => value,
            None => FixedU64(1_000_000_000),
        }
    }
    fn daily_components(_: EpochId, _: u8, _: u16) -> Vec<pallet_welfare::ComponentValue> {
        Vec::new()
    }
}
pub struct WelfareLedger;
impl pallet_welfare::LedgerSettlement for WelfareLedger {
    fn settle_scalar(pid: u64, score: FixedU64) -> frame_support::dispatch::DispatchResult {
        ConditionalLedger::settle_scalar(
            RuntimeOrigin::signed(welfare_settlement_account()),
            pid,
            score,
        )
    }
    fn settle_gate(
        pid: u64,
        gate: pallet_welfare::GateKind,
        breached: bool,
    ) -> frame_support::dispatch::DispatchResult {
        let gate = match gate {
            pallet_welfare::GateKind::Survival => futarchy_primitives::GateType::Survival,
            pallet_welfare::GateKind::Security => futarchy_primitives::GateType::Security,
        };
        ConditionalLedger::settle_gate(
            RuntimeOrigin::signed(welfare_settlement_account()),
            pid,
            gate,
            breached,
        )
    }
    fn settle_baseline(epoch: EpochId, score: FixedU64) -> frame_support::dispatch::DispatchResult {
        ConditionalLedger::settle_baseline(
            RuntimeOrigin::signed(welfare_settlement_account()),
            epoch,
            score,
        )
    }
}
impl pallet_welfare::Config for Runtime {
    type MetricGovernanceOrigin = pallet_origins::EnsureConstitutionalValues;
    type Params = WelfareParams;
    type MetricInputs = RuntimeMetricInputs;
    type Ledger = WelfareLedger;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type KeeperRebate = FutarchyTreasury;
    type WeightInfo = pallet_welfare::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

fn report_window_end(epoch: EpochId) -> Option<BlockNumber> {
    let timing = pallet_epoch::Pallet::<Runtime>::epoch_timing(epoch)?;
    timing
        .start
        .checked_add(timing.length)?
        .checked_add(kernel::BLOCKS_PER_DAY.checked_mul(2)?)
}

fn cohort_consumes_measurement(
    schedule: &pallet_epoch::CohortSchedule,
    measurement_epoch: EpochId,
) -> bool {
    measurement_epoch > schedule.epoch && measurement_epoch <= schedule.measurement_until
}

fn spec_contains_component(
    version: futarchy_primitives::MetricSpecVersion,
    component: futarchy_primitives::MetricId,
) -> bool {
    pallet_welfare::MetricSpecs::<Runtime>::get(version)
        .is_some_and(|specs| specs.iter().any(|spec| spec.id == component))
}

pub struct RuntimeReporting;
impl pallet_oracle::ReportingContext for RuntimeReporting {
    fn report_window_end(epoch: EpochId) -> u32 {
        report_window_end(epoch).map_or(0, |end| end)
    }
    fn is_expected_spec_version(component: u16, epoch: EpochId, version: u16) -> bool {
        spec_contains_component(version, component)
            && pallet_epoch::CohortSchedules::<Runtime>::iter_values().any(|schedule| {
                cohort_consumes_measurement(&schedule, epoch)
                    && schedule.specs.iter().any(|(_, spec)| *spec == version)
            })
    }
    fn stake_at_risk(component: u16, epoch: EpochId) -> Balance {
        let has_exposure =
            pallet_epoch::CohortSchedules::<Runtime>::iter_values().any(|schedule| {
                cohort_consumes_measurement(&schedule, epoch)
                    && schedule
                        .specs
                        .iter()
                        .any(|(_, version)| spec_contains_component(*version, component))
            });
        if has_exposure {
            // A8 fail-closed: 07 §6.1 freezes value-at-risk at Snapshot(m),
            // but no pallet currently stores that snapshot. Reading mutable
            // live vault escrow could only reduce a later reporter bond, so
            // price the report out until the oracle snapshot owner lands the
            // frozen backing (SQ-142).
            Balance::MAX
        } else {
            0
        }
    }
    fn expected_components(epoch: EpochId) -> Vec<(u16, u16)> {
        let mut expected = Vec::new();
        for schedule in pallet_epoch::CohortSchedules::<Runtime>::iter_values()
            .filter(|schedule| cohort_consumes_measurement(schedule, epoch))
        {
            for (_, version) in schedule.specs {
                if let Some(specs) = pallet_welfare::MetricSpecs::<Runtime>::get(version) {
                    for spec in specs {
                        if !expected.contains(&(spec.id, version)) {
                            expected.push((spec.id, version));
                        }
                    }
                }
            }
        }
        expected
    }
}
impl pallet_oracle::Config for Runtime {
    type AdjudicationOrigin = pallet_origins::EnsureOracleResolution;
    type Reporting = RuntimeReporting;
    // B4 pending probe-dispatch seam: `()` sends nothing, so every probe times
    // out fail-static (07 §8, I-24 — absence is never healthy). Swapped for
    // `bleavit_xcm::probe::XcmProbeDispatcher` when the stub XCM config below
    // (`xcm_config`: Barrier/AssetTransactor/Trader/XcmSender = ()) is replaced
    // by the bleavit-xcm components (the B4 runtime-integration follow-up).
    type ProbeDispatch = ();
    type KeeperRebate = FutarchyTreasury;
    type MaxRoundCloseBatch = ConstU32<{ kernel::TICK_BATCH }>;
    type WeightInfo = pallet_oracle::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

pub struct RegistryParams;
impl pallet_registry::RegistryParams for RegistryParams {
    fn bond_incident() -> Balance {
        balance_param(b"reg.bond_inc")
    }
    fn bond_milestone() -> Balance {
        balance_param(b"reg.bond_mile")
    }
}
pub struct OracleWatchtowers;
impl pallet_registry::WatchtowerRegistry<AccountId> for OracleWatchtowers {
    fn is_registered_watchtower(who: &AccountId) -> bool {
        pallet_oracle::Watchtowers::<Runtime>::contains_key(who)
    }
}
/// The current welfare shell has no external-component write endpoint. The
/// aggregate remains in registry storage and is pulled by `RuntimeMetricInputs`.
pub struct WelfarePullSink;
impl pallet_registry::WelfareSink for WelfarePullSink {
    fn note_external_component(
        _: registry_core::RegistryKind,
        _: EpochId,
        _: FixedU64,
    ) -> sp_runtime::DispatchResult {
        Ok(())
    }
}
pub struct RuntimeRegistryEpoch;
impl pallet_registry::EpochContext for RuntimeRegistryEpoch {
    fn filing_window_end(epoch: EpochId) -> u32 {
        report_window_end(epoch).map_or(0, |end| end)
    }
    fn frozen_spec_version(epoch: EpochId) -> Option<u16> {
        let mut versions = pallet_epoch::CohortSchedules::<Runtime>::iter_values()
            .filter(|schedule| cohort_consumes_measurement(schedule, epoch))
            .flat_map(|schedule| schedule.specs.into_iter().map(|(_, version)| version))
            .collect::<Vec<_>>();
        versions.sort_unstable();
        versions.dedup();
        (versions.len() == 1)
            .then(|| versions.first().copied())
            .flatten()
            .filter(|version| pallet_welfare::MetricSpecs::<Runtime>::contains_key(version))
    }
    fn milestone_target(_: EpochId) -> u32 {
        // A8 fail-closed: MetricSpec has no milestone-target field, so the
        // Milestone registry cannot normalize claims — owner MetricSpec schema
        // amendment/SQ-143. Zero makes filing/close reject.
        0
    }
}
parameter_types! {
    pub const IncidentKind: registry_core::RegistryKind = registry_core::RegistryKind::Incident;
    pub const MilestoneKind: registry_core::RegistryKind = registry_core::RegistryKind::Milestone;
}
macro_rules! registry_config {
    ($instance:ty, $kind:ty, $id:ty) => {
        impl pallet_registry::Config<$instance> for Runtime {
            type Collateral = ForeignAssets;
            type UsdcAssetId = UsdcAssetId;
            type Kind = $kind;
            type Params = RegistryParams;
            type Watchtowers = OracleWatchtowers;
            type Welfare = WelfarePullSink;
            type Epoch = RuntimeRegistryEpoch;
            type ResolutionAuthority = pallet_origins::EnsureOracleResolution;
            type InsuranceAccount = InsuranceAccount;
            type PalletId = $id;
            type KeeperRebate = FutarchyTreasury;
            // SQ-76: registry archive reuses the live ledger archive key.
            type ArchiveDelay = LedgerArchiveDelay;
            type MaxFilingsPerEpoch = ConstU32<{ kernel::REG_MAX_FILINGS_EPOCH }>;
            type MaxEvidenceLen = ConstU32<32>;
            // Registry exposes only its B5 placeholder `WeightInfo for ()` today.
            type WeightInfo = ();
            #[cfg(feature = "runtime-benchmarks")]
            type BenchmarkHelper = RuntimeBenchmarkHelper;
        }
    };
}
registry_config!((), IncidentKind, IncidentPalletId);
registry_config!(pallet_registry::Instance1, MilestoneKind, MilestonePalletId);

pub struct TreasuryParams;
impl pallet_futarchy_treasury::TreasuryParams for TreasuryParams {
    fn cap_proposal_bps() -> u32 {
        u32::from(percent_param(b"trs.cap_proposal")) * 100
    }
    fn cap_30d_bps() -> u32 {
        u32::from(percent_param(b"trs.cap_30d")) * 100
    }
    fn cap_180d_bps() -> u32 {
        u32::from(percent_param(b"trs.cap_180d")) * 100
    }
    fn stream_threshold_bps() -> u32 {
        perbill_param(b"trs.stream_thr") / 100_000
    }
    fn inflation_cap_bps() -> u32 {
        u32::from(percent_param(b"iss.inflation")) * 100
    }
    fn keeper_budget_epoch() -> Balance {
        balance_param(b"keeper.budget")
    }
    fn keeper_rebate() -> Balance {
        // 13 §1 marks this as a benchmark-time formula, so genesis Params
        // deliberately omits it. Unlike the other adapters, do not consult
        // `genesis_params()` as a fallback: absent/wrong-kind means zero until
        // B5 installs a calibrated raw row (conservative no-outflow default).
        let key = pallet_constitution::key16(b"keeper.rebate");
        match live_param(key) {
            Some(pallet_constitution::ParamValue::Balance(value)) => value,
            _ => 0,
        }
    }
}

pub struct TreasuryRebatePayout;
impl pallet_futarchy_treasury::RebatePayout<AccountId> for TreasuryRebatePayout {
    fn pay(
        who: &AccountId,
        amount: Balance,
        line: pallet_futarchy_treasury::PayoutLine,
    ) -> frame_support::pallet_prelude::DispatchResult {
        let source = match line {
            pallet_futarchy_treasury::PayoutLine::Keeper => treasury_keeper_account(),
            pallet_futarchy_treasury::PayoutLine::Oracle => treasury_oracle_account(),
        };
        <ForeignAssets as Mutate<AccountId>>::transfer(
            USDC_ASSET_ID,
            &source,
            who,
            amount,
            Preservation::Preserve,
        )
        .map(|_| ())
    }

    fn pot_balance(line: pallet_futarchy_treasury::PayoutLine) -> Balance {
        let source = match line {
            pallet_futarchy_treasury::PayoutLine::Keeper => treasury_keeper_account(),
            pallet_futarchy_treasury::PayoutLine::Oracle => treasury_oracle_account(),
        };
        <ForeignAssets as Inspect<AccountId>>::balance(USDC_ASSET_ID, &source)
    }
}
/// B4 pending renewal-dispatch seam: fail-closed (G-1) — every
/// `execute_coretime_renewal` rolls back until the real
/// `bleavit_xcm::coretime::XcmRenewalDispatcher` is wired with the stub XCM
/// config swap (09 §4: an unwireable transfer must not consume the quote or
/// mark the period funded; the keeper simply retries once wired).
pub struct PendingRenewalDispatch;
impl pallet_futarchy_treasury::RenewalDispatch for PendingRenewalDispatch {
    fn dispatch_renewal(
        _period_index: u32,
        _amount: Balance,
    ) -> frame_support::dispatch::DispatchResult {
        Err(sp_runtime::DispatchError::Other(
            "coretime renewal XCM dispatch not wired yet (B4 runtime integration)",
        ))
    }
}
impl pallet_futarchy_treasury::Config for Runtime {
    type TreasuryOrigin = pallet_origins::EnsureFutarchyTreasury;
    type Params = TreasuryParams;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type RenewalDispatch = PendingRenewalDispatch;
    type RebatePayout = TreasuryRebatePayout;
    type WeightInfo = pallet_futarchy_treasury::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

pub struct RuntimeGuardianStatus;
impl pallet_guardian::GuardianProposalStatus for RuntimeGuardianStatus {
    fn status(pid: u64) -> (pallet_guardian::ProposalStatus, bool) {
        let Some(proposal) = pallet_epoch::Proposals::<Runtime>::get(pid) else {
            return (pallet_guardian::ProposalStatus::Other, false);
        };
        let status = match proposal.state {
            futarchy_primitives::ProposalState::Trading => pallet_guardian::ProposalStatus::Trading,
            futarchy_primitives::ProposalState::Extended => {
                pallet_guardian::ProposalStatus::Extended
            }
            futarchy_primitives::ProposalState::Queued => pallet_guardian::ProposalStatus::Queued,
            futarchy_primitives::ProposalState::Executed
            | futarchy_primitives::ProposalState::Measuring
            | futarchy_primitives::ProposalState::Settled => {
                pallet_guardian::ProposalStatus::Executed
            }
            futarchy_primitives::ProposalState::Rerun => pallet_guardian::ProposalStatus::Rerun,
            _ => pallet_guardian::ProposalStatus::Other,
        };
        (
            status,
            proposal.rerun
                || proposal.delayed_once
                || matches!(
                    proposal.state,
                    futarchy_primitives::ProposalState::Suspended
                        | futarchy_primitives::ProposalState::Rerun
                ),
        )
    }
}
pub struct RuntimeGuardianTriggers;
impl pallet_guardian::GuardianTriggers for RuntimeGuardianTriggers {
    fn current() -> pallet_guardian::TriggerState {
        let phase_flags = crate::Constitution::phase_flags();
        let current_epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let gate_breach = pallet_welfare::GateBreachFlags::<Runtime>::get(current_epoch)
            .is_some_and(|flags| flags.s_breached || flags.c_breached);
        pallet_guardian::TriggerState {
            gate_breach,
            dead_man: phase_flags & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED != 0,
            reserve_health: phase_flags & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG
                != 0,
            // The relay-abort bit is guardian-visible but deliberately does
            // not freeze the normal re-proposal lane: Abort preserves the old
            // code and must not replace one wedge with another.
            migration_halt: MigrationHaltSources::get() != 0,
            ..pallet_guardian::TriggerState::none()
        }
    }
}
pub struct RuntimeGuardianEffects;
impl pallet_guardian::GuardianEffectDispatcher for RuntimeGuardianEffects {
    fn dispatch(
        power: pallet_guardian::GuardianPower,
        justification_hash: futarchy_primitives::H256,
    ) -> Result<(), DispatchError> {
        match power {
            pallet_guardian::GuardianPower::DelayOnce { pid } => {
                Epoch::delay_once(
                    pallet_origins::Origin::GuardianHold.into(),
                    pid,
                    justification_hash,
                )?;
                let deadline = pallet_epoch::CurrentEpoch::<Runtime>::get()
                    .checked_add(u32_param(b"grd.review_dl"))
                    .ok_or(DispatchError::Arithmetic(
                        sp_runtime::ArithmeticError::Overflow,
                    ))?;
                Epoch::note_guardian_review_deadline(pid, deadline)
            }
            pallet_guardian::GuardianPower::ForceRerun { pid } => {
                Epoch::force_rerun_from_guardian(pid)
            }
            // The Track-A pallets do not expose the preimage-committed
            // pause/playbook effect calls required by 06 §5.2/§6.2. Treat
            // those powers as unavailable instead of emitting an event-only
            // success (SQ-144).
            pallet_guardian::GuardianPower::PauseIntake { .. }
            | pallet_guardian::GuardianPower::ActivatePlaybook { .. }
            | pallet_guardian::GuardianPower::SuspendOnGate => Err(DispatchError::Other(
                "guardian downstream effect is not represented on chain",
            )),
        }
    }
}

fn guardian_review_shares(
    action: pallet_guardian::ActionId,
) -> Result<
    (
        [futarchy_primitives::AccountId; pallet_guardian::GUARDIAN_SEATS],
        u8,
    ),
    DispatchError,
> {
    let review = pallet_guardian::ReviewDeadlines::<Runtime>::get()
        .iter()
        .find(|review| review.action_id == action)
        .copied()
        .ok_or(DispatchError::Other("guardian review record missing"))?;
    if review.approver_count == 0
        || usize::from(review.approver_count) > pallet_guardian::GUARDIAN_SEATS
    {
        return Err(DispatchError::Other("guardian review approver count"));
    }
    Ok((review.approvers, review.approver_count))
}

/// Real retrospective-review submission. The one-VIT stock-referenda
/// submission deposit is fronted pro-rata by the five approving guardians;
/// enactment calls `guardian.ratify_action` with the ConstitutionalValues
/// origin (06 §5.4).
pub struct RuntimeGuardianScheduler;
impl pallet_guardian::GuardianReviewScheduler for RuntimeGuardianScheduler {
    fn schedule_review(action: u32) -> Result<u32, DispatchError> {
        let (approvers, count) = guardian_review_shares(action)?;
        let depositor = AccountId::from(approvers[0]);
        let total = SubmissionDeposit::get();
        let base = total / Balance::from(count);
        let remainder = total % Balance::from(count);
        let extra_count = usize::try_from(remainder)
            .map_err(|_| DispatchError::Other("guardian review remainder"))?;
        for (position, raw) in approvers
            .iter()
            .take(usize::from(count))
            .enumerate()
            .skip(1)
        {
            let share = base.saturating_add(Balance::from(position < extra_count));
            <Balances as frame_support::traits::fungible::Mutate<AccountId>>::transfer(
                &AccountId::from(*raw),
                &depositor,
                share,
                Preservation::Preserve,
            )?;
        }

        let call =
            RuntimeCall::Guardian(pallet_guardian::Call::ratify_action { action_id: action });
        let proposal = <Preimage as StorePreimage>::bound(call)?;
        let values_origin: RuntimeOrigin = pallet_origins::Origin::ConstitutionalValues.into();
        let proposal_origin = Box::new(values_origin.caller().clone());
        let referendum = pallet_referenda::ReferendumCount::<Runtime>::get();
        Referenda::submit(
            RuntimeOrigin::signed(depositor),
            proposal_origin,
            proposal,
            frame_support::traits::schedule::DispatchTime::After(0),
        )?;
        Ok(referendum)
    }

    fn refund_review(action: u32, referendum: u32) -> Result<(), DispatchError> {
        let (approvers, count) = guardian_review_shares(action)?;
        let depositor = AccountId::from(approvers[0]);
        Referenda::refund_submission_deposit(RuntimeOrigin::signed(epoch_account()), referendum)?;
        let total = SubmissionDeposit::get();
        let base = total / Balance::from(count);
        let remainder = total % Balance::from(count);
        let extra_count = usize::try_from(remainder)
            .map_err(|_| DispatchError::Other("guardian review remainder"))?;
        for (position, raw) in approvers
            .iter()
            .take(usize::from(count))
            .enumerate()
            .skip(1)
        {
            let share = base.saturating_add(Balance::from(position < extra_count));
            <Balances as frame_support::traits::fungible::Mutate<AccountId>>::transfer(
                &depositor,
                &AccountId::from(*raw),
                share,
                Preservation::Expendable,
            )?;
        }
        Ok(())
    }
}
impl pallet_guardian::GuardianRecallScheduler for RuntimeGuardianScheduler {
    fn schedule_recall(_: u32) -> Result<u32, DispatchError> {
        Err(DispatchError::Other(
            "guardian recall bond fronting is not wired",
        ))
    }
}
impl pallet_guardian::Config for Runtime {
    type ValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type ProposalStatusProvider = RuntimeGuardianStatus;
    type TriggerProvider = RuntimeGuardianTriggers;
    type EffectDispatcher = RuntimeGuardianEffects;
    type ReviewScheduler = RuntimeGuardianScheduler;
    type RecallScheduler = RuntimeGuardianScheduler;
    type WeightInfo = pallet_guardian::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}
impl pallet_attestor::Config for Runtime {
    type ValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    // Ratification shares ConstitutionalValues pending the stock-referenda track split SQ.
    type RatifyOrigin = pallet_origins::EnsureConstitutionalValues;
    type WeightInfo = pallet_attestor::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

// --- A8/A11 execution-guard production wiring ------------------------------

pub struct RuntimeEpochHandoff;
impl pallet_execution_guard::EpochHandoff for RuntimeEpochHandoff {
    fn payload_hash(pid: futarchy_primitives::ProposalId) -> Option<futarchy_primitives::H256> {
        pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .map(|proposal| proposal.payload_hash)
    }
    fn mark_executed(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        Epoch::mark_executed(RuntimeOrigin::signed(execution_guard_account()), pid)
    }
    fn mark_failed_executed(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        Epoch::mark_failed_executed(RuntimeOrigin::signed(execution_guard_account()), pid)
    }
    fn retry_exhausted_to_measurement(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        Epoch::retry_exhausted_to_measurement(RuntimeOrigin::signed(execution_guard_account()), pid)
    }
    fn reject_or_stale(
        pid: futarchy_primitives::ProposalId,
        reason: futarchy_primitives::RejectReason,
    ) -> DispatchResult {
        Epoch::expire_or_stale_queue(
            RuntimeOrigin::signed(execution_guard_account()),
            pid,
            Some(reason),
        )
    }
    fn is_terminal(pid: futarchy_primitives::ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid).is_none_or(|proposal| {
            !matches!(
                proposal.state,
                futarchy_primitives::ProposalState::Queued
                    | futarchy_primitives::ProposalState::FailedExecuted
                    | futarchy_primitives::ProposalState::Suspended
            )
        })
    }
}

pub struct RuntimeEpochExecutionGuard;
impl pallet_epoch::ExecutionGuardAccess for RuntimeEpochExecutionGuard {
    fn enqueue(
        pid: futarchy_primitives::ProposalId,
        payload_hash: futarchy_primitives::H256,
        version_constraint: Option<futarchy_primitives::RuntimeVersionConstraint>,
        maturity: BlockNumber,
        grace: BlockNumber,
        requires_ratification: bool,
    ) -> DispatchResult {
        use pallet_execution_guard::{BatchDispatcher, Preimages};

        let proposal = pallet_epoch::Proposals::<Runtime>::get(pid)
            .ok_or(DispatchError::Other("epoch proposal missing at enqueue"))?;
        frame_support::ensure!(
            proposal.payload_hash == payload_hash,
            DispatchError::Other("epoch payload mismatch")
        );
        // `decide` invokes this seam before persisting its in-memory Queued
        // transition. The old on-chain proposal therefore cannot be used to
        // validate maturity/grace; both values are produced by epoch-core from
        // constitution-backed class parameters and arrive over the
        // sovereign-account-only seam. Immutable payload/class fields are
        // still checked against storage here, and guard enqueue re-derives the
        // committed batch before writing either side (I-9).
        frame_support::ensure!(
            requires_ratification
                == matches!(
                    proposal.class,
                    futarchy_primitives::ProposalClass::Code
                        | futarchy_primitives::ProposalClass::Meta
                ),
            DispatchError::Other("epoch ratification-class mismatch")
        );
        let bytes = RuntimePreimages::fetch(payload_hash)
            .ok_or(DispatchError::Other("epoch payload preimage missing"))?;
        let mut input = &bytes[..];
        let calls = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::decode(&mut input)
            .map_err(|_| DispatchError::Other("epoch payload batch invalid"))?;
        frame_support::ensure!(
            input.is_empty(),
            DispatchError::Other("epoch payload trailing bytes")
        );
        let mut declared_domains = pallet_execution_guard::pallet::StoredDomains::default();
        let mut artifact = None;
        for call in &calls {
            let analysis = RuntimeBatchDispatcher::rederive_call(call)?;
            for domain in analysis.domains {
                if !declared_domains.contains(&domain) {
                    declared_domains
                        .try_push(domain)
                        .map_err(|_| DispatchError::Other("epoch payload domain bound"))?;
                }
            }
            if let Some(hash) = RuntimeBatchDispatcher::authorize_upgrade_hash(call) {
                frame_support::ensure!(
                    artifact.is_none(),
                    DispatchError::Other("multiple upgrade commitments")
                );
                artifact = Some(hash);
            }
        }
        let meters_declared = derived_execution_meters(&calls)
            .ok_or(DispatchError::Other("epoch meter derivation bound"))?;
        let ratify_ref = if requires_ratification {
            pallet_execution_guard::Ratifications::<Runtime>::get(pid)
                .map(|record| record.referendum_index)
        } else {
            None
        };
        let attestation_id = if requires_ratification {
            let committed = artifact.map_or(payload_hash, |hash| hash);
            pallet_attestor::Attestations::<Runtime>::get()
                .iter()
                .find(|record| {
                    record.pid == pid
                        && record.artifact_hash == committed
                        && <RuntimeAttestations as pallet_execution_guard::Attestations>::present_unrevoked_unchallenged(record.id)
                })
                .map(|record| record.id)
        } else {
            None
        };
        let grace_end = maturity
            .checked_add(grace)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Overflow,
            ))?;
        crate::ExecutionGuard::enqueue(
            RuntimeOrigin::signed(epoch_account()),
            pallet_execution_guard::pallet::StoredQueuedExecution {
                pid,
                payload_hash,
                payload_len: proposal.payload_len,
                class: proposal.class,
                maturity,
                grace_end,
                version_constraint: version_constraint
                    .ok_or(DispatchError::Other("runtime version constraint missing"))?,
                meters_declared,
                ratify_ref,
                ratification_passed: false,
                attestation_id,
                pre_upgrade_checkpoint: None,
                cancelled: false,
                declared_domains,
                failed_at: None,
            },
            false,
        )
    }

    fn queue_reject_reason(
        pid: futarchy_primitives::ProposalId,
    ) -> Option<futarchy_primitives::RejectReason> {
        crate::ExecutionGuard::queue_reject_reason(pid)
    }

    fn retry_exhausted(pid: futarchy_primitives::ProposalId) -> bool {
        crate::ExecutionGuard::retry_exhausted(pid)
    }

    fn dequeue_terminal(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        crate::ExecutionGuard::dequeue_terminal(pid)
    }

    fn dequeue_for_rerun(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        crate::ExecutionGuard::dequeue_for_rerun(pid)
    }
}

pub struct RuntimeAttestations;
impl pallet_execution_guard::Attestations for RuntimeAttestations {
    fn artifact_hash(attestation_id: u32) -> Option<futarchy_primitives::H256> {
        pallet_attestor::Attestations::<Runtime>::get()
            .into_iter()
            .find_map(|record| (record.id == attestation_id).then_some(record.artifact_hash))
    }
    fn present_unrevoked_unchallenged(attestation_id: u32) -> bool {
        let record = pallet_attestor::Attestations::<Runtime>::get()
            .into_iter()
            .find(|record| record.id == attestation_id);
        record.is_some_and(|record| {
            matches!(
                record.challenge,
                None | Some(pallet_attestor::ChallengeStatus::Upheld)
            ) && pallet_attestor::Members::<Runtime>::get()
                .iter()
                .any(|member| member.account == record.attestor && member.active)
        })
    }
    fn has_quorum(
        pid: futarchy_primitives::ProposalId,
        artifact_hash: futarchy_primitives::H256,
    ) -> bool {
        crate::Attestor::has_quorum(pid, artifact_hash)
    }
}

pub struct RuntimeGuardianState;
impl pallet_execution_guard::GuardianState for RuntimeGuardianState {
    fn rerun_held(pid: futarchy_primitives::ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid).is_some_and(|proposal| {
            matches!(
                proposal.state,
                futarchy_primitives::ProposalState::Suspended
                    | futarchy_primitives::ProposalState::Rerun
            )
        })
    }
    fn ledger_freeze_active() -> bool {
        crate::Guardian::playbook_active(pallet_guardian::PlaybookId::LedgerFreeze)
    }
}

pub struct ExecutionParams;
impl pallet_execution_guard::Params for ExecutionParams {
    fn exec_timelock(class: futarchy_primitives::ProposalClass) -> BlockNumber {
        match class {
            futarchy_primitives::ProposalClass::Param => u32_param(b"exec.lock.param"),
            futarchy_primitives::ProposalClass::Treasury => u32_param(b"exec.lock.trs"),
            futarchy_primitives::ProposalClass::Code => u32_param(b"exec.lock.code"),
            futarchy_primitives::ProposalClass::Meta => u32_param(b"exec.lock.meta"),
            futarchy_primitives::ProposalClass::Constitutional => 0,
        }
    }
    fn exec_grace(_: futarchy_primitives::ProposalClass) -> BlockNumber {
        u32_param(b"exec.grace")
    }
    fn code_spacing() -> BlockNumber {
        u32_param(b"code.spacing")
    }
}

pub struct RuntimeCapabilities;
impl RuntimeCapabilities {
    fn enabled(
        class: futarchy_primitives::ProposalClass,
        capability: pallet_constitution::Capability,
    ) -> bool {
        // `capability_enabled` is intentionally an exact live-table lookup:
        // an absent `(class, capability)` row is disabled, matching the core.
        crate::Constitution::capability_enabled(class, capability)
    }

    fn leaf_enabled(class: futarchy_primitives::ProposalClass, call: &RuntimeCall) -> bool {
        match call {
            RuntimeCall::Constitution(pallet_constitution::Call::set_param { key, .. }) => {
                Self::enabled(class, pallet_constitution::Capability::SetParam(*key))
            }
            RuntimeCall::Constitution(pallet_constitution::Call::set_capability { .. }) => {
                Self::enabled(class, pallet_constitution::Capability::SetCapability)
            }
            RuntimeCall::Constitution(pallet_constitution::Call::amend_registry { .. }) => {
                Self::enabled(class, pallet_constitution::Capability::AmendRegistry)
            }
            RuntimeCall::Constitution(pallet_constitution::Call::set_release_channel {
                ..
            }) => Self::enabled(class, pallet_constitution::Capability::SetReleaseChannel),
            RuntimeCall::System(frame_system::Call::authorize_upgrade { .. }) => {
                Self::enabled(class, pallet_constitution::Capability::AuthorizeUpgrade)
            }
            RuntimeCall::FutarchyTreasury(
                pallet_futarchy_treasury::Call::fund_budget_line { .. }
                | pallet_futarchy_treasury::Call::spend { .. }
                | pallet_futarchy_treasury::Call::open_stream { .. }
                | pallet_futarchy_treasury::Call::cancel_stream { .. }
                | pallet_futarchy_treasury::Call::issue_vit { .. }
                | pallet_futarchy_treasury::Call::recover_foreign { .. },
            ) => Self::enabled(class, pallet_constitution::Capability::TreasurySpend),
            _ => {
                let Ok(analysis) =
                    <RuntimeBatchDispatcher as pallet_execution_guard::BatchDispatcher<
                        RuntimeCall,
                    >>::rederive_call(call)
                else {
                    return false;
                };
                analysis.domains.iter().all(|domain| match domain {
                    pallet_execution_guard::CallDomain::Public
                    | pallet_execution_guard::CallDomain::InternalRootApplyUpgrade => true,
                    // Wrappers are peeled by `call_enabled`, so this arm only
                    // sees genuine leaves. EVERY privileged leaf requires an
                    // exact keyed/variant mapping above — a newly classified
                    // Treasury/Code/Param/Meta call fails closed until its
                    // 06 §3.2 capability row is made explicit here (it must
                    // never inherit a broad capability structurally).
                    pallet_execution_guard::CallDomain::Param
                    | pallet_execution_guard::CallDomain::Treasury
                    | pallet_execution_guard::CallDomain::Code
                    | pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade
                    | pallet_execution_guard::CallDomain::Meta => false,
                })
            }
        }
    }
}

impl pallet_execution_guard::Capabilities<RuntimeCall> for RuntimeCapabilities {
    fn call_enabled(class: futarchy_primitives::ProposalClass, call: &RuntimeCall) -> bool {
        match call {
            RuntimeCall::Utility(
                pallet_utility::Call::batch { calls }
                | pallet_utility::Call::batch_all { calls }
                | pallet_utility::Call::force_batch { calls },
            ) => calls.iter().all(|call| Self::call_enabled(class, call)),
            RuntimeCall::Utility(
                pallet_utility::Call::as_derivative { call, .. }
                | pallet_utility::Call::dispatch_as { call, .. }
                | pallet_utility::Call::with_weight { call, .. },
            )
            | RuntimeCall::Proxy(
                pallet_proxy::Call::proxy { call, .. }
                | pallet_proxy::Call::proxy_announced { call, .. },
            )
            | RuntimeCall::Multisig(
                pallet_multisig::Call::as_multi { call, .. }
                | pallet_multisig::Call::as_multi_threshold_1 { call, .. },
            )
            | RuntimeCall::Sudo(
                pallet_sudo::Call::sudo { call }
                | pallet_sudo::Call::sudo_unchecked_weight { call, .. },
            ) => Self::call_enabled(class, call),
            _ => Self::leaf_enabled(class, call),
        }
    }
}

pub struct RuntimePreimages;
impl pallet_execution_guard::Preimages for RuntimePreimages {
    fn len(hash: futarchy_primitives::H256) -> Option<u32> {
        <Preimage as QueryPreimage>::len(&Hash::from(hash))
    }
    fn fetch(hash: futarchy_primitives::H256) -> Option<Vec<u8>> {
        <Preimage as QueryPreimage>::fetch(&Hash::from(hash), None)
            .ok()
            .map(Cow::into_owned)
    }
    fn pin(hash: futarchy_primitives::H256) -> DispatchResult {
        <Preimage as QueryPreimage>::request(&Hash::from(hash));
        Ok(())
    }
    fn unpin(hash: futarchy_primitives::H256) -> DispatchResult {
        let hash = Hash::from(hash);
        if !<Preimage as QueryPreimage>::is_requested(&hash) {
            return Err(DispatchError::Unavailable);
        }
        <Preimage as QueryPreimage>::unrequest(&hash);
        Ok(())
    }
}

fn write_release_u32(
    bytes: &mut [u8; pallet_constitution::RELEASE_CHANNEL_LEN],
    range: core::ops::Range<usize>,
    value: u32,
) -> DispatchResult {
    let slot = bytes
        .get_mut(range)
        .ok_or(DispatchError::Other("release channel offset"))?;
    slot.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

pub struct RuntimeReleaseChannel;
impl pallet_execution_guard::ReleaseChannelWriter for RuntimeReleaseChannel {
    fn on_upgrade_authorized(
        target_spec_version: u32,
        authorized_at: BlockNumber,
    ) -> DispatchResult {
        let channel = pallet_constitution::ReleaseChannel::<Runtime>::get();
        let mut bytes = channel.bytes;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_UPDATED_AT,
            authorized_at,
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_SPEC_VERSION,
            target_spec_version,
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_PENDING_AUTHORIZED_AT,
            authorized_at,
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_FLAGS,
            channel.flags() | pallet_constitution::RELEASE_CHANNEL_FLAG_URGENT_UPGRADE,
        )?;
        crate::Constitution::note_release_channel(bytes)
    }
    fn on_upgrade_applied(target_spec_version: u32) -> DispatchResult {
        let channel = pallet_constitution::ReleaseChannel::<Runtime>::get();
        // Tolerant clear (G-1/SQ-134, PR #65 P1): the caller has already
        // verified the installed `:code` hash and version — an applied
        // upgrade cannot be retried, so a writer-(b) `set_release_channel`
        // rewrite that no longer shows this pending upgrade must not wedge
        // `PendingUpgrade`. Writer (b)'s newer value is authoritative; leave
        // it untouched and let the guard record the application.
        if channel.pending_authorized_at() == 0 || channel.spec_version() != target_spec_version {
            return Ok(());
        }
        let mut bytes = channel.bytes;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_UPDATED_AT,
            System::block_number(),
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_PENDING_AUTHORIZED_AT,
            0,
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_FLAGS,
            channel.flags() & !pallet_constitution::RELEASE_CHANNEL_FLAG_URGENT_UPGRADE,
        )?;
        crate::Constitution::note_release_channel(bytes)
    }
    fn on_upgrade_aborted(target_spec_version: u32) -> DispatchResult {
        let channel = pallet_constitution::ReleaseChannel::<Runtime>::get();
        // Tolerant clear (G-1/SQ-131): a writer-(b) `set_release_channel`
        // rewrite during the in-flight upgrade is newer and authoritative —
        // leave it untouched and still let the guard restore its status quo.
        // Only a channel that still shows exactly this pending upgrade is
        // cleared (bump `updated_at`, zero pending, drop URGENT — the same
        // writer-(a) shape as the applied-path clear, SQ-133 offsets).
        if channel.pending_authorized_at() == 0 || channel.spec_version() != target_spec_version {
            return Ok(());
        }
        let mut bytes = channel.bytes;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_UPDATED_AT,
            System::block_number(),
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_PENDING_AUTHORIZED_AT,
            0,
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_FLAGS,
            channel.flags() & !pallet_constitution::RELEASE_CHANNEL_FLAG_URGENT_UPGRADE,
        )?;
        crate::Constitution::note_release_channel(bytes)
    }
}

pub struct RuntimeUpgradeSchedule;
impl pallet_execution_guard::UpgradeSchedule for RuntimeUpgradeSchedule {
    fn scheduling_performed() -> bool {
        // A guard pending upgrade exists before application. Scheduling is
        // proven only once frame-system consumed AuthorizedUpgrade and
        // Cumulus durably holds the validation function for relay review.
        cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::exists()
            && System::authorized_upgrade().is_none()
    }
}

fn guard_domain(
    domain: origins_core::CallDomain,
) -> Result<pallet_execution_guard::CallDomain, DispatchError> {
    match domain {
        origins_core::CallDomain::Public => Ok(pallet_execution_guard::CallDomain::Public),
        origins_core::CallDomain::Param => Ok(pallet_execution_guard::CallDomain::Param),
        origins_core::CallDomain::Treasury => Ok(pallet_execution_guard::CallDomain::Treasury),
        origins_core::CallDomain::Code => Ok(pallet_execution_guard::CallDomain::Code),
        origins_core::CallDomain::Meta => Ok(pallet_execution_guard::CallDomain::Meta),
        origins_core::CallDomain::InternalRoot => {
            Ok(pallet_execution_guard::CallDomain::InternalRootApplyUpgrade)
        }
        origins_core::CallDomain::Nobody
        | origins_core::CallDomain::ConstitutionalValues
        | origins_core::CallDomain::OracleResolution
        | origins_core::CallDomain::GuardianHold
        | origins_core::CallDomain::EmergencyPlaybook => {
            Err(DispatchError::Other("guard-inadmissible call domain"))
        }
    }
}

fn collect_guard_domains(
    call: &origins_core::RuntimeCall,
    domains: &mut pallet_execution_guard::ReDerivedDomains,
    nested_calls: &mut u32,
) -> DispatchResult {
    *nested_calls = nested_calls
        .checked_add(1)
        .ok_or(DispatchError::Other("guard nested-call overflow"))?;
    match call {
        origins_core::RuntimeCall::Leaf(domain) => {
            let domain = guard_domain(*domain)?;
            if !domains.contains(&domain) {
                domains
                    .try_push(domain)
                    .map_err(|_| DispatchError::Other("too many guard domains"))?;
            }
        }
        origins_core::RuntimeCall::UtilityBatch(calls)
        | origins_core::RuntimeCall::UtilityBatchAll(calls)
        | origins_core::RuntimeCall::UtilityForceBatch(calls) => {
            for call in calls {
                collect_guard_domains(call, domains, nested_calls)?;
            }
        }
        origins_core::RuntimeCall::UtilityDispatchAs(call)
        | origins_core::RuntimeCall::UtilityAsDerivative(call)
        | origins_core::RuntimeCall::UtilityWithWeight(call)
        | origins_core::RuntimeCall::Proxy(call)
        | origins_core::RuntimeCall::ProxyAnnounced(call)
        | origins_core::RuntimeCall::MultisigAsMulti(call)
        | origins_core::RuntimeCall::MultisigAsMultiThreshold1(call)
        | origins_core::RuntimeCall::Sudo(call) => {
            collect_guard_domains(&call.0, domains, nested_calls)?;
        }
        origins_core::RuntimeCall::Scheduler { call, .. } => {
            collect_guard_domains(&call.0, domains, nested_calls)?;
        }
        origins_core::RuntimeCall::MultisigApproveAsMulti => {}
    }
    Ok(())
}

fn guard_has_non_atomic_wrapper(call: &origins_core::RuntimeCall) -> bool {
    match call {
        origins_core::RuntimeCall::UtilityBatch(_)
        | origins_core::RuntimeCall::UtilityForceBatch(_)
        | origins_core::RuntimeCall::Sudo(_) => true,
        origins_core::RuntimeCall::UtilityBatchAll(calls) => {
            calls.iter().any(guard_has_non_atomic_wrapper)
        }
        origins_core::RuntimeCall::UtilityDispatchAs(call)
        | origins_core::RuntimeCall::UtilityAsDerivative(call)
        | origins_core::RuntimeCall::UtilityWithWeight(call)
        | origins_core::RuntimeCall::Proxy(call)
        | origins_core::RuntimeCall::ProxyAnnounced(call)
        | origins_core::RuntimeCall::MultisigAsMulti(call)
        | origins_core::RuntimeCall::MultisigAsMultiThreshold1(call) => {
            guard_has_non_atomic_wrapper(&call.0)
        }
        origins_core::RuntimeCall::Scheduler { call, .. } => guard_has_non_atomic_wrapper(&call.0),
        origins_core::RuntimeCall::Leaf(_) | origins_core::RuntimeCall::MultisigApproveAsMulti => {
            false
        }
    }
}

fn runtime_call_contains_apply(call: &RuntimeCall) -> bool {
    match call {
        RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { .. }) => true,
        RuntimeCall::Utility(
            pallet_utility::Call::batch { calls }
            | pallet_utility::Call::batch_all { calls }
            | pallet_utility::Call::force_batch { calls },
        ) => calls.iter().any(runtime_call_contains_apply),
        RuntimeCall::Utility(
            pallet_utility::Call::as_derivative { call, .. }
            | pallet_utility::Call::dispatch_as { call, .. }
            | pallet_utility::Call::with_weight { call, .. },
        )
        | RuntimeCall::Proxy(
            pallet_proxy::Call::proxy { call, .. }
            | pallet_proxy::Call::proxy_announced { call, .. },
        )
        | RuntimeCall::Multisig(
            pallet_multisig::Call::as_multi { call, .. }
            | pallet_multisig::Call::as_multi_threshold_1 { call, .. },
        )
        | RuntimeCall::Sudo(
            pallet_sudo::Call::sudo { call }
            | pallet_sudo::Call::sudo_unchecked_weight { call, .. },
        ) => runtime_call_contains_apply(call),
        _ => false,
    }
}

/// Exact stable2603 pre-write checks performed by
/// `cumulus_pallet_parachain_system::schedule_code_upgrade`. Frame-system
/// removes `AuthorizedUpgrade` before invoking `OnSetCode`, and a direct
/// dispatch is not transactional, so every typed Cumulus rejection must be
/// refused by the filter before frame-system can consume the authorization.
fn parachain_upgrade_preflight(code: &[u8]) -> DispatchResult {
    use cumulus_pallet_parachain_system as parachain_system;

    if !parachain_system::ValidationData::<Runtime>::exists() {
        return Err(parachain_system::Error::<Runtime>::ValidationDataNotAvailable.into());
    }
    if parachain_system::UpgradeRestrictionSignal::<Runtime>::get().is_some() {
        return Err(parachain_system::Error::<Runtime>::ProhibitedByPolkadot.into());
    }
    if parachain_system::PendingValidationCode::<Runtime>::exists() {
        return Err(parachain_system::Error::<Runtime>::OverlappingUpgrades.into());
    }
    let host = parachain_system::HostConfiguration::<Runtime>::get()
        .ok_or(parachain_system::Error::<Runtime>::HostConfigurationNotAvailable)?;
    let code_len =
        u32::try_from(code.len()).map_err(|_| parachain_system::Error::<Runtime>::TooBig)?;
    if code_len > host.max_code_size {
        return Err(parachain_system::Error::<Runtime>::TooBig.into());
    }
    Ok(())
}

pub struct RuntimeBatchDispatcher;
impl pallet_execution_guard::BatchDispatcher<RuntimeCall> for RuntimeBatchDispatcher {
    fn rederive_call(
        call: &RuntimeCall,
    ) -> Result<pallet_execution_guard::ReDerivedCall, DispatchError> {
        if Self::authorize_upgrade_hash(call).is_some() {
            let mut domains = pallet_execution_guard::ReDerivedDomains::default();
            domains
                .try_push(pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade)
                .map_err(|_| DispatchError::Other("authorize domain bound"))?;
            return Ok(pallet_execution_guard::ReDerivedCall {
                domains,
                nested_calls: 1,
            });
        }
        let projected = crate::classifier::project_for_guard(call);
        let mut domains = pallet_execution_guard::ReDerivedDomains::default();
        let mut nested_calls = 0;
        collect_guard_domains(&projected, &mut domains, &mut nested_calls)?;
        if runtime_call_contains_apply(call)
            && !domains.contains(&pallet_execution_guard::CallDomain::InternalRootApplyUpgrade)
        {
            domains
                .try_push(pallet_execution_guard::CallDomain::InternalRootApplyUpgrade)
                .map_err(|_| DispatchError::Other("apply domain bound"))?;
        }
        Ok(pallet_execution_guard::ReDerivedCall {
            domains,
            nested_calls,
        })
    }

    fn safety_filter(class: futarchy_primitives::ProposalClass, call: &RuntimeCall) -> bool {
        let projected = crate::classifier::project_for_guard(call);
        if guard_has_non_atomic_wrapper(&projected) {
            return false;
        }
        origins_core::Origin::from_proposal_class(class).is_some_and(|origin| {
            crate::classifier::RuntimeBaseCallFilter::contains_for(origin, call)
        })
    }

    fn authorize_upgrade_hash(call: &RuntimeCall) -> Option<futarchy_primitives::H256> {
        match call {
            RuntimeCall::System(frame_system::Call::authorize_upgrade { code_hash }) => {
                Some(code_hash.0)
            }
            _ => None,
        }
    }

    fn dispatch_with_class_origin(
        call: RuntimeCall,
        class: futarchy_primitives::ProposalClass,
    ) -> DispatchResult {
        let class_origin =
            origins_core::Origin::from_proposal_class(class).ok_or(DispatchError::BadOrigin)?;
        if !crate::classifier::RuntimeBaseCallFilter::contains_for(class_origin, &call) {
            return Err(DispatchError::Other("guard dispatch-time safety filter"));
        }
        let origin = pallet_origins::Origin::from(class_origin);
        call.dispatch_bypass_filter(RuntimeOrigin::from(origin))
            .map(|_| ())
            .map_err(|error| error.error)
    }

    fn dispatch_authorize_upgrade(code_hash: futarchy_primitives::H256) -> DispatchResult {
        let call = RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: Hash::from(code_hash),
        });
        call.dispatch_bypass_filter(RuntimeOrigin::root())
            .map(|_| ())
            .map_err(|error| error.error)
    }

    fn dispatch_apply_authorized_upgrade(code: Vec<u8>) -> DispatchResult {
        frame_support::storage::with_storage_layer(|| {
            parachain_upgrade_preflight(&code)?;

            // PB-MIGRATION's rollback is a forward remediation upgrade. The
            // stock frame-system preflight rejects every non-empty MBM cursor,
            // so only an actually stuck cursor, or a still-live active stall
            // backed by the migration failure/stall sources, is retired before
            // scheduling. Unrelated alarms and resumed/healthy active work do
            // not make a cursor disposable.
            let retire_cursor = match pallet_migrations::Cursor::<Runtime>::get() {
                Some(pallet_migrations::MigrationCursor::Stuck) => true,
                Some(pallet_migrations::MigrationCursor::Active(ref cursor)) => {
                    MigrationHaltSources::get() & (MIGRATION_FAILURE_HALT | MIGRATION_STALL_HALT)
                        != 0
                        && active_migration_stall_is_live(cursor)
                }
                None => false,
            };
            if retire_cursor {
                pallet_migrations::Cursor::<Runtime>::kill();
                MigrationProgressMarker::kill();
            } else {
                #[cfg(not(feature = "runtime-benchmarks"))]
                System::can_set_code(&code, true).into_result()?;
            }

            let call = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code });
            call.dispatch(RuntimeOrigin::none())
                .map(|_| ())
                .map_err(|error| error.error)
        })
    }

    fn observed_runtime_version(
        code: &[u8],
    ) -> Option<futarchy_primitives::RuntimeVersionConstraint> {
        let decoded = sp_io::misc::runtime_version(code)
            .and_then(|bytes| sp_version::RuntimeVersion::decode(&mut &bytes[..]).ok());
        #[cfg(feature = "runtime-benchmarks")]
        let decoded = decoded.or_else(|| {
            let mut version = VERSION;
            version.spec_version = version.spec_version.saturating_add(1);
            Some(version)
        });
        let version = decoded?;
        let spec_name = futarchy_primitives::BoundedVec::<u8, 32>::try_from(
            version.spec_name.as_bytes().to_vec(),
        )
        .ok()?;
        Some(futarchy_primitives::RuntimeVersionConstraint {
            spec_name,
            spec_version: version.spec_version,
        })
    }

    fn checkpoint() -> (futarchy_primitives::H256, futarchy_primitives::H256) {
        let parent_hash = System::parent_hash().0;
        let root = sp_io::storage::root(VERSION.state_version());
        let state_root = Hash::decode(&mut &root[..]).map_or([0; 32], |hash| hash.0);
        (parent_hash, state_root)
    }
}

/// The origin-blind base filter admits the permissionless frame-system apply
/// call only after reproducing every artifact-dependent guard precondition.
/// This prevents a direct call from consuming `AuthorizedUpgrade` with the
/// wrong version or while pallet-migrations is active.
pub(crate) fn direct_system_upgrade_allowed(code: &[u8]) -> bool {
    let Some(pending) = pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() else {
        return false;
    };
    if sp_io::hashing::blake2_256(code) != pending.hash {
        return false;
    }
    let Some(observed) = <RuntimeBatchDispatcher as pallet_execution_guard::BatchDispatcher<
        RuntimeCall,
    >>::observed_runtime_version(code) else {
        return false;
    };
    let Some(current) = pallet_execution_guard::CurrentSpecName::<Runtime>::get() else {
        return false;
    };
    let version_matches = observed.spec_name == current.spec_name
        && observed.spec_version == pending.target_spec_version;
    #[cfg(not(feature = "runtime-benchmarks"))]
    let preflight_passes = System::can_set_code(code, true).into_result().is_ok()
        && parachain_upgrade_preflight(code).is_ok();
    #[cfg(feature = "runtime-benchmarks")]
    let preflight_passes = true;
    version_matches && preflight_passes
}

fn scheduled_upgrade_aborted() -> bool {
    use cumulus_primitives_core::relay_chain::UpgradeGoAhead;

    let Some(pending) = pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() else {
        return false;
    };
    if pallet_execution_guard::ScheduledUpgrade::<Runtime>::get() != Some(pending.hash)
        || !matches!(
            cumulus_pallet_parachain_system::UpgradeGoAhead::<Runtime>::get(),
            Some(UpgradeGoAhead::Abort)
        )
        || cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::exists()
    {
        return false;
    }
    // The scheduled latch surviving proves `on_validation_code_applied` did
    // not complete. The installed-code comparison independently separates an
    // Abort from the GoAhead path that consumed the same Cumulus pending key.
    sp_io::storage::get(sp_core::storage::well_known_keys::CODE)
        .map(|code| sp_io::hashing::blake2_256(&code) != pending.hash)
        .unwrap_or(true)
}

/// Cumulus calls this only after relay `GoAhead` has written the new `:code`.
/// Any missing/mismatched guard state raises PB-MIGRATION instead of claiming
/// that an untracked upgrade applied.
pub struct ExecutionGuardSystemEvent;
impl cumulus_pallet_parachain_system::OnSystemEvent for ExecutionGuardSystemEvent {
    fn on_validation_data(_: &cumulus_primitives_core::PersistedValidationData) {
        frame_system::Pallet::<Runtime>::register_extra_weight_unchecked(
            migration_validation_hook_weight(),
            DispatchClass::Mandatory,
        );
        // Called once by the mandatory parachain inherent before the
        // executive services the MBM cursor for this block. Comparing with
        // the prior marker is O(1) storage and bounded by CursorMaxLen.
        track_migration_progress();
        if scheduled_upgrade_aborted() {
            if crate::ExecutionGuard::validation_code_aborted().is_ok() {
                // Guardian-visible incident trigger, intentionally not an
                // execution-queue halt: the relay preserved status quo and a
                // fresh normal proposal must remain possible.
                set_migration_halt_source(UPGRADE_ABORT_TRIGGER);
            } else {
                // A failed status-quo cleanup is itself a halt-worthy applied
                // boundary mismatch; retain every pending record for review.
                set_migration_halt_source(UPGRADE_ABORT_TRIGGER | APPLIED_DETECTION_HALT);
            }
        }
    }
    fn on_validation_code_applied() {
        let valid =
            sp_io::storage::get(sp_core::storage::well_known_keys::CODE).is_some_and(|code| {
                let hash = sp_io::hashing::blake2_256(&code);
                let observed =
                    <RuntimeBatchDispatcher as pallet_execution_guard::BatchDispatcher<
                        RuntimeCall,
                    >>::observed_runtime_version(&code);
                pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some_and(
                    |pending| {
                        let current = pallet_execution_guard::CurrentSpecName::<Runtime>::get();
                        hash == pending.hash
                            && observed.is_some_and(|version| {
                                current.is_some_and(|current| {
                                    version.spec_name == current.spec_name
                                        && version.spec_version == pending.target_spec_version
                                })
                            })
                    },
                )
            });
        if !valid || crate::ExecutionGuard::validation_code_applied().is_err() {
            set_migration_halt_source(APPLIED_DETECTION_HALT);
        } else {
            MigrationFailedStep::kill();
            MigrationProgressMarker::kill();
            // A valid recovery image may intentionally carry zero MBMs, in
            // which case `MigrationStatusHandler::completed()` never fires.
            // Any MBM in the new image that later fails/stalls re-raises its
            // own source through the normal handlers.
            clear_migration_halt_sources(
                MIGRATION_FAILURE_HALT
                    | MIGRATION_STALL_HALT
                    | APPLIED_DETECTION_HALT
                    | UPGRADE_ABORT_TRIGGER,
            );
        }
    }
    fn on_relay_state_proof(
        _: &cumulus_pallet_parachain_system::relay_state_snapshot::RelayChainStateProof,
    ) -> Weight {
        Weight::zero()
    }
}

impl pallet_execution_guard::Config for Runtime {
    type Epoch = RuntimeEpochHandoff;
    type EnqueueAuthority = EnsureEpochAccount;
    type Attestations = RuntimeAttestations;
    type KeeperRebate = FutarchyTreasury;
    type Guardian = RuntimeGuardianState;
    type Params = ExecutionParams;
    type Capabilities = RuntimeCapabilities;
    type UpgradeSchedule = RuntimeUpgradeSchedule;
    type Preimages = RuntimePreimages;
    type ReleaseChannel = RuntimeReleaseChannel;
    type RatifyOrigin = pallet_origins::EnsureConstitutionalValues;
    type Dispatcher = RuntimeBatchDispatcher;
    type MaxRuntimeCodeBytes = ConstU32<{ pallet_preimage::MAX_SIZE }>;
    type WeightInfo = pallet_execution_guard::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct RuntimeBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_constitution::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    fn origin(authority: pallet_constitution::ConstitutionOrigin) -> RuntimeOrigin {
        match authority {
            pallet_constitution::ConstitutionOrigin::FutarchyParam => {
                pallet_origins::Origin::FutarchyParam.into()
            }
            pallet_constitution::ConstitutionOrigin::FutarchyTreasury => {
                pallet_origins::Origin::FutarchyTreasury.into()
            }
            pallet_constitution::ConstitutionOrigin::FutarchyCode => {
                pallet_origins::Origin::FutarchyCode.into()
            }
            pallet_constitution::ConstitutionOrigin::FutarchyMeta => {
                pallet_origins::Origin::FutarchyMeta.into()
            }
            pallet_constitution::ConstitutionOrigin::ConstitutionalValues => {
                pallet_origins::Origin::ConstitutionalValues.into()
            }
            pallet_constitution::ConstitutionOrigin::GuardianHold => {
                pallet_origins::Origin::GuardianHold.into()
            }
            pallet_constitution::ConstitutionOrigin::EmergencyPlaybook => {
                pallet_origins::Origin::EmergencyPlaybook.into()
            }
            pallet_constitution::ConstitutionOrigin::Root => RuntimeOrigin::root(),
            pallet_constitution::ConstitutionOrigin::Signed => {
                RuntimeOrigin::signed(AccountId32::new([240; 32]))
            }
        }
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_welfare::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    fn metric_governance_origin() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_oracle::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    fn adjudication_origin() -> RuntimeOrigin {
        pallet_origins::Origin::OracleResolution.into()
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_registry::BenchmarkHelper<RuntimeOrigin, AccountId> for RuntimeBenchmarkHelper {
    fn resolution_origin() -> RuntimeOrigin {
        pallet_origins::Origin::OracleResolution.into()
    }
    fn funded_account(seed: u8) -> AccountId {
        AccountId32::new([seed; 32])
    }
    fn register_watchtower(_: &AccountId) {}
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_futarchy_treasury::BenchmarkHelper<RuntimeOrigin, AccountId>
    for RuntimeBenchmarkHelper
{
    fn treasury_origin() -> RuntimeOrigin {
        pallet_origins::Origin::FutarchyTreasury.into()
    }
    fn account(seed: u8) -> AccountId {
        AccountId32::new([seed; 32])
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_guardian::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    fn signed(who: [u8; 32]) -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::new(who))
    }
    fn values() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
    fn prime_for_worst_case() {}
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_attestor::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    fn signed(who: [u8; 32]) -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::new(who))
    }
    fn values() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
    fn ratify() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_epoch::BenchmarkHelper<RuntimeOrigin, AccountId> for RuntimeBenchmarkHelper {
    fn constitutional_values_origin() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }

    fn guardian_origin() -> RuntimeOrigin {
        pallet_origins::Origin::GuardianHold.into()
    }

    fn execution_guard_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(execution_guard_account())
    }

    fn void_authority_origin() -> RuntimeOrigin {
        pallet_origins::Origin::EmergencyPlaybook.into()
    }

    fn account(seed: u8) -> AccountId {
        AccountId32::new([seed; 32])
    }

    fn proposal(
        id: futarchy_primitives::ProposalId,
        who: AccountId,
        now: BlockNumber,
        epoch: EpochId,
    ) -> futarchy_primitives::Proposal<AccountId> {
        futarchy_primitives::Proposal {
            id,
            proposer: who,
            class: futarchy_primitives::ProposalClass::Param,
            state: futarchy_primitives::ProposalState::Submitted,
            epoch,
            submitted_at: now,
            payload_hash: sp_io::hashing::blake2_256(&id.encode()),
            payload_len: 0,
            ask: 0,
            bond: balance_param(b"prop.bond.param"),
            resources: Default::default(),
            metric_spec: 0,
            decide_at: 0,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: None,
            maturity: None,
            grace_end: None,
            version_constraint: pallet_execution_guard::CurrentSpecName::<Runtime>::get(),
            decision: None,
        }
    }

    fn prime_decision(
        pid: futarchy_primitives::ProposalId,
        epoch: EpochId,
        gates: bool,
    ) -> futarchy_primitives::MarketSet {
        // B5 owns executable assembled-runtime benchmark fixtures. Returning a
        // deterministic bounded set is honest compile-time setup; no market
        // storage or grade is fabricated here, so an execution attempt fails
        // closed until B5 funds and observes real books.
        let base = pid.saturating_mul(8);
        futarchy_primitives::MarketSet {
            accept: base.saturating_add(1),
            reject: base.saturating_add(2),
            gates: gates.then_some([
                base.saturating_add(3),
                base.saturating_add(4),
                base.saturating_add(5),
                base.saturating_add(6),
            ]),
            baseline: u64::from(epoch).saturating_add(1),
        }
    }

    fn prime_settlement(_: EpochId) {
        // B5 supplies real welfare snapshots/oracle close-out. Leaving them
        // absent is the fail-closed benchmark fixture for this wiring milestone.
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_epoch_proposal(
    pid: futarchy_primitives::ProposalId,
    payload_hash: futarchy_primitives::H256,
    payload_len: u32,
    state: futarchy_primitives::ProposalState,
) -> futarchy_primitives::Proposal<AccountId> {
    futarchy_primitives::Proposal {
        id: pid,
        proposer: AccountId32::new([u8::try_from(pid).map_or(0, |value| value); 32]),
        class: futarchy_primitives::ProposalClass::Param,
        state,
        epoch: pallet_epoch::CurrentEpoch::<Runtime>::get(),
        submitted_at: System::block_number(),
        payload_hash,
        payload_len,
        ask: 0,
        bond: balance_param(b"prop.bond.param"),
        resources: Default::default(),
        metric_spec: 0,
        decide_at: System::block_number(),
        rerun: false,
        extended: false,
        delayed_once: false,
        markets: Some(futarchy_primitives::MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 3,
        }),
        maturity: None,
        grace_end: None,
        version_constraint: pallet_execution_guard::CurrentSpecName::<Runtime>::get(),
        decision: None,
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_seed_epoch_queue(
    pid: futarchy_primitives::ProposalId,
    payload_hash: futarchy_primitives::H256,
    payload_len: u32,
    maturity: BlockNumber,
    grace_end: BlockNumber,
    version_constraint: futarchy_primitives::RuntimeVersionConstraint,
) -> DispatchResult {
    let mut proposal = benchmark_epoch_proposal(
        pid,
        payload_hash,
        payload_len,
        futarchy_primitives::ProposalState::Queued,
    );
    proposal.maturity = Some(maturity);
    proposal.grace_end = Some(grace_end);
    proposal.version_constraint = Some(version_constraint);
    proposal.decision = Some(futarchy_primitives::DecisionOutcome::Adopt);
    let epoch = proposal.epoch;
    let decide_at = proposal.decide_at;
    pallet_epoch::Proposals::<Runtime>::insert(pid, proposal);
    let schedule = pallet_epoch::Schedule::<Runtime>::get();
    pallet_epoch::ProposalSchedules::<Runtime>::insert(
        pid,
        pallet_epoch::ProposalSchedule {
            epoch,
            epoch_start_block: schedule.epoch_start_block,
            epoch_length: schedule.length,
            decide_at,
            metric_spec: 0,
        },
    );
    pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
        *next = (*next).max(pid.saturating_add(1));
    });
    if !pallet_conditional_ledger::Vaults::<Runtime>::contains_key(pid) {
        ConditionalLedger::create_vault(epoch_signed_origin(), pid, 0)?;
    }
    Ok(())
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_guard_enqueue(
    pid: futarchy_primitives::ProposalId,
    call: RuntimeCall,
    domain: pallet_execution_guard::CallDomain,
) -> Result<BlockNumber, DispatchError> {
    use frame_support::traits::StorePreimage;

    let batch =
        pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(alloc::vec![call])
            .map_err(|_| DispatchError::Other("benchmark guard batch bound"))?;
    let bytes = batch.encode();
    let payload_len = u32::try_from(bytes.len())
        .map_err(|_| DispatchError::Other("benchmark guard payload length"))?;
    let hash = <Preimage as StorePreimage>::note(Cow::Owned(bytes))?;
    <Preimage as QueryPreimage>::request(&hash);

    let now = System::block_number();
    let maturity = now
        .checked_add(
            <ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                futarchy_primitives::ProposalClass::Param,
            ),
        )
        .ok_or(DispatchError::Arithmetic(
            sp_runtime::ArithmeticError::Overflow,
        ))?;
    let grace_end = maturity
        .checked_add(
            <ExecutionParams as pallet_execution_guard::Params>::exec_grace(
                futarchy_primitives::ProposalClass::Param,
            ),
        )
        .ok_or(DispatchError::Arithmetic(
            sp_runtime::ArithmeticError::Overflow,
        ))?;
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
        .ok_or(DispatchError::Other("benchmark guard current version"))?;
    let declared_domains =
        pallet_execution_guard::pallet::StoredDomains::try_from(alloc::vec![domain])
            .map_err(|_| DispatchError::Other("benchmark guard domain bound"))?;
    benchmark_seed_epoch_queue(
        pid,
        hash.0,
        payload_len,
        maturity,
        grace_end,
        version_constraint.clone(),
    )?;
    crate::ExecutionGuard::enqueue(
        RuntimeOrigin::signed(epoch_account()),
        pallet_execution_guard::pallet::StoredQueuedExecution {
            pid,
            payload_hash: hash.0,
            payload_len,
            class: futarchy_primitives::ProposalClass::Param,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains,
            failed_at: None,
        },
        false,
    )?;
    Ok(maturity)
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_execution_guard::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    fn ratify_origin() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }

    fn prime_ratify(pid: futarchy_primitives::ProposalId, _: u32) {
        let payload_hash = sp_io::hashing::blake2_256(&pid.encode());
        let proposal = benchmark_epoch_proposal(
            pid,
            payload_hash,
            0,
            futarchy_primitives::ProposalState::Submitted,
        );
        pallet_epoch::IntakeProposals::<Runtime>::insert(pid, proposal);
    }

    fn prime_execute(pid: futarchy_primitives::ProposalId) {
        let key = pallet_constitution::key16(b"mkt.obs_interval");
        let Some(mut record) = pallet_constitution::Params::<Runtime>::get(key) else {
            return;
        };
        record.cooldown_epochs = 0;
        pallet_constitution::Params::<Runtime>::insert(key, record);
        let call = RuntimeCall::Constitution(pallet_constitution::Call::set_param {
            key,
            value: record.value,
        });
        if let Ok(maturity) =
            benchmark_guard_enqueue(pid, call, pallet_execution_guard::CallDomain::Param)
        {
            System::set_block_number(maturity);
        }
    }

    fn prime_failed(pid: futarchy_primitives::ProposalId) {
        let call = RuntimeCall::System(frame_system::Call::remark {
            remark: b"guard-benchmark-failure".to_vec(),
        });
        if let Ok(maturity) =
            benchmark_guard_enqueue(pid, call, pallet_execution_guard::CallDomain::Public)
        {
            System::set_block_number(maturity);
            let caller = AccountId::new([241; 32]);
            let _ = crate::ExecutionGuard::execute(RuntimeOrigin::signed(caller), pid);
            if let Some(failed_at) = pallet_execution_guard::pallet::Queue::<Runtime>::get(pid)
                .and_then(|queued| queued.failed_at)
            {
                System::set_block_number(
                    failed_at
                        .saturating_add(pallet_execution_guard::RETRY_WINDOW)
                        .saturating_add(1),
                );
            }
        }
    }

    fn prime_pending_upgrade(code: &[u8]) {
        let hash = sp_io::hashing::blake2_256(code);
        let now = System::block_number();
        let _ = System::authorize_upgrade(RuntimeOrigin::root(), Hash::from(hash));
        pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
            pallet_execution_guard::PendingUpgrade {
                hash,
                authorized_at: now,
                applicable_at: now,
                target_spec_version: VERSION.spec_version.saturating_add(1),
            },
        );
    }

    fn prime_stale(pid: futarchy_primitives::ProposalId) {
        let key = pallet_constitution::key16(b"mkt.obs_interval");
        let Some(record) = pallet_constitution::Params::<Runtime>::get(key) else {
            return;
        };
        let call = RuntimeCall::Constitution(pallet_constitution::Call::set_param {
            key,
            value: record.value,
        });
        if benchmark_guard_enqueue(pid, call, pallet_execution_guard::CallDomain::Param).is_ok() {
            pallet_execution_guard::CurrentSpecName::<Runtime>::mutate(|current| {
                if let Some(version) = current {
                    version.spec_version = version.spec_version.saturating_add(1);
                }
            });
        }
    }
}
