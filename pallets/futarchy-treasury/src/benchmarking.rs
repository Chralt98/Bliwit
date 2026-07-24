//! `frame-benchmarking` v2 benchmarks for every extrinsic (Track-A DoD,
//! 15 §4.5) plus the one weight-bearing non-extrinsic entry point: the
//! `note_collator_block` authorship callback, which registers its own
//! Mandatory weight because `pallet_authorship` reserves nothing for its
//! `EventHandler`. 08 gives the treasury no cranks and `try_state` is
//! try-runtime-only, so this set is the complete benchmark surface. Each benchmark seeds worst-case bounded state (near-full
//! streams / a funded line) and drives its call with the exact 08 §1.1
//! authority via [`crate::BenchmarkHelper`]. B5 turns the generated output into
//! the PoV-calibrated `weights.rs`.

use super::*;
use crate::pallet::Pallet;

use frame_benchmarking::v2::*;
use frame_support::traits::Get;
use frame_support::BoundedVec;
use frame_system::RawOrigin;
use futarchy_treasury_core::{CoretimeQuote, Stream, Treasury, USDC};

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
        let amount = 100_000 * USDC;
        let custody_seeded = T::BenchmarkHelper::prime_pot_funding(amount);
        assert!(custody_seeded.is_ok());
        // 03 §7 R-4: on the real runtime every statically derived custody
        // account is genesis-endowed with `min_balance` as a permanent,
        // unspendable floor, so the payout pot does not start at zero (the
        // mock has no such genesis). Assert the funded delta, not the absolute
        // balance, so the benchmark holds under both.
        let pot_before = T::RebatePayout::pot_balance(PayoutLine::Keeper);

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, BudgetLine::Keeper, amount);

        assert_eq!(Pallet::<T>::line_balance(BudgetLine::Keeper), amount);
        assert_eq!(
            T::RebatePayout::pot_balance(PayoutLine::Keeper).saturating_sub(pot_before),
            amount
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
        t.coretime_quotes.push(CoretimeQuote {
            period_index: 1000,
            price: 100_000 * USDC,
            noted_at: 0,
        });
        Pallet::<T>::seed(&t);
        let keeper: T::AccountId = T::BenchmarkHelper::account(1);
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(keeper), 1000);
        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
    }

    #[benchmark]
    fn note_coretime_quote() {
        let mut t = funded();
        for period_index in 0..(MAX_FUNDED_CORETIME_PERIODS as u32 - 1) {
            t.coretime_quotes.push(CoretimeQuote {
                period_index,
                price: USDC,
                noted_at: 0,
            });
        }
        Pallet::<T>::seed(&t);
        let authority = T::BenchmarkHelper::account(2);
        let treasury_origin = T::BenchmarkHelper::treasury_origin();
        assert!(
            Pallet::<T>::set_coretime_authority(treasury_origin, authority.clone(), [2u8; 32],)
                .is_ok()
        );

        #[extrinsic_call]
        _(RawOrigin::Signed(authority), 1000, USDC);
    }

    #[benchmark]
    fn prune_coretime_quote() {
        let mut t = funded();
        for period_index in 0..MAX_FUNDED_CORETIME_PERIODS as u32 {
            t.coretime_quotes.push(CoretimeQuote {
                period_index,
                price: USDC,
                noted_at: 0,
            });
        }
        Pallet::<T>::seed(&t);
        frame_system::Pallet::<T>::set_block_number(u32::MAX.into());
        let keeper = T::BenchmarkHelper::account(3);
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(
            RawOrigin::Signed(keeper),
            MAX_FUNDED_CORETIME_PERIODS as u32 - 1,
        );
        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
    }

    #[benchmark]
    fn set_coretime_authority() {
        let origin = T::BenchmarkHelper::treasury_origin();
        let authority = T::BenchmarkHelper::account(4);

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, authority, [4u8; 32]);
    }

    /// 08 §1.2/§1.4 (SQ-207): the INSURANCE → `MAIN` sweep. Worst case is a
    /// non-zero amount, which exercises the custody seam as well as the `State`
    /// round-trip.
    #[benchmark]
    fn sweep_insurance() {
        // Worst case is a non-zero sweep off the `funded()` fixture: it exercises
        // the custody seam *and* the full `State` round-trip. INSURANCE must be
        // primed first — under 03 §7 R-4 it holds only `min_balance`, so
        // `Preservation::Preserve` would otherwise refuse and the benchmark
        // could not execute in the assembled runtime.
        Pallet::<T>::seed(&funded());
        let origin = T::BenchmarkHelper::treasury_origin();
        let amount = 100_000 * USDC;
        let custody_seeded = T::BenchmarkHelper::prime_insurance_custody(amount * 2);
        assert!(custody_seeded.is_ok());
        let main_before = Pallet::<T>::treasury().main_usdc;

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, amount);

        assert_eq!(
            Pallet::<T>::treasury().main_usdc,
            main_before.saturating_add(amount)
        );
    }

    #[benchmark]
    fn create_community_schedule() {
        let beneficiary: T::AccountId = T::BenchmarkHelper::account(9);
        let amount = 1_000_000 * VIT;
        Pallet::<T>::note_phase_four_arming();
        CommunityDistributionRemaining::<T>::put(T::CommunityDistributionAmount::get());

        #[extrinsic_call]
        _(T::BenchmarkHelper::community_origin(), beneficiary, amount);
    }

    #[benchmark]
    fn note_collator_block() {
        // Worst case: the accumulator holds a full prior-epoch share table,
        // the boundary is crossed (tracked epoch != current), and nothing is
        // pending — so the callback moves the whole bounded vector aside,
        // snapshots the pending epoch/count, starts a fresh accumulator, and
        // records the new author.
        let bound = T::MaxCollatorCompensationEntries::get();
        let mut shares: BoundedVec<(T::AccountId, u32), T::MaxCollatorCompensationEntries> =
            BoundedVec::new();
        for i in 0..bound {
            let filler: T::AccountId = T::BenchmarkHelper::account(i as u8);
            assert!(shares.try_push((filler, 1)).is_ok());
        }
        CollatorAuthoredBlocks::<T>::put(shares);
        let current =
            T::CollatorEpoch::epoch_at(sp_runtime::SaturatedConversion::saturated_into::<
                futarchy_primitives::BlockNumber,
            >(frame_system::Pallet::<T>::block_number()));
        CollatorAuthoredEpoch::<T>::put(current.wrapping_add(1));
        CollatorAuthoredRegisteredCount::<T>::put(bound);
        let author: T::AccountId = T::BenchmarkHelper::account(200);

        #[block]
        {
            Pallet::<T>::note_collator_block(author.clone());
        }

        assert!(CollatorPendingEpoch::<T>::get().is_some());
        assert!(CollatorAuthoredBlocks::<T>::get()
            .iter()
            .any(|(who, blocks)| *who == author && *blocks == 1));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
