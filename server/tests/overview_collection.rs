#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use cursor_server::{
    collection::CollectorConfig,
    control,
    model::{CollectedCall, NewLlmCall, OverviewIngestBatch, ProviderType, Usage},
    network::NetworkClients,
    plugin::{PluginRegistry, PluginRuntime},
    store::{ExternalApiSettings, OverviewMode, OverviewSyncSettings},
};
use serde_json::{json, Value};
use tower::ServiceExt;

const KEY: &str = "collector-secret";

async fn setup() -> (
    cursor_server::store::Store,
    tempfile::TempDir,
    control::ControlService,
) {
    let (directory, store) = fixtures::temp_store().await;
    let runtime = PluginRuntime::managed().unwrap();
    let plugins = PluginRegistry::managed(store.clone(), runtime.clone(), "0.1.0".into()).unwrap();
    let service = control::ControlService::new(
        store.clone(),
        Arc::new(fake_provider::FakeProvider::default()),
        runtime,
        plugins,
        NetworkClients::new(store.clone()),
    )
    .unwrap();
    (store, directory, service)
}

fn collector_router(store: cursor_server::store::Store) -> axum::Router {
    cursor_server::collection::router(store, KEY.to_owned())
}

async fn send(
    router: axum::Router,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        request = request.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    let response = router
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn write_call(store: &cursor_server::store::Store, call_id: &str, status: &str) {
    store
        .start_llm_call(&NewLlmCall {
            call_id: call_id.into(),
            run_id: "run".into(),
            conversation_id: "conversation".into(),
            provider_call_index: 0,
            model_hash: "model".into(),
            provider_type: ProviderType::OpenAiChat,
            provider_url: "https://api.example.com".into(),
            request_type: ProviderType::OpenAiChat,
            request_url: "https://api.example.com/v1/chat/completions".into(),
            model_id: "gpt-test".into(),
            display_name: "GPT Test".into(),
            reasoning_effort: None,
            fast: false,
            message_count: 1,
            tool_count: 0,
            detailed: false,
        })
        .await
        .unwrap();
    store
        .record_llm_usage(
            call_id,
            Usage {
                input_tokens: Some(120),
                output_tokens: Some(30),
                ..Usage::default()
            },
        )
        .await
        .unwrap();
    store
        .finish_llm_call(call_id, status, Some("stop"), 10, None, None)
        .await
        .unwrap();
}

fn collected_call(call_id: &str) -> CollectedCall {
    CollectedCall {
        call_id: call_id.into(),
        created_at_ms: 1_000,
        finished_at_ms: 1_100,
        request_type: "openai-chat".into(),
        model_key: "openai-chat:gpt-test".into(),
        display_name: "GPT Test".into(),
        status: "completed".into(),
        input_tokens: Some(120),
        output_tokens: Some(30),
        cache_read_tokens: Some(0),
        cache_write_tokens: Some(0),
    }
}

fn batch(source_id: &str, call_id: &str) -> Value {
    json!({
        "source_id": source_id,
        "source_name": "MacBook",
        "calls": [collected_call(call_id)],
    })
}

#[tokio::test]
async fn collector_requires_its_process_key() {
    let (store, _directory, _service) = setup().await;
    let router = collector_router(store.clone());
    let body = batch("source-a", "call-1");

    for key in [None, Some("wrong")] {
        let (status, _) = send(
            router.clone(),
            "POST",
            "/byok/v1/overview/ingest",
            key,
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let (status, _) = send(
        router.clone(),
        "POST",
        "/byok/v1/overview/ingest",
        Some(KEY),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn collector_read_routes_are_also_protected() {
    let (store, _directory, _service) = setup().await;
    let router = collector_router(store);
    for path in [
        "/byok/v1/overview",
        "/byok/v1/overview/models",
        "/byok/v1/overview/sources",
    ] {
        let (status, _) = send(router.clone(), "GET", path, None, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    let (status, _) = send(router.clone(), "GET", "/healthz", None, json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn repeated_ingestion_keeps_a_single_record_per_call() {
    let (store, _directory, _service) = setup().await;
    let router = collector_router(store.clone());
    for _ in 0..3 {
        let (status, _) = send(
            router.clone(),
            "POST",
            "/byok/v1/overview/ingest",
            Some(KEY),
            batch("source-a", "call-1"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send(
        router.clone(),
        "GET",
        "/byok/v1/overview?start_ms=0&end_ms=10000",
        Some(KEY),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let overview: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(overview["metrics"]["llm_calls"], 1);
    assert_eq!(overview["device_count"], 1);

    let (status, body) = send(
        router,
        "GET",
        "/byok/v1/overview/sources",
        Some(KEY),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sources: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(sources[0]["name"], "MacBook");
    assert_eq!(sources[0]["call_count"], 1);
}

#[tokio::test]
async fn collector_serves_model_filters_from_stable_keys() {
    let (store, _directory, _service) = setup().await;
    let router = collector_router(store);
    let (status, _) = send(
        router.clone(),
        "POST",
        "/byok/v1/overview/ingest",
        Some(KEY),
        batch("source-a", "call-1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        router.clone(),
        "GET",
        "/byok/v1/overview/models",
        Some(KEY),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let models: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(models[0]["key"], "openai-chat:gpt-test");

    let (status, body) = send(
        router,
        "GET",
        "/byok/v1/overview?start_ms=0&end_ms=10000&model_keys=%5B%22other%3Ax%22%5D",
        Some(KEY),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let overview: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(overview["metrics"]["llm_calls"], 0);
}

#[tokio::test]
async fn collector_forgets_a_source_on_request() {
    let (store, _directory, _service) = setup().await;
    let router = collector_router(store);
    send(
        router.clone(),
        "POST",
        "/byok/v1/overview/ingest",
        Some(KEY),
        batch("source-a", "call-1"),
    )
    .await;

    let (status, _) = send(
        router.clone(),
        "DELETE",
        "/byok/v1/overview/sources/source-a",
        Some(KEY),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = send(
        router,
        "GET",
        "/byok/v1/overview/sources",
        Some(KEY),
        json!({}),
    )
    .await;
    let sources: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(sources.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn remote_mode_no_longer_reads_a_local_collected_table() {
    let (store, _directory, service) = setup().await;
    write_call(&store, "local-1", "completed").await;
    store
        .ingest_collected_calls(&OverviewIngestBatch {
            source_id: "source-a".into(),
            source_name: "MacBook".into(),
            calls: vec![collected_call("remote-1")],
        })
        .await
        .unwrap();
    let router = control::api_router(service);

    let (status, body) = send(
        router.clone(),
        "GET",
        "/__byok-api__/api/overview",
        None,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let overview: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(overview["metrics"]["llm_calls"], 1);
    assert_eq!(overview["device_count"], 0);

    // Remote mode requires a reachable collector, so an unreachable address must
    // surface an error instead of silently falling back to local numbers.
    store
        .set_overview_sync_settings(OverviewSyncSettings {
            mode: OverviewMode::Remote,
            center_url: "http://127.0.0.1:1".into(),
            api_key: "secret".into(),
            device_name: "MacBook".into(),
            ..OverviewSyncSettings::default()
        })
        .await
        .unwrap();
    let (status, body) = send(router, "GET", "/__byok-api__/api/overview", None, json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("overview"));
}

#[tokio::test]
async fn remote_mode_requires_a_collector_address_and_key() {
    let (store, _directory, service) = setup().await;
    assert!(store
        .set_overview_sync_settings(OverviewSyncSettings {
            mode: OverviewMode::Remote,
            ..OverviewSyncSettings::default()
        })
        .await
        .is_err());

    let saved = store
        .set_overview_sync_settings(OverviewSyncSettings {
            mode: OverviewMode::Remote,
            center_url: "http://127.0.0.1:4399/".into(),
            api_key: "secret".into(),
            device_name: "MacBook".into(),
            ..OverviewSyncSettings::default()
        })
        .await
        .unwrap();
    assert_eq!(saved.center_url, "http://127.0.0.1:4399");
    assert_eq!(
        saved.ingest_url().as_deref(),
        Some("http://127.0.0.1:4399/byok/v1/overview/ingest")
    );
    assert!(!saved.device_id.is_empty());

    let router = control::api_router(service);
    let (status, _) = send(
        router,
        "GET",
        "/__byok-api__/api/settings/overview-sync",
        None,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn local_calls_are_pushed_incrementally_once_each() {
    let (store, _directory, _service) = setup().await;
    write_call(&store, "call-1", "completed").await;
    write_call(&store, "call-2", "failed").await;

    let watermark = store.overview_sync_watermark().await.unwrap();
    let pending = store.pending_collected_calls(&watermark).await.unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].model_key, "openai-chat:gpt-test");
    assert!(pending.iter().all(|call| call.status != "running"));

    let batch = OverviewIngestBatch {
        source_id: "source-a".into(),
        source_name: "MacBook".into(),
        calls: pending,
    };
    let next = cursor_server::store::next_watermark(&batch, &watermark);
    store.set_overview_sync_watermark(&next).await.unwrap();
    assert!(store
        .pending_collected_calls(&store.overview_sync_watermark().await.unwrap())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn calls_sharing_a_finish_timestamp_are_not_skipped_at_a_batch_boundary() {
    let (store, _directory, _service) = setup().await;
    write_call(&store, "call-a", "completed").await;
    write_call(&store, "call-b", "completed").await;
    sqlx::query("UPDATE llm_calls SET finished_at_ms = 5_000")
        .execute(store.pool())
        .await
        .unwrap();

    let watermark = store.overview_sync_watermark().await.unwrap();
    let first = store.pending_collected_calls(&watermark).await.unwrap();
    let consumed = OverviewIngestBatch {
        source_id: "source-a".into(),
        source_name: "MacBook".into(),
        calls: vec![first[0].clone()],
    };
    let next = cursor_server::store::next_watermark(&consumed, &watermark);
    store.set_overview_sync_watermark(&next).await.unwrap();

    let remaining = store.pending_collected_calls(&next).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].call_id, "call-b");
}

#[tokio::test]
async fn resetting_progress_requeues_the_whole_history() {
    let (store, _directory, _service) = setup().await;
    write_call(&store, "call-a", "completed").await;
    write_call(&store, "call-b", "completed").await;

    let watermark = store.overview_sync_watermark().await.unwrap();
    let pending = store.pending_collected_calls(&watermark).await.unwrap();
    let batch = OverviewIngestBatch {
        source_id: "source-a".into(),
        source_name: "MacBook".into(),
        calls: pending,
    };
    let next = cursor_server::store::next_watermark(&batch, &watermark);
    store.set_overview_sync_watermark(&next).await.unwrap();
    assert_eq!(store.pending_collected_call_count(&next).await.unwrap(), 0);

    // Switching collectors or wiping one must be recoverable: resetting the
    // cursor re-queues every finished call, and the collector dedupes by call id.
    let status = store.reset_overview_sync_watermark().await.unwrap();
    assert_eq!(status.pending_calls, 2);
    assert_eq!(
        store
            .pending_collected_calls(&store.overview_sync_watermark().await.unwrap())
            .await
            .unwrap()
            .len(),
        2
    );

    // Re-uploading the same records twice stays idempotent at the collector.
    store.ingest_collected_calls(&batch).await.unwrap();
    store.ingest_collected_calls(&batch).await.unwrap();
    // `write_call` stamps real wall-clock times, so query the whole range.
    let overview = store
        .collected_overview(Some(0), Some(i64::MAX), None, None)
        .await
        .unwrap();
    assert_eq!(overview.metrics.llm_calls, 2);
}

#[tokio::test]
async fn collector_configuration_requires_an_api_key() {
    assert!(CollectorConfig::default().validate().is_err());
    let config = CollectorConfig {
        api_key: Some("secret".into()),
        ..CollectorConfig::default()
    };
    assert!(config.validate().is_ok());
}

#[tokio::test]
async fn external_api_settings_stay_independent_from_collection_settings() {
    let (store, _directory, _service) = setup().await;
    store
        .set_external_api_settings(ExternalApiSettings {
            enabled: true,
            api_key: "inference-key".into(),
        })
        .await
        .unwrap();
    let saved = store
        .set_overview_sync_settings(OverviewSyncSettings {
            mode: OverviewMode::Remote,
            center_url: "http://center.example:4399".into(),
            api_key: "collector-key".into(),
            device_name: "MacBook".into(),
            ..OverviewSyncSettings::default()
        })
        .await
        .unwrap();

    assert_eq!(
        store.external_api_settings().await.unwrap().api_key,
        "inference-key"
    );
    assert_eq!(saved.api_key, "collector-key");
}
