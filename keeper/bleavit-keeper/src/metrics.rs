use std::{net::SocketAddr, time::SystemTime};

use anyhow::Context;
use prometheus::{Encoder, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};
use tracing::{debug, warn};

use crate::config::Role;

#[derive(Clone)]
pub struct KeeperMetrics {
    registry: Registry,
    planned: IntCounterVec,
    submitted: IntCounterVec,
    succeeded: IntCounterVec,
    failed: IntCounterVec,
    last_success: IntGaugeVec,
    stale_decision_window_books: IntGaugeVec,
    connected: IntGauge,
    current_block: IntGauge,
}

impl KeeperMetrics {
    pub fn new() -> anyhow::Result<Self> {
        let registry = Registry::new();
        let planned = counter("bleavit_keeper_planned_total", "Planned cranks by role")?;
        let submitted = counter("bleavit_keeper_submitted_total", "Submitted cranks by role")?;
        let succeeded = counter(
            "bleavit_keeper_succeeded_total",
            "Successful finalized cranks by role",
        )?;
        let failed = counter(
            "bleavit_keeper_failed_total",
            "Failed crank attempts by role",
        )?;
        let last_success = IntGaugeVec::new(
            Opts::new(
                "bleavit_keeper_last_successful_crank_timestamp_seconds",
                "Unix timestamp of the last successful crank by role",
            ),
            &["role"],
        )?;
        let stale_decision_window_books = IntGaugeVec::new(
            Opts::new(
                "bleavit_keeper_stale_decision_window_books",
                "Decision-window books beyond the TWAP staleness gap by keeper role",
            ),
            &["role"],
        )?;
        let connected = IntGauge::new(
            "bleavit_keeper_connected",
            "Whether a finalized-block subscription is connected",
        )?;
        let current_block = IntGauge::new(
            "bleavit_keeper_current_block",
            "Latest finalized block observed by the keeper",
        )?;

        for metric in [
            planned.clone(),
            submitted.clone(),
            succeeded.clone(),
            failed.clone(),
        ] {
            registry.register(Box::new(metric))?;
        }
        registry.register(Box::new(last_success.clone()))?;
        registry.register(Box::new(stale_decision_window_books.clone()))?;
        registry.register(Box::new(connected.clone()))?;
        registry.register(Box::new(current_block.clone()))?;
        for role in Role::ALL {
            let label = role.as_str();
            planned.with_label_values(&[label]);
            submitted.with_label_values(&[label]);
            succeeded.with_label_values(&[label]);
            failed.with_label_values(&[label]);
            last_success.with_label_values(&[label]).set(0);
            stale_decision_window_books
                .with_label_values(&[label])
                .set(0);
        }

        Ok(Self {
            registry,
            planned,
            submitted,
            succeeded,
            failed,
            last_success,
            stale_decision_window_books,
            connected,
            current_block,
        })
    }

    pub fn planned(&self, role: Role) {
        self.planned.with_label_values(&[role.as_str()]).inc();
    }

    pub fn submitted(&self, role: Role) {
        self.submitted.with_label_values(&[role.as_str()]).inc();
    }

    pub fn succeeded(&self, role: Role) {
        self.succeeded.with_label_values(&[role.as_str()]).inc();
        let unix = SystemTime::UNIX_EPOCH
            .elapsed()
            .ok()
            .and_then(|duration| i64::try_from(duration.as_secs()).ok())
            .unwrap_or(0);
        self.last_success
            .with_label_values(&[role.as_str()])
            .set(unix);
    }

    pub fn failed(&self, role: Role) {
        self.failed.with_label_values(&[role.as_str()]).inc();
    }

    pub fn set_stale_decision_window_books(&self, role: Role, books: usize) {
        self.stale_decision_window_books
            .with_label_values(&[role.as_str()])
            .set(i64::try_from(books).unwrap_or(i64::MAX));
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.set(i64::from(connected));
    }

    pub fn set_current_block(&self, block: u64) {
        self.current_block
            .set(i64::try_from(block).unwrap_or(i64::MAX));
    }

    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        let mut body = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut body)
            .context("failed to encode Prometheus metrics")?;
        Ok(body)
    }

    pub async fn serve(
        self,
        bind: SocketAddr,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let listener = TcpListener::bind(bind)
            .await
            .with_context(|| format!("failed to bind metrics endpoint at {bind}"))?;
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let metrics = self.clone();
                            tokio::spawn(async move {
                                if let Err(error) = metrics.respond(stream).await {
                                    debug!(%error, "metrics request failed");
                                }
                            });
                        }
                        Err(error) => warn!(%error, "metrics accept failed"),
                    }
                }
            }
        }
    }

    async fn respond(&self, mut stream: TcpStream) -> anyhow::Result<()> {
        let mut request = [0_u8; 1_024];
        let size = stream.read(&mut request).await?;
        let is_metrics =
            request[..size].starts_with(b"GET /metrics ") || request[..size].starts_with(b"GET / ");
        let (status, body) = if is_metrics {
            ("200 OK", self.encode()?)
        } else {
            ("404 Not Found", b"not found\n".to_vec())
        };
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(&body).await?;
        stream.shutdown().await?;
        Ok(())
    }
}

fn counter(name: &str, help: &str) -> anyhow::Result<IntCounterVec> {
    IntCounterVec::new(Opts::new(name, help), &["role"])
        .with_context(|| format!("failed to create metric {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_smoke_test_has_keeper_rows() {
        let metrics = KeeperMetrics::new().expect("metrics should initialize");
        metrics.set_connected(true);
        metrics.set_current_block(42);
        metrics.planned(Role::Tick);
        metrics.submitted(Role::Tick);
        metrics.succeeded(Role::Tick);
        metrics.failed(Role::Observe);
        metrics.set_stale_decision_window_books(Role::Observe, 3);
        let text = String::from_utf8(metrics.encode().expect("metrics should encode"))
            .expect("Prometheus text is UTF-8");
        assert!(text.contains("bleavit_keeper_planned_total{role=\"tick\"} 1"));
        assert!(text.contains("bleavit_keeper_submitted_total{role=\"tick\"} 1"));
        assert!(text.contains("bleavit_keeper_succeeded_total{role=\"tick\"} 1"));
        assert!(text.contains("bleavit_keeper_failed_total{role=\"observe\"} 1"));
        assert!(text.contains("bleavit_keeper_current_block 42"));
        assert!(text.contains("bleavit_keeper_connected 1"));
        assert!(text.contains("bleavit_keeper_stale_decision_window_books{role=\"observe\"} 3"));
        assert!(
            text.contains("bleavit_keeper_last_successful_crank_timestamp_seconds{role=\"tick\"}")
        );
    }
}
