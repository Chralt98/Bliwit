//! `frame-benchmarking` v2 benchmarks for every extrinsic (Track-A DoD,
//! 15 §4.5). The treasury has no weight-bearing hooks — 08 gives it no cranks
//! and `try_state` is try-runtime-only — so the call set below is the complete
//! benchmark surface. Each benchmark seeds worst-case bounded state (near-full
//! streams / a funded line) and drives its call with the exact 08 §1.1
//! authority via [`crate::BenchmarkHelper`]. B5 turns the generated output into
//! the PoV-calibrated `weights.rs`.

use super::*;
use crate::pallet::Pallet;

use frame_benchmarking::v2::*;
use frame_system::RawOrigin;
use futarchy_treasury_core::{Stream, Treasury, USDC};

/// A funded treasury: plenty of `MAIN` USDC and pre-funded lines so the outflow
/// calls have both NAV and line balances to draw on.
fn funded() -> Treasury {
    let mut t = Treasury {
        main_usdc: 50_000_000 * USDC,
        ..Treasury::default()
    };
    t.lines.push((BudgetLine::OpsCollators, 2_000_000 * USDC));
    t.lines.push((BudgetLine::Rewards, 1_000_000 * USDC));
    t.lines.push((BudgetLine::OpsCoretime, 1_000_000 * USDC));
    t
}

/// Fill the stream table to `MAX_STREAMS - 1` so a push lands at the last free
/// slot and a scan is worst-case.
fn fill_streams(t: &mut Treasury, recipient: futarchy_primitives::AccountId) {
    for i in 0..(MAX_STREAMS as u64 - 1) {
        t.streams.push(Stream {
            id: i,
            recipient,
            line: BudgetLine::Rewards,
            total: USDC,
            claimed: 0,
            start: 0,
            duration: 100,
            cancelled: false,
        });
    }
    t.next_stream_id = MAX_STREAMS as u64 - 1;
}

#[benchmarks(where T::AccountId: From<[u8; 32]>)]
mod benches {
    use super::*;

    #[benchmark]
    fn fund_budget_line() {
        Pallet::<T>::seed(&funded());
        let origin = T::BenchmarkHelper::treasury_origin();

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            BudgetLine::Keeper,
            100_000 * USDC,
        );

        assert_eq!(
            Pallet::<T>::line_balance(BudgetLine::Keeper),
            100_000 * USDC
        );
    }

    #[benchmark]
    fn spend() {
        Pallet::<T>::seed(&funded());
        let dest: T::AccountId = [7u8; 32].into();
        let origin = T::BenchmarkHelper::treasury_origin();

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            BudgetLine::OpsCollators,
            dest,
            10_000 * USDC,
        );
    }

    #[benchmark]
    fn open_stream() {
        let recipient_bytes = [9u8; 32];
        let mut t = funded();
        fill_streams(&mut t, recipient_bytes);
        Pallet::<T>::seed(&t);
        let recipient: T::AccountId = recipient_bytes.into();
        let origin = T::BenchmarkHelper::treasury_origin();

        // Above the 1% NAV stream threshold (funded NAV ≈ 54M) and within the
        // OpsCollators line and the 5% cap.
        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            BudgetLine::OpsCollators,
            recipient,
            1_000_000 * USDC,
            10u32.into(),
            100u32.into(),
        );
    }

    #[benchmark]
    fn claim_stream() {
        let who_bytes = [3u8; 32];
        let mut t = funded();
        // Worst case: the claimed stream is the last one scanned.
        fill_streams(&mut t, [0u8; 32]);
        let id = t.next_stream_id;
        t.streams.push(Stream {
            id,
            recipient: who_bytes,
            line: BudgetLine::Rewards,
            total: 100_000 * USDC,
            claimed: 0,
            start: 0,
            duration: 100,
            cancelled: false,
        });
        t.next_stream_id = id + 1;
        Pallet::<T>::seed(&t);
        frame_system::Pallet::<T>::set_block_number(60u32.into());
        let who: T::AccountId = who_bytes.into();

        #[extrinsic_call]
        _(RawOrigin::Signed(who), id);
    }

    #[benchmark]
    fn cancel_stream() {
        let mut t = funded();
        let id = t.next_stream_id;
        t.streams.push(Stream {
            id,
            recipient: [4u8; 32],
            line: BudgetLine::Rewards,
            total: 100_000 * USDC,
            claimed: 0,
            start: 0,
            duration: 100,
            cancelled: false,
        });
        t.next_stream_id = id + 1;
        Pallet::<T>::seed(&t);
        let origin = T::BenchmarkHelper::treasury_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, id);
    }

    #[benchmark]
    fn issue_vit() {
        Pallet::<T>::seed(&funded());
        let origin = T::BenchmarkHelper::treasury_origin();

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            1_000_000 * VIT,
            BudgetLine::Rewards,
        );
    }

    #[benchmark]
    fn recover_foreign() {
        Pallet::<T>::seed(&funded());
        let dest: T::AccountId = [5u8; 32].into();
        let origin = T::BenchmarkHelper::treasury_origin();

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            AssetKind::Foreign([1u8; 32]),
            dest,
            1_000 * USDC,
        );
    }

    #[benchmark]
    fn execute_coretime_renewal() {
        let mut t = funded();
        // Worst case: the funded-period ring and quote list are full.
        for p in 0..(MAX_FUNDED_CORETIME_PERIODS as u32 - 1) {
            t.funded_coretime_periods.push(p);
        }
        t.coretime_quotes.push((1000, 100_000 * USDC));
        Pallet::<T>::seed(&t);
        let keeper: T::AccountId = T::BenchmarkHelper::account(1);
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(keeper), 1000);
        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
