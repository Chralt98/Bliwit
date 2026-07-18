//! Mock runtime for `pallet-conditional-ledger` tests.
//!
//! `System` + `Balances` (the deposit `Currency` `pallet-assets` needs) +
//! `Assets` (the USDC `ForeignAssets` stand-in — `T::Collateral`) + the ledger.
//! The three internal authorities are modelled as distinct signed accounts so the
//! origin-misuse suites can assert each call rejects the wrong authority.

use crate as pallet_conditional_ledger;
use conditional_ledger_core::MAX_POSITIONS_PER_ACCOUNT;
use frame_support::{
    derive_impl, parameter_types,
    traits::{AsEnsureOriginWithArg, Contains, EnsureOrigin},
    PalletId,
};
use frame_system::{EnsureSigned, RawOrigin};
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    kernel, Balance,
};
use sp_runtime::{traits::AccountIdConversion, BuildStorage};

pub type AccountId = u64;
pub type AssetId = u32;
type Block = frame_system::mocking::MockBlock<Test>;

// Named accounts.
pub const MARKET: AccountId = 100;
pub const RESOLVER: AccountId = 101;
pub const SETTLER: AccountId = 102;
pub const ALICE: AccountId = 1;
pub const BOB: AccountId = 2;
pub const CHARLIE: AccountId = 3;
/// Protocol accounts (POL/book/fee/INSURANCE) — cap- and deposit-exempt.
pub const BOOK: AccountId = 900;
pub const POL: AccountId = 901;
pub const INSURANCE: AccountId = 902;

/// The USDC asset id inside the mock `Assets` instance.
pub const USDC: AssetId = 1337;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Assets: pallet_assets,
        Ledger: pallet_conditional_ledger,
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

// ---- internal authorities: distinct signed accounts --------------------------

macro_rules! ensure_account {
    ($name:ident, $acct:expr) => {
        pub struct $name;
        impl EnsureOrigin<RuntimeOrigin> for $name {
            type Success = ();
            fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
                match o.clone().into() {
                    Ok(RawOrigin::Signed(who)) if who == $acct => Ok(()),
                    _ => Err(o),
                }
            }
            #[cfg(feature = "runtime-benchmarks")]
            fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
                Ok(RawOrigin::Signed($acct).into())
            }
        }
    };
}
ensure_account!(EnsureMarket, MARKET);
ensure_account!(EnsureResolver, RESOLVER);
ensure_account!(EnsureSettler, SETTLER);

pub struct Protocol;
impl Contains<AccountId> for Protocol {
    fn contains(who: &AccountId) -> bool {
        matches!(*who, BOOK | POL | INSURANCE) || *who == ledger_account()
    }
}

parameter_types! {
    pub const LedgerPalletId: PalletId = PalletId(*b"bl/ledgr");
    // `static` so tests can raise the live `ledger.min_split` above the kernel
    // floor (13 §1 lets META raise it to 1 USDC) to exercise the R-2 live-floor
    // paths; defaults to the kernel floor, fresh per test thread.
    pub static MinSplit: Balance = kernel::MIN_SPLIT_USDC;
    pub PositionDeposit: Balance = kernel::POSITION_DEPOSIT_USDC;
    pub const MaxPositionsPerAccount: u32 = MAX_POSITIONS_PER_ACCOUNT;
    pub const ArchiveDelay: u64 = 100; // blocks (short, for tests)
    pub static ReapBatch: u32 = kernel::REAP_BATCH;
    pub UsdcAssetId: AssetId = USDC;
    pub InsuranceAccount: AccountId = INSURANCE;
    /// Disabled by default, so the mock behaves like the `()` sink unless a
    /// keeper-rebate regression explicitly enables recording.
    pub static RecordKeeperRebates: bool = false;
    pub static KeeperRebates: Vec<(AccountId, CrankClass)> = Vec::new();
}

pub struct TestKeeperRebate;

impl KeeperRebateSink<AccountId> for TestKeeperRebate {
    fn rebate(who: &AccountId, class: CrankClass) {
        if RecordKeeperRebates::get() {
            let mut rebates = KeeperRebates::get();
            rebates.push((*who, class));
            KeeperRebates::set(rebates);
        }
    }
}

impl pallet_conditional_ledger::Config for Test {
    type Collateral = Assets;
    type UsdcAssetId = UsdcAssetId;
    type MarketAuthority = EnsureMarket;
    type ResolveAuthority = EnsureResolver;
    type SettleAuthority = EnsureSettler;
    type EmergencyPlaybookOrigin = EnsureSettler;
    type MinSplit = MinSplit;
    type PositionDeposit = PositionDeposit;
    type MaxPositionsPerAccount = MaxPositionsPerAccount;
    type ArchiveDelay = ArchiveDelay;
    type ReapBatch = ReapBatch;
    type ProtocolAccounts = Protocol;
    type InsuranceAccount = InsuranceAccount;
    type PalletId = LedgerPalletId;
    type KeeperRebate = TestKeeperRebate;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = ();
}

/// The ledger's sovereign account (custodies escrow + deposits).
pub fn ledger_account() -> AccountId {
    LedgerPalletId::get().into_account_truncating()
}

/// One USDC = 1e6 base units (6 decimals; `MIN_SPLIT` = 10⁴ = 0.01 USDC).
pub const UNIT: Balance = 1_000_000;

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    // Endow the deposit `Currency` so `pallet-assets` account creation never fails.
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (ALICE, 1_000_000_000),
            (BOB, 1_000_000_000),
            (CHARLIE, 1_000_000_000),
            (MARKET, 1_000_000_000),
            (BOOK, 1_000_000_000),
            (POL, 1_000_000_000),
            (INSURANCE, 1_000_000_000),
            (ledger_account(), 1_000_000_000),
        ],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();

    pallet_assets::GenesisConfig::<Test> {
        // id, owner, is_sufficient, min_balance = 10⁴ (13 §3.5 USDC ED).
        assets: vec![(USDC, ALICE, true, 10_000)],
        metadata: vec![],
        accounts: vec![
            (USDC, ALICE, 100_000 * UNIT),
            (USDC, BOB, 100_000 * UNIT),
            (USDC, CHARLIE, 100_000 * UNIT),
            (USDC, MARKET, 100_000 * UNIT),
            (USDC, BOOK, 100_000 * UNIT),
            (USDC, POL, 100_000 * UNIT),
            (USDC, INSURANCE, 100_000 * UNIT),
            (USDC, ledger_account(), 10_000), // one-ED genesis endowment (03 §1)
        ],
        next_asset_id: None,
        reserves: vec![],
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);
        ReapBatch::set(kernel::REAP_BATCH);
        RecordKeeperRebates::set(false);
        KeeperRebates::set(Vec::new());
    });
    ext
}
