//! Persists token usage extracted from local Codex and Claude session logs.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{
    model::{
        LocalAgentHistoryUsage, LocalAgentUsageRecord, LocalAgentUsageSource, TokenUsageBucket,
    },
    Result,
};

use super::{now_ms, usage::fill_token_usage_series, Store};

const LOCAL_AGENT_USAGE_STATUS_KEY: &str = "local_agent_usage_sync";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct LocalAgentHistoryStatus {
    pub event_count: i64,
    pub last_sync_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub sources: Vec<LocalAgentUsageSource>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalAgentUsageSyncStatus {
    #[serde(default)]
    last_sync_at_ms: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
}

impl Store {
    pub async fn upsert_local_agent_usage(
        &self,
        records: &[LocalAgentUsageRecord],
    ) -> Result<usize> {
        let _write = self.writes.lock().await;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for record in records {
            sqlx::query(
                "INSERT INTO local_agent_usage_events(
                    event_key, source, created_at_ms, model_key,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(event_key) DO UPDATE SET
                    source = excluded.source,
                    created_at_ms = excluded.created_at_ms,
                    model_key = excluded.model_key,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    cache_read_tokens = excluded.cache_read_tokens,
                    cache_write_tokens = excluded.cache_write_tokens",
            )
            .bind(&record.event_key)
            .bind(&record.source)
            .bind(record.occurred_at_ms)
            .bind(&record.model_key)
            .bind(record.input_tokens)
            .bind(record.output_tokens)
            .bind(record.cache_read_tokens)
            .bind(record.cache_write_tokens)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(records.len())
    }

    pub async fn local_agent_usage_totals(
        &self,
        start_ms: i64,
        end_ms: i64,
        series_start_ms: i64,
        bucket_count: usize,
        bucket_ms: i64,
    ) -> Result<LocalAgentHistoryUsage> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS calls,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens,
                    COALESCE(SUM(cache_write_tokens), 0) AS cache_write_tokens
             FROM local_agent_usage_events
             WHERE created_at_ms >= ? AND created_at_ms < ?",
        )
        .bind(start_ms)
        .bind(end_ms)
        .fetch_one(&self.pool)
        .await?;
        let sources = self
            .local_agent_usage_sources(Some((start_ms, end_ms)))
            .await?;

        let series_end_ms = series_start_ms.saturating_add(
            i64::try_from(bucket_count)
                .unwrap_or(0)
                .saturating_mul(bucket_ms),
        );
        let rows = sqlx::query(
            "SELECT (created_at_ms / ?) * ? AS bucket_start_ms,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens,
                    COALESCE(SUM(cache_write_tokens), 0) AS cache_write_tokens
             FROM local_agent_usage_events
             WHERE created_at_ms >= ? AND created_at_ms < ?
             GROUP BY bucket_start_ms
             ORDER BY bucket_start_ms",
        )
        .bind(bucket_ms)
        .bind(bucket_ms)
        .bind(series_start_ms)
        .bind(series_end_ms)
        .fetch_all(&self.pool)
        .await?;
        let recorded = rows
            .into_iter()
            .map(|row| {
                let bucket_start_ms: i64 = row.try_get("bucket_start_ms")?;
                Ok((
                    bucket_start_ms,
                    TokenUsageBucket {
                        bucket_start_ms,
                        input_tokens: row.try_get("input_tokens")?,
                        cache_read_tokens: row.try_get("cache_read_tokens")?,
                        cache_write_tokens: row.try_get("cache_write_tokens")?,
                        output_tokens: row.try_get("output_tokens")?,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(LocalAgentHistoryUsage {
            calls: row.try_get("calls")?,
            input_tokens: row.try_get("input_tokens")?,
            output_tokens: row.try_get("output_tokens")?,
            cache_read_tokens: row.try_get("cache_read_tokens")?,
            cache_write_tokens: row.try_get("cache_write_tokens")?,
            last_sync_at_ms: self.local_agent_usage_sync().await?.last_sync_at_ms,
            sources,
            token_usage_series: fill_token_usage_series(
                recorded,
                series_start_ms,
                bucket_count,
                bucket_ms,
            ),
        })
    }

    pub async fn local_agent_history_status(&self) -> Result<LocalAgentHistoryStatus> {
        let sync = self.local_agent_usage_sync().await?;
        let sources = self.local_agent_usage_sources(None).await?;
        let event_count = sources.iter().map(|source| source.calls).sum();
        Ok(LocalAgentHistoryStatus {
            event_count,
            last_sync_at_ms: sync.last_sync_at_ms,
            last_error: sync.last_error,
            sources,
        })
    }

    pub async fn record_local_agent_usage_sync(&self, error: Option<String>) -> Result<()> {
        let previous = self.local_agent_usage_sync().await?;
        let last_sync_at_ms = if error.is_none() {
            Some(now_ms())
        } else {
            previous.last_sync_at_ms
        };
        self.set_local_agent_usage_sync(last_sync_at_ms, error)
            .await
    }

    async fn local_agent_usage_sources(
        &self,
        range: Option<(i64, i64)>,
    ) -> Result<Vec<LocalAgentUsageSource>> {
        let (where_clause, start_ms, end_ms) = match range {
            Some((start_ms, end_ms)) => (
                "WHERE created_at_ms >= ? AND created_at_ms < ?",
                start_ms,
                end_ms,
            ),
            None => ("", 0, 0),
        };
        let query = format!(
            "SELECT source,
                    COUNT(*) AS calls,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens,
                    COALESCE(SUM(cache_write_tokens), 0) AS cache_write_tokens
             FROM local_agent_usage_events
             {where_clause}
             GROUP BY source
             ORDER BY CASE source
                WHEN 'codex' THEN 0
                WHEN 'claude_code' THEN 1
                WHEN 'claude_cli' THEN 2
                ELSE 3
             END"
        );
        let mut query = sqlx::query(&query);
        if range.is_some() {
            query = query.bind(start_ms).bind(end_ms);
        }
        query
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok(LocalAgentUsageSource {
                    source: row.try_get("source")?,
                    calls: row.try_get("calls")?,
                    input_tokens: row.try_get("input_tokens")?,
                    output_tokens: row.try_get("output_tokens")?,
                    cache_read_tokens: row.try_get("cache_read_tokens")?,
                    cache_write_tokens: row.try_get("cache_write_tokens")?,
                })
            })
            .collect()
    }

    async fn local_agent_usage_sync(&self) -> Result<LocalAgentUsageSyncStatus> {
        let value = sqlx::query_scalar::<_, String>(
            "SELECT value_json FROM service_settings WHERE setting_key = ?",
        )
        .bind(LOCAL_AGENT_USAGE_STATUS_KEY)
        .fetch_optional(&self.pool)
        .await?;
        Ok(value
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default())
    }

    async fn set_local_agent_usage_sync(
        &self,
        last_sync_at_ms: Option<i64>,
        last_error: Option<String>,
    ) -> Result<()> {
        let status = LocalAgentUsageSyncStatus {
            last_sync_at_ms,
            last_error,
        };
        let value_json = serde_json::to_string(&status)?;
        let _write = self.writes.lock().await;
        sqlx::query(
            "INSERT INTO service_settings(setting_key, value_json, updated_at_ms) VALUES (?, ?, ?) ON CONFLICT(setting_key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
        )
        .bind(LOCAL_AGENT_USAGE_STATUS_KEY)
        .bind(value_json)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        key: &str,
        source: &str,
        timestamp: i64,
        input: i64,
        output: i64,
    ) -> LocalAgentUsageRecord {
        LocalAgentUsageRecord {
            event_key: key.into(),
            source: source.into(),
            occurred_at_ms: timestamp,
            model_key: "test".into(),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    #[tokio::test]
    async fn upserts_records_and_reports_source_totals() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap();
        store
            .upsert_local_agent_usage(&[
                record("codex-1", "codex", 1_000, 10, 2),
                record("claude-1", "claude_code", 2_000, 20, 3),
            ])
            .await
            .unwrap();
        store
            .upsert_local_agent_usage(&[record("codex-1", "codex", 1_000, 11, 2)])
            .await
            .unwrap();

        let totals = store
            .local_agent_usage_totals(0, i64::MAX, 0, 1, 86_400_000)
            .await
            .unwrap();
        assert_eq!(totals.calls, 2);
        assert_eq!(totals.input_tokens, 31);
        assert_eq!(totals.token_usage_series.len(), 1);
        assert_eq!(totals.sources.len(), 2);
        assert_eq!(totals.sources[0].source, "codex");
        assert_eq!(totals.sources[0].calls, 1);
    }
}
