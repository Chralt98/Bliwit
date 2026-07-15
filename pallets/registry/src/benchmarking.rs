//! `frame-benchmarking` v2 instance-benchmarks for every registry extrinsic
//! (Track-A DoD, 15 §4.5). B5 turns the generated output into the PoV-calibrated
//! `weights.rs`; the harness + worst-case setup are the milestone.

use super::*;
use crate::pallet::{AckRecords, Aggregates, Filings};
use frame_benchmarking::v2::*;
use frame_support::traits::Get;
use frame_system::pallet_prelude::BlockNumberFor;
use frame_system::RawOrigin;
use registry_core::{FilingClass, FilingState, RegistryKind, REG_CLOSE_BATCH};
use sp_runtime::traits::Saturating;

const EPOCH: EpochId = 1;
/// A block past every default filing challenge window (72 h) and the mock filing
/// window (1,000,000), so a keeper crank/close/reap sees due filings.
const PAST: u32 = 1_100_000;

/// The worst-case (valid) filing class for this instance's kind.
fn worst_class<T: Config<I>, I: 'static>() -> FilingClass {
    match T::Kind::get() {
        RegistryKind::Incident => FilingClass::S1,
        RegistryKind::Milestone => FilingClass::Scope(1),
    }
}

/// File one live filing for `EPOCH` from `filer`.
fn file_one<T: Config<I>, I: 'static>(filer: &T::AccountId) {
    let spec = T::Epoch::frozen_spec_version(EPOCH);
    Pallet::<T, I>::file(
        RawOrigin::Signed(filer.clone()).into(),
        EPOCH,
        worst_class::<T, I>(),
        10,
        [7u8; 32],
        spec,
    )
    .expect("file");
}

#[instance_benchmarks(where BlockNumberFor<T>: From<u32>)]
mod benches {
    use super::*;

    #[benchmark]
    fn file() {
        let caller: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let spec = T::Epoch::frozen_spec_version(EPOCH);
        let class = worst_class::<T, I>();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), EPOCH, class, 10, [7u8; 32], spec);

        assert!(Filings::<T, I>::get(EPOCH, 0).is_some());
    }

    #[benchmark]
    fn challenge_filing() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let challenger: T::AccountId = T::BenchmarkHelper::funded_account(2);
        file_one::<T, I>(&filer);

        #[extrinsic_call]
        _(RawOrigin::Signed(challenger), EPOCH, 0, [8u8; 32]);

        assert!(matches!(
            Filings::<T, I>::get(EPOCH, 0).unwrap().state,
            FilingState::Challenged { .. }
        ));
    }

    #[benchmark]
    fn ack_observed() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let wt: T::AccountId = T::BenchmarkHelper::funded_account(10);
        T::BenchmarkHelper::register_watchtower(&wt);
        file_one::<T, I>(&filer);

        #[extrinsic_call]
        _(RawOrigin::Signed(wt.clone()), EPOCH, 0);

        assert!(AckRecords::<T, I>::contains_key((EPOCH, 0, wt.into())));
    }

    #[benchmark]
    fn crank_close() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let wt1: T::AccountId = T::BenchmarkHelper::funded_account(10);
        let wt2: T::AccountId = T::BenchmarkHelper::funded_account(11);
        T::BenchmarkHelper::register_watchtower(&wt1);
        T::BenchmarkHelper::register_watchtower(&wt2);
        file_one::<T, I>(&filer);
        Pallet::<T, I>::ack_observed(RawOrigin::Signed(wt1).into(), EPOCH, 0).expect("ack1");
        Pallet::<T, I>::ack_observed(RawOrigin::Signed(wt2).into(), EPOCH, 0).expect("ack2");
        frame_system::Pallet::<T>::set_block_number(PAST.into());

        #[extrinsic_call]
        _(RawOrigin::Signed(filer), EPOCH, REG_CLOSE_BATCH as u32);

        assert!(matches!(
            Filings::<T, I>::get(EPOCH, 0).unwrap().state,
            FilingState::Upheld
        ));
    }

    #[benchmark]
    fn resolve_challenge() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let challenger: T::AccountId = T::BenchmarkHelper::funded_account(2);
        file_one::<T, I>(&filer);
        Pallet::<T, I>::challenge_filing(RawOrigin::Signed(challenger).into(), EPOCH, 0, [8u8; 32])
            .expect("challenge");

        #[extrinsic_call]
        _(
            T::BenchmarkHelper::resolution_origin() as T::RuntimeOrigin,
            EPOCH,
            0,
            false,
        );

        assert!(matches!(
            Filings::<T, I>::get(EPOCH, 0).unwrap().state,
            FilingState::Rejected
        ));
    }

    #[benchmark]
    fn close_epoch() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let wt1: T::AccountId = T::BenchmarkHelper::funded_account(10);
        let wt2: T::AccountId = T::BenchmarkHelper::funded_account(11);
        T::BenchmarkHelper::register_watchtower(&wt1);
        T::BenchmarkHelper::register_watchtower(&wt2);
        file_one::<T, I>(&filer);
        Pallet::<T, I>::ack_observed(RawOrigin::Signed(wt1).into(), EPOCH, 0).expect("ack1");
        Pallet::<T, I>::ack_observed(RawOrigin::Signed(wt2).into(), EPOCH, 0).expect("ack2");
        frame_system::Pallet::<T>::set_block_number(PAST.into());
        Pallet::<T, I>::crank_close(
            RawOrigin::Signed(filer.clone()).into(),
            EPOCH,
            REG_CLOSE_BATCH as u32,
        )
        .expect("crank");

        #[extrinsic_call]
        _(RawOrigin::Signed(filer), EPOCH);

        assert!(Aggregates::<T, I>::get(EPOCH).is_some());
    }

    #[benchmark]
    fn reap_epoch() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let wt1: T::AccountId = T::BenchmarkHelper::funded_account(10);
        let wt2: T::AccountId = T::BenchmarkHelper::funded_account(11);
        T::BenchmarkHelper::register_watchtower(&wt1);
        T::BenchmarkHelper::register_watchtower(&wt2);
        file_one::<T, I>(&filer);
        Pallet::<T, I>::ack_observed(RawOrigin::Signed(wt1).into(), EPOCH, 0).expect("ack1");
        Pallet::<T, I>::ack_observed(RawOrigin::Signed(wt2).into(), EPOCH, 0).expect("ack2");
        frame_system::Pallet::<T>::set_block_number(PAST.into());
        Pallet::<T, I>::crank_close(
            RawOrigin::Signed(filer.clone()).into(),
            EPOCH,
            REG_CLOSE_BATCH as u32,
        )
        .expect("crank");
        Pallet::<T, I>::close_epoch(RawOrigin::Signed(filer.clone()).into(), EPOCH).expect("close");
        // Advance past the archive delay so the epoch is reap-eligible.
        let due = frame_system::Pallet::<T>::block_number()
            .saturating_add(T::ArchiveDelay::get())
            .saturating_add(1u32.into());
        frame_system::Pallet::<T>::set_block_number(due);

        #[extrinsic_call]
        _(RawOrigin::Signed(filer), EPOCH);

        assert!(Filings::<T, I>::get(EPOCH, 0).is_none());
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
