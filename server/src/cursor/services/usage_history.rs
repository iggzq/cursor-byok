//! Imports usage history from the signed-in Cursor account.
//!
//! Cursor's local files keep no per-call token counts, so account history is the
//! only source for usage recorded before this machine started logging calls, and
//! the only source for CmdK / Tab usage, which never reaches the local call log.
//! The API exposes no stable call id and reports timestamps in local order only,
//! so an import replaces the stored snapshot instead of merging into it.
use prost::Message;
use sha2::{Digest, Sha256};

use crate::{
    local_app::credential,
    model::CursorUsageRecord,
    network::NetworkClients,
    store::{now_ms, CursorHistoryStatus, Store},
    Error, Result,
};

const USAGE_EVENTS_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetFilteredUsageEvents";
const PAGE_SIZE: i32 = 1000;
const MAX_PAGES: i32 = 500;
const MAX_ATTEMPTS: u32 = 4;
/// The shared Cursor client has no overall timeout (proxy traffic streams for
/// minutes), so history requests bound themselves instead of hanging a sync.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// `aiserver.v1.DashboardService/GetFilteredUsageEvents` request subset.
#[derive(Clone, PartialEq, Message)]
struct GetFilteredUsageEventsRequest {
    #[prost(int32, tag = "1")]
    team_id: i32,
    #[prost(int64, optional, tag = "2")]
    start_date: Option<i64>,
    #[prost(int64, optional, tag = "3")]
    end_date: Option<i64>,
    #[prost(int32, optional, tag = "6")]
    page: Option<i32>,
    #[prost(int32, optional, tag = "7")]
    page_size: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct GetFilteredUsageEventsResponse {
    #[prost(int32, tag = "2")]
    total_usage_events_count: i32,
    #[prost(message, repeated, tag = "3")]
    usage_events_display: Vec<UsageEventDisplay>,
}

#[derive(Clone, PartialEq, Message)]
struct UsageEventDisplay {
    #[prost(int64, tag = "1")]
    timestamp: i64,
    #[prost(string, tag = "2")]
    model: String,
    #[prost(message, optional, tag = "9")]
    token_usage: Option<TokenUsage>,
    #[prost(string, optional, tag = "23")]
    conversation_id: Option<String>,
    #[prost(enumeration = "UsageEventKind", tag = "3")]
    kind: i32,
}

#[derive(Clone, Copy, PartialEq, Message)]
struct TokenUsage {
    #[prost(int32, tag = "1")]
    input_tokens: i32,
    #[prost(int32, tag = "2")]
    output_tokens: i32,
    #[prost(int32, tag = "3")]
    cache_write_tokens: i32,
    #[prost(int32, tag = "4")]
    cache_read_tokens: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
enum UsageEventKind {
    Unspecified = 0,
    UsageBased = 1,
    UserApiKey = 2,
    IncludedInPro = 3,
    IncludedInBusiness = 4,
    ErroredNotCharged = 5,
    AbortedNotCharged = 6,
    CustomSubscription = 7,
    IncludedInProPlus = 8,
    IncludedInUltra = 9,
    FreeCredit = 10,
}

impl UsageEventDisplay {
    fn into_record(self) -> Option<CursorUsageRecord> {
        // Composer, agent, CmdK and Tab all surface here; every kind that
        // reported real tokens belongs to the account's token consumption.
        let usage = self.token_usage?;
        let tokens = [
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_write_tokens,
        ];
        if self.timestamp <= 0
            || self.model.trim().is_empty()
            || tokens.iter().all(|value| *value <= 0)
        {
            return None;
        }
        Some(CursorUsageRecord {
            event_key: Self::event_key(
                self.timestamp,
                &self.model,
                self.conversation_id.as_deref().unwrap_or_default(),
                &usage,
            ),
            occurred_at_ms: self.timestamp,
            model_key: self.model.trim().to_owned(),
            conversation_id: self.conversation_id.unwrap_or_default(),
            input_tokens: i64::from(usage.input_tokens.max(0)),
            output_tokens: i64::from(usage.output_tokens.max(0)),
            cache_read_tokens: i64::from(usage.cache_read_tokens.max(0)),
            cache_write_tokens: i64::from(usage.cache_write_tokens.max(0)),
        })
    }

    fn event_key(timestamp: i64, model: &str, conversation_id: &str, usage: &TokenUsage) -> String {
        let identity = format!(
            "{timestamp}|{model}|{conversation_id}|{}|{}|{}|{}",
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_write_tokens,
            usage.cache_read_tokens
        );
        format!(
            "cursor:{}",
            hex::encode(Sha256::digest(identity.as_bytes()))
        )
    }
}

/// Replaces the imported history with a fresh account snapshot. The local call
/// log covers BYOK-era traffic, while account history supplies official Cursor
/// usage before BYOK plus CmdK / Tab calls that never reach the local log.
pub async fn sync(store: &Store, clients: &NetworkClients) -> Result<CursorHistoryStatus> {
    let outcome = async {
        if !store.overview_sync_settings().await?.include_cursor_history {
            return Err(Error::Config(
                "Cursor history import is disabled in the overview settings".into(),
            ));
        }
        let token = credential::access_token().await?;
        let client = clients.cursor_client().await?;
        import(store, &client, &token).await
    }
    .await;
    store
        .record_cursor_usage_sync(outcome.as_ref().err().map(ToString::to_string))
        .await?;
    outcome?;
    let included = store.overview_sync_settings().await?.include_cursor_history;
    store.cursor_history_status(included).await
}

async fn import(store: &Store, client: &reqwest::Client, token: &str) -> Result<usize> {
    import_at(store, client, token).await
}

async fn import_at(store: &Store, client: &reqwest::Client, token: &str) -> Result<usize> {
    let mut records = Vec::new();
    let mut occurrences = std::collections::HashMap::<String, usize>::new();
    let mut page = 1;
    loop {
        let response = fetch_page_from(client, USAGE_EVENTS_URL, token, page).await?;
        let page_len = response.usage_events_display.len();
        records.extend(
            response
                .usage_events_display
                .into_iter()
                .filter_map(UsageEventDisplay::into_record)
                .map(|mut record| {
                    let occurrence = occurrences.entry(record.event_key.clone()).or_default();
                    *occurrence += 1;
                    if *occurrence > 1 {
                        record.event_key = format!("{}:{}", record.event_key, occurrence);
                    }
                    record
                }),
        );
        if page_len < PAGE_SIZE as usize || page >= MAX_PAGES {
            break;
        }
        page += 1;
    }
    store.replace_cursor_usage(&records).await
}

async fn fetch_page_from(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    page: i32,
) -> Result<GetFilteredUsageEventsResponse> {
    let end_ms = now_ms().saturating_add(86_400_000);
    let request = GetFilteredUsageEventsRequest {
        team_id: 0,
        start_date: Some(0),
        end_date: Some(end_ms),
        page: Some(page),
        page_size: Some(PAGE_SIZE),
    };
    let mut last_error = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500 << attempt)).await;
        }
        let response = match tokio::time::timeout(
            REQUEST_TIMEOUT,
            client
                .post(url)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
                .header(reqwest::header::CONTENT_TYPE, "application/proto")
                .header("connect-protocol-version", "1")
                .body(request.encode_to_vec())
                .send(),
        )
        .await
        {
            Ok(response) => response,
            Err(_) => {
                last_error = Some(Error::Provider(format!(
                    "Cursor usage history request timed out after {}s",
                    REQUEST_TIMEOUT.as_secs()
                )));
                continue;
            }
        };
        match response {
            Ok(response) if response.status().is_success() => {
                let body = tokio::time::timeout(REQUEST_TIMEOUT, response.bytes())
                    .await
                    .map_err(|_| {
                        Error::Provider(format!(
                            "Cursor usage history response timed out after {}s",
                            REQUEST_TIMEOUT.as_secs()
                        ))
                    })??;
                return GetFilteredUsageEventsResponse::decode(body.as_ref()).map_err(|error| {
                    Error::Protocol(format!("cannot decode Cursor usage history: {error}"))
                });
            }
            Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
                return Err(Error::Config(
                    "Cursor rejected the stored account token; sign in to Cursor again".into(),
                ));
            }
            Ok(response) => {
                last_error = Some(Error::Provider(format!(
                    "Cursor usage history request failed with status {}",
                    response.status()
                )));
            }
            Err(error) => last_error = Some(error.into()),
        }
    }
    Err(last_error
        .unwrap_or_else(|| Error::Provider("Cursor usage history request did not complete".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(timestamp: i64, input: i32, output: i32, cache_read: i32) -> UsageEventDisplay {
        UsageEventDisplay {
            timestamp,
            model: "cursor-grok-4.5".into(),
            token_usage: Some(TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cache_write_tokens: 0,
                cache_read_tokens: cache_read,
            }),
            conversation_id: Some("conversation".into()),
            kind: UsageEventKind::UsageBased as i32,
        }
    }

    #[test]
    fn keeps_records_that_reported_tokens() {
        let record = display(1_000, 10, 2, 30).into_record().unwrap();
        assert_eq!(record.occurred_at_ms, 1_000);
        assert_eq!(record.model_key, "cursor-grok-4.5");
        assert_eq!(record.input_tokens, 10);
        assert_eq!(record.cache_read_tokens, 30);
    }

    #[test]
    fn drops_records_without_usage() {
        assert!(display(1_000, 0, 0, 0).into_record().is_none());
        let mut missing = display(1_000, 5, 1, 0);
        missing.token_usage = None;
        assert!(missing.into_record().is_none());
    }

    #[test]
    fn every_usage_kind_contributes_tokens() {
        // Composer, agent, CmdK and Tab all report through the account stream.
        for kind in [
            UsageEventKind::UsageBased,
            UsageEventKind::IncludedInPro,
            UsageEventKind::IncludedInUltra,
            UsageEventKind::UserApiKey,
        ] {
            let mut event = display(1_000, 7, 1, 0);
            event.kind = kind as i32;
            assert!(event.into_record().is_some(), "{kind:?}");
        }
    }

    #[tokio::test]
    async fn import_replaces_history_with_every_account_event() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap();
        let mut recorded = display(5_000, 99, 9, 0);
        recorded.conversation_id = Some("recorded".into());
        let body = GetFilteredUsageEventsResponse {
            total_usage_events_count: 2,
            usage_events_display: vec![display(1_000, 10, 1, 0), recorded],
        }
        .encode_to_vec();
        let server = serve_once(body).await;
        let client = reqwest::Client::new();
        let imported = import_against(&store, &client, &server).await.unwrap();

        assert_eq!(imported, 2);
        let totals = store
            .cursor_usage_totals(0, i64::MAX, 0, 1, 86_400_000)
            .await
            .unwrap();
        assert_eq!(totals.calls, 2);
        assert_eq!(totals.input_tokens, 109);
        assert_eq!(totals.token_usage_series.len(), 1);
    }

    /// Exercises the page loop against a local stand-in for the usage endpoint.
    async fn import_against(store: &Store, client: &reqwest::Client, url: &str) -> Result<usize> {
        let response = fetch_page_from(client, url, "token", 1).await?;
        let records = response
            .usage_events_display
            .into_iter()
            .filter_map(UsageEventDisplay::into_record)
            .collect::<Vec<_>>();
        store.replace_cursor_usage(&records).await
    }

    /// Serves one decode-able usage page and returns its address as a URL base.
    async fn serve_once(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 8192];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/proto\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let mut payload = response.into_bytes();
            payload.extend_from_slice(&body);
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, &payload).await;
        });
        format!("http://{address}")
    }
}
