//! Extracts token usage from Claude Code sessions and Claude CLI usage ledgers.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use chrono::DateTime;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::LocalAgentUsageRecord;

pub(super) fn scan_code_projects(path: &Path) -> Result<Vec<LocalAgentUsageRecord>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut records = BTreeMap::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| format!("{}: {error}", path.display()))?;
        if !line.contains("\"usage\"") {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            if let Some(record) = parse_code_record(&value) {
                records.insert(record.event_key.clone(), record);
            }
        }
    }
    Ok(records.into_values().collect())
}

pub(super) fn scan_usage_ledger(path: &Path) -> Result<Vec<LocalAgentUsageRecord>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut records = Vec::new();
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| format!("{}: {error}", path.display()))?;
        if !line.contains("inputTokens") {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            records.extend(parse_ledger_record(path, line_number + 1, &value));
        }
    }
    Ok(records)
}

fn parse_code_record(value: &Value) -> Option<LocalAgentUsageRecord> {
    if value.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let input = non_negative(usage.get("input_tokens")?);
    let cache_read = non_negative(usage.get("cache_read_input_tokens").unwrap_or(&Value::Null));
    let cache_write = non_negative(
        usage
            .get("cache_creation_input_tokens")
            .unwrap_or(&Value::Null),
    );
    let output = non_negative(usage.get("output_tokens")?);
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return None;
    }
    let timestamp = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()?
        .timestamp_millis();
    let session = value
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let request = value
        .get("requestRef")
        .and_then(Value::as_str)
        .or_else(|| message.get("id").and_then(Value::as_str))
        .or_else(|| value.get("uuid").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    Some(LocalAgentUsageRecord {
        event_key: format!("claude-code:{session}:{request}"),
        source: "claude_code".into(),
        occurred_at_ms: timestamp,
        model_key: message
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("claude-code")
            .to_owned(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
    })
}

fn parse_ledger_record(
    path: &Path,
    line_number: usize,
    value: &Value,
) -> Vec<LocalAgentUsageRecord> {
    let Some(timestamp) = value.get("ts").and_then(Value::as_i64) else {
        return Vec::new();
    };
    let Some(models) = value.get("models").and_then(Value::as_object) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|(model, usage)| {
            let input = non_negative(usage.get("inputTokens").unwrap_or(&Value::Null));
            let output = non_negative(usage.get("outputTokens").unwrap_or(&Value::Null));
            let cache_read = non_negative(usage.get("cacheReadTokens").unwrap_or(&Value::Null));
            let write_total = non_negative(usage.get("cacheWriteTokens").unwrap_or(&Value::Null));
            let cache_write = if write_total > 0 {
                write_total
            } else {
                non_negative(usage.get("cacheWrite5mTokens").unwrap_or(&Value::Null))
                    .saturating_add(non_negative(
                        usage.get("cacheWrite1hTokens").unwrap_or(&Value::Null),
                    ))
            };
            if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
                return None;
            }
            let identity = format!("{}:{line_number}:{timestamp}:{model}", path.display());
            let digest = Sha256::digest(identity.as_bytes());
            Some(LocalAgentUsageRecord {
                event_key: format!("claude-cli:{}", hex::encode(digest)),
                source: "claude_cli".into(),
                occurred_at_ms: timestamp,
                model_key: model.clone(),
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
            })
        })
        .collect()
}

fn non_negative(value: &Value) -> i64 {
    value.as_i64().unwrap_or(0).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_code_usage_once_by_request() {
        let value = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-11-08T12:22:50.099Z",
            "sessionId": "session-1",
            "requestRef": "request-1",
            "message": {
                "id": "message-1",
                "model": "claude-sonnet-4-5",
                "usage": {
                    "input_tokens": 10,
                    "cache_creation_input_tokens": 21_235,
                    "cache_read_input_tokens": 7,
                    "output_tokens": 27
                }
            }
        });

        let record = parse_code_record(&value).unwrap();
        assert_eq!(record.event_key, "claude-code:session-1:request-1");
        assert_eq!(record.input_tokens, 10);
        assert_eq!(record.cache_read_tokens, 7);
        assert_eq!(record.cache_write_tokens, 21_235);
    }

    #[test]
    fn parses_claude_cli_ledger_models() {
        let value = serde_json::json!({
            "ts": 1_794_140_581_693_i64,
            "models": {
                "claude-haiku-4-5": {
                    "inputTokens": 8,
                    "outputTokens": 1,
                    "cacheReadTokens": 2,
                    "cacheWriteTokens": 0,
                    "cacheWrite5mTokens": 3,
                    "cacheWrite1hTokens": 4
                }
            }
        });

        let records = parse_ledger_record(Path::new("/tmp/ledger.ndjson"), 1, &value);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, "claude_cli");
        assert_eq!(records[0].cache_write_tokens, 7);
    }
}
