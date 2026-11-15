//! Member-side worker that pushes finished call records to a collector.
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::{
    model::OverviewIngestBatch,
    network::NetworkClients,
    store::{
        next_watermark, now_ms, OverviewMode, OverviewSyncSettings, OverviewSyncStatus, Store,
    },
    Error, Result,
};

const PUSH_INTERVAL: Duration = Duration::from_secs(60);
const PUSH_TIMEOUT: Duration = Duration::from_secs(20);
const PUSH_BATCH_LIMIT: i64 = 500;
/// Short pause between batches while draining a backlog, so a large first sync
/// does not monopolise the network or the SQLite writer.
const DRAIN_PAUSE: Duration = Duration::from_millis(150);
/// Bounded manual drain so a permanently failing collector cannot hang the UI.
const MAX_FLUSH_ROUNDS: usize = 400;

/// Runs the periodic push loop until `shutdown` is cancelled.
///
/// A round keeps pushing while more records are pending, so enabling remote mode
/// on a machine with a long history drains in minutes instead of one batch (500
/// records) per minute.
pub async fn run(store: Store, clients: NetworkClients, shutdown: CancellationToken) {
    let mut ticker = tokio::time::interval(PUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {
                loop {
                    if shutdown.is_cancelled() {
                        return;
                    }
                    match push_round(&store, &clients).await {
                        Ok(true) => {
                            tokio::select! {
                                () = shutdown.cancelled() => return,
                                () = tokio::time::sleep(DRAIN_PAUSE) => {}
                            }
                        }
                        Ok(false) => break,
                        Err(error) => {
                            tracing::warn!(%error, "overview push round failed");
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Drains the backlog immediately, e.g. when the user asks for a sync.
pub async fn flush(store: &Store, clients: &NetworkClients) -> Result<OverviewSyncStatus> {
    let settings = store.overview_sync_settings().await?;
    if settings.mode != OverviewMode::Remote {
        return Err(Error::Config(
            "overview data source is local; switch to remote collection first".into(),
        ));
    }
    if !settings.is_ready() {
        return Err(Error::Config(
            "overview collector address and API key are required".into(),
        ));
    }
    for _ in 0..MAX_FLUSH_ROUNDS {
        if !push_round(store, clients).await? {
            break;
        }
    }
    store.overview_sync_status().await
}

/// Clears the push cursor so the next sync uploads this machine's whole history.
///
/// The collector stores records by `(source_id, call_id)`, so re-sending history
/// never duplicates rows; this is safe after switching collectors or wiping one.
pub async fn reset_progress(store: &Store) -> Result<OverviewSyncStatus> {
    let status = store.reset_overview_sync_watermark().await?;
    tracing::info!(
        pending_calls = status.pending_calls,
        "overview sync progress reset; history will be uploaded again"
    );
    Ok(status)
}

/// Pushes the next batch. Returns whether a full batch was sent, so callers can
/// keep draining.
async fn push_round(store: &Store, clients: &NetworkClients) -> Result<bool> {
    let settings = store.overview_sync_settings().await?;
    if !settings.is_ready() {
        refresh_status(store, None, false).await?;
        return Ok(false);
    }
    let history = store
        .pending_history_collected_calls(PUSH_BATCH_LIMIT)
        .await?;
    if !history.is_empty() {
        let batch = OverviewIngestBatch {
            source_id: settings.device_id.clone(),
            source_name: settings.device_name.clone(),
            calls: history.iter().map(|pending| pending.call.clone()).collect(),
        };
        match post_batch(clients, &settings, &batch).await {
            Ok(()) => {
                store.mark_history_calls_pushed(&history).await?;
                refresh_status(store, None, true).await?;
                let watermark = store.overview_sync_watermark().await?;
                let normal_pending = store.pending_collected_call_count(&watermark).await?;
                return Ok(history.len() == PUSH_BATCH_LIMIT as usize || normal_pending > 0);
            }
            Err(error) => {
                refresh_status(store, Some(error.to_string()), false).await?;
                return Err(error);
            }
        }
    }

    let watermark = store.overview_sync_watermark().await?;
    let calls = store.pending_collected_calls(&watermark).await?;
    if calls.is_empty() {
        refresh_status(store, None, false).await?;
        return Ok(false);
    }
    let batch = OverviewIngestBatch {
        source_id: settings.device_id.clone(),
        source_name: settings.device_name.clone(),
        calls,
    };
    let next = next_watermark(&batch, &watermark);
    match post_batch(clients, &settings, &batch).await {
        Ok(()) => {
            store.set_overview_sync_watermark(&next).await?;
            refresh_status(store, None, true).await?;
            Ok(store.pending_collected_call_count(&next).await? > 0)
        }
        Err(error) => {
            refresh_status(store, Some(error.to_string()), false).await?;
            Err(error)
        }
    }
}

async fn post_batch(
    clients: &NetworkClients,
    settings: &OverviewSyncSettings,
    batch: &OverviewIngestBatch,
) -> Result<()> {
    let url = settings
        .ingest_url()
        .ok_or_else(|| Error::Config("overview collector address is missing".into()))?;
    let client = clients.default_client().await?;
    let response = tokio::time::timeout(
        PUSH_TIMEOUT,
        client
            .post(&url)
            .bearer_auth(&settings.api_key)
            .json(batch)
            .send(),
    )
    .await
    .map_err(|_| Error::Provider("overview collector request timed out".into()))??;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Provider(format!(
            "overview collector rejected the batch ({status}): {}",
            body.trim()
        )));
    }
    Ok(())
}

async fn refresh_status(store: &Store, error: Option<String>, pushed: bool) -> Result<()> {
    let previous = store.overview_sync_status().await?;
    let watermark = store.overview_sync_watermark().await?;
    let pending_calls = store
        .pending_collected_call_count(&watermark)
        .await?
        .saturating_add(store.pending_history_call_count().await?);
    let last_push_at_ms = if pushed {
        Some(now_ms())
    } else {
        previous.last_push_at_ms
    };
    store
        .set_overview_sync_status(&OverviewSyncStatus {
            pending_calls,
            last_push_at_ms,
            last_error: error,
        })
        .await
}
