//! Mock runtime for `pallet-attestor` (15 §4.1).

use crate as pallet_attestor;
use frame_support::{derive_impl, traits::EnsureOrigin};
use sp_core::crypto::AccountId32;
use sp_runtime::{traits::IdentityLookup, BuildStorage};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Attestor: pallet_attestor,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
}

pub const VALUES_ACC: [u8; 32] = [200; 32];
pub const RATIFY_ACC: [u8; 32] = [201; 32];

/// Mock `ConstitutionalValues` origin resolver.
pub struct TestValuesOrigin;
impl EnsureOrigin<RuntimeOrigin> for TestValuesOrigin {
    type Success = ();

    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let raw: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> =
            origin.clone().into();
        match raw {
            Ok(frame_system::RawOrigin::Signed(who)) if who == AccountId32::from(VALUES_ACC) => {
                Ok(())
            }
            _ => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(AccountId32::from(VALUES_ACC)))
    }
}

/// Mock `ratify`-track origin resolver.
pub struct TestRatifyOrigin;
impl EnsureOrigin<RuntimeOrigin> for TestRatifyOrigin {
    type Success = ();

    fn try_origin(origin: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let raw: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> =
            origin.clone().into();
        match raw {
            Ok(frame_system::RawOrigin::Signed(who)) if who == AccountId32::from(RATIFY_ACC) => {
                Ok(())
            }
            _ => Err(origin),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(AccountId32::from(RATIFY_ACC)))
    }
}

impl pallet_attestor::Config for Test {
    type ValuesOrigin = TestValuesOrigin;
    type RatifyOrigin = TestRatifyOrigin;
    type WeightInfo = ();

    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_attestor::BenchmarkHelper<RuntimeOrigin> for TestBenchmarkHelper {
    fn signed(who: [u8; 32]) -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::from(who))
    }

    fn values() -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::from(VALUES_ACC))
    }

    fn ratify() -> RuntimeOrigin {
        RuntimeOrigin::signed(AccountId32::from(RATIFY_ACC))
    }
}

pub fn acct(n: u8) -> AccountId32 {
    AccountId32::from([n; 32])
}

pub fn members() -> Vec<AccountId32> {
    vec![acct(1), acct(2), acct(3)]
}

pub fn values_origin() -> RuntimeOrigin {
    RuntimeOrigin::signed(AccountId32::from(VALUES_ACC))
}

pub fn ratify_origin() -> RuntimeOrigin {
    RuntimeOrigin::signed(AccountId32::from(RATIFY_ACC))
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_attestor::GenesisConfig::<Test> {
        members: members(),
        _config: Default::default(),
    })
}

pub fn new_test_ext_empty() -> sp_io::TestExternalities {
    new_test_ext_with(pallet_attestor::GenesisConfig::<Test>::default())
}

pub fn new_test_ext_with(
    attestor: pallet_attestor::GenesisConfig<Test>,
) -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        attestor,
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

pub fn set_block(block: u64) {
    System::set_block_number(block);
}
