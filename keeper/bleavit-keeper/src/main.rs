use std::{collections::BTreeMap, str::FromStr};

use anyhow::Context;
use bleavit_keeper::{
    config::{Config, Role, RoleSet},
    metrics::KeeperMetrics,
    plan,
    planner::PlannerConfig,
    snapshot::{
        resolve_chain_param, SnapshotExtractor, DEFAULT_DECISION_WINDOW_BLOCKS,
        DEFAULT_OBSERVATION_INTERVAL_BLOCKS, DEFAULT_RESERVE_PROBE_INTERVAL_BLOCKS,
        DEFAULT_RESERVE_PROBE_TIMEOUT_BLOCKS,
    },
    submit::Submitter,
    Cli,
};
use clap::Parser;
use rand::RngExt;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::{sr25519::Keypair, SecretUri};
use tokio::{sync::watch, time::sleep};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let config = Config::load(Cli::parse()).await?;
    let metrics = KeeperMetrics::new()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    spawn_shutdown_listener(shutdown_tx.clone());
    if let Some(bind) = config.metrics_bind {
        let server_metrics = metrics.clone();
        let server_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(error) = server_metrics.serve(bind, server_shutdown).await {
                warn!(%error, %bind, "metrics endpoint stopped");
            }
        });
        info!(%bind, "metrics endpoint listening");
    }

    startup_jitter(&config, shutdown_rx.clone()).await;
    if *shutdown_rx.borrow() {
        return Ok(());
    }

    let signer = if config.dry_run {
        None
    } else {
        Some(load_signer(&config).await?)
    };
    let mut cooldowns = BTreeMap::new();
    let mut reported_roles = BTreeMap::<Role, bool>::new();
    let mut endpoint = 0_usize;
    let mut shutdown = shutdown_rx;

    while !*shutdown.borrow() {
        let url = &config.node_urls[endpoint % config.node_urls.len()];
        endpoint = endpoint.wrapping_add(1);
        info!(%url, "connecting to finalized RPC");
        let client = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
                continue;
            }
            result = OnlineClient::<PolkadotConfig>::from_url(url) => {
                match result {
                    Ok(client) => client,
                    Err(error) => {
                        metrics.set_connected(false);
                        warn!(%url, %error, "RPC connection failed; trying failover");
                        wait_or_shutdown(config.retry_base, &mut shutdown).await;
                        continue;
                    }
                }
            }
        };

        let outcome = run_connection(
            client,
            &config,
            signer.clone(),
            &metrics,
            &mut cooldowns,
            &mut reported_roles,
            &mut shutdown,
        )
        .await;
        metrics.set_connected(false);
        match outcome {
            ConnectionOutcome::Shutdown => break,
            ConnectionOutcome::Reconnect => {
                wait_or_shutdown(config.retry_base, &mut shutdown).await;
            }
        }
    }

    let _ = shutdown_tx.send(true);
    info!("keeper shutdown complete");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_connection(
    client: OnlineClient<PolkadotConfig>,
    config: &Config,
    signer: Option<Keypair>,
    metrics: &KeeperMetrics,
    shared_cooldowns: &mut BTreeMap<String, u64>,
    reported_roles: &mut BTreeMap<Role, bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> ConnectionOutcome {
    let extractor = SnapshotExtractor::new(client.clone());
    report_capabilities(config, &extractor, reported_roles);
    let enabled_roles: RoleSet = config
        .enabled_roles
        .intersection(&extractor.available_roles())
        .copied()
        .collect();
    let mut submitter = signer.map(|signer| {
        let mut submitter = Submitter::new(
            client.clone(),
            signer,
            config.tx_timeout,
            config.max_retries,
            config.retry_base,
            config.cooldown_depth,
        );
        submitter.import_cooldowns(std::mem::take(shared_cooldowns));
        submitter
    });

    let mut blocks = match client.blocks().subscribe_finalized().await {
        Ok(blocks) => blocks,
        Err(error) => {
            warn!(%error, "finalized subscription failed; reconnecting");
            restore_cooldowns(&submitter, shared_cooldowns);
            return ConnectionOutcome::Reconnect;
        }
    };
    metrics.set_connected(true);
    info!("finalized subscription connected");

    loop {
        let block = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    restore_cooldowns(&submitter, shared_cooldowns);
                    return ConnectionOutcome::Shutdown;
                }
                continue;
            }
            next = blocks.next() => {
                match next {
                    Some(Ok(block)) => block,
                    Some(Err(error)) => {
                        warn!(%error, "finalized subscription error; reconnecting");
                        restore_cooldowns(&submitter, shared_cooldowns);
                        return ConnectionOutcome::Reconnect;
                    }
                    None => {
                        warn!("finalized subscription ended; reconnecting");
                        restore_cooldowns(&submitter, shared_cooldowns);
                        return ConnectionOutcome::Reconnect;
                    }
                }
            }
        };
        let current_block = u64::from(block.number());
        metrics.set_current_block(current_block);
        if current_block % config.every_n_blocks != 0 {
            continue;
        }
        let mut snapshot = tokio::select! {
            changed = shutdown.changed() => {
                restore_cooldowns(&submitter, shared_cooldowns);
                if changed.is_err() || *shutdown.borrow() {
                    return ConnectionOutcome::Shutdown;
                }
                continue;
            }
            result = extractor.extract(current_block, block.hash()) => {
                match result {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        warn!(%error, "snapshot transport failure; reconnecting");
                        restore_cooldowns(&submitter, shared_cooldowns);
                        return ConnectionOutcome::Reconnect;
                    }
                }
            }
        };
        let planner_config = PlannerConfig {
            enabled_roles: enabled_roles.clone(),
            obs_interval: resolve_chain_param(
                config.obs_interval,
                snapshot.live_params.obs_interval,
                DEFAULT_OBSERVATION_INTERVAL_BLOCKS,
            ),
            decision_window: resolve_chain_param(
                config.decision_window,
                snapshot.live_params.decision_window,
                DEFAULT_DECISION_WINDOW_BLOCKS,
            ),
            reserve_probe_interval: resolve_chain_param(
                config.reserve_probe_interval,
                snapshot.live_params.reserve_probe_interval,
                DEFAULT_RESERVE_PROBE_INTERVAL_BLOCKS,
            ),
            reserve_probe_timeout: resolve_chain_param(
                config.reserve_probe_timeout,
                snapshot.live_params.reserve_probe_timeout,
                DEFAULT_RESERVE_PROBE_TIMEOUT_BLOCKS,
            ),
            cooldown_depth: config.cooldown_depth,
            cooldowns: submitter
                .as_ref()
                .map(|submitter| submitter.cooldowns().clone())
                .unwrap_or_default(),
        };
        snapshot.apply_decision_window(planner_config.decision_window);
        let stale_market_ids = snapshot
            .books
            .iter()
            .filter(|book| book.stale_in_decision_window)
            .map(|book| book.market_id)
            .collect::<Vec<_>>();
        metrics.set_stale_decision_window_books(Role::Observe, stale_market_ids.len());
        if !stale_market_ids.is_empty() {
            warn!(
                role = %Role::Observe,
                stale_books = stale_market_ids.len(),
                market_ids = ?stale_market_ids,
                "decision-window books exceed the TWAP staleness gap"
            );
        }
        let planned = plan(&snapshot, &planner_config);
        for crank in &planned {
            metrics.planned(crank.role);
            info!(
                role = %crank.role,
                pallet = crank.pallet,
                call = crank.call,
                priority = crank.priority,
                args = ?crank.args,
                dry_run = config.dry_run,
                "crank planned"
            );
        }
        if planned.is_empty() || config.dry_run {
            continue;
        }
        let Some(submitter) = submitter.as_mut() else {
            warn!("signer unavailable outside dry-run; reconnecting");
            return ConnectionOutcome::Reconnect;
        };
        let reconnect = tokio::select! {
            changed = shutdown.changed() => {
                *shared_cooldowns = submitter.cooldowns().clone();
                if changed.is_err() || *shutdown.borrow() {
                    return ConnectionOutcome::Shutdown;
                }
                continue;
            }
            reconnect = submitter.submit_all(&planned, current_block, metrics) => reconnect,
        };
        if reconnect {
            *shared_cooldowns = submitter.cooldowns().clone();
            return ConnectionOutcome::Reconnect;
        }
        *shared_cooldowns = submitter.cooldowns().clone();
    }
}

fn report_capabilities(
    config: &Config,
    extractor: &SnapshotExtractor,
    reported: &mut BTreeMap<Role, bool>,
) {
    for capability in extractor.capabilities() {
        if !config.enabled_roles.contains(&capability.role)
            || reported.contains_key(&capability.role)
        {
            continue;
        }
        reported.insert(capability.role, capability.available);
        if capability.available {
            info!(
                role = %capability.role,
                reason = capability.reason,
                "keeper role enabled from live metadata"
            );
        } else {
            warn!(
                role = %capability.role,
                reason = capability.reason,
                "keeper role disabled from live metadata"
            );
        }
        if capability.role == Role::Welfare
            && capability.available
            && !extractor.welfare_daily_gates_plannable()
        {
            info!(
                role = %Role::Welfare,
                call = "record_daily_gate",
                "role subtask not yet plannable: SampledGateDays absent from live metadata"
            );
        }
    }
}

fn restore_cooldowns(submitter: &Option<Submitter>, shared_cooldowns: &mut BTreeMap<String, u64>) {
    if let Some(submitter) = submitter {
        *shared_cooldowns = submitter.cooldowns().clone();
    }
}

async fn load_signer(config: &Config) -> anyhow::Result<Keypair> {
    let uri = config.signer_uri().await?;
    let uri = SecretUri::from_str(&uri).context("invalid signer secret URI")?;
    Keypair::from_uri(&uri).context("failed to derive sr25519 signer")
}

async fn startup_jitter(config: &Config, mut shutdown: watch::Receiver<bool>) {
    if config.startup_jitter.is_zero() {
        return;
    }
    let maximum = config.startup_jitter.as_secs();
    let delay = rand::rng().random_range(0..=maximum);
    if delay == 0 {
        return;
    }
    info!(delay_secs = delay, "startup jitter");
    tokio::select! {
        _ = sleep(std::time::Duration::from_secs(delay)) => {}
        _ = shutdown.changed() => {}
    }
}

async fn wait_or_shutdown(duration: std::time::Duration, shutdown: &mut watch::Receiver<bool>) {
    tokio::select! {
        _ = sleep(duration) => {}
        _ = shutdown.changed() => {}
    }
}

fn spawn_shutdown_listener(shutdown: watch::Sender<bool>) {
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown.send(true);
    });
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let term = signal(SignalKind::terminate());
        match term {
            Ok(mut term) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            warn!(%error, "ctrl-c listener failed");
                        }
                    }
                    _ = term.recv() => {}
                }
            }
            Err(error) => {
                warn!(%error, "SIGTERM listener failed; waiting for ctrl-c");
                if let Err(error) = tokio::signal::ctrl_c().await {
                    warn!(%error, "ctrl-c listener failed");
                }
            }
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        warn!(%error, "ctrl-c listener failed");
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if let Err(error) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        eprintln!("failed to initialize tracing subscriber: {error}");
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionOutcome {
    Shutdown,
    Reconnect,
}
