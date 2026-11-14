//! Collector service routes.
//!
//! This is the standalone aggregation center: a long-running HTTP service that
//! only exposes the ingestion endpoint and read-only overview queries. Member
//! desktops keep serving their own control API on loopback.
use std::net::SocketAddr;

use axum::{
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{collection::constant_time_eq, model::OverviewIngestBatch, store::Store};

const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
struct CollectorState {
    store: Store,
    api_key: String,
}

#[derive(Debug, Default, Deserialize)]
struct OverviewQuery {
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    model_keys: Option<String>,
    bucket_ms: Option<i64>,
}

/// Builds the collector router with a process-level API key.
pub fn router(store: Store, api_key: String) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/byok/v1/overview/ingest/ping", get(ping))
        .route("/byok/v1/overview/ingest", post(ingest))
        .route("/byok/v1/overview", get(overview))
        .route("/byok/v1/overview/models", get(models))
        .route("/byok/v1/overview/sources", get(sources))
        .route(
            "/byok/v1/overview/sources/{source_id}",
            axum::routing::delete(forget_source),
        )
        .layer(middleware::from_fn(log_request))
        .layer(DefaultBodyLimit::max(MAX_BATCH_BYTES))
        .layer(collector_cors())
        .with_state(CollectorState { store, api_key })
}

/// Access log for the collector. `/healthz` is skipped so container health
/// checks do not flood the log.
async fn log_request(request: Request, next: Next) -> Response {
    if request.uri().path() == "/healthz" {
        return next.run(request).await;
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    let status = response.status();
    let elapsed_ms = started.elapsed().as_millis();
    if status.is_server_error() {
        tracing::error!(%peer, %method, %path, status = status.as_u16(), elapsed_ms, "collector request failed");
    } else if status.is_client_error() {
        tracing::warn!(%peer, %method, %path, status = status.as_u16(), elapsed_ms, "collector request rejected");
    } else {
        tracing::info!(%peer, %method, %path, status = status.as_u16(), elapsed_ms, "collector request");
    }
    response
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn ping(State(state): State<CollectorState>, headers: HeaderMap) -> Response {
    match authorize(&state, &headers, "GET /byok/v1/overview/ingest/ping") {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(response) => *response,
    }
}

async fn ingest(State(state): State<CollectorState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(response) = authorize(&state, &headers, "POST /byok/v1/overview/ingest") {
        return *response;
    }
    let batch = match serde_json::from_slice::<OverviewIngestBatch>(&body) {
        Ok(batch) => batch,
        Err(error) => {
            tracing::warn!(error = %error, bytes = body.len(), "collector rejected a malformed batch");
            return api_error(
                StatusCode::BAD_REQUEST,
                format!("invalid overview batch: {error}"),
            );
        }
    };
    match state.store.ingest_collected_calls(&batch).await {
        Ok(inserted) => {
            tracing::info!(
                source_id = %batch.source_id,
                source_name = %batch.source_name,
                accepted = batch.calls.len(),
                inserted,
                "collector ingested a batch"
            );
            Json(json!({"inserted": inserted, "accepted": batch.calls.len()})).into_response()
        }
        Err(error) => {
            tracing::error!(
                source_id = %batch.source_id,
                error = %error,
                "collector failed to store a batch"
            );
            api_error(StatusCode::CONFLICT, error)
        }
    }
}

async fn overview(
    State(state): State<CollectorState>,
    headers: HeaderMap,
    Query(query): Query<OverviewQuery>,
) -> Response {
    if let Err(response) = authorize(&state, &headers, "GET /byok/v1/overview") {
        return *response;
    }
    let started = std::time::Instant::now();
    match state
        .store
        .collected_overview(
            query.start_ms,
            query.end_ms,
            query.model_keys.as_deref(),
            query.bucket_ms,
        )
        .await
    {
        Ok(overview) => {
            tracing::info!(
                start_ms = ?query.start_ms,
                end_ms = ?query.end_ms,
                bucket_ms = ?query.bucket_ms,
                models = query.model_keys.as_deref().unwrap_or(""),
                calls = overview.metrics.llm_calls,
                devices = overview.device_count,
                elapsed_ms = started.elapsed().as_millis(),
                "collector served an overview query"
            );
            Json(overview).into_response()
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn models(State(state): State<CollectorState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers, "GET /byok/v1/overview/models") {
        return *response;
    }
    match state.store.collected_overview_models().await {
        Ok(models) => Json(models).into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn sources(State(state): State<CollectorState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers, "GET /byok/v1/overview/sources") {
        return *response;
    }
    match state.store.overview_sources().await {
        Ok(sources) => Json(sources).into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn forget_source(
    State(state): State<CollectorState>,
    headers: HeaderMap,
    axum::extract::Path(source_id): axum::extract::Path<String>,
) -> Response {
    if let Err(response) = authorize(&state, &headers, "DELETE /byok/v1/overview/sources") {
        return *response;
    }
    match state.store.forget_overview_source(&source_id).await {
        Ok(()) => {
            tracing::info!(source_id = %source_id, "collector forgot a source");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

fn authorize(
    state: &CollectorState,
    headers: &HeaderMap,
    route: &str,
) -> std::result::Result<(), Box<Response>> {
    let supplied = supplied_key(headers).unwrap_or_default();
    if supplied.is_empty() || !constant_time_eq(supplied.as_bytes(), state.api_key.as_bytes()) {
        // Log the route and whether a key was presented, never the key itself.
        tracing::warn!(
            route,
            key_presented = !supplied.is_empty(),
            "collector rejected a request with an invalid API key"
        );
        return Err(Box::new(api_error(
            StatusCode::UNAUTHORIZED,
            "invalid API key",
        )));
    }
    Ok(())
}

fn supplied_key(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
        })
}

fn api_error(status: StatusCode, message: impl std::fmt::Display) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message.to_string(),
                "type": if status.is_client_error() { "invalid_request_error" } else { "server_error" },
            }
        })),
    )
        .into_response()
}

/// The desktop connects from its own loopback origin, and the collector may be
/// published on another host, so browser requests need an explicit allow-list.
fn collector_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            is_loopback_origin(origin)
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
}

fn is_loopback_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(parsed) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") || parsed.path() != "/" {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Domain(host)) => {
            host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("tauri.localhost")
        }
        Some(url::Host::Ipv4(address)) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        Some(url::Host::Ipv6(address)) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
        None => false,
    }
}

/// Logs the effective collector binding so operators can find the push URL.
pub fn log_binding(address: std::net::SocketAddr) {
    tracing::info!(
        %address,
        "overview collector listening on /byok/v1/overview/ingest"
    );
}

/// Surfaces a bind failure without stopping the desktop-side member service.
pub fn report_bind_error(error: &crate::Error) {
    tracing::error!(%error, "overview collector listener failed to start");
}
