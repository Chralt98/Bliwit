//! Runtime configuration and the B1a fail-closed cross-pallet adapters.

use alloc::{borrow::Cow, boxed::Box, vec, vec::Vec};

#[cfg(feature = "runtime-benchmarks")]
use frame_support::traits::fungible::MutateHold;
use frame_support::{
    derive_impl,
    dispatch::{DispatchClass, DispatchResult},
    parameter_types,
    traits::{
        fungibles::{Inspect, Mutate},
        tokens::{Fortitude, Preservation},
        Bounded, ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, Contains, EqualPrivilegeOnly,
        Get, InstanceFilter, Nothing, OriginTrait, QueryPreimage, StorageInstance, StorePreimage,
        TransformOrigin, UnfilteredDispatchable, VariantCountOf, VestedTransfer, WithdrawReasons,
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
#[cfg(feature = "runtime-benchmarks")]
use futarchy_primitives::keeper::CrankClass;
use futarchy_primitives::{bounds, chain_identity, currency, kernel, EpochId, FixedU64, ParamKey};
use parity_scale_codec::{DecodeAll, Encode};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
#[cfg(feature = "runtime-benchmarks")]
use sp_runtime::AccountId32;
use sp_runtime::{
    traits::{AccountIdConversion, AccountIdLookup},
    DispatchError, Perbill,
};

use crate::{
    usdc_location, AccountId, AssetId, Aura, Balance, Balances, Block, BlockNumber,
    CollatorSelection, ConditionalLedger, ConsensusHook, Epoch, ExecutionGuard, ForeignAssets,
    FutarchyTreasury, Hash, Market, MessageQueue, Migrations, Nonce, PalletInfo, ParachainSystem,
    PolkadotXcm, Preimage, Referenda, Runtime, RuntimeCall, RuntimeEvent, RuntimeFreezeReason,
    RuntimeHoldReason, RuntimeOrigin, RuntimeTask, Scheduler, Session, SessionKeys, System,
    Vesting, XcmpQueue, VERSION,
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

// B16: this runtime's first storage migration — retires the inert
// `ExecutionGuard::BlockedMeters` (SQ-146) and the runtime stall-progress marker
// (SQ-132), gated on `pallet-execution-guard` storage version `0 -> 1`. See
// `crate::migrations`. NB: `SingleBlockMigrations` runs inside `on_runtime_upgrade`
// and creates **no** `pallet-migrations` cursor, so it never engages the
// `OnlyInherents` multi-block-migration lockdown (09 §3.2).
#[cfg(all(not(feature = "phase-four"), not(feature = "recovery")))]
type SingleBlockMigrations = (
    crate::migrations::RetireB16State,
    crate::migrations::MigrateConstitutionReserveProbeV2,
    crate::migrations::MigrateOracleReserveProbeV1,
);
#[cfg(all(feature = "phase-four", not(feature = "recovery")))]
type SingleBlockMigrations = (
    crate::migrations::RetireB16State,
    crate::migrations::MigrateConstitutionReserveProbeV2,
    crate::migrations::MigrateOracleReserveProbeV1,
    crate::migrations::PhaseFourTransition,
);
#[cfg(feature = "recovery")]
type SingleBlockMigrations = (
    crate::migrations::RetireB16State,
    crate::migrations::MigrateConstitutionReserveProbeV2,
    crate::migrations::MigrateOracleReserveProbeV1,
    crate::migrations::TerminalRecoveryTransition,
);

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
    type MultiBlockMigrator = RecoveryAwareMigrations;
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
    /// 08 §2.1: the keyless genesis community pot.
    pub CommunityDistributionPot: AccountId = crate::genesis::community_account();
    /// 08 §2.1: 250 million VIT held in that pot at genesis.
    pub CommunityDistributionAmount: Balance = crate::genesis::COMMUNITY_DISTRIBUTION;
    /// 08 §2.1: two nominal 365-day years at the 6-second block cadence.
    pub CommunityVestingDuration: BlockNumber = 2 * crate::genesis::BLOCKS_PER_YEAR;
    /// 13 §1/§3: the SDK's one-VIT minimum transfer.
    pub CommunityMinVestedTransfer: Balance = currency::VIT;
    /// 13 §4: bounded distribution state.
    pub MaxCommunitySchedules: u32 = bounds::MAX_COMMUNITY_SCHEDULES;
}

pub struct RuntimeCommunityVesting;
impl pallet_futarchy_treasury::CommunityVesting<AccountId, BlockNumber>
    for RuntimeCommunityVesting
{
    fn vested_transfer(
        source: &AccountId,
        beneficiary: &AccountId,
        amount: Balance,
        per_block: Balance,
        starting_block: BlockNumber,
    ) -> DispatchResult {
        <Vesting as VestedTransfer<AccountId>>::vested_transfer(
            source,
            beneficiary,
            amount,
            per_block,
            starting_block,
        )
    }
}

parameter_types! {
    pub UsdcAssetId: AssetId = usdc_location();
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

#[cfg(feature = "runtime-benchmarks")]
type ForeignAssetsForceOrigin = EnsureRoot<AccountId>;
#[cfg(not(feature = "runtime-benchmarks"))]
type ForeignAssetsForceOrigin = frame_system::EnsureNever<AccountId>;

#[cfg(feature = "runtime-benchmarks")]
pub struct AssetBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_assets::BenchmarkHelper<AssetId, ()> for AssetBenchmarkHelper {
    fn create_asset_id_parameter(id: u32) -> AssetId {
        bleavit_xcm::identity::asset_hub_asset_location(id as u128)
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
        if asset_id != usdc_location() {
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
        let asset_id = bleavit_xcm::identity::asset_hub_asset_location(id as u128);
        (asset_id.clone(), asset_id)
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

/// 06 §3.4 admits scheduling only through referenda's internal Scheduler API;
/// no user or privileged origin may submit arbitrary scheduler calls.
pub struct InternalSchedulerOnly;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for InternalSchedulerOnly {
    type Success = ();
    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            EnsureRoot::<AccountId>::try_origin(origin)
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
    type ScheduleOrigin = InternalSchedulerOnly;
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

pub struct RecoveryLockdownStorage;
impl StorageInstance for RecoveryLockdownStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryLockdown";
}
pub type RecoveryLockdown = frame_support::storage::types::StorageValue<
    RecoveryLockdownStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

pub struct RecoveryBypassStorage;
impl StorageInstance for RecoveryBypassStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryBypass";
}
pub type RecoveryBypass = frame_support::storage::types::StorageValue<
    RecoveryBypassStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

pub struct RetiredMigrationCursorStorage;
impl StorageInstance for RetiredMigrationCursorStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RetiredCursor";
}
pub type RetiredMigrationCursor = frame_support::storage::types::StorageValue<
    RetiredMigrationCursorStorage,
    pallet_migrations::CursorOf<Runtime>,
    frame_support::pallet_prelude::OptionQuery,
>;

pub struct RecoveryScheduledHashStorage;
impl StorageInstance for RecoveryScheduledHashStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryScheduledHash";
}
pub type RecoveryScheduledHash = frame_support::storage::types::StorageValue<
    RecoveryScheduledHashStorage,
    futarchy_primitives::H256,
    frame_support::pallet_prelude::OptionQuery,
>;

pub struct RecoveryAbortedStorage;
impl StorageInstance for RecoveryAbortedStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryAborted";
}
pub type RecoveryAborted = frame_support::storage::types::StorageValue<
    RecoveryAbortedStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

pub struct RecoveryCodeAppliedStorage;
impl StorageInstance for RecoveryCodeAppliedStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryCodeApplied";
}
pub type RecoveryCodeApplied = frame_support::storage::types::StorageValue<
    RecoveryCodeAppliedStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

#[cfg(feature = "recovery")]
pub struct RecoveryLedgerCursorStorage;
#[cfg(feature = "recovery")]
impl StorageInstance for RecoveryLedgerCursorStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryLedgerCursor";
}
#[cfg(feature = "recovery")]
pub type RecoveryLedgerCursor = frame_support::storage::types::StorageValue<
    RecoveryLedgerCursorStorage,
    pallet_conditional_ledger::migration::BackfillCursor,
    frame_support::pallet_prelude::OptionQuery,
>;

#[cfg(feature = "recovery")]
pub struct RecoveryLedgerRepairActiveStorage;
#[cfg(feature = "recovery")]
impl StorageInstance for RecoveryLedgerRepairActiveStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryLedgerRepairActive";
}
#[cfg(feature = "recovery")]
pub type RecoveryLedgerRepairActive = frame_support::storage::types::StorageValue<
    RecoveryLedgerRepairActiveStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

#[cfg(feature = "recovery")]
pub struct RecoveryLedgerRepairStepsStorage;
#[cfg(feature = "recovery")]
impl StorageInstance for RecoveryLedgerRepairStepsStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryLedgerRepairSteps";
}
#[cfg(feature = "recovery")]
pub type RecoveryLedgerRepairSteps = frame_support::storage::types::StorageValue<
    RecoveryLedgerRepairStepsStorage,
    u32,
    frame_support::pallet_prelude::ValueQuery,
>;

#[cfg(feature = "recovery")]
pub struct RecoveryLedgerRepairFailedStorage;
#[cfg(feature = "recovery")]
impl StorageInstance for RecoveryLedgerRepairFailedStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "RecoveryLedgerRepairFailed";
}
#[cfg(feature = "recovery")]
pub type RecoveryLedgerRepairFailed = frame_support::storage::types::StorageValue<
    RecoveryLedgerRepairFailedStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

pub struct PhaseTransitionLockStorage;
impl StorageInstance for PhaseTransitionLockStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "PhaseTransitionLock";
}
pub type PhaseTransitionLock = frame_support::storage::types::StorageValue<
    PhaseTransitionLockStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

pub struct PhaseTransitionAppliedStorage;
impl StorageInstance for PhaseTransitionAppliedStorage {
    fn pallet_prefix() -> &'static str {
        "BleavitRuntimeMigration"
    }
    const STORAGE_PREFIX: &'static str = "PhaseTransitionApplied";
}
pub type PhaseTransitionApplied = frame_support::storage::types::StorageValue<
    PhaseTransitionAppliedStorage,
    bool,
    frame_support::pallet_prelude::ValueQuery,
>;

#[cfg(feature = "recovery")]
pub(crate) fn recovery_ledger_bookkeeping_weight() -> Weight {
    // Runtime-local recovery state is outside the pallet benchmark's storage
    // inventory. Charge its fixed reads/writes plus a conservative 16 KiB
    // proof envelope in addition to the generated ledger row/terminal weight.
    <Runtime as frame_system::Config>::DbWeight::get()
        .reads_writes(8, 8)
        .saturating_add(Weight::from_parts(0, 16 * 1024))
}

#[cfg(feature = "recovery")]
pub(crate) fn recovery_guard_finalization_weight() -> Weight {
    // This executable benchmark saturates the 32-entry Queue and measures the
    // exact guard recovery-application path, including full load/clear/reinsert,
    // pending-outflow sync, release-channel update and recovery-image unpin.
    // Precharge it on every step because discovering that the ledger cursor is
    // terminal requires the benchmarked storage read itself.
    <crate::weights::pallet_execution_guard::WeightInfo<Runtime> as
        pallet_execution_guard::WeightInfo>::finalize_recovery_application()
}

pub(crate) fn recovery_aware_migration_detector_weight() -> Weight {
    // `step` must first discriminate the runtime-local repair/failure/lock
    // branches before delegating to FRAME's migrator. Charge all three
    // ValueQuery reads on every branch, including the persistent fail-locked
    // branch, plus a fixed proof envelope for their runtime-local keys.
    <Runtime as frame_system::Config>::DbWeight::get()
        .reads(3)
        .saturating_add(Weight::from_parts(0, 8 * 1024))
}

#[cfg(feature = "recovery")]
fn step_ledger_recovery() -> Weight {
    use frame_support::{
        migrations::SteppedMigration,
        storage::with_storage_layer,
        traits::{GetStorageVersion, StorageVersion},
        weights::WeightMeter,
    };

    let mut meter = WeightMeter::with_limit(
        MigrationMaxServiceWeight::get().saturating_sub(recovery_aware_migration_detector_weight()),
    );
    let overhead =
        recovery_ledger_bookkeeping_weight().saturating_add(recovery_guard_finalization_weight());
    let outcome = with_storage_layer(|| {
        meter
            .try_consume(overhead)
            .map_err(|_| DispatchError::Other("ledger recovery bookkeeping exceeds weight"))?;
        frame_support::ensure!(
            RecoveryLedgerRepairActive::get()
                && RecoveryLockdown::get()
                && RecoveryCodeApplied::get()
                && !RecoveryLedgerRepairFailed::get(),
            DispatchError::Other("ledger recovery state is not active")
        );
        let steps = RecoveryLedgerRepairSteps::get();
        frame_support::ensure!(
            steps < pallet_conditional_ledger::migration::MAX_BACKFILL_STEPS,
            DispatchError::Other("ledger recovery exceeded its release bound")
        );
        let cursor = RecoveryLedgerCursor::get();
        let next = <pallet_conditional_ledger::migration::BackfillTotalEscrowedV1<
            Runtime,
        > as SteppedMigration>::transactional_step(cursor, &mut meter)
        .map_err(|_| DispatchError::Other("ledger recovery step failed"))?;
        let next_steps = steps
            .checked_add(1)
            .ok_or(DispatchError::Other("ledger recovery step count overflow"))?;

        if let Some(cursor) = next {
            RecoveryLedgerCursor::put(cursor);
            RecoveryLedgerRepairSteps::put(next_steps);
            return Ok(());
        }

        frame_support::ensure!(
            ConditionalLedger::on_chain_storage_version() == StorageVersion::new(1)
                && pallet_conditional_ledger::TotalEscrowed::<Runtime>::exists(),
            DispatchError::Other("ledger recovery terminal state is incomplete")
        );
        let recovery = pallet_execution_guard::RecoveryImage::<Runtime>::get().ok_or(
            DispatchError::Other("ledger recovery commitment is missing"),
        )?;
        let mut installed = pallet_execution_guard::CurrentSpecName::<Runtime>::get().ok_or(
            DispatchError::Other("ledger recovery current spec is missing"),
        )?;
        installed.spec_version = recovery.target_spec_version;
        crate::ExecutionGuard::recovery_code_applied(recovery.hash, installed)?;
        complete_terminal_recovery_state();
        Ok::<(), DispatchError>(())
    });

    if outcome.is_err() {
        RecoveryLedgerRepairFailed::put(true);
        note_phase_transition_failure();
    }
    meter.consumed()
}

/// Keeps FRAME in `OnlyInherents` after the stuck cursor is transactionally
/// retired for code scheduling and until relay GoAhead installs the recovery
/// image. `RecoveryBypass` is scoped to the internal frame-system call only;
/// it is never externally writable.
pub struct RecoveryAwareMigrations;
impl frame_support::migrations::MultiStepMigrator for RecoveryAwareMigrations {
    fn ongoing() -> bool {
        if RecoveryBypass::get() {
            return false;
        }
        RecoveryLockdown::get()
            || PhaseTransitionLock::get()
            || <Migrations as frame_support::migrations::MultiStepMigrator>::ongoing()
    }

    fn step() -> Weight {
        let detector = recovery_aware_migration_detector_weight();
        #[cfg(feature = "recovery")]
        if RecoveryLedgerRepairActive::get() {
            if RecoveryLedgerRepairFailed::get() {
                return detector;
            }
            return detector.saturating_add(step_ledger_recovery());
        }
        if RecoveryLockdown::get() || PhaseTransitionLock::get() {
            detector
        } else {
            detector.saturating_add(
                <Migrations as frame_support::migrations::MultiStepMigrator>::step(),
            )
        }
    }
}

// The runtime stall-progress marker (`BleavitRuntimeMigration::ProgressMarker`)
// and its blake2 cursor hash were retired by B16 (SQ-132): the stall predicate
// now reads the SDK `cursor.started_at` directly (09 §3.2(d)(i)). The orphaned
// key is cleared by `crate::migrations::RetireB16State`.

const MIGRATION_FAILURE_HALT: u8 = 0b001;
pub(crate) const MIGRATION_STALL_HALT: u8 = 0b010;
const APPLIED_DETECTION_HALT: u8 = 0b100;
const UPGRADE_ABORT_TRIGGER: u8 = 0b1000;
const EXECUTION_HALT_SOURCES: u8 =
    MIGRATION_FAILURE_HALT | MIGRATION_STALL_HALT | APPLIED_DETECTION_HALT;

fn sync_execution_migration_halt(sources: u8) {
    pallet_execution_guard::MigrationHalt::<Runtime>::put(sources & EXECUTION_HALT_SOURCES != 0);
}

fn set_migration_halt_source(source: u8) {
    let (previous, sources) = MigrationHaltSources::mutate(|sources| {
        let previous = *sources;
        *sources |= source;
        (previous, *sources)
    });
    sync_execution_migration_halt(sources);
    // 09 §3.2(4): emit the `MigrationHalted` diagnostic on the *first* activation
    // of the halt — the transition from no execution-halt source to one —
    // carrying the SDK cursor's exact bytes and reported failed step. The event
    // is off the frozen 02 §6 ingest set by that section's (a)-(c) rule (an
    // operator/monitoring diagnostic, 12 §6.3), so it carries no contract bump.
    if previous & EXECUTION_HALT_SOURCES == 0 && sources & EXECUTION_HALT_SOURCES != 0 {
        emit_migration_halted();
    }
}

#[cfg(any(feature = "phase-four", feature = "recovery"))]
pub(crate) fn note_phase_transition_failure() {
    set_migration_halt_source(APPLIED_DETECTION_HALT);
}

#[cfg(feature = "recovery")]
pub(crate) fn complete_terminal_recovery_state() {
    RecoveryScheduledHash::kill();
    RetiredMigrationCursor::kill();
    RecoveryLedgerCursor::kill();
    RecoveryLedgerRepairActive::kill();
    RecoveryLedgerRepairSteps::kill();
    RecoveryLedgerRepairFailed::kill();
    RecoveryLockdown::kill();
    RecoveryAborted::kill();
    RecoveryCodeApplied::kill();
    PhaseTransitionLock::kill();
    PhaseTransitionApplied::kill();
    MigrationFailedStep::kill();
    clear_migration_halt_sources(
        MIGRATION_FAILURE_HALT
            | MIGRATION_STALL_HALT
            | APPLIED_DETECTION_HALT
            | UPGRADE_ABORT_TRIGGER,
    );
}

fn emit_migration_halted() {
    let cursor_bytes = pallet_migrations::Cursor::<Runtime>::get()
        .map(|cursor| cursor.encode())
        .unwrap_or_default();
    // The B16 type-bound regression proves the SDK cursor's MaxEncodedLen fits
    // this derived envelope, so `truncate_from` cannot truncate a real cursor.
    // A source-less halt yields empty bytes.
    let cursor = pallet_execution_guard::pallet::MigrationHaltCursor::truncate_from(cursor_bytes);
    crate::ExecutionGuard::note_migration_halted(cursor, MigrationFailedStep::get());
}

fn clear_migration_halt_sources(mask: u8) {
    let remaining = MigrationHaltSources::mutate(|sources| {
        *sources &= !mask;
        *sources
    });
    sync_execution_migration_halt(remaining);
}

pub(crate) fn active_migration_stall_is_live(
    cursor: &pallet_migrations::ActiveCursorOf<Runtime>,
) -> bool {
    // 09 §3.2(d): a migration is stalled iff its own start block is more than
    // MIGRATION_STALL_BLOCKS in the past — read from the SDK's own `started_at`
    // (SQ-132(d)(i)), never a runtime-maintained progress marker. It is a pure
    // function of state `pallet-migrations` already keeps, so a lawful migration
    // that drains a map while returning byte-identical cursors never false-raises.
    System::block_number().saturating_sub(cursor.started_at) > kernel::MIGRATION_STALL_BLOCKS
}

fn track_migration_progress() {
    match pallet_migrations::Cursor::<Runtime>::get() {
        Some(pallet_migrations::MigrationCursor::Active(cursor)) => {
            // Backstop only. Every registered MBM declares
            // `max_steps < MIGRATION_STALL_BLOCKS`, and their sum is likewise
            // bounded (integrity test, 09 §3.2(d)(ii)/(iii)), so the SDK's own
            // budget fires arm 1 (*failed migration step*) strictly before this
            // stall block is reached. This arm can fire only if that build-time
            // enforcement is bypassed.
            if active_migration_stall_is_live(&cursor) {
                set_migration_halt_source(MIGRATION_STALL_HALT);
            }
        }
        Some(pallet_migrations::MigrationCursor::Stuck) => {
            // The failure callback normally records this first. Seeing an
            // externally restored stuck cursor is still a machine trigger.
            set_migration_halt_source(MIGRATION_FAILURE_HALT);
        }
        None => {}
    }
}

pub(crate) fn migration_validation_hook_weight() -> Weight {
    // `remark_with_event` is the stable2606 benchmarked linear hash-of-bytes
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

pub(crate) fn dead_man_detector_hook_weight() -> Weight {
    // Worst case includes the one-time bounded MetricSpecs scan that seeds the
    // first schedule-derived deadline, plus the fixed relay/cause/flag writes.
    Weight::from_parts(50_000_000, 60_000)
        .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads_writes(24, 6))
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
        crate::ExecutionGuard::migration_completed();
        // MBM completion clears only migration failure/stall sources. An
        // applied-code mismatch remains halted until a later valid applied
        // callback resolves that condition. The additional try-state-before-
        // lift coupling is intentionally still an open specification question.
        clear_migration_halt_sources(MIGRATION_FAILURE_HALT | MIGRATION_STALL_HALT);
    }
}

/// One-read projection of the SDK-owned multi-block-migration cursor for the
/// application-time PB-MIGRATION anchor (09 section 3.2(2)).
pub struct RuntimeMigrationStatus;
impl pallet_execution_guard::MigrationStatusProvider for RuntimeMigrationStatus {
    fn cursor_exists() -> bool {
        pallet_migrations::Cursor::<Runtime>::exists()
    }

    fn recovery_state() -> pallet_execution_guard::MigrationRecoveryState {
        pallet_execution_guard::MigrationRecoveryState {
            lockdown: RecoveryLockdown::get(),
            bypass: RecoveryBypass::get(),
            retired_cursor: RetiredMigrationCursor::exists(),
            scheduled_hash: RecoveryScheduledHash::get(),
            aborted: RecoveryAborted::get(),
            recovery_code_applied: RecoveryCodeApplied::get(),
            phase_transition_lock: PhaseTransitionLock::get(),
            phase_transition_applied: PhaseTransitionApplied::get(),
        }
    }
}

impl pallet_migrations::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    #[cfg(all(not(feature = "runtime-benchmarks"), not(feature = "recovery")))]
    type Migrations = (pallet_conditional_ledger::migration::BackfillTotalEscrowedV1<Runtime>,);
    #[cfg(all(not(feature = "runtime-benchmarks"), feature = "recovery"))]
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

#[cfg(feature = "bootstrap")]
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
    type SchedulingSignatureVerifier = ();
}
impl staging_parachain_info::Config for Runtime {}

pub(crate) mod xcm_config {
    use super::*;
    use staging_xcm::latest::prelude::*;
    use staging_xcm_builder::{FixedWeightBounds, FrameTransactionalProcessor, WithUniqueTopic};
    use staging_xcm_executor::XcmExecutor;

    parameter_types! {
        pub RelayNetwork: Option<NetworkId> = Some(NetworkId::Polkadot);
        pub UniversalLocation: InteriorLocation = [
            GlobalConsensus(NetworkId::Polkadot),
            Parachain(staging_parachain_info::Pallet::<Runtime>::parachain_id().into()),
        ].into();
        pub UnitWeightCost: Weight = Weight::from_parts(1_000_000_000, 64 * 1024);
        pub const MaxInstructions: u32 = 100;
        pub const MaxAssetsIntoHolding: u32 = 64;
        pub const MaxPrefixes: u32 = 8;
        pub CheckingAccount: AccountId = PolkadotXcm::check_account();
    }
    pub type LocationToAccountId =
        bleavit_xcm::assets::StandardLocationToAccountId<AccountId, RelayNetwork>;
    pub type AssetTransactors = bleavit_xcm::assets::AssetTransactors<
        ForeignAssets,
        LocationToAccountId,
        AccountId,
        CheckingAccount,
    >;
    pub type CappedAssets = bleavit_xcm::caps::CappedInflows<
        AssetTransactors,
        PhaseInflowCaps,
        LocationToAccountId,
        AccountId,
    >;
    pub type TrapRecoveryAssets = bleavit_xcm::caps::TrapRecoveryInflows<
        AssetTransactors,
        PhaseInflowCaps,
        LocationToAccountId,
        AccountId,
    >;
    pub type ResponseHandler =
        bleavit_xcm::probe::ProbeAwareResponseHandler<PolkadotXcm, super::RuntimeOracleProbeSink>;
    pub type Barrier = bleavit_xcm::barrier::BleavitBarrier<
        ResponseHandler,
        UniversalLocation,
        MaxPrefixes,
        PhaseInflowCaps,
        LocationToAccountId,
        AccountId,
    >;
    pub type RelayRouter = cumulus_primitives_utility::ParentAsUmp<
        ParachainSystem,
        PolkadotXcm,
        polkadot_runtime_common::xcm_sender::NoPriceForMessageDelivery<()>,
    >;
    /// Parent traffic routes by UMP; sibling traffic (including the reserve
    /// probe to Asset Hub) routes by XCMP. The previous Parent-only sender made
    /// every sibling probe fail local validation (SQ-380).
    pub type NetworkRouter = (RelayRouter, XcmpQueue);
    pub type TopicRouter = WithUniqueTopic<NetworkRouter>;
    pub type Router = bleavit_xcm::health::HealthTrackingRouter<TopicRouter, XcmTrafficRecorder>;
    pub type BaseWeigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
    pub type Weigher =
        bleavit_xcm::probe::ProbeAwareWeightBounds<BaseWeigher, super::RuntimeProbeCallbackWeight>;

    /// Maps the Treasury-class execution origin to the protocol custody
    /// location under which protocol-owned local traps are keyed (09 §6.1).
    pub struct TreasuryOriginToLocation;
    impl sp_runtime::traits::TryConvert<RuntimeOrigin, Location> for TreasuryOriginToLocation {
        fn try_convert(origin: RuntimeOrigin) -> Result<Location, RuntimeOrigin> {
            let custom: Result<pallet_origins::Origin, RuntimeOrigin> = origin.clone().into();
            match custom {
                Ok(pallet_origins::Origin::FutarchyTreasury) => Ok(Location::new(
                    0,
                    [Junction::AccountId32 {
                        network: RelayNetwork::get(),
                        id: treasury_protocol_account().into(),
                    }],
                )),
                _ => Err(origin),
            }
        }
    }

    pub type LocalOriginToLocation = (
        TreasuryOriginToLocation,
        staging_xcm_builder::SignedToAccountId32<RuntimeOrigin, AccountId, RelayNetwork>,
    );

    pub struct XcmConfig<Assets = CappedAssets>(core::marker::PhantomData<Assets>);
    impl<Assets: staging_xcm_executor::traits::TransactAsset> staging_xcm_executor::Config
        for XcmConfig<Assets>
    {
        type RuntimeCall = RuntimeCall;
        // The Coretime route's local reserve withdrawal targets Parent, so the
        // production sender is the canonical parachain→relay UMP adapter.
        type XcmSender = Router;
        type XcmEventEmitter = PolkadotXcm;
        type AssetTransactor = Assets;
        type OriginConverter = ();
        type IsReserve = bleavit_xcm::assets::BleavitReserves;
        type IsTeleporter = ();
        type UniversalLocation = UniversalLocation;
        type Barrier = Barrier;
        type Weigher = Weigher;
        // Unrefunded fees use payer-adverse disposal until treasury revenue
        // routing is wired; this cannot create an unbacked claim.
        type Trader = bleavit_xcm::trader::GovernedWeightTrader<ConstitutionTraderRates, ()>;
        type ResponseHandler = ResponseHandler;
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
    /// Executor used by inbound DMP/XCMP transport: every reserve mint and
    /// beneficiary deposit passes through the live Phase-3 cap adapter.
    pub type Executor = XcmExecutor<XcmConfig<CappedAssets>>;
    /// `pallet-xcm` reconstructs an existing trapped imbalance by calling its
    /// configured executor's `mint_asset`, then immediately balances the clone
    /// so issuance is unchanged. The recovery transactor bypasses only that
    /// prospective global check; its beneficiary deposit remains capped and
    /// records the per-account cumulative meter.
    pub type TrapRecoveryExecutor = XcmExecutor<XcmConfig<TrapRecoveryAssets>>;
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
    type XcmRouter = xcm_config::Router;
    type ExecuteXcmOrigin =
        staging_xcm_builder::EnsureXcmOrigin<RuntimeOrigin, xcm_config::LocalOriginToLocation>;
    type XcmExecuteFilter = Nothing;
    type XcmExecutor = xcm_config::TrapRecoveryExecutor;
    type XcmTeleportFilter = Nothing;
    type XcmReserveTransferFilter = bleavit_xcm::filter::ReserveTransferFilter;
    type Weigher = xcm_config::Weigher;
    type UniversalLocation = xcm_config::UniversalLocation;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
    type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
    type Currency = Balances;
    type CurrencyMatcher = ();
    type TrustedLockers = ();
    type SovereignAccountOf = xcm_config::LocationToAccountId;
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
pub(crate) const METRIC_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(50), percent(60));
pub(crate) const METRIC_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_reciprocal(1, 14, percent(10), percent(2), percent(10));
pub(crate) const CONSTITUTION_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(67), percent(67));
pub(crate) const CONSTITUTION_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(5), percent(15));
pub(crate) const ENTRENCHED_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(80), percent(80));
pub(crate) const ENTRENCHED_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(10), percent(20));
pub(crate) const GUARDIAN_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(55), percent(55));
pub(crate) const GUARDIAN_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(5), percent(5));
pub(crate) const RATIFY_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(50), percent(50));
pub(crate) const RATIFY_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(5), percent(5));
pub(crate) const ORACLE_APPROVAL: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(60), percent(60));
pub(crate) const ORACLE_SUPPORT: pallet_referenda::Curve =
    pallet_referenda::Curve::make_linear(1, 1, percent(3), percent(10));
pub(crate) const TRACKS: [pallet_referenda::Track<u16, Balance, u32>; 6] = [
    pallet_referenda::Track {
        id: 0,
        info: pallet_referenda::TrackInfo {
            name: sp_runtime::str_array("metric"),
            max_deciding: 10,
            decision_deposit: 10_000 * currency::VIT,
            prepare_period: 2 * BLOCKS_PER_DAY,
            decision_period: 14 * BLOCKS_PER_DAY,
            confirm_period: 2 * BLOCKS_PER_DAY,
            min_enactment_period: 14 * BLOCKS_PER_DAY,
            min_approval: METRIC_APPROVAL,
            min_support: METRIC_SUPPORT,
        },
    },
    pallet_referenda::Track {
        id: 1,
        info: pallet_referenda::TrackInfo {
            name: sp_runtime::str_array("constitution"),
            max_deciding: 10,
            decision_deposit: 25_000 * currency::VIT,
            prepare_period: 2 * BLOCKS_PER_DAY,
            decision_period: 21 * BLOCKS_PER_DAY,
            confirm_period: 3 * BLOCKS_PER_DAY,
            min_enactment_period: 28 * BLOCKS_PER_DAY,
            min_approval: CONSTITUTION_APPROVAL,
            min_support: CONSTITUTION_SUPPORT,
        },
    },
    pallet_referenda::Track {
        id: 2,
        info: pallet_referenda::TrackInfo {
            name: sp_runtime::str_array("entrenched"),
            max_deciding: 10,
            decision_deposit: 50_000 * currency::VIT,
            prepare_period: 7 * BLOCKS_PER_DAY,
            decision_period: 28 * BLOCKS_PER_DAY,
            confirm_period: 7 * BLOCKS_PER_DAY,
            min_enactment_period: 4 * 21 * BLOCKS_PER_DAY,
            min_approval: ENTRENCHED_APPROVAL,
            min_support: ENTRENCHED_SUPPORT,
        },
    },
    pallet_referenda::Track {
        id: 3,
        info: pallet_referenda::TrackInfo {
            name: sp_runtime::str_array("guardian"),
            max_deciding: 10,
            decision_deposit: 5_000 * currency::VIT,
            prepare_period: BLOCKS_PER_DAY,
            decision_period: 7 * BLOCKS_PER_DAY,
            confirm_period: BLOCKS_PER_DAY,
            min_enactment_period: 2 * BLOCKS_PER_DAY,
            min_approval: GUARDIAN_APPROVAL,
            min_support: GUARDIAN_SUPPORT,
        },
    },
    pallet_referenda::Track {
        id: 4,
        info: pallet_referenda::TrackInfo {
            name: sp_runtime::str_array("ratify"),
            max_deciding: 10,
            decision_deposit: 1_000 * currency::VIT,
            prepare_period: BLOCKS_PER_DAY,
            decision_period: 7 * BLOCKS_PER_DAY,
            confirm_period: BLOCKS_PER_DAY,
            min_enactment_period: 0,
            min_approval: RATIFY_APPROVAL,
            min_support: RATIFY_SUPPORT,
        },
    },
    pallet_referenda::Track {
        id: 5,
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
        let scoped: Result<crate::track_origins::Origin, _> = origin.clone().try_into();
        if let Ok(scoped) = scoped {
            return Ok(match scoped {
                crate::track_origins::Origin::Metric => 0,
                crate::track_origins::Origin::Constitution => 1,
                crate::track_origins::Origin::Entrenched => 2,
                crate::track_origins::Origin::GuardianTrack => 3,
                crate::track_origins::Origin::Ratify => 4,
            });
        }
        #[cfg(feature = "runtime-benchmarks")]
        {
            // Upstream `pallet-referenda` benchmarks submit a proposal whose
            // enactment origin is Root. Map that fixture origin onto the
            // strongest values track (entrenched, id 2) in benchmark Wasm only;
            // no production track or origin mapping is added.
            let system: Result<frame_system::RawOrigin<AccountId>, _> = origin.clone().try_into();
            if matches!(system, Ok(frame_system::RawOrigin::Root)) {
                return Ok(2);
            }
        }
        let candidate: Result<pallet_origins::Origin, _> = origin.clone().try_into();
        match candidate {
            // Conservative backwards-compatible mapping for callers which have
            // not selected a scoped origin explicitly.
            Ok(pallet_origins::Origin::ConstitutionalValues) => Ok(2),
            Ok(pallet_origins::Origin::OracleResolution) => Ok(5),
            _ => Err(()),
        }
    }
}

pub trait AllowedValuesTracks {
    fn allows(origin: crate::track_origins::Origin) -> bool;
}

pub struct MetricTrack;
impl AllowedValuesTracks for MetricTrack {
    fn allows(origin: crate::track_origins::Origin) -> bool {
        matches!(origin, crate::track_origins::Origin::Metric)
    }
}

pub struct GuardianTrack;
impl AllowedValuesTracks for GuardianTrack {
    fn allows(origin: crate::track_origins::Origin) -> bool {
        matches!(origin, crate::track_origins::Origin::GuardianTrack)
    }
}

pub struct RatifyTrack;
impl AllowedValuesTracks for RatifyTrack {
    fn allows(origin: crate::track_origins::Origin) -> bool {
        matches!(origin, crate::track_origins::Origin::Ratify)
    }
}

pub struct EnsureValuesScoped<Allowed>(core::marker::PhantomData<Allowed>);
impl<Allowed: AllowedValuesTracks> frame_support::traits::EnsureOrigin<RuntimeOrigin>
    for EnsureValuesScoped<Allowed>
{
    type Success = ();

    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let legacy: Result<pallet_origins::Origin, RuntimeOrigin> = origin.clone().into();
        if matches!(legacy, Ok(pallet_origins::Origin::ConstitutionalValues)) {
            return Ok(());
        }
        let scoped: Result<crate::track_origins::Origin, RuntimeOrigin> = origin.clone().into();
        match scoped {
            Ok(track) if Allowed::allows(track) => Ok(()),
            _ => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(pallet_origins::Origin::ConstitutionalValues.into())
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

impl crate::track_origins::Config for Runtime {}

pub struct ConstitutionGovernanceOrigin;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for ConstitutionGovernanceOrigin {
    type Success = pallet_constitution::ConstitutionOrigin;
    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let scoped: Result<crate::track_origins::Origin, RuntimeOrigin> = origin.clone().into();
        if let Ok(track) = scoped {
            return match track {
                crate::track_origins::Origin::Constitution => Ok(Self::Success::ConstitutionTrack),
                crate::track_origins::Origin::Entrenched => Ok(Self::Success::EntrenchedTrack),
                _ => Err(origin),
            };
        }
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
/// 08 §4.2 minimum-viable-NAV admission for the 02 §7.3 arming bits (SQ-180).
///
/// The hard `ensure_nav_floor` variant is the right one here (SQ-381 resolution):
/// on a below-floor arming attempt 08 §4.2's loud signal *is* the extrinsic
/// failure carrying `NavFloorUnmet` — the `Err` fails the dispatch (surfaced
/// durably as `system::ExtrinsicFailed`, or bootstrap sudo's `Sudid { Err(..) }`
/// on the 09 §5.4 arming path) while leaving `PhaseFlags` unchanged (fail-static).
/// A pallet event cannot also survive the `Err` (FRAME rolls it back), and the
/// unchanged-flags requirement is exactly what mandates the `Err`. The
/// field-carrying `NavFloorUnmet { class, nav, floor }` event stays available on
/// the non-blocking, `Ok`-returning `flag_nav_floor` diagnostic variant (08 §4.4).
pub struct TreasuryPhaseArmingGate;
impl pallet_constitution::PhaseArmingGate for TreasuryPhaseArmingGate {
    fn ensure_armable(
        class: futarchy_primitives::ProposalClass,
    ) -> frame_support::dispatch::DispatchResult {
        FutarchyTreasury::ensure_nav_floor(class)
    }
}

impl pallet_constitution::Config for Runtime {
    type GovernanceOrigin = ConstitutionGovernanceOrigin;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type WeightInfo = crate::weights::pallet_constitution::WeightInfo<Runtime>;
    type PhaseArmingGate = TreasuryPhaseArmingGate;
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
fn live_balance_param(name: &[u8]) -> Option<Balance> {
    match live_param(pallet_constitution::key16(name)) {
        Some(pallet_constitution::ParamValue::Balance(value)) => Some(value),
        _ => None,
    }
}
fn live_u32_param(name: &[u8]) -> Option<u32> {
    match live_param(pallet_constitution::key16(name)) {
        Some(pallet_constitution::ParamValue::U32(value)) => Some(value),
        _ => None,
    }
}
fn live_u8_param(name: &[u8]) -> Option<u8> {
    match live_param(pallet_constitution::key16(name)) {
        Some(pallet_constitution::ParamValue::U8(value)) => Some(value),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct LiveReserveProbeEnvelope {
    fee: Balance,
    rate: Balance,
    interval: u32,
    timeout: u32,
    amount: Balance,
    fail_threshold: u8,
    recover_threshold: u8,
    runway: Balance,
}

fn live_reserve_probe_envelope() -> Option<LiveReserveProbeEnvelope> {
    let fee = live_balance_param(b"ops.probe_fee")?;
    let rate = live_balance_param(b"ops.probe_rate")?;
    let interval = live_u32_param(b"res.probe_int")?;
    let timeout = live_u32_param(b"res.probe_to")?;
    let amount = live_balance_param(b"res.probe_amount")?;
    let fail_threshold = live_u8_param(b"res.fail_thr")?;
    let recover_threshold = live_u8_param(b"res.recover_thr")?;
    if interval == 0 || timeout == 0 || amount == 0 || fail_threshold == 0 || recover_threshold == 0
    {
        return None;
    }
    let runway = pallet_futarchy_treasury::reserve_probe_runway_debit(
        fee,
        rate,
        fail_threshold,
        recover_threshold,
    )
    .ok()?;
    Some(LiveReserveProbeEnvelope {
        fee,
        rate,
        interval,
        timeout,
        amount,
        fail_threshold,
        recover_threshold,
        runway,
    })
}
pub(crate) fn balance_param(name: &[u8]) -> Balance {
    balance_param_or(name, 0)
}
fn balance_param_or(name: &[u8], default: Balance) -> Balance {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Balance(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Balance(value)) => value,
            _ => default,
        },
    }
}
/// Live 09 §6.1 DOT/USDC execution rates from constitution Params.
pub struct ConstitutionTraderRates;
impl bleavit_xcm::trader::TraderRates for ConstitutionTraderRates {
    fn dot_rate() -> bleavit_xcm::trader::WeightRate {
        bleavit_xcm::trader::WeightRate {
            units_per_second: balance_param(b"xcm.dot_per_sec"),
            units_per_megabyte: balance_param(b"xcm.dot_per_mb"),
        }
    }

    fn usdc_rate() -> bleavit_xcm::trader::WeightRate {
        bleavit_xcm::trader::WeightRate {
            units_per_second: balance_param(b"xcm.usdc_per_sec"),
            units_per_megabyte: balance_param(b"xcm.usdc_per_mb"),
        }
    }
}

/// Phase-3 caps are seeded as µUSDC (six decimals), the same base unit used
/// by the local sufficient USDC asset's issuance and account balances.
pub struct ConstitutionInflowCapParams;
impl pallet_inflow_caps::InflowCapParams for ConstitutionInflowCapParams {
    fn tvl_cap_usdc() -> u128 {
        balance_param(b"phase3.tvl_cap")
    }

    fn deposit_cap_usdc() -> u128 {
        balance_param(b"phase3.dep_cap")
    }
}

pub struct ForeignUsdcIssuance;
impl frame_support::traits::Get<u128> for ForeignUsdcIssuance {
    fn get() -> u128 {
        <ForeignAssets as Inspect<AccountId>>::total_issuance(usdc_location())
    }
}

impl pallet_inflow_caps::Config for Runtime {
    type CapParams = ConstitutionInflowCapParams;
    type UsdcIssuance = ForeignUsdcIssuance;
    type ProtocolAccounts = InflowCapProtocolAccounts;
}

/// 09 §5.2 XCM adapter over the shared on-chain meters.
pub struct PhaseInflowCaps;
impl bleavit_xcm::caps::InflowCaps<AccountId> for PhaseInflowCaps {
    fn usdc_mint_admissible(amount: u128) -> Result<(), ()> {
        pallet_inflow_caps::Pallet::<Runtime>::mint_admissible(amount)
    }

    fn note_usdc_inflow(who: &AccountId, amount: u128) -> Result<(), ()> {
        pallet_inflow_caps::Pallet::<Runtime>::note_inflow(who, amount)
    }

    fn usdc_inflow_admissible(who: &AccountId, amount: u128) -> Result<(), ()> {
        pallet_inflow_caps::Pallet::<Runtime>::inflow_admissible(who, amount)
            .then_some(())
            .ok_or(())
    }
}

/// Pure-read 09 §5.2 defense-in-depth gate for signed ledger splits.
pub struct RuntimeLedgerInflowCapGate;
impl pallet_conditional_ledger::InflowCapGate<AccountId> for RuntimeLedgerInflowCapGate {
    fn escrow_admissible(who: &AccountId) -> bool {
        pallet_inflow_caps::Pallet::<Runtime>::escrow_admissible(who)
    }
}
fn fixed_param(name: &[u8]) -> u64 {
    fixed_param_or(name, 0)
}
fn fixed_param_or(name: &[u8], default: u64) -> u64 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Fixed(value)) => value.0,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Fixed(value)) => value.0,
            _ => default,
        },
    }
}
/// Live `sec.flow_cap` (13 §1) clamped to its kernel hard minimum ×7.
///
/// The row's published value is Phase-0 sim-gated and deliberately absent from
/// the seeded genesis registry (`sec.*` stay out until calibrated — the same
/// fail-closed posture as the unpublished `sec.prize.*` floors, which
/// `in_cap_prize` renders as `None`). A ceiling has the opposite conservative
/// direction from a floor: `None` here would zero the contest term wrongly,
/// while any *large* default would widen step 9 — so an unpublished (or
/// sub-minimum) read collapses to exactly the kernel minimum 7, the smallest
/// admissible ceiling (08 §5.3; SQ-231).
#[allow(dead_code)]
pub(crate) fn sec_flow_cap_1e9() -> u64 {
    fixed_param(b"sec.flow_cap").max(kernel::SEC_FLOW_CAP_FLOOR_1E9)
}

fn u32_param(name: &[u8]) -> u32 {
    u32_param_or(name, 0)
}
fn u32_param_or(name: &[u8], default: u32) -> u32 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::U32(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::U32(value)) => value,
            _ => default,
        },
    }
}
fn perbill_param(name: &[u8]) -> u32 {
    perbill_param_or(name, 0)
}
fn perbill_param_or(name: &[u8], default: u32) -> u32 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Perbill(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Perbill(value)) => value,
            _ => default,
        },
    }
}
fn percent_param(name: &[u8]) -> u8 {
    percent_param_or(name, 0)
}
fn percent_param_or(name: &[u8], default: u8) -> u8 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::Percent(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Percent(value)) => value,
            _ => default,
        },
    }
}
fn u8_param(name: &[u8]) -> u8 {
    u8_param_or(name, 0)
}
fn u8_param_or(name: &[u8], default: u8) -> u8 {
    let key = pallet_constitution::key16(name);
    match live_param(key) {
        Some(pallet_constitution::ParamValue::U8(value)) => value,
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::U8(value)) => value,
            _ => default,
        },
    }
}

fn perbill_bps_param_or(name: &[u8], default_bps: u32) -> u32 {
    const PPB_PER_BPS: u32 = 100_000;
    let key = pallet_constitution::key16(name);
    let parts = match live_param(key) {
        Some(pallet_constitution::ParamValue::Perbill(value)) => Some(value),
        _ => match default_param(key) {
            Some(pallet_constitution::ParamValue::Perbill(value)) => Some(value),
            _ => None,
        },
    };
    parts.map_or(default_bps, |value| {
        (value / PPB_PER_BPS).saturating_add(u32::from(value % PPB_PER_BPS != 0))
    })
}

/// Live 07 §§4–8 oracle/reserve parameters. The constitution stores
/// `orc.bond_bps` as parts-per-billion (25_000_000 = 2.5%); the frame-free
/// oracle kernel consumes basis points (250), so convert at this adapter.
pub struct RuntimeOracleParams;
impl pallet_oracle::OracleParamsProvider for RuntimeOracleParams {
    fn get() -> pallet_oracle::OracleParams {
        let defaults = pallet_oracle::OracleParams::DEFAULT;
        pallet_oracle::OracleParams {
            window: u32_param_or(b"orc.window", defaults.window),
            rounds: u8_param_or(b"orc.rounds", defaults.rounds),
            bond_floor: balance_param_or(b"orc.bond_floor", defaults.bond_floor),
            bond_bps: perbill_bps_param_or(b"orc.bond_bps", defaults.bond_bps),
            reporter_stake: balance_param_or(b"orc.rep_stake", defaults.reporter_stake),
            watchtower_stake: balance_param_or(b"wt.stake", defaults.watchtower_stake),
            watchtower_quorum: u8_param_or(b"wt.quorum", defaults.watchtower_quorum),
            probe_interval: u32_param_or(b"res.probe_int", defaults.probe_interval),
            probe_timeout: u32_param_or(b"res.probe_to", defaults.probe_timeout),
            fail_threshold: u8_param_or(b"res.fail_thr", defaults.fail_threshold),
            recover_threshold: u8_param_or(b"res.recover_thr", defaults.recover_threshold),
            probe_amount: balance_param_or(b"res.probe_amount", defaults.probe_amount),
        }
    }
}

/// Live 06 §7 attestor economics and creation-time challenge window.
pub struct RuntimeAttestorParams;
impl pallet_attestor::AttestorParamsProvider for RuntimeAttestorParams {
    fn get() -> pallet_attestor::AttestorParams {
        let defaults = pallet_attestor::AttestorParams::DEFAULT;
        pallet_attestor::AttestorParams {
            bond: balance_param_or(b"att.bond", defaults.bond),
            challenge_window: u32_param_or(b"att.window", defaults.challenge_window),
        }
    }
}

/// Live 06 §5.4 retrospective-review deadline. The guardian core snapshots
/// this value when it creates a review record.
pub struct GuardianReviewDeadline;
impl Get<EpochId> for GuardianReviewDeadline {
    fn get() -> EpochId {
        u32_param_or(b"grd.review_dl", pallet_guardian::REVIEW_DEADLINE_EPOCHS)
    }
}

fn xcm_traffic_epoch_and_day() -> (EpochId, u8) {
    let info = pallet_epoch::EpochOf::<Runtime>::get();
    // The frozen EpochOf contract keeps epoch timing in the sibling live
    // schedule value; both are advanced atomically by pallet-epoch.
    let schedule = pallet_epoch::Schedule::<Runtime>::get();
    let now = System::block_number();
    let day = u8::try_from(now.saturating_sub(schedule.epoch_start_block) / BLOCKS_PER_DAY)
        .unwrap_or(u8::MAX);
    (info.index, day)
}

/// Fail-soft recorder for the three locally observable v1 XCM-health signals.
pub struct XcmTrafficRecorder;
impl bleavit_xcm::health::LocalXcmHealthSink for XcmTrafficRecorder {
    fn note_sent() {
        Self::record(pallet_welfare::XcmTrafficKind::Accepted);
    }

    fn note_send_failure() {
        Self::record(pallet_welfare::XcmTrafficKind::SendFailed);
    }

    fn note_probe_timeout() {
        Self::record(pallet_welfare::XcmTrafficKind::ProbeTimeout);
    }
}
impl XcmTrafficRecorder {
    fn record(kind: pallet_welfare::XcmTrafficKind) {
        let (epoch, day) = xcm_traffic_epoch_and_day();
        pallet_welfare::Pallet::<Runtime>::note_xcm_traffic(epoch, day, kind);
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
    pub const GuardianPalletId: PalletId = PalletId(*b"bl/guard");
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
pub(crate) fn guardian_account() -> AccountId {
    GuardianPalletId::get().into_account_truncating()
}
parameter_types! {
    pub GuardianAccount: AccountId = guardian_account();
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
fn is_canonical_protocol_account(who: &AccountId) -> bool {
    if is_reserved_market_account(who) {
        return true;
    }
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

/// Pure canonical predicate used inside the XCM inflow precheck. It performs
/// no storage reads, so the barrier's fixed execution budget remains honest.
pub struct InflowCapProtocolAccounts;
impl Contains<AccountId> for InflowCapProtocolAccounts {
    fn contains(who: &AccountId) -> bool {
        is_canonical_protocol_account(who)
    }
}

pub struct ProtocolAccounts;
impl Contains<AccountId> for ProtocolAccounts {
    fn contains(who: &AccountId) -> bool {
        is_canonical_protocol_account(who)
            // The refcounted index records ownership of live/retained market
            // accounts. Classification does not depend on this index: every
            // canonical future/present/past address is reserved above.
            || pallet_market::Pallet::<Runtime>::is_market_protocol_account(who)
    }
}
parameter_types! { pub InsuranceAccount: AccountId = insurance_account(); }
impl pallet_conditional_ledger::Config for Runtime {
    type Collateral = ForeignAssets;
    type UsdcAssetId = UsdcAssetId;
    type MarketAuthority = EnsureMarketAccount;
    type ResolveAuthority = EnsureEpochAccount;
    type SettleAuthority = EnsureWelfareAccount;
    type EmergencyPlaybookOrigin = pallet_origins::EnsureEmergencyPlaybook;
    type MinSplit = LedgerMinSplit;
    type PositionDeposit = LedgerPositionDeposit;
    type MaxPositionsPerAccount = ConstU32<{ bounds::MAX_ACCOUNT_POSITIONS }>;
    type ArchiveDelay = LedgerArchiveDelay;
    type ReapBatch = ConstU32<{ kernel::REAP_BATCH }>;
    type ProtocolAccounts = ProtocolAccounts;
    type InsuranceAccount = InsuranceAccount;
    type PalletId = LedgerPalletId;
    type KeeperRebate = FutarchyTreasury;
    type InflowCapGate = RuntimeLedgerInflowCapGate;
    type WeightInfo = crate::weights::pallet_conditional_ledger::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

const MARKET_ACCOUNT_PREFIX: [u8; 16] = *b"bleavit/mkt/v1\0\0";
const MARKET_BOOK_KIND: u8 = b'B';
const MARKET_FEES_KIND: u8 = b'F';

fn reserved_market_account(kind: u8, id: futarchy_primitives::MarketId) -> AccountId {
    let mut bytes = [0_u8; 32];
    bytes[..MARKET_ACCOUNT_PREFIX.len()].copy_from_slice(&MARKET_ACCOUNT_PREFIX);
    bytes[16] = kind;
    bytes[17..25].copy_from_slice(&id.to_le_bytes());
    AccountId::new(bytes)
}

pub(crate) fn is_reserved_market_account(who: &AccountId) -> bool {
    let bytes: &[u8] = who.as_ref();
    bytes[..MARKET_ACCOUNT_PREFIX.len()] == MARKET_ACCOUNT_PREFIX
        && matches!(bytes[16], MARKET_BOOK_KIND | MARKET_FEES_KIND)
        && bytes[25..].iter().all(|byte| *byte == 0)
}

pub(crate) fn market_book_account(id: futarchy_primitives::MarketId) -> AccountId {
    reserved_market_account(MARKET_BOOK_KIND, id)
}

pub(crate) fn market_fee_account(id: futarchy_primitives::MarketId) -> AccountId {
    reserved_market_account(MARKET_FEES_KIND, id)
}

pub struct RuntimeMarketAccounts;
impl pallet_market::MarketAccountProvider<AccountId> for RuntimeMarketAccounts {
    fn book(id: futarchy_primitives::MarketId) -> AccountId {
        market_book_account(id)
    }

    fn fees(id: futarchy_primitives::MarketId) -> AccountId {
        market_fee_account(id)
    }
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
/// §5.3; 13 `dec.v_min`): `max(dec.v_min(class), 2P)`. Both the grade adapter
/// and `FutarchyApi::decision_stats` call this helper, so the view can never
/// report a floor the grade does not enforce.
///
/// An **unavailable** prize proxy (SQ-173 leaves `in_cap_prize` unbacked for
/// every non-TREASURY class) keeps the base `dec.v_min` floor rather than
/// voiding the grade. The distinction is economic, not cosmetic: a missing
/// prize is a security-sizing *input* gap, not evidence that the book lacked
/// coverage or contest depth. At the sizing step, `decide` resolves that gap
/// through terminal T10 `Reject(SecuritySizing)`, with the intake bond fully
/// refunded. Voiding the grade instead would reach `Reject(NotDecisionGrade)`
/// first and slash 10% of the proposer's intake bond (06 §4; 08 §7) for an
/// input the chain, not the proposer, is missing.
///
/// The `2P` doubling saturates: it can only raise the floor, never wrap it
/// down into a permissive value.
pub(crate) fn effective_decision_contest_floor(
    proposal: &futarchy_primitives::Proposal<AccountId>,
    params: &pallet_epoch::CoreEpochParams,
) -> Balance {
    let base = params.v_min[proposal_class_index(proposal.class)];
    match <RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<AccountId>>::in_cap_prize(
        proposal,
    ) {
        Some(prize) => base.max(prize.saturating_mul(2)),
        None => base,
    }
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
                .map(|proposal| effective_decision_contest_floor(&proposal, params))
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
                .map(|proposal| effective_decision_contest_floor(&proposal, params))
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
        .contest_capital_blocks
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
#[cfg_attr(feature = "runtime-benchmarks", allow(unreachable_code))]
impl pallet_epoch::MarketAccess<AccountId> for RuntimeMarketAccess {
    fn open_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
        rerun: bool,
        seed_plan: Option<pallet_epoch::PolSeedPlan>,
    ) -> Result<futarchy_primitives::MarketSet, DispatchError> {
        let requires_gate_markets = seed_plan.map_or_else(
            || {
                proposal
                    .markets
                    .is_some_and(|markets| markets.gates.is_some())
            },
            |plan| plan.gate_b.is_some(),
        );
        #[cfg(feature = "runtime-benchmarks")]
        {
            // The epoch weights predate B5 calibration, but their fixtures
            // must still execute through the assembled runtime.  The sibling
            // market pallet benchmarks the bounded book writes; this adapter
            // supplies deterministic decision telemetry while preserving the
            // real conditional-ledger vault used by settlement.
            if !pallet_conditional_ledger::Vaults::<Runtime>::contains_key(proposal.id) {
                ConditionalLedger::create_vault(
                    RuntimeOrigin::signed(market_account()),
                    proposal.id,
                    proposal.metric_spec,
                )?;
            }
            let _ = rerun;
            return Ok(benchmark_market_set(
                proposal.id,
                proposal.epoch,
                requires_gate_markets,
            ));
        }

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
                    proposal.id,
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
                proposal.id,
                start,
                trailing_start,
                end,
            )?;
            return Ok(markets);
        }

        let seed_plan = seed_plan.ok_or(DispatchError::Other("funded POL seed plan missing"))?;

        if let Some(vault) = pallet_conditional_ledger::Vaults::<Runtime>::get(proposal.id) {
            if vault.spec != proposal.metric_spec {
                return Err(DispatchError::Other("proposal metric-spec vault mismatch"));
            }
        } else {
            ConditionalLedger::create_vault(
                RuntimeOrigin::signed(market_account()),
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
                proposal.id,
                start,
                trailing_start,
                end,
            )?;
            Ok::<_, DispatchError>(id)
        };

        let b = seed_plan.decision_b;
        // A8 fail-closed: the simulation-gated P/P_ref slope has no verified
        // on-chain P_ref backing yet. The normative floor is used; effective
        // v_min=2P still prevents under-sized adoption — owner Phase-0/SQ-177.
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
        let gates = if let Some(gate_b) = seed_plan.gate_b {
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
                    proposal.id,
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
                // 03 §7 R-4: Seed is the earliest point at which this
                // per-market account exists. Only the Baseline book custodies
                // plain USDC; its permanent floor makes Preserve custody
                // satisfiable when a retained sell fee is below min_balance.
                let asset = usdc_location();
                let source = pol_baseline_account();
                let book = market_book_account(id);
                let minimum_balance =
                    <ForeignAssets as Inspect<AccountId>>::minimum_balance(asset.clone());
                let book_balance =
                    <ForeignAssets as Inspect<AccountId>>::balance(asset.clone(), &book);
                if book_balance < minimum_balance {
                    let shortfall = minimum_balance.saturating_sub(book_balance);
                    let affordable = <ForeignAssets as Inspect<AccountId>>::reducible_balance(
                        asset.clone(),
                        &source,
                        Preservation::Preserve,
                        Fortitude::Polite,
                    );
                    if affordable >= shortfall {
                        // Best-effort by design (G-1): an unexpected transfer
                        // failure leaves only small Baseline sells unavailable,
                        // matching pre-B14 behavior. Propagating it would roll
                        // back the whole epoch tick and wedge every proposal in
                        // the batch, a strictly broader liveness failure.
                        let _ = <ForeignAssets as Mutate<AccountId>>::transfer(
                            asset,
                            &source,
                            &book,
                            shortfall,
                            Preservation::Preserve,
                        );
                    }
                }
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
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = proposal;
            return Ok(());
        }
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
                proposal.id,
                start,
                trailing_start,
                proposal.decide_at,
            )?;
        }
        pallet_market::Pallet::<Runtime>::register_decision_window(
            epoch_signed_origin(),
            markets.baseline,
            proposal.id,
            start,
            trailing_start,
            proposal.decide_at,
        )
    }

    fn force_rerun_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Result<(), DispatchError> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = proposal;
            return Ok(());
        }
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
                proposal.id,
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
            proposal.id,
            start,
            trailing_start,
            proposal.decide_at,
        )
    }

    fn resume_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
        previous_decide_at: BlockNumber,
    ) -> Result<(), DispatchError> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = (proposal, previous_decide_at);
            return Ok(());
        }
        let markets = proposal
            .markets
            .ok_or(DispatchError::Other("resumed market set missing"))?;
        let paused_for =
            proposal
                .decide_at
                .checked_sub(previous_decide_at)
                .ok_or(DispatchError::Arithmetic(
                    sp_runtime::ArithmeticError::Underflow,
                ))?;
        let mut ids = Vec::from([markets.accept, markets.reject, markets.baseline]);
        if let Some(gates) = markets.gates {
            ids.extend(gates);
        }
        for id in ids {
            pallet_market::Pallet::<Runtime>::shift_decision_window(
                epoch_signed_origin(),
                id,
                previous_decide_at,
                paused_for,
            )?;
        }
        Ok(())
    }

    fn close_markets(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Result<(), DispatchError> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = proposal;
            return Ok(());
        }
        let markets = proposal
            .markets
            .ok_or(DispatchError::Other("decided market set missing"))?;
        let mut proposal_books = Vec::from([markets.accept, markets.reject]);
        if let Some(gates) = markets.gates {
            proposal_books.extend(gates);
        }
        for id in proposal_books {
            pallet_market::Pallet::<Runtime>::consume_decision_windows(
                epoch_signed_origin(),
                id,
                proposal.id,
            )?;
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
        pallet_market::Pallet::<Runtime>::consume_decision_windows(
            epoch_signed_origin(),
            markets.baseline,
            proposal.id,
        )?;
        if !baseline_still_live {
            pallet_market::Pallet::<Runtime>::close(epoch_signed_origin(), markets.baseline)?;
        }
        Ok(())
    }

    fn seal_decision_window(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Result<(), DispatchError> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = proposal;
            return Ok(());
        }
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

    fn decision_windows_live(proposal: &futarchy_primitives::Proposal<AccountId>) -> bool {
        let Some(markets) = proposal.markets else {
            return false;
        };
        let mut ids = Vec::from([markets.accept, markets.reject, markets.baseline]);
        if let Some(gates) = markets.gates {
            ids.extend(gates);
        }
        ids.into_iter().all(|id| {
            pallet_market::DecisionWindowOwners::<Runtime>::get(id)
                .iter()
                .any(|owner| owner.0 == proposal.id && owner.3 == proposal.decide_at)
        })
    }

    fn baseline_market(epoch: EpochId) -> Option<futarchy_primitives::MarketId> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            return pallet_market::BaselineMarketOf::<Runtime>::get(epoch)
                .or_else(|| Some(9_000u64.saturating_add(u64::from(epoch))));
        }
        pallet_market::BaselineMarketOf::<Runtime>::get(epoch)
    }

    fn twap_full(market: futarchy_primitives::MarketId) -> Option<FixedU64> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            return Some(benchmark_quote(market));
        }
        let end = market_window_end(market)?;
        let (full, _) = pallet_market::Pallet::<Runtime>::registered_window_lengths(market, end)?;
        pallet_market::Pallet::<Runtime>::twap_at(market, end, full)
    }

    fn twap_full_at(market: futarchy_primitives::MarketId, end: BlockNumber) -> Option<FixedU64> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = end;
            return Some(benchmark_quote(market));
        }
        let (full, _) = pallet_market::Pallet::<Runtime>::registered_window_lengths(market, end)?;
        pallet_market::Pallet::<Runtime>::twap_at(market, end, full)
    }

    fn twap_trailing_at(
        market: futarchy_primitives::MarketId,
        end: BlockNumber,
        window: BlockNumber,
    ) -> Option<FixedU64> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = (end, window);
            return Some(benchmark_quote(market));
        }
        let _ = window;
        let (_, trailing) =
            pallet_market::Pallet::<Runtime>::registered_window_lengths(market, end)?;
        pallet_market::Pallet::<Runtime>::twap_at(market, end, trailing)
    }

    fn spot_at(market: futarchy_primitives::MarketId, end: BlockNumber) -> Option<FixedU64> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = end;
            return Some(benchmark_quote(market));
        }
        pallet_market::Pallet::<Runtime>::spot_at(market, end)
    }

    fn decision_grade(
        market: futarchy_primitives::MarketId,
        end: BlockNumber,
        role: pallet_epoch::BookRole,
        class: futarchy_primitives::ProposalClass,
        params: &pallet_epoch::CoreEpochParams,
    ) -> bool {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = (market, end, role, class, params);
            return true;
        }
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
        let Some((full_window, _)) =
            pallet_market::Pallet::<Runtime>::registered_window_lengths(market, end)
        else {
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
                    pallet_market::Pallet::<Runtime>::twap_at(market, end, full_window)
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
            full_window,
            coverage,
            convergence,
            contest,
            pol_floor,
            sanity,
        )
    }

    fn welfare_grade(
        market: futarchy_primitives::MarketId,
        end: BlockNumber,
        class: futarchy_primitives::ProposalClass,
        params: &pallet_epoch::CoreEpochParams,
    ) -> pallet_epoch::WelfareGrade {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = (market, end, class, params);
            pallet_epoch::WelfareGrade::Ok
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            use pallet_epoch::WelfareGrade;
            // 05 §5.2 tri-state welfare-book grade over the same facts the
            // boolean Decision-role grade folds. The reference partition
            // (reference model `grade_welfare_book`): the remediable-by-time
            // shortfalls — contest capital below the Ask-scaled class floor,
            // coverage below `dec.coverage`, a first stale event — grade
            // Insufficient; every other failure — sanity band, POL floor or
            // POL disturbed (incl. a voided contest accumulator), a second
            // stale event, non-convergence, an unsealed window, or any
            // unavailable read — grades Invalid (G-1, fail-closed).
            let Some(book) = pallet_market::Markets::<Runtime>::get(market) else {
                return WelfareGrade::Invalid;
            };
            if !matches!(
                book.kind,
                pallet_market::core_market::BookKind::Decision { .. }
            ) {
                return WelfareGrade::Invalid;
            }
            let Some(contest_floor) = contest_floor_for_grade(
                market,
                end,
                pallet_epoch::BookRole::Decision,
                class,
                params,
            ) else {
                return WelfareGrade::Invalid;
            };
            let Some(facts) = pallet_market::Pallet::<Runtime>::decision_grade_facts_at(
                market,
                end,
                params.decision_window,
                params.coverage_pct,
                params.delta_max,
                contest_floor,
                class_pol_floor(class),
                true,
            ) else {
                return WelfareGrade::Invalid;
            };
            if !facts.sane
                || !facts.sealed
                || !facts.pol_ok
                || !facts.contest_valid
                || facts.stale_events >= 2
                || !facts.converged
            {
                return WelfareGrade::Invalid;
            }
            if !facts.contest_ok || !facts.coverage_ok || facts.stale_events == 1 {
                return WelfareGrade::Insufficient;
            }
            WelfareGrade::Ok
        }
    }

    fn measured_depth(pid: futarchy_primitives::ProposalId) -> Option<Balance> {
        // B5 benchmarks need a realistic, read-free depth; the production path
        // below returns `None` when a backing read is unavailable so the B2
        // `decision_stats` view can tell "not measurable" from "depth is zero"
        // (a zero would render a fabricated measurement as observed data).
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = pid;
            Some(currency::USDC.saturating_mul(1_000_000))
        }
        // 05 §5.6 / 08 §5.2 (SQ-231): `L̂ = Σ pair POL depth +
        // min(min(contest_acc, contest_rej), sec.flow_cap · (b_acc + b_rej))`.
        // The shallower book is binding (§5.4); only b remains pair-summed.
        #[cfg(not(feature = "runtime-benchmarks"))]
        pallet_epoch::Proposals::<Runtime>::get(pid).and_then(|proposal| {
            let markets = proposal.markets?;
            let mut pol_depth = 0_u128;
            let mut pair_contest: Option<Balance> = None;
            let mut b_sum = 0_u128;
            for id in [markets.accept, markets.reject] {
                if !pallet_market::SeededMarkets::<Runtime>::contains_key(id) {
                    return None;
                }
                let book = pallet_market::Markets::<Runtime>::get(id)?;
                let pol = pallet_market::core_market::maker_loss_floor(book.b)?;
                let (window, _) = pallet_market::Pallet::<Runtime>::registered_window_lengths(
                    id,
                    proposal.decide_at,
                )?;
                let contest = pallet_market::Pallet::<Runtime>::average_contest_at(
                    id,
                    proposal.decide_at,
                    window,
                )?;
                pol_depth = pol_depth.checked_add(pol)?;
                pair_contest = Some(match pair_contest {
                    Some(binding) => binding.min(contest),
                    None => contest,
                });
                b_sum = b_sum.checked_add(book.b)?;
            }
            pallet_market::core_market::liquidity_hat(
                pol_depth,
                pair_contest?,
                sec_flow_cap_1e9(),
                b_sum,
            )
        })
    }

    fn published_flow_per_day(_: futarchy_primitives::ProposalId) -> Option<Balance> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            return Some(currency::USDC);
        }
        // A8 fail-closed telemetry deferral: None makes the decision kernel use
        // its specified L/2 fallback (08 §5.2) — owner Phase-3 calibration.
        None
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

fn live_pol_commitments() -> Result<Vec<Balance>, DispatchError> {
    // One bounded storage-value read. The market lifecycle maintains exact,
    // market-id-sorted amounts transactionally at seed/rerun/terminal/reap;
    // this replaces the former 196-key `Markets` scan on every sync caller.
    Ok(pallet_market::Pallet::<Runtime>::live_pol_commitments())
}

/// Mirror every seeded, still-live book into NAV. Baseline books are included
/// here because 08 §1.2 nets all live-book obligations; their only exemption is
/// from the *new proposal* `pol.budget_epoch` charge (08 §4.3).
pub struct RuntimePolCommitmentSync;

impl pallet_market::PolCommitmentSync for RuntimePolCommitmentSync {
    fn sync_pol_commitments() -> DispatchResult {
        crate::FutarchyTreasury::set_pol_commitments(live_pol_commitments()?)
    }

    fn pol_commitments_synced() -> bool {
        live_pol_commitments()
            .is_ok_and(|expected| crate::FutarchyTreasury::treasury().pol_commitments == expected)
    }
}

impl pallet_market::Config for Runtime {
    type WeightInfo = crate::weights::pallet_market::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
    type Fee = MarketFee;
    type ObsInterval = MarketObsInterval;
    type Kappa1e9 = MarketKappa;
    type MarketAdmin = EnsureEpochAccount;
    type EmergencyPlaybookOrigin = pallet_origins::EnsureEmergencyPlaybook;
    type ArchiveDelay = LedgerArchiveDelay;
    type PalletId = MarketPalletId;
    type MarketAccounts = RuntimeMarketAccounts;
    type KeeperRebate = FutarchyTreasury;
    type InDecisionWindow = RuntimeInDecisionWindow;
    type PolCommitmentSync = RuntimePolCommitmentSync;
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
                && match pallet_oracle::RoundSchedules::<Runtime>::get((
                    round.component,
                    round.epoch,
                    round.spec_version,
                ))
                .and_then(|schedule| {
                    pallet_oracle::stored_round_bond(schedule.round_one_bond, 1, schedule.round_cap)
                        .ok()
                }) {
                    // Malformed frozen schedule ⇒ the game's own B_1 is
                    // uncomputable; G-1 conservatively holds the decision.
                    None => true,
                    // 07 §12 merit floor = max(live `dis.merit_min`, frozen
                    // B_1) (SQ-158): the independent META lever can raise the
                    // bar above the game's B_1, but the `max` keeps it from ever
                    // dropping below the round-1 bond the challenger posted, so a
                    // lowering can never make censorship cheaper (R-7).
                    Some(frozen_b1) => round.bond >= frozen_b1.max(balance_param(b"dis.merit_min")),
                }
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

    fn review_window_closed(
        pid: futarchy_primitives::ProposalId,
        epoch: futarchy_primitives::EpochId,
        phase: futarchy_primitives::EpochPhase,
    ) -> bool {
        // The purpose-specific window is the only admissible source. Missing
        // post-B18 state fails closed; falling back to `grd.review_dl` would
        // silently restore the pre-B18 coupling.
        phase == futarchy_primitives::EpochPhase::Seed
            && pallet_epoch::GuardianReviewWindows::<Runtime>::get(pid)
                .is_some_and(|window| epoch >= window)
    }

    fn close_review_window(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        pallet_guardian::Pallet::<Runtime>::close_review_window(pid)
    }
}

pub struct RuntimeEpochAttestation;
#[cfg_attr(feature = "runtime-benchmarks", allow(unreachable_code))]
impl pallet_epoch::AttestationAccess for RuntimeEpochAttestation {
    fn present_and_quorate(
        pid: futarchy_primitives::ProposalId,
        artifact_hash: futarchy_primitives::H256,
    ) -> bool {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = (pid, artifact_hash);
            return true;
        }
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
    runtime_batch(proposal.payload_hash, proposal.payload_len)
}

/// Re-derive every call's guard domains and require each to be admissible for the
/// proposal's class — byte-for-byte the precondition the execution guard applies
/// inside `enqueue`.
///
/// 09 §1.1 states queue-time preconditions are "enforced by the decision path
/// **before** `enqueue` succeeds". Screening must therefore be a **superset** of
/// the guard's: otherwise a payload can pass screening, win Adopt, and then make
/// `epoch.decide(pid)` fail inside `with_storage_layer`, reverting the entire
/// decide on every attempt until the T20 stale path force-rejects it — 13 days of
/// market and a decided Adopt lost (SQ-308). Mirroring the check here makes
/// `decide` total.
///
/// `InternalRootApplyUpgrade` is excluded exactly as the guard excludes it: the
/// classifier matches `system.authorize_upgrade` only at top level, so nested in a
/// `utility.batch_all` it projects to the *apply* domain. 09 §2.1's multi-item
/// upgrade payload is expressible as multiple **top-level** calls, so the nested
/// form need not be admitted at all.
fn domains_admissible(
    class: futarchy_primitives::ProposalClass,
    calls: &pallet_execution_guard::pallet::RuntimeBatch<Runtime>,
) -> bool {
    use pallet_execution_guard::{BatchDispatcher, PhaseState};
    let phase_four = crate::configs::RuntimePhaseState::exact_phase_three()
        && crate::classifier::RuntimeDispatcher::phase_four_plan(class, calls).is_some();
    calls.iter().all(|call| {
        crate::classifier::RuntimeDispatcher::rederive_call(call).is_ok_and(|analysis| {
            analysis.domains.iter().all(|domain| {
                (pallet_execution_guard::domain_allowed(class, *domain)
                    || (phase_four
                        && *domain == pallet_execution_guard::CallDomain::Code
                        && crate::classifier::RuntimeDispatcher::recovery_image_descriptor(call)
                            .is_some()))
                    && !matches!(
                        domain,
                        pallet_execution_guard::CallDomain::InternalRootApplyUpgrade
                    )
            })
        })
    })
}

fn runtime_batch(
    payload_hash: futarchy_primitives::H256,
    payload_len: u32,
) -> Option<pallet_execution_guard::pallet::RuntimeBatch<Runtime>> {
    use pallet_execution_guard::Preimages;
    let bytes = RuntimePreimages::fetch(payload_hash, payload_len)?;
    if u32::try_from(bytes.len()).ok()? != payload_len {
        return None;
    }
    pallet_execution_guard::Pallet::<Runtime>::decode_batch(&bytes).ok()
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
        if !visit_runtime_leaves(call, &mut |leaf| {
            let addition = match leaf {
                RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
                    amount,
                    ..
                }) => *amount,
                RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::open_stream {
                    total,
                    ..
                }) => *total,
                RuntimeCall::FutarchyTreasury(
                    pallet_futarchy_treasury::Call::fund_budget_line { .. }
                    | pallet_futarchy_treasury::Call::cancel_stream { .. }
                    | pallet_futarchy_treasury::Call::issue_vit { .. }
                    | pallet_futarchy_treasury::Call::recover_foreign { .. }
                    | pallet_futarchy_treasury::Call::set_coretime_authority { .. }
                    // 05 §1.4 / 08 §1.4: the sweep moves USDC *into* NAV, so its
                    // derived Treasury ask is exactly zero — one of the two
                    // admissible zero-outflow Treasury leaves 05 §1.4 names.
                    | pallet_futarchy_treasury::Call::sweep_insurance { .. },
                ) => 0,
                // 05 §1.4 ask derivation (SQ-244/SQ-316): `claim_assets` moves
                // already-owned assets out of the trap register and creates **no**
                // treasury outflow, so its derived ask is exactly zero. This is
                // one of the two admissible zero-outflow Treasury leaves and MUST
                // NOT be generalized into "unknown leaves ask zero" — every other
                // unknown call still fails closed at the `_` arm below.
                RuntimeCall::PolkadotXcm(pallet_xcm::Call::claim_assets { .. }) => 0,
                // `claim_stream` is Signed-recipient-only and coretime renewal is
                // priced from live quote storage. Neither can be committed as a
                // statically-sized Treasury proposal outflow.
                RuntimeCall::FutarchyTreasury(
                    pallet_futarchy_treasury::Call::claim_stream { .. }
                    | pallet_futarchy_treasury::Call::execute_coretime_renewal { .. }
                    | pallet_futarchy_treasury::Call::note_coretime_quote { .. }
                    | pallet_futarchy_treasury::Call::prune_coretime_quote { .. }
                    | pallet_futarchy_treasury::Call::__Ignore(_, _),
                ) => return false,
                _ => return false,
            };
            let Some(updated) = ask.checked_add(addition) else {
                return false;
            };
            ask = updated;
            true
        }) {
            return None;
        }
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
        ) => visit_runtime_leaves(call, visit),
        #[cfg(feature = "bootstrap")]
        RuntimeCall::Sudo(
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

fn queued_pending_outflows() -> Result<Vec<Balance>, DispatchError> {
    let mut queue = pallet_execution_guard::Queue::<Runtime>::iter().collect::<Vec<_>>();
    queue.sort_by_key(|(pid, _)| *pid);
    let mut pending = Vec::new();
    let meter = RuntimeAdmissionMeter::TreasuryOutflow.key();
    for (pid, queued) in queue {
        if !queued.meters_declared.contains(&meter) {
            continue;
        }
        if !matches!(queued.class, futarchy_primitives::ProposalClass::Treasury) {
            return Err(DispatchError::Other(
                "treasury meter on non-treasury queue item",
            ));
        }
        let proposal = pallet_epoch::Proposals::<Runtime>::get(pid)
            .ok_or(DispatchError::Other("queued treasury proposal absent"))?;
        if proposal.payload_hash != queued.payload_hash
            || proposal.payload_len != queued.payload_len
        {
            return Err(DispatchError::Other(
                "queued treasury payload binding mismatch",
            ));
        }
        let calls = runtime_batch(queued.payload_hash, queued.payload_len)
            .ok_or(DispatchError::Other("queued treasury payload unavailable"))?;
        let amount = derived_treasury_ask(&calls).ok_or(DispatchError::Other(
            "queued treasury outflow cannot be derived",
        ))?;
        if amount != proposal.ask {
            return Err(DispatchError::Other("queued treasury ask mismatch"));
        }
        pending.push(amount);
    }
    Ok(pending)
}

pub struct RuntimePendingOutflowSync;

impl pallet_execution_guard::PendingOutflowSync for RuntimePendingOutflowSync {
    fn sync_pending_outflows() -> DispatchResult {
        // Queue is structurally capped at 32, below the treasury's 64-entry
        // mirror bound. Any rejection here therefore signals invariant drift.
        FutarchyTreasury::set_pending_outflows(queued_pending_outflows()?)
    }

    fn force_fail_static() -> bool {
        FutarchyTreasury::set_pending_outflows(Vec::from([Balance::MAX])).is_ok()
    }

    fn pending_outflows_synced() -> bool {
        queued_pending_outflows()
            .is_ok_and(|expected| FutarchyTreasury::treasury().pending_outflows == expected)
    }
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

fn recovery_descriptor_for_calls(
    calls: &[RuntimeCall],
    current_spec_version: u32,
) -> Result<Option<pallet_execution_guard::RecoveryImageDescriptor>, ()> {
    use pallet_execution_guard::BatchDispatcher;
    let mut primary = None;
    let mut recovery = None;
    for call in calls {
        if let Some(hash) = crate::classifier::RuntimeDispatcher::authorize_upgrade_hash(call) {
            if primary.replace(hash).is_some() {
                return Err(());
            }
        }
        if let Some(descriptor) =
            crate::classifier::RuntimeDispatcher::recovery_image_descriptor(call)
        {
            if recovery.replace(descriptor).is_some() {
                return Err(());
            }
        }
    }
    match (primary, recovery) {
        (None, None) => Ok(None),
        (Some(primary), Some(recovery))
            if recovery.hash != primary
                && recovery.len > 0
                && recovery.len <= pallet_preimage::MAX_SIZE
                && current_spec_version
                    .checked_add(2)
                    .is_some_and(|expected| recovery.target_spec_version == expected) =>
        {
            Ok(Some(recovery))
        }
        _ => Err(()),
    }
}

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

#[cfg_attr(feature = "runtime-benchmarks", allow(unreachable_code))]
impl pallet_epoch::ConstitutionAccess<AccountId> for RuntimeConstitutionAccess {
    fn required_bond(proposal: &futarchy_primitives::Proposal<AccountId>) -> Option<Balance> {
        required_proposal_bond(proposal)
    }

    fn static_check(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> pallet_epoch::StaticCheckDisposition {
        use pallet_epoch::StaticCheckDisposition;
        use pallet_execution_guard::{BatchDispatcher, Capabilities, RecoveryImages};
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
        let recovery = match recovery_descriptor_for_calls(
            &calls,
            proposal
                .version_constraint
                .as_ref()
                .map(|version| version.spec_version)
                .unwrap_or(u32::MAX),
        ) {
            Ok(recovery) => recovery,
            Err(()) => {
                return StaticCheckDisposition::Refund(
                    futarchy_primitives::RejectReason::ProcessHold,
                )
            }
        };
        if let Some(recovery) = recovery {
            let mut primary_hash = None;
            for call in &calls {
                if let Some(hash) =
                    crate::classifier::RuntimeDispatcher::authorize_upgrade_hash(call)
                {
                    if primary_hash.replace(hash).is_some() {
                        return StaticCheckDisposition::Refund(
                            futarchy_primitives::RejectReason::ProcessHold,
                        );
                    }
                }
            }
            let Some(primary_hash) = primary_hash else {
                return StaticCheckDisposition::Refund(
                    futarchy_primitives::RejectReason::ProcessHold,
                );
            };
            let Some(version_constraint) = proposal.version_constraint.clone() else {
                return StaticCheckDisposition::Refund(
                    futarchy_primitives::RejectReason::ProcessHold,
                );
            };
            let qualified =
                pallet_execution_guard::QualifiedRecoveryImages::<Runtime>::get(proposal.id);
            if <Preimage as QueryPreimage>::len(&Hash::from(recovery.hash)) != Some(recovery.len)
                || !RuntimePreimages::is_pinned(recovery.hash)
                || qualified
                    != Some(pallet_execution_guard::QualifiedRecoveryImage {
                        payload_hash: proposal.payload_hash,
                        primary_hash,
                        version_constraint,
                        descriptor: recovery,
                    })
                || <RuntimeAttestations as pallet_execution_guard::Attestations>::artifact_hash(
                    recovery.attestation_id,
                ) != Some(recovery.hash)
                || !<RuntimeAttestations as pallet_execution_guard::Attestations>::present_unrevoked_unchallenged(
                    recovery.attestation_id,
                )
                || !<RuntimeEpochAttestation as pallet_epoch::AttestationAccess>::present_and_quorate(
                    proposal.id,
                    recovery.hash,
                )
            {
                return StaticCheckDisposition::Refund(
                    futarchy_primitives::RejectReason::ProcessHold,
                );
            }
        }
        let footprint = crate::classifier::derive_resource_footprint(&calls);
        let footprint_failure = |error: crate::classifier::FootprintError| {
            if error == crate::classifier::FootprintError::Unclassifiable
                && proposal.resources.is_empty()
            {
                StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold)
            } else {
                StaticCheckDisposition::SlashAll(
                    futarchy_primitives::RejectReason::ConstitutionViolation,
                )
            }
        };
        // 05 §1/T4 requires every proposal payload to derive at least one
        // class domain. Empty batches and call carriers with no classifiable
        // leaf (for example an empty utility batch) are verifiable no-ops,
        // but 06 §4 reserves confiscation for constitution violations and
        // false resource declarations. Cancel and refund them before slot or
        // market allocation instead of fabricating a proposal class.
        let mut has_classifiable_domain = false;
        for call in &calls {
            let Ok(analysis) = crate::classifier::RuntimeDispatcher::rederive_call(call) else {
                return match footprint.as_ref() {
                    Err(error) => footprint_failure(*error),
                    Ok(_) => StaticCheckDisposition::Refund(
                        futarchy_primitives::RejectReason::ProcessHold,
                    ),
                };
            };
            has_classifiable_domain |= !analysis.domains.is_empty();
        }
        if !has_classifiable_domain {
            return footprint_failure(crate::classifier::FootprintError::Unclassifiable);
        }
        let phase_four_payload =
            crate::classifier::RuntimeDispatcher::phase_four_plan(proposal.class, &calls).is_some();
        if !phase_four_payload
            && !calls
                .iter()
                .all(|call| RuntimeCapabilities::call_enabled(proposal.class, call))
        {
            return StaticCheckDisposition::SlashAll(
                futarchy_primitives::RejectReason::ConstitutionViolation,
            );
        }
        // A verified false footprint is a culpable act (05 §2.1 T4) and is slashed
        // regardless of any co-occurring refundable fault. It is evaluated BEFORE the
        // refundable domain/ask arms below so a proposer cannot escape the 100%
        // false-declaration slash by *also* committing a refundable domain violation
        // (e.g. a domain-inadmissible payload that would otherwise refund at
        // `domains_admissible`): the false declaration is slashed first, and the
        // refundable arms are only reached once the declaration is known truthful
        // (SQ-480).
        let footprint = match footprint {
            Ok(footprint) => footprint,
            Err(error) => return footprint_failure(error),
        };
        let declared_matches_footprint = proposal
            .resources
            .iter()
            .all(|resource| footprint.iter().any(|derived| derived == resource))
            && footprint.iter().all(|resource| {
                proposal
                    .resources
                    .iter()
                    .any(|declared| declared == resource)
            });
        if !declared_matches_footprint {
            return StaticCheckDisposition::SlashAll(
                futarchy_primitives::RejectReason::ConstitutionViolation,
            );
        }
        // Mirror the guard's own `enqueue` domain preconditions so `decide` is total
        // (09 §1.1; SQ-308).
        //
        // Disposition is **refund**, not slash. 05 §2.1's T4 taxonomy is explicit
        // that "confiscation requires a verified culpable act" and that "the refund
        // arm is the default and the two slash arms are the enumerated exceptions".
        // This failure is in neither exception: the footprint has been verified to
        // match (checked just above), the capability check passed, and the call
        // re-derived cleanly. The only fault is a classifier projection artifact —
        // `authorize_upgrade` is matched by `is_sub_type` at top level only, so
        // nesting it inside the `utility.batch_all` wrapper that 05 §1.4 explicitly
        // blesses collapses it onto the *apply* domain. Slashing a proposer 100% for
        // using a permitted wrapper would be confiscation without a culpable act.
        if !domains_admissible(proposal.class, &calls) {
            return StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold);
        }
        if matches!(proposal.class, futarchy_primitives::ProposalClass::Treasury)
            && (derived_treasury_ask(&calls) != Some(proposal.ask)
                || Self::in_cap_prize(proposal).is_none())
        {
            return StaticCheckDisposition::Refund(futarchy_primitives::RejectReason::ProcessHold);
        }
        StaticCheckDisposition::Eligible
    }

    fn queue_time_check(proposal: &futarchy_primitives::Proposal<AccountId>) -> bool {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = proposal;
            return true;
        }
        matches!(
            Self::static_check(proposal),
            pallet_epoch::StaticCheckDisposition::Eligible
        ) && proposal.version_constraint
            == pallet_execution_guard::CurrentSpecName::<Runtime>::get()
            && proposal_calls(proposal).is_some_and(|calls| preview_batch_admission(&calls))
    }

    fn in_cap_prize(proposal: &futarchy_primitives::Proposal<AccountId>) -> Option<Balance> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = proposal;
            return Some(currency::USDC);
        }
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
            // step 9 — owner values/classifier envelope milestone (SQ-173).
            futarchy_primitives::ProposalClass::Param
            | futarchy_primitives::ProposalClass::Code
            | futarchy_primitives::ProposalClass::Meta
            | futarchy_primitives::ProposalClass::Constitutional => None,
        }
    }

    fn auxiliary_preimage(
        _proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Option<futarchy_primitives::H256> {
        // Full runtime images have a dedicated one-image qualification and
        // ownership path in execution-guard. Epoch owns only the ≤64 KiB
        // proposal payload pin; requesting the recovery image here would
        // double-pin it and reintroduce batched PoV/accounting ambiguity.
        None
    }

    fn ledger_frozen() -> bool {
        Self::phase_flags() & pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN != 0
    }

    fn phase_flags() -> u32 {
        crate::Constitution::phase_flags()
    }

    fn note_dead_man_engaged(engaged: bool) -> DispatchResult {
        crate::Constitution::note_dead_man_engaged(engaged)
    }

    fn active_metric_spec_version() -> Option<futarchy_primitives::MetricSpecVersion> {
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_welfare::Pallet::<Runtime>::active_snapshot_spec(epoch)
    }

    fn attestation_artifact(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Option<futarchy_primitives::H256> {
        use pallet_execution_guard::BatchDispatcher;
        let calls = proposal_calls(proposal)?;
        let mut artifact = None;
        for call in &calls {
            if let Some(hash) = crate::classifier::RuntimeDispatcher::authorize_upgrade_hash(call) {
                if artifact.replace(hash).is_some() {
                    return None;
                }
            }
        }
        Some(artifact.map_or(proposal.payload_hash, |hash| hash))
    }
}

/// Treasury-free runtime boundary for 08 §4.4. The budget uses the published
/// spendable NAV, which is already zero under reserve impairment and already
/// nets every existing obligation. Per-book predictions reuse market-core's
/// exact ceil-rounded seeding arithmetic, including gate books but excluding
/// the independently-funded Baseline book.
pub struct RuntimePolBudget;

impl pallet_epoch::PolBudget<AccountId> for RuntimePolBudget {
    fn epoch_budget() -> Balance {
        #[cfg(feature = "runtime-benchmarks")]
        {
            Balance::MAX
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            let nav = crate::FutarchyTreasury::nav().spendable_nav;
            Perbill::from_parts(perbill_param_or(
                b"pol.budget_epoch",
                pallet_constitution::POL_BUDGET_EPOCH_DEFAULT_PPB,
            ))
            .mul_floor(nav)
        }
    }

    fn proposal_seed_plan(
        proposal: &futarchy_primitives::Proposal<AccountId>,
    ) -> Option<pallet_epoch::PolSeedPlan> {
        let b = match proposal.class {
            futarchy_primitives::ProposalClass::Param => {
                balance_param_or(b"pol.b.param", pallet_constitution::POL_B_DEFAULTS[0])
            }
            futarchy_primitives::ProposalClass::Treasury => {
                balance_param_or(b"pol.b.trs", pallet_constitution::POL_B_DEFAULTS[1])
            }
            futarchy_primitives::ProposalClass::Code => {
                balance_param_or(b"pol.b.code", pallet_constitution::POL_B_DEFAULTS[2])
            }
            futarchy_primitives::ProposalClass::Meta => {
                balance_param_or(b"pol.b.meta", pallet_constitution::POL_B_DEFAULTS[3])
            }
            futarchy_primitives::ProposalClass::Constitutional => return None,
        };
        let decision = pallet_market::core_market::seed_headroom(b)
            .ok()?
            .checked_mul(2)?;
        let gate_required = pallet_epoch::requires_gate_markets(proposal.class);
        if gate_required {
            let gate_b = balance_param_or(b"pol.b_gate", pallet_constitution::POL_GATE_B_DEFAULT);
            let gates = pallet_market::core_market::seed_headroom(gate_b)
                .ok()?
                .checked_mul(4)?;
            decision
                .checked_add(gates)
                .map(|commitment| pallet_epoch::PolSeedPlan {
                    commitment,
                    decision_b: b,
                    gate_b: Some(gate_b),
                })
        } else {
            Some(pallet_epoch::PolSeedPlan {
                commitment: decision,
                decision_b: b,
                gate_b: None,
            })
        }
    }
}

pub struct RuntimeEpochPreimages;
impl pallet_epoch::PreimageAccess for RuntimeEpochPreimages {
    fn len(hash: futarchy_primitives::H256) -> Option<u32> {
        <RuntimePreimages as pallet_execution_guard::Preimages>::len(hash)
    }
    fn request(hash: futarchy_primitives::H256) -> DispatchResult {
        let hash = Hash::from(hash);
        if <Preimage as QueryPreimage>::len(&hash).is_none() {
            return Err(DispatchError::Other("epoch qualification preimage absent"));
        }
        <Preimage as QueryPreimage>::request(&hash);
        Ok(())
    }
    fn unrequest(hash: futarchy_primitives::H256) {
        <Preimage as QueryPreimage>::unrequest(&Hash::from(hash));
    }
}

pub struct RuntimeEpochWelfare;
impl pallet_epoch::WelfareSettlement for RuntimeEpochWelfare {
    fn gate_window_sampled(epoch: EpochId) -> bool {
        pallet_welfare::Pallet::<Runtime>::gate_window_sampled(epoch)
    }

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
    fn settle_baseline_void(cohort_epoch: EpochId) -> frame_support::dispatch::DispatchResult {
        pallet_welfare::Pallet::<Runtime>::settle_baseline_void(cohort_epoch)
    }
    fn prune(current_epoch: EpochId) -> frame_support::dispatch::DispatchResult {
        // 05 §3.3: cutoff e−19 removes exactly ≤ e−20 and retains one
        // capacity slot for the next snapshot.
        let cutoff =
            current_epoch.saturating_sub(pallet_welfare::MAX_SNAPSHOTS_BOUND.saturating_sub(1));
        pallet_welfare::Pallet::<Runtime>::prune(cutoff)
    }

    fn prune_xcm_traffic(current_epoch: EpochId) -> frame_support::dispatch::DispatchResult {
        let cutoff = current_epoch.saturating_sub(pallet_welfare::MAX_SNAPSHOTS_BOUND);
        pallet_welfare::Pallet::<Runtime>::prune_xcm_traffic(cutoff)?;
        // SQ-201 / 05 §3.3: cohort reap is not the only prune trigger. Tick
        // invokes this seam on every successful roll — including rolls that
        // settle no cohort — so it is the epoch-roll hook the snapshot/gate
        // window needs. The cutoff is the same `current - 19` used by `prune`
        // above, so this retires strictly nothing that reap would have kept.
        let history_cutoff =
            current_epoch.saturating_sub(pallet_welfare::MAX_SNAPSHOTS_BOUND.saturating_sub(1));
        pallet_welfare::Pallet::<Runtime>::prune_epoch_roll(history_cutoff)
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
            None => {
                ConditionalLedger::create_vault(RuntimeOrigin::signed(market_account()), pid, spec)
            }
        }
    }

    fn resolve(
        pid: futarchy_primitives::ProposalId,
        branch: futarchy_primitives::Branch,
    ) -> DispatchResult {
        ConditionalLedger::resolve(epoch_signed_origin(), pid, branch)
    }

    fn void(pid: futarchy_primitives::ProposalId) -> DispatchResult {
        frame_support::storage::with_storage_layer(|| {
            ConditionalLedger::void(epoch_signed_origin(), pid)?;
            pallet_market::Pallet::<Runtime>::observe_proposal_terminal(pid)
        })
    }
}

pub struct RuntimeProposalBond;
impl pallet_epoch::ProposalBondCurrency<AccountId> for RuntimeProposalBond {
    fn hold(who: &AccountId, amount: Balance) -> DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::transfer(
            usdc_location(),
            who,
            &epoch_account(),
            amount,
            Preservation::Expendable,
        )
        .map(|_| ())
    }

    fn release(who: &AccountId, amount: Balance) -> DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::transfer(
            usdc_location(),
            &epoch_account(),
            who,
            amount,
            Preservation::Expendable,
        )
        .map(|_| ())
    }

    fn slash_to_insurance(amount: Balance) -> DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::transfer(
            usdc_location(),
            &epoch_account(),
            &insurance_account(),
            amount,
            Preservation::Expendable,
        )
        .map(|_| ())
    }

    fn escrow_balance() -> Balance {
        <ForeignAssets as Inspect<AccountId>>::balance(usdc_location(), &epoch_account())
    }
}

impl pallet_epoch::Config for Runtime {
    type Params = RuntimeEpochParams;
    type Market = RuntimeMarketAccess;
    type Oracle = RuntimeEpochOracle;
    type Guardian = RuntimeEpochGuardian;
    type Attestation = RuntimeEpochAttestation;
    type Constitution = RuntimeConstitutionAccess;
    type PolBudget = RuntimePolBudget;
    type ProposalBond = RuntimeProposalBond;
    type Preimage = RuntimeEpochPreimages;
    type ExecutionGuard = RuntimeEpochExecutionGuard;
    type Welfare = RuntimeEpochWelfare;
    type Ledger = RuntimeEpochLedger;
    type KeeperRebate = FutarchyTreasury;
    type GuardianOrigin = pallet_origins::EnsureGuardianHold;
    type ExecutionGuardOrigin = EnsureExecutionGuardAccount;
    type VoidAuthority = pallet_origins::EnsureEmergencyPlaybook;
    type EmergencyPlaybookOrigin = pallet_origins::EnsureEmergencyPlaybook;
    type ConstitutionalValuesOrigin = pallet_origins::EnsureConstitutionalValues;
    type WeightInfo = crate::weights::pallet_epoch::WeightInfo<Runtime>;
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
#[allow(dead_code)]
fn xcm_health(counters: pallet_welfare::XcmTrafficCounters) -> FixedU64 {
    let total = u128::from(counters.accepted)
        .saturating_add(u128::from(counters.failed))
        .saturating_add(u128::from(counters.probe_timeouts));
    if total == 0 {
        return FixedU64(pallet_welfare::ONE);
    }

    // The 1e9-grid division floors, so rounding can only reduce reported
    // health. Every checked-arithmetic failure also falls back to zero rather
    // than fabricating an optimistic value.
    let value = u128::from(counters.accepted)
        .checked_mul(u128::from(pallet_welfare::ONE))
        .and_then(|numerator| numerator.checked_div(total))
        .and_then(|scaled| u64::try_from(scaled).ok())
        .map_or(0, |scaled| scaled);
    FixedU64(value)
}

#[allow(dead_code)]
fn metric_components(
    epoch: EpochId,
    spec_version: u16,
    counters: pallet_welfare::XcmTrafficCounters,
) -> Vec<pallet_welfare::ComponentValue> {
    let Some(specs) = pallet_welfare::MetricSpecs::<Runtime>::get(spec_version) else {
        return Vec::new();
    };
    let x = xcm_health(counters);
    specs
        .iter()
        .filter(|spec| {
            // Honor the 05 §4.3 source column: X is an on-chain counter input.
            // Registration already rejects a C_onchain spec with an attested
            // source (`source_matches_pillar`), so this is defense in depth
            // against emitting a computed value for an oracle-sourced game.
            spec.activation_epoch <= epoch
                && spec.pillar == pallet_welfare::Pillar::COnchain
                && spec.source == pallet_welfare::SourceClass::Onchain
        })
        .filter_map(|spec| {
            let value = match spec.id {
                futarchy_primitives::metric_ids::X => x,
                futarchy_primitives::metric_ids::R => {
                    // 07 §8 makes R probe-day-resolved and says absence is
                    // never healthy. The current reserve-unhealthy latch is
                    // fail-open before the first probe and recovery rewrites
                    // the apparent history, so v1 binds X only. R remains
                    // unbound until a day-resolved probe-outcome store exists;
                    // a registered R therefore fails the crank status-quo-safe,
                    // exactly like the other unavailable on-chain components.
                    return None;
                }
                // Inputs for every other registered component land with the
                // A8/values wiring. Welfare treats registered-but-missing input
                // as an error, failing the crank status-quo-safe instead of
                // fabricating health.
                _ => return None,
            };
            Some(pallet_welfare::ComponentValue { id: spec.id, value })
        })
        .collect()
}

/// Runtime metric projection. Local XCM traffic and final oracle components
/// are live. Every other unavailable registered input remains absent so the
/// welfare pallet rejects an incomplete snapshot (G-1).
pub struct RuntimeMetricInputs;
impl pallet_welfare::MetricInputs for RuntimeMetricInputs {
    fn onchain_components(epoch: EpochId, version: u16) -> Vec<pallet_welfare::ComponentValue> {
        let Some(specs) = pallet_welfare::MetricSpecs::<Runtime>::get(version) else {
            return Vec::new();
        };
        #[cfg(feature = "runtime-benchmarks")]
        {
            specs
                .iter()
                .filter(|spec| spec.activation_epoch <= epoch)
                .map(|spec| pallet_welfare::ComponentValue {
                    id: spec.id,
                    value: FixedU64(1_000_000_000),
                })
                .collect()
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            let mut components = metric_components(
                epoch,
                version,
                pallet_welfare::Pallet::<Runtime>::xcm_traffic_epoch(epoch),
            );
            components.extend(
                specs
                    .iter()
                    .filter(|spec| {
                        spec.activation_epoch <= epoch
                            && matches!(spec.source, pallet_welfare::SourceClass::Attested)
                    })
                    .filter_map(|spec| {
                        pallet_oracle::Pallet::<Runtime>::settled_component(spec.id, epoch, version)
                            .map(|settled| pallet_welfare::ComponentValue {
                                id: spec.id,
                                value: settled.value,
                            })
                    }),
            );
            components
        }
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
    fn daily_components(
        epoch: EpochId,
        day: u8,
        version: u16,
    ) -> Vec<pallet_welfare::ComponentValue> {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = day;
            pallet_welfare::MetricSpecs::<Runtime>::get(version)
                .into_iter()
                .flatten()
                .filter(|spec| spec.activation_epoch <= epoch)
                .map(|spec| pallet_welfare::ComponentValue {
                    id: spec.id,
                    value: FixedU64(1_000_000_000),
                })
                .collect()
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        metric_components(
            epoch,
            version,
            pallet_welfare::Pallet::<Runtime>::xcm_traffic(epoch, day),
        )
    }
}
pub struct WelfareLedger;
impl pallet_welfare::LedgerSettlement for WelfareLedger {
    fn settle_scalar(pid: u64, score: FixedU64) -> frame_support::dispatch::DispatchResult {
        frame_support::storage::with_storage_layer(|| {
            ConditionalLedger::settle_scalar(
                RuntimeOrigin::signed(welfare_settlement_account()),
                pid,
                score,
            )?;
            pallet_market::Pallet::<Runtime>::observe_proposal_terminal(pid)
        })
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
        frame_support::storage::with_storage_layer(|| {
            ConditionalLedger::settle_baseline(
                RuntimeOrigin::signed(welfare_settlement_account()),
                epoch,
                score,
            )?;
            pallet_market::Pallet::<Runtime>::observe_baseline_terminal(epoch)
        })
    }
    fn baseline_open(epoch: EpochId) -> bool {
        matches!(
            pallet_conditional_ledger::BaselineVaults::<Runtime>::get(epoch)
                .map(|vault| vault.state),
            Some(pallet_conditional_ledger::core_ledger::BaselineState::Open)
        )
    }
}

pub struct RuntimeSnapshotSchedule;
impl pallet_welfare::SnapshotSchedule for RuntimeSnapshotSchedule {
    fn snapshot_due(epoch: EpochId) -> Option<BlockNumber> {
        pallet_epoch::Pallet::<Runtime>::scheduled_epoch_end(epoch)
    }
}

impl pallet_welfare::Config for Runtime {
    type MetricGovernanceOrigin = EnsureValuesScoped<MetricTrack>;
    type Params = WelfareParams;
    type MetricInputs = RuntimeMetricInputs;
    type Ledger = WelfareLedger;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type SnapshotSchedule = RuntimeSnapshotSchedule;
    type KeeperRebate = FutarchyTreasury;
    type WeightInfo = crate::weights::pallet_welfare::WeightInfo<Runtime>;
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
            #[cfg(feature = "runtime-benchmarks")]
            {
                // B5 fixture value: 500,000 x 250 bps = 12,500 USDC,
                // strictly above the 10,000 `orc.bond_floor` — the variable
                // bond path, not the floor knee — without overflowing the
                // checked calculation.
                500_000 * currency::USDC
            }
            #[cfg(not(feature = "runtime-benchmarks"))]
            {
                // A8 fail-closed: 07 §6.1 freezes value-at-risk at Snapshot(m),
                // but no pallet currently stores that snapshot. Reading mutable
                // live vault escrow could only reduce a later reporter bond, so
                // price the report out until the oracle snapshot owner lands the
                // frozen backing (SQ-174).
                Balance::MAX
            }
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
    type Params = RuntimeOracleParams;
    type ProbeDispatch = RuntimeProbeDispatch;
    type ProbeTimeoutSink = OracleProbeTimeoutToWelfare;
    type ReserveHealthSink = RuntimeReserveHealthSink;
    type KeeperRebate = FutarchyTreasury;
    type MaxRoundCloseBatch = ConstU32<{ kernel::TICK_BATCH }>;
    type WeightInfo = crate::weights::pallet_oracle::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

/// Oracle timeout folds share the router recorder's attribution and remain
/// unable to affect the fail-static reserve transition that called the sink.
pub struct OracleProbeTimeoutToWelfare;
impl pallet_oracle::ProbeTimeoutSink for OracleProbeTimeoutToWelfare {
    fn probe_timed_out() {
        <XcmTrafficRecorder as bleavit_xcm::health::LocalXcmHealthSink>::note_probe_timeout();
    }
}

/// Authenticated Asset Hub response sink for the production XCM executor
/// (07 §8, SQ-380). Only the exact outstanding unflagged oracle id is exposed;
/// `ProbeAwareResponseHandler` additionally authenticates the sibling origin,
/// querier and high-bit partition before calling this sink.
pub struct RuntimeOracleProbeSink;
impl bleavit_xcm::probe::ProbeSink for RuntimeOracleProbeSink {
    fn pending_query_id() -> Option<u64> {
        let health = pallet_oracle::ReserveHealth::<Runtime>::get();
        health.pending_since.map(|_| health.last_query_id)
    }

    fn probe_result(query_id: u64, passed: bool) -> Weight {
        // A sibling-write refusal leaves the pending query and all three health
        // records unchanged inside the oracle's explicit storage layer. The
        // response then degrades through the ordinary timeout path (G-1).
        let _ = crate::Oracle::reserve_probe_result(query_id, passed);
        RuntimeProbeCallbackWeight::get()
    }
}

/// Pre-dispatch accounting for one bounded reserve-probe DOT envelope
/// (07 §8 / 08 §1.1, SQ-114). The XCM dispatcher wraps this debit and local
/// send validation in one storage layer: insufficient funding refuses the
/// send, while a locally rejected message rolls the debit back.
pub struct RuntimeProbeBudget;
impl bleavit_xcm::probe::ProbeBudget for RuntimeProbeBudget {
    fn ready_to_arm(params: &pallet_oracle::OracleParams) -> bool {
        let Some(live) = live_reserve_probe_envelope() else {
            return false;
        };
        // Require the exact live records consumed by the oracle snapshot. A
        // missing/wrongly-typed row must not arm through the provider's benign
        // standalone fallback defaults.
        if live.interval != params.probe_interval
            || live.timeout != params.probe_timeout
            || live.amount != params.probe_amount
            || live.fail_threshold != params.fail_threshold
            || live.recover_threshold != params.recover_threshold
        {
            return false;
        }
        crate::FutarchyTreasury::line_balance(pallet_futarchy_treasury::BudgetLine::OpsReserveProbe)
            >= live.runway
    }

    fn reserve_fee(probe_amount: Balance) -> Result<Balance, DispatchError> {
        // Post-arm attempts remain fail-static, but a malformed live envelope
        // must still refuse the actual debit/send instead of composing the
        // provider's benign fallback values into an unauthorized program.
        let live = live_reserve_probe_envelope()
            .filter(|live| live.amount == probe_amount)
            .ok_or(DispatchError::Other("reserve probe envelope unavailable"))?;
        crate::FutarchyTreasury::charge_reserve_probe_fee(live.fee, live.rate)?;
        Ok(live.fee)
    }
}

pub struct RuntimeBootstrapOpsFundingPolicy;
impl pallet_futarchy_treasury::BootstrapOpsFundingPolicy for RuntimeBootstrapOpsFundingPolicy {
    fn reserve_probe_ceiling() -> Option<Balance> {
        live_reserve_probe_envelope().map(|live| live.runway)
    }
}

pub struct RuntimeParaId;
impl Get<u32> for RuntimeParaId {
    fn get() -> u32 {
        staging_parachain_info::Pallet::<Runtime>::parachain_id().into()
    }
}

pub struct ProbeExecWeightBudget;
impl Get<Weight> for ProbeExecWeightBudget {
    fn get() -> Weight {
        xcm_config::UnitWeightCost::get()
            .saturating_mul(u64::from(xcm_config::MaxInstructions::get()))
    }
}

pub struct ProbeMaxResponseWeight;
impl Get<Weight> for ProbeMaxResponseWeight {
    fn get() -> Weight {
        RuntimeProbeCallbackWeight::get()
    }
}

/// Generated worst-case oracle callback plus both authentication reads: the
/// barrier's `expecting_response` check and the executor's `on_response` route.
pub struct RuntimeProbeCallbackWeight;
impl Get<Weight> for RuntimeProbeCallbackWeight {
    fn get() -> Weight {
        <crate::weights::pallet_oracle::WeightInfo<Runtime> as pallet_oracle::WeightInfo>::reserve_probe_result()
            .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads(2))
    }
}

/// 07 §8 / 08 §1.2 (SQ-205): carry a reserve-health transition to both owners of
/// its consequences — the constitution's 02 §7.3 bit-7 mirror and the treasury's
/// fail-static NAV haircut — as one indivisible act.
///
/// Ordering is deliberate but not load-bearing: the oracle invokes this inside
/// an explicit storage layer, so if the treasury write fails the constitution
/// write and the oracle transition unwind with it. 08 §1.2 ties `spendable_nav`
/// to exactly this flag, so a half-applied transition would leave `PhaseFlags`
/// and NAV disagreeing about solvency (R-7).
///
pub struct ReserveHealthToConstitutionAndTreasury;
impl pallet_oracle::ReserveHealthSink for ReserveHealthToConstitutionAndTreasury {
    fn reserve_health_changed(unhealthy: bool) -> frame_support::dispatch::DispatchResult {
        crate::Constitution::note_reserve_health(unhealthy)?;
        crate::FutarchyTreasury::set_reserve_impaired(unhealthy)?;
        Ok(())
    }
}

type RuntimeReserveHealthSink = ReserveHealthToConstitutionAndTreasury;

type RuntimeProbeDispatch = bleavit_xcm::probe::XcmProbeDispatcher<
    xcm_config::TopicRouter,
    RuntimeProbeBudget,
    ProbeExecWeightBudget,
    ProbeMaxResponseWeight,
    RuntimeParaId,
    XcmTrafficRecorder,
>;

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
        // amendment/SQ-175. Zero makes `file` and `close_epoch` reject with
        // `MilestoneTargetUnset` (07 §7 *Milestone normalization*: "until the
        // MetricSpec surface carries the field no milestone component may be
        // admitted"), so no milestone filing is admitted and no fabricated 0.0
        // aggregate ever reaches welfare (SQ-291).
        //
        // Benchmark-only exception, on the B5 precedent of benchmark seams with
        // zero pallets dropped: `define_benchmarks!` measures the
        // `MilestoneRegistry` instance, and every setup routes through `file()`
        // (`file_many` in `pallets/registry/src/benchmarking.rs`). A zero target
        // aborts each setup with `MilestoneTargetUnset` before anything is
        // measured, so weight generation for the whole instance would die
        // silently rather than loudly. This value is measurement scaffolding
        // only: `runtime-benchmarks` is never enabled in a release runtime, so
        // the fail-closed production posture above is unchanged.
        #[cfg(feature = "runtime-benchmarks")]
        {
            registry_core::MILESTONE_TARGET_POINTS
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            0
        }
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
        // SQ-117 (ruled 2026-07-21): the row is now genesis-seeded from the
        // 08 §6.2 fee basis (`kernel::KEEPER_REBATE_FEE_BASIS_USDC`), so the
        // rebate pipeline pays a real amount rather than zero. The seed value
        // still carries a 13 §1 `[VERIFY]` tag pending launch benchmarking. The
        // absent/wrong-kind fallback stays a conservative no-outflow zero (G-1)
        // rather than consulting `genesis_params()`, exactly as before.
        let key = pallet_constitution::key16(b"keeper.rebate");
        match live_param(key) {
            Some(pallet_constitution::ParamValue::Balance(value)) => value,
            _ => 0,
        }
    }

    fn coretime_dot_rate() -> Balance {
        balance_param(b"ops.ct_dot_rate")
    }

    fn reserve_probe_dot_rate() -> Balance {
        balance_param(b"ops.probe_rate")
    }

    fn coretime_fee_dot() -> Balance {
        balance_param(b"ops.ct_fee_dot")
    }

    fn coretime_quote_ttl() -> u32 {
        u32_param(b"ops.ct_quote_ttl")
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
            usdc_location(),
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
        <ForeignAssets as Inspect<AccountId>>::balance(usdc_location(), &source)
    }
}
/// Atomically synchronize pot-backed internal budget credit with real USDC
/// custody (08 §1.4). Unlike fail-soft rebate recording/payout, a failure here
/// must abort the entire `fund_budget_line` call.
pub struct TreasuryPotFunding;
impl pallet_futarchy_treasury::PotFunding<AccountId> for TreasuryPotFunding {
    fn fund(
        line: pallet_futarchy_treasury::PayoutLine,
        amount: Balance,
    ) -> frame_support::dispatch::DispatchResult {
        let destination = match line {
            pallet_futarchy_treasury::PayoutLine::Keeper => treasury_keeper_account(),
            pallet_futarchy_treasury::PayoutLine::Oracle => treasury_oracle_account(),
        };
        <ForeignAssets as Mutate<AccountId>>::transfer(
            usdc_location(),
            &crate::genesis::treasury_account(),
            &destination,
            amount,
            // 03 §7 R-4 / 08 §1.4: MAIN is a permanent custody account.
            // Bound funding by `main_balance - min_balance`; failure toward the
            // status quo is G-1-conservative and cannot reap MAIN.
            Preservation::Preserve,
        )
        .map(|_| ())
    }
}
/// 08 §1.2/§1.4 (SQ-207): the custody half of `sweep_insurance` — INSURANCE →
/// `MAIN`, and nowhere else.
///
/// `Preservation::Preserve` is normative, not defensive: INSURANCE is a
/// genesis-endowed permanent custody account under 03 §7 R-4, so at most
/// `balance − min_balance` is sweepable and an over-large request fails whole
/// instead of reaping the account (G-1).
pub struct TreasuryInsuranceSweep;
impl pallet_futarchy_treasury::InsuranceSweep for TreasuryInsuranceSweep {
    fn sweep(amount: Balance) -> frame_support::dispatch::DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::transfer(
            usdc_location(),
            &insurance_account(),
            &crate::genesis::treasury_account(),
            amount,
            Preservation::Preserve,
        )
        .map(|_| ())
    }
}

#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
pub struct CoretimeTreasuryLocation;
#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
impl Get<staging_xcm::latest::Location> for CoretimeTreasuryLocation {
    fn get() -> staging_xcm::latest::Location {
        staging_xcm::latest::Location::new(
            0,
            [staging_xcm::latest::Junction::AccountId32 {
                network: xcm_config::RelayNetwork::get(),
                id: treasury_protocol_account().into(),
            }],
        )
    }
}

#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
pub struct CoretimeFeeBudget;
#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
impl Get<Balance> for CoretimeFeeBudget {
    fn get() -> Balance {
        <TreasuryParams as pallet_futarchy_treasury::TreasuryParams>::coretime_fee_dot()
    }
}

#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
pub struct CoretimeRenewalAccount;
#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
impl Get<Option<[u8; 32]>> for CoretimeRenewalAccount {
    fn get() -> Option<[u8; 32]> {
        pallet_futarchy_treasury::CoretimeRenewalAccount::<Runtime>::get()
    }
}

#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
parameter_types! {
    // SQ-261: conservative B12-only XCM execution bounds. Replace with
    // measured Coretime route limits in the next treasury weight calibration.
    pub CoretimeRelayWeightLimit: Weight = Weight::from_parts(100_000_000_000, 1_048_576);
    pub CoretimeRemoteWeightLimit: Weight = Weight::from_parts(100_000_000_000, 1_048_576);
    pub CoretimeLocalWeightLimit: Weight = xcm_config::UnitWeightCost::get().saturating_mul(10);
}

#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
type ProductionRenewalDispatch = bleavit_xcm::coretime::XcmRenewalDispatcher<
    xcm_config::Executor,
    RuntimeCall,
    CoretimeTreasuryLocation,
    CoretimeFeeBudget,
    CoretimeRenewalAccount,
    CoretimeRelayWeightLimit,
    CoretimeRemoteWeightLimit,
    CoretimeLocalWeightLimit,
>;

#[cfg(test)]
std::thread_local! {
    static TEST_CORETIME_RENEWALS: core::cell::RefCell<Vec<(u32, Balance)>> =
        const { core::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub struct TestRenewalDispatch;
#[cfg(test)]
impl pallet_futarchy_treasury::RenewalDispatch for TestRenewalDispatch {
    fn dispatch_renewal(
        period_index: u32,
        amount: Balance,
    ) -> frame_support::dispatch::DispatchResult {
        TEST_CORETIME_RENEWALS.with(|calls| calls.borrow_mut().push((period_index, amount)));
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn take_test_coretime_renewals() -> Vec<(u32, Balance)> {
    TEST_CORETIME_RENEWALS.with(|calls| core::mem::take(&mut *calls.borrow_mut()))
}

#[cfg(feature = "runtime-benchmarks")]
pub struct BenchmarkRenewalDispatch;
// The live XCM executor needs custody and transport that the generated
// benchmark harness does not provide. The B12-only delta remains explicitly
// conservative pending SQ-261 calibration; bleavit-xcm exercises the real
// executor route and rollback behavior.
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
#[cfg(all(not(feature = "runtime-benchmarks"), not(test)))]
type RuntimeRenewalDispatch = ProductionRenewalDispatch;
#[cfg(all(not(feature = "runtime-benchmarks"), test))]
type RuntimeRenewalDispatch = TestRenewalDispatch;

pub struct RuntimeTreasuryPhase;
impl pallet_futarchy_treasury::TreasuryPhase for RuntimeTreasuryPhase {
    fn treasury_armed() -> bool {
        crate::Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::TREASURY_ARMED
            != 0
    }
}

impl pallet_futarchy_treasury::Config for Runtime {
    type TreasuryOrigin = pallet_origins::EnsureFutarchyTreasury;
    type CommunityDistributionOrigin = pallet_origins::EnsureFutarchyParam;
    type CommunityVesting = RuntimeCommunityVesting;
    type CommunityPot = CommunityDistributionPot;
    type CommunityDistributionAmount = CommunityDistributionAmount;
    type CommunityVestingDuration = CommunityVestingDuration;
    type CommunityMinVestedTransfer = CommunityMinVestedTransfer;
    type MaxCommunitySchedules = MaxCommunitySchedules;
    type Params = TreasuryParams;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type TreasuryPhase = RuntimeTreasuryPhase;
    type BootstrapOpsFundingPolicy = RuntimeBootstrapOpsFundingPolicy;
    type RenewalDispatch = RuntimeRenewalDispatch;
    type RebatePayout = TreasuryRebatePayout;
    type PotFunding = TreasuryPotFunding;
    type InsuranceSweep = TreasuryInsuranceSweep;
    type WeightInfo = crate::weights::pallet_futarchy_treasury::WeightInfo<Runtime>;
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

pub struct RuntimeAttestorProposalStatus;
impl pallet_attestor::AttestorProposalStatus for RuntimeAttestorProposalStatus {
    fn has_executed(pid: futarchy_primitives::ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid).is_some_and(|proposal| {
            matches!(
                proposal.state,
                futarchy_primitives::ProposalState::Executed
                    | futarchy_primitives::ProposalState::Measuring
                    | futarchy_primitives::ProposalState::Settled
            )
        })
    }

    fn is_terminal(pid: futarchy_primitives::ProposalId) -> bool {
        pallet_epoch::Proposals::<Runtime>::get(pid).is_some_and(|proposal| {
            matches!(
                proposal.state,
                futarchy_primitives::ProposalState::Executed
                    | futarchy_primitives::ProposalState::Measuring
                    | futarchy_primitives::ProposalState::Settled
                    | futarchy_primitives::ProposalState::Cancelled
                    | futarchy_primitives::ProposalState::Expired
                    | futarchy_primitives::ProposalState::Rejected(_)
            )
        })
    }
}
pub struct RuntimeGuardianTriggers;
impl pallet_guardian::GuardianTriggers for RuntimeGuardianTriggers {
    fn current() -> pallet_guardian::TriggerState {
        let phase_flags = crate::Constitution::phase_flags();
        let current_epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let gate_breach = pallet_welfare::GateBreachFlags::<Runtime>::get(current_epoch)
            .is_some_and(|flags| flags.s_breached || flags.c_breached);
        #[allow(unused_assignments, unused_mut)]
        let mut state = pallet_guardian::TriggerState {
            gate_breach,
            dead_man: phase_flags & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED != 0,
            reserve_health: phase_flags & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG
                != 0,
            ledger_drift: pallet_conditional_ledger::Pallet::<Runtime>::ledger_drifted(),
            // A relay abort preserves the old code and is not the 09 §3.2
            // halt-at-fault trigger. Only the execution-halt projection of a
            // failed/stalled/applied-invalid migration admits PB-MIGRATION.
            migration_halt: pallet_execution_guard::MigrationHalt::<Runtime>::get(),
            // An activation record is authorization state, never a trigger
            // source (06 §6.2). The target-specific pending VOID latch is read
            // by `oracle_deadlock` below.
            void_in_flight: pallet_epoch::PendingOracleVoids::<Runtime>::count() > 0,
            ..pallet_guardian::TriggerState::none()
        };
        // Benchmark Wasm must exercise every verified-trigger branch, but the
        // live reads above still execute first so the measured DB-op pattern
        // (PhaseFlags + CurrentEpoch + GateBreachFlags + MigrationHaltSources)
        // matches production — a constant-only bench arm would under-account
        // those reads at the next weight regeneration.
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _production_reads = state;
            state = pallet_guardian::TriggerState {
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
        state
    }

    fn oracle_deadlock(epoch: EpochId) -> bool {
        #[cfg(feature = "runtime-benchmarks")]
        {
            let _ = epoch;
            true
        }
        #[cfg(not(feature = "runtime-benchmarks"))]
        {
            pallet_epoch::PendingOracleVoids::<Runtime>::contains_key(epoch)
        }
    }
}
pub struct RuntimeGuardianEffects;

impl RuntimeGuardianEffects {
    fn dispatch_emergency(call: RuntimeCall) -> Result<(), DispatchError> {
        frame_support::ensure!(
            crate::classifier::RuntimeBaseCallFilter::contains_for(
                origins_core::Origin::EmergencyPlaybook,
                &call,
            ),
            DispatchError::Other("emergency playbook call is not admissible")
        );
        call.dispatch_bypass_filter(pallet_origins::Origin::EmergencyPlaybook.into())
            .map(|_| ())
            .map_err(|error| error.error)
    }

    fn dispatch_emergency_all(calls: Vec<RuntimeCall>) -> Result<(), DispatchError> {
        for call in calls {
            Self::dispatch_emergency(call)?;
        }
        Ok(())
    }

    /// Kernel-enumerated 06 §6.2 activation routine. Keeping construction
    /// separate from dispatch gives the conformance suite one exact surface
    /// to compare with the playbook table.
    pub(crate) fn playbook_calls(
        id: pallet_guardian::PlaybookId,
        expiry: BlockNumber,
        target: Option<EpochId>,
    ) -> Result<Vec<RuntimeCall>, DispatchError> {
        let now = System::block_number();
        let bounded_expiry = now
            .checked_add(kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Overflow,
            ))?;
        frame_support::ensure!(
            expiry >= now && expiry <= bounded_expiry,
            DispatchError::Other("playbook expiry exceeds kernel window")
        );

        let calls = match id {
            pallet_guardian::PlaybookId::Depeg => {
                frame_support::ensure!(
                    target.is_none(),
                    DispatchError::Other("unexpected playbook target")
                );
                let epoch_bound = now.checked_add(kernel::MIN_EPOCH_LENGTH_BLOCKS).ok_or(
                    DispatchError::Arithmetic(sp_runtime::ArithmeticError::Overflow),
                )?;
                frame_support::ensure!(
                    expiry <= epoch_bound,
                    DispatchError::Other("depeg expiry exceeds one epoch")
                );
                vec![RuntimeCall::Market(pallet_market::Call::freeze_creation {
                    expiry,
                })]
            }
            pallet_guardian::PlaybookId::Migration => {
                frame_support::ensure!(
                    target.is_none(),
                    DispatchError::Other("unexpected playbook target")
                );
                // stable2603 exposes only Root-only destructive cursor controls.
                // The safe recovery substrate is the automatic active-cursor
                // continuation plus source-scoped execution halt and ratified
                // remediation path above; fabricating Root here would widen
                // EmergencyPlaybook beyond the pre-ratified 06 §6.2 surface.
                return Err(DispatchError::Other(
                    "PB-MIGRATION cursor retry has no EmergencyPlaybook-safe runtime call",
                ));
            }
            pallet_guardian::PlaybookId::OracleVoid => {
                let epoch = target.ok_or(DispatchError::Other(
                    "oracle-void playbook requires target epoch",
                ))?;
                vec![RuntimeCall::Epoch(pallet_epoch::Call::void_cohort {
                    epoch,
                })]
            }
            pallet_guardian::PlaybookId::HaltIntake => {
                frame_support::ensure!(
                    target.is_none(),
                    DispatchError::Other("unexpected playbook target")
                );
                vec![RuntimeCall::Epoch(pallet_epoch::Call::set_intake_paused {
                    paused: true,
                    expiry: expiry.min(bounded_expiry),
                })]
            }
            pallet_guardian::PlaybookId::Reserve => {
                frame_support::ensure!(
                    target.is_none(),
                    DispatchError::Other("unexpected playbook target")
                );
                vec![RuntimeCall::ConditionalLedger(
                    pallet_conditional_ledger::Call::set_split_paused {
                        paused: true,
                        expiry,
                    },
                )]
            }
            pallet_guardian::PlaybookId::LedgerFreeze => {
                frame_support::ensure!(
                    target.is_none(),
                    DispatchError::Other("unexpected playbook target")
                );
                vec![
                    RuntimeCall::ConditionalLedger(pallet_conditional_ledger::Call::set_frozen {
                        frozen: true,
                    }),
                    RuntimeCall::Market(pallet_market::Call::set_frozen { frozen: true }),
                ]
            }
        };
        Ok(calls)
    }
}

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
                    .checked_add(GuardianReviewDeadline::get())
                    .ok_or(DispatchError::Arithmetic(
                        sp_runtime::ArithmeticError::Overflow,
                    ))?;
                let window = pallet_epoch::CurrentEpoch::<Runtime>::get()
                    .checked_add(1)
                    .ok_or(DispatchError::Arithmetic(
                        sp_runtime::ArithmeticError::Overflow,
                    ))?;
                Epoch::note_guardian_review_window(pid, deadline, window)
            }
            pallet_guardian::GuardianPower::ForceRerun { pid } => {
                Epoch::force_rerun_from_guardian(pid)
            }
            pallet_guardian::GuardianPower::PauseIntake { until } => {
                Epoch::set_intake_paused_internal(until)
            }
            pallet_guardian::GuardianPower::SuspendOnGate => {
                let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
                let breached = pallet_welfare::GateBreachFlags::<Runtime>::get(epoch)
                    .is_some_and(|flags| flags.s_breached || flags.c_breached);
                frame_support::ensure!(
                    breached,
                    DispatchError::Other("hard gate breach is not active")
                );
                ExecutionGuard::set_gate_suspension(epoch);
                Ok(())
            }
            pallet_guardian::GuardianPower::ActivatePlaybook {
                id, expiry, target, ..
            } => {
                if id == pallet_guardian::PlaybookId::OracleVoid {
                    let epoch = target.ok_or(DispatchError::Other(
                        "oracle-void playbook requires target epoch",
                    ))?;
                    frame_support::ensure!(
                        pallet_epoch::PendingOracleVoids::<Runtime>::contains_key(epoch),
                        DispatchError::Other("oracle-void target has no pending deadlock")
                    );
                }
                let calls = Self::playbook_calls(id, expiry, target)?;
                if id == pallet_guardian::PlaybookId::LedgerFreeze {
                    let _ = calls;
                    Self::set_live_conditioned_playbook(id, true)
                } else {
                    Self::dispatch_emergency_all(calls)
                }
            }
        }
    }

    fn revert_playbook(id: pallet_guardian::PlaybookId) -> Result<(), DispatchError> {
        let calls = match id {
            pallet_guardian::PlaybookId::Depeg => {
                Market::clear_creation_freeze();
                Vec::new()
            }
            pallet_guardian::PlaybookId::Migration | pallet_guardian::PlaybookId::OracleVoid => {
                Vec::new()
            }
            pallet_guardian::PlaybookId::HaltIntake => {
                vec![RuntimeCall::Epoch(pallet_epoch::Call::set_intake_paused {
                    paused: false,
                    expiry: 0,
                })]
            }
            pallet_guardian::PlaybookId::Reserve => vec![RuntimeCall::ConditionalLedger(
                pallet_conditional_ledger::Call::set_split_paused {
                    paused: false,
                    expiry: 0,
                },
            )],
            pallet_guardian::PlaybookId::LedgerFreeze => {
                return Self::set_live_conditioned_playbook(id, false);
            }
        };
        Self::dispatch_emergency_all(calls)
    }

    fn renew_playbook(id: pallet_guardian::PlaybookId) -> Result<(), DispatchError> {
        frame_support::ensure!(
            id == pallet_guardian::PlaybookId::LedgerFreeze,
            DispatchError::Other("only ledger-freeze is renewable")
        );
        if !<RuntimeGuardianTriggers as pallet_guardian::GuardianTriggers>::current().ledger_drift {
            return Self::set_live_conditioned_playbook(id, false);
        }
        if !Self::playbook_effect_matches(id, true) {
            return Self::set_live_conditioned_playbook(id, true);
        }
        frame_support::storage::with_storage_layer(|| {
            ConditionalLedger::extend_freeze_once()?;
            Market::extend_freeze_once()
        })
    }

    fn set_live_conditioned_playbook(
        id: pallet_guardian::PlaybookId,
        applied: bool,
    ) -> Result<(), DispatchError> {
        frame_support::ensure!(
            id == pallet_guardian::PlaybookId::LedgerFreeze,
            DispatchError::Other("playbook is not live-conditioned")
        );
        frame_support::storage::with_storage_layer(|| {
            let now = System::block_number();
            let ledger_applied = pallet_conditional_ledger::FrozenUntil::<Runtime>::get()
                .is_some_and(|until| now < until);
            let market_applied =
                pallet_market::FrozenUntil::<Runtime>::get().is_some_and(|until| now < until);
            let mut calls = Vec::new();
            if ledger_applied != applied {
                calls.push(RuntimeCall::ConditionalLedger(
                    pallet_conditional_ledger::Call::set_frozen { frozen: applied },
                ));
            }
            if market_applied != applied {
                calls.push(RuntimeCall::Market(pallet_market::Call::set_frozen {
                    frozen: applied,
                }));
            }
            Self::dispatch_emergency_all(calls)?;
            crate::Constitution::note_ledger_frozen(applied)
        })
    }

    fn playbook_effect_matches(id: pallet_guardian::PlaybookId, applied: bool) -> bool {
        if id != pallet_guardian::PlaybookId::LedgerFreeze {
            return false;
        }
        let now = System::block_number();
        let ledger = pallet_conditional_ledger::FrozenUntil::<Runtime>::get()
            .is_some_and(|until| now < until);
        let market = pallet_market::FrozenUntil::<Runtime>::get().is_some_and(|until| now < until);
        let constitution = crate::Constitution::phase_flags()
            & pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN
            != 0;
        ledger == applied && market == applied && constitution == applied
    }
}

pub struct RuntimeGuardianProposalVeto;
impl pallet_guardian::GuardianProposalVeto for RuntimeGuardianProposalVeto {
    fn uphold(pid: futarchy_primitives::ProposalId) -> Result<(), DispatchError> {
        Epoch::veto_upheld_from_review(pid)
    }
}

/// Real retrospective-review and recall submission. The guardian pallet moves
/// pro-rata slices from SeatBond holds into the sovereign before entering this
/// adapter; both stock-referenda deposits are placed immediately.
pub struct RuntimeGuardianScheduler;
impl pallet_guardian::GuardianReviewScheduler for RuntimeGuardianScheduler {
    fn review_deposit() -> Balance {
        SubmissionDeposit::get().saturating_add(1_000 * currency::VIT)
    }

    fn schedule_review(
        action: u32,
        verdict: pallet_guardian::ReviewVerdict,
    ) -> Result<u32, DispatchError> {
        let call = match verdict {
            pallet_guardian::ReviewVerdict::Ratify => {
                RuntimeCall::Guardian(pallet_guardian::Call::ratify_action { action_id: action })
            }
            pallet_guardian::ReviewVerdict::UpholdVeto => {
                RuntimeCall::Guardian(pallet_guardian::Call::uphold_veto { action_id: action })
            }
        };
        let proposal = <Preimage as StorePreimage>::bound(call)?;
        let values_origin: RuntimeOrigin = crate::track_origins::Origin::Ratify.into();
        let proposal_origin = Box::new(values_origin.caller().clone());
        let referendum = pallet_referenda::ReferendumCount::<Runtime>::get();
        Referenda::submit(
            RuntimeOrigin::signed(guardian_account()),
            proposal_origin,
            proposal,
            frame_support::traits::schedule::DispatchTime::After(0),
        )?;
        Referenda::place_decision_deposit(RuntimeOrigin::signed(guardian_account()), referendum)
            .map_err(|error| error.error)?;
        Ok(referendum)
    }

    fn cancel_review(referendum: u32) -> Result<(), DispatchError> {
        match pallet_referenda::ReferendumInfoFor::<Runtime>::get(referendum) {
            Some(pallet_referenda::ReferendumInfo::Ongoing(_)) => Referenda::cancel(
                pallet_origins::Origin::ConstitutionalValues.into(),
                referendum,
            ),
            Some(_) => Ok(()),
            None => Err(DispatchError::Other("guardian review referendum missing")),
        }
    }

    fn refund_review(referendum: u32) -> Result<(), DispatchError> {
        Referenda::refund_decision_deposit(RuntimeOrigin::signed(guardian_account()), referendum)?;
        Referenda::refund_submission_deposit(
            RuntimeOrigin::signed(guardian_account()),
            referendum,
        )?;
        Ok(())
    }
}
impl pallet_guardian::GuardianRecallScheduler for RuntimeGuardianScheduler {
    fn schedule_recall(action: u32, slash_pool: Balance) -> Result<u32, DispatchError> {
        let deposit = SubmissionDeposit::get().saturating_add(5_000 * currency::VIT);
        if slash_pool < deposit {
            return Err(DispatchError::Other("guardian recall slash pool too small"));
        }
        let call = RuntimeCall::Guardian(pallet_guardian::Call::recall { action_id: action });
        let proposal = <Preimage as StorePreimage>::bound(call)?;
        let values_origin: RuntimeOrigin = crate::track_origins::Origin::GuardianTrack.into();
        let proposal_origin = Box::new(values_origin.caller().clone());
        let referendum = pallet_referenda::ReferendumCount::<Runtime>::get();
        Referenda::submit(
            RuntimeOrigin::signed(guardian_account()),
            proposal_origin,
            proposal,
            frame_support::traits::schedule::DispatchTime::After(0),
        )?;
        Referenda::place_decision_deposit(RuntimeOrigin::signed(guardian_account()), referendum)
            .map_err(|error| error.error)?;
        <Balances as frame_support::traits::fungible::Mutate<AccountId>>::transfer(
            &guardian_account(),
            &crate::genesis::treasury_account(),
            slash_pool.saturating_sub(deposit),
            Preservation::Expendable,
        )?;
        Ok(referendum)
    }

    fn refund_recall(referendum: u32) -> Result<(), DispatchError> {
        Referenda::refund_decision_deposit(RuntimeOrigin::signed(guardian_account()), referendum)?;
        Referenda::refund_submission_deposit(
            RuntimeOrigin::signed(guardian_account()),
            referendum,
        )?;
        <Balances as frame_support::traits::fungible::Mutate<AccountId>>::transfer(
            &guardian_account(),
            &crate::genesis::treasury_account(),
            SubmissionDeposit::get().saturating_add(5_000 * currency::VIT),
            Preservation::Expendable,
        )?;
        Ok(())
    }

    fn forward_failed_recall_pool(amount: Balance) -> Result<(), DispatchError> {
        <Balances as frame_support::traits::fungible::Mutate<AccountId>>::transfer(
            &guardian_account(),
            &crate::genesis::treasury_account(),
            amount,
            Preservation::Expendable,
        )?;
        Ok(())
    }
}
impl pallet_guardian::Config for Runtime {
    type ValuesOrigin = EnsureValuesScoped<RatifyTrack>;
    type AdminOrigin = EnsureValuesScoped<GuardianTrack>;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type SovereignAccount = GuardianAccount;
    type CurrentEpoch = pallet_epoch::CurrentEpoch<Runtime>;
    type ProposalStatusProvider = RuntimeGuardianStatus;
    type TriggerProvider = RuntimeGuardianTriggers;
    type EffectDispatcher = RuntimeGuardianEffects;
    type ProposalVeto = RuntimeGuardianProposalVeto;
    type ReviewScheduler = RuntimeGuardianScheduler;
    type RecallScheduler = RuntimeGuardianScheduler;
    type ReviewDeadlineEpochs = GuardianReviewDeadline;
    type WeightInfo = crate::weights::pallet_guardian::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}
impl pallet_attestor::Config for Runtime {
    type ValuesOrigin = EnsureValuesScoped<GuardianTrack>;
    type RatifyOrigin = EnsureValuesScoped<RatifyTrack>;
    type Params = RuntimeAttestorParams;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type InsuranceAccount = InsuranceAccount;
    type ProposalStatus = RuntimeAttestorProposalStatus;
    type WeightInfo = crate::weights::pallet_attestor::WeightInfo<Runtime>;
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
    fn requires_ratification(pid: futarchy_primitives::ProposalId) -> Option<bool> {
        pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .map(|proposal| {
                matches!(
                    proposal.class,
                    futarchy_primitives::ProposalClass::Code
                        | futarchy_primitives::ProposalClass::Meta
                )
            })
    }
    fn recovery_qualification_context(
        pid: futarchy_primitives::ProposalId,
    ) -> Option<(
        futarchy_primitives::H256,
        futarchy_primitives::RuntimeVersionConstraint,
    )> {
        let proposal = pallet_epoch::IntakeProposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::Proposals::<Runtime>::get(pid))?;
        matches!(
            proposal.state,
            futarchy_primitives::ProposalState::Submitted
                | futarchy_primitives::ProposalState::Screening
                | futarchy_primitives::ProposalState::Qualified
                | futarchy_primitives::ProposalState::Trading
                | futarchy_primitives::ProposalState::Extended
        )
        .then_some(proposal.version_constraint)
        .flatten()
        .map(|version| (proposal.payload_hash, version))
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
    fn bind_ratification(
        pid: futarchy_primitives::ProposalId,
        referendum_index: u32,
    ) -> DispatchResult {
        let proposal = pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .ok_or(DispatchError::Other(
                "epoch proposal missing for ratification",
            ))?;
        frame_support::ensure!(
            matches!(
                proposal.class,
                futarchy_primitives::ProposalClass::Code | futarchy_primitives::ProposalClass::Meta
            ),
            DispatchError::Other("ratification binding requires CODE or META")
        );

        // A passed record is the only terminal form of this identity.  It is
        // enough to make a repeated proposer call idempotent; the original
        // referendum preimage was already checked when the record was enacted.
        if let Some(record) = pallet_execution_guard::Ratifications::<Runtime>::get(pid) {
            frame_support::ensure!(
                record.payload_hash == proposal.payload_hash
                    && record.referendum_index == referendum_index,
                DispatchError::Other("ratification binding mismatch")
            );
            return pallet_execution_guard::Pallet::<Runtime>::bind_ratification(
                pid,
                referendum_index,
            );
        }

        let info = pallet_referenda::ReferendumInfoFor::<Runtime>::get(referendum_index)
            .ok_or(DispatchError::Other("ratification referendum missing"))?;
        let status = match info {
            pallet_referenda::ReferendumInfo::Ongoing(status) => status,
            _ => {
                return Err(DispatchError::Other(
                    "ratification referendum is not ongoing",
                ))
            }
        };
        frame_support::ensure!(
            status.track == 4,
            DispatchError::Other("ratification referendum is not on the ratify track")
        );
        let expected_origin = RuntimeOrigin::from(crate::track_origins::Origin::Ratify)
            .caller()
            .clone();
        frame_support::ensure!(
            status.origin == expected_origin,
            DispatchError::Other("ratification referendum origin is not scoped")
        );

        let decode_call = |bytes: &[u8]| {
            // Referenda lookup preimages are backed by the generic 4 MiB
            // pallet-preimage bound, while this adapter's fixed weight only
            // covers the protocol's 64 KiB payload ceiling.  Reject oversized
            // bytes before SCALE decoding so a proposer cannot turn G-9 into
            // an undercharged multi-megabyte storage read/allocation.
            frame_support::ensure!(
                bytes.len() <= pallet_execution_guard::MAX_PAYLOAD_BYTES as usize,
                DispatchError::Other("ratification preimage too large")
            );
            RuntimeCall::decode_all(&mut &bytes[..])
                .map_err(|_| DispatchError::Other("ratification preimage is not exact"))
        };
        let call = match &status.proposal {
            Bounded::Inline(bytes) => decode_call(bytes.as_ref())?,
            Bounded::Lookup { hash, len } => {
                frame_support::ensure!(
                    *len <= pallet_execution_guard::MAX_PAYLOAD_BYTES,
                    DispatchError::Other("ratification preimage too large")
                );
                let bytes = <Preimage as QueryPreimage>::fetch(hash, Some(*len))
                    .map_err(|_| DispatchError::Other("ratification preimage unavailable"))?;
                decode_call(bytes.as_ref())?
            }
            Bounded::Legacy { .. } => {
                return Err(DispatchError::Other(
                    "legacy ratification preimage is not admissible",
                ))
            }
        };
        frame_support::ensure!(
            matches!(
                call,
                RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::ratify {
                    pid: call_pid,
                    referendum_index: call_index,
                }) if call_pid == pid && call_index == referendum_index
            ),
            DispatchError::Other("ratification referendum call does not bind proposal")
        );
        pallet_execution_guard::Pallet::<Runtime>::bind_ratification(pid, referendum_index)
    }

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
        let bytes = RuntimePreimages::fetch(payload_hash, proposal.payload_len)
            .ok_or(DispatchError::Other("epoch payload preimage missing"))?;
        let calls = pallet_execution_guard::Pallet::<Runtime>::decode_batch(&bytes)
            .map_err(|_| DispatchError::Other("epoch payload batch invalid"))?;
        // The same mirror screening applies (SQ-308). Screening should already have
        // rejected such a payload; failing here too keeps the adapter honest if a
        // future path reaches it without screening.
        frame_support::ensure!(
            domains_admissible(proposal.class, &calls),
            DispatchError::Other("epoch payload domain inadmissible for class")
        );
        let mut declared_domains = pallet_execution_guard::pallet::StoredDomains::default();
        let mut artifact = None;
        for call in &calls {
            let analysis = crate::classifier::RuntimeDispatcher::rederive_call(call)?;
            for domain in analysis.domains {
                if !declared_domains.contains(&domain) {
                    declared_domains
                        .try_push(domain)
                        .map_err(|_| DispatchError::Other("epoch payload domain bound"))?;
                }
            }
            if let Some(hash) = crate::classifier::RuntimeDispatcher::authorize_upgrade_hash(call) {
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
            ) && !pallet_attestor::Pallet::<Runtime>::is_revoked(record.id)
        })
    }
    fn has_quorum(
        pid: futarchy_primitives::ProposalId,
        artifact_hash: futarchy_primitives::H256,
    ) -> bool {
        crate::Attestor::has_quorum(pid, artifact_hash)
    }
    fn has_record_quorum(
        pid: futarchy_primitives::ProposalId,
        artifact_hash: futarchy_primitives::H256,
    ) -> bool {
        crate::Attestor::has_record_quorum(pid, artifact_hash)
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
        pallet_constitution::PhaseFlags::<Runtime>::get()
            & pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN
            != 0
    }
    fn dead_man_freeze_active() -> bool {
        pallet_constitution::PhaseFlags::<Runtime>::get()
            & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED
            != 0
    }
    fn gate_suspended() -> bool {
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_execution_guard::GateSuspension::<Runtime>::get() == Some(epoch)
            && pallet_welfare::GateBreachFlags::<Runtime>::get(epoch)
                .is_some_and(|flags| flags.s_breached || flags.c_breached)
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
            RuntimeCall::Constitution(pallet_constitution::Call::amend_registry {
                key,
                min,
                max,
                max_delta,
                cooldown_epochs,
            }) => {
                // 05 §1.4 T4 / 13 rule 7 (SQ-150): registry amendment is
                // META-only, but the capability row alone is insufficient.
                // Unknown keys, kernel-bounded rows and malformed metadata are
                // verifiable constitution violations and must fail at static
                // screening rather than survive until guarded dispatch.
                Self::enabled(class, pallet_constitution::Capability::AmendRegistry)
                    && pallet_constitution::Params::<Runtime>::get(*key).is_some_and(|record| {
                        record
                            .checked_amend(*min, *max, *max_delta, *cooldown_epochs)
                            .is_ok()
                    })
            }
            RuntimeCall::Constitution(pallet_constitution::Call::set_release_channel {
                ..
            }) => Self::enabled(class, pallet_constitution::Capability::SetReleaseChannel),
            RuntimeCall::System(frame_system::Call::authorize_upgrade { .. }) => {
                Self::enabled(class, pallet_constitution::Capability::AuthorizeUpgrade)
            }
            RuntimeCall::ExecutionGuard(
                pallet_execution_guard::Call::commit_recovery_image { .. },
            ) => {
                matches!(class, futarchy_primitives::ProposalClass::Code | futarchy_primitives::ProposalClass::Meta)
                    && Self::enabled(class, pallet_constitution::Capability::AuthorizeUpgrade)
            }
            RuntimeCall::FutarchyTreasury(
                pallet_futarchy_treasury::Call::create_community_schedule { .. },
            ) => matches!(class, futarchy_primitives::ProposalClass::Param),
            RuntimeCall::FutarchyTreasury(
                pallet_futarchy_treasury::Call::fund_budget_line { .. }
                | pallet_futarchy_treasury::Call::spend { .. }
                | pallet_futarchy_treasury::Call::open_stream { .. }
                | pallet_futarchy_treasury::Call::cancel_stream { .. }
                | pallet_futarchy_treasury::Call::issue_vit { .. }
                | pallet_futarchy_treasury::Call::recover_foreign { .. }
                | pallet_futarchy_treasury::Call::set_coretime_authority { .. }
                // 08 §1.2/§1.4 (SQ-207): the INSURANCE sweep is a TREASURY-class
                // decision like every other treasury act. Omitting it here made
                // `call_enabled` fall to the fail-closed `_` arm, which for
                // `CallDomain::Treasury` is `SlashAll(ConstitutionViolation)` —
                // a lawful sweep would have confiscated the whole intake bond.
                | pallet_futarchy_treasury::Call::sweep_insurance { .. },
            ) => Self::enabled(class, pallet_constitution::Capability::TreasurySpend),
            // 05 §1.4 class safety (SQ-244/SQ-316): the base call-filter projection
            // of `claim_assets` stays **Public** — a Signed origin reclaiming its own
            // self-keyed trap is 09 §6.1's ordinary path and must not need governance.
            // Belief-execution admission is gated separately and narrowly: the leaf is
            // payload-admissible only for TREASURY carrying the Treasury-spend
            // capability (06 §3.2). Without this explicit arm the call would fall to
            // the generic Public allowance below and let a PARAM/CODE/META payload
            // carry it — precisely the 06 §1 / I-8 class confusion.
            RuntimeCall::PolkadotXcm(pallet_xcm::Call::claim_assets { .. }) => {
                matches!(class, futarchy_primitives::ProposalClass::Treasury)
                    && Self::enabled(class, pallet_constitution::Capability::TreasurySpend)
            }
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
            ) => Self::call_enabled(class, call),
            #[cfg(feature = "bootstrap")]
            RuntimeCall::Sudo(
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
    fn fetch(hash: futarchy_primitives::H256, expected_len: u32) -> Option<Vec<u8>> {
        if expected_len > futarchy_primitives::kernel::MAX_BYTES {
            return None;
        }
        <Preimage as QueryPreimage>::fetch(&Hash::from(hash), Some(expected_len))
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

impl pallet_execution_guard::RecoveryImages for RuntimePreimages {
    fn len(hash: futarchy_primitives::H256) -> Option<u32> {
        <Preimage as QueryPreimage>::len(&Hash::from(hash))
    }
    fn fetch(hash: futarchy_primitives::H256, expected_len: u32) -> Option<Vec<u8>> {
        if expected_len > pallet_preimage::MAX_SIZE {
            return None;
        }
        <Preimage as QueryPreimage>::fetch(&Hash::from(hash), Some(expected_len))
            .ok()
            .map(Cow::into_owned)
    }
    fn is_pinned(hash: futarchy_primitives::H256) -> bool {
        <Preimage as QueryPreimage>::is_requested(&Hash::from(hash))
    }
    fn preflight_qualifies(code: &[u8]) -> bool {
        let Ok(code_len) = u32::try_from(code.len()) else {
            return false;
        };
        cumulus_pallet_parachain_system::HostConfiguration::<Runtime>::get()
            .is_some_and(|host| code_len <= host.max_code_size)
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

pub struct RuntimePhaseState;

fn sudo_key_storage_exists() -> bool {
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(&sp_io::hashing::twox_128(b"Sudo"));
    key[16..].copy_from_slice(&sp_io::hashing::twox_128(b"Key"));
    sp_io::storage::exists(&key)
}

impl pallet_execution_guard::PhaseState for RuntimePhaseState {
    fn exact_phase_three() -> bool {
        #[cfg(not(feature = "bootstrap"))]
        {
            false
        }
        #[cfg(feature = "bootstrap")]
        {
            let expected = pallet_constitution::PhaseFlagsValue::SHADOW_MODE
                | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT;
            pallet_constitution::PhaseFlags::<Runtime>::get() == expected
                && pallet_sudo::Key::<Runtime>::get().is_some()
        }
    }
    fn exact_phase_four() -> bool {
        pallet_constitution::PhaseFlags::<Runtime>::get()
            == pallet_constitution::PhaseFlagsValue::PARAM_ARMED
            && !sudo_key_storage_exists()
    }
    fn post_sudo_phase() -> bool {
        let flags = pallet_constitution::PhaseFlags::<Runtime>::get();
        flags & pallet_constitution::PhaseFlagsValue::PARAM_ARMED != 0
            && flags
                & (pallet_constitution::PhaseFlagsValue::SHADOW_MODE
                    | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT)
                == 0
            && !sudo_key_storage_exists()
    }

    fn class_execution_enabled(class: futarchy_primitives::ProposalClass) -> bool {
        let flags = pallet_constitution::PhaseFlags::<Runtime>::get();
        match class {
            futarchy_primitives::ProposalClass::Param => {
                flags & pallet_constitution::PhaseFlagsValue::PARAM_ARMED != 0
            }
            futarchy_primitives::ProposalClass::Treasury => {
                flags & pallet_constitution::PhaseFlagsValue::TREASURY_ARMED != 0
            }
            futarchy_primitives::ProposalClass::Code | futarchy_primitives::ProposalClass::Meta => {
                flags & pallet_constitution::PhaseFlagsValue::CODE_META_ARMED != 0
            }
            futarchy_primitives::ProposalClass::Constitutional => false,
        }
    }

    fn phase_four_plan_valid(plan: &pallet_execution_guard::PhaseFourPlan) -> bool {
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let now = System::block_number();
        [
            (
                pallet_constitution::key16(b"phase3.tvl_cap"),
                pallet_constitution::ParamValue::Balance(plan.tvl_cap),
            ),
            (
                pallet_constitution::key16(b"phase3.dep_cap"),
                pallet_constitution::ParamValue::Balance(plan.deposit_cap),
            ),
        ]
        .into_iter()
        .all(|(key, value)| {
            pallet_constitution::Params::<Runtime>::get(key).is_some_and(|record| {
                value.as_u128() > record.value.as_u128()
                    && record.checked_update(value, epoch, now).is_ok()
            })
        })
    }
}

pub struct EnsureCurrentSudoKey;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for EnsureCurrentSudoKey {
    type Success = AccountId;

    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        #[cfg(not(feature = "bootstrap"))]
        {
            Err(origin)
        }
        #[cfg(feature = "bootstrap")]
        {
            let raw: Result<frame_system::RawOrigin<AccountId>, RuntimeOrigin> = origin.into();
            raw.and_then(|raw| match raw {
                frame_system::RawOrigin::Signed(who)
                    if pallet_sudo::Key::<Runtime>::get().as_ref() == Some(&who) =>
                {
                    Ok(who)
                }
                other => Err(RuntimeOrigin::from(other)),
            })
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        #[cfg(not(feature = "bootstrap"))]
        {
            Err(())
        }
        #[cfg(feature = "bootstrap")]
        {
            pallet_sudo::Key::<Runtime>::get()
                .map(RuntimeOrigin::signed)
                .ok_or(())
        }
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
        _target_spec_version: u32,
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
        let mut bytes = channel.bytes;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_UPDATED_AT,
            System::block_number(),
        )?;
        write_release_u32(
            &mut bytes,
            pallet_constitution::RELEASE_CHANNEL_SPEC_VERSION,
            target_spec_version,
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
    fn on_upgrade_aborted(_target_spec_version: u32) -> DispatchResult {
        let channel = pallet_constitution::ReleaseChannel::<Runtime>::get();
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

    fn pending_upgrade_indication() -> (BlockNumber, bool) {
        let channel = pallet_constitution::ReleaseChannel::<Runtime>::get();
        (
            channel.pending_authorized_at(),
            channel.flags() & pallet_constitution::RELEASE_CHANNEL_FLAG_URGENT_UPGRADE != 0,
        )
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

/// Exact stable2606 pre-write checks (re-verified at the D-19 line move:
/// cumulus-pallet-parachain-system 0.29.0 is condition-for-condition identical)
/// performed by
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
    let bridge_matches = match pallet_execution_guard::PhaseFourBridge::<Runtime>::get() {
        pallet_execution_guard::PhaseFourBridgeState::Pending { code_hash, .. } => {
            code_hash == pending.hash
        }
        pallet_execution_guard::PhaseFourBridgeState::Scheduled { .. } => false,
        pallet_execution_guard::PhaseFourBridgeState::Unused
        | pallet_execution_guard::PhaseFourBridgeState::Consumed => true,
    };
    version_matches && preflight_passes && bridge_matches
}

fn scheduled_upgrade_abort_candidate() -> Option<futarchy_primitives::H256> {
    use cumulus_primitives_core::relay_chain::UpgradeGoAhead;

    let pending = pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get()?;
    if pallet_execution_guard::ScheduledUpgrade::<Runtime>::get() != Some(pending.hash)
        || !matches!(
            cumulus_pallet_parachain_system::UpgradeGoAhead::<Runtime>::get(),
            Some(UpgradeGoAhead::Abort)
        )
        || cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::exists()
    {
        return None;
    }
    Some(pending.hash)
}

fn installed_code_differs(expected: futarchy_primitives::H256) -> bool {
    sp_io::storage::get(sp_core::storage::well_known_keys::CODE)
        .map(|code| sp_io::hashing::blake2_256(&code) != expected)
        .unwrap_or(true)
}

enum RecoveryTrigger {
    Cursor(pallet_migrations::CursorOf<Runtime>),
    PhaseTransition,
}

fn recovery_trigger() -> Option<RecoveryTrigger> {
    let sources = MigrationHaltSources::get();
    match pallet_migrations::Cursor::<Runtime>::get() {
        Some(cursor @ pallet_migrations::MigrationCursor::Stuck)
            if sources & MIGRATION_FAILURE_HALT != 0 =>
        {
            Some(RecoveryTrigger::Cursor(cursor))
        }
        Some(pallet_migrations::MigrationCursor::Active(cursor)) => {
            (active_migration_stall_is_live(&cursor) && sources & MIGRATION_STALL_HALT != 0)
                .then_some(RecoveryTrigger::Cursor(
                    pallet_migrations::MigrationCursor::Active(cursor),
                ))
        }
        Some(_) => None,
        None if PhaseTransitionLock::get()
            && matches!(
                pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
                pallet_execution_guard::PhaseFourBridgeState::Scheduled { .. }
            )
            && sources & APPLIED_DETECTION_HALT != 0 =>
        {
            Some(RecoveryTrigger::PhaseTransition)
        }
        None => None,
    }
}

pub(crate) fn recovery_hook_weight(bytes: u32) -> Weight {
    // The recovery path performs the same bounded full-Wasm read, version
    // inspection, hash and preimage bookkeeping as the generated recovery
    // qualifier. Reuse that measured worst-case envelope so this mandatory
    // hook moves with the committed benchmark artifact and its >10% regression
    // gate instead of carrying an unmeasured runtime-local constant.
    <crate::weights::pallet_execution_guard::WeightInfo<Runtime> as pallet_execution_guard::WeightInfo>::qualify_recovery_image(bytes)
}

pub(crate) fn recovery_schedule_hook_weight(bytes: u32) -> Weight {
    // `schedule_committed_recovery_image` performs the full bounded
    // qualification/preimage path, then FRAME authorization and Cumulus code
    // scheduling, plus the runtime-local lockdown/cursor/receipt writes. The
    // mandatory inherent must pre-charge all of it (I-20); charging only the
    // qualifier undercounts a 4 MiB application by roughly an order of
    // magnitude. The final term conservatively covers the additional fixed
    // runtime-local reads/writes and the retired cursor proof not present in
    // the generated dispatch weights.
    recovery_hook_weight(bytes)
        .saturating_add(
            <<Runtime as frame_system::Config>::SystemWeightInfo as frame_system::WeightInfo>::authorize_upgrade(),
        )
        .saturating_add(
            <<Runtime as frame_system::Config>::SystemWeightInfo as frame_system::WeightInfo>::apply_authorized_upgrade(),
        )
        .saturating_add(
            <Runtime as frame_system::Config>::DbWeight::get().reads_writes(8, 8),
        )
        .saturating_add(Weight::from_parts(
            0,
            u64::from(bounds::MIGRATION_CURSOR_MAX_LEN),
        ))
}

fn schedule_committed_recovery_image() -> DispatchResult {
    if RecoveryLockdown::get() || RecoveryAborted::get() || RecoveryScheduledHash::exists() {
        return Ok(());
    }
    frame_support::storage::with_storage_layer(|| {
        let trigger = recovery_trigger().ok_or(DispatchError::Other("recovery trigger missing"))?;
        let (recovery, code) = crate::ExecutionGuard::prepare_recovery_image()?;
        parachain_upgrade_preflight(&code)?;

        // From this write until GoAhead/Abort, the wrapper keeps FRAME in
        // OnlyInherents even though frame-system requires the SDK cursor to be
        // absent while applying the authorization.
        RecoveryLockdown::put(true);
        match trigger {
            RecoveryTrigger::Cursor(cursor) => {
                RetiredMigrationCursor::put(cursor);
                pallet_migrations::Cursor::<Runtime>::kill();
            }
            RecoveryTrigger::PhaseTransition => {
                frame_support::ensure!(
                    !RetiredMigrationCursor::exists()
                        && !pallet_migrations::Cursor::<Runtime>::exists(),
                    DispatchError::Other("phase recovery cursor conflict")
                );
            }
        }
        RecoveryCodeApplied::kill();
        RecoveryBypass::put(true);
        let result = (|| {
            RuntimeCall::System(frame_system::Call::authorize_upgrade {
                code_hash: Hash::from(recovery.hash),
            })
            .dispatch_bypass_filter(RuntimeOrigin::root())
            .map_err(|error| error.error)?;
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code })
                .dispatch_bypass_filter(RuntimeOrigin::none())
                .map_err(|error| error.error)?;
            crate::ExecutionGuard::recovery_scheduled(recovery.hash)?;
            RecoveryScheduledHash::put(recovery.hash);
            Ok(())
        })();
        RecoveryBypass::kill();
        result
    })
}

fn recovery_upgrade_abort_candidate() -> Option<futarchy_primitives::H256> {
    use cumulus_primitives_core::relay_chain::UpgradeGoAhead;
    let hash = RecoveryScheduledHash::get()?;
    (matches!(
        cumulus_pallet_parachain_system::UpgradeGoAhead::<Runtime>::get(),
        Some(UpgradeGoAhead::Abort)
    ) && !cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::exists())
    .then_some(hash)
}

fn restore_recovery_cursor_after_abort() -> DispatchResult {
    if let Some(cursor) = RetiredMigrationCursor::get() {
        if pallet_migrations::Cursor::<Runtime>::exists() {
            return Err(DispatchError::Other("recovery cursor already present"));
        }
        pallet_migrations::Cursor::<Runtime>::put(cursor);
    } else if !PhaseTransitionLock::get() {
        return Err(DispatchError::Other("recovery cause missing"));
    }
    RecoveryScheduledHash::kill();
    RecoveryCodeApplied::kill();
    RecoveryAborted::put(true);
    // Keep RecoveryLockdown set: no ordinary call may observe the restored
    // half-migrated layout, and a relay-rejected image is never auto-retried.
    Ok(())
}

pub(crate) fn installed_code_identity() -> Option<(
    futarchy_primitives::H256,
    futarchy_primitives::RuntimeVersionConstraint,
)> {
    use pallet_execution_guard::BatchDispatcher;
    let code = sp_io::storage::get(sp_core::storage::well_known_keys::CODE)?;
    let hash = sp_io::hashing::blake2_256(&code);
    let version = crate::classifier::RuntimeDispatcher::observed_runtime_version(&code)?;
    Some((hash, version))
}

/// Cumulus calls this only after relay `GoAhead` has written the new `:code`.
/// Any missing/mismatched guard state raises PB-MIGRATION instead of claiming
/// that an untracked upgrade applied.
pub struct ExecutionGuardSystemEvent;
impl cumulus_pallet_parachain_system::OnSystemEvent for ExecutionGuardSystemEvent {
    fn on_validation_data(data: &cumulus_primitives_core::PersistedValidationData) {
        frame_system::Pallet::<Runtime>::register_extra_weight_unchecked(
            migration_validation_hook_weight().saturating_add(dead_man_detector_hook_weight()),
            DispatchClass::Mandatory,
        );
        // Called once by the mandatory parachain inherent before the
        // executive services the MBM cursor for this block. Reading the
        // cursor's persisted start block is O(1) storage and bounded by
        // CursorMaxLen.
        track_migration_progress();
        if let Some(expected) = recovery_upgrade_abort_candidate() {
            // The cheap relay/pending-code predicate is true; register the
            // full bounded `:code` read/hash before performing it.
            frame_system::Pallet::<Runtime>::register_extra_weight_unchecked(
                recovery_hook_weight(pallet_preimage::MAX_SIZE),
                DispatchClass::Mandatory,
            );
            if installed_code_differs(expected) {
                if restore_recovery_cursor_after_abort().is_err() {
                    set_migration_halt_source(APPLIED_DETECTION_HALT);
                }
                set_migration_halt_source(UPGRADE_ABORT_TRIGGER);
                return;
            }
        }
        let now = frame_system::Pallet::<Runtime>::block_number();
        let snapshot_overdue = pallet_welfare::Pallet::<Runtime>::snapshot_overdue(now)
            && !snapshot_close_blocked_by_pause();
        // The pallet seam accepts only the plain relay number (I-24); detector
        // failure leaves the already-latched status quo untouched.
        let _ = pallet_epoch::Pallet::<Runtime>::observe_dead_man(
            data.relay_parent_number,
            snapshot_overdue,
        );
        if let Some(expected) = scheduled_upgrade_abort_candidate() {
            frame_system::Pallet::<Runtime>::register_extra_weight_unchecked(
                recovery_hook_weight(pallet_preimage::MAX_SIZE),
                DispatchClass::Mandatory,
            );
            if installed_code_differs(expected) {
                let phase_transition = PhaseTransitionLock::get();
                if crate::ExecutionGuard::validation_code_aborted().is_ok() {
                    if phase_transition {
                        PhaseTransitionLock::kill();
                        PhaseTransitionApplied::kill();
                    }
                    // Guardian-visible incident trigger, intentionally not an
                    // execution-queue halt: the relay preserved status quo and a
                    // fresh normal proposal must remain possible.
                    set_migration_halt_source(UPGRADE_ABORT_TRIGGER);
                } else {
                    // A failed status-quo cleanup is itself a halt-worthy applied
                    // boundary mismatch; retain every pending record for review.
                    set_migration_halt_source(UPGRADE_ABORT_TRIGGER | APPLIED_DETECTION_HALT);
                }
                return;
            }
        }
        if let Some(recovery) = pallet_execution_guard::RecoveryImage::<Runtime>::get() {
            if recovery_trigger().is_some() {
                frame_system::Pallet::<Runtime>::register_extra_weight_unchecked(
                    recovery_schedule_hook_weight(recovery.len.min(pallet_preimage::MAX_SIZE)),
                    DispatchClass::Mandatory,
                );
                if schedule_committed_recovery_image().is_err() {
                    // The original cursor/anchor/commitment remain byte-identical
                    // because scheduling is one storage transaction.
                    set_migration_halt_source(MIGRATION_FAILURE_HALT);
                }
            }
        }
    }
    fn on_validation_code_applied() {
        // The callback's first proof is the installed `:code` identity. Charge
        // its maximum bounded read/hash before touching the bytes.
        frame_system::Pallet::<Runtime>::register_extra_weight_unchecked(
            recovery_hook_weight(pallet_preimage::MAX_SIZE),
            DispatchClass::Mandatory,
        );
        if let Some(hash) = RecoveryScheduledHash::get() {
            let valid = installed_code_identity().is_some_and(|(installed_hash, version)| {
                installed_hash == hash
                    && pallet_execution_guard::RecoveryImage::<Runtime>::get().is_some_and(
                        |recovery| {
                            recovery.hash == hash
                                && recovery.target_spec_version == version.spec_version
                                && pallet_execution_guard::CurrentSpecName::<Runtime>::get()
                                    .is_some_and(|current| current.spec_name == version.spec_name)
                        },
                    )
            });
            if valid {
                // Cumulus invokes this in the old runtime. The terminal
                // recovery profile performs its bounded repair and atomic
                // guard/channel finalization at the next block's
                // `on_runtime_upgrade`; both locks remain active until then.
                RecoveryCodeApplied::put(true);
            } else {
                set_migration_halt_source(APPLIED_DETECTION_HALT);
            }
            return;
        }
        let installed_identity = installed_code_identity();
        let installed_hash = installed_identity.as_ref().map(|(hash, _)| *hash);
        let valid = installed_identity.as_ref().is_some_and(|(hash, observed)| {
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some_and(
                |pending| {
                    let current = pallet_execution_guard::CurrentSpecName::<Runtime>::get();
                    *hash == pending.hash
                        && current.is_some_and(|current| {
                            observed.spec_name == current.spec_name
                                && observed.spec_version == pending.target_spec_version
                        })
                },
            )
        });
        let bridge = pallet_execution_guard::PhaseFourBridge::<Runtime>::get();
        if matches!(
            bridge,
            pallet_execution_guard::PhaseFourBridgeState::Pending { code_hash, .. }
                if installed_hash == Some(code_hash)
        ) {
            if !valid
                || frame_support::storage::with_storage_layer(|| {
                    let hash = installed_hash
                        .ok_or(DispatchError::Other("phase-four installed code missing"))?;
                    crate::ExecutionGuard::phase_four_scheduled(hash)?;
                    PhaseTransitionLock::put(true);
                    PhaseTransitionApplied::put(true);
                    Ok::<(), DispatchError>(())
                })
                .is_err()
            {
                set_migration_halt_source(APPLIED_DETECTION_HALT);
            }
            return;
        }
        if matches!(
            bridge,
            pallet_execution_guard::PhaseFourBridgeState::Scheduled { code_hash, .. }
                if installed_hash == Some(code_hash)
        ) || PhaseTransitionLock::get()
        {
            if valid && PhaseTransitionLock::get() {
                // The no-Sudo image's one-shot migration runs only at the next
                // block's `on_runtime_upgrade`, so preserve the OnlyInherents
                // lock and leave guard pending state intact for that atomic
                // transition.
                PhaseTransitionApplied::put(true);
            } else {
                set_migration_halt_source(APPLIED_DETECTION_HALT);
            }
            return;
        }
        if matches!(
            bridge,
            pallet_execution_guard::PhaseFourBridgeState::Pending { .. }
                | pallet_execution_guard::PhaseFourBridgeState::Scheduled { .. }
        ) {
            set_migration_halt_source(APPLIED_DETECTION_HALT);
            return;
        }
        if !valid || crate::ExecutionGuard::validation_code_applied().is_err() {
            set_migration_halt_source(APPLIED_DETECTION_HALT);
        } else {
            MigrationFailedStep::kill();
            // A valid primary image may carry zero MBMs, in which case
            // `MigrationStatusHandler::completed()` never fires.
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

/// A snapshot for the current/future epoch cannot legally be recorded while
/// the dead-man clock blocks that epoch's close. Suppress only that impossible
/// cause; an already-overdue earlier epoch remains an active incident.
fn snapshot_close_blocked_by_pause() -> bool {
    pallet_epoch::DeadMan::<Runtime>::get().paused_at.is_some()
        && pallet_welfare::SnapshotDeadline::<Runtime>::get().is_some_and(|progress| {
            progress.due_epoch >= pallet_epoch::EpochOf::<Runtime>::get().index
        })
}

impl pallet_execution_guard::Config for Runtime {
    type Epoch = RuntimeEpochHandoff;
    type EnqueueAuthority = EnsureEpochAccount;
    type Attestations = RuntimeAttestations;
    type KeeperRebate = FutarchyTreasury;
    type PendingOutflowSync = RuntimePendingOutflowSync;
    type Guardian = RuntimeGuardianState;
    type Params = ExecutionParams;
    type Capabilities = RuntimeCapabilities;
    type UpgradeSchedule = RuntimeUpgradeSchedule;
    type MigrationStatus = RuntimeMigrationStatus;
    type Preimages = RuntimePreimages;
    type RecoveryImages = RuntimePreimages;
    type ReleaseChannel = RuntimeReleaseChannel;
    type RatifyOrigin = EnsureValuesScoped<RatifyTrack>;
    type RecoveryCommitOrigin = frame_support::traits::EitherOfDiverse<
        pallet_origins::EnsureFutarchyCode,
        pallet_origins::EnsureFutarchyMeta,
    >;
    type PhaseFourBridgeOrigin = EnsureCurrentSudoKey;
    type PhaseState = RuntimePhaseState;
    type Dispatcher = crate::classifier::RuntimeDispatcher;
    type MaxRuntimeCodeBytes = ConstU32<{ pallet_preimage::MAX_SIZE }>;
    type WeightInfo = crate::weights::pallet_execution_guard::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = RuntimeBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct RuntimeBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
const BENCHMARK_KEEPER_REBATE: Balance = currency::USDC;
#[cfg(feature = "runtime-benchmarks")]
const BENCHMARK_REBATE_LINE_BALANCE: Balance = 100 * currency::USDC;

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
            last_change_block: 0,
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
                let _ = state.lines.try_push((line, BENCHMARK_REBATE_LINE_BALANCE));
            }
        }
        state.keeper_meter = pallet_futarchy_treasury::KeeperMeter {
            epoch: pallet_epoch::CurrentEpoch::<Runtime>::get(),
            ..Default::default()
        };
    });

    benchmark_ensure_usdc();
    for pot in [treasury_keeper_account(), treasury_oracle_account()] {
        let balance = <ForeignAssets as Inspect<AccountId>>::balance(usdc_location(), &pot);
        if balance < BENCHMARK_REBATE_LINE_BALANCE {
            let _ = <ForeignAssets as Mutate<AccountId>>::mint_into(
                usdc_location(),
                &pot,
                BENCHMARK_REBATE_LINE_BALANCE - balance,
            );
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
fn benchmark_ensure_usdc() {
    if !ForeignAssets::asset_exists(usdc_location()) {
        let _ = ForeignAssets::force_create(
            RuntimeOrigin::root(),
            usdc_location(),
            sp_runtime::MultiAddress::Id(AccountId32::new([0; 32])),
            true,
            currency::USDC_CENT,
        );
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_payload_bytes_for(seed: futarchy_primitives::ProposalId) -> Vec<u8> {
    let calls = (0..pallet_execution_guard::MAX_CALLS)
        .map(|index| {
            let mut remark = vec![index as u8; 4_000];
            if index == 0 {
                remark[..core::mem::size_of::<futarchy_primitives::ProposalId>()]
                    .copy_from_slice(&seed.to_le_bytes());
            }
            RuntimeCall::System(frame_system::Call::remark { remark })
        })
        .collect::<Vec<_>>();
    benchmark_pad_payload(calls)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_pad_payload(mut calls: Vec<RuntimeCall>) -> Vec<u8> {
    let target = pallet_execution_guard::MAX_PAYLOAD_BYTES as usize;
    loop {
        let bytes =
            pallet_execution_guard::RuntimeBatch::<Runtime>::truncate_from(calls.clone()).encode();
        if bytes.len() == target {
            return bytes;
        }
        let Some(RuntimeCall::System(frame_system::Call::remark { remark })) = calls.last_mut()
        else {
            return bytes;
        };
        if bytes.len() < target {
            remark.resize(remark.len().saturating_add(target - bytes.len()), 0xff);
        } else {
            remark.truncate(remark.len().saturating_sub(bytes.len() - target));
        }
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_ensure_payload_preimage(
    seed: futarchy_primitives::ProposalId,
) -> (futarchy_primitives::H256, u32) {
    let bytes = benchmark_payload_bytes_for(seed);
    let payload_len = u32::try_from(bytes.len()).unwrap_or_default();
    let hash = sp_io::hashing::blake2_256(&bytes);
    if <Preimage as QueryPreimage>::len(&hash.into()).is_none() {
        let _ = <Preimage as StorePreimage>::note(Cow::Owned(bytes));
    }
    (hash, payload_len)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_market_set(
    pid: futarchy_primitives::ProposalId,
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

/// Create the real (unseeded) market books behind a `benchmark_market_set`.
///
/// B10 latches every ledger terminal into the market pallet
/// (`observe_proposal_terminal` / `observe_baseline_terminal` run inside the
/// production resolve/void/settle seams), and that latch walks
/// `ProposalMarketIds` and requires each book to exist with the owning kind.
/// Benchmark fixtures that only fabricate market *ids* therefore make every
/// terminal-crossing dispatch fail with `TryStateViolation`; back the ids with
/// bounded books through the production `create_market` entry point instead.
#[cfg(feature = "runtime-benchmarks")]
fn benchmark_ensure_market_books(
    pid: futarchy_primitives::ProposalId,
    epoch: EpochId,
    gates: bool,
) -> futarchy_primitives::MarketSet {
    use futarchy_primitives::{Branch, GateType};
    use pallet_market::core_market::BookKind;

    let set = benchmark_market_set(pid, epoch, gates);
    let decision_b = balance_param(b"pol.b.param");
    let gate_b = balance_param(b"pol.b_gate");
    let mut books = Vec::from([
        (
            set.accept,
            BookKind::Decision {
                proposal: pid,
                branch: Branch::Accept,
            },
            decision_b,
        ),
        (
            set.reject,
            BookKind::Decision {
                proposal: pid,
                branch: Branch::Reject,
            },
            decision_b,
        ),
    ]);
    if let Some(gate_ids) = set.gates {
        // 05 §5.1 order: (S,C) × (adopt,reject), as in the production adapter.
        books.extend([
            (
                gate_ids[0],
                BookKind::Gate {
                    proposal: pid,
                    branch: Branch::Accept,
                    gate: GateType::Survival,
                },
                gate_b,
            ),
            (
                gate_ids[1],
                BookKind::Gate {
                    proposal: pid,
                    branch: Branch::Reject,
                    gate: GateType::Survival,
                },
                gate_b,
            ),
            (
                gate_ids[2],
                BookKind::Gate {
                    proposal: pid,
                    branch: Branch::Accept,
                    gate: GateType::Security,
                },
                gate_b,
            ),
            (
                gate_ids[3],
                BookKind::Gate {
                    proposal: pid,
                    branch: Branch::Reject,
                    gate: GateType::Security,
                },
                gate_b,
            ),
        ]);
    }
    for (id, kind, b) in books {
        if !pallet_market::Markets::<Runtime>::contains_key(id) {
            let _ = pallet_market::Pallet::<Runtime>::create_market(
                epoch_signed_origin(),
                id,
                kind,
                market_book_account(id),
                market_fee_account(id),
                b,
            );
        }
    }
    set
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_quote(market: futarchy_primitives::MarketId) -> FixedU64 {
    match market % 10 {
        1 => FixedU64(750_000_000),
        2 => FixedU64(250_000_000),
        3 | 5 => FixedU64(10_000_000),
        4 | 6 => FixedU64(50_000_000),
        _ => FixedU64(500_000_000),
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_runtime_version() -> futarchy_primitives::RuntimeVersionConstraint {
    let spec_name =
        match futarchy_primitives::BoundedVec::try_from(VERSION.spec_name.as_bytes().to_vec()) {
            Ok(value) => value,
            Err(_) => futarchy_primitives::BoundedVec::new(),
        };
    futarchy_primitives::RuntimeVersionConstraint {
        spec_name,
        spec_version: VERSION.spec_version,
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_attestations(
    pid: futarchy_primitives::ProposalId,
    artifact_hash: futarchy_primitives::H256,
) {
    benchmark_fill_upgrade_attestations(pid, artifact_hash, None);
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_upgrade_attestations(
    pid: futarchy_primitives::ProposalId,
    primary_hash: futarchy_primitives::H256,
    recovery_hash: Option<futarchy_primitives::H256>,
) {
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
            let primary = id
                >= pallet_attestor::MAX_ATTESTATIONS
                    .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM);
            let recovery = recovery_hash.is_some()
                && id
                    >= pallet_attestor::MAX_ATTESTATIONS
                        .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM.saturating_mul(2))
                && !primary;
            let target = primary || recovery;
            pallet_attestor::Attestation {
                id,
                pid: if target {
                    pid
                } else {
                    100_000u64.saturating_add(u64::from(id))
                },
                artifact_hash: if primary {
                    primary_hash
                } else if recovery {
                    recovery_hash.unwrap_or_default()
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
            pallet_constitution::ConstitutionOrigin::ConstitutionTrack => {
                crate::track_origins::Origin::Constitution.into()
            }
            pallet_constitution::ConstitutionOrigin::EntrenchedTrack => {
                crate::track_origins::Origin::Entrenched.into()
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

    fn prime_phase_arming() -> DispatchResult {
        benchmark_ensure_usdc();
        let amount =
            FutarchyTreasury::floor(futarchy_primitives::ProposalClass::Meta).saturating_mul(4);
        <ForeignAssets as Mutate<AccountId>>::mint_into(
            usdc_location(),
            &insurance_account(),
            amount,
        )?;
        FutarchyTreasury::sweep_insurance(pallet_origins::Origin::FutarchyTreasury.into(), amount)
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_welfare::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();

    fn metric_governance_origin() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
    fn prime_finalized_epoch(epoch: EpochId) {
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = epoch.saturating_add(1));
    }
    fn prime_metric_inputs(_: u16) {}
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_oracle::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();

    fn adjudication_origin() -> RuntimeOrigin {
        pallet_origins::Origin::OracleResolution.into()
    }
    fn prime_reserve_probe() {
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );
        let Some(envelope) = live_reserve_probe_envelope() else {
            return;
        };
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = state.main_usdc.saturating_add(envelope.runway);
        });
        let _ = crate::FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            pallet_futarchy_treasury::BudgetLine::OpsReserveProbe,
            envelope.runway,
        );
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
        let cohort_epoch = epoch.saturating_sub(1);
        pallet_epoch::CohortSchedules::<Runtime>::insert(
            cohort_epoch,
            pallet_epoch::CohortSchedule {
                epoch: cohort_epoch,
                creation_epoch_length:
                    <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get().epoch_length,
                measurement_until: epoch,
                settlement_epoch: epoch.saturating_add(1),
                specs: frame_support::BoundedVec::truncate_from(Vec::from([(1, version)])),
            },
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
        benchmark_ensure_usdc();
        let _ = <ForeignAssets as Mutate<AccountId>>::mint_into(
            usdc_location(),
            &who,
            currency::USDC.saturating_mul(1_000_000),
        );
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
    fn community_origin() -> RuntimeOrigin {
        pallet_origins::Origin::FutarchyParam.into()
    }
    fn account(seed: u8) -> AccountId {
        AccountId32::new([seed; 32])
    }
    fn prime_pot_funding(amount: Balance) -> DispatchResult {
        let main = TreasuryPalletId::get().into_account_truncating();
        <ForeignAssets as Mutate<AccountId>>::mint_into(usdc_location(), &main, amount).map(|_| ())
    }
    fn prime_insurance_custody(amount: Balance) -> DispatchResult {
        <ForeignAssets as Mutate<AccountId>>::mint_into(
            usdc_location(),
            &insurance_account(),
            amount,
        )
        .map(|_| ())
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
    fn admin() -> RuntimeOrigin {
        crate::track_origins::Origin::GuardianTrack.into()
    }
    fn prime_for_worst_case() {
        if pallet_execution_guard::CurrentSpecName::<Runtime>::get().is_none() {
            pallet_execution_guard::CurrentSpecName::<Runtime>::put(benchmark_runtime_version());
        }
        let call = RuntimeCall::System(frame_system::Call::remark {
            remark: b"guardian-benchmark-queue".to_vec(),
        });
        let _ = benchmark_guard_enqueue(1, call, pallet_execution_guard::CallDomain::Public);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [0; 2],
            },
        );
        for seed in 1..=pallet_guardian::GUARDIAN_SEATS as u8 {
            let who = AccountId32::new([seed; 32]);
            let _ = <Balances as frame_support::traits::fungible::Mutate<AccountId>>::mint_into(
                &who,
                SubmissionDeposit::get().saturating_mul(2),
            );
        }
    }

    fn prime_review_approved(action: pallet_guardian::ActionId) {
        let Some(referendum) = pallet_guardian::ReviewReferenda::<Runtime>::get(action) else {
            return;
        };
        pallet_referenda::ReferendumInfoFor::<Runtime>::mutate(referendum, |maybe_info| {
            let Some(pallet_referenda::ReferendumInfo::Ongoing(status)) = maybe_info.as_ref()
            else {
                return;
            };
            let submission_deposit = status.submission_deposit.clone();
            let decision_deposit = status.decision_deposit.clone();
            *maybe_info = Some(pallet_referenda::ReferendumInfo::Approved(
                System::block_number(),
                Some(submission_deposit),
                decision_deposit,
            ));
        });
    }
    fn prime_maintenance_epoch(epoch: EpochId) {
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = epoch);
    }
    fn close_review(referendum: u32) -> Result<(), DispatchError> {
        Referenda::cancel(
            pallet_origins::Origin::ConstitutionalValues.into(),
            referendum,
        )
    }
}
#[cfg(feature = "runtime-benchmarks")]
impl pallet_attestor::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    fn prime_funds() {
        let reason: RuntimeHoldReason = pallet_attestor::HoldReason::AttestorBond.into();
        let challenge_reason: RuntimeHoldReason = pallet_attestor::HoldReason::ChallengeBond.into();
        let _ = Balances::force_set_balance(
            RuntimeOrigin::root(),
            sp_runtime::MultiAddress::Id(insurance_account()),
            1_000_000 * currency::VIT,
        );
        for seed in 1..=255u8 {
            let who = AccountId32::new([seed; 32]);
            let _ = Balances::force_set_balance(
                RuntimeOrigin::root(),
                sp_runtime::MultiAddress::Id(who.clone()),
                1_000_000 * currency::VIT,
            );
            if seed <= pallet_attestor::MAX_ATTESTORS as u8 {
                let _ = <Balances as MutateHold<AccountId>>::hold(
                    &reason,
                    &who,
                    pallet_attestor::ATTESTOR_BOND,
                );
            }
            if seed == 250 {
                let _ = <Balances as MutateHold<AccountId>>::hold(
                    &challenge_reason,
                    &who,
                    pallet_attestor::CHALLENGE_BOND,
                );
            }
        }
    }
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
        System::set_block_number(1);
        let now = System::block_number();
        let params = <RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        pallet_epoch::EpochOf::<Runtime>::put(pallet_epoch::EpochInfo {
            index: epoch,
            phase: futarchy_primitives::EpochPhase::Intake,
            phase_start_block: now,
        });
        pallet_epoch::Schedule::<Runtime>::put(pallet_epoch::EpochSchedule {
            epoch_start_block: now,
            length: params.epoch_length,
            next_length: params.epoch_length,
        });
        pallet_epoch::NextProposalId::<Runtime>::put(1);
        pallet_execution_guard::CurrentSpecName::<Runtime>::put(benchmark_runtime_version());
        benchmark_ensure_usdc();
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

    fn prime_ratification(pid: futarchy_primitives::ProposalId, referendum_index: u32) {
        let payload_hash = pallet_epoch::Proposals::<Runtime>::get(pid)
            .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
            .map(|proposal| proposal.payload_hash)
            .unwrap_or_default();
        pallet_execution_guard::Ratifications::<Runtime>::insert(
            pid,
            pallet_execution_guard::RatificationRecord {
                referendum_index,
                payload_hash,
                ratified_at: System::block_number(),
            },
        );
    }

    fn void_authority_origin() -> RuntimeOrigin {
        pallet_origins::Origin::EmergencyPlaybook.into()
    }

    fn account(seed: u8) -> AccountId {
        let who = AccountId32::new([seed; 32]);
        benchmark_ensure_usdc();
        let _ = <ForeignAssets as Mutate<AccountId>>::mint_into(
            usdc_location(),
            &who,
            currency::USDC.saturating_mul(1_000_000),
        );
        who
    }

    fn proposal(
        id: futarchy_primitives::ProposalId,
        who: AccountId,
        now: BlockNumber,
        epoch: EpochId,
    ) -> futarchy_primitives::Proposal<AccountId> {
        let (payload_hash, payload_len) = benchmark_ensure_payload_preimage(id);
        futarchy_primitives::Proposal {
            id,
            proposer: who,
            class: futarchy_primitives::ProposalClass::Param,
            state: futarchy_primitives::ProposalState::Submitted,
            epoch,
            submitted_at: now,
            payload_hash,
            payload_len,
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
            version_constraint: Some(
                pallet_execution_guard::CurrentSpecName::<Runtime>::get()
                    .map_or_else(benchmark_runtime_version, |version| version),
            ),
            decision: None,
        }
    }

    fn prime_recovery_qualification(proposal: &mut futarchy_primitives::Proposal<AccountId>) {
        use frame_support::traits::StorePreimage;

        if pallet_execution_guard::CurrentSpecName::<Runtime>::get().is_none() {
            pallet_execution_guard::CurrentSpecName::<Runtime>::put(benchmark_runtime_version());
        }
        let primary_hash = sp_io::hashing::blake2_256(&proposal.id.encode());
        let Ok(recovery) = benchmark_recovery_image(proposal.id, primary_hash) else {
            return;
        };
        let calls = alloc::vec![
            RuntimeCall::System(frame_system::Call::authorize_upgrade {
                code_hash: primary_hash.into(),
            }),
            RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::commit_recovery_image {
                hash: recovery.hash,
                len: recovery.len,
                target_spec_version: recovery.target_spec_version,
                attestation_id: recovery.attestation_id,
            }),
        ];
        let Ok(batch) = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(calls)
        else {
            return;
        };
        let payload = batch.encode();
        let Ok(payload_len) = u32::try_from(payload.len()) else {
            return;
        };
        let Ok(payload_hash) = <Preimage as StorePreimage>::note(Cow::Owned(payload)) else {
            return;
        };
        let qualification_context = benchmark_epoch_proposal(
            proposal.id,
            payload_hash.0,
            payload_len,
            futarchy_primitives::ProposalState::Submitted,
        );
        pallet_epoch::IntakeProposals::<Runtime>::insert(proposal.id, qualification_context);
        let caller = AccountId32::new([249; 32]);
        if crate::ExecutionGuard::qualify_recovery_image(RuntimeOrigin::signed(caller), proposal.id)
            .is_err()
        {
            return;
        }
        proposal.class = futarchy_primitives::ProposalClass::Code;
        proposal.payload_hash = payload_hash.0;
        proposal.payload_len = payload_len;
        proposal.bond = balance_param(b"prop.bond.code");
        proposal.resources =
            futarchy_primitives::BoundedVec::try_from(alloc::vec![[0x03, 0, 0, 0, 0, 0, 0, 0,]])
                .unwrap_or_default();
        proposal.version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get();
    }

    fn prime_decision(
        pid: futarchy_primitives::ProposalId,
        epoch: EpochId,
        gates: bool,
    ) -> futarchy_primitives::MarketSet {
        if pallet_execution_guard::CurrentSpecName::<Runtime>::get().is_none() {
            pallet_execution_guard::CurrentSpecName::<Runtime>::put(benchmark_runtime_version());
        }
        if !pallet_conditional_ledger::Vaults::<Runtime>::contains_key(pid) {
            let _ =
                ConditionalLedger::create_vault(RuntimeOrigin::signed(market_account()), pid, 0);
        }
        if gates {
            let payload_hash = benchmark_ensure_payload_preimage(pid).0;
            benchmark_fill_attestations(pid, payload_hash);
        }
        benchmark_ensure_market_books(pid, epoch, gates)
    }

    fn prime_guard_enqueue(_: futarchy_primitives::ProposalId) {}

    fn prime_settlement(epoch: EpochId) {
        for (pid, proposal) in pallet_epoch::Proposals::<Runtime>::iter() {
            if proposal.epoch == epoch {
                let _ = ConditionalLedger::resolve(
                    epoch_signed_origin(),
                    pid,
                    futarchy_primitives::Branch::Accept,
                );
            }
        }
        let baseline = 9_000u64.saturating_add(u64::from(epoch));
        if !pallet_market::Markets::<Runtime>::contains_key(baseline)
            && !pallet_conditional_ledger::BaselineVaults::<Runtime>::contains_key(epoch)
        {
            // The production entry point creates the baseline vault and the
            // `BaselineMarketOf` index, and the settlement path's terminal latch
            // (`observe_baseline_terminal`, B10) requires the real book.
            let _ = pallet_market::Pallet::<Runtime>::create_market(
                epoch_signed_origin(),
                baseline,
                pallet_market::core_market::BookKind::Baseline { epoch },
                market_book_account(baseline),
                market_fee_account(baseline),
                balance_param(b"pol.b_baseline"),
            );
        }
        for offset in 1..=pallet_welfare::MAX_SNAPSHOTS_BOUND {
            let measured_epoch = epoch.saturating_add(offset);
            pallet_welfare::Snapshots::<Runtime>::insert(
                (measured_epoch, 0),
                pallet_welfare::StoredSnapshot {
                    epoch: measured_epoch,
                    spec_version: 0,
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
            pallet_welfare::GateBreachFlags::<Runtime>::insert(
                measured_epoch,
                pallet_welfare::CoreGateBreachFlags {
                    s_breached: false,
                    c_breached: false,
                    day_bitmap: [0; 2],
                },
            );
        }
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
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
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
    class: futarchy_primitives::ProposalClass,
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
    proposal.class = class;
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
        ConditionalLedger::create_vault(RuntimeOrigin::signed(market_account()), pid, 0)?;
    }
    Ok(())
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_guard_enqueue(
    pid: futarchy_primitives::ProposalId,
    call: RuntimeCall,
    domain: pallet_execution_guard::CallDomain,
) -> Result<BlockNumber, DispatchError> {
    benchmark_guard_enqueue_for_class(
        pid,
        call,
        domain,
        futarchy_primitives::ProposalClass::Param,
        None,
        false,
    )
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_guard_enqueue_for_class(
    pid: futarchy_primitives::ProposalId,
    call: RuntimeCall,
    domain: pallet_execution_guard::CallDomain,
    class: futarchy_primitives::ProposalClass,
    attestation_id: Option<u32>,
    ratified: bool,
) -> Result<BlockNumber, DispatchError> {
    benchmark_guard_enqueue_calls_for_class(
        pid,
        alloc::vec![call],
        alloc::vec![domain],
        class,
        attestation_id,
        ratified,
    )
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_guard_enqueue_calls_for_class(
    pid: futarchy_primitives::ProposalId,
    calls: Vec<RuntimeCall>,
    domains: Vec<pallet_execution_guard::CallDomain>,
    class: futarchy_primitives::ProposalClass,
    attestation_id: Option<u32>,
    ratified: bool,
) -> Result<BlockNumber, DispatchError> {
    use frame_support::traits::{QueryPreimage, StorePreimage};
    use pallet_execution_guard::BatchDispatcher;

    if pallet_execution_guard::CurrentSpecName::<Runtime>::get().is_none() {
        pallet_execution_guard::CurrentSpecName::<Runtime>::put(benchmark_runtime_version());
    }

    let primary_hash = calls
        .iter()
        .find_map(crate::classifier::RuntimeDispatcher::authorize_upgrade_hash);
    let recovery = calls
        .iter()
        .find_map(crate::classifier::RuntimeDispatcher::recovery_image_descriptor);
    let batch = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(calls)
        .map_err(|_| DispatchError::Other("benchmark guard batch bound"))?;
    let bytes = batch.encode();
    let payload_len = u32::try_from(bytes.len())
        .map_err(|_| DispatchError::Other("benchmark guard payload length"))?;
    let hash = <Preimage as StorePreimage>::note(Cow::Owned(bytes))?;
    <Preimage as QueryPreimage>::request(&hash);

    let now = System::block_number();
    let maturity = now
        .checked_add(<ExecutionParams as pallet_execution_guard::Params>::exec_timelock(class))
        .ok_or(DispatchError::Arithmetic(
            sp_runtime::ArithmeticError::Overflow,
        ))?;
    let grace_end = maturity
        .checked_add(<ExecutionParams as pallet_execution_guard::Params>::exec_grace(class))
        .ok_or(DispatchError::Arithmetic(
            sp_runtime::ArithmeticError::Overflow,
        ))?;
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
        .ok_or(DispatchError::Other("benchmark guard current version"))?;
    if let (Some(primary_hash), Some(descriptor)) = (primary_hash, recovery) {
        <Preimage as QueryPreimage>::request(&Hash::from(descriptor.hash));
        pallet_execution_guard::QualifiedRecoveryImages::<Runtime>::insert(
            pid,
            pallet_execution_guard::QualifiedRecoveryImage {
                payload_hash: hash.0,
                primary_hash,
                version_constraint: version_constraint.clone(),
                descriptor,
            },
        );
    }
    let declared_domains = pallet_execution_guard::pallet::StoredDomains::try_from(domains)
        .map_err(|_| DispatchError::Other("benchmark guard domain bound"))?;
    benchmark_seed_epoch_queue(
        pid,
        hash.0,
        payload_len,
        class,
        maturity,
        grace_end,
        version_constraint.clone(),
    )?;
    if ratified
        && matches!(
            class,
            futarchy_primitives::ProposalClass::Code | futarchy_primitives::ProposalClass::Meta
        )
    {
        // CODE/META ratification is a two-step join: bind the pending
        // referendum before queue admission, then record its passed identity.
        crate::ExecutionGuard::bind_ratification(pid, 1)?;
    }
    crate::ExecutionGuard::enqueue(
        RuntimeOrigin::signed(epoch_account()),
        pallet_execution_guard::pallet::StoredQueuedExecution {
            pid,
            payload_hash: hash.0,
            payload_len,
            class,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains,
            failed_at: None,
        },
        false,
    )?;
    if ratified {
        crate::ExecutionGuard::ratify(pallet_origins::Origin::ConstitutionalValues.into(), pid, 1)?;
    }
    Ok(maturity)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_recovery_image(
    pid: futarchy_primitives::ProposalId,
    primary_hash: futarchy_primitives::H256,
) -> Result<pallet_execution_guard::RecoveryImageDescriptor, DispatchError> {
    benchmark_recovery_image_sized(pid, primary_hash, 512)
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_recovery_image_sized(
    pid: futarchy_primitives::ProposalId,
    primary_hash: futarchy_primitives::H256,
    bytes: u32,
) -> Result<pallet_execution_guard::RecoveryImageDescriptor, DispatchError> {
    use frame_support::traits::StorePreimage;

    let current = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
        .unwrap_or_else(benchmark_runtime_version);
    let target_spec_version =
        current
            .spec_version
            .checked_add(2)
            .ok_or(DispatchError::Arithmetic(
                sp_runtime::ArithmeticError::Overflow,
            ))?;
    let code = benchmark_runtime_code_with_spec(bytes, target_spec_version);
    ParachainSystem::initialize_for_set_code_benchmark(code.len() as u32);
    let len =
        u32::try_from(code.len()).map_err(|_| DispatchError::Other("benchmark recovery length"))?;
    let hash = <Preimage as StorePreimage>::note(Cow::Owned(code))?.0;
    let attestation_id = pallet_attestor::MAX_ATTESTATIONS
        .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM.saturating_mul(2));
    benchmark_fill_upgrade_attestations(pid, primary_hash, Some(hash));
    Ok(pallet_execution_guard::RecoveryImageDescriptor {
        hash,
        len,
        target_spec_version,
        attestation_id,
    })
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_execution_guard::BenchmarkHelper<RuntimeOrigin> for RuntimeBenchmarkHelper {
    benchmark_keeper_rebate_hooks!();

    fn ratify_origin() -> RuntimeOrigin {
        pallet_origins::Origin::ConstitutionalValues.into()
    }
    fn recovery_commit_origin() -> RuntimeOrigin {
        pallet_origins::Origin::FutarchyCode.into()
    }
    fn phase_four_origin() -> RuntimeOrigin {
        #[cfg(feature = "bootstrap")]
        {
            pallet_sudo::Key::<Runtime>::get()
                .map(RuntimeOrigin::signed)
                .unwrap_or_else(|| RuntimeOrigin::signed(AccountId32::new([0; 32])))
        }
        #[cfg(not(feature = "bootstrap"))]
        RuntimeOrigin::signed(AccountId32::new([0; 32]))
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

    fn prime_execute(pid: futarchy_primitives::ProposalId, calls: u32) {
        System::set_block_number(System::block_number().max(1));
        pallet_constitution::PhaseFlags::<Runtime>::put(
            pallet_constitution::PhaseFlagsValue::CODE_META_ARMED,
        );
        let artifact = sp_io::hashing::blake2_256(&pid.encode());
        let Ok(recovery) = benchmark_recovery_image(pid, artifact) else {
            return;
        };
        let attestation_id = pallet_attestor::MAX_ATTESTATIONS
            .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM);
        let mut batch = alloc::vec![
            RuntimeCall::System(frame_system::Call::authorize_upgrade {
                code_hash: artifact.into(),
            }),
            RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::commit_recovery_image {
                hash: recovery.hash,
                len: recovery.len,
                target_spec_version: recovery.target_spec_version,
                attestation_id: recovery.attestation_id,
            },),
        ];
        batch.extend((2..calls).map(|index| {
            RuntimeCall::System(frame_system::Call::remark {
                remark: alloc::vec![index as u8; 32],
            })
        }));
        let mut domains = alloc::vec![
            pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
            pallet_execution_guard::CallDomain::Code,
        ];
        if calls > 2 {
            domains.push(pallet_execution_guard::CallDomain::Public);
        }
        if let Ok(maturity) = benchmark_guard_enqueue_calls_for_class(
            pid,
            batch,
            domains,
            futarchy_primitives::ProposalClass::Code,
            Some(attestation_id),
            true,
        ) {
            System::set_block_number(maturity.saturating_add(1));
            pallet_execution_guard::Queue::<Runtime>::mutate(pid, |queued| {
                if let Some(queued) = queued {
                    queued.maturity = System::block_number();
                }
            });
        }
    }
    fn prime_recovery_commit(
        pid: futarchy_primitives::ProposalId,
    ) -> pallet_execution_guard::RecoveryImageDescriptor {
        benchmark_recovery_image(pid, [0x51; 32]).unwrap_or(
            pallet_execution_guard::RecoveryImageDescriptor {
                hash: [0x52; 32],
                len: 512,
                target_spec_version: VERSION.spec_version.saturating_add(2),
                attestation_id: pallet_attestor::MAX_ATTESTATIONS
                    .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM.saturating_mul(2)),
            },
        )
    }
    fn prime_recovery_qualification(pid: futarchy_primitives::ProposalId, bytes: u32) {
        use frame_support::traits::StorePreimage;

        if pallet_execution_guard::CurrentSpecName::<Runtime>::get().is_none() {
            pallet_execution_guard::CurrentSpecName::<Runtime>::put(benchmark_runtime_version());
        }
        let primary_hash = sp_io::hashing::blake2_256(&pid.encode());
        let Ok(recovery) = benchmark_recovery_image_sized(pid, primary_hash, bytes) else {
            return;
        };
        let mut batch = alloc::vec![
            RuntimeCall::System(frame_system::Call::authorize_upgrade {
                code_hash: primary_hash.into(),
            }),
            RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::commit_recovery_image {
                hash: recovery.hash,
                len: recovery.len,
                target_spec_version: recovery.target_spec_version,
                attestation_id: recovery.attestation_id,
            }),
            RuntimeCall::System(frame_system::Call::remark {
                remark: alloc::vec![],
            }),
        ];
        loop {
            let encoded_len = batch.encode().len();
            if encoded_len == futarchy_primitives::kernel::MAX_BYTES as usize {
                break;
            }
            let RuntimeCall::System(frame_system::Call::remark { remark }) =
                batch.last_mut().expect("qualifier benchmark has padding")
            else {
                return;
            };
            if encoded_len < futarchy_primitives::kernel::MAX_BYTES as usize {
                remark.resize(
                    remark.len().saturating_add(
                        futarchy_primitives::kernel::MAX_BYTES as usize - encoded_len,
                    ),
                    0xff,
                );
            } else {
                remark.truncate(
                    remark.len().saturating_sub(
                        encoded_len - futarchy_primitives::kernel::MAX_BYTES as usize,
                    ),
                );
            }
        }
        let Ok(batch) = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(batch)
        else {
            return;
        };
        let payload = batch.encode();
        let Ok(payload_len) = u32::try_from(payload.len()) else {
            return;
        };
        let Ok(payload_hash) = <Preimage as StorePreimage>::note(Cow::Owned(payload)) else {
            return;
        };
        let proposal = benchmark_epoch_proposal(
            pid,
            payload_hash.0,
            payload_len,
            futarchy_primitives::ProposalState::Submitted,
        );
        pallet_epoch::IntakeProposals::<Runtime>::insert(pid, proposal);
    }
    fn prime_phase_four(pid: futarchy_primitives::ProposalId) {
        #[cfg(not(feature = "bootstrap"))]
        {
            let _ = pid;
        }
        #[cfg(feature = "bootstrap")]
        {
            System::set_block_number(System::block_number().max(1));
            let artifact = sp_io::hashing::blake2_256(&pid.encode());
            let Ok(recovery) = benchmark_recovery_image(pid, artifact) else {
                return;
            };
            let tvl_key = pallet_constitution::key16(b"phase3.tvl_cap");
            let deposit_key = pallet_constitution::key16(b"phase3.dep_cap");
            let Some(tvl) = pallet_constitution::Params::<Runtime>::get(tvl_key) else {
                return;
            };
            let Some(deposit) = pallet_constitution::Params::<Runtime>::get(deposit_key) else {
                return;
            };
            let batch = alloc::vec![
                RuntimeCall::System(frame_system::Call::authorize_upgrade {
                    code_hash: artifact.into(),
                }),
                RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::commit_recovery_image {
                    hash: recovery.hash,
                    len: recovery.len,
                    target_spec_version: recovery.target_spec_version,
                    attestation_id: recovery.attestation_id,
                },),
                RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                    key: tvl_key,
                    value: pallet_constitution::ParamValue::Balance(
                        tvl.value.as_u128().saturating_add(1),
                    ),
                }),
                RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                    key: deposit_key,
                    value: pallet_constitution::ParamValue::Balance(
                        deposit.value.as_u128().saturating_add(1),
                    ),
                }),
            ];
            let attestation_id = pallet_attestor::MAX_ATTESTATIONS
                .saturating_sub(futarchy_primitives::kernel::ATT_QUORUM);
            if let Ok(maturity) = benchmark_guard_enqueue_calls_for_class(
                pid,
                batch,
                alloc::vec![
                    pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
                    pallet_execution_guard::CallDomain::Code,
                    pallet_execution_guard::CallDomain::Meta,
                ],
                futarchy_primitives::ProposalClass::Meta,
                Some(attestation_id),
                true,
            ) {
                // Benchmark dispatch runs after the queue has been admitted;
                // advance strictly past the timelock so a boundary-sensitive
                // runtime hook cannot re-read the just-matured block as early.
                System::set_block_number(maturity.saturating_add(1));
                pallet_execution_guard::Queue::<Runtime>::mutate(pid, |queued| {
                    if let Some(queued) = queued {
                        queued.maturity = System::block_number();
                    }
                });
            }
        }
    }

    fn prime_recovery_application() -> (
        futarchy_primitives::H256,
        futarchy_primitives::RuntimeVersionConstraint,
    ) {
        use frame_support::traits::QueryPreimage;
        use pallet_execution_guard::ReleaseChannelWriter;

        System::set_block_number(System::block_number().max(1));
        if pallet_execution_guard::CurrentSpecName::<Runtime>::get().is_none() {
            pallet_execution_guard::CurrentSpecName::<Runtime>::put(benchmark_runtime_version());
        }
        let current = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
            .unwrap_or_else(benchmark_runtime_version);
        for pid in 10_000..10_000u64.saturating_add(pallet_execution_guard::MAX_QUEUE_BOUND.into())
        {
            let _ = benchmark_guard_enqueue(
                pid,
                RuntimeCall::System(frame_system::Call::remark {
                    remark: alloc::vec![0x52; 32],
                }),
                pallet_execution_guard::CallDomain::Public,
            );
        }
        let primary_hash = [0x51; 32];
        let recovery = benchmark_recovery_image(1, primary_hash).unwrap_or(
            pallet_execution_guard::RecoveryImageDescriptor {
                hash: [0x52; 32],
                len: 512,
                target_spec_version: current.spec_version.saturating_add(2),
                attestation_id: 8,
            },
        );
        <Preimage as QueryPreimage>::request(&Hash::from(recovery.hash));
        pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
            pallet_execution_guard::PendingUpgrade {
                hash: primary_hash,
                authorized_at: System::block_number(),
                applicable_at: System::block_number(),
                target_spec_version: current.spec_version.saturating_add(1),
            },
        );
        pallet_execution_guard::RecoveryImage::<Runtime>::put(
            pallet_execution_guard::RecoveryImageCommitment {
                pid: 1,
                primary_hash,
                hash: recovery.hash,
                len: recovery.len,
                target_spec_version: recovery.target_spec_version,
                attestation_id: recovery.attestation_id,
                committed_at: System::block_number(),
            },
        );
        let _ = RuntimeReleaseChannel::on_upgrade_authorized(
            current.spec_version.saturating_add(1),
            System::block_number(),
        );
        (
            recovery.hash,
            futarchy_primitives::RuntimeVersionConstraint {
                spec_name: current.spec_name,
                spec_version: recovery.target_spec_version,
            },
        )
    }

    fn prime_failed(pid: futarchy_primitives::ProposalId) {
        pallet_constitution::PhaseFlags::<Runtime>::put(
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
        );
        let call = RuntimeCall::System(frame_system::Call::remark_with_event {
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

    fn prime_pending_upgrade(bytes: u32) -> Vec<u8> {
        pallet_constitution::PhaseFlags::<Runtime>::put(
            pallet_constitution::PhaseFlagsValue::CODE_META_ARMED,
        );
        let target_spec_version = VERSION.spec_version.saturating_add(1);
        let code = benchmark_runtime_code_with_spec(bytes, target_spec_version);
        let hash = sp_io::hashing::blake2_256(&code);
        ParachainSystem::initialize_for_set_code_benchmark(code.len() as u32);
        let now = System::block_number().max(1);
        System::set_block_number(now);
        let _ = RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: hash.into(),
        })
        .dispatch_bypass_filter(RuntimeOrigin::root());
        pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
            pallet_execution_guard::PendingUpgrade {
                hash,
                authorized_at: now,
                applicable_at: now,
                target_spec_version,
            },
        );
        code
    }

    fn prime_stale(pid: futarchy_primitives::ProposalId) {
        pallet_constitution::PhaseFlags::<Runtime>::put(
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
        );
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
fn benchmark_runtime_code_with_spec(target_code_bytes: u32, spec_version: u32) -> Vec<u8> {
    const WASM_HEADER: [u8; 8] = [0, 97, 115, 109, 1, 0, 0, 0];
    const VERSION_SECTION: &[u8] = b"runtime_version";
    const PADDING_SECTION: &[u8] = b"benchmark_padding";
    let target = target_code_bytes as usize;
    let mut version = VERSION;
    version.spec_version = spec_version;
    let mut code = Vec::from(WASM_HEADER);
    code.extend(benchmark_custom_section(VERSION_SECTION, &version.encode()));
    let mut padding_len = target.saturating_sub(code.len());
    loop {
        let section = benchmark_custom_section(PADDING_SECTION, &vec![0; padding_len]);
        match code.len().saturating_add(section.len()).cmp(&target) {
            core::cmp::Ordering::Equal => {
                code.extend(section);
                return code;
            }
            core::cmp::Ordering::Greater => padding_len = padding_len.saturating_sub(1),
            core::cmp::Ordering::Less => padding_len = padding_len.saturating_add(1),
        }
    }
}
