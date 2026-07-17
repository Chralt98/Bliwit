//! Runtime configuration and the B1a fail-closed cross-pallet adapters.

#[cfg(feature = "runtime-benchmarks")]
use alloc::vec;
use alloc::{borrow::Cow, vec::Vec};

use frame_support::{
    derive_impl,
    dispatch::{DispatchClass, DispatchResult},
    parameter_types,
    traits::{
        fungibles::{Inspect, Mutate},
        tokens::Preservation,
        ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, Contains, EqualPrivilegeOnly,
        InstanceFilter, Nothing, QueryPreimage, StorageInstance, TransformOrigin, VariantCountOf,
        WithdrawReasons,
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
use futarchy_primitives::{
    bounds, chain_identity, currency, kernel, EpochId, FixedU64, ParamKey, ProposalClass,
    ProposalId, ProposalState, RuntimeVersionConstraint, H256,
};
#[cfg(feature = "runtime-benchmarks")]
use futarchy_primitives::{keeper::CrankClass, EpochPhase};
use parity_scale_codec::{Decode, Encode};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
#[cfg(feature = "runtime-benchmarks")]
use sp_runtime::AccountId32;
use sp_runtime::{
    traits::{AccountIdConversion, AccountIdLookup},
    DispatchError, Perbill,
};

#[cfg(feature = "runtime-benchmarks")]
use crate::Welfare;
use crate::{
    AccountId, AssetId, Aura, Balance, Balances, Block, BlockNumber, CollatorSelection,
    ConditionalLedger, ConsensusHook, Epoch, ExecutionGuard, ForeignAssets, FutarchyTreasury, Hash,
    MessageQueue, Migrations, Nonce, PalletInfo, ParachainSystem, PolkadotXcm, Preimage, Referenda,
    Runtime, RuntimeCall, RuntimeEvent, RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin,
    RuntimeTask, Scheduler, Session, SessionKeys, System, XcmpQueue, USDC_ASSET_ID, VERSION,
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
    type SystemWeightInfo = crate::weights::frame_system::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::pallet_timestamp::WeightInfo<Runtime>;
}

impl pallet_balances::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Balance = Balance;
    type DustRemoval = ();
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
    type WeightInfo = crate::weights::pallet_balances::WeightInfo<Runtime>;
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
    type ForceOrigin = ForeignAssetsForceOrigin;
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
    type WeightInfo = crate::weights::pallet_assets::WeightInfo<Runtime>;
    type RemoveItemsLimit = ConstU32<1_000>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = AssetBenchmarkHelper;
}

// Production deliberately has no force origin for bridged USDC. The stock
// `pallet-assets` benchmarks use `ForceOrigin` to create their isolated fixture
// asset, so benchmark Wasm follows the Asset Hub convention and admits Root for
// setup only. This cannot alter a production dispatch path because the alias is
// selected at compile time by `runtime-benchmarks`.
#[cfg(feature = "runtime-benchmarks")]
type ForeignAssetsForceOrigin = EnsureRoot<AccountId>;
#[cfg(not(feature = "runtime-benchmarks"))]
type ForeignAssetsForceOrigin = frame_system::EnsureNever<AccountId>;

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
    type WeightInfo = crate::weights::pallet_preimage::WeightInfo<Runtime>;
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
        #[cfg(feature = "runtime-benchmarks")]
        {
            return EnsureRoot::<AccountId>::try_origin(origin);
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        Err(origin)
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        // Stock scheduler benchmarks dispatch Root directly. Production keeps
        // this seam closed; Root exists here only in benchmark Wasm.
        Ok(RuntimeOrigin::root())
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
    type WeightInfo = crate::weights::pallet_scheduler::WeightInfo<Runtime>;
    type OriginPrivilegeCmp = EqualPrivilegeOnly;
    type Preimages = Preimage;
    type BlockNumberProvider = System;
}

impl pallet_utility::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type PalletsOrigin = <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin;
    type WeightInfo = crate::weights::pallet_utility::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::pallet_proxy::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::pallet_multisig::WeightInfo<Runtime>;
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

/// PB-MIGRATION remediation support (09 §7): retire the MBM cursor only when
/// it is genuinely disposable — actually stuck, or an active cursor whose
/// stall is still live and backed by the migration failure/stall halt sources.
pub(crate) fn retire_stuck_migration_cursor_for_remediation() -> bool {
    let retire = match pallet_migrations::Cursor::<Runtime>::get() {
        Some(pallet_migrations::MigrationCursor::Stuck) => true,
        Some(pallet_migrations::MigrationCursor::Active(ref cursor)) => {
            MigrationHaltSources::get() & (MIGRATION_FAILURE_HALT | MIGRATION_STALL_HALT) != 0
                && active_migration_stall_is_live(cursor)
        }
        None => false,
    };
    if retire {
        pallet_migrations::Cursor::<Runtime>::kill();
        MigrationProgressMarker::kill();
    }
    retire
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
    type WeightInfo = crate::weights::pallet_migrations::WeightInfo<Runtime>;
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = crate::weights::pallet_sudo::WeightInfo<Runtime>;
}

parameter_types! {
    pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const RelayOrigin: cumulus_primitives_core::AggregateMessageOrigin = cumulus_primitives_core::AggregateMessageOrigin::Parent;
}
impl cumulus_pallet_parachain_system::Config for Runtime {
    type WeightInfo = crate::weights::cumulus_pallet_parachain_system::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::pallet_message_queue::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::cumulus_pallet_xcmp_queue::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::pallet_session::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::pallet_collator_selection::WeightInfo<Runtime>;
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
        #[cfg(feature = "runtime-benchmarks")]
        {
            // Upstream `pallet-referenda` benchmarks submit a proposal whose
            // enactment origin is Root. Map that fixture origin onto the
            // existing strongest values track in benchmark Wasm only; no
            // production track or origin mapping is added.
            let system: Result<frame_system::RawOrigin<AccountId>, _> = origin.clone().try_into();
            if matches!(system, Ok(frame_system::RawOrigin::Root)) {
                return Ok(0);
            }
        }
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
    pub const AlarmInterval: u32 = 10;
    pub const MaxTurnout: Balance = currency::VIT_TOTAL_SUPPLY;
    pub const VoteLockingPeriod: u32 = 32 * BLOCKS_PER_WEEK;
}
#[cfg(not(feature = "runtime-benchmarks"))]
parameter_types! {
    pub const UndecidingTimeout: u32 = 7 * BLOCKS_PER_DAY;
}
#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
    // The upstream `nudge_referendum_no_deposit` fixture advances through the
    // full prepare period before measuring the no-deposit branch. Production's
    // equal 7-day timeout makes that synthetic referendum terminal at the same
    // block, so benchmark Wasm gives the fixture one additional prepare period.
    pub const UndecidingTimeout: u32 = 14 * BLOCKS_PER_DAY;
}
impl pallet_referenda::Config for Runtime {
    type RuntimeCall = RuntimeCall;
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = crate::weights::pallet_referenda::WeightInfo<Runtime>;
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
    type WeightInfo = crate::weights::pallet_conviction_voting::WeightInfo<Runtime>;
    type Currency = Balances;
    type Polls = Referenda;
    type MaxTurnout = MaxTurnout;
    type MaxVotes = ConstU32<512>;
    type VoteLockingPeriod = VoteLockingPeriod;
    type BlockNumberProvider = System;
    type VotingHooks = ();
}

impl pallet_origins::Config for Runtime {
    type WeightInfo = crate::weights::pallet_origins::WeightInfo<Runtime>;
}

type LiveEpochClock = pallet_epoch::CurrentEpoch<Runtime>;

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
    type CurrentEpoch = LiveEpochClock;
    type WeightInfo = crate::weights::pallet_constitution::WeightInfo<Runtime>;
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
    pub const TreasuryPalletId: PalletId = PalletId(*b"bl/trsry");
    pub const IncidentPalletId: PalletId = PalletId(*b"bl/reg/i");
    pub const MilestonePalletId: PalletId = PalletId(*b"bl/reg/m");
    pub const EpochPalletId: PalletId = PalletId(*b"bl/epoch");
    pub const ExecutionGuardPalletId: PalletId = PalletId(*b"bl/guard");
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
pub fn epoch_account() -> AccountId {
    EpochPalletId::get().into_account_truncating()
}
/// B6's runtime tests drive the guard's `enqueue` through the configured
/// `EnqueueAuthority`; on this runtime that is the epoch sovereign (I-9).
#[cfg(test)]
pub(crate) fn test_execution_enqueue_origin() -> RuntimeOrigin {
    RuntimeOrigin::signed(epoch_account())
}

pub fn execution_guard_account() -> AccountId {
    ExecutionGuardPalletId::get().into_account_truncating()
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
    type Success = ();
    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        match EnsureSigned::<AccountId>::try_origin(origin.clone()) {
            Ok(who) if who == epoch_account() => Ok(()),
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
    type Success = ();
    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        match EnsureSigned::<AccountId>::try_origin(origin.clone()) {
            Ok(who) if who == execution_guard_account() => Ok(()),
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
    type WeightInfo = crate::weights::pallet_conditional_ledger::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}
impl pallet_market::Config for Runtime {
    type WeightInfo = crate::weights::pallet_market::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
    type Fee = MarketFee;
    type ObsInterval = MarketObsInterval;
    type Kappa1e9 = MarketKappa;
    type MarketAdmin = EnsureEpochAccount;
    type ArchiveDelay = LedgerArchiveDelay;
    type PalletId = MarketPalletId;
    type KeeperRebate = FutarchyTreasury;
    // Pending A8 decision-window lookup: classify unknown windows as General,
    // preserving the reserved tranche until the real adapter lands.
    type InDecisionWindow = Nothing;
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
/// Runtime metric projection. Final oracle components and the incident
/// multiplier are live; normalized on-chain/relay counters and daily inputs do
/// not yet have a production source, so those entries remain absent and the
/// welfare pallet rejects an incomplete snapshot (G-1).
pub struct RuntimeMetricInputs;
impl pallet_welfare::MetricInputs for RuntimeMetricInputs {
    fn onchain_components(epoch: EpochId, version: u16) -> Vec<pallet_welfare::ComponentValue> {
        let Some(specs) = pallet_welfare::MetricSpecs::<Runtime>::get(version) else {
            return Vec::new();
        };
        #[cfg(feature = "runtime-benchmarks")]
        {
            // The production on-chain counter projection is intentionally
            // fail-closed until B-track sources exist. Benchmark Wasm injects
            // a complete, max-bound component set so the crank measures the
            // full admissible persistence path rather than an early error.
            return specs
                .iter()
                .filter(|spec| spec.activation_epoch <= epoch)
                .map(|spec| pallet_welfare::ComponentValue {
                    id: spec.id,
                    value: FixedU64(1_000_000_000),
                })
                .collect();
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        specs
            .iter()
            .filter(|spec| {
                spec.activation_epoch <= epoch
                    && matches!(spec.source, pallet_welfare::SourceClass::Attested)
            })
            .filter_map(|spec| {
                pallet_oracle::Pallet::<Runtime>::settled_component(spec.id, epoch, version).map(
                    |settled| pallet_welfare::ComponentValue {
                        id: spec.id,
                        value: settled.value,
                    },
                )
            })
            .collect()
    }
    fn incident_multiplier(epoch: EpochId) -> FixedU64 {
        // The IncidentRegistry aggregate IS the C_attested multiplier
        // (registry-core: an empty closed epoch records exactly 1.0). An
        // absent entry means the epoch is not closed yet; the neutral 1.0 is
        // returned because an incomplete component set still makes the
        // snapshot fail. Returning zero would erase C_attested and would be
        // fail-destructive rather than fail-safe.
        match pallet_registry::Aggregates::<Runtime>::get(epoch) {
            Some(value) => value,
            None => FixedU64(1_000_000_000),
        }
    }
    fn daily_components(
        epoch: EpochId,
        _: u8,
        version: u16,
    ) -> Vec<pallet_welfare::ComponentValue> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            return pallet_welfare::MetricSpecs::<Runtime>::get(version)
                .into_iter()
                .flatten()
                .filter(|spec| spec.activation_epoch <= epoch)
                .map(|spec| pallet_welfare::ComponentValue {
                    id: spec.id,
                    value: FixedU64(1_000_000_000),
                })
                .collect();
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            let _ = (epoch, version);
            Vec::new()
        }
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
    type CurrentEpoch = LiveEpochClock;
    type KeeperRebate = FutarchyTreasury;
    type WeightInfo = crate::weights::pallet_welfare::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

fn epoch_end(epoch: EpochId) -> Option<u32> {
    let current = pallet_epoch::EpochOf::<Runtime>::get();
    let schedule = pallet_epoch::Schedule::<Runtime>::get();
    if current.index == epoch {
        return schedule.epoch_start_block.checked_add(schedule.length);
    }
    // There is no epoch-indexed, bounded historical schedule source. Scanning
    // retained proposal schedules would make this signed path unbounded, so a
    // past/future epoch has no admissible reporting window (SQ-141).
    None
}

fn frozen_versions_for_measurement(epoch: EpochId) -> Vec<u16> {
    let mut versions = Vec::new();
    // A cohort can measure for at most the kernel-bounded live-cohort horizon.
    // Direct gets keep this projection bounded even if historical schedules
    // accumulate before their owning pallet gains a reap path (SQ-92).
    for distance in 1..=futarchy_primitives::bounds::MAX_NON_TERMINAL_COHORTS {
        let Some(cohort_epoch) = epoch.checked_sub(distance) else {
            continue;
        };
        if let Some(schedule) = pallet_epoch::CohortSchedules::<Runtime>::get(cohort_epoch) {
            if epoch > schedule.measurement_until {
                continue;
            }
            for (_, version) in schedule.specs {
                if !versions.contains(&version) {
                    versions.push(version);
                }
            }
        }
    }
    versions
}

pub struct RuntimeReporting;
impl pallet_oracle::ReportingContext for RuntimeReporting {
    #[allow(clippy::manual_unwrap_or, clippy::manual_unwrap_or_default)]
    fn report_window_end(epoch: EpochId) -> u32 {
        match epoch_end(epoch).and_then(|end| end.checked_add(kernel::ORC_REPORT_WINDOW_BLOCKS)) {
            Some(end) => end,
            None => 0,
        }
    }
    fn is_expected_spec_version(component: u16, epoch: EpochId, version: u16) -> bool {
        #[cfg(feature = "runtime-benchmarks")]
        {
            return pallet_welfare::MetricSpecs::<Runtime>::get(version).is_some_and(|specs| {
                specs
                    .iter()
                    .any(|spec| spec.id == component && spec.activation_epoch <= epoch)
            });
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            let _ = (component, epoch, version);
            // The oracle core accepts StakeAtRisk=0 with a zero bond. Until a
            // cohort-escrow custody projection exists, reject report admission
            // outright rather than opening an economically unbonded game (G-1).
            false
        }
    }
    fn stake_at_risk(_: u16, _: EpochId) -> Balance {
        #[cfg(feature = "runtime-benchmarks")]
        {
            // Exercise the value-scaled bond calculation above its floor.
            return 400_000u128.saturating_mul(currency::USDC);
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        // No production cohort-escrow custody/source exists yet. Zero makes
        // the projection honest; `is_expected_spec_version` separately rejects
        // every report so the core cannot interpret this as a zero bond (G-1).
        0
    }
    fn expected_components(epoch: EpochId) -> Vec<(u16, u16)> {
        frozen_versions_for_measurement(epoch)
            .into_iter()
            .flat_map(|version| {
                pallet_welfare::MetricSpecs::<Runtime>::get(version)
                    .into_iter()
                    .flatten()
                    .filter_map(move |spec| {
                        (spec.activation_epoch <= epoch).then_some((spec.id, version))
                    })
            })
            .collect()
    }
}
impl pallet_oracle::Config for Runtime {
    type AdjudicationOrigin = pallet_origins::EnsureOracleResolution;
    type Reporting = RuntimeReporting;
    // Production remains fail-static until the B4 XCM dispatcher is wired.
    // Benchmark Wasm uses a live no-op sender so the reserve-probe benchmark
    // reaches its documented post-commit rebate path.
    type ProbeDispatch = RuntimeProbeDispatch;
    type KeeperRebate = FutarchyTreasury;
    type MaxRoundCloseBatch = ConstU32<{ kernel::TICK_BATCH }>;
    type WeightInfo = crate::weights::pallet_oracle::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct BenchmarkProbeDispatch;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_oracle::ProbeDispatch for BenchmarkProbeDispatch {
    fn live() -> bool {
        true
    }

    fn probe_due(_: u64) {}
}
#[cfg(feature = "runtime-benchmarks")]
type RuntimeProbeDispatch = BenchmarkProbeDispatch;
#[cfg(not(feature = "runtime-benchmarks"))]
type RuntimeProbeDispatch = ();

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
/// Epoch-backed registry context. Ambiguous concurrent frozen versions and the
/// absent milestone-target field fail closed instead of selecting a value.
pub struct RuntimeRegistryEpoch;
impl pallet_registry::EpochContext for RuntimeRegistryEpoch {
    fn filing_window_end(epoch: EpochId) -> u32 {
        if frozen_versions_for_measurement(epoch).len() == 1 {
            <RuntimeReporting as pallet_oracle::ReportingContext>::report_window_end(epoch)
        } else {
            // `frozen_spec_version` cannot return Option. Close admission as
            // well as returning the sentinel so a caller cannot file a
            // fabricated spec-0 record during an ambiguous/no-version epoch.
            0
        }
    }
    fn frozen_spec_version(epoch: EpochId) -> u16 {
        let versions = frozen_versions_for_measurement(epoch);
        if versions.len() == 1 {
            versions[0]
        } else {
            0
        }
    }
    fn milestone_target(_: EpochId) -> u32 {
        #[cfg(feature = "runtime-benchmarks")]
        {
            // The production source is absent (SQ-141); benchmark Wasm supplies
            // a non-zero frozen target so Milestone aggregation takes its full
            // division/clamp path.
            return registry_core::MILESTONE_TARGET_POINTS;
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
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
            // Both registry instances share the one generated weight file (the
            // benchmarks run per-instance over the same code paths).
            type WeightInfo = crate::weights::pallet_registry::WeightInfo<Runtime>;
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
#[cfg(not(feature = "runtime-benchmarks"))]
pub struct PendingRenewalDispatch;
#[cfg(not(feature = "runtime-benchmarks"))]
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

#[cfg(feature = "runtime-benchmarks")]
pub struct BenchmarkRenewalDispatch;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_futarchy_treasury::RenewalDispatch for BenchmarkRenewalDispatch {
    fn dispatch_renewal(
        _period_index: u32,
        _amount: Balance,
    ) -> frame_support::dispatch::DispatchResult {
        Ok(())
    }
}
#[cfg(feature = "runtime-benchmarks")]
type RuntimeRenewalDispatch = BenchmarkRenewalDispatch;
#[cfg(not(feature = "runtime-benchmarks"))]
type RuntimeRenewalDispatch = PendingRenewalDispatch;

impl pallet_futarchy_treasury::Config for Runtime {
    type TreasuryOrigin = pallet_origins::EnsureFutarchyTreasury;
    type Params = TreasuryParams;
    // The A8 runtime wiring landed: the live epoch clock drives the keeper
    // meter's per-epoch reset (08 §6.3).
    type CurrentEpoch = LiveEpochClock;
    type RenewalDispatch = RuntimeRenewalDispatch;
    type RebatePayout = TreasuryRebatePayout;
    type WeightInfo = crate::weights::pallet_futarchy_treasury::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

pub struct RuntimeGuardianStatus;
impl pallet_guardian::GuardianProposalStatus for RuntimeGuardianStatus {
    fn status(pid: u64) -> (pallet_guardian::ProposalStatus, bool) {
        let proposal = pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid));
        let Some(proposal) = proposal else {
            return (pallet_guardian::ProposalStatus::Other, false);
        };
        let status = match proposal.state {
            futarchy_primitives::ProposalState::Trading => pallet_guardian::ProposalStatus::Trading,
            futarchy_primitives::ProposalState::Extended => {
                pallet_guardian::ProposalStatus::Extended
            }
            // `Suspended` is deliberately NOT mapped to `Queued`: guardian-core
            // gates `DelayOnce` on `status == Queued` (06 §5 — delay-once acts on
            // queued execution only), and a suspended proposal is not queued.
            futarchy_primitives::ProposalState::Queued => pallet_guardian::ProposalStatus::Queued,
            futarchy_primitives::ProposalState::Executed => {
                pallet_guardian::ProposalStatus::Executed
            }
            futarchy_primitives::ProposalState::Rerun => pallet_guardian::ProposalStatus::Rerun,
            _ => pallet_guardian::ProposalStatus::Other,
        };
        (status, proposal.rerun)
    }
}
pub struct RuntimeGuardianTriggers;
impl pallet_guardian::GuardianTriggers for RuntimeGuardianTriggers {
    fn current() -> pallet_guardian::TriggerState {
        #[cfg(feature = "runtime-benchmarks")]
        {
            // Some production trigger feeds are deliberately absent/fail-closed
            // in B1a. The guardian benchmark must nevertheless exercise every
            // admissible playbook branch, so benchmark Wasm supplies the full
            // verified-trigger context through this Config seam.
            return pallet_guardian::TriggerState {
                depeg: true,
                migration_halt: true,
                oracle_deadlock: true,
                gate_breach: true,
                dead_man: true,
                void_in_flight: true,
                reserve_health: true,
                ledger_drift: true,
            };
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            // Current-epoch record only (06 §5 auto-release; PR #66 Codex P1):
            // a breached record retained from a prior epoch must not keep
            // authorizing suspend_on_gate after recovery.
            let gate_breach = crate::classifier::current_epoch_gate_breach();
            let dead_man = pallet_constitution::PhaseFlags::<Runtime>::get()
                & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED
                != 0;
            let current_epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
            let void_in_flight = (0..=futarchy_primitives::bounds::MAX_NON_TERMINAL_COHORTS)
                .filter_map(|distance| current_epoch.checked_sub(distance))
                .filter_map(pallet_epoch::Cohorts::<Runtime>::get)
                .any(|cohort| matches!(cohort.status, pallet_epoch::CohortStatus::Void));
            pallet_guardian::TriggerState {
                // No price/depeg probe, oracle-deadlock classifier or persisted
                // ledger-drift flag exists yet; false prevents fabricated powers.
                depeg: false,
                // B6: the halt trigger is sourced from the aggregated halt-sources word
                // (includes the relay-abort bit; Abort preserves old code and must
                // not freeze the re-proposal lane).
                migration_halt: MigrationHaltSources::get() != 0,
                oracle_deadlock: false,
                gate_breach,
                dead_man,
                void_in_flight,
                reserve_health: pallet_oracle::Pallet::<Runtime>::reserve_unhealthy(),
                ledger_drift: false,
            }
        }
    }
}
/// The guardian scheduler traits are infallible (`u32`) and are called only
/// after the action was persisted. A failed stock-referenda submission could
/// therefore not roll the action back; until the seam becomes fallible, return
/// the explicit sentinel and never claim a referendum was scheduled (G-1).
pub struct PendingGuardianReviewScheduler;
impl pallet_guardian::GuardianReviewScheduler for PendingGuardianReviewScheduler {
    fn schedule_review(_: u32) -> u32 {
        u32::MAX
    }
}
pub struct PendingGuardianRecallScheduler;
impl pallet_guardian::GuardianRecallScheduler for PendingGuardianRecallScheduler {
    fn schedule_recall(_: u32) -> u32 {
        u32::MAX
    }
}
impl pallet_guardian::Config for Runtime {
    type ValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    type CurrentEpoch = LiveEpochClock;
    type ProposalStatusProvider = RuntimeGuardianStatus;
    type TriggerProvider = RuntimeGuardianTriggers;
    type ReviewScheduler = PendingGuardianReviewScheduler;
    type RecallScheduler = PendingGuardianRecallScheduler;
    type WeightInfo = crate::weights::pallet_guardian::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}
impl pallet_attestor::Config for Runtime {
    type ValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    // Ratification shares ConstitutionalValues pending the stock-referenda track split SQ.
    type RatifyOrigin = pallet_origins::EnsureConstitutionalValues;
    type WeightInfo = crate::weights::pallet_attestor::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

// -----------------------------------------------------------------------------
// A8 epoch machine and A11 execution guard

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
                FixedU64(1_000_000_000),
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
            gate_v_min: v_min.map(|value| value / 10),
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
        }
    }
}

/// Market storage supplies live spot and baseline identifiers. The market
/// pallet does not expose decision-window TWAP/coverage/contest telemetry or a
/// canonical market-id allocator, so deployment and decision grading remain
/// explicit fail-closed seams instead of fabricating books or grade inputs.
pub struct RuntimeEpochMarket;
impl pallet_epoch::MarketAccess<AccountId> for RuntimeEpochMarket {
    fn open_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
        _: bool,
        requires_gates: bool,
    ) -> Result<futarchy_primitives::MarketSet, DispatchError> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            // No production canonical market-id allocator or decision telemetry
            // exists yet. This reaches epoch's largest gate-bearing six-book
            // branch with deterministic quotes, but cannot also hydrate the
            // sibling market pallet's observation history. Pallet-market's own
            // benchmarks cover that bounded work.
            if !pallet_conditional_ledger::Vaults::<Runtime>::contains_key(proposal.id) {
                ConditionalLedger::create_vault(
                    RuntimeOrigin::signed(market_account()),
                    proposal.id,
                    proposal.metric_spec,
                )?;
            }
            return Ok(benchmark_market_set(
                proposal.id,
                proposal.epoch,
                requires_gates,
            ));
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            let _ = (proposal, requires_gates);
            Err(DispatchError::Other(
                "epoch market deployment source absent",
            ))
        }
    }
    fn baseline_market(epoch: EpochId) -> Option<u64> {
        #[cfg(feature = "runtime-benchmarks")]
        if let Some(market) = pallet_market::BaselineMarketOf::<Runtime>::get(epoch) {
            return Some(market);
        } else {
            return Some(9_000u64.saturating_add(u64::from(epoch)));
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        pallet_market::BaselineMarketOf::<Runtime>::get(epoch)
    }
    fn twap_full(market: u64) -> Option<FixedU64> {
        #[cfg(feature = "runtime-benchmarks")]
        return Some(benchmark_quote(market));
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            let _ = market;
            None
        }
    }
    fn twap_trailing(market: u64, _: u32) -> Option<FixedU64> {
        #[cfg(feature = "runtime-benchmarks")]
        return Some(benchmark_quote(market));
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            let _ = market;
            None
        }
    }
    fn spot(market: u64) -> Option<FixedU64> {
        #[cfg(feature = "runtime-benchmarks")]
        return Some(benchmark_quote(market));
        #[cfg(not(feature = "runtime-benchmarks"))]
        pallet_market::Markets::<Runtime>::get(market).map(|book| book.last_quote_1e9)
    }
    fn decision_grade(
        _: u64,
        _: pallet_epoch::BookRole,
        _: ProposalClass,
        _: &pallet_epoch::CoreEpochParams,
    ) -> bool {
        #[cfg(feature = "runtime-benchmarks")]
        return true;
        #[cfg(not(feature = "runtime-benchmarks"))]
        false
    }
    fn measured_depth(_: ProposalId) -> Balance {
        #[cfg(feature = "runtime-benchmarks")]
        return 1_000_000u128.saturating_mul(currency::USDC);
        #[cfg(not(feature = "runtime-benchmarks"))]
        0
    }
    fn published_flow_per_day(_: ProposalId) -> Option<Balance> {
        #[cfg(feature = "runtime-benchmarks")]
        return Some(currency::USDC);
        #[cfg(not(feature = "runtime-benchmarks"))]
        None
    }
    fn second_insufficiency(_: ProposalId) -> bool {
        false
    }
    fn previous_settled_baseline_twap(_: EpochId) -> Option<FixedU64> {
        None
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_market_set(
    pid: ProposalId,
    epoch: EpochId,
    gates: bool,
) -> futarchy_primitives::MarketSet {
    let first = pid.saturating_mul(10);
    futarchy_primitives::MarketSet {
        accept: first.saturating_add(1),
        reject: first.saturating_add(2),
        gates: gates.then_some([
            first.saturating_add(3),
            first.saturating_add(4),
            first.saturating_add(5),
            first.saturating_add(6),
        ]),
        baseline: 9_000u64.saturating_add(u64::from(epoch)),
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_quote(market: u64) -> FixedU64 {
    match market % 10 {
        1 => FixedU64(750_000_000),
        2 => FixedU64(250_000_000),
        // Code-proposal safety gates must pass so the epoch benchmark reaches
        // the full attestation and execution-queue path instead of measuring
        // the cheaper gate-veto/cohort path.
        3 | 5 => FixedU64(10_000_000),
        4 | 6 => FixedU64(50_000_000),
        _ => FixedU64(500_000_000),
    }
}

pub struct RuntimeEpochOracle;
impl pallet_epoch::OracleAccess for RuntimeEpochOracle {
    fn any_open_dispute_touching(spec: u16) -> bool {
        pallet_oracle::Rounds::<Runtime>::iter_values()
            .any(|round| round.spec_version == spec && round.challenger.is_some())
    }
}

pub struct RuntimeEpochGuardian;
impl pallet_epoch::GuardianAccess for RuntimeEpochGuardian {
    fn hold_active(pid: ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .is_some_and(|proposal| matches!(proposal.state, ProposalState::Suspended))
    }
    fn dead_man_engaged() -> bool {
        pallet_constitution::PhaseFlags::<Runtime>::get()
            & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED
            != 0
    }
    fn review_window_closed(_: ProposalId) -> bool {
        // Guardian storage does not retain an action-to-proposal binding after
        // dispatch/reap, so this cannot be derived without inventing identity.
        false
    }
}

pub struct RuntimeEpochAttestation;
impl pallet_epoch::AttestationAccess for RuntimeEpochAttestation {
    fn present_and_quorate(pid: ProposalId, payload_hash: H256) -> bool {
        let Some(payload_len) = pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .filter(|proposal| proposal.payload_hash == payload_hash)
            .map(|proposal| proposal.payload_len)
        else {
            return false;
        };
        let artifact_hash = <RuntimeGuardPreimages as pallet_execution_guard::Preimages>::fetch(
            payload_hash,
            payload_len,
        )
        .and_then(|bytes| {
            let mut input = &bytes[..];
            let calls = pallet_execution_guard::RuntimeBatch::<Runtime>::decode(&mut input).ok()?;
            if !input.is_empty() {
                return None;
            }
            match calls.iter().find_map(
                <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
                    RuntimeCall,
                >>::authorize_upgrade_hash,
            ) {
                Some(hash) => Some(hash),
                None => Some(payload_hash),
            }
        });
        artifact_hash.is_some_and(|hash| pallet_attestor::Pallet::<Runtime>::has_quorum(pid, hash))
    }
}

pub struct RuntimeEpochConstitution;
impl pallet_epoch::ConstitutionAccess<AccountId> for RuntimeEpochConstitution {
    fn static_checks_pass(proposal: &futarchy_primitives::Proposal<AccountId>) -> bool {
        let Some(bytes) = <RuntimeGuardPreimages as pallet_execution_guard::Preimages>::fetch(
            proposal.payload_hash,
            proposal.payload_len,
        ) else {
            return false;
        };
        let mut input = &bytes[..];
        let Ok(batch) = pallet_execution_guard::RuntimeBatch::<Runtime>::decode(&mut input) else {
            return false;
        };
        input.is_empty()
            && batch.iter().all(|call| {
                crate::classifier::capability_enabled_for_call(proposal.class, call)
                    && (<crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
                        RuntimeCall,
                    >>::authorize_upgrade_hash(call)
                    .is_some()
                        || <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
                        RuntimeCall,
                    >>::safety_filter(proposal.class, call))
            })
    }
    fn queue_time_check(proposal: &futarchy_primitives::Proposal<AccountId>) -> bool {
        // Re-run the exact payload/class capability projection at decision
        // time. Resource and rate-meter conflicts are independently checked by
        // epoch and the execution guard.
        Self::static_checks_pass(proposal)
    }
    fn in_cap_prize(proposal: &futarchy_primitives::Proposal<AccountId>) -> Option<Balance> {
        #[cfg(feature = "runtime-benchmarks")]
        return Some(proposal.ask);
        #[cfg(not(feature = "runtime-benchmarks"))]
        // Treasury's committed outflow is exact. PARAM/CODE/META require the
        // certified envelope and sec.prize floors whose production source is
        // not yet calibrated; None is the mandated conservative default.
        matches!(proposal.class, ProposalClass::Treasury).then_some(proposal.ask)
    }
    fn ledger_frozen() -> bool {
        pallet_constitution::PhaseFlags::<Runtime>::get()
            & pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN
            != 0
    }
    fn phase_flags() -> u32 {
        pallet_constitution::PhaseFlags::<Runtime>::get()
    }
    #[allow(clippy::manual_unwrap_or, clippy::manual_unwrap_or_default)]
    fn active_metric_spec_version() -> u16 {
        let epoch = Epoch::current_epoch();
        match pallet_welfare::MetricSpecs::<Runtime>::iter()
            .filter(|(_, specs)| specs.iter().any(|spec| spec.activation_epoch <= epoch))
            .map(|(version, _)| version)
            .max()
        {
            Some(version) => version,
            None => 0,
        }
    }
    fn treasury_gate_required(proposal: &futarchy_primitives::Proposal<AccountId>) -> bool {
        matches!(proposal.class, ProposalClass::Code | ProposalClass::Meta)
            || (matches!(proposal.class, ProposalClass::Treasury)
                && proposal.ask > pallet_futarchy_treasury::Pallet::<Runtime>::nav().nav / 100)
    }
}

pub struct RuntimeEpochPreimage;
impl pallet_epoch::PreimageAccess for RuntimeEpochPreimage {
    fn len(hash: H256) -> Option<u32> {
        <RuntimeGuardPreimages as pallet_execution_guard::Preimages>::len(hash)
    }
    fn request(hash: H256) -> frame_support::dispatch::DispatchResult {
        let hash = hash.into();
        if <Preimage as QueryPreimage>::len(&hash).is_none() {
            return Err(DispatchError::Other("epoch qualification preimage absent"));
        }
        <Preimage as QueryPreimage>::request(&hash);
        Ok(())
    }
    fn unrequest(hash: H256) {
        let hash = hash.into();
        if <Preimage as QueryPreimage>::is_requested(&hash) {
            <Preimage as QueryPreimage>::unrequest(&hash);
        }
    }
}

pub struct RuntimeEpochGuard;
impl pallet_epoch::ExecutionGuardAccess for RuntimeEpochGuard {
    fn enqueue(
        pid: ProposalId,
        payload_hash: H256,
        version_constraint: Option<RuntimeVersionConstraint>,
        maturity: u32,
        grace: u32,
        requires_ratification: bool,
    ) -> frame_support::dispatch::DispatchResult {
        let proposal = pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .ok_or(DispatchError::Other("epoch proposal absent"))?;
        if proposal.payload_hash != payload_hash {
            return Err(DispatchError::Other("epoch payload binding mismatch"));
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        if !proposal.resources.is_empty() {
            // No production writer currently projects live machinery/resource
            // conflicts into the guard's BlockedMeters set. Refuse every
            // resource-bearing proposal until that bounded source exists.
            return Err(DispatchError::Other("guard resource source absent"));
        }
        let version_constraint =
            version_constraint.ok_or(DispatchError::Other("epoch version constraint absent"))?;
        let bytes = <RuntimeGuardPreimages as pallet_execution_guard::Preimages>::fetch(
            payload_hash,
            proposal.payload_len,
        )
        .ok_or(DispatchError::Other("epoch preimage absent"))?;
        let mut input = &bytes[..];
        let calls = pallet_execution_guard::RuntimeBatch::<Runtime>::decode(&mut input)
            .map_err(|_| DispatchError::Other("epoch preimage is not runtime batch"))?;
        if !input.is_empty() {
            return Err(DispatchError::Other("epoch preimage has trailing bytes"));
        }
        let mut domains = pallet_execution_guard::StoredDomains::default();
        for call in &calls {
            let derived = <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<RuntimeCall>>::rederive_call(call)?;
            for domain in derived.domains {
                if !domains.contains(&domain) {
                    domains
                        .try_push(domain)
                        .map_err(|_| DispatchError::Other("epoch domain bound exceeded"))?;
                }
            }
        }
        let meters =
            pallet_execution_guard::StoredMeters::try_from(proposal.resources.clone().into_inner())
                .map_err(|_| DispatchError::Other("epoch resource bound exceeded"))?;
        let ratify_ref = pallet_execution_guard::Ratifications::<Runtime>::get(pid)
            .map(|record| record.referendum_index);
        if requires_ratification && ratify_ref.is_none() {
            return Err(DispatchError::Other("epoch ratification absent"));
        }
        let grace_end = maturity
            .checked_add(grace)
            .ok_or(DispatchError::Other("epoch grace-end overflow"))?;
        let artifact = match calls.iter().find_map(
            <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
                RuntimeCall,
            >>::authorize_upgrade_hash,
        ) {
            Some(hash) => hash,
            None => payload_hash,
        };
        let attestation_id = pallet_attestor::Attestations::<Runtime>::get()
            .into_iter()
            .find_map(|attestation| {
                (attestation.pid == pid && attestation.artifact_hash == artifact)
                    .then_some(attestation.id)
            });
        let item = pallet_execution_guard::StoredQueuedExecution {
            pid,
            payload_hash,
            payload_len: proposal.payload_len,
            class: proposal.class,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: meters,
            ratify_ref,
            ratification_passed: false,
            attestation_id,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains: domains,
            failed_at: None,
        };
        frame_support::storage::with_storage_layer(|| {
            <Preimage as QueryPreimage>::request(&payload_hash.into());
            ExecutionGuard::enqueue(RuntimeOrigin::signed(epoch_account()), item, false)
        })
    }
    fn queue_reject_reason(pid: ProposalId) -> Option<futarchy_primitives::RejectReason> {
        ExecutionGuard::queue_reject_reason(pid)
    }
    fn retry_exhausted(pid: ProposalId) -> bool {
        ExecutionGuard::retry_exhausted(pid)
    }
    fn dequeue_terminal(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        ExecutionGuard::dequeue_terminal(pid)
    }
}

pub struct RuntimeEpochWelfare;
impl pallet_epoch::WelfareSettlement for RuntimeEpochWelfare {
    fn compute_settlement(
        cohort_epoch: EpochId,
        spec: u16,
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
        pallet_welfare::Pallet::<Runtime>::compute_settlement(cohort_epoch, spec, target)?;
        pallet_welfare::Snapshots::<Runtime>::get((cohort_epoch, spec))
            .map(|snapshot| snapshot.welfare)
            .ok_or(DispatchError::Other("welfare settlement snapshot absent"))
    }
}

pub struct RuntimeEpochLedger;
impl pallet_epoch::LedgerResolution for RuntimeEpochLedger {
    fn create_vault(pid: ProposalId, spec: u16) -> frame_support::dispatch::DispatchResult {
        if let Some(vault) = pallet_conditional_ledger::Vaults::<Runtime>::get(pid) {
            return if vault.spec == spec {
                Ok(())
            } else {
                Err(DispatchError::Other("epoch vault spec mismatch"))
            };
        }
        ConditionalLedger::create_vault(RuntimeOrigin::signed(market_account()), pid, spec)
    }
    fn resolve(
        pid: ProposalId,
        branch: futarchy_primitives::Branch,
    ) -> frame_support::dispatch::DispatchResult {
        ConditionalLedger::resolve(RuntimeOrigin::signed(epoch_account()), pid, branch)
    }
    fn void(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        ConditionalLedger::void(RuntimeOrigin::signed(epoch_account()), pid)
    }
}

impl pallet_epoch::Config for Runtime {
    type Params = RuntimeEpochParams;
    type Market = RuntimeEpochMarket;
    type Oracle = RuntimeEpochOracle;
    type Guardian = RuntimeEpochGuardian;
    type Attestation = RuntimeEpochAttestation;
    type Constitution = RuntimeEpochConstitution;
    type Preimage = RuntimeEpochPreimage;
    type ExecutionGuard = RuntimeEpochGuard;
    type Welfare = RuntimeEpochWelfare;
    type Ledger = RuntimeEpochLedger;
    type GuardianOrigin = pallet_origins::EnsureGuardianHold;
    type ExecutionGuardOrigin = EnsureExecutionGuardAccount;
    type VoidAuthority = pallet_origins::EnsureEmergencyPlaybook;
    type ConstitutionalValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    // B9 keeper rebates ride the treasury meter (08 §6.3).
    type KeeperRebate = FutarchyTreasury;
    type WeightInfo = crate::weights::pallet_epoch::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

pub struct RuntimeGuardEpoch;
/// Test-only: whether `pid` exists in the real epoch pallet. B6's runtime e2e
/// fixtures drive the guard with synthetic pids committed through the test-only
/// payload store; for those, the epoch callbacks are no-ops (B6's original
/// pending-handoff behavior). Any pid with real epoch state takes the full
/// live path.
#[cfg(test)]
fn epoch_has_proposal(pid: ProposalId) -> bool {
    pallet_epoch::Proposals::<Runtime>::contains_key(pid)
        || pallet_epoch::IntakeProposals::<Runtime>::contains_key(pid)
}

#[cfg(test)]
fn test_pending_epoch_payload(pid: ProposalId) -> Option<H256> {
    sp_io::storage::get(&pending_epoch_payload_key(pid))
        .and_then(|bytes| H256::decode(&mut &bytes[..]).ok())
}

impl pallet_execution_guard::EpochHandoff for RuntimeGuardEpoch {
    fn payload_hash(pid: ProposalId) -> Option<H256> {
        let live = pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .map(|proposal| proposal.payload_hash);
        #[cfg(test)]
        return live.or_else(|| test_pending_epoch_payload(pid));
        #[cfg(not(test))]
        live
    }
    fn mark_executed(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        #[cfg(test)]
        if !epoch_has_proposal(pid) {
            return Ok(());
        }
        Epoch::mark_executed(RuntimeOrigin::signed(execution_guard_account()), pid)
    }
    fn mark_failed_executed(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        #[cfg(test)]
        if !epoch_has_proposal(pid) {
            return Ok(());
        }
        Epoch::mark_failed_executed(RuntimeOrigin::signed(execution_guard_account()), pid)
    }
    fn retry_exhausted_to_measurement(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        #[cfg(test)]
        if !epoch_has_proposal(pid) {
            return Ok(());
        }
        Epoch::retry_exhausted_to_measurement(RuntimeOrigin::signed(execution_guard_account()), pid)
    }
    fn reject_or_stale(
        pid: ProposalId,
        reason: futarchy_primitives::RejectReason,
    ) -> frame_support::dispatch::DispatchResult {
        #[cfg(test)]
        if !epoch_has_proposal(pid) {
            let _ = reason;
            return Ok(());
        }
        Epoch::expire_or_stale_queue(
            RuntimeOrigin::signed(execution_guard_account()),
            pid,
            Some(reason),
        )
    }
    fn is_terminal(pid: ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .is_none_or(|proposal| {
                matches!(
                    proposal.state,
                    ProposalState::Rejected(_)
                        | ProposalState::Executed
                        | ProposalState::Measuring
                        | ProposalState::Settled
                        | ProposalState::Cancelled
                        | ProposalState::Expired
                )
            })
    }
}

pub struct RuntimeGuardPreimages;
impl pallet_execution_guard::Preimages for RuntimeGuardPreimages {
    fn len(hash: H256) -> Option<u32> {
        <Preimage as QueryPreimage>::len(&hash.into())
    }
    fn fetch(hash: H256, expected_len: u32) -> Option<Vec<u8>> {
        if expected_len > futarchy_primitives::kernel::MAX_BYTES {
            return None;
        }
        <Preimage as QueryPreimage>::fetch(&hash.into(), Some(expected_len))
            .ok()
            .map(Cow::into_owned)
    }
    fn unpin(hash: H256) -> frame_support::dispatch::DispatchResult {
        let hash = hash.into();
        if <Preimage as QueryPreimage>::is_requested(&hash) {
            <Preimage as QueryPreimage>::unrequest(&hash);
        }
        Ok(())
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_payload_hash() -> H256 {
    benchmark_payload_hash_for(0)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_payload_hash_for(seed: ProposalId) -> H256 {
    sp_io::hashing::blake2_256(&benchmark_payload_bytes_for(seed))
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_payload_len() -> u32 {
    benchmark_payload_bytes().len() as u32
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_payload_bytes() -> Vec<u8> {
    benchmark_payload_bytes_for(0)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_payload_bytes_for(seed: ProposalId) -> Vec<u8> {
    let calls = (0..pallet_execution_guard::MAX_CALLS)
        .map(|index| {
            let mut remark = Vec::new();
            remark.resize(4_000, index as u8);
            if index == 0 {
                remark[..core::mem::size_of::<ProposalId>()].copy_from_slice(&seed.to_le_bytes());
            }
            RuntimeCall::System(frame_system::Call::remark { remark })
        })
        .collect::<Vec<_>>();
    benchmark_pad_payload(calls)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_upgrade_payload_bytes(artifact_hash: H256, call_count: u32) -> Vec<u8> {
    assert!((1..=pallet_execution_guard::MAX_CALLS_BOUND).contains(&call_count));
    let mut calls = Vec::from([RuntimeCall::System(frame_system::Call::authorize_upgrade {
        code_hash: artifact_hash.into(),
    })]);
    calls.extend((1..call_count).map(|index| {
        RuntimeCall::System(frame_system::Call::remark {
            remark: vec![index as u8; 4_000],
        })
    }));
    // With at least one public leaf, fill the payload byte ceiling as well as
    // the requested call count. A one-call Code batch must be the sole
    // authorize call, so its smaller payload is the worst admissible case.
    if call_count > 1 {
        benchmark_pad_payload(calls)
    } else {
        pallet_execution_guard::RuntimeBatch::<Runtime>::truncate_from(calls).encode()
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_pad_payload(mut calls: Vec<RuntimeCall>) -> Vec<u8> {
    let target = pallet_execution_guard::MAX_PAYLOAD_BYTES as usize;
    loop {
        let bytes =
            pallet_execution_guard::RuntimeBatch::<Runtime>::truncate_from(calls.clone()).encode();
        match bytes.len().cmp(&target) {
            core::cmp::Ordering::Equal => return bytes,
            core::cmp::Ordering::Less => {
                let RuntimeCall::System(frame_system::Call::remark { remark }) = calls
                    .last_mut()
                    .expect("benchmark payload has a final call")
                else {
                    unreachable!("benchmark payload's final call is always a System remark")
                };
                remark.resize(remark.len().saturating_add(target - bytes.len()), 0xff);
            }
            core::cmp::Ordering::Greater => {
                let RuntimeCall::System(frame_system::Call::remark { remark }) = calls
                    .last_mut()
                    .expect("benchmark payload has a final call")
                else {
                    unreachable!("benchmark payload's final call is always a System remark")
                };
                remark.truncate(remark.len().saturating_sub(bytes.len() - target));
            }
        }
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_ensure_payload_preimage(seed: ProposalId) -> (H256, u32) {
    let bytes = benchmark_payload_bytes_for(seed);
    let payload_len = bytes.len() as u32;
    let hash = sp_io::hashing::blake2_256(&bytes);
    if <Preimage as QueryPreimage>::len(&hash.into()).is_none() {
        let noted = benchmark_note_preimage(bytes);
        debug_assert_eq!(noted, hash);
    }
    (hash, payload_len)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_attestations(pid: ProposalId, artifact_hash: H256) {
    let members = (0..pallet_attestor::MAX_ATTESTORS)
        .map(|index| pallet_attestor::AttestorInfo {
            account: [100u8.saturating_add(index as u8); 32],
            bond: pallet_attestor::ATTESTOR_BOND,
            false_count: 0,
            active: true,
        })
        .collect::<Vec<_>>();
    pallet_attestor::Members::<Runtime>::put(frame_support::BoundedVec::truncate_from(members));

    let attestations = (0..pallet_attestor::MAX_ATTESTATIONS)
        .map(|id| {
            // Put the two matching records at the tail so both queue-time
            // binding and execute-time id lookup scan the full 256-entry
            // attestation ledger before the real quorum scan.
            let target = id
                >= pallet_attestor::MAX_ATTESTATIONS
                    .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM);
            pallet_attestor::Attestation {
                id,
                pid: if target {
                    pid
                } else {
                    100_000u64.saturating_add(u64::from(id))
                },
                artifact_hash: if target {
                    artifact_hash
                } else {
                    [id as u8; 32]
                },
                statement_hash: [id as u8; 32],
                attestor: [100u8.saturating_add((id % pallet_attestor::MAX_ATTESTORS) as u8); 32],
                submitted_at: 0,
                challenge_deadline: 0,
                challenge: None,
            }
        })
        .collect::<Vec<_>>();
    pallet_attestor::Attestations::<Runtime>::put(frame_support::BoundedVec::truncate_from(
        attestations,
    ));
    pallet_attestor::NextAttestationId::<Runtime>::put(pallet_attestor::MAX_ATTESTATIONS);
}

pub struct RuntimeGuardAttestations;
impl pallet_execution_guard::Attestations for RuntimeGuardAttestations {
    fn artifact_hash(attestation_id: u32) -> Option<H256> {
        pallet_attestor::Attestations::<Runtime>::get()
            .into_iter()
            .find_map(|attestation| {
                (attestation.id == attestation_id).then_some(attestation.artifact_hash)
            })
    }
    fn present_unrevoked_unchallenged(attestation_id: u32) -> bool {
        pallet_attestor::Attestations::<Runtime>::get()
            .into_iter()
            .find(|attestation| attestation.id == attestation_id)
            .is_some_and(|attestation| {
                !matches!(
                    attestation.challenge,
                    Some(pallet_attestor::ChallengeStatus::Open { .. })
                        | Some(pallet_attestor::ChallengeStatus::Upheld)
                )
            })
    }
    fn has_quorum(pid: ProposalId, artifact_hash: H256) -> bool {
        pallet_attestor::Pallet::<Runtime>::has_quorum(pid, artifact_hash)
    }
}

pub struct RuntimeGuardGuardian;
impl pallet_execution_guard::GuardianState for RuntimeGuardGuardian {
    fn rerun_held(pid: ProposalId) -> bool {
        <RuntimeEpochGuardian as pallet_epoch::GuardianAccess>::hold_active(pid)
    }
    fn ledger_freeze_active() -> bool {
        <RuntimeEpochConstitution as pallet_epoch::ConstitutionAccess<AccountId>>::ledger_frozen()
            || pallet_guardian::Pallet::<Runtime>::playbook_active(
                pallet_guardian::PlaybookId::LedgerFreeze,
            )
    }
}

pub struct RuntimeGuardParams;
fn class_index(class: ProposalClass) -> usize {
    match class {
        ProposalClass::Param => 0,
        ProposalClass::Treasury => 1,
        ProposalClass::Code => 2,
        ProposalClass::Meta => 3,
        ProposalClass::Constitutional => 4,
    }
}
impl pallet_execution_guard::Params for RuntimeGuardParams {
    fn exec_timelock(class: ProposalClass) -> u32 {
        <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get().timelock
            [class_index(class)]
    }
    fn exec_grace(class: ProposalClass) -> u32 {
        <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get().grace[class_index(class)]
    }
    fn code_spacing() -> u32 {
        u32_param(b"code.spacing")
    }
}

pub struct RuntimeReleaseChannel;
impl pallet_execution_guard::ReleaseChannelWriter for RuntimeReleaseChannel {
    fn on_upgrade_authorized(
        target_spec_version: u32,
        authorized_at: u32,
    ) -> frame_support::dispatch::DispatchResult {
        let mut bytes = pallet_constitution::ReleaseChannel::<Runtime>::get().bytes;
        bytes[108..112].copy_from_slice(&System::block_number().to_le_bytes());
        bytes[112..116].copy_from_slice(&target_spec_version.to_le_bytes());
        bytes[116..120].copy_from_slice(&authorized_at.to_le_bytes());
        let flags = u32::from_le_bytes([bytes[164], bytes[165], bytes[166], bytes[167]]) | (1 << 2);
        bytes[164..168].copy_from_slice(&flags.to_le_bytes());
        pallet_constitution::Pallet::<Runtime>::note_release_channel(bytes)
    }
    fn on_upgrade_applied(target_spec_version: u32) -> frame_support::dispatch::DispatchResult {
        // Tolerant clear (G-1/SQ-134, PR #65 P1): an applied upgrade cannot be
        // retried, so a writer-(b) `set_release_channel` rewrite that no longer
        // shows this pending upgrade is newer and authoritative — leave it
        // untouched and let the guard record the application. Only a channel
        // still showing exactly this pending upgrade is cleared (the
        // authorize-path already wrote `spec_version = target`).
        let channel = pallet_constitution::ReleaseChannel::<Runtime>::get();
        let mut bytes = channel.bytes;
        let pending_authorized_at =
            u32::from_le_bytes([bytes[116], bytes[117], bytes[118], bytes[119]]);
        let spec_version = u32::from_le_bytes([bytes[112], bytes[113], bytes[114], bytes[115]]);
        if pending_authorized_at == 0 || spec_version != target_spec_version {
            return Ok(());
        }
        bytes[108..112].copy_from_slice(&System::block_number().to_le_bytes());
        bytes[116..120].copy_from_slice(&0u32.to_le_bytes());
        let flags =
            u32::from_le_bytes([bytes[164], bytes[165], bytes[166], bytes[167]]) & !(1 << 2);
        bytes[164..168].copy_from_slice(&flags.to_le_bytes());
        pallet_constitution::Pallet::<Runtime>::note_release_channel(bytes)
    }
    fn on_upgrade_aborted(target_spec_version: u32) -> frame_support::dispatch::DispatchResult {
        // Tolerant clear (G-1/SQ-131 relay-abort ruling): a newer
        // `set_release_channel` rewrite during the in-flight upgrade is
        // authoritative — leave it untouched. Only a channel still showing
        // exactly this pending upgrade is cleared (bump `updated_at`, zero
        // `pending_authorized_at`, drop the urgent flag; `spec_version` stays
        // at status quo — the old code keeps running).
        let channel = pallet_constitution::ReleaseChannel::<Runtime>::get();
        let mut bytes = channel.bytes;
        let pending_authorized_at =
            u32::from_le_bytes([bytes[116], bytes[117], bytes[118], bytes[119]]);
        let spec_version = u32::from_le_bytes([bytes[112], bytes[113], bytes[114], bytes[115]]);
        if pending_authorized_at == 0 || spec_version != target_spec_version {
            return Ok(());
        }
        bytes[108..112].copy_from_slice(&System::block_number().to_le_bytes());
        bytes[116..120].copy_from_slice(&0u32.to_le_bytes());
        let flags =
            u32::from_le_bytes([bytes[164], bytes[165], bytes[166], bytes[167]]) & !(1 << 2);
        bytes[164..168].copy_from_slice(&flags.to_le_bytes());
        pallet_constitution::Pallet::<Runtime>::note_release_channel(bytes)
    }
}

impl pallet_execution_guard::Config for Runtime {
    type Epoch = RuntimeGuardEpoch;
    type EnqueueAuthority = EnsureEpochAccount;
    type Attestations = RuntimeGuardAttestations;
    type Guardian = RuntimeGuardGuardian;
    type Params = RuntimeGuardParams;
    // B6's two new pallet seams (upgrade path e2e): guard-side capability
    // projection and the parachain-system upgrade-schedule/migrations bridge.
    type Capabilities = RuntimeCapabilities;
    type UpgradeSchedule = RuntimeUpgradeSchedule;
    type Preimages = RuntimeGuardPreimages;
    type ReleaseChannel = RuntimeReleaseChannel;
    type RatifyOrigin = pallet_origins::EnsureConstitutionalValues;
    type Dispatcher = crate::classifier::RuntimeDispatcher;
    type MaxRuntimeCodeBytes = ConstU32<2_097_152>;
    // B9 keeper rebates ride the treasury meter (08 §6.3).
    type KeeperRebate = FutarchyTreasury;
    type WeightInfo = crate::weights::pallet_execution_guard::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

// --- B6 execution-guard production wiring ---------------------------------

/// Test/benchmark-only payload commitment store for the reserved A8 seam.
/// Production has no key path at all and therefore returns `None` below.
#[cfg(any(test, feature = "runtime-benchmarks"))]
fn pending_epoch_payload_key(pid: futarchy_primitives::ProposalId) -> Vec<u8> {
    let mut key = b":bleavit:b6:epoch-payload:".to_vec();
    key.extend_from_slice(&pid.to_le_bytes());
    key
}

#[cfg(any(test, feature = "runtime-benchmarks"))]
fn note_pending_epoch_payload(
    pid: futarchy_primitives::ProposalId,
    hash: futarchy_primitives::H256,
) {
    sp_io::storage::set(&pending_epoch_payload_key(pid), &hash.encode());
}

#[cfg(test)]
pub(crate) fn set_test_execution_payload(
    pid: futarchy_primitives::ProposalId,
    hash: futarchy_primitives::H256,
) {
    note_pending_epoch_payload(pid, hash);
}

/// Slot 61 is intentionally still empty. This adapter makes every production
/// enqueue fail its immutable epoch commitment check and makes terminal
/// callbacks no-ops; only cfg-gated tests/benchmarks can seed a commitment.
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
                    <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
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

/// Exact stable2603 pre-write checks performed by
/// `cumulus_pallet_parachain_system::schedule_code_upgrade`. Frame-system
/// removes `AuthorizedUpgrade` before invoking `OnSetCode`, and a direct
/// dispatch is not transactional, so every typed Cumulus rejection must be
/// refused by the filter before frame-system can consume the authorization.
pub(crate) fn parachain_upgrade_preflight(code: &[u8]) -> DispatchResult {
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

pub(crate) fn direct_system_upgrade_allowed(code: &[u8]) -> bool {
    let Some(pending) = pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() else {
        return false;
    };
    if sp_io::hashing::blake2_256(code) != pending.hash {
        return false;
    }
    let Some(observed) =
        <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
            RuntimeCall,
        >>::observed_runtime_version(code)
    else {
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
                    <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
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

#[cfg(feature = "runtime-benchmarks")]
pub struct RuntimeBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
const BENCHMARK_KEEPER_REBATE: Balance = currency::USDC;
#[cfg(feature = "runtime-benchmarks")]
const BENCHMARK_REBATE_LINE_BALANCE: Balance = 100 * currency::USDC;

/// Prime every storage/custody dependency of `do_keeper_rebate` so benchmark
/// dispatches measure the full successful payout path, never the zero-pay or
/// exhausted-latch path.
#[cfg(feature = "runtime-benchmarks")]
pub(crate) fn prime_keeper_rebate_worst_case() {
    let key = pallet_constitution::key16(b"keeper.rebate");
    pallet_constitution::Params::<Runtime>::insert(
        key,
        pallet_constitution::ParamRecord {
            key,
            value: pallet_constitution::ParamValue::Balance(BENCHMARK_KEEPER_REBATE),
            min: pallet_constitution::ParamValue::Balance(1),
            max: pallet_constitution::ParamValue::Balance(Balance::MAX),
            max_delta: None,
            cooldown_epochs: 0,
            last_changed_epoch: 0,
            class: pallet_constitution::ParamClass::Param,
            kernel_bounded: false,
        },
    );

    pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
        for line in [
            pallet_futarchy_treasury::BudgetLine::Keeper,
            pallet_futarchy_treasury::BudgetLine::Oracle,
        ] {
            if let Some((_, balance)) = state.lines.iter_mut().find(|(stored, _)| *stored == line) {
                *balance = BENCHMARK_REBATE_LINE_BALANCE;
            } else {
                state
                    .lines
                    .try_push((line, BENCHMARK_REBATE_LINE_BALANCE))
                    .expect("benchmark treasury has room for rebate lines");
            }
        }
        state.keeper_meter = pallet_futarchy_treasury::KeeperMeter {
            epoch: <LiveEpochClock as frame_support::traits::Get<EpochId>>::get(),
            ..Default::default()
        };
    });

    for pot in [treasury_keeper_account(), treasury_oracle_account()] {
        let balance = <ForeignAssets as Inspect<AccountId>>::balance(USDC_ASSET_ID, &pot);
        if balance < BENCHMARK_REBATE_LINE_BALANCE {
            <ForeignAssets as Mutate<AccountId>>::mint_into(
                USDC_ASSET_ID,
                &pot,
                BENCHMARK_REBATE_LINE_BALANCE - balance,
            )
            .expect("benchmark rebate pot funding must succeed");
        }
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn assert_keeper_rebate_was_paid(class: CrankClass) {
    let state = pallet_futarchy_treasury::State::<Runtime>::get();
    let line = match class {
        CrankClass::OracleLine => pallet_futarchy_treasury::BudgetLine::Oracle,
        CrankClass::DecisionCritical | CrankClass::General => {
            pallet_futarchy_treasury::BudgetLine::Keeper
        }
    };
    let line_balance = state
        .lines
        .iter()
        .find_map(|(stored, balance)| (*stored == line).then_some(*balance));
    assert_eq!(
        line_balance,
        Some(BENCHMARK_REBATE_LINE_BALANCE - BENCHMARK_KEEPER_REBATE),
        "benchmark crank must debit the funded rebate line"
    );
    match class {
        CrankClass::OracleLine => {}
        CrankClass::DecisionCritical => {
            assert_eq!(state.keeper_meter.spent, BENCHMARK_KEEPER_REBATE);
            assert_eq!(state.keeper_meter.general_spent, 0);
        }
        CrankClass::General => {
            assert_eq!(state.keeper_meter.spent, BENCHMARK_KEEPER_REBATE);
            assert_eq!(state.keeper_meter.general_spent, BENCHMARK_KEEPER_REBATE);
        }
    }
}

#[cfg(feature = "runtime-benchmarks")]
macro_rules! benchmark_keeper_rebate_hooks {
    () => {
        fn prime_keeper_rebate() {
            prime_keeper_rebate_worst_case();
        }

        fn assert_keeper_rebate_paid(class: CrankClass) {
            assert_keeper_rebate_was_paid(class);
        }
    };
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_conditional_ledger::BenchmarkHelper for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_market::BenchmarkHelper for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();
}

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
    benchmark_keeper_rebate_hooks!();

    fn metric_governance_origin() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
    fn prime_finalized_epoch(epoch: EpochId) {
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| {
            info.index = epoch.saturating_add(1);
        });
    }
    fn prime_metric_inputs(_: u16) {
        // RuntimeMetricInputs derives all active benchmark components from the
        // live MetricSpecs map, so no additional storage fixture is required.
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_oracle::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();

    fn adjudication_origin() -> RuntimeOrigin {
        pallet_origins::Origin::OracleResolution.into()
    }
    fn prime_reporting(component: u16, epoch: EpochId, version: u16) {
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = epoch);
        pallet_epoch::Schedule::<Runtime>::mutate(|schedule| {
            schedule.epoch_start_block = 0;
            schedule.length =
                <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get().epoch_length;
            schedule.next_length = schedule.length;
        });
        let spec = pallet_welfare::MetricSpec {
            id: component,
            version,
            pillar: pallet_welfare::Pillar::A,
            weight: FixedU64(1_000_000_000),
            epsilon_floor: FixedU64(1),
            activation_epoch: epoch,
            source: pallet_welfare::SourceClass::Attested,
            formula_ref: [1; 32],
            units: [2; 16],
            repr: [3; 16],
            cadence_blocks: 1,
            sanity_min: FixedU64(0),
            sanity_max: FixedU64(1_000_000_000),
            has_normalization_rule: true,
            has_missing_data_rule: true,
            has_gaming_vectors: true,
            has_challenge_procedure: true,
            prior_bounds: [FixedU64(1_000_000_000); pallet_welfare::HISTORY_PRIORS],
        };
        pallet_welfare::MetricSpecs::<Runtime>::insert(
            version,
            frame_support::BoundedVec::truncate_from(Vec::from([spec])),
        );
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_registry::BenchmarkHelper<RuntimeOrigin, AccountId> for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();

    fn resolution_origin() -> RuntimeOrigin {
        pallet_origins::Origin::OracleResolution.into()
    }
    fn funded_account(seed: u8) -> AccountId {
        let who = AccountId32::new([seed; 32]);
        let reserve = currency::USDC.saturating_mul(1_000_000);
        let _ = <ForeignAssets as frame_support::traits::fungibles::Mutate<AccountId>>::mint_into(
            USDC_ASSET_ID,
            &who,
            reserve,
        );
        for sovereign in [
            IncidentPalletId::get().into_account_truncating(),
            MilestonePalletId::get().into_account_truncating(),
        ] {
            let _ =
                <ForeignAssets as frame_support::traits::fungibles::Mutate<AccountId>>::mint_into(
                    USDC_ASSET_ID,
                    &sovereign,
                    reserve,
                );
        }
        who
    }
    fn register_watchtower(who: &AccountId) {
        let _ = pallet_oracle::Pallet::<Runtime>::register_watchtower(RuntimeOrigin::signed(
            who.clone(),
        ));
    }
    fn prime_epoch(epoch: EpochId) {
        <RuntimeBenchmarkHelper as pallet_oracle::BenchmarkHelper<RuntimeOrigin>>::prime_reporting(
            1, epoch, 1,
        );
        pallet_epoch::CohortSchedules::<Runtime>::insert(
            epoch.saturating_sub(1),
            pallet_epoch::CohortSchedule {
                epoch: epoch.saturating_sub(1),
                creation_epoch_length:
                    <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get().epoch_length,
                measurement_until: epoch,
                settlement_epoch: epoch.saturating_add(1),
                specs: frame_support::BoundedVec::truncate_from(Vec::from([(1, 1)])),
            },
        );
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_futarchy_treasury::BenchmarkHelper<RuntimeOrigin, AccountId>
    for RuntimeBenchmarkHelper
{
    benchmark_keeper_rebate_hooks!();

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
    fn prime_for_worst_case() {
        let who = AccountId32::new([1; 32]);
        let mut proposal = <RuntimeBenchmarkHelper as pallet_epoch::BenchmarkHelper<
            RuntimeOrigin,
            AccountId,
        >>::proposal(1, who, 1, 1);
        proposal.state = ProposalState::Queued;
        pallet_epoch::Proposals::<Runtime>::insert(1, proposal);
    }
    fn prime_maintenance_epoch(epoch: EpochId) {
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = epoch);
    }
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
    benchmark_keeper_rebate_hooks!();

    fn prime_submit_epoch(epoch: EpochId) {
        // The benchmark runner dispatches calls in block one even when its raw
        // externalities begin at zero; establish the same block during setup so
        // the submission timestamp exercises the real shape check.
        System::set_block_number(1);
        let now = System::block_number();
        let params = <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        pallet_epoch::EpochOf::<Runtime>::put(pallet_epoch::EpochInfo {
            index: epoch,
            phase: EpochPhase::Intake,
            phase_start_block: now,
        });
        pallet_epoch::Schedule::<Runtime>::put(pallet_epoch::EpochSchedule {
            epoch_start_block: now,
            length: params.epoch_length,
            next_length: params.epoch_length,
        });
        // `frame-omni-bencher` starts from raw externalities, not the runtime
        // genesis builder, so seed the same first admissible proposal id here.
        pallet_epoch::NextProposalId::<Runtime>::put(1);
    }
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
        id: ProposalId,
        who: AccountId,
        now: u32,
        epoch: EpochId,
    ) -> futarchy_primitives::Proposal<AccountId> {
        // Each proposal gets its own valid 64 KiB batch. In particular, the
        // epoch tick benchmark now proves one PreimageFor read per item rather
        // than collapsing ten commitments onto one physical trie key.
        let (payload_hash, payload_len) = benchmark_ensure_payload_preimage(id);
        futarchy_primitives::Proposal {
            id,
            proposer: who,
            class: ProposalClass::Param,
            state: ProposalState::Submitted,
            epoch,
            submitted_at: now,
            payload_hash,
            payload_len,
            ask: 0,
            bond: 1,
            resources: Default::default(),
            metric_spec: 1,
            decide_at: 0,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: None,
            maturity: None,
            grace_end: None,
            version_constraint: Some(RuntimeVersionConstraint {
                spec_name: futarchy_primitives::BoundedVec::try_from(
                    VERSION.spec_name.as_bytes().to_vec(),
                )
                .expect("benchmark runtime spec name fits the frozen bound"),
                spec_version: VERSION.spec_version,
            }),
            decision: None,
        }
    }
    fn prime_decision(
        pid: ProposalId,
        epoch: EpochId,
        gates: bool,
    ) -> futarchy_primitives::MarketSet {
        if !pallet_conditional_ledger::Vaults::<Runtime>::contains_key(pid) {
            ConditionalLedger::create_vault(RuntimeOrigin::signed(market_account()), pid, 1)
                .expect("benchmark vault creation must succeed");
        }
        if gates {
            let payload_hash = benchmark_payload_hash_for(pid);
            pallet_execution_guard::Ratifications::<Runtime>::insert(
                pid,
                pallet_execution_guard::pallet::RatificationRecord {
                    referendum_index: pid as u32,
                    payload_hash,
                    ratified_at: 1,
                },
            );
            benchmark_fill_attestations(pid, payload_hash);
        }
        benchmark_market_set(pid, epoch, gates)
    }
    fn prime_guard_enqueue(pid: ProposalId) {
        // Leave exactly one queue slot and one eight-meter held-resource slot
        // for the measured epoch decision's nested guard enqueue.
        benchmark_fill_guard_queue_to(pallet_execution_guard::MAX_QUEUE_BOUND - 1, pid);
        benchmark_fill_guard_records();
        benchmark_fill_guard_envelopes();
    }
    fn prime_settlement(epoch: EpochId) {
        for (pid, proposal) in pallet_epoch::Proposals::<Runtime>::iter() {
            if proposal.epoch == epoch {
                let _ = ConditionalLedger::resolve(
                    RuntimeOrigin::signed(epoch_account()),
                    pid,
                    futarchy_primitives::Branch::Accept,
                );
            }
        }
        if !pallet_conditional_ledger::BaselineVaults::<Runtime>::contains_key(epoch) {
            ConditionalLedger::create_baseline_vault(
                RuntimeOrigin::signed(market_account()),
                epoch,
            )
            .expect("benchmark baseline vault creation must succeed");
        }
        pallet_market::BaselineMarketOf::<Runtime>::insert(
            epoch,
            9_000u64.saturating_add(u64::from(epoch)),
        );
        // The runtime settlement adapter reads the cohort-key snapshot after
        // delegating the actual H=2 computation to welfare. Keep that adapter
        // lookup populated in addition to the two normative measurement epochs.
        for measured_epoch in [epoch, epoch.saturating_add(1), epoch.saturating_add(2)] {
            pallet_welfare::Snapshots::<Runtime>::insert(
                (measured_epoch, 1),
                pallet_welfare::StoredSnapshot {
                    epoch: measured_epoch,
                    spec_version: 1,
                    s_pillar: FixedU64(500_000_000),
                    c_onchain: FixedU64(500_000_000),
                    c_attested: FixedU64(500_000_000),
                    p_pillar: FixedU64(500_000_000),
                    a_pillar: FixedU64(500_000_000),
                    gate_s: FixedU64(500_000_000),
                    gate_c: FixedU64(500_000_000),
                    welfare: FixedU64(500_000_000),
                    components: frame_support::BoundedVec::truncate_from(
                        pallet_welfare::benchmarking::healthy(
                            pallet_welfare::MAX_COMPONENTS_PER_SPEC as u16,
                        ),
                    ),
                },
            );
        }
        benchmark_fill_welfare_aggregate();
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_execution_guard::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();

    fn ratify_origin() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
    fn prime_ratify(pid: ProposalId, referendum_index: u32) {
        let hash = benchmark_payload_hash();
        let payload_len = benchmark_payload_len();
        let meters = benchmark_guard_meters(pid);
        benchmark_insert_epoch_proposal_with_resources(
            pid,
            hash,
            payload_len,
            ProposalState::Submitted,
            meters.clone().into_inner(),
        );
        let mut item = benchmark_queue_item(pid, hash, payload_len, ProposalClass::Code);
        item.meters_declared = meters;
        item.ratify_ref = Some(referendum_index);
        pallet_execution_guard::pallet::Queue::<Runtime>::insert(pid, item);
        benchmark_fill_guard_queue(pid);
        benchmark_fill_guard_records();
        benchmark_fill_guard_envelopes();
        benchmark_fill_ratifications(pid);
    }
    fn prime_execute(pid: ProposalId, calls: u32) {
        let artifact_hash = [0x42; 32];
        let spacing = <RuntimeGuardParams as pallet_execution_guard::Params>::code_spacing();
        System::set_block_number(
            spacing.saturating_mul(pallet_execution_guard::MAX_EXECUTION_RECORDS as u32 + 1),
        );
        let bytes = benchmark_upgrade_payload_bytes(artifact_hash, calls);
        let hash = benchmark_note_preimage(bytes.clone());
        let meters = benchmark_guard_meters(pid);
        benchmark_insert_epoch_proposal_with_resources(
            pid,
            hash,
            bytes.len() as u32,
            ProposalState::Queued,
            meters.clone().into_inner(),
        );
        let referendum_index = pid as u32;
        pallet_execution_guard::pallet::Ratifications::<Runtime>::insert(
            pid,
            pallet_execution_guard::pallet::RatificationRecord {
                referendum_index,
                payload_hash: hash,
                ratified_at: System::block_number(),
            },
        );
        benchmark_fill_attestations(pid, artifact_hash);
        let mut item = benchmark_queue_item(pid, hash, bytes.len() as u32, ProposalClass::Code);
        item.meters_declared = meters;
        item.ratify_ref = Some(referendum_index);
        item.attestation_id = Some(
            pallet_attestor::MAX_ATTESTATIONS
                .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM),
        );
        item.declared_domains = if calls > 1 {
            frame_support::BoundedVec::truncate_from(Vec::from([
                pallet_execution_guard::CallDomain::Public,
                pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
            ]))
        } else {
            frame_support::BoundedVec::truncate_from(Vec::from([
                pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
            ]))
        };
        let now = System::block_number();
        item.maturity = now.saturating_add(
            <RuntimeGuardParams as pallet_execution_guard::Params>::exec_timelock(item.class),
        );
        item.grace_end = item.maturity.saturating_add(
            <RuntimeGuardParams as pallet_execution_guard::Params>::exec_grace(item.class),
        );
        ExecutionGuard::enqueue(RuntimeOrigin::signed(epoch_account()), item, false)
            .expect("benchmark guard enqueue must succeed");
        System::set_block_number(now.saturating_add(
            <RuntimeGuardParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Code,
            ),
        ));
        benchmark_fill_guard_queue(pid);
        benchmark_fill_guard_records();
        benchmark_fill_guard_envelopes();
        benchmark_fill_upgrade_spacing_history(System::block_number());
        benchmark_fill_epoch_aggregate_for_measurement();
    }
    fn prime_failed(pid: ProposalId) {
        let hash = benchmark_payload_hash();
        let payload_len = benchmark_payload_len();
        let meters = benchmark_guard_meters(pid);
        benchmark_insert_epoch_proposal_with_resources(
            pid,
            hash,
            payload_len,
            ProposalState::FailedExecuted,
            meters.clone().into_inner(),
        );
        let mut item = benchmark_queue_item(pid, hash, payload_len, ProposalClass::Param);
        item.meters_declared = meters;
        item.failed_at = Some(1);
        pallet_execution_guard::pallet::Queue::<Runtime>::insert(pid, item);
        benchmark_fill_guard_queue(pid);
        benchmark_fill_guard_records();
        benchmark_fill_guard_envelopes();
        benchmark_fill_epoch_aggregate_for_measurement();
        System::set_block_number(1u32.saturating_add(pallet_execution_guard::RETRY_WINDOW + 1));
    }
    fn prime_pending_upgrade(bytes: u32) -> Vec<u8> {
        let code = benchmark_runtime_code(bytes);
        let hash = sp_io::hashing::blake2_256(&code);
        let now = System::block_number();
        ParachainSystem::initialize_for_set_code_benchmark(code.len() as u32);
        System::authorize_upgrade(RuntimeOrigin::root(), hash.into())
            .expect("benchmark system upgrade authorization must succeed");
        pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
            pallet_execution_guard::PendingUpgrade {
                hash,
                authorized_at: now,
                applicable_at: now.saturating_add(pallet_execution_guard::DESCRIPTOR_LEAD_TIME),
                target_spec_version: VERSION.spec_version,
            },
        );
        System::set_block_number(now.saturating_add(pallet_execution_guard::DESCRIPTOR_LEAD_TIME));
        benchmark_fill_guard_queue(ProposalId::MAX);
        benchmark_fill_guard_records();
        benchmark_fill_guard_envelopes();
        code
    }
    fn prime_stale(pid: ProposalId) {
        let hash = benchmark_payload_hash();
        let payload_len = benchmark_payload_len();
        let meters = benchmark_guard_meters(pid);
        benchmark_insert_epoch_proposal_with_resources(
            pid,
            hash,
            payload_len,
            ProposalState::Queued,
            meters.clone().into_inner(),
        );
        let mut item = benchmark_queue_item(pid, hash, payload_len, ProposalClass::Param);
        item.meters_declared = meters;
        item.version_constraint.spec_version =
            item.version_constraint.spec_version.saturating_add(1);
        pallet_execution_guard::pallet::Queue::<Runtime>::insert(pid, item);
        benchmark_fill_guard_queue(pid);
        benchmark_fill_guard_records();
        benchmark_fill_guard_envelopes();
        benchmark_fill_epoch_aggregate_for_measurement();
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_epoch_aggregate_for_measurement() {
    let mut state = Epoch::epoch_state();
    pallet_epoch::benchmarking::fill_epoch_state::<Runtime>(
        &mut state,
        pallet_epoch::MAX_INTAKE_QUEUE,
        pallet_epoch::MAX_LIVE_PROPOSALS,
        // All three guard callbacks measured here start the target's
        // measurement cohort. Three existing cohorts are therefore the
        // maximum admissible pre-state; the measured callback fills slot four.
        pallet_epoch::MAX_NON_TERMINAL_COHORTS - 1,
    );
    Epoch::seed(state).expect("benchmark epoch aggregate must satisfy every frozen bound");
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_welfare_aggregate() {
    let mut state = Welfare::welfare_state();
    for version in 1..=pallet_welfare::MAX_METRIC_SPECS as u16 {
        if state.specs.iter().all(|(stored, _)| *stored != version) {
            state
                .register_metric_spec(
                    0,
                    version,
                    pallet_welfare::benchmarking::full_specs(version),
                )
                .expect("benchmark welfare specification must be valid");
        }
    }
    let mut epoch = 100u32;
    while state.snapshots.len() < pallet_welfare::MAX_SNAPSHOTS {
        state
            .record_snapshot(
                epoch,
                1,
                pallet_welfare::benchmarking::healthy(
                    pallet_welfare::MAX_COMPONENTS_PER_SPEC as u16,
                ),
                FixedU64(1_000_000_000),
                &pallet_welfare::CoreWelfareParams::DEFAULT,
            )
            .expect("benchmark welfare snapshot must be valid");
        epoch = epoch.saturating_add(1);
    }
    epoch = 100;
    while state.gate_flags.len() < pallet_welfare::MAX_GATE_FLAGS {
        state
            .record_daily_gate(
                epoch,
                0,
                1,
                pallet_welfare::benchmarking::healthy(
                    pallet_welfare::MAX_COMPONENTS_PER_SPEC as u16,
                ),
                &pallet_welfare::CoreWelfareParams::DEFAULT,
            )
            .expect("benchmark welfare gate flags must be valid");
        epoch = epoch.saturating_add(1);
    }
    Welfare::seed(&state).expect("benchmark welfare aggregate must satisfy every frozen bound");
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_runtime_version() -> RuntimeVersionConstraint {
    RuntimeVersionConstraint {
        spec_name: futarchy_primitives::BoundedVec::try_from(VERSION.spec_name.as_bytes().to_vec())
            .expect("benchmark runtime spec name fits the frozen bound"),
        spec_version: VERSION.spec_version,
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_push_leb128(mut value: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_custom_section(name: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    benchmark_push_leb128(name.len(), &mut body);
    body.extend_from_slice(name);
    body.extend_from_slice(payload);
    let mut section = Vec::new();
    section.push(0);
    benchmark_push_leb128(body.len(), &mut section);
    section.extend(body);
    section
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_runtime_code(target_code_bytes: u32) -> Vec<u8> {
    const WASM_HEADER: [u8; 8] = [0, 97, 115, 109, 1, 0, 0, 0];
    const VERSION_SECTION: &[u8] = b"runtime_version";
    const PADDING_SECTION: &[u8] = b"benchmark_padding";
    let max_code_bytes = <<Runtime as pallet_execution_guard::Config>::MaxRuntimeCodeBytes as frame_support::traits::Get<u32>>::get()
        as usize;
    let target_code_bytes = target_code_bytes as usize;
    assert!(target_code_bytes <= max_code_bytes);

    // A minimal valid module with the same embedded RuntimeVersion is enough
    // for both guard and frame-system version checks. A custom section pads it
    // to the sampled byte count so the generated coefficient follows the exact
    // `code.len()` quantity charged by the dispatch.
    let mut code = Vec::from(WASM_HEADER);
    code.extend(benchmark_custom_section(VERSION_SECTION, &VERSION.encode()));
    assert!(
        code.len()
            .saturating_add(benchmark_custom_section(PADDING_SECTION, &[]).len())
            <= target_code_bytes,
        "benchmark runtime fixture needs at least the component's 512-byte floor"
    );
    let mut padding_len = target_code_bytes.saturating_sub(code.len());
    loop {
        let padding = vec![0; padding_len];
        let section = benchmark_custom_section(PADDING_SECTION, &padding);
        match code
            .len()
            .saturating_add(section.len())
            .cmp(&target_code_bytes)
        {
            core::cmp::Ordering::Equal => {
                code.extend(section);
                break;
            }
            core::cmp::Ordering::Greater => padding_len = padding_len.saturating_sub(1),
            core::cmp::Ordering::Less => {
                padding_len = padding_len.saturating_add(1);
            }
        }
    }
    code
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_guard_meters(pid: ProposalId) -> pallet_execution_guard::StoredMeters {
    let meters = (0..pallet_execution_guard::MAX_RESOURCE_LOCKS)
        .map(|index| {
            let mut meter = pid.to_le_bytes();
            meter[7] = index as u8;
            meter
        })
        .collect::<Vec<_>>();
    frame_support::BoundedVec::truncate_from(meters)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_note_preimage(bytes: Vec<u8>) -> H256 {
    let who = AccountId32::new([249; 32]);
    let _ = <Balances as frame_support::traits::fungible::Mutate<AccountId>>::mint_into(
        &who,
        currency::VIT,
    );
    let hash = sp_io::hashing::blake2_256(&bytes);
    Preimage::note_preimage(RuntimeOrigin::signed(who), bytes)
        .expect("benchmark preimage note must succeed");
    hash
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_queue_item(
    pid: ProposalId,
    payload_hash: H256,
    payload_len: u32,
    class: ProposalClass,
) -> pallet_execution_guard::StoredQueuedExecution {
    pallet_execution_guard::StoredQueuedExecution {
        pid,
        payload_hash,
        payload_len,
        class,
        maturity: 0,
        grace_end: u32::MAX,
        version_constraint: benchmark_runtime_version(),
        meters_declared: Default::default(),
        ratify_ref: None,
        ratification_passed: false,
        attestation_id: None,
        pre_upgrade_checkpoint: None,
        cancelled: false,
        declared_domains: frame_support::BoundedVec::truncate_from(Vec::from([
            pallet_execution_guard::CallDomain::Public,
        ])),
        failed_at: None,
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_insert_epoch_proposal_with_resources(
    pid: ProposalId,
    payload_hash: H256,
    payload_len: u32,
    state: ProposalState,
    resources: Vec<[u8; 8]>,
) {
    let epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
    let who = AccountId32::new([248; 32]);
    let mut proposal = <RuntimeBenchmarkHelper as pallet_epoch::BenchmarkHelper<
        RuntimeOrigin,
        AccountId,
    >>::proposal(pid, who, 1, epoch);
    proposal.payload_hash = payload_hash;
    proposal.payload_len = payload_len;
    proposal.state = state;
    proposal.decide_at = 1;
    proposal.maturity = Some(0);
    proposal.grace_end = Some(u32::MAX);
    proposal.decision = Some(futarchy_primitives::DecisionOutcome::Adopt);
    proposal.markets = Some(benchmark_market_set(pid, epoch, false));
    proposal.resources = futarchy_primitives::BoundedVec::try_from(resources.clone())
        .expect("benchmark resource set fits the kernel bound");
    pallet_epoch::Proposals::<Runtime>::insert(pid, proposal);
    pallet_epoch::ProposalSchedules::<Runtime>::insert(
        pid,
        pallet_epoch::ProposalSchedule {
            epoch,
            epoch_start_block: 0,
            epoch_length: <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get()
                .epoch_length,
            decide_at: 1,
            metric_spec: 1,
        },
    );
    pallet_epoch::ResourceLocks::<Runtime>::mutate(|locks| {
        for resource in resources {
            let _ = locks.try_push((resource, pid));
        }
    });
    if !pallet_conditional_ledger::Vaults::<Runtime>::contains_key(pid) {
        ConditionalLedger::create_vault(RuntimeOrigin::signed(market_account()), pid, 1)
            .expect("benchmark callback vault creation must succeed");
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_guard_queue(except: ProposalId) {
    benchmark_fill_guard_queue_to(pallet_execution_guard::MAX_QUEUE_BOUND, except);
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_guard_queue_to(target: u32, except: ProposalId) {
    let mut pid = 10_000;
    while pallet_execution_guard::pallet::Queue::<Runtime>::count() < target {
        if pid != except {
            let mut item = benchmark_queue_item(
                pid,
                benchmark_payload_hash(),
                benchmark_payload_len(),
                ProposalClass::Param,
            );
            item.meters_declared = benchmark_guard_meters(pid);
            pallet_execution_guard::pallet::Queue::<Runtime>::insert(pid, item);
        }
        pid = pid.saturating_add(1);
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_guard_envelopes() {
    let held = pallet_execution_guard::pallet::Queue::<Runtime>::iter()
        .flat_map(|(pid, item)| {
            item.meters_declared
                .into_inner()
                .into_iter()
                .map(move |meter| (pid, meter))
        })
        .collect::<Vec<_>>();
    pallet_execution_guard::pallet::HeldResources::<Runtime>::put(
        frame_support::BoundedVec::truncate_from(held),
    );

    // Blocked meters are an independent bounded maintenance envelope. Keep
    // them disjoint from every queued declaration so the measured successful
    // path still performs the maximum scan without taking an early rejection.
    let blocked = (0..pallet_execution_guard::MAX_BLOCKED_METERS_BOUND)
        .map(|index| {
            let mut meter = [0xff; 8];
            meter[4..8].copy_from_slice(&index.to_le_bytes());
            meter
        })
        .collect::<Vec<_>>();
    pallet_execution_guard::pallet::BlockedMeters::<Runtime>::put(
        frame_support::BoundedVec::truncate_from(blocked),
    );
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_guard_records() {
    let records = (0..pallet_execution_guard::MAX_EXECUTION_RECORDS)
        .map(|index| futarchy_primitives::ExecutionRecord {
            pid: 20_000u64.saturating_add(index as u64),
            payload_hash: [index as u8; 32],
            class: ProposalClass::Param,
            executed_at: index as u32,
            result: futarchy_primitives::DispatchOutcomeCode::Ok,
        })
        .collect::<Vec<_>>();
    pallet_execution_guard::pallet::ExecutionRecords::<Runtime>::put(
        frame_support::BoundedVec::truncate_from(records),
    );
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_upgrade_spacing_history(now: u32) {
    let spacing = <RuntimeGuardParams as pallet_execution_guard::Params>::code_spacing();
    let count = pallet_execution_guard::MAX_EXECUTION_RECORDS as u32;
    let history = (0..count)
        .map(|index| {
            (
                now.saturating_sub(spacing.saturating_mul(count.saturating_sub(index))),
                spacing,
            )
        })
        .collect::<Vec<_>>();
    let history = pallet_execution_guard::pallet::StoredUpgradeSpacingHistory::try_from(history)
        .expect("benchmark upgrade history equals the execution-record bound");
    pallet_execution_guard::pallet::UpgradeSpacingHistory::<Runtime>::put(history.clone());
    if let Some((authorized_at, _)) = history.last() {
        pallet_execution_guard::pallet::LastUpgradeAuthorized::<Runtime>::put(*authorized_at);
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_ratifications(except: ProposalId) {
    let mut pid = 30_000;
    while pallet_execution_guard::pallet::Ratifications::<Runtime>::count().saturating_add(1)
        < pallet_execution_guard::MAX_RATIFICATIONS_BOUND
    {
        if pid != except {
            pallet_execution_guard::pallet::Ratifications::<Runtime>::insert(
                pid,
                pallet_execution_guard::pallet::RatificationRecord {
                    referendum_index: pid as u32,
                    payload_hash: [pid as u8; 32],
                    ratified_at: 1,
                },
            );
        }
        pid = pid.saturating_add(1);
    }
}
