//! Extracts token usage from Codex rollout JSONL files.
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use chrono::DateTime;
use serde_json::Value;

use crate::model::LocalAgentUsageRecord;

pub(super) fn scan_file(path: &Path) -> Result<Vec<LocalAgentUsageRecord>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| format!("{}: {error}", path.display()))?;
        if !line.contains("token_usage_record") {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            if let Some(record) = parse_record(&value) {
                records.push(record);
            }
        }
    }
    Ok(records)
}

fn parse_record(value: &Value) -> Option<LocalAgentUsageRecord> {
    if value.get("type")?.as_str()? != "token_usage_record" {
        return None;
    }
    let payload = value.get("payload")?;
    let usage = payload.get("usage")?;
    let input = non_negative(usage.get("input_tokens")?);
    let cached = non_negative(usage.get("cached_input_tokens").unwrap_or(&Value::Null));
    let cache_write = non_negative(
        usage
            .get("cache_write_input_tokens")
            .unwrap_or(&Value::Null),
    );
    let output = non_negative(usage.get("output_tokens")?);
    if input == 0 && output == 0 && cached == 0 && cache_write == 0 {
        return None;
    }
    let timestamp = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()?
        .timestamp_millis();
    let response_id = payload
        .get("response_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{}:{}:{}",
                payload
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                timestamp,
                value.get("ordinal").and_then(Value::as_i64).unwrap_or(0)
            )
        });
    Some(LocalAgentUsageRecord {
        event_key: format!("codex:{response_id}"),
        source: "codex".into(),
        occurred_at_ms: timestamp,
        model_key: "codex".into(),
        // Codex input_tokens includes cached prompt tokens.
        input_tokens: input.saturating_sub(cached).saturating_sub(cache_write),
        output_tokens: output,
        cache_read_tokens: cached,
        cache_write_tokens: cache_write,
    })
}

fn non_negative(value: &Value) -> i64 {
    value.as_i64().unwrap_or(0).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_usage_and_subtracts_cached_input() {
        let value = serde_json::json!({
            "timestamp": "2026-11-15T05:26:10.124Z",
            "type": "token_usage_record",
            "payload": {
                "response_id": "resp-1",
                "usage": {
                    "input_tokens": 29_863,
                    "cached_input_tokens": 8_192,
                    "cache_write_input_tokens": 0,
                    "output_tokens": 345
                }
            }
        });

        let record = parse_record(&value).unwrap();
        assert_eq!(record.event_key, "codex:resp-1");
        assert_eq!(record.input_tokens, 21_671);
        assert_eq!(record.cache_read_tokens, 8_192);
        assert_eq!(record.output_tokens, 345);
    }
}
