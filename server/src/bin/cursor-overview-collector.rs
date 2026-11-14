//! Standalone overview aggregation center.
//!
//! This binary runs on a central host and only serves the collector API: member
//! machines push call records here, and any desktop in remote mode reads the
//! aggregates back. It shares the SQLite store implementation with the desktop
//! but has no Cursor forwarding, plugin runtime, or desktop control API.
use std::{future::IntoFuture, net::SocketAddr, time::Duration};

use cursor_server::{collection, collection::CollectorConfig, store::Store, Error, Result};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::prelude::*;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:4399";
const DEFAULT_DATABASE_URL: &str = "sqlite://overview-collector.db";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cursor_server=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = CollectorConfig::from_env()?;
    config.validate()?;
    let listen_addr = config
        .listen_addr
        .unwrap_or_else(|| DEFAULT_LISTEN_ADDR.parse().expect("valid default address"));
    let database_url = config
        .database_url
        .clone()
        .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_owned());
    let api_key = config.api_key.clone().unwrap_or_default();
    serve(listen_addr, database_url, api_key).await
}

async fn serve(listen_addr: SocketAddr, database_url: String, api_key: String) -> Result<()> {
    let store = Store::connect(&database_url).await?;
    let listener = TcpListener::bind(listen_addr).await.map_err(|error| {
        tracing::error!(
            addr = %listen_addr,
            error = %error,
            "overview collector cannot bind its listen address; another process may already use this port"
        );
        Error::Io(error)
    })?;
    let address = listener.local_addr()?;
    collection::log_binding(address);
    tracing::info!(
        %database_url,
        "overview collector started; members push to /byok/v1/overview/ingest"
    );

    let shutdown = CancellationToken::new();
    let signal_shutdown = shutdown.clone();
    let router = collection::router(store, api_key);
    let graceful = shutdown.clone();
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { graceful.cancelled().await })
    .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => result.map_err(Error::from),
        () = shutdown_signal() => {
            tracing::info!("shutdown signal received; stopping collector");
            signal_shutdown.cancel();
            match tokio::time::timeout(Duration::from_secs(10), &mut server).await {
                Ok(result) => result.map_err(Error::from),
                Err(_) => {
                    tracing::warn!("graceful shutdown timed out; forcing collector close");
                    Ok(())
                }
            }
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}
