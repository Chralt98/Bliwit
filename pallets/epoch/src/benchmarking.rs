//! `frame-benchmarking` v2 coverage for every epoch dispatchable (15 §4.5).
//! B5 replaces the hand-seeded weights after assembled-runtime PoV calibration.

use super::*;
use crate::pallet::{NextProposalId, Pallet, TickBatch};
use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_support::{
    pallet_prelude::{Blake2_128Concat, OptionQuery, ValueQuery},
    storage::types::{StorageDoubleMap, StorageMap, StorageValue},
    traits::{EnsureOrigin, StorageInstance},
    Twox64Concat,
};
use frame_system::RawOrigin;
use futarchy_primitives::{phase_offsets, DecisionOutcome, EpochPhase, ProposalState};
use sp_runtime::SaturatedConversion;

// `pallet-epoch` deliberately depends on welfare only through `WelfareSettlement`.
// These benchmark-only aliases address the public welfare storage prefixes without
// adding a production dependency between the two settlement pallets. The runtime's
// `prime_settlement` helper seeds the typed snapshots/gate flags; this file adds the
// two B4-residual auxiliary histories that the terminal prune must also retire.
macro_rules! welfare_storage_instance {
    ($name:ident, $storage:literal) => {
        struct $name;
        impl StorageInstance for $name {
            fn pallet_prefix() -> &'static str {
                "Welfare"
            }

            const STORAGE_PREFIX: &'static str = $storage;
        }
    };
}

welfare_storage_instance!(WelfareSnapshots, "Snapshots");
welfare_storage_instance!(WelfareGateBreachFlags, "GateBreachFlags");
welfare_storage_instance!(WelfareSampledGateDays, "SampledGateDays");
welfare_storage_instance!(WelfareXcmTraffic, "XcmTraffic");
welfare_storage_instance!(WelfareXcmTrafficEpochs, "XcmTrafficEpochs");

type BenchmarkSnapshots =
    StorageMap<WelfareSnapshots, Blake2_128Concat, (EpochId, MetricSpecVersion), (), OptionQuery>;
type BenchmarkGateBreachFlags =
    StorageMap<WelfareGateBreachFlags, Blake2_128Concat, EpochId, (), OptionQuery>;
type BenchmarkSampledGateDays =
    StorageMap<WelfareSampledGateDays, Blake2_128Concat, EpochId, [u32; 2], OptionQuery>;
type BenchmarkXcmTraffic = StorageDoubleMap<
    WelfareXcmTraffic,
    Twox64Concat,
    EpochId,
    Twox64Concat,
    u8,
    [u64; 3],
    ValueQuery,
>;
// `Vec<EpochId>` has the same SCALE representation as welfare's bounded vector.
// Setup derives the exact capacity from the already-seeded snapshot window.
type BenchmarkXcmTrafficEpochs = StorageValue<WelfareXcmTrafficEpochs, Vec<EpochId>, ValueQuery>;

const TERMINAL_SETTLEMENT_EPOCH: EpochId = 1_000;
// Benchmark-only mirror of welfare's frozen 21-epoch index capacity. Epoch
// deliberately has no production dependency on pallet-welfare, so the aliases
// above cannot name its Rust constant directly.
const XCM_TRAFFIC_FULL_BACKLOG_EPOCHS: EpochId = 21;
const XCM_TRAFFIC_DRAINED_PER_CALL: usize = 2;
const XCM_TRAFFIC_DAYS_PER_EPOCH: usize = 1usize << u8::BITS;

struct XcmTrafficFixture {
    epochs: Vec<EpochId>,
    entries: usize,
}

struct WelfareRetirementFixture {
    snapshots: usize,
    gate_flags: usize,
    traffic: XcmTrafficFixture,
}

fn seed_xcm_traffic_history(mut traffic_epochs: Vec<EpochId>) -> XcmTrafficFixture {
    traffic_epochs.sort_unstable();
    traffic_epochs.dedup();
    BenchmarkXcmTrafficEpochs::put(traffic_epochs.clone());
    for epoch in &traffic_epochs {
        for day in u8::MIN..=u8::MAX {
            BenchmarkXcmTraffic::insert(epoch, day, [u64::MAX; 3]);
        }
    }

    let traffic_entries = traffic_epochs
        .len()
        .saturating_mul(XCM_TRAFFIC_DAYS_PER_EPOCH);
    assert_eq!(BenchmarkXcmTraffic::iter_keys().count(), traffic_entries);

    XcmTrafficFixture {
        epochs: traffic_epochs,
        entries: traffic_entries,
    }
}

/// Mirror the configured welfare seam for the pallet-only benchmark test
/// runtime, which has no pallet-welfare instance behind these raw aliases.
#[cfg(test)]
pub(crate) fn prune_benchmark_xcm_traffic(cutoff_epoch: EpochId) {
    BenchmarkXcmTrafficEpochs::mutate(|epochs| {
        for _ in 0..XCM_TRAFFIC_DRAINED_PER_CALL {
            let oldest = epochs
                .iter()
                .filter(|epoch| **epoch < cutoff_epoch)
                .min()
                .copied();
            let Some(epoch) = oldest else {
                break;
            };
            let _ = BenchmarkXcmTraffic::clear_prefix(epoch, u8::MAX as u32 + 1, None);
            if let Some(position) = epochs.iter().position(|stored| *stored == epoch) {
                epochs.remove(position);
            }
        }
    });
}

fn assert_bounded_xcm_traffic_drain(fixture: &XcmTrafficFixture) {
    let remaining = fixture
        .epochs
        .iter()
        .skip(XCM_TRAFFIC_DRAINED_PER_CALL)
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(BenchmarkXcmTrafficEpochs::get(), remaining);
    for epoch in fixture.epochs.iter().take(XCM_TRAFFIC_DRAINED_PER_CALL) {
        assert_eq!(BenchmarkXcmTraffic::iter_prefix(epoch).count(), 0);
    }
    for epoch in fixture.epochs.iter().skip(XCM_TRAFFIC_DRAINED_PER_CALL) {
        assert_eq!(
            BenchmarkXcmTraffic::iter_prefix(epoch).count(),
            XCM_TRAFFIC_DAYS_PER_EPOCH
        );
    }
    assert_eq!(
        BenchmarkXcmTraffic::iter_keys().count(),
        fixture.entries.saturating_sub(
            XCM_TRAFFIC_DRAINED_PER_CALL.saturating_mul(XCM_TRAFFIC_DAYS_PER_EPOCH)
        )
    );
}

fn seed_welfare_retirement_history() -> Option<WelfareRetirementFixture> {
    let snapshots = BenchmarkSnapshots::iter_keys().count();
    // The pallet-only mock has a seam double rather than pallet-welfare storage.
    // The assembled runtime helper populates this map at its retained capacity.
    if snapshots == 0 {
        return None;
    }

    let gate_epochs = BenchmarkGateBreachFlags::iter_keys().collect::<Vec<_>>();
    let mut traffic_epochs = BenchmarkSnapshots::iter_keys()
        .map(|(epoch, _)| epoch)
        .collect::<Vec<_>>();
    traffic_epochs.sort_unstable();
    traffic_epochs.dedup();
    if let Some(last) = traffic_epochs.last().copied() {
        traffic_epochs.push(last.saturating_add(1));
    }

    for epoch in &gate_epochs {
        BenchmarkSampledGateDays::insert(epoch, [u32::MAX; 2]);
    }
    let traffic = seed_xcm_traffic_history(traffic_epochs);

    assert_eq!(snapshots, gate_epochs.len());
    assert_eq!(BenchmarkSampledGateDays::iter_keys().count(), snapshots);

    Some(WelfareRetirementFixture {
        snapshots,
        gate_flags: gate_epochs.len(),
        traffic,
    })
}

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
    proposal.resources = max_resources(pid)
        .try_into()
        .expect("benchmark resources equal the kernel bound");
    proposal.state = state;
    proposal.decide_at = now;
    proposal
}

fn max_resources(pid: ProposalId) -> Vec<futarchy_primitives::ResourceId> {
    (0..futarchy_primitives::bounds::MAX_RESOURCES_PER_PROPOSAL)
        .map(|index| [pid as u8, index as u8, 0, 0, 0, 0, 0, 0])
        .collect()
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
    state.resource_locks.extend(
        proposal
            .resources
            .iter()
            .copied()
            .map(|resource| (resource, pid)),
    );
    state.proposals.push(proposal);
    state
}

fn is_live(state: ProposalState) -> bool {
    !matches!(
        state,
        ProposalState::Submitted
            | ProposalState::Screening
            | ProposalState::Cancelled
            | ProposalState::Settled
            | ProposalState::Rejected(_)
            | ProposalState::Expired
    )
}

fn dummy_markets(pid: ProposalId, epoch: EpochId) -> futarchy_primitives::MarketSet {
    let first = pid.saturating_mul(10);
    futarchy_primitives::MarketSet {
        accept: first.saturating_add(1),
        reject: first.saturating_add(2),
        gates: Some([
            first.saturating_add(3),
            first.saturating_add(4),
            first.saturating_add(5),
            first.saturating_add(6),
        ]),
        baseline: 9_000u64.saturating_add(u64::from(epoch)),
    }
}

fn next_aux_id<T: Config>(state: &EpochState<T::AccountId>, cursor: &mut ProposalId) -> ProposalId {
    while state
        .proposals
        .iter()
        .any(|proposal| proposal.id == *cursor)
    {
        *cursor = cursor.saturating_add(1);
    }
    let id = *cursor;
    *cursor = cursor.saturating_add(1);
    id
}

/// Fill every collection rebuilt and rewritten by `Epoch::mutate`. The target
/// counts reserve space for calls that create an intake item or a cohort. When
/// a full tick batch is present, four five-member cohorts cannot coexist
/// with the 32-live-proposal ceiling; the fixture distributes the 12 remaining
/// live slots across all four cohorts (three members each), which is the worst
/// simultaneously reachable combination of those independent bounds.
pub fn fill_epoch_state<T: Config>(
    state: &mut EpochState<T::AccountId>,
    intake_target: usize,
    live_target: usize,
    cohort_target: usize,
) {
    let mut cursor = 10_000u64;

    while state.cohorts.len() < cohort_target {
        let remaining_cohorts = cohort_target.saturating_sub(state.cohorts.len());
        let live = state
            .proposals
            .iter()
            .filter(|proposal| is_live(proposal.state))
            .count();
        let available = live_target.saturating_sub(live);
        let members = MAX_ACTIVE_PER_EPOCH.min(available / remaining_cohorts.max(1));
        let epoch = 100u32.saturating_add(state.cohorts.len() as u32);
        let mut ids = Vec::new();
        for _ in 0..members {
            let pid = next_aux_id::<T>(state, &mut cursor);
            let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Measuring, epoch);
            proposal.markets = Some(dummy_markets(pid, epoch));
            proposal.decision = Some(DecisionOutcome::Adopt);
            state.proposals.push(proposal);
            ids.push(pid);
        }
        state.cohorts.push(CoreCohortInfo {
            epoch,
            proposals: ids,
            status: CohortStatus::Measuring {
                until_epoch: epoch.saturating_add(2),
            },
        });
    }

    while state
        .proposals
        .iter()
        .filter(|proposal| is_live(proposal.state))
        .count()
        < live_target
    {
        let pid = next_aux_id::<T>(state, &mut cursor);
        let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Trading, 200);
        proposal.markets = Some(dummy_markets(pid, 200));
        state.proposals.push(proposal);
    }

    while state
        .proposals
        .iter()
        .filter(|proposal| proposal.state == ProposalState::Submitted)
        .count()
        < intake_target
    {
        let pid = next_aux_id::<T>(state, &mut cursor);
        state
            .proposals
            .push(benchmark_proposal::<T>(pid, ProposalState::Submitted, 0));
        state.intake_queue.push(pid);
    }

    state.resource_locks.clear();
    for proposal in state
        .proposals
        .iter_mut()
        .filter(|proposal| is_live(proposal.state))
    {
        proposal.resources = max_resources(proposal.id)
            .try_into()
            .expect("benchmark resources equal the kernel bound");
        state.resource_locks.extend(
            proposal
                .resources
                .iter()
                .copied()
                .map(|resource| (resource, proposal.id)),
        );
    }
    state.rollovers = state
        .proposals
        .iter()
        .filter(|proposal| proposal.state == ProposalState::Submitted)
        .map(|proposal| (proposal.id, 1))
        .collect();
    state.recent = (0..RECENT_COHORTS)
        .map(|index| futarchy_primitives::CohortSummary {
            epoch: 1_000u32.saturating_add(index as u32),
            s_1e9: FixedU64(500_000_000),
            baseline_twap_1e9: FixedU64(500_000_000),
            proposals: futarchy_primitives::BoundedVec::try_from(
                (0..MAX_ACTIVE_PER_EPOCH)
                    .map(|offset| {
                        (
                            50_000u64
                                .saturating_add((index * MAX_ACTIVE_PER_EPOCH + offset) as u64),
                            ProposalClass::Param,
                            DecisionOutcome::Adopt,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .expect("benchmark summary equals the cohort bound"),
            voided: false,
            settled_at: index as u32,
        })
        .collect();
    state.proposal_id_high_water = state
        .proposals
        .iter()
        .map(|proposal| proposal.id)
        .max()
        .unwrap_or(0);
}

#[benchmarks(where T: Config)]
mod benches {
    use super::*;

    #[benchmark]
    fn submit() -> Result<(), BenchmarkError> {
        T::BenchmarkHelper::prime_submit_epoch(0);
        let caller = T::BenchmarkHelper::account(1);
        let mut state = EpochState::new();
        // `submit` requires a free slot in both bounded collections before it
        // appends, so 39 intake and 31 live entries are the maximum admissible
        // pre-state; the measured call fills both bounds.
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE - 1,
            MAX_LIVE_PROPOSALS - 1,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
        let pid = NextProposalId::<T>::get();
        let now = frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>();
        let mut proposal = T::BenchmarkHelper::proposal(pid, caller.clone(), now, 0);
        proposal.resources = max_resources(pid)
            .try_into()
            .expect("benchmark resources equal the kernel bound");

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), proposal);

        assert_eq!(crate::IntakeQueue::<T>::get().len(), MAX_INTAKE_QUEUE);
        Ok(())
    }

    #[benchmark]
    fn withdraw() -> Result<(), BenchmarkError> {
        let caller = T::BenchmarkHelper::account(1);
        let mut proposal = T::BenchmarkHelper::proposal(1, caller.clone(), 1, 0);
        proposal.resources = max_resources(1)
            .try_into()
            .expect("benchmark resources equal the kernel bound");
        let mut state = EpochState::new();
        state.proposals.push(proposal);
        state.intake_queue.push(1);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), 1);

        assert_eq!(crate::IntakeQueue::<T>::get().len(), MAX_INTAKE_QUEUE - 1);
        Ok(())
    }

    // The runtime fetches PreimageFor by (hash, recorded_len), and admission caps
    // recorded_len at the 64 KiB kernel maximum, so measured PoV is the true bound.
    // The recorder still measures the full proof for 512 maximal fixed counters.
    // Ignore only the generator's synthetic per-key estimate for this unbounded
    // double map: it assumes 512 independent maximum-depth trie paths instead of
    // their shared prefix. The generated total proof envelope remains above the
    // benchmark's recorded proof at every sampled component.
    #[benchmark(pov_mode = MaxEncodedLen {
        Preimage::PreimageFor: Measured,
        Welfare::XcmTraffic: Ignored
    })]
    fn tick(n: Linear<1, TICK_BATCH_BOUND>) -> Result<(), BenchmarkError> {
        let params = T::Params::get();
        let mut state = EpochState::new();
        let mut ids = Vec::new();
        let mut payload_hashes = Vec::new();
        for pid in 1..=u64::from(n) {
            let mut proposal =
                benchmark_proposal::<T>(pid, ProposalState::Qualified, TERMINAL_SETTLEMENT_EPOCH);
            if proposal.payload_len != futarchy_primitives::kernel::MAX_BYTES
                || payload_hashes.contains(&proposal.payload_hash)
            {
                return Err(BenchmarkError::Stop(
                    "tick items require distinct maximum-size payload preimages",
                ));
            }
            payload_hashes.push(proposal.payload_hash);
            T::Preimage::request(proposal.payload_hash)
                .map_err(|_| BenchmarkError::Stop("tick qualification preimage pin failed"))?;
            crate::QualificationPreimageRequests::<T>::insert(pid, proposal.payload_hash);
            proposal.class = ProposalClass::Code;
            proposal.decide_at = block_for(
                TERMINAL_SETTLEMENT_EPOCH,
                phase_offsets::DECIDE_NUM,
                params.epoch_length,
            );
            state.proposals.push(proposal);
            ids.push(pid);
        }
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        state.epoch.index = TERMINAL_SETTLEMENT_EPOCH;
        state.epoch.phase = EpochPhase::Seed;
        state.epoch.length = params.epoch_length;
        state.epoch.next_length = params.epoch_length;
        state.epoch.epoch_start_block =
            TERMINAL_SETTLEMENT_EPOCH.saturating_mul(params.epoch_length);
        state.epoch.phase_start_block = block_for(
            TERMINAL_SETTLEMENT_EPOCH,
            phase_offsets::SEED_NUM,
            params.epoch_length,
        );
        Pallet::<T>::seed(state)?;
        let traffic =
            seed_xcm_traffic_history((0..XCM_TRAFFIC_FULL_BACKLOG_EPOCHS).collect::<Vec<_>>());
        assert_eq!(
            traffic.epochs.len(),
            XCM_TRAFFIC_FULL_BACKLOG_EPOCHS as usize
        );
        set_block::<T>(block_for(
            TERMINAL_SETTLEMENT_EPOCH,
            phase_offsets::SEED_NUM,
            params.epoch_length,
        ));
        let pids = TickBatch::try_from(ids)
            .map_err(|_| BenchmarkError::Stop("benchmark tick batch exceeded"))?;
        let caller = T::BenchmarkHelper::account(250);
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pids);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::DecisionCritical,
        );
        assert_eq!(crate::Proposals::<T>::count(), MAX_LIVE_PROPOSALS_BOUND);
        assert_bounded_xcm_traffic_drain(&traffic);
        Ok(())
    }

    // The runtime fetches PreimageFor by (hash, recorded_len), and admission caps
    // recorded_len at the 64 KiB kernel maximum, so measured PoV is the true bound.
    #[benchmark(pov_mode = MaxEncodedLen {
        Preimage::PreimageFor: Measured
    })]
    fn decide() -> Result<(), BenchmarkError> {
        let pid = 1;
        let caller = T::BenchmarkHelper::account(250);
        set_block::<T>(1);
        let mut state = EpochState::new();
        let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Trading, 0);
        proposal.class = ProposalClass::Code;
        T::Preimage::request(proposal.payload_hash)
            .map_err(|_| BenchmarkError::Stop("decide qualification preimage pin failed"))?;
        crate::QualificationPreimageRequests::<T>::insert(pid, proposal.payload_hash);
        proposal.markets = Some(T::BenchmarkHelper::prime_decision(pid, 0, true));
        T::BenchmarkHelper::prime_guard_enqueue(pid);
        state.resource_locks = proposal
            .resources
            .iter()
            .copied()
            .map(|resource| (resource, pid))
            .collect();
        state.proposals.push(proposal);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), pid);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::DecisionCritical,
        );
        assert_eq!(
            crate::Proposals::<T>::get(pid).map(|proposal| proposal.state),
            Some(ProposalState::Queued)
        );
        Ok(())
    }

    // The terminal fixture fills every touched collection to capacity and all
    // 512 drainable traffic keys hold the maximum fixed counter payload. The
    // recorder measures their full proof, but the generator's unbounded-map
    // estimate assumes 512 independent maximum-depth trie paths instead of the
    // shared double-map prefix. Ignore only that synthetic per-key estimate; the
    // generated total proof envelope remains above the recorded proof at every
    // sampled component. Every other storage remains MaxEncodedLen.
    #[benchmark(pov_mode = MaxEncodedLen {
        Welfare::XcmTraffic: Ignored
    })]
    fn settle_cohort(n: Linear<1, MAX_COHORT_PROPOSALS_BOUND>) -> Result<(), BenchmarkError> {
        let params = T::Params::get();
        let mut state = EpochState::new();
        let mut ids = Vec::new();
        // Keep the cohort at its five-proposal bound for every sample. `n` is
        // still the exact dispatch batch: the cursor places the measured range
        // on the final `n` items (ending with Baseline), so every point executes
        // terminal reap + welfare retirement while preserving the runtime
        // WeightInfo argument's meaning.
        for pid in 1..=u64::from(MAX_COHORT_PROPOSALS_BOUND) {
            let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Measuring, 0);
            proposal.markets = Some(T::BenchmarkHelper::prime_decision(pid, 0, true));
            proposal.decision = Some(DecisionOutcome::Adopt);
            state.resource_locks.extend(
                proposal
                    .resources
                    .iter()
                    .copied()
                    .map(|resource| (resource, pid)),
            );
            state.proposals.push(proposal);
            ids.push(pid);
        }
        state.cohorts.push(CoreCohortInfo {
            epoch: 0,
            proposals: ids,
            status: CohortStatus::Settling {
                cursor: MAX_COHORT_PROPOSALS_BOUND
                    .saturating_add(1)
                    .saturating_sub(n),
            },
        });
        state.epoch.index = TERMINAL_SETTLEMENT_EPOCH;
        state.epoch.phase = futarchy_primitives::EpochPhase::Housekeeping;
        state.epoch.length = params.epoch_length;
        state.epoch.next_length = params.epoch_length;
        state.epoch.epoch_start_block =
            TERMINAL_SETTLEMENT_EPOCH.saturating_mul(params.epoch_length);
        state.epoch.phase_start_block = block_for(
            TERMINAL_SETTLEMENT_EPOCH,
            phase_offsets::HOUSEKEEPING_NUM,
            params.epoch_length,
        );
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
        T::BenchmarkHelper::prime_settlement(0);
        let welfare = seed_welfare_retirement_history();
        set_block::<T>(block_for(
            TERMINAL_SETTLEMENT_EPOCH,
            phase_offsets::HOUSEKEEPING_NUM,
            params.epoch_length,
        ));
        let caller = T::BenchmarkHelper::account(250);
        T::BenchmarkHelper::prime_keeper_rebate();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), 0, n);

        T::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::DecisionCritical,
        );
        assert!(!crate::Cohorts::<T>::contains_key(0));
        if let Some(welfare) = welfare {
            assert_eq!(welfare.snapshots, welfare.gate_flags);
            assert_eq!(
                welfare.traffic.epochs.len(),
                welfare.snapshots.saturating_add(1)
            );
            assert_eq!(
                welfare.traffic.entries,
                welfare
                    .traffic
                    .epochs
                    .len()
                    .saturating_mul(XCM_TRAFFIC_DAYS_PER_EPOCH)
            );
            assert_eq!(BenchmarkSnapshots::iter_keys().count(), 0);
            assert_eq!(BenchmarkGateBreachFlags::iter_keys().count(), 0);
            assert_eq!(BenchmarkSampledGateDays::iter_keys().count(), 0);
            assert_bounded_xcm_traffic_drain(&welfare.traffic);
        }
        Ok(())
    }

    #[benchmark]
    fn set_next_epoch_length() -> Result<(), BenchmarkError> {
        let mut state = EpochState::new();
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
        let origin = T::BenchmarkHelper::constitutional_values_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin);

        assert_eq!(
            crate::Schedule::<T>::get().next_length,
            T::Params::get().epoch_length
        );
        Ok(())
    }

    #[benchmark]
    fn delay_once() -> Result<(), BenchmarkError> {
        let mut state = callback_state::<T>(1, ProposalState::Queued);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
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
    fn mark_executed() -> Result<(), BenchmarkError> {
        let mut state = callback_state::<T>(1, ProposalState::Queued);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS - 1,
        );
        Pallet::<T>::seed(state)?;
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
        let mut state = callback_state::<T>(1, ProposalState::Queued);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
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
        let mut state = callback_state::<T>(1, ProposalState::FailedExecuted);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS - 1,
        );
        Pallet::<T>::seed(state)?;
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
        let mut state = callback_state::<T>(1, ProposalState::Queued);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS - 1,
        );
        Pallet::<T>::seed(state)?;
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
        let mut state = callback_state::<T>(1, ProposalState::Trading);
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
        let origin = T::BenchmarkHelper::guardian_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 1);

        assert!(!crate::Proposals::<T>::contains_key(1));
        Ok(())
    }

    #[benchmark]
    fn void_cohort(n: Linear<1, MAX_COHORT_PROPOSALS_BOUND>) -> Result<(), BenchmarkError> {
        let mut state = EpochState::new();
        let mut ids = Vec::new();
        for pid in 1..=u64::from(n) {
            let mut proposal = benchmark_proposal::<T>(pid, ProposalState::Measuring, 0);
            proposal.markets = Some(T::BenchmarkHelper::prime_decision(pid, 0, true));
            state.resource_locks.extend(
                proposal
                    .resources
                    .iter()
                    .copied()
                    .map(|resource| (resource, pid)),
            );
            state.proposals.push(proposal);
            ids.push(pid);
        }
        state.cohorts.push(CoreCohortInfo {
            epoch: 0,
            proposals: ids,
            status: CohortStatus::Measuring { until_epoch: 2 },
        });
        fill_epoch_state::<T>(
            &mut state,
            MAX_INTAKE_QUEUE,
            MAX_LIVE_PROPOSALS,
            MAX_NON_TERMINAL_COHORTS,
        );
        Pallet::<T>::seed(state)?;
        let origin = T::BenchmarkHelper::void_authority_origin();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, 0);

        assert!(!crate::Cohorts::<T>::contains_key(0));
        assert!(crate::RecentCohortSummaries::<T>::get()
            .iter()
            .any(|summary| summary.epoch == 0 && summary.voided));
        Ok(())
    }

    #[benchmark]
    fn set_intake_paused() -> Result<(), BenchmarkError> {
        let origin = T::EmergencyPlaybookOrigin::try_successful_origin()
            .map_err(|_| BenchmarkError::Stop("EmergencyPlaybook origin unavailable"))?;
        let now = frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>();
        let expiry = now.saturating_add(futarchy_primitives::kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS);

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, true, expiry);

        assert_eq!(crate::IntakePausedUntil::<T>::get(), Some(expiry));
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
