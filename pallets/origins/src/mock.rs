//! Mock runtime for `pallet-origins` (15 §4.1).
//!
//! A real `construct_runtime!` that includes the pallet's `#[pallet::origin]`,
//! so the tests exercise the genuine FRAME origin integration — building the
//! custom origins inside `RuntimeOrigin`, resolving them through the
//! `EnsureOrigin` set, and proving no `RawOrigin::Signed`/`Root` resolves to a
//! custom variant (G-5, I-10).

use crate as pallet_origins;
use frame_support::derive_impl;
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Origins: pallet_origins,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

impl pallet_origins::Config for Test {
    type WeightInfo = ();
}

/// Externalities at block 1.
pub fn new_test_ext() -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
