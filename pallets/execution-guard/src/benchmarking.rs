//! `frame-benchmarking` v2 coverage for every execution-guard dispatchable (15 §4.5).
//! B5 replaces the hand-seeded weights after assembled-runtime PoV calibration.

use super::*;
use crate::pallet::{
    ExecutingUpgrade, ExecutingUpgradeContext, Pallet, PendingUpgrade, Queue, QueuedRecoveryImages,
    Ratifications, RecoveryImage,
};
use frame_benchmarking::v2::*;
use frame_system::RawOrigin;

// Both the production runtime and benchmark mock configure this exact maximum.
// The 512-byte floor is the smallest sampled size that can hold the runtime's
// encoded `runtime_version` custom section plus a padding section.
const BENCHMARK_RUNTIME_CODE_BYTES_BOUND: u32 = 4_194_304;

#[benchmarks(where T: Config)]
mod benches {
    use super::*;

    // The runtime fetches PreimageFor by (hash, queued payload_len), and enqueue
    // caps payload_len at the 64 KiB kernel maximum, so measured PoV is bounded.
    // The benchmark's inner calls are System remarks intentionally: execute's
    // declared pre-charge separately reserves the full `prop.max_weight`
    // ceiling, and post-dispatch accounting adds the real/fallback-declared
    // inner-call weight. This benchmark measures only guard checks/bookkeeping.
    #[benchmark(pov_mode = MaxEncodedLen {
        Preimage::PreimageFor: Measured
    })]
    fn execute(c: Linear<2, MAX_CALLS_BOUND>) {
        let pid = 1;
        T::BenchmarkHelper::prime_execute(pid, c);
        let caller: T::AccountId = whitelisted_caller();
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pid);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert!(!Queue::<T>::contains_key(pid));
        // Execute's benchmark fixture is deliberately CODE: authorization is
        // the maximal guard path and must remain covered even though the
        // retired full-storage-root checkpoint no longer exists.
        assert!(PendingUpgrade::<T>::get().is_some());
    }

    #[benchmark]
    fn apply_authorized_upgrade(
        b: Linear<512, BENCHMARK_RUNTIME_CODE_BYTES_BOUND>,
    ) -> Result<(), BenchmarkError> {
        let bytes = T::BenchmarkHelper::prime_pending_upgrade(b);
        debug_assert_eq!(bytes.len(), b as usize);
        let code = RuntimeCode::<T>::try_from(bytes)
            .map_err(|_| BenchmarkError::Stop("benchmark runtime exceeded payload bound"))?;
        let caller: T::AccountId = whitelisted_caller();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), code);

        // The extrinsic schedules the candidate with parachain-system. The
        // guard record is cleared only at relay `GoAhead`, through the
        // assembled runtime's `OnSystemEvent` callback.
        assert!(PendingUpgrade::<T>::get().is_some());
        Ok(())
    }

    #[benchmark]
    fn expire_failed_execution() {
        let pid = 1;
        T::BenchmarkHelper::prime_failed(pid);
        let caller: T::AccountId = whitelisted_caller();
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pid);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert!(!Queue::<T>::contains_key(pid));
    }

    #[benchmark]
    fn ratify() {
        let pid = 1;
        let referendum_index = 9;
        T::BenchmarkHelper::prime_ratify(pid, referendum_index);
        let origin = T::BenchmarkHelper::ratify_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, pid, referendum_index);

        assert_eq!(
            Ratifications::<T>::get(pid).map(|record| record.referendum_index),
            Some(referendum_index)
        );
    }

    #[benchmark]
    fn reject_stale() {
        let pid = 1;
        T::BenchmarkHelper::prime_stale(pid);
        let caller: T::AccountId = whitelisted_caller();
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pid);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert!(!Queue::<T>::contains_key(pid));
    }

    #[benchmark]
    fn commit_recovery_image() {
        let pid = 1;
        let descriptor = T::BenchmarkHelper::prime_recovery_commit(pid);
        ExecutingUpgrade::<T>::put(ExecutingUpgradeContext {
            pid,
            primary_hash: [0x51; 32],
            primary_target_spec_version: descriptor.target_spec_version.saturating_sub(1),
        });
        QueuedRecoveryImages::<T>::insert(pid, descriptor);
        let origin = T::BenchmarkHelper::recovery_commit_origin();

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            descriptor.hash,
            descriptor.len,
            descriptor.target_spec_version,
            descriptor.attestation_id,
        );

        assert!(RecoveryImage::<T>::get().is_some_and(|image| image.hash == descriptor.hash));
    }

    #[benchmark]
    fn authorize_phase_four() {
        let pid = 1;
        T::BenchmarkHelper::prime_phase_four(pid);
        let origin = T::BenchmarkHelper::phase_four_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, pid, [0x4f; 32]);

        assert!(matches!(
            crate::pallet::PhaseFourBridge::<T>::get(),
            crate::pallet::PhaseFourBridgeState::Pending { .. }
        ));
    }

    #[benchmark(pov_mode = Measured)]
    fn qualify_recovery_image(b: Linear<512, BENCHMARK_RUNTIME_CODE_BYTES_BOUND>) {
        let pid = 1;
        T::BenchmarkHelper::prime_recovery_qualification(pid, b);
        let caller: T::AccountId = whitelisted_caller();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pid);

        assert!(crate::pallet::QualifiedRecoveryImages::<T>::contains_key(
            pid
        ));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
