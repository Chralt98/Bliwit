use std::collections::BTreeMap;

use subxt::ext::scale_value::{Composite, Value};

use crate::{
    config::{Role, RoleSet},
    snapshot::{
        ChainSnapshot, RegistryEpochSnapshot, DEFAULT_DECISION_WINDOW_BLOCKS,
        DEFAULT_OBSERVATION_INTERVAL_BLOCKS, DEFAULT_RESERVE_PROBE_INTERVAL_BLOCKS,
        DEFAULT_RESERVE_PROBE_TIMEOUT_BLOCKS, DEFAULT_TICK_BATCH,
    },
};

const SETTLE_BATCH: u64 = 100;
const ORACLE_CLOSE_BATCH: u64 = 10;
const REGISTRY_CLOSE_BATCH: usize = 20;
const EXECUTION_RETRY_WINDOW_BLOCKS: u64 = 43_200;

pub const PRIORITY_TICK: u16 = 1_000;
pub const PRIORITY_STALE_DECISION_OBSERVE: u16 = 975;
pub const PRIORITY_DECISION_OBSERVE: u16 = 950;
pub const PRIORITY_DECIDE: u16 = 925;
pub const PRIORITY_ORACLE_CLOSE: u16 = 900;
pub const PRIORITY_SETTLE: u16 = 875;
pub const PRIORITY_WELFARE: u16 = 850;
pub const PRIORITY_EXECUTE: u16 = 700;
pub const PRIORITY_REGISTRY_CLOSE: u16 = 650;
pub const PRIORITY_RENEWAL: u16 = 600;
pub const PRIORITY_OBSERVE: u16 = 500;
pub const PRIORITY_CLEANUP: u16 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCrank {
    pub role: Role,
    pub pallet: &'static str,
    pub call: &'static str,
    pub args: Composite<()>,
    pub priority: u16,
}

impl PlannedCrank {
    pub fn cooldown_key(&self) -> String {
        format!(
            "{}:{}:{}:{:?}",
            self.role, self.pallet, self.call, self.args
        )
    }
}

#[derive(Clone, Debug)]
pub struct PlannerConfig {
    pub enabled_roles: RoleSet,
    pub obs_interval: u64,
    pub decision_window: u64,
    pub reserve_probe_interval: u64,
    pub reserve_probe_timeout: u64,
    pub cooldown_depth: u64,
    pub cooldowns: BTreeMap<String, u64>,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            enabled_roles: Role::ALL.into_iter().collect(),
            obs_interval: DEFAULT_OBSERVATION_INTERVAL_BLOCKS,
            decision_window: DEFAULT_DECISION_WINDOW_BLOCKS,
            reserve_probe_interval: DEFAULT_RESERVE_PROBE_INTERVAL_BLOCKS,
            reserve_probe_timeout: DEFAULT_RESERVE_PROBE_TIMEOUT_BLOCKS,
            cooldown_depth: 3,
            cooldowns: BTreeMap::new(),
        }
    }
}

pub fn plan(snapshot: &ChainSnapshot, config: &PlannerConfig) -> Vec<PlannedCrank> {
    let mut cranks = Vec::new();
    plan_tick(snapshot, config, &mut cranks);
    plan_observations(snapshot, config, &mut cranks);
    plan_decisions(snapshot, config, &mut cranks);
    plan_oracle(snapshot, config, &mut cranks);
    plan_settlement(snapshot, config, &mut cranks);
    plan_welfare(snapshot, config, &mut cranks);
    plan_execution(snapshot, config, &mut cranks);
    plan_registries(snapshot, config, &mut cranks);
    plan_renewals(snapshot, config, &mut cranks);
    plan_cleanup(snapshot, config, &mut cranks);

    cranks.retain(|crank| !cooling_down(snapshot.current_block, crank, config));
    cranks.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.role.cmp(&right.role))
            .then_with(|| left.pallet.cmp(right.pallet))
            .then_with(|| left.call.cmp(right.call))
            .then_with(|| left.cooldown_key().cmp(&right.cooldown_key()))
    });
    cranks
}

fn plan_tick(snapshot: &ChainSnapshot, config: &PlannerConfig, cranks: &mut Vec<PlannedCrank>) {
    if !enabled(config, Role::Tick) || !snapshot.has_call("Epoch", "tick") {
        return;
    }
    let Some(epoch) = &snapshot.epoch else {
        return;
    };
    let boundary_due = epoch
        .next_boundary
        .is_some_and(|boundary| snapshot.current_block >= boundary);
    let mut pids = snapshot
        .proposals
        .iter()
        .filter(|proposal| {
            matches!(
                (epoch.phase.as_str(), proposal.state.as_str()),
                ("Qualify", "Submitted") | ("Seed", "Qualified") | ("Seed", "Rerun")
            ) || (proposal.state == "Queued"
                && proposal
                    .grace_end
                    .is_some_and(|end| snapshot.current_block > end))
        })
        .map(|proposal| proposal.proposal_id)
        .collect::<Vec<_>>();
    if boundary_due {
        let transition_state = match epoch.phase.as_str() {
            "Intake" => Some("Submitted"),
            "Qualify" => Some("Qualified"),
            _ => None,
        };
        if let Some(state) = transition_state {
            pids.extend(
                snapshot
                    .proposals
                    .iter()
                    .filter(|proposal| proposal.state == state)
                    .map(|proposal| proposal.proposal_id),
            );
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids.truncate(snapshot.tick_batch.unwrap_or(DEFAULT_TICK_BATCH));
    if boundary_due || !pids.is_empty() {
        cranks.push(crank(
            Role::Tick,
            "Epoch",
            "tick",
            [("pids", sequence(pids))],
            PRIORITY_TICK,
        ));
    }
}

fn plan_observations(
    snapshot: &ChainSnapshot,
    config: &PlannerConfig,
    cranks: &mut Vec<PlannedCrank>,
) {
    if !enabled(config, Role::Observe) || !snapshot.has_call("Market", "crank_observe") {
        return;
    }
    for book in &snapshot.books {
        if !matches!(book.phase.as_str(), "Trading" | "Extended") {
            continue;
        }
        let Some(last) = book.last_observed_block else {
            continue;
        };
        if snapshot.current_block < last.saturating_add(config.obs_interval) {
            continue;
        }
        cranks.push(crank(
            Role::Observe,
            "Market",
            "crank_observe",
            [("market", number(book.market_id))],
            if book.stale_in_decision_window {
                PRIORITY_STALE_DECISION_OBSERVE
            } else if book.decision_window {
                PRIORITY_DECISION_OBSERVE
            } else {
                PRIORITY_OBSERVE
            },
        ));
    }
}

fn plan_decisions(
    snapshot: &ChainSnapshot,
    config: &PlannerConfig,
    cranks: &mut Vec<PlannedCrank>,
) {
    if !enabled(config, Role::Decide) || !snapshot.has_call("Epoch", "decide") {
        return;
    }
    for proposal in &snapshot.proposals {
        if matches!(proposal.state.as_str(), "Trading" | "Extended")
            && proposal
                .decide_at
                .is_some_and(|at| snapshot.current_block >= at)
        {
            cranks.push(crank(
                Role::Decide,
                "Epoch",
                "decide",
                [("pid", number(proposal.proposal_id))],
                PRIORITY_DECIDE,
            ));
        }
    }
}

fn plan_oracle(snapshot: &ChainSnapshot, config: &PlannerConfig, cranks: &mut Vec<PlannedCrank>) {
    if !enabled(config, Role::OracleClose) {
        return;
    }
    let due_rounds = snapshot
        .oracle_rounds
        .iter()
        .filter(|round| {
            round
                .deadline
                .is_some_and(|deadline| deadline < snapshot.current_block)
        })
        .count();
    if due_rounds > 0 && snapshot.has_call("Oracle", "crank_round_close") {
        cranks.push(crank(
            Role::OracleClose,
            "Oracle",
            "crank_round_close",
            [("batch", number(ORACLE_CLOSE_BATCH.min(due_rounds as u64)))],
            PRIORITY_ORACLE_CLOSE,
        ));
    }
    if snapshot.has_call("Oracle", "crank_reserve_probe")
        && snapshot.reserve_health.as_ref().is_some_and(|health| {
            let interval_due = health.last_probe_at.is_some_and(|last| {
                snapshot.current_block >= last.saturating_add(config.reserve_probe_interval)
            });
            let timeout_due = health.pending_since.is_some_and(|since| {
                snapshot.current_block >= since.saturating_add(config.reserve_probe_timeout)
            });
            interval_due || timeout_due
        })
    {
        cranks.push(crank(
            Role::OracleClose,
            "Oracle",
            "crank_reserve_probe",
            std::iter::empty::<(&str, Value<()>)>(),
            PRIORITY_ORACLE_CLOSE,
        ));
    }
}

fn plan_settlement(
    snapshot: &ChainSnapshot,
    config: &PlannerConfig,
    cranks: &mut Vec<PlannedCrank>,
) {
    if !enabled(config, Role::Settle) || !snapshot.has_call("Epoch", "settle_cohort") {
        return;
    }
    let Some(epoch) = &snapshot.epoch else {
        return;
    };
    if epoch.phase != "Housekeeping" {
        return;
    }
    for cohort in &snapshot.cohorts {
        let due = matches!(cohort.status.as_str(), "AwaitingOracle" | "Settling")
            || (cohort.status == "Measuring"
                && cohort
                    .until_epoch
                    .is_some_and(|until| epoch.index >= until.saturating_add(1)));
        if due {
            cranks.push(crank(
                Role::Settle,
                "Epoch",
                "settle_cohort",
                [
                    ("epoch", number(cohort.epoch)),
                    ("batch", number(SETTLE_BATCH)),
                ],
                PRIORITY_SETTLE,
            ));
        }
    }
}

fn plan_welfare(snapshot: &ChainSnapshot, config: &PlannerConfig, cranks: &mut Vec<PlannedCrank>) {
    if !enabled(config, Role::Welfare) {
        return;
    }
    let Some(welfare) = &snapshot.welfare else {
        return;
    };
    for (epoch, spec_version) in &welfare.snapshot_candidates {
        if !snapshot.has_call("Welfare", "record_snapshot") {
            break;
        }
        cranks.push(crank(
            Role::Welfare,
            "Welfare",
            "record_snapshot",
            [
                ("epoch", number(*epoch)),
                ("spec_version", number(*spec_version)),
            ],
            PRIORITY_WELFARE,
        ));
    }
    for (epoch, day, spec_version) in &welfare.daily_gate_candidates {
        if !snapshot.has_call("Welfare", "record_daily_gate") {
            break;
        }
        cranks.push(crank(
            Role::Welfare,
            "Welfare",
            "record_daily_gate",
            [
                ("epoch", number(*epoch)),
                ("day", number(u64::from(*day))),
                ("spec_version", number(*spec_version)),
            ],
            PRIORITY_WELFARE,
        ));
    }
}

fn plan_execution(
    snapshot: &ChainSnapshot,
    config: &PlannerConfig,
    cranks: &mut Vec<PlannedCrank>,
) {
    if !enabled(config, Role::Execute) {
        return;
    }
    for item in &snapshot.execution_queue {
        if item.cancelled || item.maturity.is_none_or(|at| snapshot.current_block < at) {
            continue;
        }
        let (call, due) = match item.failed_at {
            Some(failed_at)
                if snapshot.current_block
                    <= failed_at.saturating_add(EXECUTION_RETRY_WINDOW_BLOCKS) =>
            {
                ("execute", true)
            }
            Some(failed_at)
                if snapshot.current_block
                    > failed_at.saturating_add(EXECUTION_RETRY_WINDOW_BLOCKS) =>
            {
                ("expire_failed_execution", true)
            }
            None => (
                "execute",
                item.grace_end
                    .is_some_and(|end| snapshot.current_block <= end),
            ),
            Some(_) => ("execute", false),
        };
        if due && snapshot.has_call("ExecutionGuard", call) {
            cranks.push(crank(
                Role::Execute,
                "ExecutionGuard",
                call,
                [("pid", number(item.proposal_id))],
                PRIORITY_EXECUTE,
            ));
        }
    }
}

fn plan_registries(
    snapshot: &ChainSnapshot,
    config: &PlannerConfig,
    cranks: &mut Vec<PlannedCrank>,
) {
    if !enabled(config, Role::RegistryClose) {
        return;
    }
    for registry in &snapshot.registry_epochs {
        let Some(pallet) = registry_pallet(&registry.pallet) else {
            continue;
        };
        let due = registry
            .filings
            .iter()
            .filter(|filing| {
                filing.state == "Filed"
                    && filing
                        .deadline
                        .is_some_and(|deadline| deadline < snapshot.current_block)
            })
            .count();
        if due > 0 {
            if !snapshot.has_call(pallet, "crank_close") {
                continue;
            }
            cranks.push(crank(
                Role::RegistryClose,
                pallet,
                "crank_close",
                [
                    ("epoch", number(registry.epoch)),
                    ("batch", number(REGISTRY_CLOSE_BATCH.min(due) as u64)),
                ],
                PRIORITY_REGISTRY_CLOSE,
            ));
        }
        if registry_close_proven(snapshot, registry) && snapshot.has_call(pallet, "close_epoch") {
            cranks.push(crank(
                Role::RegistryClose,
                pallet,
                "close_epoch",
                [("epoch", number(registry.epoch))],
                PRIORITY_REGISTRY_CLOSE,
            ));
        }
    }
}

fn registry_close_proven(snapshot: &ChainSnapshot, registry: &RegistryEpochSnapshot) -> bool {
    let all_terminal = registry
        .filings
        .iter()
        .all(|filing| matches!(filing.state.as_str(), "Upheld" | "Rejected"));
    let filings_proven =
        !registry.filings.is_empty() && registry.filing_count_present && all_terminal;
    let reporting_epoch_passed = snapshot
        .epoch
        .as_ref()
        .is_some_and(|epoch| epoch.index > registry.epoch.saturating_add(1));
    !registry.aggregate_present
        && registry.closed_at.is_none()
        && filings_proven
        && reporting_epoch_passed
}

fn plan_renewals(snapshot: &ChainSnapshot, config: &PlannerConfig, cranks: &mut Vec<PlannedCrank>) {
    if !enabled(config, Role::Renewal) {
        return;
    }
    let Some(coretime) = &snapshot.coretime else {
        return;
    };
    let Some(ttl) = snapshot.live_params.coretime_quote_ttl else {
        return;
    };
    for quote in &coretime.quotes {
        let Some(age) = snapshot.current_block.checked_sub(quote.noted_at) else {
            continue;
        };
        if age > ttl {
            if snapshot.has_call("FutarchyTreasury", "prune_coretime_quote") {
                cranks.push(crank(
                    Role::Renewal,
                    "FutarchyTreasury",
                    "prune_coretime_quote",
                    [("period_index", number(u64::from(quote.period_index)))],
                    PRIORITY_RENEWAL,
                ));
            }
        } else if quote.price > 0
            && !coretime.funded_periods.contains(&quote.period_index)
            && snapshot.has_call("FutarchyTreasury", "execute_coretime_renewal")
        {
            cranks.push(crank(
                Role::Renewal,
                "FutarchyTreasury",
                "execute_coretime_renewal",
                [("period_index", number(u64::from(quote.period_index)))],
                PRIORITY_RENEWAL,
            ));
        }
    }
}

fn plan_cleanup(snapshot: &ChainSnapshot, config: &PlannerConfig, cranks: &mut Vec<PlannedCrank>) {
    if !enabled(config, Role::Cleanup) {
        return;
    }
    for candidate in &snapshot.market_reaps {
        if snapshot.has_call("Market", "reap")
            && reap_due(
                snapshot.current_block,
                candidate.terminal_at,
                candidate.archive_delay,
            )
        {
            cranks.push(crank(
                Role::Cleanup,
                "Market",
                "reap",
                [("market", number(candidate.id))],
                PRIORITY_CLEANUP,
            ));
        }
    }
    for candidate in &snapshot.proposal_dust {
        if snapshot.has_call("ConditionalLedger", "sweep_dust")
            && reap_due(
                snapshot.current_block,
                candidate.terminal_at,
                candidate.archive_delay,
            )
        {
            cranks.push(crank(
                Role::Cleanup,
                "ConditionalLedger",
                "sweep_dust",
                [("pid", number(candidate.id))],
                PRIORITY_CLEANUP,
            ));
        }
    }
    for candidate in &snapshot.baseline_dust {
        if snapshot.has_call("ConditionalLedger", "sweep_dust_baseline")
            && reap_due(
                snapshot.current_block,
                candidate.terminal_at,
                candidate.archive_delay,
            )
        {
            cranks.push(crank(
                Role::Cleanup,
                "ConditionalLedger",
                "sweep_dust_baseline",
                [("epoch", number(candidate.id))],
                PRIORITY_CLEANUP,
            ));
        }
    }
    for registry in &snapshot.registry_epochs {
        if reap_due(
            snapshot.current_block,
            registry.closed_at,
            registry.archive_delay,
        ) {
            if let Some(pallet) = registry_pallet(&registry.pallet) {
                if !snapshot.has_call(pallet, "reap_epoch") {
                    continue;
                }
                cranks.push(crank(
                    Role::Cleanup,
                    pallet,
                    "reap_epoch",
                    [("epoch", number(registry.epoch))],
                    PRIORITY_CLEANUP,
                ));
            }
        }
    }
}

fn reap_due(current: u64, terminal_at: Option<u64>, archive_delay: Option<u64>) -> bool {
    terminal_at
        .zip(archive_delay)
        .is_some_and(|(at, delay)| current >= at.saturating_add(delay))
}

fn enabled(config: &PlannerConfig, role: Role) -> bool {
    config.enabled_roles.contains(&role)
}

fn cooling_down(current_block: u64, crank: &PlannedCrank, config: &PlannerConfig) -> bool {
    config.cooldown_depth > 0
        && config
            .cooldowns
            .get(&crank.cooldown_key())
            .is_some_and(|last| current_block.saturating_sub(*last) < config.cooldown_depth)
}

fn registry_pallet(value: &str) -> Option<&'static str> {
    match value {
        "IncidentRegistry" => Some("IncidentRegistry"),
        "MilestoneRegistry" => Some("MilestoneRegistry"),
        _ => None,
    }
}

fn crank<I, S>(
    role: Role,
    pallet: &'static str,
    call: &'static str,
    args: I,
    priority: u16,
) -> PlannedCrank
where
    I: IntoIterator<Item = (S, Value<()>)>,
    S: Into<String>,
{
    PlannedCrank {
        role,
        pallet,
        call,
        args: Composite::named(args),
        priority,
    }
}

fn number(value: u64) -> Value<()> {
    Value::u128(u128::from(value))
}

fn sequence(values: Vec<u64>) -> Value<()> {
    Value::unnamed_composite(values.into_iter().map(number))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use subxt::ext::scale_value::ValueDef;

    use super::*;
    use crate::snapshot::{
        BookSnapshot, CohortSnapshot, CoretimeQuoteSnapshot, CoretimeSnapshot, EpochSnapshot,
        ExecutionSnapshot, OracleRoundSnapshot, ProposalSnapshot, ReapSnapshot,
        RegistryFilingSnapshot, ReserveHealthSnapshot, WelfareSnapshot,
    };

    fn snapshot() -> ChainSnapshot {
        ChainSnapshot {
            current_block: 1_000,
            available_pallets: [
                "Epoch",
                "Market",
                "Oracle",
                "IncidentRegistry",
                "ConditionalLedger",
                "ExecutionGuard",
                "FutarchyTreasury",
                "Welfare",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            available_calls: [
                "Epoch.tick",
                "Epoch.decide",
                "Epoch.settle_cohort",
                "Market.crank_observe",
                "Market.reap",
                "Oracle.crank_round_close",
                "Oracle.crank_reserve_probe",
                "IncidentRegistry.crank_close",
                "IncidentRegistry.close_epoch",
                "IncidentRegistry.reap_epoch",
                "ConditionalLedger.sweep_dust",
                "ConditionalLedger.sweep_dust_baseline",
                "ExecutionGuard.execute",
                "ExecutionGuard.expire_failed_execution",
                "FutarchyTreasury.execute_coretime_renewal",
                "FutarchyTreasury.prune_coretime_quote",
                "Welfare.record_snapshot",
                "Welfare.record_daily_gate",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            live_params: crate::snapshot::LivePlannerParams {
                coretime_quote_ttl: Some(100),
                ..crate::snapshot::LivePlannerParams::default()
            },
            tick_batch: Some(DEFAULT_TICK_BATCH),
            epoch: Some(EpochSnapshot {
                index: 5,
                phase: "Housekeeping".to_owned(),
                phase_start_block: 900,
                epoch_start_block: Some(0),
                length: Some(1_000),
                next_boundary: Some(1_000),
            }),
            books: vec![BookSnapshot {
                market_id: 7,
                phase: "Trading".to_owned(),
                last_observed_block: Some(980),
                decision_window: true,
                stale_in_decision_window: false,
            }],
            proposals: vec![crate::snapshot::ProposalSnapshot {
                proposal_id: 3,
                state: "Trading".to_owned(),
                epoch: Some(5),
                decide_at: Some(999),
                maturity: None,
                grace_end: None,
                market_ids: vec![7],
            }],
            cohorts: vec![CohortSnapshot {
                epoch: 2,
                status: "Measuring".to_owned(),
                until_epoch: Some(4),
                cursor: None,
                metric_spec: Some(2),
            }],
            oracle_rounds: vec![OracleRoundSnapshot {
                component: 1,
                epoch: 4,
                spec_version: 2,
                deadline: Some(999),
            }],
            reserve_health: Some(ReserveHealthSnapshot {
                last_probe_at: Some(0),
                pending_since: None,
            }),
            registry_epochs: vec![RegistryEpochSnapshot {
                pallet: "IncidentRegistry".to_owned(),
                epoch: 2,
                filings: vec![RegistryFilingSnapshot {
                    filing_id: 1,
                    state: "Filed".to_owned(),
                    deadline: Some(999),
                }],
                filing_count_present: true,
                aggregate_present: false,
                closed_at: None,
                archive_delay: Some(50),
            }],
            execution_queue: vec![ExecutionSnapshot {
                proposal_id: 9,
                maturity: Some(900),
                grace_end: Some(1_100),
                failed_at: None,
                cancelled: false,
            }],
            coretime: Some(CoretimeSnapshot {
                quotes: vec![CoretimeQuoteSnapshot {
                    period_index: 12,
                    price: 1_000,
                    noted_at: 950,
                }],
                funded_periods: BTreeSet::new(),
            }),
            market_reaps: vec![ReapSnapshot {
                id: 88,
                terminal_at: Some(900),
                archive_delay: Some(50),
            }],
            proposal_dust: vec![ReapSnapshot {
                id: 3,
                terminal_at: Some(900),
                archive_delay: Some(50),
            }],
            baseline_dust: Vec::new(),
            welfare: Some(WelfareSnapshot {
                active_spec_version: Some(2),
                recorded_snapshots: BTreeSet::new(),
                snapshot_candidates: vec![(4, 2)],
                daily_gate_candidates: vec![(4, 20, 2)],
            }),
        }
    }

    fn config_for(role: Role) -> PlannerConfig {
        PlannerConfig {
            enabled_roles: [role].into(),
            ..PlannerConfig::default()
        }
    }

    fn sequence_argument_len(crank: &PlannedCrank) -> usize {
        let value = crank.args.values().next().expect("crank sequence argument");
        match &value.value {
            ValueDef::Composite(values) => values.values().count(),
            _ => panic!("expected a composite sequence argument"),
        }
    }

    fn empty_registry(pallet: &str, epoch: u64) -> RegistryEpochSnapshot {
        RegistryEpochSnapshot {
            pallet: pallet.to_owned(),
            epoch,
            filings: Vec::new(),
            filing_count_present: false,
            aggregate_present: false,
            closed_at: None,
            archive_delay: Some(50),
        }
    }

    #[test]
    fn empty_snapshot_produces_no_plan() {
        assert!(plan(&ChainSnapshot::default(), &PlannerConfig::default()).is_empty());
    }

    #[test]
    fn due_items_are_planned_for_each_role_in_isolation() {
        let expected_calls = [
            (Role::Tick, "tick"),
            (Role::Observe, "crank_observe"),
            (Role::Decide, "decide"),
            (Role::Settle, "settle_cohort"),
            (Role::Execute, "execute"),
            (Role::OracleClose, "crank_round_close"),
            (Role::RegistryClose, "crank_close"),
            (Role::Cleanup, "reap"),
            (Role::Renewal, "execute_coretime_renewal"),
            (Role::Welfare, "record_snapshot"),
        ];

        for (role, expected_call) in expected_calls {
            let planned = plan(&snapshot(), &config_for(role));
            assert!(
                planned
                    .iter()
                    .any(|crank| crank.role == role && crank.call == expected_call),
                "expected a due {role} crank using {expected_call}; got {planned:?}"
            );
            assert!(planned.iter().all(|crank| crank.role == role));
        }
    }

    #[test]
    fn unsampled_daily_gate_uses_the_welfare_decision_critical_priority() {
        let mut snapshot = snapshot();
        let welfare = snapshot.welfare.as_mut().expect("fixture welfare");
        welfare.snapshot_candidates.clear();
        welfare.daily_gate_candidates = vec![(4, 3, 2)];

        let planned = plan(&snapshot, &config_for(Role::Welfare));
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].call, "record_daily_gate");
        assert_eq!(planned[0].priority, PRIORITY_WELFARE);
    }

    #[test]
    fn decision_critical_work_sorts_before_general_work() {
        let planned = plan(&snapshot(), &PlannerConfig::default());
        assert!(planned
            .windows(2)
            .all(|pair| pair[0].priority >= pair[1].priority));
        assert_eq!(planned.first().map(|crank| crank.role), Some(Role::Tick));
        let decision_observe = planned
            .iter()
            .position(|crank| {
                crank.role == Role::Observe && crank.priority == PRIORITY_DECISION_OBSERVE
            })
            .expect("fixture has a decision-window observation");
        let decide = planned
            .iter()
            .position(|crank| crank.role == Role::Decide)
            .expect("fixture has a due decision");
        let general_observe_priority = PRIORITY_OBSERVE;
        assert!(decision_observe < decide);
        assert!(planned[decision_observe].priority > general_observe_priority);
    }

    #[test]
    fn stale_decision_window_observation_gets_escalated_priority() {
        let mut snapshot = snapshot();
        let normal = plan(&snapshot, &config_for(Role::Observe))
            .into_iter()
            .next()
            .expect("decision-window observation should be due");
        assert_eq!(normal.priority, PRIORITY_DECISION_OBSERVE);

        snapshot.books[0].stale_in_decision_window = true;
        let stale = plan(&snapshot, &config_for(Role::Observe))
            .into_iter()
            .next()
            .expect("stale decision-window observation should be due");
        assert_eq!(stale.call, "crank_observe");
        assert_eq!(stale.priority, PRIORITY_STALE_DECISION_OBSERVE);
        assert!(stale.priority > normal.priority);
    }

    #[test]
    fn tick_batch_comes_from_the_snapshot_with_fallback() {
        let mut snapshot = snapshot();
        snapshot.epoch.as_mut().expect("fixture epoch").phase = "Qualify".to_owned();
        snapshot
            .epoch
            .as_mut()
            .expect("fixture epoch")
            .next_boundary = Some(2_000);
        snapshot.proposals = (1..=12)
            .map(|proposal_id| ProposalSnapshot {
                proposal_id,
                state: "Submitted".to_owned(),
                epoch: Some(5),
                decide_at: None,
                maturity: None,
                grace_end: None,
                market_ids: Vec::new(),
            })
            .collect();

        snapshot.tick_batch = Some(2);
        let metadata_batch = plan(&snapshot, &config_for(Role::Tick));
        assert_eq!(metadata_batch.len(), 1);
        assert_eq!(sequence_argument_len(&metadata_batch[0]), 2);

        snapshot.tick_batch = None;
        let fallback_batch = plan(&snapshot, &config_for(Role::Tick));
        assert_eq!(fallback_batch.len(), 1);
        assert_eq!(
            sequence_argument_len(&fallback_batch[0]),
            DEFAULT_TICK_BATCH
        );
    }

    #[test]
    fn not_due_items_are_suppressed() {
        let mut snapshot = snapshot();
        snapshot
            .epoch
            .as_mut()
            .expect("fixture epoch")
            .next_boundary = Some(2_000);
        snapshot.books[0].last_observed_block = Some(995);
        snapshot.proposals[0].decide_at = Some(2_000);
        snapshot.cohorts[0].until_epoch = Some(5);
        snapshot.oracle_rounds[0].deadline = Some(1_000);
        snapshot.reserve_health = Some(ReserveHealthSnapshot {
            last_probe_at: Some(999),
            pending_since: None,
        });
        snapshot.registry_epochs[0].filings[0].deadline = Some(1_000);
        snapshot.execution_queue[0].maturity = Some(2_000);
        snapshot
            .coretime
            .as_mut()
            .expect("fixture coretime")
            .funded_periods
            .insert(12);
        snapshot.market_reaps[0].archive_delay = Some(200);
        snapshot.proposal_dust[0].archive_delay = Some(200);
        snapshot
            .welfare
            .as_mut()
            .expect("fixture welfare")
            .snapshot_candidates
            .clear();
        snapshot
            .welfare
            .as_mut()
            .expect("fixture welfare")
            .daily_gate_candidates
            .clear();
        for role in Role::ALL {
            let planned = plan(&snapshot, &config_for(role));
            assert!(
                planned.is_empty(),
                "expected no not-due {role} cranks; got {planned:?}"
            );
        }
    }

    #[test]
    fn cooldown_suppresses_only_the_matching_crank() {
        let snapshot = snapshot();
        let first = plan(&snapshot, &PlannerConfig::default());
        let observe = first
            .iter()
            .find(|crank| crank.role == Role::Observe)
            .expect("fixture observation");
        let mut config = PlannerConfig::default();
        config
            .cooldowns
            .insert(observe.cooldown_key(), snapshot.current_block);
        let planned = plan(&snapshot, &config);
        assert!(!planned.iter().any(|crank| crank.role == Role::Observe));
        assert!(planned.iter().any(|crank| crank.role == Role::Decide));
    }

    #[test]
    fn cooldown_expires_at_configured_depth() {
        let snapshot = snapshot();
        let observe = plan(&snapshot, &config_for(Role::Observe))
            .into_iter()
            .next()
            .expect("fixture observation");
        let mut config = config_for(Role::Observe);
        config.cooldown_depth = 3;
        config
            .cooldowns
            .insert(observe.cooldown_key(), snapshot.current_block - 3);

        assert_eq!(plan(&snapshot, &config), vec![observe]);
    }

    #[test]
    fn disabled_role_is_not_planned() {
        let mut config = PlannerConfig::default();
        config.enabled_roles.remove(&Role::Renewal);
        assert!(!plan(&snapshot(), &config)
            .iter()
            .any(|crank| crank.role == Role::Renewal));
    }

    #[test]
    fn renewal_uses_fresh_positive_unfunded_quotes_and_prunes_only_expired_quotes() {
        let mut snapshot = snapshot();
        snapshot.coretime = Some(CoretimeSnapshot {
            quotes: vec![
                CoretimeQuoteSnapshot {
                    period_index: 1,
                    price: 1_000,
                    noted_at: 900,
                },
                CoretimeQuoteSnapshot {
                    period_index: 2,
                    price: 2_000,
                    noted_at: 899,
                },
                CoretimeQuoteSnapshot {
                    period_index: 3,
                    price: 0,
                    noted_at: 950,
                },
                CoretimeQuoteSnapshot {
                    period_index: 4,
                    price: 4_000,
                    noted_at: 950,
                },
                CoretimeQuoteSnapshot {
                    period_index: 5,
                    price: 5_000,
                    noted_at: 1_001,
                },
            ],
            funded_periods: BTreeSet::from([4]),
        });

        let planned = plan(&snapshot, &config_for(Role::Renewal));
        let calls = planned
            .iter()
            .map(|crank| {
                (
                    crank.call,
                    crank
                        .args
                        .values()
                        .next()
                        .and_then(Value::as_u128)
                        .expect("renewal period argument"),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            calls,
            vec![("execute_coretime_renewal", 1), ("prune_coretime_quote", 2)]
        );
    }

    #[test]
    fn renewal_fails_closed_without_ttl_or_required_call() {
        let mut snapshot = snapshot();
        snapshot.live_params.coretime_quote_ttl = None;
        assert!(plan(&snapshot, &config_for(Role::Renewal)).is_empty());

        snapshot.live_params.coretime_quote_ttl = Some(100);
        snapshot
            .available_calls
            .remove("FutarchyTreasury.execute_coretime_renewal");
        assert!(plan(&snapshot, &config_for(Role::Renewal)).is_empty());

        snapshot.coretime.as_mut().expect("fixture coretime").quotes[0].noted_at = 899;
        snapshot
            .available_calls
            .remove("FutarchyTreasury.prune_coretime_quote");
        assert!(plan(&snapshot, &config_for(Role::Renewal)).is_empty());
    }

    #[test]
    fn awaiting_oracle_cohort_is_settlement_actionable() {
        let mut snapshot = snapshot();
        snapshot.cohorts[0].status = "AwaitingOracle".to_owned();
        snapshot.cohorts[0].until_epoch = None;
        assert!(plan(&snapshot, &config_for(Role::Settle))
            .iter()
            .any(|crank| crank.call == "settle_cohort"));
    }

    #[test]
    fn absent_sibling_call_is_suppressed_individually() {
        let mut snapshot = snapshot();
        snapshot
            .available_calls
            .remove("Oracle.crank_reserve_probe");
        let planned = plan(&snapshot, &config_for(Role::OracleClose));
        assert!(planned
            .iter()
            .any(|crank| crank.call == "crank_round_close"));
        assert!(!planned
            .iter()
            .any(|crank| crank.call == "crank_reserve_probe"));
    }

    #[test]
    fn reserve_probe_is_due_at_the_pending_timeout_boundary() {
        let mut snapshot = snapshot();
        snapshot.oracle_rounds.clear();
        snapshot.reserve_health = Some(ReserveHealthSnapshot {
            last_probe_at: Some(snapshot.current_block),
            pending_since: Some(
                snapshot
                    .current_block
                    .saturating_sub(DEFAULT_RESERVE_PROBE_TIMEOUT_BLOCKS),
            ),
        });
        let timed_out = plan(&snapshot, &config_for(Role::OracleClose));
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0].call, "crank_reserve_probe");

        snapshot
            .reserve_health
            .as_mut()
            .expect("fixture reserve health")
            .pending_since = Some(
            snapshot
                .current_block
                .saturating_sub(DEFAULT_RESERVE_PROBE_TIMEOUT_BLOCKS - 1),
        );
        assert!(plan(&snapshot, &config_for(Role::OracleClose)).is_empty());
    }

    #[test]
    fn failed_execution_retries_through_the_window_then_expires() {
        let failed_at = 100;
        let mut snapshot = snapshot();
        snapshot.execution_queue = vec![ExecutionSnapshot {
            proposal_id: 9,
            maturity: Some(failed_at),
            grace_end: None,
            failed_at: Some(failed_at),
            cancelled: false,
        }];
        snapshot.current_block = failed_at + EXECUTION_RETRY_WINDOW_BLOCKS;
        let retry = plan(&snapshot, &config_for(Role::Execute));
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].call, "execute");

        snapshot.current_block += 1;
        let expiry = plan(&snapshot, &config_for(Role::Execute));
        assert_eq!(expiry.len(), 1);
        assert_eq!(expiry[0].call, "expire_failed_execution");
    }

    #[test]
    fn pallet_filing_count_guard_keeps_zero_filing_past_epochs_unplanned() {
        let mut snapshot = snapshot();
        snapshot
            .available_calls
            .insert("MilestoneRegistry.close_epoch".to_owned());
        snapshot.registry_epochs = vec![
            empty_registry("IncidentRegistry", 3),
            empty_registry("MilestoneRegistry", 3),
        ];
        let planned = plan(&snapshot, &config_for(Role::RegistryClose));
        // `pallet-registry::close_epoch` requires a live `FilingCount` entry, so
        // row-free epochs must never become doomed `NothingToClose` submissions.
        assert!(planned.is_empty());
    }

    #[test]
    fn registry_close_proven_requires_visible_terminal_filings_and_filing_count() {
        let snapshot = snapshot();
        let empty = empty_registry("IncidentRegistry", 3);
        assert!(!registry_close_proven(&snapshot, &empty));

        let mut terminal = empty_registry("IncidentRegistry", 3);
        terminal.filing_count_present = true;
        terminal.filings = vec![RegistryFilingSnapshot {
            filing_id: 1,
            state: "Upheld".to_owned(),
            deadline: None,
        }];
        assert!(registry_close_proven(&snapshot, &terminal));

        let mut missing_count = terminal.clone();
        missing_count.filing_count_present = false;
        assert!(!registry_close_proven(&snapshot, &missing_count));

        let mut non_terminal = terminal.clone();
        non_terminal.filings[0].state = "Filed".to_owned();
        assert!(!registry_close_proven(&snapshot, &non_terminal));

        let mut aggregated = terminal.clone();
        aggregated.aggregate_present = true;
        assert!(!registry_close_proven(&snapshot, &aggregated));

        let mut closed = terminal.clone();
        closed.closed_at = Some(999);
        assert!(!registry_close_proven(&snapshot, &closed));

        let mut window_open = snapshot.clone();
        window_open.epoch.as_mut().expect("fixture epoch").index = 4;
        assert!(!registry_close_proven(&window_open, &terminal));
    }
}
