//! Persists usage imported from the Cursor account usage API.
//!
//! The account API exposes no stable per-call id, so a sync replaces the table
//! atomically instead of upserting individual records.
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{
    model::{CursorHistoryUsage, CursorUsageRecord, TokenUsageBucket},
    Result,
};

use super::{now_ms, usage::fill_token_usage_series, Store};

const CURSOR_USAGE_STATUS_KEY: &str = "cursor_usage_sync";

/// Visibility for the manual Cursor history import.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct CursorHistoryStatus {
    pub included: bool,
    pub event_count: i64,
    pub last_sync_at_ms: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CursorUsageSyncStatus {
    #[serde(default)]
    last_sync_at_ms: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
}

impl Store {
    /// Replaces every imported record with one account snapshot.
    pub async fn replace_cursor_usage(&self, records: &[CursorUsageRecord]) -> Result<usize> {
        let _write = self.writes.lock().await;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM cursor_usage_events")
            .execute(&mut *transaction)
            .await?;
        for record in records {
            sqlx::query(
                "INSERT INTO cursor_usage_events(
                    event_key, created_at_ms, model_key, display_name, conversation_id,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&record.event_key)
            .bind(record.occurred_at_ms)
            .bind(&record.model_key)
            .bind(&record.model_key)
            .bind(&record.conversation_id)
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

    pub async fn clear_cursor_usage(&self) -> Result<()> {
        {
            let _write = self.writes.lock().await;
            sqlx::query("DELETE FROM cursor_usage_events")
                .execute(&self.pool)
                .await?;
        }
        self.set_cursor_usage_sync(None, None).await
    }

    /// Imported usage inside `[start_ms, end_ms)`. The per-bucket series mirrors
    /// the aggregate's own series: same start, same bucket count, zero-filled so
    /// both can be added bucket by bucket.
    /// Per-model filters are ignored because Cursor reports its own model names.
    pub async fn cursor_usage_totals(
        &self,
        start_ms: i64,
        end_ms: i64,
        series_start_ms: i64,
        bucket_count: usize,
        bucket_ms: i64,
    ) -> Result<CursorHistoryUsage> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS calls,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens,
                    COALESCE(SUM(cache_write_tokens), 0) AS cache_write_tokens
             FROM cursor_usage_events
             WHERE created_at_ms >= ? AND created_at_ms < ?",
        )
        .bind(start_ms)
        .bind(end_ms)
        .fetch_one(&self.pool)
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
             FROM cursor_usage_events
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
            .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
        Ok(CursorHistoryUsage {
            calls: row.try_get("calls")?,
            input_tokens: row.try_get("input_tokens")?,
            output_tokens: row.try_get("output_tokens")?,
            cache_read_tokens: row.try_get("cache_read_tokens")?,
            cache_write_tokens: row.try_get("cache_write_tokens")?,
            last_sync_at_ms: self.cursor_usage_sync().await?.last_sync_at_ms,
            token_usage_series: fill_token_usage_series(
                recorded,
                series_start_ms,
                bucket_count,
                bucket_ms,
            ),
        })
    }

    pub async fn cursor_history_status(&self, included: bool) -> Result<CursorHistoryStatus> {
        let sync = self.cursor_usage_sync().await?;
        let event_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM cursor_usage_events")
            .fetch_one(&self.pool)
            .await?;
        Ok(CursorHistoryStatus {
            included,
            event_count,
            last_sync_at_ms: sync.last_sync_at_ms,
            last_error: sync.last_error,
        })
    }

    pub async fn record_cursor_usage_sync(&self, error: Option<String>) -> Result<()> {
        let previous = self.cursor_usage_sync().await?;
        let last_sync_at_ms = if error.is_none() {
            Some(now_ms())
        } else {
            previous.last_sync_at_ms
        };
        self.set_cursor_usage_sync(last_sync_at_ms, error).await
    }

    async fn cursor_usage_sync(&self) -> Result<CursorUsageSyncStatus> {
        let value = sqlx::query_scalar::<_, String>(
            "SELECT value_json FROM service_settings WHERE setting_key = ?",
        )
        .bind(CURSOR_USAGE_STATUS_KEY)
        .fetch_optional(&self.pool)
        .await?;
        Ok(value
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default())
    }

    async fn set_cursor_usage_sync(
        &self,
        last_sync_at_ms: Option<i64>,
        last_error: Option<String>,
    ) -> Result<()> {
        let status = CursorUsageSyncStatus {
            last_sync_at_ms,
            last_error,
        };
        let value_json = serde_json::to_string(&status)?;
        let _write = self.writes.lock().await;
        sqlx::query(
            "INSERT INTO service_settings(setting_key, value_json, updated_at_ms) VALUES (?, ?, ?) ON CONFLICT(setting_key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
        )
        .bind(CURSOR_USAGE_STATUS_KEY)
        .bind(value_json)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
