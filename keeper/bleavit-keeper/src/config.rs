use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{bail, Context};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

const DEFAULT_NODE_URL: &str = "ws://127.0.0.1:9944";

#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize, ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Tick,
    Observe,
    Decide,
    Settle,
    Execute,
    OracleClose,
    RegistryClose,
    Cleanup,
    Renewal,
    Welfare,
}

impl Role {
    pub const ALL: [Self; 10] = [
        Self::Tick,
        Self::Observe,
        Self::Decide,
        Self::Settle,
        Self::Execute,
        Self::OracleClose,
        Self::RegistryClose,
        Self::Cleanup,
        Self::Renewal,
        Self::Welfare,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tick => "tick",
            Self::Observe => "observe",
            Self::Decide => "decide",
            Self::Settle => "settle",
            Self::Execute => "execute",
            Self::OracleClose => "oracle-close",
            Self::RegistryClose => "registry-close",
            Self::Cleanup => "cleanup",
            Self::Renewal => "renewal",
            Self::Welfare => "welfare",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub type RoleSet = BTreeSet<Role>;

#[derive(Clone, Debug, Parser)]
#[command(name = "bleavit-keeper", version, about)]
pub struct Cli {
    /// Optional TOML configuration file. CLI values take precedence.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// WebSocket RPC endpoint. Repeat for ordered failover.
    #[arg(long = "node-url")]
    pub node_urls: Vec<String>,

    /// Development/secret URI, for example //Alice. Never use a dev URI in production.
    #[arg(long, conflicts_with = "signer_file")]
    pub signer_uri: Option<String>,

    /// File containing one secret URI. Whitespace around the URI is ignored.
    #[arg(long, conflicts_with = "signer_uri")]
    pub signer_file: Option<PathBuf>,

    /// Enabled roles. Repeat or comma-separate values; defaults to all roles.
    #[arg(long, value_delimiter = ',')]
    pub enabled_roles: Vec<Role>,

    /// Override the on-chain/default TWAP observation interval in blocks.
    #[arg(long)]
    pub obs_interval: Option<u64>,

    /// Plan and log cranks without signing or submitting.
    #[arg(long)]
    pub dry_run: bool,

    /// Optional Prometheus HTTP bind address, for example 127.0.0.1:9616.
    #[arg(long)]
    pub metrics_bind: Option<SocketAddr>,

    /// Re-plan only every Nth finalized block.
    #[arg(long)]
    pub every_n_blocks: Option<u64>,

    /// Maximum random startup delay for multi-instance politeness.
    #[arg(long)]
    pub startup_jitter_secs: Option<u64>,

    /// Suppress the same crank for this many finalized blocks after submission.
    #[arg(long)]
    pub cooldown_depth: Option<u64>,

    /// Timeout for one submission/finalization attempt.
    #[arg(long)]
    pub tx_timeout_secs: Option<u64>,

    /// Number of bounded retries after the first attempt.
    #[arg(long)]
    pub max_retries: Option<u32>,

    /// Initial retry backoff in milliseconds.
    #[arg(long)]
    pub retry_base_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignerSource {
    Uri(String),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Config {
    pub node_urls: Vec<String>,
    pub signer: Option<SignerSource>,
    pub enabled_roles: RoleSet,
    pub obs_interval: u64,
    pub dry_run: bool,
    pub metrics_bind: Option<SocketAddr>,
    pub every_n_blocks: u64,
    pub startup_jitter: Duration,
    pub cooldown_depth: u64,
    pub tx_timeout: Duration,
    pub max_retries: u32,
    pub retry_base: Duration,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    node_urls: Option<Vec<String>>,
    signer_uri: Option<String>,
    signer_file: Option<PathBuf>,
    enabled_roles: Option<Vec<Role>>,
    obs_interval: Option<u64>,
    dry_run: Option<bool>,
    metrics_bind: Option<SocketAddr>,
    every_n_blocks: Option<u64>,
    startup_jitter_secs: Option<u64>,
    cooldown_depth: Option<u64>,
    tx_timeout_secs: Option<u64>,
    max_retries: Option<u32>,
    retry_base_ms: Option<u64>,
}

impl Config {
    pub async fn load(cli: Cli) -> anyhow::Result<Self> {
        let file = match &cli.config {
            Some(path) => {
                let raw = tokio::fs::read_to_string(path)
                    .await
                    .with_context(|| format!("failed to read config file {}", path.display()))?;
                toml::from_str::<FileConfig>(&raw)
                    .with_context(|| format!("invalid config file {}", path.display()))?
            }
            None => FileConfig::default(),
        };
        Self::merge(cli, file)
    }

    fn merge(cli: Cli, file: FileConfig) -> anyhow::Result<Self> {
        if file.signer_uri.is_some() && file.signer_file.is_some() {
            bail!("config must set only one of signer_uri or signer_file");
        }

        let dry_run = cli.dry_run || file.dry_run.unwrap_or(false);

        let signer = match (cli.signer_uri, cli.signer_file) {
            (Some(uri), None) => Some(SignerSource::Uri(uri)),
            (None, Some(path)) => Some(SignerSource::File(path)),
            (None, None) => match (file.signer_uri, file.signer_file) {
                (Some(uri), None) => Some(SignerSource::Uri(uri)),
                (None, Some(path)) => Some(SignerSource::File(path)),
                (None, None) if dry_run => None,
                (None, None) => bail!(
                    "explicit signer required: set --signer-uri or --signer-file, or use --dry-run"
                ),
                (Some(_), Some(_)) => bail!("config must set only one signer source"),
            },
            (Some(_), Some(_)) => bail!("set only one signer source"),
        };

        let node_urls = if cli.node_urls.is_empty() {
            file.node_urls
                .unwrap_or_else(|| vec![DEFAULT_NODE_URL.to_owned()])
        } else {
            cli.node_urls
        };
        if node_urls.is_empty() || node_urls.iter().any(|url| url.trim().is_empty()) {
            bail!("at least one non-empty node URL is required");
        }

        let roles = if cli.enabled_roles.is_empty() {
            file.enabled_roles.unwrap_or_else(|| Role::ALL.to_vec())
        } else {
            cli.enabled_roles
        };
        let enabled_roles = roles.into_iter().collect::<RoleSet>();
        if enabled_roles.is_empty() {
            bail!("at least one keeper role must be enabled");
        }

        let obs_interval = cli.obs_interval.or(file.obs_interval).unwrap_or(10);
        let every_n_blocks = cli.every_n_blocks.or(file.every_n_blocks).unwrap_or(1);
        let cooldown_depth = cli.cooldown_depth.or(file.cooldown_depth).unwrap_or(3);
        let tx_timeout_secs = cli.tx_timeout_secs.or(file.tx_timeout_secs).unwrap_or(90);
        let retry_base_ms = cli.retry_base_ms.or(file.retry_base_ms).unwrap_or(500);
        if obs_interval == 0 || every_n_blocks == 0 || tx_timeout_secs == 0 || retry_base_ms == 0 {
            bail!("intervals and timeouts must be greater than zero");
        }

        Ok(Self {
            node_urls,
            signer,
            enabled_roles,
            obs_interval,
            dry_run,
            metrics_bind: cli.metrics_bind.or(file.metrics_bind),
            every_n_blocks,
            startup_jitter: Duration::from_secs(
                cli.startup_jitter_secs
                    .or(file.startup_jitter_secs)
                    .unwrap_or(0),
            ),
            cooldown_depth,
            tx_timeout: Duration::from_secs(tx_timeout_secs),
            max_retries: cli.max_retries.or(file.max_retries).unwrap_or(2),
            retry_base: Duration::from_millis(retry_base_ms),
        })
    }

    pub async fn signer_uri(&self) -> anyhow::Result<String> {
        match &self.signer {
            Some(SignerSource::Uri(uri)) => Ok(uri.clone()),
            Some(SignerSource::File(path)) => {
                let uri = tokio::fs::read_to_string(path)
                    .await
                    .with_context(|| format!("failed to read signer file {}", path.display()))?;
                let uri = uri.trim();
                if uri.is_empty() {
                    bail!("signer file {} is empty", path.display());
                }
                Ok(uri.to_owned())
            }
            None => bail!("no signer configured; set --signer-uri or --signer-file"),
        }
    }
}

impl FromStr for SignerSource {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            bail!("signer URI cannot be empty");
        }
        Ok(Self::Uri(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_roles_and_failover_urls() {
        let cli = Cli::try_parse_from([
            "keeper",
            "--node-url",
            "wss://one.example",
            "--node-url",
            "wss://two.example",
            "--enabled-roles",
            "tick,oracle-close",
            "--obs-interval",
            "12",
            "--dry-run",
        ])
        .expect("test CLI should parse");
        let config = Config::merge(cli, FileConfig::default()).expect("config should merge");
        assert_eq!(config.node_urls.len(), 2);
        assert_eq!(config.obs_interval, 12);
        assert!(config.dry_run);
        assert_eq!(config.enabled_roles, [Role::Tick, Role::OracleClose].into());
    }

    #[test]
    fn file_values_are_used_and_cli_wins() {
        let file: FileConfig = toml::from_str(
            r#"
                node_urls = ["wss://file.example"]
                signer_uri = "//Bob"
                enabled_roles = ["cleanup"]
                obs_interval = 20
                cooldown_depth = 9
            "#,
        )
        .expect("test TOML should parse");
        let cli =
            Cli::try_parse_from(["keeper", "--obs-interval", "7"]).expect("test CLI should parse");
        let config = Config::merge(cli, file).expect("config should merge");
        assert_eq!(config.node_urls, ["wss://file.example"]);
        assert_eq!(config.signer, Some(SignerSource::Uri("//Bob".to_owned())));
        assert_eq!(config.obs_interval, 7);
        assert_eq!(config.cooldown_depth, 9);
        assert_eq!(config.enabled_roles, [Role::Cleanup].into());
    }

    #[test]
    fn rejects_zero_intervals() {
        let cli = Cli::try_parse_from(["keeper", "--obs-interval", "0", "--dry-run"])
            .expect("test CLI should parse");
        assert!(Config::merge(cli, FileConfig::default()).is_err());
    }

    #[test]
    fn refuses_to_start_without_an_explicit_signer_unless_dry_run() {
        let cli = Cli::try_parse_from(["keeper"]).expect("test CLI should parse");
        let error = Config::merge(cli, FileConfig::default())
            .expect_err("live mode without a signer must fail");
        let message = error.to_string();
        assert!(message.contains("--signer-uri"));
        assert!(message.contains("--signer-file"));
        assert!(message.contains("--dry-run"));

        let cli =
            Cli::try_parse_from(["keeper", "--dry-run"]).expect("test dry-run CLI should parse");
        let config =
            Config::merge(cli, FileConfig::default()).expect("dry-run should not require a signer");
        assert!(config.dry_run);
        assert_eq!(config.signer, None);
    }
}
