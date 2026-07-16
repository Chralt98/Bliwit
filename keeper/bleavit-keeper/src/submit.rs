use std::{collections::BTreeMap, time::Duration};

use futures::{stream::FuturesUnordered, StreamExt};
use subxt::{
    config::polkadot::PolkadotExtrinsicParamsBuilder, dynamic, tx::TxProgress, OnlineClient,
    PolkadotConfig,
};
use subxt_signer::sr25519::Keypair;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::{metrics::KeeperMetrics, planner::PlannedCrank};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttemptOutcome {
    Success,
    ExpectedFailure,
    TransportFailure,
}

struct PendingCrank {
    crank: PlannedCrank,
    nonce: u64,
    progress: TxProgress<PolkadotConfig, OnlineClient<PolkadotConfig>>,
}

enum SubmissionOutcome {
    Pending(PendingCrank),
    ExpectedFailure,
    TransportFailure,
}

pub struct Submitter {
    client: OnlineClient<PolkadotConfig>,
    signer: Keypair,
    nonce: Option<u64>,
    timeout: Duration,
    max_retries: u32,
    retry_base: Duration,
    cooldown_depth: u64,
    cooldowns: BTreeMap<String, u64>,
}

impl Submitter {
    pub fn new(
        client: OnlineClient<PolkadotConfig>,
        signer: Keypair,
        timeout: Duration,
        max_retries: u32,
        retry_base: Duration,
        cooldown_depth: u64,
    ) -> Self {
        Self {
            client,
            signer,
            nonce: None,
            timeout,
            max_retries,
            retry_base,
            cooldown_depth,
            cooldowns: BTreeMap::new(),
        }
    }

    pub fn cooldowns(&self) -> &BTreeMap<String, u64> {
        &self.cooldowns
    }

    pub fn import_cooldowns(&mut self, cooldowns: BTreeMap<String, u64>) {
        self.cooldowns = cooldowns;
    }

    pub fn prune_cooldowns(&mut self, current_block: u64) {
        let depth = self.cooldown_depth;
        self.cooldowns
            .retain(|_, last| current_block.saturating_sub(*last) < depth);
    }

    /// Returns `true` when the caller should reconnect before processing more blocks.
    pub async fn submit_all(
        &mut self,
        cranks: &[PlannedCrank],
        current_block: u64,
        metrics: &KeeperMetrics,
    ) -> bool {
        self.prune_cooldowns(current_block);
        let mut pending = Vec::new();
        let mut reconnect = false;
        for crank in cranks {
            match self.begin_submission(crank, current_block, metrics).await {
                SubmissionOutcome::Pending(crank) => pending.push(crank),
                SubmissionOutcome::ExpectedFailure => {}
                SubmissionOutcome::TransportFailure => {
                    reconnect = true;
                    break;
                }
            }
        }
        if reconnect {
            // Accepted calls are already in cooldown. Drop their progress
            // subscriptions and reconnect immediately instead of waiting on a
            // transport that has just failed.
            return true;
        }

        // Submit the complete priority-ordered batch before awaiting any finality.
        // A finalized block can include several observation cranks, which is
        // required to sustain the ten-block grid across all live books.
        let mut finalities = FuturesUnordered::new();
        for pending in pending {
            let timeout = self.timeout;
            let metrics = metrics.clone();
            finalities.push(async move { await_finality(pending, timeout, metrics).await });
        }
        while let Some(outcome) = finalities.next().await {
            if outcome == AttemptOutcome::TransportFailure {
                reconnect = true;
                break;
            }
        }
        reconnect
    }

    async fn begin_submission(
        &mut self,
        crank: &PlannedCrank,
        current_block: u64,
        metrics: &KeeperMetrics,
    ) -> SubmissionOutcome {
        for attempt in 0..=self.max_retries {
            if self.nonce.is_none() {
                match timeout(self.timeout, self.fetch_nonce()).await {
                    Ok(Ok(nonce)) => self.nonce = Some(nonce),
                    Ok(Err(error)) => {
                        metrics.failed(crank.role);
                        warn!(
                            role = %crank.role,
                            pallet = crank.pallet,
                            call = crank.call,
                            attempt,
                            %error,
                            "nonce fetch failed"
                        );
                        if attempt < self.max_retries {
                            sleep(backoff(self.retry_base, attempt)).await;
                            continue;
                        }
                        return SubmissionOutcome::TransportFailure;
                    }
                    Err(_) => {
                        metrics.failed(crank.role);
                        warn!(
                            role = %crank.role,
                            pallet = crank.pallet,
                            call = crank.call,
                            attempt,
                            "nonce fetch timed out"
                        );
                        if attempt < self.max_retries {
                            sleep(backoff(self.retry_base, attempt)).await;
                            continue;
                        }
                        return SubmissionOutcome::TransportFailure;
                    }
                }
            }
            let Some(nonce) = self.nonce else {
                metrics.failed(crank.role);
                return SubmissionOutcome::TransportFailure;
            };
            let payload = dynamic::tx(crank.pallet, crank.call, crank.args.clone());
            let params = PolkadotExtrinsicParamsBuilder::<PolkadotConfig>::new()
                .nonce(nonce)
                .mortal(64)
                .build();
            let mut tx = self.client.tx();
            let submission = timeout(
                self.timeout,
                tx.sign_and_submit_then_watch(&payload, &self.signer, params),
            )
            .await;
            let progress = match submission {
                Ok(Ok(progress)) => progress,
                Ok(Err(error)) if is_transport(&error) => {
                    metrics.failed(crank.role);
                    warn!(
                        role = %crank.role,
                        pallet = crank.pallet,
                        call = crank.call,
                        nonce,
                        attempt,
                        %error,
                        "transport error before crank submission"
                    );
                    self.nonce = None;
                    if attempt < self.max_retries {
                        sleep(backoff(self.retry_base, attempt)).await;
                        continue;
                    }
                    return SubmissionOutcome::TransportFailure;
                }
                Ok(Err(error)) => {
                    metrics.failed(crank.role);
                    debug!(
                        role = %crank.role,
                        pallet = crank.pallet,
                        call = crank.call,
                        nonce,
                        %error,
                        "crank rejected before inclusion"
                    );
                    self.nonce = None;
                    return SubmissionOutcome::ExpectedFailure;
                }
                Err(_) => {
                    metrics.failed(crank.role);
                    warn!(
                        role = %crank.role,
                        pallet = crank.pallet,
                        call = crank.call,
                        nonce,
                        attempt,
                        "crank submission timed out"
                    );
                    self.nonce = None;
                    if attempt < self.max_retries {
                        sleep(backoff(self.retry_base, attempt)).await;
                        continue;
                    }
                    return SubmissionOutcome::TransportFailure;
                }
            };

            // The node accepted the extrinsic. Reserve the nonce locally and enter
            // cooldown before awaiting finality; a lost subscription must not cause an
            // immediate duplicate submission of an extrinsic that may still land.
            self.nonce = Some(nonce.saturating_add(1));
            self.cooldowns.insert(crank.cooldown_key(), current_block);
            metrics.submitted(crank.role);
            info!(
                role = %crank.role,
                pallet = crank.pallet,
                call = crank.call,
                nonce,
                "crank submitted"
            );
            return SubmissionOutcome::Pending(PendingCrank {
                crank: crank.clone(),
                nonce,
                progress,
            });
        }
        SubmissionOutcome::TransportFailure
    }

    async fn fetch_nonce(&self) -> Result<u64, subxt::Error> {
        let account = self.signer.public_key().to_account_id();
        self.client.tx().account_nonce(&account).await
    }
}

async fn await_finality(
    pending: PendingCrank,
    finality_timeout: Duration,
    metrics: KeeperMetrics,
) -> AttemptOutcome {
    let PendingCrank {
        crank,
        nonce,
        progress,
    } = pending;
    match timeout(finality_timeout, progress.wait_for_finalized_success()).await {
        Ok(Ok(_)) => {
            metrics.succeeded(crank.role);
            info!(
                role = %crank.role,
                pallet = crank.pallet,
                call = crank.call,
                nonce,
                "crank finalized successfully"
            );
            AttemptOutcome::Success
        }
        Ok(Err(error)) if is_transport(&error) => {
            metrics.failed(crank.role);
            warn!(
                role = %crank.role,
                pallet = crank.pallet,
                call = crank.call,
                nonce,
                %error,
                "transport error while awaiting crank finality; reconnecting"
            );
            AttemptOutcome::TransportFailure
        }
        Ok(Err(error)) => {
            metrics.failed(crank.role);
            // A concurrent keeper commonly wins the state race between our
            // finalized snapshot and inclusion. This is expected and quiet.
            debug!(
                role = %crank.role,
                pallet = crank.pallet,
                call = crank.call,
                nonce,
                %error,
                "crank extrinsic failed (expected under keeper concurrency)"
            );
            AttemptOutcome::ExpectedFailure
        }
        Err(_) => {
            metrics.failed(crank.role);
            warn!(
                role = %crank.role,
                pallet = crank.pallet,
                call = crank.call,
                nonce,
                "timed out awaiting crank finality; reconnecting without resubmission"
            );
            AttemptOutcome::TransportFailure
        }
    }
}

fn is_transport(error: &subxt::Error) -> bool {
    matches!(error, subxt::Error::Io(_) | subxt::Error::Rpc(_))
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    let multiplier = 1_u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX);
    base.saturating_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_is_bounded_and_monotone() {
        let base = Duration::from_millis(100);
        assert_eq!(backoff(base, 0), base);
        assert_eq!(backoff(base, 3), Duration::from_millis(800));
        assert!(backoff(base, u32::MAX) >= backoff(base, 3));
    }
}
