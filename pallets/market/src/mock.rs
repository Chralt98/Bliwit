//! Mock runtime for `pallet-market`: System + Balances + Assets + the real
//! conditional ledger + market pallet.

use crate as pallet_market;
use frame_support::{
    derive_impl, parameter_types,
    traits::{AsEnsureOriginWithArg, Contains, EnsureOrigin},
    PalletId,
};
use frame_system::{EnsureSigned, RawOrigin};
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    kernel, Balance, MarketId,
};
use parity_scale_codec::{Decode, Encode};
use sp_runtime::{traits::AccountIdConversion, BuildStorage};

pub type AccountId = u64;
pub type AssetId = u32;
type Block = frame_system::mocking::MockBlock<Test>;

pub const ALICE: AccountId = 1;
pub const BOB: AccountId = 2;
pub const CHARLIE: AccountId = 3;
pub const RESOLVER: AccountId = 101;
pub const SETTLER: AccountId = 102;
pub const MARKET_ADMIN: AccountId = 103;
pub const BOOK: AccountId = 900;
pub const FEES: AccountId = 901;
pub const POL: AccountId = 902;
pub const TREASURY: AccountId = 903;
pub const INSURANCE: AccountId = 904;
pub const USDC: AssetId = 1337;
pub const UNIT: Balance = 1_000_000;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Assets: pallet_assets,
        Ledger: pallet_conditional_ledger,
        Market: pallet_market,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
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
    type AssetId = AssetId;
    type AssetIdParameter = AssetId;
    type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
    type ForceOrigin = frame_system::EnsureRoot<AccountId>;
}

macro_rules! ensure_account {
    ($name:ident, $acct:expr) => {
        pub struct $name;
        impl EnsureOrigin<RuntimeOrigin> for $name {
            type Success = ();

            fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
                match origin.clone().into() {
                    Ok(RawOrigin::Signed(who)) if who == $acct => Ok(()),
                    _ => Err(origin),
                }
            }

            #[cfg(feature = "runtime-benchmarks")]
            fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
                Ok(RawOrigin::Signed($acct).into())
            }
        }
    };
}

ensure_account!(EnsureResolver, RESOLVER);
ensure_account!(EnsureSettler, SETTLER);
ensure_account!(EnsureMarketAdmin, MARKET_ADMIN);

pub struct EnsureMarketPallet;
impl EnsureOrigin<RuntimeOrigin> for EnsureMarketPallet {
    type Success = ();

    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        match origin.clone().into() {
            Ok(RawOrigin::Signed(who)) if who == market_account() => Ok(()),
            _ => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RawOrigin::Signed(market_account()).into())
    }
}

pub struct Protocol;
impl Contains<AccountId> for Protocol {
    fn contains(who: &AccountId) -> bool {
        matches!(*who, BOOK | FEES | POL | TREASURY | INSURANCE)
            || *who == market_account()
            || *who == ledger_account()
    }
}

parameter_types! {
    pub const LedgerPalletId: PalletId = PalletId(*b"bl/ledgr");
    pub const MarketPalletId: PalletId = PalletId(*b"bl/mrket");
    pub static MinSplit: Balance = kernel::MIN_SPLIT_USDC;
    pub PositionDeposit: Balance = kernel::POSITION_DEPOSIT_USDC;
    pub const MaxPositionsPerAccount: u32 = pallet_conditional_ledger::core_ledger::MAX_POSITIONS_PER_ACCOUNT;
    pub const LedgerArchiveDelay: u64 = 100;
    pub const ReapBatch: u32 = kernel::REAP_BATCH;
    pub UsdcAssetId: AssetId = USDC;
    pub InsuranceAccount: AccountId = INSURANCE;
    pub const Fee: u128 = 30;
    pub const ObsInterval: u64 = 10;
    pub const Kappa1e9: u64 = 5_000_000;
    pub const MarketArchiveDelay: u64 = 100;
    pub static DecisionWindowMarkets: Vec<MarketId> = Vec::new();
    pub static RecordKeeperRebates: bool = false;
}

pub struct TestInDecisionWindow;

impl Contains<MarketId> for TestInDecisionWindow {
    fn contains(market: &MarketId) -> bool {
        DecisionWindowMarkets::get().contains(market)
    }
}

pub struct KeeperRebates;

impl KeeperRebates {
    const KEY: &'static [u8] = b":test:market:keeper-rebates";

    pub fn get() -> Vec<(AccountId, CrankClass)> {
        sp_io::storage::get(Self::KEY)
            .and_then(|encoded| {
                let mut input: &[u8] = encoded.as_ref();
                Vec::<(AccountId, CrankClass)>::decode(&mut input).ok()
            })
            .unwrap_or_default()
    }

    fn push(who: AccountId, class: CrankClass) {
        let mut rebates = Self::get();
        rebates.push((who, class));
        sp_io::storage::set(Self::KEY, &rebates.encode());
    }
}

pub struct TestKeeperRebate;

impl KeeperRebateSink<AccountId> for TestKeeperRebate {
    fn rebate(who: &AccountId, class: CrankClass) {
        if RecordKeeperRebates::get() {
            KeeperRebates::push(*who, class);
        }
    }
}

impl pallet_conditional_ledger::Config for Test {
    type Collateral = Assets;
    type UsdcAssetId = UsdcAssetId;
    type MarketAuthority = EnsureMarketPallet;
    type ResolveAuthority = EnsureResolver;
    type SettleAuthority = EnsureSettler;
    type EmergencyPlaybookOrigin = EnsureSettler;
    type MinSplit = MinSplit;
    type PositionDeposit = PositionDeposit;
    type MaxPositionsPerAccount = MaxPositionsPerAccount;
    type ArchiveDelay = LedgerArchiveDelay;
    type ReapBatch = ReapBatch;
    type ProtocolAccounts = Protocol;
    type InsuranceAccount = InsuranceAccount;
    type PalletId = LedgerPalletId;
    type KeeperRebate = ();
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = ();
}

impl pallet_market::Config for Test {
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = ();
    type Fee = Fee;
    type ObsInterval = ObsInterval;
    type Kappa1e9 = Kappa1e9;
    type MarketAdmin = EnsureMarketAdmin;
    type EmergencyPlaybookOrigin = EnsureMarketAdmin;
    type ArchiveDelay = MarketArchiveDelay;
    type PalletId = MarketPalletId;
    type KeeperRebate = TestKeeperRebate;
    type InDecisionWindow = TestInDecisionWindow;
}

pub fn ledger_account() -> AccountId {
    LedgerPalletId::get().into_account_truncating()
}

pub fn market_account() -> AccountId {
    MarketPalletId::get().into_account_truncating()
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    DecisionWindowMarkets::set(Vec::new());
    RecordKeeperRebates::set(false);
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .expect("mock system genesis builds");

    let native_accounts = vec![
        ALICE,
        BOB,
        CHARLIE,
        RESOLVER,
        SETTLER,
        MARKET_ADMIN,
        BOOK,
        FEES,
        POL,
        TREASURY,
        INSURANCE,
        ledger_account(),
        market_account(),
    ];
    pallet_balances::GenesisConfig::<Test> {
        balances: native_accounts
            .into_iter()
            .map(|who| (who, 1_000_000_000))
            .collect(),
        ..Default::default()
    }
    .assimilate_storage(&mut storage)
    .expect("mock balances genesis builds");

    let usdc_accounts = vec![
        ALICE,
        BOB,
        CHARLIE,
        BOOK,
        FEES,
        POL,
        TREASURY,
        INSURANCE,
        market_account(),
    ];
    pallet_assets::GenesisConfig::<Test> {
        assets: vec![(USDC, ALICE, true, 10_000)],
        metadata: vec![],
        accounts: usdc_accounts
            .into_iter()
            .map(|who| (USDC, who, 100_000 * UNIT))
            .chain(core::iter::once((USDC, ledger_account(), 10_000)))
            .collect(),
        next_asset_id: None,
        reserves: vec![],
    }
    .assimilate_storage(&mut storage)
    .expect("mock assets genesis builds");

    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
