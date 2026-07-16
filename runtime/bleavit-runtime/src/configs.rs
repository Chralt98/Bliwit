//! Runtime configuration and the B1a fail-closed cross-pallet adapters.

use alloc::{borrow::Cow, vec::Vec};

use frame_support::{
    derive_impl,
    dispatch::DispatchClass,
    parameter_types,
    traits::{
        ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, Contains, EqualPrivilegeOnly,
        InstanceFilter, Nothing, TransformOrigin, VariantCountOf,
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
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
#[cfg(feature = "runtime-benchmarks")]
use sp_runtime::AccountId32;
use sp_runtime::{
    traits::{AccountIdConversion, AccountIdLookup},
    Perbill,
};

use crate::{
    AccountId, AssetId, Aura, Balance, Balances, Block, CollatorSelection, ConditionalLedger,
    ConsensusHook, ForeignAssets, Hash, MessageQueue, Migrations, Nonce, PalletInfo,
    ParachainSystem, PolkadotXcm, Preimage, Referenda, Runtime, RuntimeCall, RuntimeEvent,
    RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin, RuntimeTask, Scheduler, Session,
    SessionKeys, System, XcmpQueue, USDC_ASSET_ID, VERSION,
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

/// A8 owns user scheduling authority. B1a deliberately admits no external
/// scheduler origin; referenda use the scheduler's internal trait API.
pub struct PendingSchedulerOrigin;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for PendingSchedulerOrigin {
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
    type ScheduleOrigin = PendingSchedulerOrigin;
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
    pub MigrationMaxServiceWeight: Weight = Perbill::from_percent(50) * RuntimeBlockWeights::get().max_block;
}
#[derive_impl(pallet_migrations::config_preludes::TestDefaultConfig)]
impl pallet_migrations::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type Migrations = ();
    #[cfg(feature = "runtime-benchmarks")]
    type Migrations = pallet_migrations::mock_helpers::MockedMigrations;
    type MaxServiceWeight = MigrationMaxServiceWeight;
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
    type OnSystemEvent = ();
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

/// Shared fail-closed A8 epoch seam. Epoch zero is the reserved pre-genesis
/// sentinel; live epochs begin at one when A8 lands.
pub struct PendingEpochClock;
impl frame_support::traits::Get<EpochId> for PendingEpochClock {
    fn get() -> EpochId {
        0
    }
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
    type CurrentEpoch = PendingEpochClock;
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
fn balance_param(name: &[u8]) -> Balance {
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
    pub const IncidentPalletId: PalletId = PalletId(*b"bl/reg/i");
    pub const MilestonePalletId: PalletId = PalletId(*b"bl/reg/m");
}
pub fn market_account() -> AccountId {
    MarketPalletId::get().into_account_truncating()
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
pub fn fee_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"FEES____")
}
pub fn treasury_protocol_account() -> AccountId {
    LedgerPalletId::get().into_sub_account_truncating(*b"TREASRY_")
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
/// A8 pending authority: no real origin is admitted before the epoch pallet
/// lands. Production (`try_origin`) always rejects — the G-1 fail-closed
/// direction for the `ResolveAuthority`/`MarketAdmin` seams that A8 will own.
///
/// Under `runtime-benchmarks` ONLY, a single sentinel account is accepted so
/// B5's benchmarks for the A8-authority-gated calls can construct a passing
/// origin (`try_successful_origin` returns exactly this account). This is gated
/// out of production, so the fail-closed guarantee is unaffected.
pub struct PendingA8Authority;
#[cfg(feature = "runtime-benchmarks")]
const A8_BENCH_ACCOUNT: [u8; 32] = [241; 32];
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for PendingA8Authority {
    type Success = ();
    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let raw: Result<frame_system::RawOrigin<AccountId>, RuntimeOrigin> =
                origin.clone().into();
            if matches!(raw, Ok(frame_system::RawOrigin::Signed(who)) if who == AccountId32::new(A8_BENCH_ACCOUNT))
            {
                return Ok(());
            }
        }
        Err(origin)
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(AccountId32::new(A8_BENCH_ACCOUNT)))
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
            fee_account(),
            treasury_protocol_account(),
            insurance_account(),
            IncidentPalletId::get().into_account_truncating(),
            MilestonePalletId::get().into_account_truncating(),
            welfare_settlement_account(),
        ];
        accounts.contains(who)
    }
}
parameter_types! { pub InsuranceAccount: AccountId = insurance_account(); }
impl pallet_conditional_ledger::Config for Runtime {
    type Collateral = ForeignAssets;
    type UsdcAssetId = UsdcAssetId;
    type MarketAuthority = EnsureMarketAccount;
    type ResolveAuthority = PendingA8Authority;
    type SettleAuthority = EnsureWelfareAccount;
    type MinSplit = LedgerMinSplit;
    type PositionDeposit = LedgerPositionDeposit;
    type MaxPositionsPerAccount = ConstU32<{ bounds::MAX_ACCOUNT_POSITIONS }>;
    type ArchiveDelay = LedgerArchiveDelay;
    type ReapBatch = ConstU32<{ kernel::REAP_BATCH }>;
    type ProtocolAccounts = ProtocolAccounts;
    type InsuranceAccount = InsuranceAccount;
    type PalletId = LedgerPalletId;
    type WeightInfo = pallet_conditional_ledger::weights::SubstrateWeight<Runtime>;
}
impl pallet_market::Config for Runtime {
    type WeightInfo = pallet_market::weights::SubstrateWeight<Runtime>;
    type Fee = MarketFee;
    type ObsInterval = MarketObsInterval;
    type Kappa1e9 = MarketKappa;
    type MarketAdmin = PendingA8Authority;
    type ArchiveDelay = LedgerArchiveDelay;
    type PalletId = MarketPalletId;
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
/// A8 pending inputs deliberately return empty vectors, causing snapshot cranks
/// to reject instead of persisting a metric derived from absent inputs.
pub struct PendingMetricInputs;
impl pallet_welfare::MetricInputs for PendingMetricInputs {
    fn onchain_components(_: EpochId, _: u16) -> Vec<pallet_welfare::ComponentValue> {
        Vec::new()
    }
    fn incident_multiplier(epoch: EpochId) -> FixedU64 {
        // The IncidentRegistry aggregate IS the C_attested multiplier
        // (registry-core: an empty closed epoch records exactly 1.0). An
        // absent entry means the epoch is not closed yet; the neutral 1.0 is
        // returned because this pending seam is unreachable while
        // `onchain_components` is empty (snapshot cranks reject first) — A8's
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
    type MetricInputs = PendingMetricInputs;
    type Ledger = WelfareLedger;
    type CurrentEpoch = PendingEpochClock;
    type WeightInfo = pallet_welfare::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

/// A8 pending reporting seam: welfare supplies known spec metadata, while a
/// zero window and stake keep new reports fail-closed until epoch timing lands.
pub struct PendingReporting;
impl pallet_oracle::ReportingContext for PendingReporting {
    fn report_window_end(_: EpochId) -> u32 {
        0
    }
    fn is_expected_spec_version(component: u16, epoch: EpochId, version: u16) -> bool {
        match pallet_welfare::MetricSpecs::<Runtime>::get(version) {
            Some(specs) => specs
                .iter()
                .any(|spec| spec.id == component && spec.activation_epoch <= epoch),
            None => false,
        }
    }
    fn stake_at_risk(_: u16, _: EpochId) -> Balance {
        0
    }
    fn expected_components(epoch: EpochId) -> Vec<(u16, u16)> {
        pallet_welfare::MetricSpecs::<Runtime>::iter()
            .flat_map(|(version, specs)| {
                specs.into_iter().filter_map(move |spec| {
                    (spec.activation_epoch <= epoch).then_some((spec.id, version))
                })
            })
            .collect()
    }
}
impl pallet_oracle::Config for Runtime {
    type AdjudicationOrigin = pallet_origins::EnsureOracleResolution;
    type Reporting = PendingReporting;
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
/// aggregate remains in registry storage and is pulled by `PendingMetricInputs`.
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
/// A8 pending registry epoch seam: known welfare spec data is readable, but
/// the zero filing window and target authorize no new epoch filing activity.
pub struct PendingRegistryEpoch;
impl pallet_registry::EpochContext for PendingRegistryEpoch {
    fn filing_window_end(_: EpochId) -> u32 {
        0
    }
    fn frozen_spec_version(epoch: EpochId) -> u16 {
        pallet_welfare::MetricSpecs::<Runtime>::iter_keys()
            .filter(
                |version| match pallet_welfare::MetricSpecs::<Runtime>::get(version) {
                    Some(specs) => specs.iter().any(|spec| spec.activation_epoch <= epoch),
                    None => false,
                },
            )
            .max()
            .map_or(0, |version| version)
    }
    fn milestone_target(_: EpochId) -> u32 {
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
            type Epoch = PendingRegistryEpoch;
            type ResolutionAuthority = pallet_origins::EnsureOracleResolution;
            type InsuranceAccount = InsuranceAccount;
            type PalletId = $id;
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
}
impl pallet_futarchy_treasury::Config for Runtime {
    type TreasuryOrigin = pallet_origins::EnsureFutarchyTreasury;
    type Params = TreasuryParams;
    type CurrentEpoch = PendingEpochClock;
    type WeightInfo = pallet_futarchy_treasury::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

/// A8 pending guardian proposal-status seam: `Other` authorizes no action.
pub struct PendingGuardianStatus;
impl pallet_guardian::GuardianProposalStatus for PendingGuardianStatus {
    fn status(_: u64) -> (pallet_guardian::ProposalStatus, bool) {
        (pallet_guardian::ProposalStatus::Other, false)
    }
}
/// A8 pending guardian-trigger seam: no trigger is active before epoch lands.
pub struct PendingGuardianTriggers;
impl pallet_guardian::GuardianTriggers for PendingGuardianTriggers {
    fn current() -> pallet_guardian::TriggerState {
        pallet_guardian::TriggerState::none()
    }
}
/// A8/referenda integration pending adapter. Empty guardian membership at B1a
/// genesis means no review can become overdue before the real adapter lands.
pub struct PendingGuardianScheduler;
impl pallet_guardian::GuardianReviewScheduler for PendingGuardianScheduler {
    fn schedule_review(_: u32) -> u32 {
        u32::MAX
    }
}
impl pallet_guardian::GuardianRecallScheduler for PendingGuardianScheduler {
    fn schedule_recall(_: u32) -> u32 {
        u32::MAX
    }
}
impl pallet_guardian::Config for Runtime {
    type ValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    type CurrentEpoch = PendingEpochClock;
    type ProposalStatusProvider = PendingGuardianStatus;
    type TriggerProvider = PendingGuardianTriggers;
    type ReviewScheduler = PendingGuardianScheduler;
    type RecallScheduler = PendingGuardianScheduler;
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
