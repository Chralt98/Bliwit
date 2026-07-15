//! `frame-benchmarking` v2 benchmarks for every extrinsic (Track-A DoD, 15 §4.5).
//!
//! `pallet-oracle`'s only weight-bearing hook is the try-runtime `try_state`
//! (not benchmarked; it is try-runtime-only), so the ten calls below are the
//! complete benchmark surface. B5 turns the generated output into the
//! PoV-calibrated `weights.rs`. The `QueryResponse` handler and
//! `request_adjudication` are runtime-internal (not extrinsics) and are covered
//! by unit tests.

use super::*;
use crate::pallet::{Pallet, Reporters, Rounds, Watchtowers};

use frame_benchmarking::v2::*;
use frame_support::pallet_prelude::*;
use frame_system::RawOrigin;
use futarchy_primitives::{FixedU64, H256};
use oracle_core::{hash_evidence, hash_report, ORC_ROUNDS, ORC_WINDOW_BLOCKS, RES_PROBE_INTERVAL};

const COMPONENT: MetricId = 1;
const EPOCH: EpochId = 1;
const SPEC: MetricSpecVersion = 3;

fn account<T: Config>(seed: u8) -> T::AccountId {
    [seed; 32].into()
}

fn seed_reporter<T: Config>(seed: u8) -> T::AccountId {
    let who = account::<T>(seed);
    Pallet::<T>::register_reporter(RawOrigin::Signed(who.clone()).into())
        .expect("register_reporter");
    who
}

fn seed_report<T: Config>(reporter: &T::AccountId, value: FixedU64, evidence: H256) {
    Pallet::<T>::report(
        RawOrigin::Signed(reporter.clone()).into(),
        COMPONENT,
        EPOCH,
        SPEC,
        value,
        evidence,
    )
    .expect("report");
}

/// Drive the game to a terminal (round `R_max`, challenged) state so
/// `adjudicate` is admissible (07 §5.4 / F10).
fn seed_terminal<T: Config>(reporter: &T::AccountId, challenger: &T::AccountId) {
    seed_report::<T>(reporter, FixedU64(500_000_000), [9u8; 32]);
    let challenge = |now: u32| {
        frame_system::Pallet::<T>::set_block_number(now.into());
        Pallet::<T>::challenge(
            RawOrigin::Signed(challenger.clone()).into(),
            COMPONENT,
            EPOCH,
            SPEC,
            FixedU64(440_000_000),
            [10u8; 32],
        )
        .expect("challenge");
    };
    for _ in 1..ORC_ROUNDS {
        let deadline = Rounds::<T>::get((COMPONENT, EPOCH, SPEC))
            .expect("round")
            .challenge_deadline;
        challenge(deadline - 1);
        frame_system::Pallet::<T>::set_block_number(deadline.into());
        Pallet::<T>::crank_round_close(RawOrigin::Signed(challenger.clone()).into(), 1)
            .expect("crank");
    }
    let deadline = Rounds::<T>::get((COMPONENT, EPOCH, SPEC))
        .expect("round")
        .challenge_deadline;
    challenge(deadline - 1);
}

#[benchmarks(where T: Config)]
mod benches {
    use super::*;

    #[benchmark]
    fn register_reporter() {
        let who = account::<T>(1);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));

        assert_eq!(Reporters::<T>::count(), 1);
    }

    #[benchmark]
    fn deregister_reporter() {
        let who = seed_reporter::<T>(1);

        #[extrinsic_call]
        _(RawOrigin::Signed(who));

        assert_eq!(Reporters::<T>::count(), 0);
    }

    #[benchmark]
    fn report() {
        let who = seed_reporter::<T>(1);

        #[extrinsic_call]
        _(
            RawOrigin::Signed(who),
            COMPONENT,
            EPOCH,
            SPEC,
            FixedU64(500_000_000),
            [9u8; 32],
        );

        assert!(Rounds::<T>::contains_key((COMPONENT, EPOCH, SPEC)));
    }

    #[benchmark]
    fn challenge() {
        let reporter = seed_reporter::<T>(1);
        seed_report::<T>(&reporter, FixedU64(500_000_000), [9u8; 32]);
        let challenger = account::<T>(2);

        #[extrinsic_call]
        _(
            RawOrigin::Signed(challenger),
            COMPONENT,
            EPOCH,
            SPEC,
            FixedU64(440_000_000),
            [10u8; 32],
        );

        assert!(Rounds::<T>::get((COMPONENT, EPOCH, SPEC))
            .map(|r| r.challenger.is_some())
            .unwrap_or(false));
    }

    #[benchmark]
    fn recompute_proof() {
        let reporter = seed_reporter::<T>(1);
        let value = FixedU64(500_000_000);
        let mut proof_bytes = alloc::vec![0u8; 24];
        proof_bytes[..8].copy_from_slice(&value.0.to_le_bytes());
        let evidence = hash_evidence(&proof_bytes);
        seed_report::<T>(&reporter, value, evidence);
        Pallet::<T>::note_recomputable(COMPONENT, SPEC).expect("note_recomputable");
        let prover = account::<T>(3);
        let proof: BoundedVec<u8, ConstU32<MAX_PROOF_BYTES_BOUND>> =
            BoundedVec::truncate_from(proof_bytes);

        #[extrinsic_call]
        _(RawOrigin::Signed(prover), COMPONENT, EPOCH, SPEC, proof);

        assert!(Pallet::<T>::settled_component(COMPONENT, EPOCH, SPEC).is_some());
    }

    #[benchmark]
    fn register_watchtower() {
        let who = account::<T>(1);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));

        assert_eq!(Watchtowers::<T>::count(), 1);
    }

    #[benchmark]
    fn ack_observed() {
        let reporter = seed_reporter::<T>(1);
        let value = FixedU64(500_000_000);
        let evidence = [9u8; 32];
        seed_report::<T>(&reporter, value, evidence);
        let watchtower = account::<T>(2);
        Pallet::<T>::register_watchtower(RawOrigin::Signed(watchtower.clone()).into())
            .expect("register_watchtower");
        let report_hash = hash_report(COMPONENT, EPOCH, 1, value, evidence);

        #[extrinsic_call]
        _(
            RawOrigin::Signed(watchtower),
            COMPONENT,
            EPOCH,
            SPEC,
            1,
            report_hash,
        );

        assert_eq!(
            Rounds::<T>::get((COMPONENT, EPOCH, SPEC)).map(|r| r.acks),
            Some(1)
        );
    }

    #[benchmark]
    fn crank_round_close() {
        let reporter = seed_reporter::<T>(1);
        let value = FixedU64(500_000_000);
        let evidence = [9u8; 32];
        seed_report::<T>(&reporter, value, evidence);
        let report_hash = hash_report(COMPONENT, EPOCH, 1, value, evidence);
        for seed in 2..4u8 {
            let wt = account::<T>(seed);
            Pallet::<T>::register_watchtower(RawOrigin::Signed(wt.clone()).into())
                .expect("register_watchtower");
            Pallet::<T>::ack_observed(
                RawOrigin::Signed(wt).into(),
                COMPONENT,
                EPOCH,
                SPEC,
                1,
                report_hash,
            )
            .expect("ack_observed");
        }
        frame_system::Pallet::<T>::set_block_number((ORC_WINDOW_BLOCKS + 2).into());
        let keeper = account::<T>(5);

        #[extrinsic_call]
        _(RawOrigin::Signed(keeper), 20);

        assert!(Pallet::<T>::settled_component(COMPONENT, EPOCH, SPEC).is_some());
    }

    #[benchmark]
    fn crank_reserve_probe() {
        frame_system::Pallet::<T>::set_block_number(RES_PROBE_INTERVAL.into());
        let keeper = account::<T>(1);

        #[extrinsic_call]
        _(RawOrigin::Signed(keeper));

        assert_eq!(ReserveHealth::<T>::get().last_query_id, 1);
    }

    #[benchmark]
    fn adjudicate() {
        let reporter = seed_reporter::<T>(1);
        let challenger = account::<T>(2);
        seed_terminal::<T>(&reporter, &challenger);
        let origin = T::BenchmarkHelper::adjudication_origin();

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            COMPONENT,
            EPOCH,
            SPEC,
            FixedU64(440_000_000),
            false,
        );

        assert!(Pallet::<T>::settled_component(COMPONENT, EPOCH, SPEC).is_some());
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
