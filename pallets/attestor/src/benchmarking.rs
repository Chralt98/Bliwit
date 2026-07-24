//! `frame-benchmarking` v2 benchmarks for every attestor extrinsic (Track-A
//! DoD, 15 §4.5). B5 recalibrates the placeholder weights from this harness.

use super::*;
// `Vec` is not in the no_std prelude — the runtime's wasm `runtime-benchmarks`
// build compiles this file `no_std`, unlike the std-only pallet gate (B1a).
use crate::pallet::{Attestations, Members, NextAttestationId};
use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_support::BoundedVec;

fn member<T: Config>(i: u8) -> T::AccountId {
    T::AccountId::from([i + 1; 32])
}

fn seed_members<T: Config>() {
    let members = (0..MAX_ATTESTORS)
        .map(|i| AttestorInfo {
            account: [i as u8 + 1; 32],
            bond: ATTESTOR_BOND,
            false_count: 0,
            active: true,
        })
        .collect::<Vec<_>>();
    Members::<T>::put(BoundedVec::truncate_from(members));
}

/// Fill the flat ledger, returning the final id. The measured call therefore
/// scans a worst-case bounded vector.
fn seed_attestations<T: Config>(count: u32, open_last: bool) -> AttestationId {
    seed_members::<T>();
    let mut attestations = Vec::new();
    for id in 0..count {
        let challenge = if open_last && id + 1 == count {
            Some(ChallengeStatus::Open {
                challenger: [250; 32],
                evidence_hash: [9; 32],
                bond: CHALLENGE_BOND,
            })
        } else {
            None
        };
        attestations.push(Attestation {
            id,
            pid: id as futarchy_primitives::ProposalId,
            artifact_hash: [id as u8; 32],
            statement_hash: [7; 32],
            attestor: [1; 32],
            submitted_at: 0,
            challenge_deadline: CHALLENGE_WINDOW_BLOCKS,
            challenge,
        });
    }
    Attestations::<T>::put(BoundedVec::truncate_from(attestations));
    NextAttestationId::<T>::put(count);
    count.saturating_sub(1)
}

#[benchmarks(where T: Config)]
mod benches {
    use super::*;

    #[benchmark]
    fn set_members() {
        T::BenchmarkHelper::prime_funds();
        // Worst case for the SQ-262 unsettled-liability scan. Since `load`
        // (`pallets/attestor/src/lib.rs`) rebuilds the core registry from
        // `Members` *and* `Attestations`, and the core `set_members` runs one
        // `has_unsettled_liability` scan per **departing** member over the whole
        // flat ledger, the measured worst case is O(MAX_ATTESTORS ×
        // MAX_ATTESTATIONS) = 16 × 256. The empty-registry fixture this replaced
        // never read `Attestations` at all (load short-circuits on empty
        // `Members`), so the generated weight undercharged the scan and the full
        // ledger read/write (Finding 1; R-7).
        //
        // Construction, all three properties needed simultaneously:
        //  * a FULL previous roster (16), so every departing member is scanned;
        //  * a FULL ledger (256), so each scan is as long as possible;
        //  * a NEW roster DISJOINT from the previous one, so no member is
        //    re-seated and skipped — all 16 are rescanned.
        // The sole unsettled record is placed LAST and owned by a departing
        // member: `has_unsettled_liability` therefore traverses the entire 256
        // for that member before returning (no early `any()` exit), and the
        // member is moved to an independent liability row. The other 255 records
        // are owned by a non-member sentinel, so every member's scan runs to the
        // end. Attributing the unsettled record early, or spreading it across
        // members, would let `any()` short-circuit and measure a *smaller* scan
        // — see the `worst_case_liability_scan_stays_within_bound` core test.
        seed_members::<T>(); // previous roster: [1; 32]..=[16; 32]

        const SENTINEL: futarchy_primitives::AccountId = [180; 32];
        let mut attestations = Vec::new();
        for id in 0..MAX_ATTESTATIONS - 1 {
            attestations.push(Attestation {
                id,
                pid: id as futarchy_primitives::ProposalId,
                artifact_hash: [id as u8; 32],
                statement_hash: [7; 32],
                attestor: SENTINEL,
                submitted_at: 0,
                challenge_deadline: CHALLENGE_WINDOW_BLOCKS,
                challenge: None,
            });
        }
        // The one unsettled-liability record: owned by departing member [1; 32],
        // challenge `Open` (unsettled regardless of the benchmark block), last.
        attestations.push(Attestation {
            id: MAX_ATTESTATIONS - 1,
            pid: (MAX_ATTESTATIONS - 1) as futarchy_primitives::ProposalId,
            artifact_hash: [255; 32],
            statement_hash: [7; 32],
            attestor: [1; 32],
            submitted_at: 0,
            challenge_deadline: CHALLENGE_WINDOW_BLOCKS,
            challenge: Some(ChallengeStatus::Open {
                challenger: [250; 32],
                evidence_hash: [9; 32],
                bond: CHALLENGE_BOND,
            }),
        });
        Attestations::<T>::put(BoundedVec::truncate_from(attestations));
        NextAttestationId::<T>::put(MAX_ATTESTATIONS);

        // 16 fresh members disjoint from the previous roster and the sentinel;
        // the independent liability remains outside the replacement roster.
        let members = (0..MAX_ATTESTORS)
            .map(|i| member::<T>((i + 17) as u8))
            .collect::<Vec<_>>();

        #[extrinsic_call]
        _(T::BenchmarkHelper::values() as T::RuntimeOrigin, members);

        // 15 new + 1 liable departing member = full bound.
        assert_eq!(Members::<T>::get().len(), MAX_ATTESTORS as usize);
    }

    #[benchmark]
    fn attest() {
        seed_attestations::<T>(MAX_ATTESTATIONS - 1, false);

        #[extrinsic_call]
        _(
            T::BenchmarkHelper::signed([1; 32]),
            MAX_ATTESTATIONS as futarchy_primitives::ProposalId,
            [250; 32],
            [251; 32],
        );

        assert_eq!(Attestations::<T>::get().len(), MAX_ATTESTATIONS as usize);
    }

    #[benchmark]
    fn challenge_attestation() {
        T::BenchmarkHelper::prime_funds();
        let id = seed_attestations::<T>(MAX_ATTESTATIONS, false);

        #[extrinsic_call]
        _(
            T::BenchmarkHelper::signed([250; 32]),
            id,
            [9; 32],
            CHALLENGE_BOND,
        );

        assert!(matches!(
            Attestations::<T>::get()[id as usize].challenge,
            Some(ChallengeStatus::Open { .. })
        ));
    }

    #[benchmark]
    fn resolve_challenge() {
        T::BenchmarkHelper::prime_funds();
        let id = seed_attestations::<T>(MAX_ATTESTATIONS, true);

        #[extrinsic_call]
        _(T::BenchmarkHelper::ratify() as T::RuntimeOrigin, id, false);

        assert!(matches!(
            Attestations::<T>::get()[id as usize].challenge,
            Some(ChallengeStatus::Rejected)
        ));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext_empty(), crate::mock::Test);
}
