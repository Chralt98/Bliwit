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
        codec::{Compact, Decode},
        scale_decode::DecodeAsType,
        scale_value::{At, Value, ValueDef},
    },
    ArcMetadata, Metadata, OnlineClient, OnlineClientAtBlock, PolkadotConfig,
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
    /// True when either epoch proposal map could not be read or decoded in
    /// full. Absence-dependent planning must fail closed when this is set.
    pub proposal_snapshot_incomplete: bool,
    pub cohorts: Vec<CohortSnapshot>,
    /// True when the live cohort map could not be read or decoded in full.
    /// The orphan finalizer also depends on proving cohort absence.
    pub cohort_snapshot_incomplete: bool,
    pub oracle_rounds: Vec<OracleRoundSnapshot>,
    pub reserve_health: Option<ReserveHealthSnapshot>,
    pub registry_epochs: Vec<RegistryEpochSnapshot>,
    pub execution_queue: Vec<ExecutionSnapshot>,
    pub qualified_recovery_proposals: BTreeSet<u64>,
    pub coretime: Option<CoretimeSnapshot>,
    pub market_reaps: Vec<ReapSnapshot>,
    pub proposal_dust: Vec<ReapSnapshot>,
    pub baseline_dust: Vec<ReapSnapshot>,
    pub baseline_vaults: Vec<BaselineVaultSnapshot>,
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
    pub coretime_quote_ttl: Option<u64>,
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
    pub class: String,
    pub state: String,
    pub epoch: Option<u64>,
    pub payload_hash: [u8; 32],
    pub payload_len: u32,
    /// True only when the committed proposal preimage contains the exact
    /// top-level primary-authorization and recovery-descriptor pair consumed
    /// by `ExecutionGuard.qualify_recovery_image`.
    pub recovery_qualification_required: bool,
    pub decide_at: Option<u64>,
    pub maturity: Option<u64>,
    pub grace_end: Option<u64>,
    pub market_ids: Vec<u64>,
}

/// One `ConditionalLedger::BaselineVaults` entry (03 §2.2). Only the epoch key
/// and whether the vault is still `Open` matter to the planner: an `Open` vault
/// is the necessary condition for 05 §7(6)'s `finalize_epoch_baseline` to do
/// anything at all, and the call is a documented no-op once it is `Settled`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BaselineVaultSnapshot {
    pub epoch: u64,
    pub open: bool,
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
    pub quotes: Vec<CoretimeQuoteSnapshot>,
    pub funded_periods: BTreeSet<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoretimeQuoteSnapshot {
    pub period_index: u32,
    pub price: u128,
    pub noted_at: u64,
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
                has_call("Epoch", "settle_cohort") || has_call("Epoch", "finalize_epoch_baseline"),
                "Epoch settlement calls absent",
            ),
            capability(
                Role::Execute,
                [
                    "execute",
                    "expire_failed_execution",
                    "reject_stale",
                    "qualify_recovery_image",
                ]
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
                has_call("FutarchyTreasury", "execute_coretime_renewal")
                    || has_call("FutarchyTreasury", "prune_coretime_quote"),
                "treasury renewal calls absent",
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
        let (proposals, proposal_snapshot_incomplete) = self.extract_proposals(&at_block).await;
        let (cohorts, cohort_snapshot_incomplete) = self.extract_cohorts(&at_block).await;
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
            proposal_snapshot_incomplete,
            cohorts,
            cohort_snapshot_incomplete,
            oracle_rounds: self.extract_oracle_rounds(&at_block).await,
            reserve_health: self.extract_reserve_health(&at_block).await,
            registry_epochs,
            execution_queue: self.extract_execution_queue(&at_block).await,
            qualified_recovery_proposals: self
                .iter_values(&at_block, "ExecutionGuard", "QualifiedRecoveryImages")
                .await
                .into_iter()
                .filter_map(|(keys, _)| keys.first().and_then(as_u64))
                .collect(),
            coretime: self.extract_coretime(&at_block).await,
            market_reaps: self
                .extract_reaps(&at_block, "Market", "SettlementObservedAt", market_archive)
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
            baseline_vaults: self.extract_baseline_vaults(&at_block).await,
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
        let (
            obs_interval,
            decision_window,
            reserve_probe_interval,
            reserve_probe_timeout,
            coretime_quote_ttl,
        ) = tokio::join!(
            self.fetch_u32_param(at_block, b"mkt.obs_interval"),
            self.fetch_u32_param(at_block, b"dec.window"),
            self.fetch_u32_param(at_block, b"res.probe_int"),
            self.fetch_u32_param(at_block, b"res.probe_to"),
            self.fetch_u32_param(at_block, b"ops.ct_quote_ttl"),
        );
        LivePlannerParams {
            obs_interval,
            decision_window,
            reserve_probe_interval,
            reserve_probe_timeout,
            coretime_quote_ttl,
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
    ) -> (Vec<ProposalSnapshot>, bool) {
        let (intake, intake_incomplete) = self
            .iter_values_checked(at_block, "Epoch", "IntakeProposals")
            .await;
        let (live, live_incomplete) = self
            .iter_values_checked(at_block, "Epoch", "Proposals")
            .await;
        let (mut proposals, merge_incomplete) = merge_proposal_entries(intake, live);
        for proposal in &mut proposals {
            if matches!(
                proposal.state.as_str(),
                "Submitted" | "Screening" | "Qualified" | "Trading" | "Extended"
            ) && matches!(proposal.class.as_str(), "Code" | "Meta")
            {
                proposal.recovery_qualification_required = self
                    .proposal_preimage_requires_recovery(at_block, proposal)
                    .await;
            }
        }
        (
            proposals,
            intake_incomplete || live_incomplete || merge_incomplete,
        )
    }

    async fn proposal_preimage_requires_recovery(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        proposal: &ProposalSnapshot,
    ) -> bool {
        let key = preimage_for_storage_key(&proposal.payload_hash, proposal.payload_len);
        let Some(encoded) = self
            .fetch_value_with_keys(at_block, "Preimage", "PreimageFor", vec![key])
            .await
            .and_then(|value| bytes_from_value(&value))
        else {
            return false;
        };
        let Some(max_calls) = self
            .constant_u64(at_block, "ExecutionGuard", "MaxCalls")
            .and_then(|value| u32::try_from(value).ok())
        else {
            return false;
        };
        batch_has_recovery_descriptor(&encoded, at_block.metadata_ref(), max_calls)
    }

    async fn extract_cohorts(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> (Vec<CohortSnapshot>, bool) {
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
        let (entries, iteration_incomplete) =
            self.iter_values_checked(at_block, "Epoch", "Cohorts").await;
        let (cohorts, decode_incomplete) = decode_cohort_entries(entries, &schedules);
        (cohorts, iteration_incomplete || decode_incomplete)
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
            .map(coretime_quotes)
            .unwrap_or_default();
        let funded_periods = value
            .at("funded_coretime_periods")
            .map(composite_u32s)
            .unwrap_or_default()
            .into_iter()
            .collect();
        Some(CoretimeSnapshot {
            quotes,
            funded_periods,
        })
    }

    /// `ConditionalLedger::BaselineVaults` (03 §2.2), the epoch-keyed Baseline
    /// vault map. Fails closed: an absent storage entry or an undecodable
    /// `state` field yields no candidate, so the 05 §7(6) crank is never
    /// planned on a guess.
    async fn extract_baseline_vaults(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
    ) -> Vec<BaselineVaultSnapshot> {
        self.iter_values(at_block, "ConditionalLedger", "BaselineVaults")
            .await
            .into_iter()
            .filter_map(|(keys, value)| {
                Some(BaselineVaultSnapshot {
                    epoch: keys.first().and_then(as_u64)?,
                    open: variant_name(value.at("state")?)? == "Open",
                })
            })
            .collect()
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
        self.iter_values_checked(at_block, pallet, storage_name)
            .await
            .0
    }

    /// Return every decoded map entry plus a fail-closed completeness bit.
    /// Most planners act only on entries that are present, but the orphan
    /// Baseline crank depends on proving an entry is absent from two maps.
    async fn iter_values_checked(
        &self,
        at_block: &OnlineClientAtBlock<PolkadotConfig>,
        pallet: &str,
        storage_name: &str,
    ) -> (Vec<(Vec<Value<()>>, Value<()>)>, bool) {
        if !self.has_storage(pallet, storage_name) {
            return (Vec::new(), true);
        }
        let address = dynamic::storage::<Vec<Value<()>>, Value<()>>(pallet, storage_name);
        let entry = match at_block.storage().entry(address) {
            Ok(entry) => entry,
            Err(error) => {
                warn!(pallet, storage = storage_name, %error, "dynamic storage entry unavailable");
                return (Vec::new(), true);
            }
        };
        let mut entries = match entry.iter(Vec::<Value<()>>::new()).await {
            Ok(entries) => entries,
            Err(error) => {
                let error = subxt::Error::from(error);
                self.note_transport_error(&error);
                warn!(pallet, storage = storage_name, %error, "dynamic storage iteration failed");
                return (Vec::new(), true);
            }
        };
        let mut values = Vec::new();
        let mut incomplete = false;
        while let Some(entry) = entries.next().await {
            match entry {
                Ok(entry) => {
                    let keys = match entry.key().and_then(|key| key.decode()) {
                        Ok(keys) => keys,
                        Err(error) => {
                            warn!(pallet, storage = storage_name, %error, "dynamic storage key decode failed");
                            incomplete = true;
                            break;
                        }
                    };
                    match entry.value().decode() {
                        Ok(value) => values.push((keys, value)),
                        Err(error) => {
                            incomplete = true;
                            warn!(
                                pallet,
                                storage = storage_name,
                                %error,
                                "dynamic storage item decode failed"
                            );
                        }
                    }
                }
                Err(error) => {
                    let error = subxt::Error::from(error);
                    self.note_transport_error(&error);
                    warn!(pallet, storage = storage_name, %error, "dynamic storage item read failed");
                    incomplete = true;
                    break;
                }
            }
        }
        (values, incomplete)
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

fn decode_proposal_entry(keys: &[Value<()>], value: &Value<()>) -> Option<ProposalSnapshot> {
    let proposal_id = value
        .at("id")
        .and_then(as_u64)
        .or_else(|| keys.first().and_then(as_u64))?;
    let state = value.at("state").and_then(variant_name)?.to_owned();
    let class = value.at("class").and_then(variant_name)?.to_owned();
    // Baseline finalization is absence-dependent: without the proposal's
    // epoch the keeper cannot prove that this entry belongs to some other
    // epoch. Treat a missing or undecodable epoch as an incomplete snapshot.
    let epoch = value.at("epoch").and_then(as_u64)?;
    let payload_hash = value.at("payload_hash").and_then(fixed_32_from_value)?;
    let payload_len = u32::try_from(value.at("payload_len").and_then(as_u64)?).ok()?;
    let market_ids = value
        .at("markets")
        .and_then(option_inner)
        .map(market_set_ids)
        .unwrap_or_default();
    Some(ProposalSnapshot {
        proposal_id,
        class,
        state,
        epoch: Some(epoch),
        payload_hash,
        payload_len,
        recovery_qualification_required: false,
        decide_at: nonzero(value.at("decide_at").and_then(as_u64)),
        maturity: value.at("maturity").and_then(option_u64),
        grace_end: value.at("grace_end").and_then(option_u64),
        market_ids,
    })
}

/// Merge the epoch pallet's disjoint proposal maps into the keeper's canonical
/// view. The maps are invariant-disjoint, so a duplicate id is corrupt state.
/// A duplicate or undecodable entry makes the view incomplete, which suppresses
/// absence-dependent planning. On a duplicate, retain a non-terminal entry if
/// either map has one so other planners also take the conservative view. The
/// `BTreeMap` makes de-duplication and output order independent of iteration.
fn merge_proposal_entries(
    intake: impl IntoIterator<Item = (Vec<Value<()>>, Value<()>)>,
    live: impl IntoIterator<Item = (Vec<Value<()>>, Value<()>)>,
) -> (Vec<ProposalSnapshot>, bool) {
    let mut proposals = BTreeMap::new();
    let mut incomplete = false;
    for (keys, value) in intake.into_iter().chain(live) {
        if let Some(proposal) = decode_proposal_entry(&keys, &value) {
            match proposals.entry(proposal.proposal_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(proposal);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    incomplete = true;
                    if proposal_snapshot_terminal(entry.get())
                        && !proposal_snapshot_terminal(&proposal)
                    {
                        entry.insert(proposal);
                    }
                }
            }
        } else {
            incomplete = true;
        }
    }
    (proposals.into_values().collect(), incomplete)
}

fn proposal_snapshot_terminal(proposal: &ProposalSnapshot) -> bool {
    matches!(
        proposal.state.as_str(),
        "Settled" | "Cancelled" | "Expired" | "Rejected"
    )
}

fn decode_cohort_entries(
    entries: impl IntoIterator<Item = (Vec<Value<()>>, Value<()>)>,
    schedules: &BTreeMap<u64, Option<u64>>,
) -> (Vec<CohortSnapshot>, bool) {
    let mut cohorts = Vec::new();
    let mut incomplete = false;
    for (keys, value) in entries {
        let decoded = (|| {
            let epoch = value
                .at("epoch")
                .and_then(as_u64)
                .or_else(|| keys.first().and_then(as_u64))?;
            let status_value = value.at("status")?;
            Some(CohortSnapshot {
                epoch,
                status: variant_name(status_value)?.to_owned(),
                until_epoch: variant_field(status_value, "until_epoch").and_then(as_u64),
                cursor: variant_field(status_value, "cursor").and_then(as_u64),
                metric_spec: schedules.get(&epoch).copied().flatten(),
            })
        })();
        if let Some(cohort) = decoded {
            cohorts.push(cohort);
        } else {
            incomplete = true;
        }
    }
    (cohorts, incomplete)
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

fn coretime_quotes<C>(value: &Value<C>) -> Vec<CoretimeQuoteSnapshot> {
    composite_values(value)
        .filter_map(|quote| {
            Some(CoretimeQuoteSnapshot {
                period_index: u32::try_from(quote.at("period_index").and_then(as_u64)?).ok()?,
                price: quote.at("price")?.as_u128()?,
                noted_at: quote.at("noted_at").and_then(as_u64)?,
            })
        })
        .collect()
}

fn composite_u32s<C>(value: &Value<C>) -> Vec<u32> {
    composite_values(value)
        .filter_map(|value| u32::try_from(as_u64(value)?).ok())
        .collect()
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

fn fixed_32_from_value<C>(value: &Value<C>) -> Option<[u8; 32]> {
    composite_values(value)
        .map(|byte| u8::try_from(as_u64(byte)?).ok())
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

fn bytes_from_value<C>(value: &Value<C>) -> Option<Vec<u8>> {
    composite_values(value)
        .map(|byte| u8::try_from(as_u64(byte)?).ok())
        .collect()
}

fn preimage_for_storage_key(hash: &[u8; 32], len: u32) -> Value<()> {
    let hash = Value::unnamed_composite(hash.iter().map(|byte| Value::u128(u128::from(*byte))));
    Value::unnamed_composite([hash, Value::u128(u128::from(len))])
}

fn variant_child<C>(value: &Value<C>) -> Option<&Value<C>> {
    match &value.value {
        ValueDef::Variant(variant) => variant.values.values().next(),
        _ => None,
    }
}

fn runtime_call_name<C>(value: &Value<C>) -> Option<(&str, &str)> {
    let pallet = variant_name(value)?;
    let call = variant_child(value).and_then(variant_name)?;
    Some((pallet, call))
}

fn record_recovery_leaf<C>(
    call: &Value<C>,
    authorize_upgrade: &mut bool,
    commit_recovery_image: &mut bool,
) -> bool {
    match runtime_call_name(call) {
        Some(("System", "authorize_upgrade")) if !*authorize_upgrade => {
            *authorize_upgrade = true;
            true
        }
        Some(("ExecutionGuard", "commit_recovery_image")) if !*commit_recovery_image => {
            *commit_recovery_image = true;
            true
        }
        Some(("System", "authorize_upgrade"))
        | Some(("ExecutionGuard", "commit_recovery_image")) => false,
        _ => true,
    }
}

/// Decode the committed `Vec<RuntimeCall>` with live metadata and recognize the
/// two distinct top-level leaves required by the recovery qualifier. Other
/// calls remain legal in the bounded upgrade batch (for example the committed
/// release-channel/cap updates); duplicate descriptor leaves, malformed,
/// oversized, or trailing-byte payloads fail closed and are never planned.
fn batch_has_recovery_descriptor(bytes: &[u8], metadata: &Metadata, max_calls: u32) -> bool {
    let mut input = bytes;
    let Ok(Compact(call_count)) = Compact::<u32>::decode(&mut input) else {
        return false;
    };
    if call_count > max_calls {
        return false;
    }
    let call_ty = metadata.outer_enums().call_enum_ty();
    let mut authorize_upgrade = false;
    let mut commit_recovery_image = false;
    for _ in 0..call_count {
        let Ok(call) = Value::decode_as_type(&mut input, call_ty, metadata.types()) else {
            return false;
        };
        if !record_recovery_leaf(&call, &mut authorize_upgrade, &mut commit_recovery_image) {
            return false;
        }
    }
    input.is_empty() && authorize_upgrade && commit_recovery_image
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
    use crate::planner::{plan, PlannerConfig};
    use scale_info::TypeInfo;
    use subxt::ext::{
        codec::Encode, scale_decode::DecodeAsType, scale_encode::EncodeAsType, scale_value::Value,
    };

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

    fn proposal_value(proposal_id: u64, state: &str, epoch: u64, decide_at: u64) -> Value<()> {
        Value::named_composite([
            ("id", Value::u128(u128::from(proposal_id))),
            ("class", Value::unnamed_variant("Param", [])),
            ("state", Value::unnamed_variant(state, [])),
            ("epoch", Value::u128(u128::from(epoch))),
            (
                "payload_hash",
                Value::unnamed_composite((0u8..32).map(|byte| Value::u128(u128::from(byte)))),
            ),
            ("payload_len", Value::u128(0)),
            ("decide_at", Value::u128(u128::from(decide_at))),
            ("maturity", Value::unnamed_variant("None", [])),
            ("grace_end", Value::unnamed_variant("None", [])),
            ("markets", Value::unnamed_variant("None", [])),
        ])
    }

    fn actual_runtime_metadata() -> Metadata {
        // Extracted from this repository's bootstrap runtime Wasm with:
        // `subwasm metadata --format scale`. Keeping the SCALE artifact in the
        // keeper workspace exercises Subxt against the production outer-call
        // and Preimage storage types without coupling its isolated dependency
        // graph to the runtime workspace.
        let encoded = include_bytes!("../tests/fixtures/runtime-metadata.scale");
        Metadata::decode(&mut &encoded[..]).expect("actual runtime metadata decodes for Subxt")
    }

    fn hash_value(byte: u8) -> Value<()> {
        Value::unnamed_composite(
            [byte; 32]
                .into_iter()
                .map(|byte| Value::u128(u128::from(byte))),
        )
    }

    fn encode_runtime_batch(
        metadata: &Metadata,
        calls: impl IntoIterator<Item = Value<()>>,
    ) -> Vec<u8> {
        let calls = calls.into_iter().collect::<Vec<_>>();
        let mut encoded =
            Compact(u32::try_from(calls.len()).expect("test batch fits u32")).encode();
        let call_ty = metadata.outer_enums().call_enum_ty();
        for call in calls {
            encoded.extend(
                call.encode_as_type(call_ty, metadata.types())
                    .expect("call shape follows actual runtime metadata"),
            );
        }
        encoded
    }

    fn authorize_upgrade_call() -> Value<()> {
        Value::unnamed_variant(
            "System",
            [Value::named_variant(
                "authorize_upgrade",
                [("code_hash", hash_value(0x11))],
            )],
        )
    }

    fn commit_recovery_image_call() -> Value<()> {
        Value::unnamed_variant(
            "ExecutionGuard",
            [Value::named_variant(
                "commit_recovery_image",
                [
                    ("hash", hash_value(0x22)),
                    ("len", Value::u128(1_024)),
                    ("target_spec_version", Value::u128(43)),
                    ("attestation_id", Value::u128(7)),
                ],
            )],
        )
    }

    fn ordinary_meta_call() -> Value<()> {
        Value::unnamed_variant(
            "Constitution",
            [Value::named_variant(
                "set_phase_flag",
                [
                    ("flag", Value::u128(1 << 3)),
                    ("enabled", Value::bool(true)),
                ],
            )],
        )
    }

    #[test]
    fn actual_runtime_metadata_decodes_exact_recovery_preimage_and_rejects_meta_or_malformed() {
        let metadata = actual_runtime_metadata();
        let exact = encode_runtime_batch(
            &metadata,
            [authorize_upgrade_call(), commit_recovery_image_call()],
        );
        assert!(batch_has_recovery_descriptor(&exact, &metadata, 16));

        let ordinary_meta = encode_runtime_batch(&metadata, [ordinary_meta_call()]);
        assert!(!batch_has_recovery_descriptor(
            &ordinary_meta,
            &metadata,
            16
        ));

        let mixed = encode_runtime_batch(
            &metadata,
            [
                authorize_upgrade_call(),
                commit_recovery_image_call(),
                ordinary_meta_call(),
            ],
        );
        assert!(
            batch_has_recovery_descriptor(&mixed, &metadata, 16),
            "the required pair may coexist with other bounded upgrade calls"
        );

        let mut malformed = exact;
        malformed.push(0xff);
        assert!(!batch_has_recovery_descriptor(&malformed, &metadata, 16));
    }

    #[test]
    fn actual_runtime_preimage_key_is_one_scale_tuple_key() {
        let metadata = actual_runtime_metadata();
        let entry = metadata
            .pallet_by_name("Preimage")
            .and_then(|pallet| pallet.storage())
            .and_then(|storage| storage.entry_by_name("PreimageFor"))
            .expect("actual runtime exposes Preimage.PreimageFor");
        let keys = entry.keys().collect::<Vec<_>>();
        assert_eq!(keys.len(), 1, "PreimageFor is keyed by one tuple");

        let hash = [0x42; 32];
        let len = 2_048;
        let dynamically_encoded = preimage_for_storage_key(&hash, len)
            .encode_as_type(keys[0].key_id, metadata.types())
            .expect("snapshot key follows actual PreimageFor tuple metadata");
        assert_eq!(dynamically_encoded, (hash, len).encode());
    }

    #[test]
    fn recovery_leaf_detection_requires_one_exact_top_level_pair() {
        let authorize =
            Value::unnamed_variant("System", [Value::unnamed_variant("authorize_upgrade", [])]);
        let recovery = Value::unnamed_variant(
            "ExecutionGuard",
            [Value::unnamed_variant("commit_recovery_image", [])],
        );
        let ordinary_meta = Value::unnamed_variant(
            "Constitution",
            [Value::unnamed_variant("amend_registry", [])],
        );

        let mut saw_authorize = false;
        let mut saw_recovery = false;
        assert!(record_recovery_leaf(
            &ordinary_meta,
            &mut saw_authorize,
            &mut saw_recovery
        ));
        assert!(!saw_authorize && !saw_recovery);
        assert!(record_recovery_leaf(
            &authorize,
            &mut saw_authorize,
            &mut saw_recovery
        ));
        assert!(record_recovery_leaf(
            &recovery,
            &mut saw_authorize,
            &mut saw_recovery
        ));
        assert!(saw_authorize && saw_recovery);
        assert!(!record_recovery_leaf(
            &recovery,
            &mut saw_authorize,
            &mut saw_recovery
        ));
    }

    fn proposal_entry(proposal_id: u64, state: &str) -> (Vec<Value<()>>, Value<()>) {
        (
            vec![Value::u128(u128::from(proposal_id))],
            proposal_value(proposal_id, state, 3, 50),
        )
    }

    #[test]
    fn merged_proposals_include_intake_only_submitted_and_screening_entries() {
        let (proposals, incomplete) = merge_proposal_entries(
            vec![
                proposal_entry(2, "Screening"),
                proposal_entry(1, "Submitted"),
            ],
            Vec::new(),
        );

        assert!(!incomplete);
        assert_eq!(
            proposals
                .iter()
                .map(|proposal| (proposal.proposal_id, proposal.state.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "Submitted"), (2, "Screening")]
        );
    }

    #[test]
    fn merged_proposals_include_ordinary_live_entries() {
        let (proposals, incomplete) = merge_proposal_entries(
            Vec::new(),
            vec![proposal_entry(9, "Trading"), proposal_entry(7, "Qualified")],
        );

        assert!(!incomplete);
        assert_eq!(
            proposals
                .iter()
                .map(|proposal| (proposal.proposal_id, proposal.state.as_str()))
                .collect::<Vec<_>>(),
            vec![(7, "Qualified"), (9, "Trading")]
        );
    }

    #[test]
    fn corrupt_collision_prefers_non_terminal_and_blocks_finalization() {
        let intake = vec![(vec![Value::u128(11)], proposal_value(11, "Settled", 3, 50))];
        let live = vec![(vec![Value::u128(11)], proposal_value(11, "Trading", 3, 75))];
        let (proposals, incomplete) = merge_proposal_entries(intake, live);

        assert!(incomplete);
        assert_eq!(
            proposals,
            vec![ProposalSnapshot {
                proposal_id: 11,
                class: "Param".to_owned(),
                state: "Trading".to_owned(),
                epoch: Some(3),
                payload_hash: core::array::from_fn(|index| index as u8),
                payload_len: 0,
                recovery_qualification_required: false,
                decide_at: Some(75),
                maturity: None,
                grace_end: None,
                market_ids: Vec::new(),
            }]
        );

        let snapshot = ChainSnapshot {
            available_calls: BTreeSet::from(["Epoch.finalize_epoch_baseline".to_owned()]),
            epoch: Some(EpochSnapshot {
                index: 4,
                phase: "Intake".to_owned(),
                phase_start_block: 100,
                epoch_start_block: None,
                length: None,
                next_boundary: None,
            }),
            proposals,
            proposal_snapshot_incomplete: incomplete,
            baseline_vaults: vec![BaselineVaultSnapshot {
                epoch: 3,
                open: true,
            }],
            ..ChainSnapshot::default()
        };
        assert!(!plan(&snapshot, &PlannerConfig::default())
            .iter()
            .any(|crank| crank.call == "finalize_epoch_baseline"));
    }

    #[test]
    fn malformed_proposal_entries_mark_snapshot_incomplete_and_block_finalization() {
        let missing_id = (
            Vec::new(),
            Value::named_composite([("state", Value::unnamed_variant("Submitted", []))]),
        );
        let malformed_state = (
            vec![Value::u128(2)],
            Value::named_composite([("id", Value::u128(2)), ("state", Value::u128(99))]),
        );

        let (proposals, incomplete) = merge_proposal_entries(
            vec![missing_id, proposal_entry(3, "Settled")],
            vec![malformed_state, proposal_entry(4, "Cancelled")],
        );
        assert!(incomplete);
        assert_eq!(
            proposals
                .iter()
                .map(|proposal| proposal.proposal_id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );

        let snapshot = ChainSnapshot {
            available_calls: BTreeSet::from(["Epoch.finalize_epoch_baseline".to_owned()]),
            epoch: Some(EpochSnapshot {
                index: 4,
                phase: "Intake".to_owned(),
                phase_start_block: 100,
                epoch_start_block: None,
                length: None,
                next_boundary: None,
            }),
            proposals,
            proposal_snapshot_incomplete: incomplete,
            baseline_vaults: vec![BaselineVaultSnapshot {
                epoch: 3,
                open: true,
            }],
            ..ChainSnapshot::default()
        };
        assert!(!plan(&snapshot, &PlannerConfig::default())
            .iter()
            .any(|crank| crank.call == "finalize_epoch_baseline"));
    }

    #[test]
    fn proposal_missing_epoch_marks_snapshot_incomplete_and_blocks_finalization() {
        let missing_epoch = (
            vec![Value::u128(2)],
            Value::named_composite([
                ("id", Value::u128(2)),
                ("state", Value::unnamed_variant("Screening", [])),
            ]),
        );
        let (proposals, incomplete) = merge_proposal_entries(vec![missing_epoch], Vec::new());
        assert!(proposals.is_empty());
        assert!(incomplete);

        let snapshot = ChainSnapshot {
            available_calls: BTreeSet::from(["Epoch.finalize_epoch_baseline".to_owned()]),
            epoch: Some(EpochSnapshot {
                index: 4,
                phase: "Intake".to_owned(),
                phase_start_block: 100,
                epoch_start_block: None,
                length: None,
                next_boundary: None,
            }),
            proposals,
            proposal_snapshot_incomplete: incomplete,
            baseline_vaults: vec![BaselineVaultSnapshot {
                epoch: 3,
                open: true,
            }],
            ..ChainSnapshot::default()
        };
        assert!(!plan(&snapshot, &PlannerConfig::default())
            .iter()
            .any(|crank| crank.call == "finalize_epoch_baseline"));
    }

    #[test]
    fn malformed_cohort_entry_marks_snapshot_incomplete_and_blocks_finalization() {
        let malformed = (
            vec![Value::u128(3)],
            Value::named_composite([("epoch", Value::u128(3)), ("status", Value::u128(99))]),
        );
        let (cohorts, incomplete) = decode_cohort_entries(vec![malformed], &BTreeMap::new());
        assert!(cohorts.is_empty());
        assert!(incomplete);

        let snapshot = ChainSnapshot {
            available_calls: BTreeSet::from(["Epoch.finalize_epoch_baseline".to_owned()]),
            epoch: Some(EpochSnapshot {
                index: 4,
                phase: "Intake".to_owned(),
                phase_start_block: 100,
                epoch_start_block: None,
                length: None,
                next_boundary: None,
            }),
            cohort_snapshot_incomplete: incomplete,
            baseline_vaults: vec![BaselineVaultSnapshot {
                epoch: 3,
                open: true,
            }],
            ..ChainSnapshot::default()
        };
        assert!(!plan(&snapshot, &PlannerConfig::default())
            .iter()
            .any(|crank| crank.call == "finalize_epoch_baseline"));
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
    fn named_coretime_quotes_decode_period_price_and_noted_at() {
        let value = Value::unnamed_composite([
            Value::named_composite([
                ("period_index", Value::u128(12)),
                ("price", Value::u128(1_000)),
                ("noted_at", Value::u128(900)),
            ]),
            Value::named_composite([
                ("period_index", Value::u128(13)),
                ("price", Value::u128(2_000)),
                ("noted_at", Value::u128(950)),
            ]),
        ]);

        assert_eq!(
            coretime_quotes(&value),
            vec![
                CoretimeQuoteSnapshot {
                    period_index: 12,
                    price: 1_000,
                    noted_at: 900,
                },
                CoretimeQuoteSnapshot {
                    period_index: 13,
                    price: 2_000,
                    noted_at: 950,
                },
            ]
        );
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
