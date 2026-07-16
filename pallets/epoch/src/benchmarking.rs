//! `frame-benchmarking` v2 coverage for every epoch dispatchable (15 §4.5).
//! B5 replaces the hand-seeded weights after assembled-runtime PoV calibration.

use super::*;
use crate::pallet::{Pallet, TickBatch};
use frame_benchmarking::v2::*;
use frame_system::RawOrigin;
use futarchy_primitives::{phase_offsets, DecisionOutcome, ProposalState};
use sp_runtime::SaturatedConversion;

fn block_for(epoch: EpochId, numerator: BlockNumber, length: BlockNumber) -> BlockNumber {
    epoch
        .saturating_mul(length)
        .saturating_add(length.saturating_mul(numerator) / phase_offsets::DENOMINATOR)
}

fn set_block<T: Config>(block: BlockNumber) {
    frame_system::Pallet::<T>::set_block_number(block.saturated_into());
}

fn benchmark_proposal<T: Config>(
    pid: ProposalId,
    state: ProposalState,
    epoch: EpochId,
) -> Proposal<T::AccountId> {
    let now = frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>();
    let who = T::BenchmarkHelper::account((pid % 200) as u8);
    let mut proposal = T::BenchmarkHelper::proposal(pid, who, now, epoch);
    proposal.state = state;
    proposal.decide_at = now;
    proposal
}

fn callback_state<T: Config>(
    pid: ProposalId,
    proposal_state: ProposalState,
) -> EpochState<T::AccountId> {
    let mut state = EpochState::new();
    let mut proposal = benchmark_proposal::<T>(pid, proposal_state, 0);
    proposal.markets = Some(T::BenchmarkHelper::prime_decision(pid, 0, false));
    proposal.maturity = Some(1);
    proposal.grace_end = Some(1u32.saturating_add(T::Params::get().grace[0]));
    proposal.decision = Some(DecisionOutcome::Adopt);
    state.proposals.push(proposal);
    state
}

#[benchmarks(where T: Config)]
mod benches {
    use super::*;

    #[benchmark]
    fn submit() -> Result<(), BenchmarkError> {
        let caller = T::BenchmarkHelper::account(1);
        let now = frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>();
        let proposal = T::BenchmarkHelper::proposal(1, caller.clone(), now, 0);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), proposal);

        assert_eq!(crate::IntakeQueue::<T>::get().len(), 1);
        Ok(())
    }

    #[benchmark]
    fn withdraw() -> Result<(), BenchmarkError> {
        let caller = T::BenchmarkHelper::account(1);
        let proposal = T::BenchmarkHelper::proposal(1, caller.clone(), 1, 0);
        let mut state = EpochState::new();
        state.proposals.push(proposal);
        state.intake_queue.push(1);
        Pallet::<T>::seed(state)?;

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), 1);

        assert!(crate::IntakeQueue::<T>::get().is_empty());
        Ok(())
    }

    #[benchmark]
    fn tick() -> Result<(), BenchmarkError> {
        let params = T::Params::get();
        let mut state = EpochState::new();
        let mut ids = Vec::new();
        for pid in 1..=TICK_BATCH_BOUND as u64 {
            let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Qualified, 0);
            proposal.decide_at = block_for(0, phase_offsets::DECIDE_NUM, params.epoch_length);
            state.proposals.push(proposal);
            let _ = T::BenchmarkHelper::prime_decision(pid, 0, false);
            ids.push(pid);
        }
        Pallet::<T>::seed(state)?;
        set_block::<T>(block_for(0, phase_offsets::SEED_NUM, params.epoch_length));
        let pids = TickBatch::try_from(ids)
            .map_err(|_| BenchmarkError::Stop("benchmark tick batch exceeded"))?;
        let caller = T::BenchmarkHelper::account(250);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pids);

        assert_eq!(crate::Proposals::<T>::count(), TICK_BATCH_BOUND);
        Ok(())
    }

    #[benchmark]
    fn decide() -> Result<(), BenchmarkError> {
        let pid = 1;
        let caller = T::BenchmarkHelper::account(250);
        let mut state = EpochState::new();
        let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Trading, 0);
        proposal.markets = Some(T::BenchmarkHelper::prime_decision(pid, 0, false));
        state.resource_locks = proposal
            .resources
            .iter()
            .copied()
            .map(|resource| (resource, pid))
            .collect();
        state.proposals.push(proposal);
        Pallet::<T>::seed(state)?;

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pid);

        assert_eq!(
            crate::Proposals::<T>::get(pid).map(|proposal| proposal.state),
            Some(ProposalState::Queued)
        );
        Ok(())
    }

    #[benchmark]
    fn settle_cohort() -> Result<(), BenchmarkError> {
        let params = T::Params::get();
        let pid = 1;
        let mut state = EpochState::new();
        let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Measuring, 0);
        proposal.markets = Some(T::BenchmarkHelper::prime_decision(pid, 0, false));
        proposal.decision = Some(DecisionOutcome::Adopt);
        state.proposals.push(proposal);
        state.cohorts.push(CoreCohortInfo {
            epoch: 0,
            proposals: vec![pid],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        Pallet::<T>::seed(state)?;
        T::BenchmarkHelper::prime_settlement(0);
        set_block::<T>(block_for(
            3,
            phase_offsets::HOUSEKEEPING_NUM,
            params.epoch_length,
        ));
        let caller = T::BenchmarkHelper::account(250);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), 0, 2);

        assert!(!crate::Cohorts::<T>::contains_key(0));
        Ok(())
    }

    #[benchmark]
    fn set_next_epoch_length() {
        let origin = T::BenchmarkHelper::constitutional_values_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin);

        assert_eq!(
            crate::Schedule::<T>::get().next_length,
            T::Params::get().epoch_length
        );
    }

    #[benchmark]
    fn delay_once() -> Result<(), BenchmarkError> {
        Pallet::<T>::seed(callback_state::<T>(1, ProposalState::Queued))?;
        let origin = T::BenchmarkHelper::guardian_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 1, [1; 32]);

        assert_eq!(
            crate::Proposals::<T>::get(1).map(|p| p.state),
            Some(ProposalState::Suspended)
        );
        Ok(())
    }

    #[benchmark]
    fn veto_upheld() -> Result<(), BenchmarkError> {
        Pallet::<T>::seed(callback_state::<T>(1, ProposalState::Suspended))?;
        let origin = T::BenchmarkHelper::guardian_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 1);

        assert_eq!(
            crate::Proposals::<T>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        Ok(())
    }

    #[benchmark]
    fn mark_executed() -> Result<(), BenchmarkError> {
        Pallet::<T>::seed(callback_state::<T>(1, ProposalState::Queued))?;
        let origin = T::BenchmarkHelper::execution_guard_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 1);

        assert_eq!(
            crate::Proposals::<T>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        Ok(())
    }

    #[benchmark]
    fn mark_failed_executed() -> Result<(), BenchmarkError> {
        Pallet::<T>::seed(callback_state::<T>(1, ProposalState::Queued))?;
        let origin = T::BenchmarkHelper::execution_guard_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 1);

        assert_eq!(
            crate::Proposals::<T>::get(1).map(|p| p.state),
            Some(ProposalState::FailedExecuted)
        );
        Ok(())
    }

    #[benchmark]
    fn retry_exhausted_to_measurement() -> Result<(), BenchmarkError> {
        Pallet::<T>::seed(callback_state::<T>(1, ProposalState::FailedExecuted))?;
        let origin = T::BenchmarkHelper::execution_guard_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 1);

        assert_eq!(
            crate::Proposals::<T>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        Ok(())
    }

    #[benchmark]
    fn expire_or_stale_queue() -> Result<(), BenchmarkError> {
        Pallet::<T>::seed(callback_state::<T>(1, ProposalState::Queued))?;
        let origin = T::BenchmarkHelper::execution_guard_origin();

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            1,
            Some(RejectReason::StaleQueue),
        );

        assert_eq!(
            crate::Proposals::<T>::get(1).map(|p| p.state),
            Some(ProposalState::Measuring)
        );
        Ok(())
    }

    #[benchmark]
    fn force_reject_process_hold() -> Result<(), BenchmarkError> {
        Pallet::<T>::seed(callback_state::<T>(1, ProposalState::Trading))?;
        let origin = T::BenchmarkHelper::guardian_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 1);

        assert!(!crate::Proposals::<T>::contains_key(1));
        Ok(())
    }

    #[benchmark]
    fn void_cohort() -> Result<(), BenchmarkError> {
        let pid = 1;
        let mut state = EpochState::new();
        let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Measuring, 0);
        proposal.markets = Some(T::BenchmarkHelper::prime_decision(pid, 0, false));
        state.proposals.push(proposal);
        state.cohorts.push(CoreCohortInfo {
            epoch: 0,
            proposals: vec![pid],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        Pallet::<T>::seed(state)?;
        let origin = T::BenchmarkHelper::void_authority_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 0);

        assert_eq!(
            crate::Cohorts::<T>::get(0).map(|cohort| cohort.status),
            Some(CohortStatus::Void)
        );
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
