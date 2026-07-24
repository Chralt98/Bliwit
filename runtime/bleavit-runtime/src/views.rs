//! Read-only assembly for the contract-v6 `FutarchyApi` surface (02 §3-§4).

use alloc::vec::Vec;

use frame_support::traits::{fungibles::Inspect, Get};
use futarchy_primitives::{
    bounds, AccountId as ViewAccountId, Balance, BoundedVec, CohortSummaryView, DecisionStatsView,
    EpochStatusView, FixedU64, MarketId, NavView, OracleRoundView, ParamKey, ParamView,
    PositionView, ProposalClass, ProposalId, ProposalSummaryView, QueuedExecutionView, QuoteView,
    RatificationStatus, TradeSide, VaultState, WelfareView,
};

use crate::{usdc_location, AccountId, ForeignAssets, Runtime};

/// Assemble `FutarchyApi::epoch_status` per 02 §3/§4. The epoch pallet's
/// narrow reader touches only the clock, schedule and live provider flags;
/// bounded proposal/cohort collections are not part of this seven-field view.
pub fn epoch_status() -> EpochStatusView {
    pallet_epoch::Pallet::<Runtime>::status_view()
}

/// Assemble `FutarchyApi::proposal_summaries` per 02 §3/§4/§7.1.
/// `Proposals` is bounded by `MaxLiveProposals`; explicit id sorting removes
/// storage-hasher iteration order from the API contract.
///
/// Ratification mirrors the execution guard's own projection byte-for-byte
/// (`execution_guard_core::Guard::view`, the source of `execution_queue`'s
/// field) so the two API surfaces can never contradict each other for one
/// proposal: `Ratifications` is written only by the `RatifyOrigin`-gated
/// `ratify` call, so a present record is `Passed`; a class that needs no
/// values ratification is `NotRequired`; and a class that requires it with no
/// record on chain is `NoPassedRecord`. That spelling deliberately makes no
/// claim about a referendum lifecycle that the guard cannot observe.
pub fn proposal_summaries() -> BoundedVec<ProposalSummaryView, { bounds::MAX_PROPOSAL_SUMMARIES }> {
    let mut proposals = pallet_epoch::Proposals::<Runtime>::iter_values().collect::<Vec<_>>();
    proposals.sort_unstable_by_key(|proposal| proposal.id);
    let mut out = BoundedVec::new();
    for proposal in proposals {
        let (decision_market, gate_markets) = proposal.markets.map_or((None, None), |markets| {
            (Some((markets.accept, markets.reject)), markets.gates)
        });
        let ratification = match pallet_execution_guard::Ratifications::<Runtime>::get(proposal.id)
        {
            Some(record) => RatificationStatus::Passed {
                referendum: record.referendum_index,
            },
            None if !pallet_execution_guard::requires_ratification(proposal.class) => {
                RatificationStatus::NotRequired
            }
            None => RatificationStatus::NoPassedRecord,
        };
        if out
            .try_push(ProposalSummaryView {
                id: proposal.id,
                class: proposal.class,
                state: proposal.state,
                proposer: proposal.proposer.into(),
                epoch: proposal.epoch,
                payload_hash: proposal.payload_hash,
                ask: proposal.ask,
                decision_market,
                gate_markets,
                decide_at: proposal.decide_at,
                maturity: proposal.maturity,
                ratification,
            })
            .is_err()
        {
            break;
        }
    }
    out
}

/// Assemble `FutarchyApi::decision_stats` from the exact snapshot shared with
/// `pallet_epoch::decide` (02 §3/§4; 05 §5.2-§5.6; 08 §5.2-§5.3).
/// Any unavailable read returns `None`; the view never exposes the crank's
/// internal fail-closed zero sentinels as observed market data.
pub fn decision_stats(pid: ProposalId) -> Option<DecisionStatsView> {
    let snapshot = pallet_epoch::Pallet::<Runtime>::decision_input_snapshot(pid)?;
    if !snapshot.backing_complete {
        return None;
    }
    let proposal = &snapshot.proposal;
    let input = &snapshot.inputs;
    let (baseline_full, _) = pallet_epoch::effective_baseline_twaps(input)?;
    let market_stats = crate::configs::decision_market_stats_for_view(proposal, &snapshot.params)?;

    // 05 §5.4: the same exported epoch-core helpers used by decide() own
    // saturating Baseline-σ arithmetic and close-spot convergence.
    let r_eff = pallet_epoch::effective_reject_1e9(
        input.reject_full,
        baseline_full,
        snapshot.params.class_sigma(proposal.class),
    );
    let converged = pallet_epoch::decision_converged(input, snapshot.params.delta_max);

    // D-4 (05 §5.6; 08 §5.2, SQ-231): measured_depth already combines the
    // pair's rounded-down POL depth with its 04 §7a contest capital under the
    // sec.flow_cap ceiling. `None` published flow is the normative L/2
    // fallback, not missing backing.
    let attack_cost_hat = pallet_epoch::attack_cost_hat(
        input.measured_depth,
        input.published_flow_per_day,
        snapshot.params.decision_window,
    )
    .ok()?;
    let in_cap_prize = input.in_cap_prize?;

    Some(DecisionStatsView {
        pid,
        twap_accept_1e9: input.accept_full,
        twap_reject_1e9: input.reject_full,
        // 05 §5.3 carry is the effective Baseline decide() compares, so the
        // public statistic cannot display a different, invalid live book.
        twap_baseline_1e9: baseline_full,
        r_eff_1e9: FixedU64(r_eff),
        trailing_accept_1e9: input.accept_trailing,
        trailing_reject_1e9: input.reject_trailing,
        coverage_pct: market_stats.coverage_pct,
        traded_volume: market_stats.traded_volume,
        v_min_required: market_stats.v_min_required,
        converged,
        gate_twaps_1e9: input.gate_twaps,
        attack_cost_hat,
        in_cap_prize,
    })
}

/// Assemble `FutarchyApi::recent_cohorts` per 02 §4/§7.1. The stored
/// `CohortSummary` is the view type, including FIFO ring order.
pub fn recent_cohorts() -> BoundedVec<CohortSummaryView, { bounds::RECENT_COHORT_SUMMARIES }> {
    let mut out = BoundedVec::new();
    for summary in pallet_epoch::RecentCohortSummaries::<Runtime>::get() {
        if out.try_push(summary).is_err() {
            break;
        }
    }
    out
}

fn quote_sentinel(max_trade: Balance) -> QuoteView {
    QuoteView {
        cost: 0,
        fee: 0,
        p_after_1e9: FixedU64(0),
        max_trade,
        within_domain: false,
        evaluable: false,
    }
}

/// Assemble `FutarchyApi::quote` per 02 §3/§4 and 04 §4/§6.
/// Missing, trade-inadmissible, overflowing, or inventory-invalid books use
/// the G-1 zero sentinel with `evaluable = false`; an existing book retains
/// its real `b/4` maximum. Successfully computed out-of-domain quotes retain
/// their real values with `evaluable = true` and `within_domain = false`. The
/// pallet-owned preflight is the same predicate called by `buy`/`sell`,
/// including registered-window expiry (04 §6.4).
pub fn quote(market: MarketId, side: TradeSide, amount: Balance) -> QuoteView {
    let Some(book) = pallet_market::Markets::<Runtime>::get(market) else {
        return quote_sentinel(0);
    };
    let max_trade = pallet_market::core_market::max_trade_amount(book.b);
    if pallet_market::Pallet::<Runtime>::ensure_trade_admissible(market, &book).is_err() {
        return quote_sentinel(max_trade);
    }
    match pallet_market::core_market::quote(
        &book,
        side,
        amount,
        <Runtime as pallet_market::Config>::Fee::get(),
    ) {
        Ok(view) => view,
        Err(_) => quote_sentinel(max_trade),
    }
}

fn push_position(
    out: &mut BoundedVec<PositionView, { bounds::MAX_ACCOUNT_POSITIONS }>,
    who: &AccountId,
    position: futarchy_primitives::PositionId,
    vault_state: VaultState,
) -> bool {
    let balance = pallet_conditional_ledger::Positions::<Runtime>::get(position, who);
    balance == 0
        || out
            .try_push(PositionView {
                position,
                balance,
                vault_state,
            })
            .is_ok()
}

/// Assemble `FutarchyApi::account_positions` per 02 §3/§7.4 and 03 §2.
/// Proposal vaults sort by proposal id and precede Baseline vaults sorted by
/// epoch; each vault uses conditional-ledger-core's canonical instrument order.
/// Truncation at 64 is deterministic. User accounts cannot exceed 64, while
/// 13 §4 explicitly exempts protocol accounts, so only those can be truncated.
pub fn account_positions(
    who: ViewAccountId,
) -> BoundedVec<PositionView, { bounds::MAX_ACCOUNT_POSITIONS }> {
    let who = AccountId::new(who);
    let mut out = BoundedVec::new();
    let mut proposals =
        pallet_conditional_ledger::Vaults::<Runtime>::iter_keys().collect::<Vec<_>>();
    proposals.sort_unstable();
    for proposal in proposals {
        let Some(vault) = pallet_conditional_ledger::Vaults::<Runtime>::get(proposal) else {
            continue;
        };
        for position in pallet_conditional_ledger::core_ledger::proposal_positions(proposal) {
            if !push_position(&mut out, &who, position, vault.state) {
                return out;
            }
        }
    }

    let mut baselines =
        pallet_conditional_ledger::BaselineVaults::<Runtime>::iter_keys().collect::<Vec<_>>();
    baselines.sort_unstable();
    for epoch in baselines {
        let Some(vault) = pallet_conditional_ledger::BaselineVaults::<Runtime>::get(epoch) else {
            continue;
        };
        // Contract v6 gives Baseline settlement a branch-free projection: a
        // Baseline instrument has no winning proposal branch to publish.
        let state = match vault.state {
            pallet_conditional_ledger::core_ledger::BaselineState::Open => VaultState::Open,
            pallet_conditional_ledger::core_ledger::BaselineState::Settled(s) => {
                VaultState::BaselineSettled { s }
            }
        };
        for position in pallet_conditional_ledger::core_ledger::baseline_positions(epoch) {
            if !push_position(&mut out, &who, position, state) {
                return out;
            }
        }
    }
    out
}

/// Assemble `FutarchyApi::execution_queue` per 02 §3/§4 and 09 §1.
/// The pallet accessor single-homes queue ordering, ratification, and blocked-
/// meter semantics; defensive truncation retains the first 32 proposal ids.
pub fn execution_queue() -> BoundedVec<QueuedExecutionView, { bounds::MAX_LIVE_PROPOSALS }> {
    let mut out = BoundedVec::new();
    for view in pallet_execution_guard::Pallet::<Runtime>::queue_view() {
        if out.try_push(view).is_err() {
            break;
        }
    }
    out
}

fn welfare_sentinel(
    epoch: u32,
    spec_version: u16,
    reserve_flag: bool,
    active_spec_available: bool,
) -> WelfareView {
    WelfareView {
        epoch,
        spec_version,
        s_pillar_1e9: FixedU64(0),
        c_onchain_1e9: FixedU64(0),
        c_attested_1e9: FixedU64(0),
        p_pillar_1e9: FixedU64(0),
        a_pillar_1e9: FixedU64(0),
        gate_s_1e9: FixedU64(0),
        gate_c_1e9: FixedU64(0),
        w_current_1e9: FixedU64(0),
        s_breached: false,
        c_breached: false,
        reserve_flag,
        active_spec_available,
    }
}

/// Assemble `FutarchyApi::welfare_current` per 02 §3/§4 and 05 §4.6/§4.7.
/// Qualification and this view share the constitution's canonical active-spec
/// selector, including its fail-closed `None` on a latest-activation tie. Per
/// 02 §3 and 05 §4.6, two surfaces must never name different active specs.
/// `active_spec_available` distinguishes no active spec from the legal active
/// version zero. The latest finalized snapshot for that spec is selected by
/// a deterministic O(`MAX_SNAPSHOTS`) scan: production rejects snapshots for
/// `epoch >= CurrentEpoch`, and `WelfareView.epoch` names the closed epoch whose
/// pillars and gate flags are returned. A missing finalized snapshot keeps the
/// uniquely selected version in the sentinel. Its false breach flags are
/// welfare-core's pre-existing absent-epoch default (SQ-79), not an assertion
/// that no breach exists. Oracle reserve health is authoritative; constitution
/// bit 7 is only its 02 §7.3 mirror.
pub fn welfare_current() -> WelfareView {
    let current_epoch = <Runtime as pallet_welfare::Config>::CurrentEpoch::get();
    let reserve_flag = pallet_oracle::Pallet::<Runtime>::reserve_unhealthy();
    let Some(spec_version) =
        <<Runtime as pallet_epoch::Config>::Constitution as pallet_epoch::ConstitutionAccess<
            AccountId,
        >>::active_metric_spec_version()
    else {
        return welfare_sentinel(current_epoch, 0, reserve_flag, false);
    };
    let Some(latest_finalized_epoch) = pallet_welfare::Snapshots::<Runtime>::iter_keys()
        .filter_map(|(epoch, version)| {
            (version == spec_version && epoch < current_epoch).then_some(epoch)
        })
        .max()
    else {
        return welfare_sentinel(current_epoch, spec_version, reserve_flag, true);
    };
    match pallet_welfare::Pallet::<Runtime>::welfare_state().current_view(
        latest_finalized_epoch,
        spec_version,
        reserve_flag,
    ) {
        Ok(view) => view,
        Err(_) => welfare_sentinel(current_epoch, spec_version, reserve_flag, true),
    }
}

/// Assemble `FutarchyApi::params` per 02 §3/§4 and 13 reading rule 7.
/// Unknown keys are skipped and found keys retain input order (including
/// duplicates). `max_delta` is the conservative symmetric projection, while
/// `min_next`/`max_next` are the exact inclusive interval single-homed in
/// `ParamRecord::admissible_next_interval`. Malformed records are skipped
/// rather than presented as unbounded. Cooldowns use the live `epoch.length`,
/// saturating at `u32::MAX`.
pub fn params(
    keys: BoundedVec<ParamKey, { bounds::MAX_PARAM_KEYS }>,
) -> BoundedVec<ParamView, { bounds::MAX_PARAM_KEYS }> {
    let epoch_length =
        pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(b"epoch.length"))
            .and_then(|record| match record.value {
                pallet_constitution::ParamValue::U32(value) => Some(value),
                _ => None,
            });
    let mut out = BoundedVec::new();
    for key in keys {
        let Some(record) = pallet_constitution::Params::<Runtime>::get(key) else {
            continue;
        };
        let Ok(max_delta) = record.max_delta_allowance() else {
            continue;
        };
        let Ok((min_next, max_next)) = record.admissible_next_interval() else {
            continue;
        };
        let cooldown_blocks = if record.cooldown_epochs == 0 {
            0
        } else {
            match epoch_length {
                Some(length) => record.cooldown_epochs.saturating_mul(length),
                None => u32::MAX,
            }
        };
        if out
            .try_push(ParamView {
                key,
                value: record.value.as_u128(),
                min: record.min.as_u128(),
                max: record.max.as_u128(),
                max_delta,
                cooldown_blocks,
                last_change: record.last_change_block,
                class: record.class.as_proposal_class(),
                min_next,
                max_next,
            })
            .is_err()
        {
            break;
        }
    }
    out
}

/// Assemble contract-v6 `FutarchyApi::nav` per 02 §3/§4 and 08
/// §1.1/§1.2/§4.1. POL includes both proposal and dedicated Baseline
/// lines. Insurance comes from the actual INSURANCE USDC custody account.
pub fn nav() -> NavView {
    let components = pallet_futarchy_treasury::Pallet::<Runtime>::nav();
    let treasury = pallet_futarchy_treasury::Pallet::<Runtime>::treasury();
    let proposal_pol = pallet_futarchy_treasury::Pallet::<Runtime>::line_balance(
        pallet_futarchy_treasury::BudgetLine::Pol,
    );
    let baseline_pol = pallet_futarchy_treasury::Pallet::<Runtime>::line_balance(
        pallet_futarchy_treasury::BudgetLine::PolBaseline,
    );
    // SQ-101: the instance is keyed by the frozen 02 §8 XCM Location, not a u32.
    let insurance = <ForeignAssets as Inspect<AccountId>>::balance(
        usdc_location(),
        &crate::configs::insurance_account(),
    );
    NavView {
        total: components.nav,
        main: treasury.main_usdc,
        pol: proposal_pol.saturating_add(baseline_pol),
        insurance,
        keeper: pallet_futarchy_treasury::Pallet::<Runtime>::line_balance(
            pallet_futarchy_treasury::BudgetLine::Keeper,
        ),
        oracle: pallet_futarchy_treasury::Pallet::<Runtime>::line_balance(
            pallet_futarchy_treasury::BudgetLine::Oracle,
        ),
        rewards: pallet_futarchy_treasury::Pallet::<Runtime>::line_balance(
            pallet_futarchy_treasury::BudgetLine::Rewards,
        ),
        stream_remainders: treasury.open_stream_remainders(),
        obligations: treasury.obligations(),
        haircut_flag: components.reserve_impaired,
        spendable_nav: components.spendable_nav,
        meter_utilization_bps: components.meter_utilization_bps,
        class_floors: [
            pallet_futarchy_treasury::Pallet::<Runtime>::floor(ProposalClass::Param),
            pallet_futarchy_treasury::Pallet::<Runtime>::floor(ProposalClass::Treasury),
            pallet_futarchy_treasury::Pallet::<Runtime>::floor(ProposalClass::Code),
            pallet_futarchy_treasury::Pallet::<Runtime>::floor(ProposalClass::Meta),
        ],
    }
}

/// Assemble `FutarchyApi::open_oracle_rounds` per 02 §3/§4/§7.2 and
/// 07 §5. `escalated` means a prior round advanced the game (`round > 1`):
/// `challenger.is_some()` is only a live challenge in the current round and is
/// cleared when escalation occurs. Results sort by the frozen triple key.
pub fn open_oracle_rounds() -> BoundedVec<OracleRoundView, { bounds::MAX_OPEN_ORACLE_ROUNDS }> {
    let mut rounds = pallet_oracle::Rounds::<Runtime>::iter_values().collect::<Vec<_>>();
    rounds.sort_unstable_by_key(|round| (round.component, round.epoch, round.spec_version));
    let mut out = BoundedVec::new();
    for round in rounds {
        if out
            .try_push(OracleRoundView {
                component: round.component,
                epoch: round.epoch,
                spec_version: round.spec_version,
                round: round.round,
                reporter: round.reporter,
                value_1e9: round.value,
                evidence_hash: round.evidence_hash,
                bond: round.bond,
                challenge_deadline: round.challenge_deadline,
                acked_by_watchtowers: round.acks,
                escalated: round.round > 1,
            })
            .is_err()
        {
            break;
        }
    }
    out
}
