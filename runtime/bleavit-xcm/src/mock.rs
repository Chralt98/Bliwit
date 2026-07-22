//! Mock runtime proving that the B4 XCM components compose (15 §4.1).
//!
//! `ForeignAssets` is keyed by the canonical XCM [`Location`], matching the
//! frozen `ForeignAssets.Account(USDC_LOCATION, who)` surface (02 §7.4).

use crate::{
    assets::{AssetTransactors, StandardLocationToAccountId},
    barrier::BleavitBarrier,
    caps::{CappedInflows, InflowCaps},
    coretime::XcmRenewalDispatcher,
    health::{HealthTrackingRouter, LocalXcmHealthSink},
    probe::{
        ProbeAwareResponseHandler, ProbeAwareWeightBounds, ProbeBudget, ProbeSink,
        XcmProbeDispatcher,
    },
    trader::{GovernedWeightTrader, TraderRates, WeightRate},
};
use frame_support::{
    derive_impl, parameter_types,
    traits::{AsEnsureOriginWithArg, Everything, NeverEnsureOrigin, Nothing},
    weights::Weight,
};
use frame_system::EnsureSigned;
use futarchy_primitives::{
    chain_identity, Balance, BlockNumber, EpochId, MetricId, MetricSpecVersion,
};
use parity_scale_codec::Encode;
use sp_runtime::{traits::IdentityLookup, AccountId32, BuildStorage};
use staging_xcm::latest::{prelude::*, SendError, SendResult, XcmHash};
use staging_xcm::prelude::XcmVersion;
use staging_xcm_builder::{
    EnsureXcmOrigin, FixedWeightBounds, FrameTransactionalProcessor, SignedAccountId32AsNative,
    SignedToAccountId32, SovereignSignedViaLocation, WithUniqueTopic,
};
use staging_xcm_executor::{traits::OnResponse, XcmExecutor};
use std::{cell::RefCell, collections::BTreeMap};

pub type AccountId = AccountId32;
type Block = frame_system::mocking::MockBlock<Test>;

pub const ALICE_BYTES: [u8; 32] = [1; 32];
pub const BOB_BYTES: [u8; 32] = [2; 32];

pub fn alice() -> AccountId {
    AccountId::from(ALICE_BYTES)
}

pub fn bob() -> AccountId {
    AccountId::from(BOB_BYTES)
}

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        ForeignAssets: pallet_assets,
        Oracle: pallet_oracle,
        Treasury: pallet_futarchy_treasury,
        PalletXcm: pallet_xcm,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId;
    type Lookup = IdentityLookup<AccountId>;
    type AccountData = pallet_balances::AccountData<Balance>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type AccountStore = System;
    type Balance = Balance;
}

#[derive_impl(pallet_assets::config_preludes::TestDefaultConfig)]
impl pallet_assets::Config for Test {
    type Currency = Balances;
    type Balance = Balance;
    type AssetId = Location;
    type AssetIdParameter = Location;
    type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
    type ForceOrigin = NeverEnsureOrigin<()>;
}

parameter_types! {
    pub static ReportWindowEnd: BlockNumber = 10;
    pub static ExpectedSpecs: Vec<MetricSpecVersion> = vec![3];
    pub static ExpectedComponents: Vec<(MetricId, MetricSpecVersion)> = vec![];
    pub static StakeAtRiskValue: Balance = 400_000_000_000;
    pub const MaxRoundCloseBatch: u32 = 20;
}

pub struct TestReporting;
impl pallet_oracle::ReportingContext for TestReporting {
    fn report_window_end(_measurement_epoch: EpochId) -> BlockNumber {
        ReportWindowEnd::get()
    }

    fn is_expected_spec_version(
        _component: MetricId,
        _epoch: EpochId,
        version: MetricSpecVersion,
    ) -> bool {
        ExpectedSpecs::get().contains(&version)
    }

    fn stake_at_risk(_component: MetricId, _epoch: EpochId) -> Balance {
        StakeAtRiskValue::get()
    }

    fn expected_components(_measurement_epoch: EpochId) -> Vec<(MetricId, MetricSpecVersion)> {
        ExpectedComponents::get()
    }
}

impl pallet_oracle::Config for Test {
    type AdjudicationOrigin = NeverEnsureOrigin<()>;
    type Reporting = TestReporting;
    type Params = TestOracleParams;
    type MaxRoundCloseBatch = MaxRoundCloseBatch;
    type ProbeDispatch = TestProbeDispatcher;
    type ProbeTimeoutSink = ();
    type ReserveHealthSink = ();
    type KeeperRebate = ();
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestOracleBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestOracleBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_oracle::BenchmarkHelper<RuntimeOrigin> for TestOracleBenchmarkHelper {
    fn adjudication_origin() -> RuntimeOrigin {
        RuntimeOrigin::none()
    }
}

parameter_types! {
    pub const CurrentEpoch: EpochId = 0;
}

pub struct TestOracleParams;
impl pallet_oracle::OracleParamsProvider for TestOracleParams {
    fn get() -> pallet_oracle::OracleParams {
        pallet_oracle::OracleParams {
            probe_amount: ProbeAmount::get(),
            ..pallet_oracle::OracleParams::DEFAULT
        }
    }
}

pub struct TestTreasuryParams;
impl pallet_futarchy_treasury::TreasuryParams for TestTreasuryParams {
    fn cap_proposal_bps() -> u32 {
        futarchy_treasury_core::TRS_CAP_PROPOSAL_BPS
    }

    fn cap_30d_bps() -> u32 {
        futarchy_treasury_core::TRS_CAP_30D_BPS
    }

    fn cap_180d_bps() -> u32 {
        futarchy_treasury_core::TRS_CAP_180D_BPS
    }

    fn stream_threshold_bps() -> u32 {
        futarchy_treasury_core::TRS_STREAM_THRESHOLD_BPS
    }

    fn inflation_cap_bps() -> u32 {
        futarchy_treasury_core::ISS_INFLATION_CAP_BPS
    }

    fn keeper_budget_epoch() -> u128 {
        futarchy_treasury_core::KEEPER_BUDGET_EPOCH
    }

    fn keeper_rebate() -> u128 {
        0
    }

    fn coretime_dot_rate() -> u128 {
        10_000_000_000
    }

    fn reserve_probe_dot_rate() -> u128 {
        10_000_000_000
    }

    fn coretime_fee_dot() -> u128 {
        CoretimeFeeBudget::get()
    }

    fn coretime_quote_ttl() -> u32 {
        100
    }
}

impl pallet_futarchy_treasury::Config for Test {
    type TreasuryOrigin = EnsureSigned<AccountId>;
    type Params = TestTreasuryParams;
    type CurrentEpoch = CurrentEpoch;
    type TreasuryPhase = ();
    type BootstrapOpsFundingPolicy = ();
    type RenewalDispatch = TestRenewalDispatcher;
    type PotFunding = ();
    type InsuranceSweep = ();
    type RebatePayout = ();
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestTreasuryBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestTreasuryBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_futarchy_treasury::BenchmarkHelper<RuntimeOrigin, AccountId>
    for TestTreasuryBenchmarkHelper
{
    fn treasury_origin() -> RuntimeOrigin {
        RuntimeOrigin::signed(alice())
    }

    fn account(seed: u8) -> AccountId {
        AccountId::from([seed; 32])
    }
}

thread_local! {
    static SENT: RefCell<Vec<(Location, Xcm<()>)>> = const { RefCell::new(Vec::new()) };
    static SEND_MODE: RefCell<SendMode> = const { RefCell::new(SendMode::Success) };
    static SEND_FEE: RefCell<Assets> = RefCell::new(Assets::new());
    static SENT_COUNT: RefCell<u32> = const { RefCell::new(0) };
    static PROBE_TIMEOUT_COUNT: RefCell<u32> = const { RefCell::new(0) };
    static TVL_CAP: RefCell<u128> = const { RefCell::new(u128::MAX) };
    static ACCOUNT_CAP: RefCell<u128> = const { RefCell::new(u128::MAX) };
    static ACCOUNT_INFLOWS: RefCell<BTreeMap<AccountId, u128>> = const { RefCell::new(BTreeMap::new()) };
}

const SEND_FAILURE_COUNT_KEY: &[u8] = b"bleavit-xcm/mock/send-failure-count";
const PARTIAL_SEND_KEY: &[u8] = b"bleavit-xcm/mock/partial-send";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendMode {
    Success,
    ValidateFailure,
    DeliverFailure,
}

pub fn set_send_mode(mode: SendMode) {
    SEND_MODE.with(|value| *value.borrow_mut() = mode);
}

pub fn set_send_fee(fee: Assets) {
    SEND_FEE.with(|value| *value.borrow_mut() = fee);
}

pub fn sent_messages() -> Vec<(Location, Xcm<()>)> {
    SENT.with(|messages| messages.borrow().clone())
}

pub fn health_counts() -> (u32, u32) {
    let sent = SENT_COUNT.with(|count| *count.borrow());
    let failed = frame_support::storage::unhashed::get(SEND_FAILURE_COUNT_KEY).unwrap_or(0);
    (sent, failed)
}

pub fn partial_send_marker_exists() -> bool {
    frame_support::storage::unhashed::exists(PARTIAL_SEND_KEY)
}

pub fn set_probe_line_balance(amount: u128) {
    use futarchy_treasury_core::BudgetLine;
    pallet_futarchy_treasury::State::<Test>::mutate(|state| {
        if let Some((_, balance)) = state
            .lines
            .iter_mut()
            .find(|(line, _)| *line == BudgetLine::OpsReserveProbe)
        {
            *balance = amount;
        } else {
            assert!(
                state
                    .lines
                    .try_push((BudgetLine::OpsReserveProbe, amount))
                    .is_ok(),
                "treasury line bound"
            );
        }
    });
}

pub fn set_caps(tvl_cap: u128, account_cap: u128) {
    TVL_CAP.with(|value| *value.borrow_mut() = tvl_cap);
    ACCOUNT_CAP.with(|value| *value.borrow_mut() = account_cap);
}

pub fn recorded_inflow(who: &AccountId) -> u128 {
    ACCOUNT_INFLOWS.with(|inflows| inflows.borrow().get(who).copied().unwrap_or_default())
}

/// Recording sender used to prove the health wrapper is observational only (09 §6.4).
pub struct RecordingSender;
impl SendXcm for RecordingSender {
    type Ticket = (Location, Xcm<()>);

    fn validate(
        destination: &mut Option<Location>,
        message: &mut Option<Xcm<()>>,
    ) -> SendResult<Self::Ticket> {
        if SEND_MODE.with(|mode| *mode.borrow() == SendMode::ValidateFailure) {
            frame_support::storage::unhashed::put(PARTIAL_SEND_KEY, &1_u32);
            return Err(SendError::Transport("mock validation failure"));
        }
        match (destination.take(), message.take()) {
            (Some(destination), Some(message)) => {
                let fee = SEND_FEE.with(|value| value.borrow().clone());
                Ok(((destination, message), fee))
            }
            _ => Err(SendError::MissingArgument),
        }
    }

    fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
        if SEND_MODE.with(|mode| *mode.borrow() == SendMode::DeliverFailure) {
            frame_support::storage::unhashed::put(PARTIAL_SEND_KEY, &2_u32);
            return Err(SendError::Transport("mock delivery failure"));
        }
        let hash = ticket.1.using_encoded(sp_io::hashing::blake2_256);
        SENT.with(|messages| messages.borrow_mut().push(ticket));
        Ok(hash)
    }
}

pub struct TestHealthSink;
impl LocalXcmHealthSink for TestHealthSink {
    fn note_sent() {
        SENT_COUNT.with(|count| {
            let mut count = count.borrow_mut();
            *count = count.saturating_add(1);
        });
    }

    fn note_send_failure() {
        let count: u32 = frame_support::storage::unhashed::get(SEND_FAILURE_COUNT_KEY).unwrap_or(0);
        frame_support::storage::unhashed::put(SEND_FAILURE_COUNT_KEY, &count.saturating_add(1));
    }

    fn note_probe_timeout() {
        PROBE_TIMEOUT_COUNT.with(|count| {
            let mut count = count.borrow_mut();
            *count = count.saturating_add(1);
        });
    }
}

pub struct TestRates;
impl TraderRates for TestRates {
    fn dot_rate() -> WeightRate {
        WeightRate {
            units_per_second: 2,
            units_per_megabyte: 2,
        }
    }

    fn usdc_rate() -> WeightRate {
        WeightRate {
            units_per_second: 3,
            units_per_megabyte: 3,
        }
    }
}

pub struct OracleProbeSink;
impl ProbeSink for OracleProbeSink {
    fn pending_query_id() -> Option<u64> {
        let health = pallet_oracle::ReserveHealth::<Test>::get();
        health.pending_since.map(|_| health.last_query_id)
    }

    fn probe_result(query_id: u64, passed: bool) -> Weight {
        let _ = Oracle::reserve_probe_result(query_id, passed);
        ProbeMaxResponseWeight::get()
    }
}

pub struct TestCaps;
impl InflowCaps<AccountId> for TestCaps {
    fn usdc_mint_admissible(amount: u128) -> Result<(), ()> {
        let issuance =
            <ForeignAssets as frame_support::traits::fungibles::Inspect<AccountId>>::total_issuance(
                crate::identity::usdc_location(),
            );
        let next = issuance.checked_add(amount).ok_or(())?;
        (next <= TVL_CAP.with(|cap| *cap.borrow()))
            .then_some(())
            .ok_or(())
    }

    fn usdc_inflow_admissible(who: &AccountId, amount: u128) -> Result<(), ()> {
        ACCOUNT_INFLOWS.with(|inflows| {
            let previous = inflows.borrow().get(who).copied().unwrap_or_default();
            let next = previous.checked_add(amount).ok_or(())?;
            (next <= ACCOUNT_CAP.with(|cap| *cap.borrow()))
                .then_some(())
                .ok_or(())
        })
    }

    fn note_usdc_inflow(who: &AccountId, amount: u128) -> Result<(), ()> {
        ACCOUNT_INFLOWS.with(|inflows| {
            let mut inflows = inflows.borrow_mut();
            let previous = inflows.get(who).copied().unwrap_or_default();
            let next = previous.checked_add(amount).ok_or(())?;
            if next > ACCOUNT_CAP.with(|cap| *cap.borrow()) {
                return Err(());
            }
            inflows.insert(who.clone(), next);
            Ok(())
        })
    }
}

parameter_types! {
    pub const AnyNetwork: Option<NetworkId> = None;
    pub UniversalLocation: InteriorLocation = [
        GlobalConsensus(NetworkId::Polkadot),
        Parachain(chain_identity::FIXTURE_PARA_ID),
    ].into();
    pub const MaxPrefixes: u32 = 8;
    pub const UnitWeightCost: Weight = Weight::from_parts(1, 1);
    pub const MaxInstructions: u32 = 32;
    pub const MaxAssetsIntoHolding: u32 = 8;
    pub CheckingAccount: AccountId = AccountId::from([99; 32]);
    pub DotLocation: Location = crate::identity::dot_location();
    pub AdvertisedXcmVersion: XcmVersion = crate::identity::XCM_VERSION_PINNED;
    pub const ProbeAmount: u128 = 10;
    pub const ProbeFeeBudget: u128 = 100;
    pub const ProbeExecWeightBudget: Weight = Weight::from_parts(100, 100);
    pub const ProbeMaxResponseWeight: Weight = Weight::from_parts(100, 100);
    pub const OurParaId: u32 = chain_identity::FIXTURE_PARA_ID;
    pub TreasuryLocation: Location = Location::new(0, [Junction::AccountId32 { network: None, id: ALICE_BYTES }]);
    pub const CoretimeFeeBudget: u128 = 100;
    pub static RenewalAccount: Option<[u8; 32]> = Some([44; 32]);
    pub const RelayWeightLimit: Weight = Weight::from_parts(100, 100);
    pub const CoretimeWeightLimit: Weight = Weight::from_parts(100, 100);
}

pub type TestLocationToAccountId = StandardLocationToAccountId<AccountId, AnyNetwork>;
pub type TestAssetTransactors =
    AssetTransactors<ForeignAssets, TestLocationToAccountId, AccountId, CheckingAccount>;
pub type TestCappedAssets =
    CappedInflows<TestAssetTransactors, TestCaps, TestLocationToAccountId, AccountId>;
pub type TestResponseHandler = ProbeAwareResponseHandler<PalletXcm, OracleProbeSink>;
pub type TestBaseWeigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
pub type TestWeigher = ProbeAwareWeightBounds<TestBaseWeigher, ProbeMaxResponseWeight>;
pub type TestBarrier = BleavitBarrier<
    TestResponseHandler,
    UniversalLocation,
    MaxPrefixes,
    TestCaps,
    TestLocationToAccountId,
    AccountId,
>;
pub type TestRouter = HealthTrackingRouter<RecordingSender, TestHealthSink>;
pub struct TestProbeBudget;
impl ProbeBudget for TestProbeBudget {
    fn ready_to_arm(_: &pallet_oracle::OracleParams) -> bool {
        true
    }

    fn reserve_fee(_: u128) -> Result<u128, sp_runtime::DispatchError> {
        Treasury::charge_reserve_probe_fee(
            ProbeFeeBudget::get(),
            <TestTreasuryParams as pallet_futarchy_treasury::TreasuryParams>::reserve_probe_dot_rate(),
        )?;
        Ok(ProbeFeeBudget::get())
    }
}
pub type TestProbeDispatcher = XcmProbeDispatcher<
    WithUniqueTopic<RecordingSender>,
    TestProbeBudget,
    ProbeExecWeightBudget,
    ProbeMaxResponseWeight,
    OurParaId,
    TestHealthSink,
>;
pub type TestRenewalDispatcher = XcmRenewalDispatcher<
    XcmExecutor<XcmConfig>,
    RuntimeCall,
    TreasuryLocation,
    CoretimeFeeBudget,
    RenewalAccount,
    RelayWeightLimit,
    CoretimeWeightLimit,
    // The local execution budget is a distinct scale from the remote-leg fee
    // bounds (re-review minor); the mock sizes them equally for simplicity.
    RelayWeightLimit,
>;

pub type LocalOriginConverter = (
    SovereignSignedViaLocation<TestLocationToAccountId, RuntimeOrigin>,
    SignedAccountId32AsNative<AnyNetwork, RuntimeOrigin>,
);

pub struct XcmConfig;
impl staging_xcm_executor::Config for XcmConfig {
    type RuntimeCall = RuntimeCall;
    type XcmSender = TestRouter;
    type XcmEventEmitter = PalletXcm;
    type AssetTransactor = TestCappedAssets;
    type OriginConverter = LocalOriginConverter;
    type IsReserve = crate::assets::BleavitReserves;
    type IsTeleporter = ();
    type UniversalLocation = UniversalLocation;
    type Barrier = TestBarrier;
    type Weigher = TestWeigher;
    type Trader = GovernedWeightTrader<TestRates, ()>;
    type ResponseHandler = TestResponseHandler;
    type AssetTrap = PalletXcm;
    type AssetLocker = ();
    type AssetExchanger = ();
    type SubscriptionService = PalletXcm;
    type PalletInstancesInfo = AllPalletsWithSystem;
    type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
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
    type XcmRecorder = PalletXcm;
}

pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, AnyNetwork>;

impl pallet_xcm::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type CurrencyMatcher = staging_xcm_builder::IsConcrete<DotLocation>;
    type AuthorizedAliasConsideration = ();
    type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
    type XcmRouter = TestRouter;
    type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
    type XcmExecuteFilter = Everything;
    type XcmExecutor = XcmExecutor<XcmConfig>;
    type XcmTeleportFilter = Nothing;
    type XcmReserveTransferFilter = crate::filter::ReserveTransferFilter;
    type Weigher = TestWeigher;
    type UniversalLocation = UniversalLocation;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 16;
    type AdvertisedXcmVersion = AdvertisedXcmVersion;
    type AdminOrigin = NeverEnsureOrigin<()>;
    type TrustedLockers = ();
    type SovereignAccountOf = TestLocationToAccountId;
    type MaxLockers = frame_support::traits::ConstU32<0>;
    type MaxRemoteLockConsumers = frame_support::traits::ConstU32<0>;
    type RemoteLockConsumerIdentifier = ();
    type WeightInfo = pallet_xcm::TestWeightInfo;
}

/// Fresh externalities with DOT and USDC registered as sufficient foreign assets (09 §6.1).
pub fn new_test_ext() -> sp_io::TestExternalities {
    let genesis = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: vec![(alice(), 1_000_000), (bob(), 1_000_000)],
            ..Default::default()
        },
        foreign_assets: pallet_assets::GenesisConfig {
            assets: vec![
                (crate::identity::usdc_location(), alice(), true, 1),
                (crate::identity::dot_location(), alice(), true, 1),
            ],
            metadata: vec![],
            accounts: vec![],
            next_asset_id: None,
            reserves: vec![],
        },
        oracle: Default::default(),
        treasury: pallet_futarchy_treasury::GenesisConfig {
            main_usdc: 2_000_000,
            coretime_quote_authority: Some(alice()),
            coretime_renewal_account: Some([44; 32]),
            ..Default::default()
        },
        pallet_xcm: Default::default(),
    };
    let storage = genesis.build_storage().unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| {
        System::set_block_number(1);
        reset_test_state();
        let _ = Treasury::fund_budget_line(
            RuntimeOrigin::signed(alice()),
            futarchy_treasury_core::BudgetLine::OpsReserveProbe,
            100_000,
        );
    });
    ext
}

pub fn reset_test_state() {
    SENT.with(|messages| messages.borrow_mut().clear());
    SEND_MODE.with(|mode| *mode.borrow_mut() = SendMode::Success);
    SEND_FEE.with(|fee| *fee.borrow_mut() = Assets::new());
    SENT_COUNT.with(|count| *count.borrow_mut() = 0);
    frame_support::storage::unhashed::kill(SEND_FAILURE_COUNT_KEY);
    frame_support::storage::unhashed::kill(PARTIAL_SEND_KEY);
    PROBE_TIMEOUT_COUNT.with(|count| *count.borrow_mut() = 0);
    TVL_CAP.with(|cap| *cap.borrow_mut() = u128::MAX);
    ACCOUNT_CAP.with(|cap| *cap.borrow_mut() = u128::MAX);
    ACCOUNT_INFLOWS.with(|inflows| inflows.borrow_mut().clear());
    RenewalAccount::set(Some([44; 32]));
}

/// The executor type is intentionally named so the composability assertion can
/// require every `staging-xcm-executor` bound at compile time (15 §4.1).
pub type BleavitXcmExecutor = XcmExecutor<XcmConfig>;

pub struct KnownResponse;
impl OnResponse for KnownResponse {
    fn expecting_response(origin: &Location, query_id: u64, _querier: Option<&Location>) -> bool {
        origin == &crate::identity::asset_hub_location() && query_id == 77
    }

    fn on_response(
        _origin: &Location,
        _query_id: u64,
        _querier: Option<&Location>,
        _response: Response,
        _max_weight: Weight,
        _context: &XcmContext,
    ) -> Weight {
        Weight::zero()
    }
}

pub type BarrierWithKnownResponse = BleavitBarrier<
    KnownResponse,
    UniversalLocation,
    MaxPrefixes,
    TestCaps,
    TestLocationToAccountId,
    AccountId,
>;
