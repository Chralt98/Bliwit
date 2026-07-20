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

fn file_many<T: Config<I>, I: 'static>(filer: &T::AccountId, epoch: EpochId, count: u32) {
    T::BenchmarkHelper::prime_epoch(epoch);
    let spec = T::Epoch::frozen_spec_version(epoch).unwrap_or_default();
    for index in 0..count {
        let mut evidence = [7u8; 32];
        evidence[..4].copy_from_slice(&index.to_le_bytes());
        Pallet::<T, I>::file(
            RawOrigin::Signed(filer.clone()).into(),
            epoch,
            worst_class::<T, I>(),
            10,
            evidence,
            spec,
        )
        .expect("file");
    }
}

/// Fill the other live-epoch slots so every measured scoped load sees the
/// 13 §4 `registry.max_live_epochs = 4` bound as well as the target epoch.
fn fill_other_live_epochs<T: Config<I>, I: 'static>(filer: &T::AccountId) {
    for epoch in (EPOCH + 1)..(EPOCH + registry_core::MAX_LIVE_EPOCHS as u32) {
        file_many::<T, I>(filer, epoch, 1);
    }
    // Filing the auxiliary epochs advances the real runtime epoch feed. Restore
    // the measured epoch so a final `file(EPOCH, ..)` remains inside its window.
    T::BenchmarkHelper::prime_epoch(EPOCH);
}

fn ack_all<T: Config<I>, I: 'static>(wt1: &T::AccountId, wt2: &T::AccountId) {
    let wt1: <T as frame_system::Config>::AccountId = wt1.clone();
    let wt2: <T as frame_system::Config>::AccountId = wt2.clone();
    for filing_id in 0..T::MaxFilingsPerEpoch::get() {
        Filings::<T, I>::mutate(EPOCH, filing_id, |filing| {
            if let Some(filing) = filing {
                if let FilingState::Filed { acks, .. } = &mut filing.state {
                    *acks = registry_core::WT_QUORUM;
                }
            }
        });
        AckRecords::<T, I>::insert((EPOCH, filing_id, wt1.clone().into()), ());
        AckRecords::<T, I>::insert((EPOCH, filing_id, wt2.clone().into()), ());
    }
}

fn close_all<T: Config<I>, I: 'static>(keeper: &T::AccountId) {
    let batches = T::MaxFilingsPerEpoch::get().saturating_add(REG_CLOSE_BATCH as u32 - 1)
        / REG_CLOSE_BATCH as u32;
    for _ in 0..batches {
        Pallet::<T, I>::crank_close(
            RawOrigin::Signed(keeper.clone()).into(),
            EPOCH,
            REG_CLOSE_BATCH as u32,
        )
        .expect("crank");
    }
}

/// Fill retained aggregate slots that the pallet's scoped loader always
/// hydrates. `count` is three before closing the target epoch (which adds the
/// fourth) and four for operations that do not add an aggregate.
fn fill_aggregates<T: Config<I>, I: 'static>(count: u32) {
    for index in 0..count {
        Aggregates::<T, I>::insert(10_000u32.saturating_add(index), FixedU64(0));
    }
}

#[instance_benchmarks(where BlockNumberFor<T>: From<u32>)]
mod benches {
    use super::*;

    #[benchmark]
    fn file() {
        let caller: T::AccountId = T::BenchmarkHelper::funded_account(1);
        file_many::<T, I>(&caller, EPOCH, T::MaxFilingsPerEpoch::get() - 1);
        fill_other_live_epochs::<T, I>(&caller);
        fill_aggregates::<T, I>(registry_core::MAX_AGGREGATES as u32);
        let spec = T::Epoch::frozen_spec_version(EPOCH).unwrap_or_default();
        let class = worst_class::<T, I>();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), EPOCH, class, 10, [7u8; 32], spec);

        assert!(Filings::<T, I>::get(EPOCH, T::MaxFilingsPerEpoch::get() - 1).is_some());
    }

    #[benchmark]
    fn challenge_filing() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let challenger: T::AccountId = T::BenchmarkHelper::funded_account(2);
        file_many::<T, I>(&filer, EPOCH, T::MaxFilingsPerEpoch::get());
        fill_other_live_epochs::<T, I>(&filer);
        fill_aggregates::<T, I>(registry_core::MAX_AGGREGATES as u32);
        let filing_id = T::MaxFilingsPerEpoch::get() - 1;

        #[extrinsic_call]
        _(RawOrigin::Signed(challenger), EPOCH, filing_id, [8u8; 32]);

        assert!(matches!(
            Filings::<T, I>::get(EPOCH, filing_id).unwrap().state,
            FilingState::Challenged { .. }
        ));
    }

    #[benchmark]
    fn ack_observed() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let wt: T::AccountId = T::BenchmarkHelper::funded_account(10);
        T::BenchmarkHelper::register_watchtower(&wt);
        file_many::<T, I>(&filer, EPOCH, T::MaxFilingsPerEpoch::get());
        fill_other_live_epochs::<T, I>(&filer);
        fill_aggregates::<T, I>(registry_core::MAX_AGGREGATES as u32);
        let filing_id = T::MaxFilingsPerEpoch::get() - 1;
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(wt.clone()), EPOCH, filing_id);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::OracleLine,
        );
        assert!(AckRecords::<T, I>::contains_key((
            EPOCH,
            filing_id,
            wt.into()
        )));
    }

    #[benchmark]
    fn crank_close() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let wt1: T::AccountId = T::BenchmarkHelper::funded_account(10);
        let wt2: T::AccountId = T::BenchmarkHelper::funded_account(11);
        T::BenchmarkHelper::register_watchtower(&wt1);
        T::BenchmarkHelper::register_watchtower(&wt2);
        file_many::<T, I>(&filer, EPOCH, T::MaxFilingsPerEpoch::get());
        fill_other_live_epochs::<T, I>(&filer);
        fill_aggregates::<T, I>(registry_core::MAX_AGGREGATES as u32);
        ack_all::<T, I>(&wt1, &wt2);
        frame_system::Pallet::<T>::set_block_number(PAST.into());
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(filer), EPOCH, REG_CLOSE_BATCH as u32);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::OracleLine,
        );
        assert!(matches!(
            Filings::<T, I>::get(EPOCH, 0).unwrap().state,
            FilingState::Upheld
        ));
    }

    #[benchmark]
    fn resolve_challenge() {
        let filer: T::AccountId = T::BenchmarkHelper::funded_account(1);
        let challenger: T::AccountId = T::BenchmarkHelper::funded_account(2);
        file_many::<T, I>(&filer, EPOCH, T::MaxFilingsPerEpoch::get());
        fill_other_live_epochs::<T, I>(&filer);
        fill_aggregates::<T, I>(registry_core::MAX_AGGREGATES as u32);
        let filing_id = T::MaxFilingsPerEpoch::get() - 1;
        Pallet::<T, I>::challenge_filing(
            RawOrigin::Signed(challenger).into(),
            EPOCH,
            filing_id,
            [8u8; 32],
        )
        .expect("challenge");

        #[extrinsic_call]
        _(
            T::BenchmarkHelper::resolution_origin() as T::RuntimeOrigin,
            EPOCH,
            filing_id,
            false,
        );

        assert!(matches!(
            Filings::<T, I>::get(EPOCH, filing_id).unwrap().state,
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
        file_many::<T, I>(&filer, EPOCH, T::MaxFilingsPerEpoch::get());
        fill_other_live_epochs::<T, I>(&filer);
        fill_aggregates::<T, I>((registry_core::MAX_AGGREGATES - 1) as u32);
        ack_all::<T, I>(&wt1, &wt2);
        frame_system::Pallet::<T>::set_block_number(PAST.into());
        close_all::<T, I>(&filer);

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
        file_many::<T, I>(&filer, EPOCH, T::MaxFilingsPerEpoch::get());
        fill_other_live_epochs::<T, I>(&filer);
        fill_aggregates::<T, I>((registry_core::MAX_AGGREGATES - 1) as u32);
        ack_all::<T, I>(&wt1, &wt2);
        frame_system::Pallet::<T>::set_block_number(PAST.into());
        close_all::<T, I>(&filer);
        Pallet::<T, I>::close_epoch(RawOrigin::Signed(filer.clone()).into(), EPOCH).expect("close");
        // Advance past the archive delay so the epoch is reap-eligible.
        let due = frame_system::Pallet::<T>::block_number()
            .saturating_add(T::ArchiveDelay::get())
            .saturating_add(1u32.into());
        frame_system::Pallet::<T>::set_block_number(due);
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(filer), EPOCH);

        // Archival cleanup is rebated from the metered general tranche, not the
        // oracle budget line (07 §7 *Crank funding lines*; 08 §6.3 — SQ-297).
        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert_eq!(Filings::<T, I>::iter_prefix(EPOCH).count(), 0);
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
