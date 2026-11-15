//! Persists and reads call records exchanged between member machines and a
//! collector. Only usage and metadata are exchanged, never request payloads.
use sqlx::Row;

use crate::{
    model::{CollectedCall, OverviewIngestBatch},
    Result,
};

use super::{now_ms, OverviewWatermark, Store};

const PUSH_BATCH_LIMIT: i64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryPushKind {
    Cursor,
    LocalAgent,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingHistoryCall {
    pub kind: HistoryPushKind,
    pub event_key: String,
    pub call: CollectedCall,
}

impl Store {
    /// Calls finished after `watermark` that still need to be pushed.
    pub async fn pending_collected_calls(
        &self,
        watermark: &OverviewWatermark,
    ) -> Result<Vec<CollectedCall>> {
        let rows = sqlx::query(
            "SELECT call_id, created_at_ms, COALESCE(finished_at_ms, created_at_ms) AS finished_at_ms,
                    request_type, model_id, display_name, status,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
             FROM llm_calls
             WHERE status != 'running'
               AND (COALESCE(finished_at_ms, created_at_ms) > ?
                    OR (COALESCE(finished_at_ms, created_at_ms) = ? AND call_id > ?))
             ORDER BY COALESCE(finished_at_ms, created_at_ms), call_id
             LIMIT ?",
        )
        .bind(watermark.finished_at_ms)
        .bind(watermark.finished_at_ms)
        .bind(&watermark.call_id)
        .bind(PUSH_BATCH_LIMIT)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let request_type: String = row.try_get("request_type")?;
                let model_id: String = row.try_get("model_id")?;
                Ok(CollectedCall {
                    call_id: row.try_get("call_id")?,
                    created_at_ms: row.try_get("created_at_ms")?,
                    finished_at_ms: row.try_get("finished_at_ms")?,
                    model_key: model_key(&request_type, &model_id),
                    request_type,
                    display_name: row.try_get("display_name")?,
                    status: row.try_get("status")?,
                    input_tokens: row.try_get("input_tokens")?,
                    output_tokens: row.try_get("output_tokens")?,
                    cache_read_tokens: row.try_get("cache_read_tokens")?,
                    cache_write_tokens: row.try_get("cache_write_tokens")?,
                })
            })
            .collect()
    }

    /// Number of finished local calls not yet reflected in the watermark.
    pub async fn pending_collected_call_count(&self, watermark: &OverviewWatermark) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM llm_calls
             WHERE status != 'running'
               AND (COALESCE(finished_at_ms, created_at_ms) > ?
                    OR (COALESCE(finished_at_ms, created_at_ms) = ? AND call_id > ?))",
        )
        .bind(watermark.finished_at_ms)
        .bind(watermark.finished_at_ms)
        .bind(&watermark.call_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub(crate) async fn pending_history_collected_calls(
        &self,
        limit: i64,
    ) -> Result<Vec<PendingHistoryCall>> {
        let rows = sqlx::query(
            "SELECT 'cursor' AS history_kind, event_key, created_at_ms, model_key,
                    '' AS source, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
             FROM cursor_usage_events
             WHERE pushed_at_ms IS NULL
             UNION ALL
             SELECT 'local_agent' AS history_kind, event_key, created_at_ms, model_key,
                    source, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
             FROM local_agent_usage_events
             WHERE pushed_at_ms IS NULL
             ORDER BY created_at_ms, event_key
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let kind = match row.try_get::<String, _>("history_kind")?.as_str() {
                    "cursor" => HistoryPushKind::Cursor,
                    "local_agent" => HistoryPushKind::LocalAgent,
                    value => {
                        return Err(crate::Error::Store(format!(
                            "unknown imported history kind: {value}"
                        )))
                    }
                };
                let event_key: String = row.try_get("event_key")?;
                let created_at_ms: i64 = row.try_get("created_at_ms")?;
                let raw_model: String = row.try_get("model_key")?;
                let source: String = row.try_get("source")?;
                let (request_type, model_key, display_name) = match kind {
                    HistoryPushKind::Cursor => (
                        "cursor-history",
                        format!("cursor-history:{raw_model}"),
                        raw_model,
                    ),
                    HistoryPushKind::LocalAgent => {
                        let label = match source.as_str() {
                            "codex" => "Codex",
                            "claude_code" => "Claude Code",
                            "claude_cli" => "Claude CLI",
                            value => value,
                        };
                        (
                            "local-agent-history",
                            format!("local-agent-history:{source}:{raw_model}"),
                            format!("{label}: {raw_model}"),
                        )
                    }
                };
                Ok(PendingHistoryCall {
                    kind,
                    event_key: event_key.clone(),
                    call: CollectedCall {
                        call_id: format!("{request_type}:{event_key}"),
                        created_at_ms,
                        finished_at_ms: created_at_ms,
                        request_type: request_type.into(),
                        model_key,
                        display_name,
                        status: "completed".into(),
                        input_tokens: row.try_get("input_tokens")?,
                        output_tokens: row.try_get("output_tokens")?,
                        cache_read_tokens: row.try_get("cache_read_tokens")?,
                        cache_write_tokens: row.try_get("cache_write_tokens")?,
                    },
                })
            })
            .collect()
    }

    pub(crate) async fn pending_history_call_count(&self) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT
                (SELECT COUNT(*) FROM cursor_usage_events WHERE pushed_at_ms IS NULL) +
                (SELECT COUNT(*) FROM local_agent_usage_events WHERE pushed_at_ms IS NULL)",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub(crate) async fn mark_history_calls_pushed(
        &self,
        calls: &[PendingHistoryCall],
    ) -> Result<()> {
        if calls.is_empty() {
            return Ok(());
        }
        let pushed_at_ms = now_ms();
        let _write = self.writes.lock().await;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for pending in calls {
            let table = match pending.kind {
                HistoryPushKind::Cursor => "cursor_usage_events",
                HistoryPushKind::LocalAgent => "local_agent_usage_events",
            };
            sqlx::query(&format!(
                "UPDATE {table} SET pushed_at_ms = ? WHERE event_key = ?"
            ))
            .bind(pushed_at_ms)
            .bind(&pending.event_key)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn reset_history_push_state(&self) -> Result<()> {
        let _write = self.writes.lock().await;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE cursor_usage_events SET pushed_at_ms = NULL")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE local_agent_usage_events SET pushed_at_ms = NULL")
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Stores a pushed batch idempotently and refreshes the source registry.
    pub async fn ingest_collected_calls(&self, batch: &OverviewIngestBatch) -> Result<usize> {
        if batch.source_id.trim().is_empty() {
            return Err(crate::Error::Config(
                "overview source id is required".into(),
            ));
        }
        let source_id = batch.source_id.trim();
        let source_name = batch.source_name.trim();
        let source_name = if source_name.is_empty() {
            source_id
        } else {
            source_name
        };
        let _write = self.writes.lock().await;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut inserted = 0_usize;
        for call in batch.calls.iter().filter(|call| call.status != "running") {
            let result = sqlx::query(
                "INSERT INTO overview_collected_calls(
                    source_id, call_id, source_name, created_at_ms, finished_at_ms,
                    request_type, model_key, display_name, status,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(source_id, call_id) DO NOTHING",
            )
            .bind(source_id)
            .bind(&call.call_id)
            .bind(source_name)
            .bind(call.created_at_ms)
            .bind(call.finished_at_ms)
            .bind(&call.request_type)
            .bind(&call.model_key)
            .bind(&call.display_name)
            .bind(&call.status)
            .bind(call.input_tokens)
            .bind(call.output_tokens)
            .bind(call.cache_read_tokens)
            .bind(call.cache_write_tokens)
            .execute(&mut *transaction)
            .await?;
            inserted = inserted.saturating_add(result.rows_affected() as usize);
        }
        let call_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM overview_collected_calls WHERE source_id = ?")
                .bind(source_id)
                .fetch_one(&mut *transaction)
                .await?;
        sqlx::query(
            "INSERT INTO overview_sources(source_id, name, last_push_at_ms, call_count)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(source_id) DO UPDATE SET
                 name = excluded.name,
                 last_push_at_ms = excluded.last_push_at_ms,
                 call_count = excluded.call_count",
        )
        .bind(source_id)
        .bind(source_name)
        .bind(now_ms())
        .bind(call_count)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(inserted)
    }

    /// Machines that have pushed at least one record, newest first.
    pub async fn overview_sources(&self) -> Result<Vec<crate::model::OverviewSource>> {
        let rows = sqlx::query(
            "SELECT source_id, name, last_push_at_ms, call_count
             FROM overview_sources
             ORDER BY last_push_at_ms DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(crate::model::OverviewSource {
                    source_id: row.try_get("source_id")?,
                    name: row.try_get("name")?,
                    last_push_at_ms: row.try_get("last_push_at_ms")?,
                    call_count: row.try_get("call_count")?,
                })
            })
            .collect()
    }

    /// Removes every collected record of a source, e.g. after re-keying a machine.
    pub async fn forget_overview_source(&self, source_id: &str) -> Result<()> {
        let _write = self.writes.lock().await;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM overview_collected_calls WHERE source_id = ?")
            .bind(source_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM overview_sources WHERE source_id = ?")
            .bind(source_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }
}

/// Advances the cursor to the last record of a pushed batch.
pub fn next_watermark(
    batch: &OverviewIngestBatch,
    current: &OverviewWatermark,
) -> OverviewWatermark {
    let Some(last) = batch.calls.iter().max_by(|left, right| {
        (left.finished_at_ms, &left.call_id).cmp(&(right.finished_at_ms, &right.call_id))
    }) else {
        return current.clone();
    };
    if (last.finished_at_ms, &last.call_id) > (current.finished_at_ms, &current.call_id) {
        OverviewWatermark {
            finished_at_ms: last.finished_at_ms,
            call_id: last.call_id.clone(),
        }
    } else {
        current.clone()
    }
}

/// Cross-machine stable model identity: the same model on two machines with
/// different local hashes still maps to one filter entry.
pub fn model_key(request_type: &str, model_id: &str) -> String {
    format!("{request_type}:{model_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CollectedCall, OverviewIngestBatch, ProviderType};
    use crate::store::OverviewWatermark;

    async fn store() -> Store {
        let directory = tempfile::tempdir().unwrap();
        Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap()
    }

    fn call(call_id: &str, finished_at_ms: i64, status: &str) -> CollectedCall {
        let request_type = ProviderType::OpenAiChat.as_str();
        CollectedCall {
            call_id: call_id.into(),
            created_at_ms: finished_at_ms,
            finished_at_ms,
            request_type: request_type.into(),
            model_key: model_key(request_type, "gpt-test"),
            display_name: "GPT Test".into(),
            status: status.into(),
            input_tokens: Some(100),
            output_tokens: Some(10),
            cache_read_tokens: Some(0),
            cache_write_tokens: Some(0),
        }
    }

    #[tokio::test]
    async fn a_repeated_batch_is_idempotent() {
        let store = store().await;
        let batch = OverviewIngestBatch {
            source_id: "source-a".into(),
            source_name: "Mac".into(),
            calls: vec![call("call-1", 1_000, "completed")],
        };
        assert_eq!(store.ingest_collected_calls(&batch).await.unwrap(), 1);
        assert_eq!(store.ingest_collected_calls(&batch).await.unwrap(), 0);

        let overview = store
            .collected_overview(Some(0), Some(10_000), None, None)
            .await
            .unwrap();
        assert_eq!(overview.metrics.llm_calls, 1);
        assert_eq!(overview.metrics.input_tokens, 100);
        assert_eq!(overview.device_count, 1);
    }

    #[tokio::test]
    async fn running_calls_are_not_collected() {
        let store = store().await;
        store
            .ingest_collected_calls(&OverviewIngestBatch {
                source_id: "source-a".into(),
                source_name: "Mac".into(),
                calls: vec![call("call-1", 1_000, "running")],
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .collected_overview(Some(0), Some(10_000), None, None)
                .await
                .unwrap()
                .metrics
                .llm_calls,
            0
        );
    }

    #[tokio::test]
    async fn collected_models_are_listed_by_stable_key() {
        let store = store().await;
        store
            .ingest_collected_calls(&OverviewIngestBatch {
                source_id: "source-a".into(),
                source_name: "Mac".into(),
                calls: vec![call("call-1", 1_000, "completed")],
            })
            .await
            .unwrap();
        let models = store.collected_overview_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].key, "openai-chat:gpt-test");
        assert_eq!(models[0].label, "GPT Test");
    }

    #[tokio::test]
    async fn imported_history_is_exported_and_marked_idempotently() {
        let store = store().await;
        store
            .replace_cursor_usage(&[crate::model::CursorUsageRecord {
                event_key: "cursor:one".into(),
                occurred_at_ms: 1_000,
                model_key: "cursor-model".into(),
                conversation_id: "conversation".into(),
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: 20,
                cache_write_tokens: 0,
            }])
            .await
            .unwrap();
        store
            .upsert_local_agent_usage(&[crate::model::LocalAgentUsageRecord {
                event_key: "codex:one".into(),
                source: "codex".into(),
                occurred_at_ms: 2_000,
                model_key: "codex".into(),
                input_tokens: 30,
                output_tokens: 3,
                cache_read_tokens: 40,
                cache_write_tokens: 0,
            }])
            .await
            .unwrap();

        assert_eq!(store.pending_history_call_count().await.unwrap(), 2);
        let pending = store.pending_history_collected_calls(10).await.unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].call.request_type, "cursor-history");
        assert_eq!(pending[1].call.request_type, "local-agent-history");
        store.mark_history_calls_pushed(&pending).await.unwrap();
        assert_eq!(store.pending_history_call_count().await.unwrap(), 0);

        store.reset_history_push_state().await.unwrap();
        assert_eq!(store.pending_history_call_count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn imported_history_uses_fresh_input_semantics_when_aggregated_remotely() {
        let store = store().await;
        store
            .ingest_collected_calls(&OverviewIngestBatch {
                source_id: "source-a".into(),
                source_name: "Mac".into(),
                calls: vec![CollectedCall {
                    call_id: "cursor-history:one".into(),
                    created_at_ms: 1_000,
                    finished_at_ms: 1_000,
                    request_type: "cursor-history".into(),
                    model_key: "cursor-history:model".into(),
                    display_name: "Cursor history".into(),
                    status: "completed".into(),
                    input_tokens: Some(100),
                    output_tokens: Some(10),
                    cache_read_tokens: Some(20),
                    cache_write_tokens: Some(5),
                }],
            })
            .await
            .unwrap();

        let metrics = store
            .collected_overview(Some(0), Some(10_000), None, None)
            .await
            .unwrap()
            .metrics;
        assert_eq!(metrics.input_tokens, 100);
        assert_eq!(metrics.cache_read_tokens, 20);
        assert_eq!(metrics.cache_write_tokens, 5);
        assert_eq!(metrics.prompt_tokens, 125);
        assert_eq!(metrics.token_usage, 135);
    }

    #[tokio::test]
    async fn forgetting_a_source_drops_its_records() {
        let store = store().await;
        store
            .ingest_collected_calls(&OverviewIngestBatch {
                source_id: "source-a".into(),
                source_name: "Mac".into(),
                calls: vec![call("call-1", 1_000, "completed")],
            })
            .await
            .unwrap();
        store.forget_overview_source("source-a").await.unwrap();
        assert_eq!(
            store
                .collected_overview(Some(0), Some(10_000), None, None)
                .await
                .unwrap()
                .device_count,
            0
        );
    }

    #[test]
    fn a_watermark_advances_past_records_sharing_one_timestamp() {
        let current = OverviewWatermark::default();
        let first = OverviewIngestBatch {
            source_id: "source-a".into(),
            source_name: "Mac".into(),
            calls: vec![call("call-a", 1_000, "completed")],
        };
        let after_first = next_watermark(&first, &current);
        assert_eq!(after_first.finished_at_ms, 1_000);
        assert_eq!(after_first.call_id, "call-a");

        let second = OverviewIngestBatch {
            calls: vec![call("call-b", 1_000, "completed")],
            ..first.clone()
        };
        let after_second = next_watermark(&second, &after_first);
        assert_eq!(after_second.call_id, "call-b");
        // An older batch never moves the cursor backwards.
        assert_eq!(next_watermark(&first, &after_second), after_second);
    }
}
