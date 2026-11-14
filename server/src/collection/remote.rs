//! Reads aggregates from a remote collector over HTTP.
use std::time::Duration;

use crate::{
    model::{Overview, OverviewModelOption, OverviewSource},
    network::NetworkClients,
    store::OverviewSyncSettings,
    Error, Result,
};

const READ_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub struct RemoteClient {
    clients: NetworkClients,
}

/// Query parameters mirrored on the collector's `/byok/v1/overview` route.
pub struct RemoteQuery<'a> {
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub model_keys: Option<&'a str>,
    pub bucket_ms: Option<i64>,
}

impl RemoteClient {
    pub fn new(clients: NetworkClients) -> Self {
        Self { clients }
    }

    pub async fn overview(
        &self,
        settings: &OverviewSyncSettings,
        query: &RemoteQuery<'_>,
    ) -> Result<Overview> {
        let mut url = self.base_url(settings)?;
        url.push_str("/byok/v1/overview");
        let mut params = Vec::new();
        if let Some(start_ms) = query.start_ms {
            params.push(format!("start_ms={start_ms}"));
        }
        if let Some(end_ms) = query.end_ms {
            params.push(format!("end_ms={end_ms}"));
        }
        if let Some(bucket_ms) = query.bucket_ms {
            params.push(format!("bucket_ms={bucket_ms}"));
        }
        if let Some(model_keys) = query.model_keys.filter(|value| !value.is_empty()) {
            params.push(format!(
                "model_keys={}",
                url::form_urlencoded::byte_serialize(model_keys.as_bytes()).collect::<String>()
            ));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        self.get(settings, &url).await
    }

    pub async fn models(
        &self,
        settings: &OverviewSyncSettings,
    ) -> Result<Vec<OverviewModelOption>> {
        let mut url = self.base_url(settings)?;
        url.push_str("/byok/v1/overview/models");
        self.get(settings, &url).await
    }

    pub async fn sources(&self, settings: &OverviewSyncSettings) -> Result<Vec<OverviewSource>> {
        let mut url = self.base_url(settings)?;
        url.push_str("/byok/v1/overview/sources");
        self.get(settings, &url).await
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        settings: &OverviewSyncSettings,
        url: &str,
    ) -> Result<T> {
        let client = self.clients.default_client().await?;
        let response = tokio::time::timeout(
            READ_TIMEOUT,
            client.get(url).bearer_auth(&settings.api_key).send(),
        )
        .await
        .map_err(|_| Error::Provider("overview collector read timed out".into()))??;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::Provider(format!(
                "overview collector rejected the request ({status}): {}",
                body.trim()
            )));
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    fn base_url(&self, settings: &OverviewSyncSettings) -> Result<String> {
        let base = settings.center_url.trim().trim_end_matches('/');
        if base.is_empty() {
            return Err(Error::Config(
                "overview collector address is not configured".into(),
            ));
        }
        Ok(base.to_owned())
    }
}
