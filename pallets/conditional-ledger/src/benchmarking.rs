//! Benchmarks for `pallet-conditional-ledger` (Track-A DoD: a `#[benchmarks]`
//! case per extrinsic and the `sweep_dust` cranks). PoV-calibrated weights are
//! generated in B5 (15 §4.5); this is the harness the generator consumes.

use crate::*;
use frame_benchmarking::v2::*;
use frame_support::traits::{fungibles::Mutate, EnsureOrigin, Get};
use frame_system::RawOrigin;
use futarchy_primitives::{kernel, Balance, Branch, FixedU64, GateType, ProposalId, ScalarSide};
use sp_runtime::traits::{AccountIdConversion, Saturating};

const UNIT: Balance = 1_000_000;
const SEED_AMT: Balance = 1_000 * UNIT;

fn fund<T: Config>(who: &T::AccountId, amount: Balance) {
    let _ = <T::Collateral as Mutate<T::AccountId>>::mint_into(T::UsdcAssetId::get(), who, amount);
}

fn fund_sovereign_reserve<T: Config>() {
    // Custody payouts use `Preservation::Preserve`: the protocol sovereign must
    // remain an asset account after paying the measured claimant. Production
    // custody naturally carries unrelated vault escrow; this isolated fixture
    // supplies the same keep-alive reserve without changing ledger accounting.
    let sovereign = T::PalletId::get().into_account_truncating();
    fund::<T>(&sovereign, SEED_AMT);
}

fn market_origin<T: Config>() -> T::RuntimeOrigin {
    T::MarketAuthority::try_successful_origin().expect("mock provides a MarketAuthority origin")
}
fn resolve_origin<T: Config>() -> T::RuntimeOrigin {
    T::ResolveAuthority::try_successful_origin().expect("mock provides a ResolveAuthority origin")
}
fn settle_origin<T: Config>() -> T::RuntimeOrigin {
    T::SettleAuthority::try_successful_origin().expect("mock provides a SettleAuthority origin")
}
fn emergency_origin<T: Config>() -> Result<T::RuntimeOrigin, BenchmarkError> {
    T::EmergencyPlaybookOrigin::try_successful_origin()
        .map_err(|_| BenchmarkError::Stop("EmergencyPlaybook origin unavailable"))
}

/// A funded, `Open` proposal vault with the caller already holding branch-USDC.
fn seeded_vault<T: Config>(pid: ProposalId, caller: &T::AccountId) {
    Pallet::<T>::create_vault(market_origin::<T>(), pid, 0).expect("create vault");
    fund::<T>(caller, SEED_AMT.saturating_mul(4));
    Pallet::<T>::split(RawOrigin::Signed(caller.clone()).into(), pid, SEED_AMT).expect("split");
    fund_sovereign_reserve::<T>();
}

fn seeded_baseline<T: Config>(epoch: futarchy_primitives::EpochId, caller: &T::AccountId) {
    Pallet::<T>::create_baseline_vault(market_origin::<T>(), epoch).expect("create baseline");
    fund::<T>(caller, SEED_AMT.saturating_mul(4));
    Pallet::<T>::split_baseline(RawOrigin::Signed(caller.clone()).into(), epoch, SEED_AMT)
        .expect("split baseline");
    fund_sovereign_reserve::<T>();
}

fn seeded_vault_reap_batch<T: Config>(pid: ProposalId) {
    Pallet::<T>::create_vault(market_origin::<T>(), pid, 0).expect("create vault");
    // Each split creates two position entries. Fifty distinct owners therefore
    // put exactly the 13 §4 `ledger.reap_batch = 100` entries on the prefixes
    // this measured sweep walks, while remaining below the per-account cap.
    for index in 0..(T::ReapBatch::get() / 2) {
        let who: T::AccountId = account("dust", index, 0);
        fund::<T>(&who, SEED_AMT.saturating_mul(2));
        Pallet::<T>::split(RawOrigin::Signed(who).into(), pid, SEED_AMT).expect("split");
    }
    fund_sovereign_reserve::<T>();
}

fn seeded_baseline_reap_batch<T: Config>(epoch: futarchy_primitives::EpochId) {
    Pallet::<T>::create_baseline_vault(market_origin::<T>(), epoch).expect("create baseline");
    for index in 0..(T::ReapBatch::get() / 2) {
        let who: T::AccountId = account("base-dust", index, 0);
        fund::<T>(&who, SEED_AMT.saturating_mul(2));
        Pallet::<T>::split_baseline(RawOrigin::Signed(who).into(), epoch, SEED_AMT)
            .expect("split baseline");
    }
    fund_sovereign_reserve::<T>();
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn split() {
        let caller: T::AccountId = whitelisted_caller();
        Pallet::<T>::create_vault(market_origin::<T>(), 1, 0).unwrap();
        fund::<T>(&caller, SEED_AMT.saturating_mul(2));
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1, SEED_AMT);
        assert!(Vaults::<T>::get(1).unwrap().escrowed == SEED_AMT);
    }

    #[benchmark]
    fn merge() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1, SEED_AMT);
    }

    #[benchmark]
    fn split_scalar() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            Branch::Accept,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn merge_scalar() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::split_scalar(
            RawOrigin::Signed(caller.clone()).into(),
            1,
            Branch::Accept,
            SEED_AMT,
        )
        .unwrap();
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            Branch::Accept,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn split_gate() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            Branch::Accept,
            GateType::Survival,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn merge_gate() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::split_gate(
            RawOrigin::Signed(caller.clone()).into(),
            1,
            Branch::Accept,
            GateType::Survival,
            SEED_AMT,
        )
        .unwrap();
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            Branch::Accept,
            GateType::Survival,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn transfer() {
        let caller: T::AccountId = whitelisted_caller();
        let dest: T::AccountId = account("dest", 0, 0);
        fund::<T>(&dest, SEED_AMT);
        seeded_vault::<T>(1, &caller);
        let id = crate::core_ledger::position(
            1,
            Branch::Accept,
            futarchy_primitives::PositionKind::BranchUsdc,
        );
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            id,
            dest.clone(),
            SEED_AMT,
        );
    }

    #[benchmark]
    fn split_baseline() {
        let caller: T::AccountId = whitelisted_caller();
        Pallet::<T>::create_baseline_vault(market_origin::<T>(), 7).unwrap();
        fund::<T>(&caller, SEED_AMT.saturating_mul(2));
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 7, SEED_AMT);
    }

    #[benchmark]
    fn merge_baseline() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_baseline::<T>(7, &caller);
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 7, SEED_AMT);
    }

    #[benchmark]
    fn resolve() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        let origin = resolve_origin::<T>();
        #[block]
        {
            Pallet::<T>::resolve(origin, 1, Branch::Accept).unwrap();
        }
        assert!(Vaults::<T>::get(1).is_some());
    }

    #[benchmark]
    fn void() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        let origin = resolve_origin::<T>();
        #[block]
        {
            Pallet::<T>::void(origin, 1).unwrap();
        }
        assert!(VaultTerminalAt::<T>::get(1).is_some());
    }

    #[benchmark]
    fn settle_scalar() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::resolve(resolve_origin::<T>(), 1, Branch::Accept).unwrap();
        let origin = settle_origin::<T>();
        #[block]
        {
            Pallet::<T>::settle_scalar(origin, 1, FixedU64(500_000_000)).unwrap();
        }
    }

    #[benchmark]
    fn settle_gate() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::resolve(resolve_origin::<T>(), 1, Branch::Accept).unwrap();
        let origin = settle_origin::<T>();
        #[block]
        {
            Pallet::<T>::settle_gate(origin, 1, GateType::Survival, true).unwrap();
        }
    }

    #[benchmark]
    fn settle_baseline() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_baseline::<T>(7, &caller);
        let origin = settle_origin::<T>();
        #[block]
        {
            Pallet::<T>::settle_baseline(origin, 7, FixedU64(500_000_000)).unwrap();
        }
    }

    #[benchmark]
    fn redeem() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::resolve(resolve_origin::<T>(), 1, Branch::Accept).unwrap();
        Pallet::<T>::settle_scalar(settle_origin::<T>(), 1, FixedU64(500_000_000)).unwrap();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1, SEED_AMT);
    }

    #[benchmark]
    fn redeem_scalar() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::split_scalar(
            RawOrigin::Signed(caller.clone()).into(),
            1,
            Branch::Accept,
            SEED_AMT,
        )
        .unwrap();
        Pallet::<T>::resolve(resolve_origin::<T>(), 1, Branch::Accept).unwrap();
        Pallet::<T>::settle_scalar(settle_origin::<T>(), 1, FixedU64(500_000_000)).unwrap();
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            ScalarSide::Long,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn redeem_scalar_pair() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::split_scalar(
            RawOrigin::Signed(caller.clone()).into(),
            1,
            Branch::Accept,
            SEED_AMT,
        )
        .unwrap();
        Pallet::<T>::resolve(resolve_origin::<T>(), 1, Branch::Accept).unwrap();
        Pallet::<T>::settle_scalar(settle_origin::<T>(), 1, FixedU64(500_000_000)).unwrap();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1, SEED_AMT);
    }

    #[benchmark]
    fn redeem_gate() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::split_gate(
            RawOrigin::Signed(caller.clone()).into(),
            1,
            Branch::Accept,
            GateType::Survival,
            SEED_AMT,
        )
        .unwrap();
        Pallet::<T>::resolve(resolve_origin::<T>(), 1, Branch::Accept).unwrap();
        Pallet::<T>::settle_scalar(settle_origin::<T>(), 1, FixedU64(500_000_000)).unwrap();
        Pallet::<T>::settle_gate(settle_origin::<T>(), 1, GateType::Survival, true).unwrap();
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            GateType::Survival,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn redeem_void() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault::<T>(1, &caller);
        Pallet::<T>::void(resolve_origin::<T>(), 1).unwrap();
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            Branch::Accept,
            futarchy_primitives::PositionKind::BranchUsdc,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn redeem_baseline() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_baseline::<T>(7, &caller);
        Pallet::<T>::settle_baseline(settle_origin::<T>(), 7, FixedU64(500_000_000)).unwrap();
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            7,
            ScalarSide::Long,
            SEED_AMT,
        );
    }

    #[benchmark]
    fn redeem_baseline_pair() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_baseline::<T>(7, &caller);
        Pallet::<T>::settle_baseline(settle_origin::<T>(), 7, FixedU64(500_000_000)).unwrap();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 7, SEED_AMT);
    }

    #[benchmark]
    fn sweep_dust() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_vault_reap_batch::<T>(1);
        Pallet::<T>::void(resolve_origin::<T>(), 1).unwrap();
        // Force reap-eligibility by back-dating the terminal block.
        VaultTerminalAt::<T>::insert(
            1,
            frame_system::pallet_prelude::BlockNumberFor::<T>::from(0u32),
        );
        frame_system::Pallet::<T>::set_block_number(T::ArchiveDelay::get() + 10u32.into());
        T::BenchmarkHelper::prime_keeper_rebate();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1);
        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
    }

    #[benchmark]
    fn sweep_dust_baseline() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_baseline_reap_batch::<T>(7);
        Pallet::<T>::settle_baseline(settle_origin::<T>(), 7, FixedU64(500_000_000)).unwrap();
        BaselineTerminalAt::<T>::insert(
            7,
            frame_system::pallet_prelude::BlockNumberFor::<T>::from(0u32),
        );
        frame_system::Pallet::<T>::set_block_number(T::ArchiveDelay::get() + 10u32.into());
        T::BenchmarkHelper::prime_keeper_rebate();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 7);
        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
    }

    #[benchmark]
    fn set_split_paused() -> Result<(), BenchmarkError> {
        let expiry = frame_system::Pallet::<T>::block_number()
            .saturating_add(kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS.into());
        let origin = emergency_origin::<T>()?;
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, true, expiry);
        assert_eq!(SplitPausedUntil::<T>::get(), Some(expiry));
        Ok(())
    }

    #[benchmark]
    fn set_frozen() -> Result<(), BenchmarkError> {
        let origin = emergency_origin::<T>()?;
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, true);
        assert!(FrozenUntil::<T>::get().is_some());
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
