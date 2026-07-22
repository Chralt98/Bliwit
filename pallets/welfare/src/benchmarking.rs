//! `frame-benchmarking` v2 coverage for every welfare extrinsic (15 §4.5).
//! B5 replaces the hand-seeded weights after PoV calibration.

use super::*;
// `Vec`/`vec!` are not in the no_std prelude — the runtime's wasm
// `runtime-benchmarks` build compiles this file `no_std`, unlike the std-only
// pallet gate (B1a).
use crate::pallet::{BoundedSpecSet, Pallet};
use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_system::RawOrigin;

fn metric_spec(id: u16, pillar: Pillar, weight: u64, version: u16) -> MetricSpec {
    let source = match pillar {
        Pillar::CAttested | Pillar::A => SourceClass::Attested,
        Pillar::S | Pillar::COnchain | Pillar::P => SourceClass::Onchain,
    };
    MetricSpec {
        id,
        version,
        pillar,
        weight: FixedU64(weight),
        epsilon_floor: EPSILON_PILLAR,
        activation_epoch: 2,
        source,
        formula_ref: [1; 32],
        units: [2; 16],
        repr: [3; 16],
        cadence_blocks: 1,
        sanity_min: FixedU64(0),
        sanity_max: FixedU64(ONE),
        has_normalization_rule: true,
        has_missing_data_rule: true,
        has_gaming_vectors: true,
        has_challenge_procedure: true,
        prior_bounds: [FixedU64(ONE); HISTORY_PRIORS],
    }
}

pub fn full_specs(version: u16) -> Vec<MetricSpec> {
    let mut specs = (1..=12)
        .map(|id| metric_spec(id, Pillar::S, 0, version))
        .collect::<Vec<_>>();
    specs.push(metric_spec(13, Pillar::COnchain, ONE / 2, version));
    // Keep both C sources live so snapshot/gate benchmarks exercise the
    // attested-C incident-multiplier path as well as the on-chain component.
    specs.push(metric_spec(14, Pillar::CAttested, ONE / 2, version));
    specs.push(metric_spec(15, Pillar::P, ONE, version));
    specs.push(metric_spec(16, Pillar::A, ONE, version));
    specs
}

pub fn healthy(count: u16) -> Vec<ComponentValue> {
    (1..=count)
        .map(|id| ComponentValue {
            id,
            value: FixedU64(ONE),
        })
        .collect()
}

fn fill_snapshots(state: &mut WelfareState, count: usize) -> Result<(), BenchmarkError> {
    for epoch in 2..(count as u32 + 2) {
        state
            .record_snapshot(
                epoch,
                1,
                healthy(MAX_COMPONENTS_PER_SPEC as u16),
                FixedU64(ONE),
                &CoreWelfareParams::DEFAULT,
            )
            .map_err(|_| BenchmarkError::Stop("benchmark snapshot setup failed"))?;
    }
    Ok(())
}

fn fill_gate_flags(state: &mut WelfareState, count: usize) -> Result<(), BenchmarkError> {
    for epoch in 2..(count as u32 + 2) {
        state
            .record_daily_gate(
                epoch,
                0,
                1,
                healthy(MAX_COMPONENTS_PER_SPEC as u16),
                &CoreWelfareParams::DEFAULT,
            )
            .map_err(|_| BenchmarkError::Stop("benchmark gate setup failed"))?;
    }
    Ok(())
}

fn fill_specs(state: &mut WelfareState, first_version: u16) -> Result<(), BenchmarkError> {
    for version in first_version..=MAX_METRIC_SPECS as u16 {
        state
            .register_metric_spec(Registration::Genesis, version, full_specs(version))
            .map_err(|_| BenchmarkError::Stop("benchmark spec setup failed"))?;
    }
    Ok(())
}

#[benchmarks]
mod benches {
    use super::*;

    #[benchmark]
    fn register_spec() -> Result<(), BenchmarkError> {
        let mut state = WelfareState::new();
        for version in 1..MAX_METRIC_SPECS as u16 {
            state
                .register_metric_spec(Registration::Genesis, version, full_specs(version))
                .map_err(|_| BenchmarkError::Stop("benchmark setup failed"))?;
        }
        fill_snapshots(&mut state, MAX_SNAPSHOTS)?;
        fill_gate_flags(&mut state, MAX_GATE_FLAGS)?;
        Pallet::<T>::seed(&state)?;
        let version = MAX_METRIC_SPECS as u16;
        // The extrinsic registers at the live clock, so its specs must clear the
        // two-epoch activation lead (05 §4.6) — unlike the epoch-0 seed above.
        let activation =
            <T::CurrentEpoch as frame_support::traits::Get<EpochId>>::get().saturating_add(2);
        let specs_vec = full_specs(version)
            .into_iter()
            .map(|spec| MetricSpec {
                activation_epoch: activation,
                ..spec
            })
            .collect::<Vec<_>>();
        let specs = BoundedSpecSet::try_from(specs_vec)
            .map_err(|_| BenchmarkError::Stop("benchmark specs exceed bound"))?;
        let origin = T::BenchmarkHelper::metric_governance_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, version, specs);

        assert_eq!(MetricSpecs::<T>::iter().count(), MAX_METRIC_SPECS);
        Ok(())
    }

    #[benchmark]
    fn record_snapshot() -> Result<(), BenchmarkError> {
        let mut state = WelfareState::new();
        state
            .register_metric_spec(Registration::Genesis, 1, full_specs(1))
            .map_err(|_| BenchmarkError::Stop("benchmark setup failed"))?;
        fill_specs(&mut state, 2)?;
        fill_snapshots(&mut state, MAX_SNAPSHOTS - 1)?;
        fill_gate_flags(&mut state, MAX_GATE_FLAGS)?;
        Pallet::<T>::seed(&state)?;
        T::BenchmarkHelper::prime_finalized_epoch(MAX_SNAPSHOTS as u32 + 1);
        T::BenchmarkHelper::prime_metric_inputs(MAX_COMPONENTS_PER_SPEC as u16);
        let caller: T::AccountId = whitelisted_caller();
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), MAX_SNAPSHOTS as u32 + 1, 1);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::DecisionCritical,
        );
        assert_eq!(Snapshots::<T>::iter().count(), MAX_SNAPSHOTS);
        Ok(())
    }

    #[benchmark]
    fn record_daily_gate() -> Result<(), BenchmarkError> {
        let mut state = WelfareState::new();
        state
            .register_metric_spec(Registration::Genesis, 1, full_specs(1))
            .map_err(|_| BenchmarkError::Stop("benchmark setup failed"))?;
        fill_specs(&mut state, 2)?;
        fill_snapshots(&mut state, MAX_SNAPSHOTS)?;
        fill_gate_flags(&mut state, MAX_GATE_FLAGS - 1)?;
        Pallet::<T>::seed(&state)?;
        T::BenchmarkHelper::prime_finalized_epoch(MAX_GATE_FLAGS as u32 + 1);
        T::BenchmarkHelper::prime_metric_inputs(MAX_COMPONENTS_PER_SPEC as u16);
        let caller: T::AccountId = whitelisted_caller();
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), MAX_GATE_FLAGS as u32 + 1, 0, 1);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert_eq!(GateBreachFlags::<T>::iter().count(), MAX_GATE_FLAGS);
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
