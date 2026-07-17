//! FRAME v2 benchmarks for every public market call and internal admin operation.

use crate::*;
use frame_benchmarking::v2::*;
use frame_support::traits::{fungibles::Mutate, EnsureOrigin, Get};
use frame_system::RawOrigin;
use futarchy_primitives::{Balance, Branch, MarketId, ScalarSide};
use market_core::{BookKind, MarketPhase};
use sp_runtime::traits::Saturating;

const UNIT: Balance = 1_000_000;
const B: Balance = 1_000 * UNIT;
// Generated benchmark accounts are not necessarily runtime protocol accounts;
// keep the fee leg above the ledger's live position-creation floor.
const TRADE: Balance = 10 * UNIT;

fn fund<T: Config>(who: &T::AccountId, amount: Balance) {
    <T::Collateral as Mutate<T::AccountId>>::mint_into(T::UsdcAssetId::get(), who, amount)
        .expect("benchmark collateral mint succeeds");
}

fn admin_origin<T: Config>() -> T::RuntimeOrigin {
    T::MarketAdmin::try_successful_origin().expect("benchmark MarketAdmin origin exists")
}

fn seeded_decision<T: Config>(market: MarketId) -> (T::AccountId, T::AccountId, T::AccountId) {
    let book: T::AccountId = account("book", market as u32, 0);
    let fees: T::AccountId = account("fees", market as u32, 0);
    let treasury: T::AccountId = account("treasury", market as u32, 0);
    fund::<T>(&book, 10_000 * UNIT);
    fund::<T>(&fees, 10_000 * UNIT);
    fund::<T>(&treasury, 10_000 * UNIT);
    Pallet::<T>::create_market(
        admin_origin::<T>(),
        market,
        BookKind::Decision {
            proposal: market,
            branch: Branch::Accept,
        },
        book.clone(),
        fees.clone(),
        B,
    )
    .expect("benchmark market creation succeeds");
    Pallet::<T>::seed(admin_origin::<T>(), market, treasury.clone())
        .expect("benchmark seeding succeeds");
    (book, fees, treasury)
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn buy() {
        let caller: T::AccountId = whitelisted_caller();
        fund::<T>(&caller, 10_000 * UNIT);
        seeded_decision::<T>(1);
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        );
        assert_eq!(Markets::<T>::get(1).expect("book exists").q_long, TRADE);
    }

    #[benchmark]
    fn sell() {
        let caller: T::AccountId = whitelisted_caller();
        fund::<T>(&caller, 10_000 * UNIT);
        seeded_decision::<T>(1);
        Pallet::<T>::buy(
            RawOrigin::Signed(caller.clone()).into(),
            1,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        )
        .expect("benchmark buy succeeds");
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            ScalarSide::Long,
            TRADE,
            0,
        );
        assert_eq!(Markets::<T>::get(1).expect("book exists").q_long, 0);
    }

    #[benchmark]
    fn crank_observe() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_decision::<T>(1);
        let now = frame_system::Pallet::<T>::block_number();
        frame_system::Pallet::<T>::set_block_number(now.saturating_add(100u32.into()));
        <T as Config>::BenchmarkHelper::prime_keeper_rebate();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1);
        <T as Config>::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert!(
            Markets::<T>::get(1)
                .expect("book exists")
                .last_observed_block
                > 0
        );
    }

    #[benchmark]
    fn reap() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_decision::<T>(1);
        Pallet::<T>::close(admin_origin::<T>(), 1).expect("benchmark close succeeds");
        let now = frame_system::Pallet::<T>::block_number();
        frame_system::Pallet::<T>::set_block_number(
            now.saturating_add(<T as Config>::ArchiveDelay::get())
                .saturating_add(1u32.into()),
        );
        <T as Config>::BenchmarkHelper::prime_keeper_rebate();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1);
        <T as Config>::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert!(!Markets::<T>::contains_key(1));
    }

    #[benchmark]
    fn create_market() {
        let book: T::AccountId = account("book", 0, 0);
        let fees: T::AccountId = account("fees", 0, 0);
        #[block]
        {
            Pallet::<T>::create_market(
                admin_origin::<T>(),
                1,
                BookKind::Decision {
                    proposal: 1,
                    branch: Branch::Accept,
                },
                book,
                fees,
                B,
            )
            .expect("benchmark market creation succeeds");
        }
        assert!(Markets::<T>::contains_key(1));
    }

    #[benchmark]
    fn seed() {
        let book: T::AccountId = account("book", 0, 0);
        let fees: T::AccountId = account("fees", 0, 0);
        let treasury: T::AccountId = account("treasury", 0, 0);
        fund::<T>(&book, 10_000 * UNIT);
        fund::<T>(&fees, 10_000 * UNIT);
        fund::<T>(&treasury, 10_000 * UNIT);
        Pallet::<T>::create_market(
            admin_origin::<T>(),
            1,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            book,
            fees,
            B,
        )
        .expect("benchmark market creation succeeds");
        #[block]
        {
            Pallet::<T>::seed(admin_origin::<T>(), 1, treasury)
                .expect("benchmark seeding succeeds");
        }
    }

    #[benchmark]
    fn close() {
        seeded_decision::<T>(1);
        #[block]
        {
            Pallet::<T>::close(admin_origin::<T>(), 1).expect("benchmark close succeeds");
        }
        assert!(matches!(
            Markets::<T>::get(1).expect("book exists").phase,
            MarketPhase::Closed
        ));
    }

    #[benchmark]
    fn try_state() {
        seeded_decision::<T>(1);
        #[block]
        {
            Pallet::<T>::do_try_state().expect("benchmark try-state succeeds");
        }
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
