//! Mock runtime for `pallet-registry` (15 §4.1).
//!
//! The runtime account is `AccountId32` (02 §8), so `T::AccountId` satisfies the
//! `Into<[u8; 32]> + From<[u8; 32]>` bridge the pallet requires. The pallet is
//! deployed **twice** — `IncidentRegistry` (default instance) and
//! `MilestoneRegistry` (`Instance1`) — proving the "one pallet, two instances"
//! shape (07 §7). USDC bonds ride a `pallet-assets` instance (`T::Collateral`);
//! the cross-pallet seams (`Params`, `Watchtowers`, `Welfare`, `Epoch`,
//! `ResolutionAuthority`) are thread-local statics so tests can drive them.

use crate as pallet_registry;
use crate::{EpochContext, RegistryParams, WatchtowerRegistry, WelfareSink};
use frame_support::{
    derive_impl,
    instances::Instance1,
    parameter_types,
    traits::{AsEnsureOriginWithArg, EnsureOrigin},
    PalletId,
};
use frame_system::{EnsureSigned, RawOrigin};
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    Balance, EpochId, FixedU64,
};
use registry_core::{RegistryKind, REG_BOND_INCIDENT, REG_BOND_MILESTONE};
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage};

pub type AccountId = AccountId32;
pub type AssetId = u32;
type Block = frame_system::mocking::MockBlock<Test>;

// Incident is the **default** instance `()` (so the benchmark test-suite, which
// targets the default, has a `Config<()>`); Milestone is `Instance1`. Two
// distinct instances still exercise the "one pallet, two instances" shape.
pub type IncidentInstance = ();
pub type MilestoneInstance = Instance1;

/// The USDC asset id inside the mock `Assets` instance.
pub const USDC: AssetId = 1337;
/// One USDC = 1e6 base units (6 decimals).
pub const UNIT: Balance = 1_000_000;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Assets: pallet_assets,
        IncidentRegistry: pallet_registry,
        MilestoneRegistry: pallet_registry::<Instance1>,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
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

// ---- named accounts ----------------------------------------------------------

/// Raw 32-byte account.
pub fn acct(n: u8) -> AccountId32 {
    AccountId32::from([n; 32])
}
pub fn raw(n: u8) -> [u8; 32] {
    [n; 32]
}

pub const ALICE: u8 = 1;
pub const BOB: u8 = 2;
pub const CHARLIE: u8 = 3;
pub const WT1: u8 = 10;
pub const WT2: u8 = 11;
pub const WT3: u8 = 12;
pub const RESOLVER: u8 = 20;
pub const INSURANCE: u8 = 30;

pub const INCIDENT_PALLET_ID: PalletId = PalletId(*b"bl/reg/i");
pub const MILESTONE_PALLET_ID: PalletId = PalletId(*b"bl/reg/m");

// ---- seam statics ------------------------------------------------------------

parameter_types! {
    /// Live `reg.bond_*`, overridable per-test to prove the pallet reads `Params`
    /// (rule 4), never a hardcode. Default = the 13 §1 defaults.
    pub static BondIncident: Balance = REG_BOND_INCIDENT;
    pub static BondMilestone: Balance = REG_BOND_MILESTONE;
    /// The filing-window-end block (07 §7); a single far-future default so filing
    /// is open — tests lower it to exercise the `WindowClosed` path.
    pub static FilingWindowEnd: u32 = 1_000_000;
    /// The frozen MetricSpec version filings must attest under (I-16); tests file
    /// a mismatching version to exercise `SpecVersionMismatch`.
    pub static FrozenSpec: u16 = 3;
    /// The Milestone completion target (frozen MetricSpec field, 07 §7 / 05 §4.4);
    /// overridable per-test to prove it is a seam, not the core's `100` default.
    pub static MilestoneTarget: u32 = 100;
    /// `reg.archive_delay` in blocks — short for tests (the ledger uses 1 yr).
    pub const ArchiveDelay: u64 = 100;
    /// The registered bonded watchtowers (07 §4).
    pub static RegisteredWatchtowers: alloc::vec::Vec<AccountId32> = alloc::vec::Vec::new();
    /// Welfare hand-off log — `(kind, epoch, aggregate.0)`; tests assert the
    /// settlement-time consumer received the derived aggregate.
    pub static WelfareLog: alloc::vec::Vec<(RegistryKind, EpochId, u64)> = alloc::vec::Vec::new();
    /// When set, the welfare sink refuses — exercises the G-1 `close_epoch`
    /// rollback path (07 §7 / rule 1).
    pub static WelfareFails: bool = false;
    /// Per-instance keeper rebates; both registry instances bind the recording
    /// double so tests cover the 07 §4 mandate on each concrete instance.
    pub static KeeperRebates: alloc::vec::Vec<(AccountId32, CrankClass)> = alloc::vec::Vec::new();
    pub const MaxFilingsPerEpoch: u32 = registry_core::MAX_FILINGS_PER_EPOCH;
    pub const MaxEvidenceLen: u32 = 32;
    pub UsdcAssetId: AssetId = USDC;
    pub IncidentPalletId: PalletId = INCIDENT_PALLET_ID;
    pub MilestonePalletId: PalletId = MILESTONE_PALLET_ID;
    pub IncidentKind: RegistryKind = RegistryKind::Incident;
    pub MilestoneKind: RegistryKind = RegistryKind::Milestone;
    pub InsuranceAccount: AccountId32 = acct(INSURANCE);
}

extern crate alloc;

pub struct TestParams;
impl RegistryParams for TestParams {
    fn bond_incident() -> Balance {
        BondIncident::get()
    }
    fn bond_milestone() -> Balance {
        BondMilestone::get()
    }
}

pub struct TestWatchtowers;
impl WatchtowerRegistry<AccountId32> for TestWatchtowers {
    fn is_registered_watchtower(who: &AccountId32) -> bool {
        RegisteredWatchtowers::get().iter().any(|w| w == who)
    }
}

pub struct TestWelfare;
impl WelfareSink for TestWelfare {
    fn note_external_component(
        kind: RegistryKind,
        epoch: EpochId,
        aggregate: FixedU64,
    ) -> sp_runtime::DispatchResult {
        if WelfareFails::get() {
            return Err(sp_runtime::DispatchError::Other("welfare refused"));
        }
        WelfareLog::mutate(|log| log.push((kind, epoch, aggregate.0)));
        Ok(())
    }
}

pub struct TestEpoch;
impl EpochContext for TestEpoch {
    fn filing_window_end(_epoch: EpochId) -> u32 {
        FilingWindowEnd::get()
    }
    fn frozen_spec_version(_epoch: EpochId) -> u16 {
        FrozenSpec::get()
    }
    fn milestone_target(_epoch: EpochId) -> u32 {
        MilestoneTarget::get()
    }
}

pub struct RecordingKeeperRebate;
impl KeeperRebateSink<AccountId32> for RecordingKeeperRebate {
    fn rebate(who: &AccountId32, class: CrankClass) {
        KeeperRebates::mutate(|rebates| rebates.push((who.clone(), class)));
    }
}

/// The resolution authority (07 §7): a fixed `RESOLVER` account stands in for the
/// recompute-keeper / `OracleResolution` path wired in B1a.
pub struct TestResolutionAuthority;
impl EnsureOrigin<RuntimeOrigin> for TestResolutionAuthority {
    type Success = ();
    fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        match o.clone().into() {
            Ok(RawOrigin::Signed(who)) if who == acct(RESOLVER) => Ok(()),
            _ => Err(o),
        }
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RawOrigin::Signed(acct(RESOLVER)).into())
    }
}

impl pallet_registry::Config<IncidentInstance> for Test {
    type Collateral = Assets;
    type UsdcAssetId = UsdcAssetId;
    type Kind = IncidentKind;
    type Params = TestParams;
    type Watchtowers = TestWatchtowers;
    type Welfare = TestWelfare;
    type Epoch = TestEpoch;
    type ResolutionAuthority = TestResolutionAuthority;
    type InsuranceAccount = InsuranceAccount;
    type PalletId = IncidentPalletId;
    type ArchiveDelay = ArchiveDelay;
    type MaxFilingsPerEpoch = MaxFilingsPerEpoch;
    type MaxEvidenceLen = MaxEvidenceLen;
    type WeightInfo = ();
    type KeeperRebate = RecordingKeeperRebate;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

impl pallet_registry::Config<MilestoneInstance> for Test {
    type Collateral = Assets;
    type UsdcAssetId = UsdcAssetId;
    type Kind = MilestoneKind;
    type Params = TestParams;
    type Watchtowers = TestWatchtowers;
    type Welfare = TestWelfare;
    type Epoch = TestEpoch;
    type ResolutionAuthority = TestResolutionAuthority;
    type InsuranceAccount = InsuranceAccount;
    type PalletId = MilestonePalletId;
    type ArchiveDelay = ArchiveDelay;
    type MaxFilingsPerEpoch = MaxFilingsPerEpoch;
    type MaxEvidenceLen = MaxEvidenceLen;
    type WeightInfo = ();
    type KeeperRebate = RecordingKeeperRebate;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl crate::BenchmarkHelper<RuntimeOrigin, AccountId32> for TestBenchmarkHelper {
    fn resolution_origin() -> RuntimeOrigin {
        RawOrigin::Signed(acct(RESOLVER)).into()
    }
    fn funded_account(seed: u8) -> AccountId32 {
        acct(seed)
    }
    fn register_watchtower(who: &AccountId32) {
        RegisteredWatchtowers::mutate(|s| {
            if !s.iter().any(|w| w == who) {
                s.push(who.clone());
            }
        });
    }
    fn prime_epoch(_: EpochId) {}
}

// ---- test helpers ------------------------------------------------------------

/// Incident-instance sovereign (bond custody).
pub fn incident_account() -> AccountId32 {
    use sp_runtime::traits::AccountIdConversion;
    INCIDENT_PALLET_ID.into_account_truncating()
}
/// Milestone-instance sovereign (bond custody).
pub fn milestone_account() -> AccountId32 {
    use sp_runtime::traits::AccountIdConversion;
    MILESTONE_PALLET_ID.into_account_truncating()
}

/// USDC balance of an account.
pub fn usdc(who: &AccountId32) -> Balance {
    use frame_support::traits::fungibles::Inspect;
    <Assets as Inspect<AccountId32>>::balance(USDC, who)
}

/// Register a watchtower for the mock oracle registry (07 §4).
pub fn register_watchtower(n: u8) {
    RegisteredWatchtowers::mutate(|s| s.push(acct(n)));
}

/// Set the filing-window-end block (07 §7).
pub fn set_filing_window_end(block: u32) {
    FilingWindowEnd::set(block);
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    let funded: alloc::vec::Vec<AccountId32> =
        [ALICE, BOB, CHARLIE, WT1, WT2, WT3, RESOLVER, INSURANCE]
            .into_iter()
            .map(acct)
            .chain([incident_account(), milestone_account()])
            .collect();

    pallet_balances::GenesisConfig::<Test> {
        balances: funded.iter().cloned().map(|a| (a, 1_000_000_000)).collect(),
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();

    // USDC is `is_sufficient = true` so any account may hold it without a native
    // ED and a transfer creates the destination. Only the filing/challenge parties
    // are pre-funded (each bond is ≤ 5,000 USDC); the sovereigns and INSURANCE
    // start at 0 so custody-conservation assertions read cleanly.
    // Fund generously — the per-epoch cap test escrows 64 × 5,000 USDC from one
    // filer, so ≥ 320,000 USDC is required.
    let accounts: alloc::vec::Vec<(AssetId, AccountId32, Balance)> = [ALICE, BOB, CHARLIE]
        .into_iter()
        .map(|a| (USDC, acct(a), 1_000_000 * UNIT))
        .collect();
    pallet_assets::GenesisConfig::<Test> {
        assets: vec![(USDC, acct(ALICE), true, 10_000)],
        metadata: vec![],
        accounts,
        next_asset_id: None,
        reserves: vec![],
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);
        // Reset overridable statics to defaults for a clean per-test start.
        BondIncident::set(REG_BOND_INCIDENT);
        BondMilestone::set(REG_BOND_MILESTONE);
        FilingWindowEnd::set(1_000_000);
        FrozenSpec::set(3);
        MilestoneTarget::set(100);
        RegisteredWatchtowers::set(alloc::vec::Vec::new());
        WelfareLog::set(alloc::vec::Vec::new());
        WelfareFails::set(false);
        KeeperRebates::set(alloc::vec::Vec::new());
    });
    ext
}
