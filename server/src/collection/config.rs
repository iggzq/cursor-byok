//! Configuration for the standalone overview collector service.
use std::net::SocketAddr;

use crate::{Error, Result};

/// Environment-driven configuration for `cursor-overview-collector`.
///
/// | Variable | Meaning |
/// | --- | --- |
/// | `CURSOR_OVERVIEW_COLLECTOR_ADDR` | Listen address, default `0.0.0.0:4399` |
/// | `CURSOR_OVERVIEW_COLLECTOR_API_KEY` | Shared key members must present |
/// | `CURSOR_DATABASE_URL` | SQLite URL, default `sqlite://overview-collector.db` |
#[derive(Clone, Debug, Default)]
pub struct CollectorConfig {
    pub listen_addr: Option<SocketAddr>,
    pub api_key: Option<String>,
    pub database_url: Option<String>,
}

impl CollectorConfig {
    pub fn from_env() -> Result<Self> {
        let listen_addr = match std::env::var("CURSOR_OVERVIEW_COLLECTOR_ADDR") {
            Ok(value) => Some(value.parse().map_err(|error| {
                Error::Config(format!("invalid CURSOR_OVERVIEW_COLLECTOR_ADDR: {error}"))
            })?),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => {
                return Err(Error::Config(format!(
                    "invalid CURSOR_OVERVIEW_COLLECTOR_ADDR: {error}"
                )))
            }
        };
        Ok(Self {
            listen_addr,
            api_key: optional_env("CURSOR_OVERVIEW_COLLECTOR_API_KEY"),
            database_url: optional_env("CURSOR_DATABASE_URL"),
        })
    }

    /// Rejects configurations the service cannot run with.
    pub fn validate(&self) -> Result<()> {
        match &self.api_key {
            Some(key) if !key.trim().is_empty() => Ok(()),
            _ => Err(Error::Config(
                "CURSOR_OVERVIEW_COLLECTOR_API_KEY is required; members authenticate with it"
                    .into(),
            )),
        }
    }
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}
