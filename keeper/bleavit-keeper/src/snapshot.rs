use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use subxt::{
    config::HashFor,
    dynamic,
    ext::{
        scale_decode::DecodeAsType,
        scale_value::{At, Value, ValueDef},
    },
    ArcMetadata, OnlineClient, OnlineClientAtBlock, PolkadotConfig,
};
use tracing::{debug, warn};

use crate::{
    config::{Role, RoleSet},
    transport::is_transport,
};

pub const DEFAULT_OBSERVATION_INTERVAL_BLOCKS: u64 = 10;
pub const DEFAULT_DECISION_WINDOW_BLOCKS: u64 = 43_200;
pub const STALE_OBSERVATION_GAP_BLOCKS: u64 = 50;
pub const DEFAULT_RESERVE_PROBE_INTERVAL_BLOCKS: u64 = 14_400;
pub const DEFAULT_RESERVE_PROBE_TIMEOUT_BLOCKS: u64 = 600;
pub const DEFAULT_TICK_BATCH: usize = 10;
/// Compatibility fallbacks for older metadata. The chain publishes the same
/// welfare-core bounds as `Welfare.MaxGateFlags`/`MaxDailyGateSamples`.
pub const DEFAULT_WELFARE_LOOKBACK: usize = 20;
pub const DEFAULT_DAILY_GATE_SAMPLES: u8 = 64;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChainSnapshot {
    pub current_block: u64,
    pub available_pallets: BTreeSet<String>,
    pub available_calls: BTreeSet<String>,
    pub live_params: LivePlannerParams,
    pub tick_batch: Option<usize>,
    pub epoch: Option<EpochSnapshot>,
    pub books: Vec<BookSnapshot>,
    pub proposals: Vec<ProposalSnapshot>,
    pub cohorts: Vec<CohortSnapshot>,
    pub oracle_rounds: Vec<OracleRoundSnapshot>,
    pub reserve_health: Option<ReserveHealthSnapshot>,
    pub registry_epochs: Vec<RegistryEpochSnapshot>,
    pub execution_queue: Vec<ExecutionSnapshot>,
    pub coretime: Option<CoretimeSnapshot>,
    pub market_reaps: Vec<ReapSnapshot>,
    pub proposal_dust: Vec<ReapSnapshot>,
    pub baseline_dust: Vec<ReapSnapshot>,
    pub welfare: Option<WelfareSnapshot>,
}

impl ChainSnapshot {
    pub fn has_call(&self, pallet: &str, call: &str) -> bool {
        self.available_calls.contains(&call_key(pallet, call))
    }

    pub fn apply_decision_window(&mut self, decision_window: u64) {
        mark_decision_window(
            self.current_block,
            decision_window,
            &self.proposals,
            &mut self.books,
        );
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LivePlannerParams {
    pub obs_interval: Option<u64>,
    pub decision_window: Option<u64>,
    pub reserve_probe_interval: Option<u64>,
    pub reserve_probe_timeout: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpochSnapshot {
    pub index: u64,
    pub phase: String,
    pub phase_start_block: u64,
    pub epoch_start_block: Option<u64>,
    pub length: Option<u64>,
    pub next_boundary: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookSnapshot {
    pub market_id: u64,
    pub phase: String,
    pub last_observed_block: Option<u64>,
    pub decision_window: bool,
    pub stale_in_decision_window: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalSnapshot {
    pub proposal_id: u64,
    pub state: String,
    pub epoch: Option<u64>,
    pub decide_at: Option<u64>,
    pub maturity: Option<u64>,
    pub grace_end: Option<u64>,
    pub market_ids: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CohortSnapshot {
    pub epoch: u64,
    pub status: String,
    pub until_epoch: Option<u64>,
    pub cursor: Option<u64>,
    pub metric_spec: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleRoundSnapshot {
    pub component: u64,
    pub epoch: u64,
    pub spec_version: u64,
    pub deadline: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReserveHealthSnapshot {
    pub last_probe_at: Option<u64>,
    pub pending_since: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryFilingSnapshot {
    pub filing_id: u64,
    pub state: String,
    pub deadline: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryEpochSnapshot {
    pub pallet: String,
    pub epoch: u64,
    pub filings: Vec<RegistryFilingSnapshot>,
    pub filing_count_present: bool,
    pub aggregate_present: bool,
    pub closed_at: Option<u64>,
    pub archive_delay: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionSnapshot {
    pub proposal_id: u64,
    pub maturity: Option<u64>,
    pub grace_end: Option<u64>,
    pub failed_at: Option<u64>,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoretimeSnapshot {
    pub quotes: Vec<(u64, u128)>,
    pub funded_periods: BTreeSet<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReapSnapshot {
    pub id: u64,
    pub terminal_at: Option<u64>,
    pub archive_delay: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WelfareSnapshot {
    pub active_spec_version: Option<u64>,
    pub recorded_snapshots: BTreeSet<(u64, u64)>,
    pub snapshot_candidates: Vec<(u64, u64)>,
    pub daily_gate_candidates: Vec<(u64, u8, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleCapability {
    pub role: Role,
    pub available: bool,
    pub reason: &'static str,
}

#[derive(Clone)]
pub struct SnapshotExtractor {
    client: OnlineClient<PolkadotConfig>,
    metadata: ArcMetadata,
    capabilities: Vec<RoleCapability>,
    pallets: BTreeSet<String>,
    calls: BTreeSet<String>,
    transport_failed: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotTransportError;

impl std::fmt::Display for SnapshotTransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("transport failed while extracting finalized storage")
    }
}

impl std::error::Error for SnapshotTransportError {}

impl SnapshotExtractor {
    pub async fn new(
        client: OnlineClient<PolkadotConfig>,
    ) -> Result<Self, subxt::error::OnlineClientAtBlockError> {
        let metadata = client.at_current_block().await?.metadata();
        let pallets = metadata
            .pallets()
            .map(|pallet| pallet.name().to_owned())
            .collect::<BTreeSet<_>>();
        let calls = metadata
            .pallets()
            .flat_map(|pallet| {
                let pallet_name = pallet.name().to_owned();
                pallet
                    .call_variants()
                    .into_iter()
                    .flatten()
                    .map(move |call| call_key(&pallet_name, &call.name))
            })
            .collect::<BTreeSet<_>>();
        let has_call = |pallet: &str, call: &str| calls.contains(&call_key(pallet, call));
        let any_registry = ["IncidentRegistry", "MilestoneRegistry"]
            .iter()
            .any(|pallet| {
                ["crank_close", "close_epoch", "reap_epoch"]
                    .iter()
                    .any(|call| has_call(pallet, call))
            });
        let capabilities = vec![
            capability(Role::Tick, has_call("Epoch", "tick"), "Epoch.tick absent"),
            capability(
                Role::Observe,
                has_call("Market", "crank_observe"),
                "Market.crank_observe absent",
            ),
            capability(
                Role::Decide,
                has_call("Epoch", "decide"),
                "Epoch.decide absent",
            ),
            capability(
                Role::Settle,
                has_call("Epoch", "settle_cohort"),
                "Epoch.settle_cohort absent",
            ),
            capability(
                Role::Execute,
                ["execute", "expire_failed_execution", "reject_stale"]
                    .iter()
                    .any(|call| has_call("ExecutionGuard", call)),
                "ExecutionGuard keeper calls absent",
            ),
            capability(
                Role::OracleClose,
                has_call("Oracle", "crank_round_close")
                    || has_call("Oracle", "crank_reserve_probe"),
                "Oracle crank calls absent",
            ),
            capability(
                Role::RegistryClose,
                any_registry,
                "registry crank calls absent",
            ),
            capability(
                Role::Cleanup,
                has_call("Market", "reap")
                    || has_call("ConditionalLedger", "sweep_dust")
                    || has_call("ConditionalLedger", "sweep_dust_baseline")
                    || any_registry,
                "cleanup calls absent",
            ),
            capability(
                Role::Renewal,
                has_call("FutarchyTreasury", "execute_coretime_renewal"),
                "treasury renewal call absent",
            ),
            capability(
                Role::Welfare,
                has_call("Welfare", "record_snapshot") || has_call("Welfare", "record_daily_gate"),
                "welfare crank calls absent",
            ),
        ];
        Ok(Self {
            client,
            metadata,
            capabilities,
            pallets,
            calls,
            transport_failed: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn capabilities(&self) -> &[RoleCapability] {
        &self.capabilities
    }

    pub fn available_roles(&self) -> RoleSet {
        self.capabilities
            .iter()
            .filter(|capability| capability.available)
            .map(|capability| capability.role)
            .collect()
    }

    /// Older runtimes do not expose the auxiliary marker added after the B9
    /// review. In that case daily-gate planning must remain disabled.
    pub fn welfare_daily_gates_plannable(&self) -> bool {
        self.has_storage("Welfare", "SampledGateDays")
    }

    pub async fn extract(
        &self,
        current_block: u64,
        block_hash: HashFor<PolkadotConfig>,
    ) -> Result<ChainSnapshot, SnapshotTransportError> {
        self.transport_failed.store(false, Ordering::Relaxed);
        let at_block = match self.client.at_block(block_hash).await {
            Ok(at_block) => at_block,
            Err(error) => {
                let error = subxt::Error::from(error);
                if is_transport(&error) {
                    return Err(SnapshotTransportError);
                }
                warn!(%error, current_block, "block-scoped snapshot client unavailable");
                return Ok(ChainSnapshot {
                    current_block,
                    available_pallets: self.pallets.clone(),
                    available_calls: self.calls.clone(),
                    ..ChainSnapshot::default()
                });
            }
        };
        let live_params = self.extract_live_planner_params(&at_block).await;
        let epoch = self.extract_epoch(&at_block).await;
        let proposals = self.extract_proposals(&at_block).await;
        let cohorts = self.extract_cohorts(&at_block).await;
        let mut books = self.extract_books(&at_block).await;
        mark_decision_window(
            current_block,
            resolve_chain_param(
                None,
                live_params.decision_window,
                DEFAULT_DECISION_WINDOW_BLOCKS,
            ),
            &proposals,
            &mut books,
        );
        let market_archive = self.constant_u64(&at_block, "Market", "ArchiveDelay");
        let ledger_archive = self.constant_u64(&at_block, "ConditionalLedger", "ArchiveDelay");
        let tick_batch = resolve_tick_batch(self.constant_u64(&at_block, "Epoch", "TickBatch"));
        let registry_epochs = self.extract_registries(&at_block).await;

        let welfare = self
            .extract_welfare(&at_block, epoch.as_ref(), &cohorts)
            .await;
        let snapshot = ChainSnapshot {
            current_block,
            available_pallets: self.pallets.clone(),
            available_calls: self.calls.clone(),
            live_params,
            tick_batch: Some(tick_batch),
            epoch,
            books,
            proposals,
            cohorts,
            oracle_rounds: self.extract_oracle_rounds(&at_block).await,
            reserve_health: self.extract_reserve_health(&at_block).await,
            registry_epochs,
            execution_queue: self.extract_execution_queue(&at_block).await,
            coretime: self.extract_coretime(&at_block).await,
            market_reaps: self
                .extract_reaps(&at_block, "Market", "ClosedAt", market_archive)
                .await,
            proposal_dust: self
                .extract_reaps(
                    &at_block,
                    "ConditionalLedger",
                    "VaultTerminalAt",
                    ledger_archive,
                )
                .await,
            baseline_dust: self
                .extract_reaps(
                    &at_block,
                    "ConditionalLedger",
                    "BaselineTerminalAt",
                    ledger_archive,
                )
                .await,
            welfare,
        };
        if self.transport_failed.swap(false, Ordering::Relaxed) {
            Err(SnapshotTransportError)
        } else {
            Ok(snapshot)
        }
    }

    async fn extract_live_planner_params(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> LivePlannerParams {
        let (obs_interval, decision_window, reserve_probe_interval, reserve_probe_timeout) = tokio::join!(
            self.fetch_u32_param(at_block, b"mkt.obs_interval"),
            self.fetch_u32_param(at_block, b"dec.window"),
            self.fetch_u32_param(at_block, b"res.probe_int"),
            self.fetch_u32_param(at_block, b"res.probe_to"),
        );
        LivePlannerParams {
            obs_interval,
            decision_window,
            reserve_probe_interval,
            reserve_probe_timeout,
        }
    }

    async fn extract_epoch(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Option<EpochSnapshot> {
        let value = self.fetch_value(at_block, "Epoch", "EpochOf").await?;
        let schedule = self.fetch_value(at_block, "Epoch", "Schedule").await;
        let index = value.at("index").and_then(as_u64)?;
        let phase = value.at("phase").and_then(variant_name)?.to_owned();
        let phase_start_block = value.at("phase_start_block").and_then(as_u64)?;
        let epoch_start_block = schedule
            .as_ref()
            .and_then(|item| item.at("epoch_start_block"))
            .and_then(as_u64);
        let length = schedule
            .as_ref()
            .and_then(|item| item.at("length"))
            .and_then(as_u64);
        let next_boundary = epoch_start_block
            .zip(length)
            .and_then(|(start, length)| phase_boundary(start, length, &phase));
        Some(EpochSnapshot {
            index,
            phase,
            phase_start_block,
            epoch_start_block,
            length,
            next_boundary,
        })
    }

    async fn extract_proposals(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Vec<ProposalSnapshot> {
        self.iter_values(at_block, "Epoch", "Proposals")
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                let proposal_id = value
                    .at("id")
                    .and_then(as_u64)
                    .or_else(|| keys.first().and_then(as_u64))?;
                let state = value.at("state").and_then(variant_name)?.to_owned();
                let market_ids = value
                    .at("markets")
                    .and_then(option_inner)
                    .map(market_set_ids)
                    .unwrap_or_default();
                Some(ProposalSnapshot {
                    proposal_id,
                    state,
                    epoch: value.at("epoch").and_then(as_u64),
                    decide_at: nonzero(value.at("decide_at").and_then(as_u64)),
                    maturity: value.at("maturity").and_then(option_u64),
                    grace_end: value.at("grace_end").and_then(option_u64),
                    market_ids,
                })
            })
            .collect()
    }

    async fn extract_cohorts(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Vec<CohortSnapshot> {
        let schedules = self
            .iter_values(at_block, "Epoch", "CohortSchedules")
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                let epoch = value
                    .at("epoch")
                    .and_then(as_u64)
                    .or_else(|| keys.first().and_then(as_u64))?;
                Some((epoch, value.at("specs").and_then(single_cohort_spec)))
            })
            .collect::<BTreeMap<_, _>>();
        self.iter_values(at_block, "Epoch", "Cohorts")
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                let epoch = value
                    .at("epoch")
                    .and_then(as_u64)
                    .or_else(|| keys.first().and_then(as_u64))?;
                let status_value = value.at("status")?;
                let status = variant_name(status_value)?.to_owned();
                Some(CohortSnapshot {
                    epoch,
                    status,
                    until_epoch: variant_field(status_value, "until_epoch").and_then(as_u64),
                    cursor: variant_field(status_value, "cursor").and_then(as_u64),
                    metric_spec: schedules.get(&epoch).copied().flatten(),
                })
            })
            .collect()
    }

    async fn extract_books(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Vec<BookSnapshot> {
        self.iter_values(at_block, "Market", "Markets")
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                let market_id = value
                    .at("id")
                    .and_then(as_u64)
                    .or_else(|| keys.first().and_then(as_u64))?;
                Some(BookSnapshot {
                    market_id,
                    phase: value.at("phase").and_then(variant_name)?.to_owned(),
                    last_observed_block: value.at("last_observed_block").and_then(as_u64),
                    decision_window: false,
                    stale_in_decision_window: false,
                })
            })
            .collect()
    }

    async fn extract_oracle_rounds(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Vec<OracleRoundSnapshot> {
        self.iter_values(at_block, "Oracle", "Rounds")
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                Some(OracleRoundSnapshot {
                    component: value
                        .at("component")
                        .and_then(as_u64)
                        .or_else(|| keys.first().and_then(as_u64))?,
                    epoch: value
                        .at("epoch")
                        .and_then(as_u64)
                        .or_else(|| keys.get(1).and_then(as_u64))?,
                    spec_version: value
                        .at("spec_version")
                        .and_then(as_u64)
                        .or_else(|| keys.get(2).and_then(as_u64))?,
                    deadline: value.at("challenge_deadline").and_then(as_u64),
                })
            })
            .collect()
    }

    async fn extract_reserve_health(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Option<ReserveHealthSnapshot> {
        let value = self
            .fetch_value(at_block, "Oracle", "ReserveHealth")
            .await?;
        Some(ReserveHealthSnapshot {
            last_probe_at: value.at("last_probe_at").and_then(as_u64),
            pending_since: value.at("pending_since").and_then(option_u64),
        })
    }

    async fn extract_registries(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Vec<RegistryEpochSnapshot> {
        let mut result = Vec::new();
        for pallet in ["IncidentRegistry", "MilestoneRegistry"] {
            if !self.has_storage(pallet, "Filings") {
                continue;
            }
            let archive_delay = self.constant_u64(at_block, pallet, "ArchiveDelay");
            let mut by_epoch = BTreeMap::<u64, RegistryEpochSnapshot>::new();
            for (keys, value) in self.iter_values(at_block, pallet, "Filings").await {
                let Some(epoch) = keys.first().and_then(as_u64) else {
                    continue;
                };
                let Some(filing_id) = keys.get(1).and_then(as_u64) else {
                    continue;
                };
                let Some(state_value) = value.at("state") else {
                    continue;
                };
                let Some(state) = variant_name(state_value) else {
                    continue;
                };
                by_epoch
                    .entry(epoch)
                    .or_insert_with(|| registry_epoch(pallet, epoch, archive_delay))
                    .filings
                    .push(RegistryFilingSnapshot {
                        filing_id,
                        state: state.to_owned(),
                        deadline: variant_field(state_value, "window_end").and_then(as_u64),
                    });
            }
            for (keys, _) in self.iter_values(at_block, pallet, "FilingCount").await {
                if let Some(epoch) = keys.first().and_then(as_u64) {
                    by_epoch
                        .entry(epoch)
                        .or_insert_with(|| registry_epoch(pallet, epoch, archive_delay))
                        .filing_count_present = true;
                }
            }
            for (keys, _) in self.iter_values(at_block, pallet, "Aggregates").await {
                if let Some(epoch) = keys.first().and_then(as_u64) {
                    by_epoch
                        .entry(epoch)
                        .or_insert_with(|| registry_epoch(pallet, epoch, archive_delay))
                        .aggregate_present = true;
                }
            }
            for (keys, value) in self.iter_values(at_block, pallet, "ClosedAt").await {
                if let Some(epoch) = keys.first().and_then(as_u64) {
                    by_epoch
                        .entry(epoch)
                        .or_insert_with(|| registry_epoch(pallet, epoch, archive_delay))
                        .closed_at = as_u64(&value);
                }
            }
            result.extend(by_epoch.into_values());
        }
        result
    }

    async fn extract_execution_queue(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Vec<ExecutionSnapshot> {
        self.iter_values(at_block, "ExecutionGuard", "Queue")
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                Some(ExecutionSnapshot {
                    proposal_id: value
                        .at("pid")
                        .and_then(as_u64)
                        .or_else(|| keys.first().and_then(as_u64))?,
                    maturity: value.at("maturity").and_then(as_u64),
                    grace_end: value.at("grace_end").and_then(as_u64),
                    failed_at: value.at("failed_at").and_then(option_u64),
                    cancelled: value
                        .at("cancelled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect()
    }

    async fn extract_coretime(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Option<CoretimeSnapshot> {
        let value = self
            .fetch_value(at_block, "FutarchyTreasury", "State")
            .await?;
        let quotes = value
            .at("coretime_quotes")
            .map(tuple_pairs)
            .unwrap_or_default();
        let funded_periods = value
            .at("funded_coretime_periods")
            .map(composite_u64s)
            .unwrap_or_default()
            .into_iter()
            .collect();
        Some(CoretimeSnapshot {
            quotes,
            funded_periods,
        })
    }

    async fn extract_reaps(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
        archive_delay: Option<u64>,
    ) -> Vec<ReapSnapshot> {
        self.iter_values(at_block, pallet, storage_name)
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                Some(ReapSnapshot {
                    id: keys.first().and_then(as_u64)?,
                    terminal_at: as_u64(&value),
                    archive_delay,
                })
            })
            .collect()
    }

    async fn extract_welfare(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        epoch: Option<&EpochSnapshot>,
        cohorts: &[CohortSnapshot],
    ) -> Option<WelfareSnapshot> {
        if !self.has_storage("Welfare", "MetricSpecs") {
            return None;
        }
        let metric_specs = self.iter_values(at_block, "Welfare", "MetricSpecs").await;
        let recorded_snapshots: BTreeSet<(u64, u64)> = self
            .iter_values(at_block, "Welfare", "Snapshots")
            .await
            .into_iter()
            .filter_map(|(keys, _)| tuple_key_pair(&keys))
            .collect();
        let spec_activations = metric_specs
            .iter()
            .filter_map(|(keys, specs)| {
                let version = keys.first().and_then(as_u64)?;
                let activations = composite_values(specs)
                    .map(|spec| spec.at("activation_epoch").and_then(as_u64))
                    .collect::<Option<Vec<_>>>()?;
                let activation = activations.into_iter().max()?;
                Some((version, activation))
            })
            .collect::<BTreeMap<_, _>>();
        let (active_spec_version, snapshot_candidates) = derive_welfare_candidates(
            epoch.map(|value| value.index),
            &spec_activations,
            &recorded_snapshots,
            cohorts,
        );
        let sampled_gate_days = if self.welfare_daily_gates_plannable() {
            Some(
                self.iter_values(at_block, "Welfare", "SampledGateDays")
                    .await
                    .into_iter()
                    .filter_map(|(keys, value)| {
                        Some((keys.first().and_then(as_u64)?, gate_day_bitmap(&value)?))
                    })
                    .collect::<BTreeMap<_, _>>(),
            )
        } else {
            None
        };
        let lookback =
            resolve_welfare_lookback(self.constant_u64(at_block, "Welfare", "MaxGateFlags"));
        let daily_samples = resolve_daily_gate_samples(self.constant_u64(
            at_block,
            "Welfare",
            "MaxDailyGateSamples",
        ));
        let daily_gate_candidates = derive_daily_gate_candidates(
            epoch.map(|value| value.index),
            &spec_activations,
            sampled_gate_days.as_ref(),
            lookback,
            daily_samples,
        );
        Some(WelfareSnapshot {
            active_spec_version,
            recorded_snapshots,
            snapshot_candidates,
            daily_gate_candidates,
        })
    }

    async fn fetch_value(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
    ) -> Option<Value<()>> {
        self.fetch_value_with_keys(at_block, pallet, storage_name, Vec::new())
            .await
    }

    async fn fetch_u32_param(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        name: &[u8],
    ) -> Option<u64> {
        let key = param_key(name)?;
        let value = self
            .fetch_value_with_keys(
                at_block,
                "Constitution",
                "Params",
                vec![param_key_value(&key)],
            )
            .await?;
        param_record_u32(&value, &key)
    }

    async fn fetch_value_with_keys(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
        keys: Vec<Value<()>>,
    ) -> Option<Value<()>> {
        if !self.has_storage(pallet, storage_name) {
            return None;
        }
        let address = dynamic::storage::<Vec<Value<()>>, Value<()>>(pallet, storage_name);
        let entry = match at_block.storage().entry(address) {
            Ok(entry) => entry,
            Err(error) => {
                warn!(pallet, storage = storage_name, %error, "dynamic storage entry unavailable");
                return None;
            }
        };
        let key = match entry.fetch_key(keys) {
            Ok(key) => key,
            Err(error) => {
                warn!(pallet, storage = storage_name, %error, "dynamic storage key unavailable");
                return None;
            }
        };
        let value_ty = match at_block
            .metadata_ref()
            .pallet_by_name(pallet)
            .and_then(|details| details.storage())
            .and_then(|storage| storage.entry_by_name(storage_name))
        {
            Some(entry) => entry.value_ty(),
            None => return None,
        };
        // Subxt 0.50's `try_fetch` applies a metadata default when raw state is
        // absent. The 0.44 dynamic `fetch` used here returned `None`, so fetch
        // raw bytes to preserve the keeper's fail-closed absence semantics.
        match at_block.storage().fetch_raw(key).await {
            Ok(bytes) => match Value::decode_as_type(
                &mut bytes.as_slice(),
                value_ty,
                at_block.metadata_ref().types(),
            ) {
                Ok(value) => Some(value),
                Err(error) => {
                    warn!(pallet, storage = storage_name, %error, "dynamic storage decode failed");
                    None
                }
            },
            Err(subxt::error::StorageError::NoValueFound) => None,
            Err(error) => {
                let error = subxt::Error::from(error);
                self.note_transport_error(&error);
                warn!(pallet, storage = storage_name, %error, "dynamic storage read failed");
                None
            }
        }
    }

    async fn iter_values(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
    ) -> Vec<(Vec<Value<()>>, Value<()>)> {
        if !self.has_storage(pallet, storage_name) {
            return Vec::new();
        }
        let address = dynamic::storage::<Vec<Value<()>>, Value<()>>(pallet, storage_name);
        let entry = match at_block.storage().entry(address) {
            Ok(entry) => entry,
            Err(error) => {
                warn!(pallet, storage = storage_name, %error, "dynamic storage entry unavailable");
                return Vec::new();
            }
        };
        let mut entries = match entry.iter(Vec::<Value<()>>::new()).await {
            Ok(entries) => entries,
            Err(error) => {
                let error = subxt::Error::from(error);
                self.note_transport_error(&error);
                warn!(pallet, storage = storage_name, %error, "dynamic storage iteration failed");
                return Vec::new();
            }
        };
        let mut values = Vec::new();
        while let Some(entry) = entries.next().await {
            match entry {
                Ok(entry) => {
                    let keys = match entry.key().and_then(|key| key.decode()) {
                        Ok(keys) => keys,
                        Err(error) => {
                            warn!(pallet, storage = storage_name, %error, "dynamic storage key decode failed");
                            break;
                        }
                    };
                    match entry.value().decode() {
                        Ok(value) => values.push((keys, value)),
                        Err(error) => warn!(
                            pallet,
                            storage = storage_name,
                            %error,
                            "dynamic storage item decode failed"
                        ),
                    }
                }
                Err(error) => {
                    let error = subxt::Error::from(error);
                    self.note_transport_error(&error);
                    warn!(pallet, storage = storage_name, %error, "dynamic storage item read failed");
                    break;
                }
            }
        }
        values
    }

    fn has_storage(&self, pallet: &str, storage_name: &str) -> bool {
        self.metadata
            .pallet_by_name(pallet)
            .and_then(|details| details.storage())
            .and_then(|storage| storage.entry_by_name(storage_name))
            .is_some()
    }

    fn note_transport_error(&self, error: &subxt::Error) {
        if is_transport(error) {
            self.transport_failed.store(true, Ordering::Relaxed);
        }
    }

    fn constant_u64(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        pallet: &str,
        constant: &str,
    ) -> Option<u64> {
        if !self.pallets.contains(pallet) {
            return None;
        }
        let address = dynamic::constant::<Value<()>>(pallet, constant);
        match at_block.constants().entry(address) {
            Ok(value) => as_u64(&value),
            Err(error) => {
                debug!(pallet, constant, %error, "dynamic constant unavailable");
                None
            }
        }
    }
}

fn call_key(pallet: &str, call: &str) -> String {
    format!("{pallet}.{call}")
}

/// Shared precedence for every constitution row mirrored by the keeper.
pub const fn resolve_chain_param(
    operator_override: Option<u64>,
    live_value: Option<u64>,
    documented_default: u64,
) -> u64 {
    match operator_override {
        Some(value) => value,
        None => match live_value {
            Some(value) => value,
            None => documented_default,
        },
    }
}

/// 13 rule 6: UTF-8 names are zero-padded to the full 16-byte `ParamKey`.
/// Names longer than 16 bytes require an explicit short registry key.
fn param_key(name: &[u8]) -> Option<[u8; 16]> {
    if name.len() > 16 {
        return None;
    }
    let mut key = [0u8; 16];
    key.get_mut(..name.len())?.copy_from_slice(name);
    Some(key)
}

fn param_key_value(key: &[u8; 16]) -> Value<()> {
    Value::unnamed_composite(key.iter().map(|byte| Value::u128(u128::from(*byte))))
}

fn param_key_from_value<C>(value: &Value<C>) -> Option<[u8; 16]> {
    composite_values(value)
        .map(|byte| u8::try_from(as_u64(byte)?).ok())
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

/// Exact `ParamRecord { key, value: ParamValue, ... }` navigation from
/// constitution-core. Only the `U32` variant is admitted for mirrored rows.
fn param_record_u32<C>(record: &Value<C>, expected_key: &[u8; 16]) -> Option<u64> {
    let stored_key = record.at("key").and_then(param_key_from_value)?;
    if &stored_key != expected_key {
        return None;
    }
    let value = record.at("value")?;
    match &value.value {
        ValueDef::Variant(variant) if variant.name == "U32" => {
            let mut fields = variant.values.values();
            let decoded = as_u64(fields.next()?)?;
            fields.next().is_none().then_some(decoded)
        }
        _ => None,
    }
}

fn capability(role: Role, available: bool, missing_reason: &'static str) -> RoleCapability {
    RoleCapability {
        role,
        available,
        reason: if available {
            "metadata call surface present"
        } else {
            missing_reason
        },
    }
}

fn registry_epoch(pallet: &str, epoch: u64, archive_delay: Option<u64>) -> RegistryEpochSnapshot {
    RegistryEpochSnapshot {
        pallet: pallet.to_owned(),
        epoch,
        filings: Vec::new(),
        filing_count_present: false,
        aggregate_present: false,
        closed_at: None,
        archive_delay,
    }
}

fn resolve_tick_batch(value: Option<u64>) -> usize {
    value
        .filter(|value| *value > 0)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_TICK_BATCH)
}

fn resolve_welfare_lookback(value: Option<u64>) -> usize {
    value
        .filter(|value| *value > 0)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_WELFARE_LOOKBACK)
}

fn resolve_daily_gate_samples(value: Option<u64>) -> u8 {
    value
        .filter(|value| *value > 0)
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| *value <= DEFAULT_DAILY_GATE_SAMPLES)
        .unwrap_or(DEFAULT_DAILY_GATE_SAMPLES)
}

fn phase_boundary(epoch_start: u64, length: u64, phase: &str) -> Option<u64> {
    let numerator = match phase {
        "Intake" => 3,
        "Qualify" => 4,
        "Seed" => 5,
        "Trade" => 18,
        "Decide" => 20,
        "Housekeeping" => 21,
        "Review" | "Execute" => {
            return Some(
                epoch_start
                    .saturating_add(length.saturating_mul(18) / 21)
                    .saturating_add(1),
            );
        }
        _ => return None,
    };
    Some(epoch_start.saturating_add(length.saturating_mul(numerator) / 21))
}

fn mark_decision_window(
    current_block: u64,
    decision_window: u64,
    proposals: &[ProposalSnapshot],
    books: &mut [BookSnapshot],
) {
    let critical = proposals
        .iter()
        .filter(|proposal| matches!(proposal.state.as_str(), "Trading" | "Extended"))
        .filter(|proposal| {
            proposal.decide_at.is_some_and(|decide_at| {
                current_block >= decide_at.saturating_sub(decision_window)
                    && current_block <= decide_at
            })
        })
        .flat_map(|proposal| proposal.market_ids.iter().copied())
        .collect::<BTreeSet<_>>();
    for book in books {
        book.decision_window = critical.contains(&book.market_id);
        book.stale_in_decision_window = book.decision_window
            && book.last_observed_block.is_some_and(|last| {
                current_block.saturating_sub(last) > STALE_OBSERVATION_GAP_BLOCKS
            });
    }
}

fn market_set_ids<C>(value: &Value<C>) -> Vec<u64> {
    let mut ids = Vec::new();
    for name in ["accept", "reject", "baseline"] {
        if let Some(id) = value.at(name).and_then(as_u64) {
            ids.push(id);
        }
    }
    if let Some(gates) = value.at("gates").and_then(option_inner) {
        ids.extend(composite_values(gates).filter_map(as_u64));
    }
    ids
}

fn tuple_pairs<C>(value: &Value<C>) -> Vec<(u64, u128)> {
    composite_values(value)
        .filter_map(|pair| {
            let mut values = composite_values(pair);
            Some((as_u64(values.next()?)?, values.next()?.as_u128()?))
        })
        .collect()
}

fn composite_u64s<C>(value: &Value<C>) -> Vec<u64> {
    composite_values(value).filter_map(as_u64).collect()
}

fn tuple_key_pair(keys: &[Value<()>]) -> Option<(u64, u64)> {
    let mut values = composite_values(keys.first()?);
    Some((as_u64(values.next()?)?, as_u64(values.next()?)?))
}

fn gate_day_bitmap<C>(value: &Value<C>) -> Option<[u32; 2]> {
    let mut words = composite_values(value).map(as_u64);
    let first = u32::try_from(words.next()??).ok()?;
    let second = u32::try_from(words.next()??).ok()?;
    words.next().is_none().then_some([first, second])
}

fn single_cohort_spec<C>(value: &Value<C>) -> Option<u64> {
    let mut specs = composite_values(value).map(|binding| {
        let mut fields = composite_values(binding);
        let _proposal = as_u64(fields.next()?)?;
        as_u64(fields.next()?)
    });
    let first = specs.next()??;
    specs.all(|spec| spec == Some(first)).then_some(first)
}

fn derive_welfare_candidates(
    current_epoch: Option<u64>,
    spec_activations: &BTreeMap<u64, u64>,
    recorded: &BTreeSet<(u64, u64)>,
    cohorts: &[CohortSnapshot],
) -> (Option<u64>, Vec<(u64, u64)>) {
    let Some(current_epoch) = current_epoch else {
        return (None, Vec::new());
    };
    let finalized_epoch = current_epoch.checked_sub(1);
    let active_spec = finalized_epoch.and_then(|finalized| {
        spec_activations
            .iter()
            .filter(|(_, activation)| **activation <= finalized)
            .map(|(version, _)| *version)
            .max()
    });
    let mut candidates = BTreeSet::new();
    if let Some(candidate) = finalized_epoch.zip(active_spec) {
        if !recorded.contains(&candidate) {
            candidates.insert(candidate);
        }
    }
    for cohort in cohorts {
        let Some(spec) = cohort.metric_spec else {
            continue;
        };
        let Some(activation) = spec_activations.get(&spec) else {
            continue;
        };
        for offset in [1_u64, 2] {
            let Some(target_epoch) = cohort.epoch.checked_add(offset) else {
                continue;
            };
            let candidate = (target_epoch, spec);
            if target_epoch < current_epoch
                && target_epoch >= *activation
                && !recorded.contains(&candidate)
            {
                candidates.insert(candidate);
            }
        }
    }
    (active_spec, candidates.into_iter().collect())
}

fn derive_daily_gate_candidates(
    current_epoch: Option<u64>,
    spec_activations: &BTreeMap<u64, u64>,
    sampled: Option<&BTreeMap<u64, [u32; 2]>>,
    lookback: usize,
    daily_samples: u8,
) -> Vec<(u64, u8, u64)> {
    let (Some(current_epoch), Some(sampled)) = (current_epoch, sampled) else {
        return Vec::new();
    };
    let lookback = u64::try_from(lookback).unwrap_or(u64::MAX);
    let first_epoch = current_epoch.saturating_sub(lookback).max(1);
    let mut candidates = Vec::new();
    for epoch in first_epoch..current_epoch {
        let Some(spec_version) = spec_activations
            .iter()
            .filter(|(_, activation)| **activation <= epoch)
            .map(|(version, _)| *version)
            .max()
        else {
            continue;
        };
        for day in 0..daily_samples {
            let bit = 1u32 << (day % 32);
            let already_sampled = sampled
                .get(&epoch)
                .and_then(|bitmap| bitmap.get(usize::from(day / 32)))
                .is_some_and(|word| *word & bit != 0);
            if !already_sampled {
                candidates.push((epoch, day, spec_version));
            }
        }
    }
    candidates
}

fn composite_values<C>(value: &Value<C>) -> impl Iterator<Item = &Value<C>> {
    match &value.value {
        ValueDef::Composite(composite) => Some(composite.values()),
        _ => None,
    }
    .into_iter()
    .flatten()
}

fn variant_name<C>(value: &Value<C>) -> Option<&str> {
    match &value.value {
        ValueDef::Variant(variant) => Some(variant.name.as_str()),
        _ => None,
    }
}

fn variant_field<'a, C>(value: &'a Value<C>, name: &str) -> Option<&'a Value<C>> {
    match &value.value {
        ValueDef::Variant(variant) => variant.values.at(name),
        _ => None,
    }
}

fn option_inner<C>(value: &Value<C>) -> Option<&Value<C>> {
    match &value.value {
        ValueDef::Variant(variant) if variant.name == "Some" => variant.values.values().next(),
        _ => None,
    }
}

fn option_u64<C>(value: &Value<C>) -> Option<u64> {
    option_inner(value).and_then(as_u64)
}

fn as_u64<C>(value: &Value<C>) -> Option<u64> {
    u64::try_from(value.as_u128()?).ok()
}

fn nonzero(value: Option<u64>) -> Option<u64> {
    value.filter(|value| *value != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scale_info::TypeInfo;
    use subxt::ext::{codec::Encode, scale_decode::DecodeAsType, scale_value::Value};

    #[allow(dead_code)]
    #[derive(Encode, TypeInfo)]
    struct EncodedFixedU64(u64);

    #[allow(dead_code)]
    #[derive(Encode, TypeInfo)]
    enum EncodedParamValue {
        U8(u8),
        U32(u32),
        Balance(u128),
        Fixed(EncodedFixedU64),
        Percent(u8),
        Perbill(u32),
    }

    #[allow(dead_code)]
    #[derive(Encode, TypeInfo)]
    enum EncodedMaxDelta {
        Absolute(EncodedParamValue),
        Percent(u8),
        Factor(u8),
    }

    #[allow(dead_code)]
    #[derive(Encode, TypeInfo)]
    enum EncodedParamClass {
        Param,
        Treasury,
        Meta,
        Const,
        Entrenched,
        MetaAndValues,
    }

    #[derive(Encode, TypeInfo)]
    struct EncodedParamRecord {
        key: [u8; 16],
        value: EncodedParamValue,
        min: EncodedParamValue,
        max: EncodedParamValue,
        max_delta: Option<EncodedMaxDelta>,
        cooldown_epochs: u32,
        last_changed_epoch: u32,
        last_change_block: u32,
        class: EncodedParamClass,
        kernel_bounded: bool,
    }

    #[test]
    fn phase_boundaries_follow_the_frozen_fraction_grid() {
        assert_eq!(phase_boundary(100, 302_400, "Intake"), Some(43_300));
        assert_eq!(phase_boundary(100, 302_400, "Trade"), Some(259_300));
        assert_eq!(phase_boundary(100, 302_400, "Housekeeping"), Some(302_500));
    }

    #[test]
    fn option_helpers_are_non_panicking() {
        let some = Value::unnamed_variant("Some", [Value::u128(7)]);
        let none = Value::unnamed_variant("None", []);
        assert_eq!(option_u64(&some), Some(7));
        assert_eq!(option_u64(&none), None);
    }

    #[test]
    fn param_record_u32_decodes_the_hand_encoded_constitution_shape() {
        let key = param_key(b"mkt.obs_interval").expect("canonical key fits");
        let record = EncodedParamRecord {
            key,
            value: EncodedParamValue::U32(7),
            min: EncodedParamValue::U32(5),
            max: EncodedParamValue::U32(50),
            max_delta: Some(EncodedMaxDelta::Absolute(EncodedParamValue::U32(5))),
            cooldown_epochs: 1,
            last_changed_epoch: 3,
            last_change_block: 42,
            class: EncodedParamClass::Param,
            kernel_bounded: false,
        };
        let mut registry = scale_info::Registry::new();
        let type_id = registry
            .register_type(&scale_info::MetaType::new::<EncodedParamRecord>())
            .id;
        let portable: scale_info::PortableRegistry = registry.into();
        let encoded = record.encode();
        let decoded = Value::decode_as_type(&mut encoded.as_slice(), type_id, &portable)
            .expect("hand-encoded ParamRecord follows its type metadata");

        assert_eq!(param_record_u32(&decoded, &key), Some(7));
    }

    #[test]
    fn chain_param_precedence_is_override_then_live_then_default() {
        assert_eq!(resolve_chain_param(Some(7), Some(5), 10), 7);
        assert_eq!(resolve_chain_param(None, Some(5), 10), 5);
        assert_eq!(resolve_chain_param(None, None, 10), 10);
    }

    #[test]
    fn cohort_binding_decoder_requires_one_frozen_spec() {
        let same = Value::unnamed_composite([
            Value::unnamed_composite([Value::u128(1), Value::u128(7)]),
            Value::unnamed_composite([Value::u128(2), Value::u128(7)]),
        ]);
        let mixed = Value::unnamed_composite([
            Value::unnamed_composite([Value::u128(1), Value::u128(7)]),
            Value::unnamed_composite([Value::u128(2), Value::u128(8)]),
        ]);
        assert_eq!(single_cohort_spec(&same), Some(7));
        assert_eq!(single_cohort_spec(&mixed), None);
    }

    #[test]
    fn tuple_storage_key_decoder_handles_dynamic_composite() {
        let keys = [Value::unnamed_composite([Value::u128(12), Value::u128(4)])];
        assert_eq!(tuple_key_pair(&keys), Some((12, 4)));
    }

    #[test]
    fn tick_batch_uses_metadata_value_with_documented_fallback() {
        assert_eq!(resolve_tick_batch(Some(2)), 2);
        assert_eq!(resolve_tick_batch(None), DEFAULT_TICK_BATCH);
        assert_eq!(resolve_tick_batch(Some(0)), DEFAULT_TICK_BATCH);
    }

    #[test]
    fn welfare_gate_bounds_use_metadata_with_documented_fallbacks() {
        assert_eq!(resolve_welfare_lookback(Some(3)), 3);
        assert_eq!(resolve_welfare_lookback(None), DEFAULT_WELFARE_LOOKBACK);
        assert_eq!(resolve_daily_gate_samples(Some(21)), 21);
        assert_eq!(resolve_daily_gate_samples(None), DEFAULT_DAILY_GATE_SAMPLES);
    }

    #[test]
    fn gate_day_bitmap_decoder_requires_exactly_two_u32_words() {
        let bitmap = Value::unnamed_composite([Value::u128(3), Value::u128(5)]);
        let short = Value::unnamed_composite([Value::u128(3)]);
        assert_eq!(gate_day_bitmap(&bitmap), Some([3, 5]));
        assert_eq!(gate_day_bitmap(&short), None);
    }

    #[test]
    fn unsampled_daily_gate_is_due_and_sampled_day_is_not_due() {
        let activations = BTreeMap::from([(1, 1)]);
        let sampled = BTreeMap::from([(2, [1, 0])]);
        assert_eq!(
            derive_daily_gate_candidates(Some(3), &activations, Some(&sampled), 1, 2),
            vec![(2, 1, 1)]
        );
    }

    #[test]
    fn absent_sample_marker_disables_daily_gate_candidates() {
        let activations = BTreeMap::from([(1, 1)]);
        assert!(derive_daily_gate_candidates(Some(3), &activations, None, 1, 2).is_empty());
    }

    #[test]
    fn welfare_candidates_follow_cohort_frozen_spec_across_activation() {
        let activations = BTreeMap::from([(1, 1), (2, 10)]);
        let recorded = BTreeSet::from([(9, 1)]);
        let cohorts = [CohortSnapshot {
            epoch: 8,
            status: "Measuring".to_owned(),
            until_epoch: Some(10),
            cursor: None,
            metric_spec: Some(1),
        }];

        let (active, candidates) =
            derive_welfare_candidates(Some(11), &activations, &recorded, &cohorts);
        assert_eq!(active, Some(2));
        assert_eq!(candidates, vec![(10, 1), (10, 2)]);
    }
}
