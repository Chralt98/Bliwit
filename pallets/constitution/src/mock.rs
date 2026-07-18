//! Mock runtime for `pallet-constitution` (15 §4.1).

use crate as pallet_constitution;
use crate::ConstitutionOrigin;
use frame_support::{derive_impl, parameter_types, traits::EnsureOrigin};
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        // D-14 (02 §12): the instance name `Constitution` is part of the
        // frozen raw storage key — tests assert the derived key.
        Constitution: pallet_constitution,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

parameter_types! {
    pub static CurrentEpochValue: u32 = 0;
}

/// Accounts the test resolver maps onto the 06 §3 authority-matrix origins;
/// any other signed account resolves to `Signed`, which every governance
/// predicate refuses (defense-in-depth negative path).
pub const PARAM_ACC: u64 = 1;
pub const TREASURY_ACC: u64 = 2;
pub const CODE_ACC: u64 = 3;
pub const META_ACC: u64 = 4;
pub const VALUES_ACC: u64 = 5;
pub const GUARDIAN_ACC: u64 = 6;
pub const PLAYBOOK_ACC: u64 = 7;
pub const NOBODY_ACC: u64 = 99;

/// Test stand-in for the runtime's `pallet-origins`-backed resolver (A4/B1a).
pub struct TestGovernanceOrigin;

impl EnsureOrigin<RuntimeOrigin> for TestGovernanceOrigin {
    type Success = ConstitutionOrigin;

    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let raw: Result<frame_system::RawOrigin<u64>, RuntimeOrigin> = origin.into();
        match raw {
            Ok(frame_system::RawOrigin::Root) => Ok(ConstitutionOrigin::Root),
            Ok(frame_system::RawOrigin::Signed(PARAM_ACC)) => Ok(ConstitutionOrigin::FutarchyParam),
            Ok(frame_system::RawOrigin::Signed(TREASURY_ACC)) => {
                Ok(ConstitutionOrigin::FutarchyTreasury)
            }
            Ok(frame_system::RawOrigin::Signed(CODE_ACC)) => Ok(ConstitutionOrigin::FutarchyCode),
            Ok(frame_system::RawOrigin::Signed(META_ACC)) => Ok(ConstitutionOrigin::FutarchyMeta),
            Ok(frame_system::RawOrigin::Signed(VALUES_ACC)) => {
                Ok(ConstitutionOrigin::ConstitutionalValues)
            }
            Ok(frame_system::RawOrigin::Signed(GUARDIAN_ACC)) => {
                Ok(ConstitutionOrigin::GuardianHold)
            }
            Ok(frame_system::RawOrigin::Signed(PLAYBOOK_ACC)) => {
                Ok(ConstitutionOrigin::EmergencyPlaybook)
            }
            Ok(frame_system::RawOrigin::Signed(_)) => Ok(ConstitutionOrigin::Signed),
            Ok(other) => Err(other.into()),
            Err(origin) => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::root())
    }
}

impl pallet_constitution::Config for Test {
    type GovernanceOrigin = TestGovernanceOrigin;
    type CurrentEpoch = CurrentEpochValue;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_constitution::BenchmarkHelper<RuntimeOrigin> for TestBenchmarkHelper {
    fn origin(authority: ConstitutionOrigin) -> RuntimeOrigin {
        match authority {
            ConstitutionOrigin::FutarchyParam => RuntimeOrigin::signed(PARAM_ACC),
            ConstitutionOrigin::FutarchyTreasury => RuntimeOrigin::signed(TREASURY_ACC),
            ConstitutionOrigin::FutarchyCode => RuntimeOrigin::signed(CODE_ACC),
            ConstitutionOrigin::FutarchyMeta => RuntimeOrigin::signed(META_ACC),
            ConstitutionOrigin::ConstitutionTrack
            | ConstitutionOrigin::EntrenchedTrack
            | ConstitutionOrigin::ConstitutionalValues => RuntimeOrigin::signed(VALUES_ACC),
            ConstitutionOrigin::GuardianHold => RuntimeOrigin::signed(GUARDIAN_ACC),
            ConstitutionOrigin::EmergencyPlaybook => RuntimeOrigin::signed(PLAYBOOK_ACC),
            ConstitutionOrigin::Root => RuntimeOrigin::root(),
            ConstitutionOrigin::Signed => RuntimeOrigin::signed(NOBODY_ACC),
        }
    }
}

/// Externalities with the default (code-owned 13 §1 registry) genesis.
pub fn new_test_ext() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_constitution::GenesisConfig::default())
}

/// Externalities with an explicit constitution genesis.
pub fn new_test_ext_with(
    constitution: pallet_constitution::GenesisConfig<Test>,
) -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        constitution,
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Drive the mock epoch clock (`Config::CurrentEpoch`).
pub fn set_epoch(epoch: u32) {
    CurrentEpochValue::set(epoch);
}
