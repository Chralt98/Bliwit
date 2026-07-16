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
    ext::scale_value::{At, Value, ValueDef},
    OnlineClient, PolkadotConfig,
};
use tracing::{debug, warn};

use crate::config::{Role, RoleSet};

pub const DECISION_WINDOW_BLOCKS: u64 = 43_200;
pub const STALE_OBSERVATION_GAP_BLOCKS: u64 = 50;
pub const RESERVE_PROBE_INTERVAL_BLOCKS: u64 = 14_400;
pub const RESERVE_PROBE_TIMEOUT_BLOCKS: u64 = 600;
pub const DEFAULT_TICK_BATCH: usize = 10;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChainSnapshot {
    pub current_block: u64,
    pub available_pallets: BTreeSet<String>,
    pub available_calls: BTreeSet<String>,
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
    pub fn new(client: OnlineClient<PolkadotConfig>) -> Self {
        let metadata = client.metadata();
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
        Self {
            client,
            capabilities,
            pallets,
            calls,
            transport_failed: Arc::new(AtomicBool::new(false)),
        }
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

    pub async fn extract(
        &self,
        current_block: u64,
        block_hash: HashFor<PolkadotConfig>,
    ) -> Result<ChainSnapshot, SnapshotTransportError> {
        self.transport_failed.store(false, Ordering::Relaxed);
        let epoch = self.extract_epoch(block_hash).await;
        let proposals = self.extract_proposals(block_hash).await;
        let cohorts = self.extract_cohorts(block_hash).await;
        let mut books = self.extract_books(block_hash).await;
        mark_decision_window(current_block, &proposals, &mut books);
        let market_archive = self.constant_u64("Market", "ArchiveDelay");
        let ledger_archive = self.constant_u64("ConditionalLedger", "ArchiveDelay");
        let tick_batch = resolve_tick_batch(self.constant_u64("Epoch", "TickBatch"));
        let registry_epochs = self.extract_registries(block_hash).await;

        let welfare = self
            .extract_welfare(block_hash, epoch.as_ref(), &cohorts)
            .await;
        let snapshot = ChainSnapshot {
            current_block,
            available_pallets: self.pallets.clone(),
            available_calls: self.calls.clone(),
            tick_batch: Some(tick_batch),
            epoch,
            books,
            proposals,
            cohorts,
            oracle_rounds: self.extract_oracle_rounds(block_hash).await,
            reserve_health: self.extract_reserve_health(block_hash).await,
            registry_epochs,
            execution_queue: self.extract_execution_queue(block_hash).await,
            coretime: self.extract_coretime(block_hash).await,
            market_reaps: self
                .extract_reaps(block_hash, "Market", "ClosedAt", market_archive)
                .await,
            proposal_dust: self
                .extract_reaps(
                    block_hash,
                    "ConditionalLedger",
                    "VaultTerminalAt",
                    ledger_archive,
                )
                .await,
            baseline_dust: self
                .extract_reaps(
                    block_hash,
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

    async fn extract_epoch(&self, block_hash: HashFor<PolkadotConfig>) -> Option<EpochSnapshot> {
        let value = self.fetch_value(block_hash, "Epoch", "EpochOf").await?;
        let schedule = self.fetch_value(block_hash, "Epoch", "Schedule").await;
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
        block_hash: HashFor<PolkadotConfig>,
    ) -> Vec<ProposalSnapshot> {
        self.iter_values(block_hash, "Epoch", "Proposals")
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

    async fn extract_cohorts(&self, block_hash: HashFor<PolkadotConfig>) -> Vec<CohortSnapshot> {
        let schedules = self
            .iter_values(block_hash, "Epoch", "CohortSchedules")
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
        self.iter_values(block_hash, "Epoch", "Cohorts")
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

    async fn extract_books(&self, block_hash: HashFor<PolkadotConfig>) -> Vec<BookSnapshot> {
        self.iter_values(block_hash, "Market", "Markets")
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
        block_hash: HashFor<PolkadotConfig>,
    ) -> Vec<OracleRoundSnapshot> {
        self.iter_values(block_hash, "Oracle", "Rounds")
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
        block_hash: HashFor<PolkadotConfig>,
    ) -> Option<ReserveHealthSnapshot> {
        let value = self
            .fetch_value(block_hash, "Oracle", "ReserveHealth")
            .await?;
        Some(ReserveHealthSnapshot {
            last_probe_at: value.at("last_probe_at").and_then(as_u64),
            pending_since: value.at("pending_since").and_then(option_u64),
        })
    }

    async fn extract_registries(
        &self,
        block_hash: HashFor<PolkadotConfig>,
    ) -> Vec<RegistryEpochSnapshot> {
        let mut result = Vec::new();
        for pallet in ["IncidentRegistry", "MilestoneRegistry"] {
            if !self.has_storage(pallet, "Filings") {
                continue;
            }
            let archive_delay = self.constant_u64(pallet, "ArchiveDelay");
            let mut by_epoch = BTreeMap::<u64, RegistryEpochSnapshot>::new();
            for (keys, value) in self.iter_values(block_hash, pallet, "Filings").await {
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
            for (keys, _) in self.iter_values(block_hash, pallet, "FilingCount").await {
                if let Some(epoch) = keys.first().and_then(as_u64) {
                    by_epoch
                        .entry(epoch)
                        .or_insert_with(|| registry_epoch(pallet, epoch, archive_delay))
                        .filing_count_present = true;
                }
            }
            for (keys, _) in self.iter_values(block_hash, pallet, "Aggregates").await {
                if let Some(epoch) = keys.first().and_then(as_u64) {
                    by_epoch
                        .entry(epoch)
                        .or_insert_with(|| registry_epoch(pallet, epoch, archive_delay))
                        .aggregate_present = true;
                }
            }
            for (keys, value) in self.iter_values(block_hash, pallet, "ClosedAt").await {
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
        block_hash: HashFor<PolkadotConfig>,
    ) -> Vec<ExecutionSnapshot> {
        self.iter_values(block_hash, "ExecutionGuard", "Queue")
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
        block_hash: HashFor<PolkadotConfig>,
    ) -> Option<CoretimeSnapshot> {
        let value = self
            .fetch_value(block_hash, "FutarchyTreasury", "State")
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
        block_hash: HashFor<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
        archive_delay: Option<u64>,
    ) -> Vec<ReapSnapshot> {
        self.iter_values(block_hash, pallet, storage_name)
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
        block_hash: HashFor<PolkadotConfig>,
        epoch: Option<&EpochSnapshot>,
        cohorts: &[CohortSnapshot],
    ) -> Option<WelfareSnapshot> {
        if !self.has_storage("Welfare", "MetricSpecs") {
            return None;
        }
        let metric_specs = self.iter_values(block_hash, "Welfare", "MetricSpecs").await;
        let recorded_snapshots: BTreeSet<(u64, u64)> = self
            .iter_values(block_hash, "Welfare", "Snapshots")
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
        Some(WelfareSnapshot {
            active_spec_version,
            recorded_snapshots,
            snapshot_candidates,
            // GateBreachFlags only marks breached days. A healthy recorded day has no
            // durable per-day bit, so finalized storage cannot prove non-submission.
            daily_gate_candidates: Vec::new(),
        })
    }

    async fn fetch_value(
        &self,
        block_hash: HashFor<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
    ) -> Option<Value<u32>> {
        if !self.has_storage(pallet, storage_name) {
            return None;
        }
        let address = dynamic::storage(pallet, storage_name, Vec::<Value<()>>::new());
        match self.client.storage().at(block_hash).fetch(&address).await {
            Ok(Some(value)) => match value.to_value() {
                Ok(value) => Some(value),
                Err(error) => {
                    warn!(pallet, storage = storage_name, %error, "dynamic storage decode failed");
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                self.note_transport_error(&error);
                warn!(pallet, storage = storage_name, %error, "dynamic storage read failed");
                None
            }
        }
    }

    async fn iter_values(
        &self,
        block_hash: HashFor<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
    ) -> Vec<(Vec<Value<()>>, Value<u32>)> {
        if !self.has_storage(pallet, storage_name) {
            return Vec::new();
        }
        let address = dynamic::storage(pallet, storage_name, Vec::<Value<()>>::new());
        let mut entries = match self.client.storage().at(block_hash).iter(address).await {
            Ok(entries) => entries,
            Err(error) => {
                self.note_transport_error(&error);
                warn!(pallet, storage = storage_name, %error, "dynamic storage iteration failed");
                return Vec::new();
            }
        };
        let mut values = Vec::new();
        while let Some(entry) = entries.next().await {
            match entry {
                Ok(entry) => match entry.value.to_value() {
                    Ok(value) => values.push((entry.keys, value)),
                    Err(error) => warn!(
                        pallet,
                        storage = storage_name,
                        %error,
                        "dynamic storage item decode failed"
                    ),
                },
                Err(error) => {
                    self.note_transport_error(&error);
                    warn!(pallet, storage = storage_name, %error, "dynamic storage item read failed");
                    break;
                }
            }
        }
        values
    }

    fn has_storage(&self, pallet: &str, storage_name: &str) -> bool {
        self.client
            .metadata()
            .pallet_by_name(pallet)
            .and_then(|details| details.storage())
            .and_then(|storage| storage.entry_by_name(storage_name))
            .is_some()
    }

    fn note_transport_error(&self, error: &subxt::Error) {
        if matches!(error, subxt::Error::Io(_) | subxt::Error::Rpc(_)) {
            self.transport_failed.store(true, Ordering::Relaxed);
        }
    }

    fn constant_u64(&self, pallet: &str, constant: &str) -> Option<u64> {
        if !self.pallets.contains(pallet) {
            return None;
        }
        let address = dynamic::constant(pallet, constant);
        match self.client.constants().at(&address) {
            Ok(value) => match value.to_value() {
                Ok(value) => as_u64(&value),
                Err(error) => {
                    debug!(pallet, constant, %error, "dynamic constant decode failed");
                    None
                }
            },
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
    proposals: &[ProposalSnapshot],
    books: &mut [BookSnapshot],
) {
    let critical = proposals
        .iter()
        .filter(|proposal| matches!(proposal.state.as_str(), "Trading" | "Extended"))
        .filter(|proposal| {
            proposal.decide_at.is_some_and(|decide_at| {
                current_block >= decide_at.saturating_sub(DECISION_WINDOW_BLOCKS)
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
    use subxt::ext::scale_value::Value;

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
