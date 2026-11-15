//! Shared helpers for imported token-usage aggregates.
use std::collections::BTreeMap;

use crate::model::TokenUsageBucket;

pub(crate) fn fill_token_usage_series(
    mut recorded: BTreeMap<i64, TokenUsageBucket>,
    series_start_ms: i64,
    bucket_count: usize,
    bucket_ms: i64,
) -> Vec<TokenUsageBucket> {
    (0..bucket_count)
        .map(|offset| {
            let bucket_start_ms = series_start_ms
                .saturating_add(i64::try_from(offset).unwrap_or(0).saturating_mul(bucket_ms));
            recorded
                .remove(&bucket_start_ms)
                .unwrap_or(TokenUsageBucket {
                    bucket_start_ms,
                    ..TokenUsageBucket::default()
                })
        })
        .collect()
}
