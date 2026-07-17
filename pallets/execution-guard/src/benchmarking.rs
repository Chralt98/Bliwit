//! `frame-benchmarking` v2 coverage for every execution-guard dispatchable (15 §4.5).
//! B5 replaces the hand-seeded weights after assembled-runtime PoV calibration.

use super::*;
use crate::pallet::{Pallet, PendingUpgrade, Queue, Ratifications};
use frame_benchmarking::v2::*;
use frame_system::RawOrigin;

// Both the production runtime and benchmark mock configure this exact maximum.
// The 512-byte floor is the smallest sampled size that can hold the runtime's
// encoded `runtime_version` custom section plus a padding section.
const BENCHMARK_RUNTIME_CODE_BYTES_BOUND: u32 = 2_097_152;

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
    fn execute(c: Linear<1, MAX_CALLS_BOUND>) {
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

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
