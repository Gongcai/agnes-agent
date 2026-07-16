use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::sync::auth::SyncCredential;
use crate::sync::protocol::{
    AckRequest, AckResponse, BootstrapResponse, ErrorResponse, PullResponse, PushRequest,
    PushResponse,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Authentication,
    Permanent,
    Retryable,
}

#[derive(Debug, Clone)]
pub struct TransportFailure {
    pub kind: FailureKind,
    pub code: String,
    pub message: String,
    pub retry_after_ms: Option<i64>,
}

#[async_trait]
pub trait SyncTransport: Send + Sync {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, TransportFailure>;
    async fn pull(&self, after: i64, limit: usize) -> Result<PullResponse, TransportFailure>;
    async fn bootstrap(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<BootstrapResponse, TransportFailure>;
    async fn ack(&self, request: &AckRequest) -> Result<AckResponse, TransportFailure>;
}

pub struct HttpSyncTransport {
    client: reqwest::Client,
    gateway_url: String,
    credential: SyncCredential,
}

impl HttpSyncTransport {
    pub fn new(client: reqwest::Client, gateway_url: &str, credential: SyncCredential) -> Self {
        Self {
            client,
            gateway_url: gateway_url.trim_end_matches('/').to_string(),
            credential,
        }
    }

    async fn execute<T>(
        &self,
        request_builder: reqwest::RequestBuilder,
    ) -> Result<T, TransportFailure>
    where
        T: DeserializeOwned,
    {
        let response = request_builder
            .send()
            .await
            .map_err(|error| TransportFailure {
                kind: FailureKind::Retryable,
                code: "NETWORK_ERROR".into(),
                message: error.to_string(),
                retry_after_ms: None,
            })?;
        let status = response.status();
        let retry_after_ms = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok())
            .map(|seconds| seconds.saturating_mul(1_000));
        if status.is_success() {
            return response
                .json::<T>()
                .await
                .map_err(|error| TransportFailure {
                    kind: FailureKind::Retryable,
                    code: "INVALID_RESPONSE".into(),
                    message: error.to_string(),
                    retry_after_ms: None,
                });
        }

        let fallback_code = format!("HTTP_{}", status.as_u16());
        let body = response.json::<ErrorResponse>().await.ok();
        let code = body
            .as_ref()
            .map(|body| body.error.code.clone())
            .unwrap_or(fallback_code);
        let message = body
            .map(|body| body.error.message)
            .unwrap_or_else(|| format!("sync gateway returned HTTP {status}"));
        let kind = match status.as_u16() {
            401 | 403 => FailureKind::Authentication,
            400 | 404 | 409 | 413 => FailureKind::Permanent,
            429 | 500..=599 => FailureKind::Retryable,
            _ => FailureKind::Retryable,
        };
        Err(TransportFailure {
            kind,
            code,
            message,
            retry_after_ms,
        })
    }
}

#[async_trait]
impl SyncTransport for HttpSyncTransport {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, TransportFailure> {
        let request_builder = self
            .client
            .post(format!("{}/v1/sync/push", self.gateway_url));
        self.execute(self.credential.apply(request_builder).json(request))
            .await
    }

    async fn pull(&self, after: i64, limit: usize) -> Result<PullResponse, TransportFailure> {
        let request_builder = self
            .client
            .get(format!("{}/v1/sync/pull", self.gateway_url))
            .query(&[("after", after.to_string()), ("limit", limit.to_string())]);
        self.execute(self.credential.apply(request_builder)).await
    }

    async fn bootstrap(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<BootstrapResponse, TransportFailure> {
        let mut request_builder = self
            .client
            .get(format!("{}/v1/sync/bootstrap", self.gateway_url))
            .query(&[("limit", limit.to_string())]);
        if let Some(cursor) = cursor {
            request_builder = request_builder.query(&[("cursor", cursor)]);
        }
        self.execute(self.credential.apply(request_builder)).await
    }

    async fn ack(&self, request: &AckRequest) -> Result<AckResponse, TransportFailure> {
        let request_builder = self
            .client
            .post(format!("{}/v1/sync/ack", self.gateway_url));
        self.execute(self.credential.apply(request_builder).json(request))
            .await
    }
}
