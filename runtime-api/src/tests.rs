use core::fmt::Debug;

use futarchy_primitives::{
    bounds, AccountId, BoundedVec, Branch, CohortSummaryView, DecisionOutcome, DecisionStatsView,
    EpochPhase, EpochStatusView, FixedU64, NavView, OracleRoundView, ParamKey, ParamView,
    PositionId, PositionKind, PositionView, ProposalClass, ProposalState, ProposalSummaryView,
    QueuedExecutionView, QuoteView, RatificationStatus, RuntimeVersionConstraint, TradeSide,
    VaultState, WelfareView,
};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use sp_runtime::traits::Block as BlockT;

use super::{runtime_decl_for_futarchy_api, FutarchyApi, MAX_QUEUED_EXECUTIONS};

struct MockApi;
type Block = sp_runtime::generic::Block<sp_runtime::testing::Header, sp_runtime::OpaqueExtrinsic>;

sp_api::mock_impl_runtime_apis! {
    impl FutarchyApi<Block> for MockApi {
        fn epoch_status() -> EpochStatusView {
            epoch_status()
        }

        fn proposal_summaries() -> BoundedVec<ProposalSummaryView, { bounds::MAX_PROPOSAL_SUMMARIES }> {
            singleton(proposal_summary())
        }

        fn quote(market: u64, side: TradeSide, amount: u128) -> QuoteView {
            quote(market, side, amount)
        }

        fn decision_stats(pid: u64) -> Option<DecisionStatsView> {
            Some(decision_stats(pid))
        }

        fn account_positions(who: AccountId) -> BoundedVec<PositionView, { bounds::MAX_ACCOUNT_POSITIONS }> {
            singleton(position(who))
        }

        fn execution_queue() -> BoundedVec<QueuedExecutionView, { MAX_QUEUED_EXECUTIONS }> {
            singleton(queued_execution())
        }

        fn welfare_current() -> WelfareView {
            welfare()
        }

        fn params(keys: BoundedVec<ParamKey, { bounds::MAX_PARAM_KEYS }>) -> BoundedVec<ParamView, { bounds::MAX_PARAM_KEYS }> {
            keys.into_iter()
                .map(param)
                .collect::<Vec<_>>()
                .try_into()
                .expect("mapping one output per bounded input preserves the bound")
        }

        fn nav() -> NavView {
            nav()
        }

        fn recent_cohorts() -> BoundedVec<CohortSummaryView, { bounds::RECENT_COHORT_SUMMARIES }> {
            singleton(cohort())
        }

        fn open_oracle_rounds() -> BoundedVec<OracleRoundView, { bounds::MAX_OPEN_ORACLE_ROUNDS }> {
            singleton(oracle_round())
        }
    }
}

fn singleton<T, const N: u32>(value: T) -> BoundedVec<T, N> {
    vec![value]
        .try_into()
        .expect("a single item must fit every API collection bound")
}

fn epoch_status() -> EpochStatusView {
    EpochStatusView {
        index: 7,
        phase: EpochPhase::Trade,
        phase_start_block: 101,
        next_boundary: 202,
        dead_man_armed: true,
        ledger_frozen: false,
        phase_flags: 0b101,
    }
}

fn proposal_summary() -> ProposalSummaryView {
    ProposalSummaryView {
        id: 11,
        class: ProposalClass::Treasury,
        state: ProposalState::Queued,
        proposer: [1; 32],
        epoch: 7,
        payload_hash: [2; 32],
        ask: 3_000_000,
        decision_market: Some((21, 22)),
        gate_markets: Some([23, 24, 25, 26]),
        decide_at: 303,
        maturity: Some(404),
        ratification: RatificationStatus::Pending { referendum: 9 },
    }
}

fn quote(market: u64, side: TradeSide, amount: u128) -> QuoteView {
    QuoteView {
        cost: amount,
        fee: u128::from(market),
        p_after_1e9: FixedU64(510_000_000),
        max_trade: 8_000_000,
        within_domain: side == TradeSide::BuyLong,
    }
}

fn decision_stats(pid: u64) -> DecisionStatsView {
    DecisionStatsView {
        pid,
        twap_accept_1e9: FixedU64(600_000_000),
        twap_reject_1e9: FixedU64(400_000_000),
        twap_baseline_1e9: FixedU64(450_000_000),
        r_eff_1e9: FixedU64(440_000_000),
        trailing_accept_1e9: FixedU64(610_000_000),
        trailing_reject_1e9: FixedU64(390_000_000),
        coverage_pct: 95,
        traded_volume: 12_000_000,
        v_min_required: 10_000_000,
        converged: true,
        gate_twaps_1e9: Some([
            FixedU64(10_000_000),
            FixedU64(20_000_000),
            FixedU64(30_000_000),
            FixedU64(40_000_000),
        ]),
        attack_cost_hat: 90_000_000,
        in_cap_prize: 30_000_000,
    }
}

fn position(who: AccountId) -> PositionView {
    PositionView {
        position: PositionId::Proposal {
            proposal: 11,
            branch: Branch::Accept,
            kind: PositionKind::Long,
        },
        balance: u128::from(who[0]),
        vault_state: VaultState::Resolved(Branch::Accept),
    }
}

fn queued_execution() -> QueuedExecutionView {
    QueuedExecutionView {
        pid: 11,
        class: ProposalClass::Code,
        payload_hash: [3; 32],
        maturity: 500,
        grace_end: 600,
        version_constraint: RuntimeVersionConstraint {
            spec_name: b"bleavit"
                .to_vec()
                .try_into()
                .expect("spec name is under 32 bytes"),
            spec_version: 12,
        },
        cancelled: false,
        ratification: RatificationStatus::Passed { referendum: 9 },
        meters_clear: true,
    }
}

fn welfare() -> WelfareView {
    WelfareView {
        epoch: 7,
        spec_version: 3,
        s_pillar_1e9: FixedU64(800_000_000),
        c_onchain_1e9: FixedU64(810_000_000),
        c_attested_1e9: FixedU64(820_000_000),
        p_pillar_1e9: FixedU64(830_000_000),
        a_pillar_1e9: FixedU64(840_000_000),
        gate_s_1e9: FixedU64(200_000_000),
        gate_c_1e9: FixedU64(210_000_000),
        w_current_1e9: FixedU64(825_000_000),
        s_breached: false,
        c_breached: true,
        reserve_flag: false,
    }
}

fn param(key: ParamKey) -> ParamView {
    ParamView {
        key,
        value: 50,
        min: 10,
        max: 100,
        max_delta: 5,
        cooldown_blocks: 20,
        last_change: 300,
        class: ProposalClass::Param,
    }
}

fn nav() -> NavView {
    NavView {
        total: 1_000,
        main: 200,
        pol: 150,
        insurance: 100,
        keeper: 90,
        oracle: 80,
        rewards: 70,
        stream_remainders: 60,
        obligations: 50,
        haircut_flag: true,
        spendable_nav: 0,
        meter_utilization_bps: 7_500,
        class_floors: [10, 20, 30, 40],
    }
}

fn cohort() -> CohortSummaryView {
    CohortSummaryView {
        epoch: 6,
        s_1e9: FixedU64(700_000_000),
        baseline_twap_1e9: FixedU64(650_000_000),
        proposals: singleton((11, ProposalClass::Treasury, DecisionOutcome::Adopt)),
        voided: false,
        settled_at: 700,
    }
}

fn oracle_round() -> OracleRoundView {
    OracleRoundView {
        component: 4,
        epoch: 7,
        spec_version: 3,
        round: 2,
        reporter: [5; 32],
        value_1e9: FixedU64(750_000_000),
        evidence_hash: [6; 32],
        bond: 10_000_000,
        challenge_deadline: 800,
        acked_by_watchtowers: 2,
        escalated: true,
    }
}

#[test]
fn all_methods_are_callable_through_api_ref() {
    let api: sp_api::ApiRef<'_, MockApi> = MockApi.into();
    let at = <Block as BlockT>::Hash::default();

    assert_eq!(
        api.epoch_status(at).expect("epoch status call succeeds"),
        epoch_status()
    );
    assert_eq!(
        api.proposal_summaries(at)
            .expect("proposal summaries call succeeds"),
        singleton(proposal_summary())
    );
    assert_eq!(
        api.quote(at, 21, TradeSide::BuyLong, 3_000_000)
            .expect("quote call succeeds"),
        quote(21, TradeSide::BuyLong, 3_000_000)
    );
    assert_eq!(
        api.decision_stats(at, 11)
            .expect("decision stats call succeeds"),
        Some(decision_stats(11))
    );
    assert_eq!(
        api.account_positions(at, [7; 32])
            .expect("account positions call succeeds"),
        singleton(position([7; 32]))
    );
    assert_eq!(
        api.execution_queue(at)
            .expect("execution queue call succeeds"),
        singleton(queued_execution())
    );
    assert_eq!(
        api.welfare_current(at).expect("welfare call succeeds"),
        welfare()
    );

    let keys = singleton([8; 16]);
    assert_eq!(
        api.params(at, keys).expect("params call succeeds"),
        singleton(param([8; 16]))
    );
    assert_eq!(api.nav(at).expect("nav call succeeds"), nav());
    assert_eq!(
        api.recent_cohorts(at)
            .expect("recent cohorts call succeeds"),
        singleton(cohort())
    );
    assert_eq!(
        api.open_oracle_rounds(at)
            .expect("open oracle rounds call succeeds"),
        singleton(oracle_round())
    );
}

#[test]
fn runtime_api_id_and_version_are_frozen() {
    assert_eq!(
        runtime_decl_for_futarchy_api::ID,
        [52, 172, 53, 103, 236, 227, 15, 254]
    );
    assert_eq!(runtime_decl_for_futarchy_api::VERSION, 1);
}

#[test]
fn api_collection_bounds_match_contract() {
    // Expected figures are the frozen table in 02 §3; unlike runtime code, test
    // literals intentionally quote that independent contract oracle.
    assert_eq!(
        BoundedVec::<ProposalSummaryView, { bounds::MAX_PROPOSAL_SUMMARIES }>::BOUND,
        32
    );
    assert_eq!(
        BoundedVec::<PositionView, { bounds::MAX_ACCOUNT_POSITIONS }>::BOUND,
        64
    );
    assert_eq!(
        BoundedVec::<QueuedExecutionView, { MAX_QUEUED_EXECUTIONS }>::BOUND,
        32
    );
    assert_eq!(
        BoundedVec::<ParamView, { bounds::MAX_PARAM_KEYS }>::BOUND,
        64
    );
    assert_eq!(
        BoundedVec::<CohortSummaryView, { bounds::RECENT_COHORT_SUMMARIES }>::BOUND,
        32
    );
    assert_eq!(bounds::MAX_COHORT_PROPOSALS, 12);
    assert_eq!(CohortSummaryView::max_encoded_len(), 158);
    assert_eq!(
        BoundedVec::<OracleRoundView, { bounds::MAX_OPEN_ORACLE_ROUNDS }>::BOUND,
        192
    );
}

fn assert_encodes_like_frame_bounded_vec<T, const N: u32>(value: &BoundedVec<T, N>)
where
    T: Encode + Decode + PartialEq + Debug + Clone,
{
    let frame: bounded_collections::BoundedVec<T, bounded_collections::ConstU32<N>> = value
        .as_slice()
        .to_vec()
        .try_into()
        .expect("the FRAME bound equals the primitives bound");
    assert_eq!(value.encode(), frame.encode());
    let ours = BoundedVec::<T, N>::decode(&mut &frame.encode()[..])
        .expect("primitives BoundedVec decodes the FRAME encoding");
    assert_eq!(&ours, value);
    let theirs = bounded_collections::BoundedVec::<T, bounded_collections::ConstU32<N>>::decode(
        &mut &value.encode()[..],
    )
    .expect("FRAME BoundedVec decodes the primitives encoding");
    assert_eq!(theirs.into_inner(), value.as_slice().to_vec());
}

#[test]
fn api_collections_encode_identically_to_frame_bounded_vec() {
    // 02 §3 spells the API collection bounds as FRAME's `BoundedVec<T, ConstU32<N>>`,
    // while the declaration uses the frame-free primitives `BoundedVec<T, const N>`
    // (rule 9 / 01 §5.2 — the view crate cannot take sp types; SQ-99). This pins the
    // two spellings to one wire encoding, cross-decoded in both directions, for every
    // collection on the 02 §3 surface.
    assert_encodes_like_frame_bounded_vec(&singleton::<_, { bounds::MAX_PROPOSAL_SUMMARIES }>(
        proposal_summary(),
    ));
    assert_encodes_like_frame_bounded_vec(&singleton::<_, { bounds::MAX_ACCOUNT_POSITIONS }>(
        position([7; 32]),
    ));
    assert_encodes_like_frame_bounded_vec(&singleton::<_, { MAX_QUEUED_EXECUTIONS }>(
        queued_execution(),
    ));
    assert_encodes_like_frame_bounded_vec(&singleton::<ParamKey, { bounds::MAX_PARAM_KEYS }>(
        [8; 16],
    ));
    assert_encodes_like_frame_bounded_vec(&singleton::<_, { bounds::MAX_PARAM_KEYS }>(param(
        [8; 16],
    )));
    assert_encodes_like_frame_bounded_vec(&singleton::<_, { bounds::RECENT_COHORT_SUMMARIES }>(
        cohort(),
    ));
    assert_encodes_like_frame_bounded_vec(&singleton::<_, { bounds::MAX_OPEN_ORACLE_ROUNDS }>(
        oracle_round(),
    ));
}

fn assert_scale_round_trip<T>(value: T)
where
    T: Encode + Decode + PartialEq + Debug,
{
    let encoded = value.encode();
    let decoded = T::decode(&mut &encoded[..]).expect("API view SCALE decoding succeeds");
    assert_eq!(decoded, value);
}

#[test]
fn every_populated_view_round_trips_across_api_boundary() {
    assert_scale_round_trip(epoch_status());
    assert_scale_round_trip(proposal_summary());
    assert_scale_round_trip(quote(21, TradeSide::BuyLong, 3_000_000));
    assert_scale_round_trip(decision_stats(11));
    assert_scale_round_trip(position([7; 32]));
    assert_scale_round_trip(queued_execution());
    assert_scale_round_trip(welfare());
    assert_scale_round_trip(param([8; 16]));
    assert_scale_round_trip(nav());
    assert_scale_round_trip(cohort());
    assert_scale_round_trip(oracle_round());
}
