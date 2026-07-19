use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::sync::auth::SyncCredential;
use crate::sync::protocol::{
    AckRequest, AckResponse, BootstrapResponse, CreatePairingSessionRequest,
    CreatePairingSessionResponse, DeviceListResponse, ErrorResponse, FinalizePairingSessionRequest,
    FinalizePairingSessionResponse, JoinPairingSessionRequest, ObjectChangesResponse,
    ObjectManifestResponse, ObjectStateRequest, ObjectStateResponse, PairingJoinResponse,
    PairingPackageResponse, PairingStatusResponse, PublicPairingSessionResponse, PullResponse,
    PushRequest, PushResponse, RevokeDeviceResponse,
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

#[async_trait]
pub trait ObjectSyncTransport: Send + Sync {
    async fn list_object_changes(
        &self,
        after: i64,
        limit: usize,
    ) -> Result<ObjectChangesResponse, TransportFailure>;
    async fn get_object_manifest(
        &self,
        object_id: &str,
    ) -> Result<ObjectManifestResponse, TransportFailure>;
    async fn update_object_state(
        &self,
        request: &ObjectStateRequest,
    ) -> Result<ObjectStateResponse, TransportFailure>;
}

pub struct HttpSyncTransport {
    client: reqwest::Client,
    gateway_url: String,
    credential: Option<SyncCredential>,
}

impl HttpSyncTransport {
    pub fn new(client: reqwest::Client, gateway_url: &str, credential: SyncCredential) -> Self {
        Self {
            client,
            gateway_url: gateway_url.trim_end_matches('/').to_string(),
            credential: Some(credential),
        }
    }

    pub fn new_public(client: reqwest::Client, gateway_url: &str) -> Self {
        Self {
            client,
            gateway_url: gateway_url.trim_end_matches('/').to_string(),
            credential: None,
        }
    }

    fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, TransportFailure> {
        self.credential
            .as_ref()
            .map(|credential| credential.apply(request))
            .ok_or_else(|| TransportFailure {
                kind: FailureKind::Authentication,
                code: "UNAUTHENTICATED".into(),
                message: "Sync credential is not configured".into(),
                retry_after_ms: None,
            })
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
            400 | 404 | 409 | 410 | 413 => FailureKind::Permanent,
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

    pub async fn list_devices(&self) -> Result<DeviceListResponse, TransportFailure> {
        let request_builder = self.client.get(format!("{}/v1/devices", self.gateway_url));
        self.execute(self.authenticate(request_builder)?).await
    }

    pub async fn revoke_device(
        &self,
        device_id: &str,
    ) -> Result<RevokeDeviceResponse, TransportFailure> {
        let request_builder = self.client.post(format!(
            "{}/v1/devices/{}/revoke",
            self.gateway_url, device_id
        ));
        self.execute(self.authenticate(request_builder)?).await
    }

    pub async fn create_pairing_session(
        &self,
        request: &CreatePairingSessionRequest,
    ) -> Result<CreatePairingSessionResponse, TransportFailure> {
        let request_builder = self
            .client
            .post(format!("{}/v1/pairing/sessions", self.gateway_url));
        self.execute(self.authenticate(request_builder)?.json(request))
            .await
    }

    pub async fn get_pairing_join(
        &self,
        session_id: &str,
    ) -> Result<PairingJoinResponse, TransportFailure> {
        let request_builder = self.client.get(format!(
            "{}/v1/pairing/sessions/{session_id}/join",
            self.gateway_url
        ));
        self.execute(self.authenticate(request_builder)?).await
    }

    pub async fn finalize_pairing_session(
        &self,
        session_id: &str,
        request: &FinalizePairingSessionRequest,
    ) -> Result<FinalizePairingSessionResponse, TransportFailure> {
        let request_builder = self.client.post(format!(
            "{}/v1/pairing/sessions/{session_id}/finalize",
            self.gateway_url
        ));
        self.execute(self.authenticate(request_builder)?.json(request))
            .await
    }

    pub async fn get_public_pairing_session(
        &self,
        session_id: &str,
    ) -> Result<PublicPairingSessionResponse, TransportFailure> {
        let request_builder = self.client.get(format!(
            "{}/v1/pairing/sessions/{session_id}",
            self.gateway_url
        ));
        self.execute(request_builder).await
    }

    pub async fn join_pairing_session(
        &self,
        session_id: &str,
        request: &JoinPairingSessionRequest,
    ) -> Result<PairingStatusResponse, TransportFailure> {
        let request_builder = self.client.post(format!(
            "{}/v1/pairing/sessions/{session_id}/join",
            self.gateway_url
        ));
        self.execute(request_builder.json(request)).await
    }

    pub async fn get_pairing_package(
        &self,
        session_id: &str,
    ) -> Result<PairingPackageResponse, TransportFailure> {
        let request_builder = self.client.get(format!(
            "{}/v1/pairing/sessions/{session_id}/package",
            self.gateway_url
        ));
        self.execute(request_builder).await
    }

    pub async fn consume_pairing_session(
        &self,
        session_id: &str,
    ) -> Result<PairingStatusResponse, TransportFailure> {
        let request_builder = self.client.post(format!(
            "{}/v1/pairing/sessions/{session_id}/consume",
            self.gateway_url
        ));
        self.execute(self.authenticate(request_builder)?).await
    }
}

#[async_trait]
impl SyncTransport for HttpSyncTransport {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, TransportFailure> {
        let request_builder = self
            .client
            .post(format!("{}/v1/sync/push", self.gateway_url));
        self.execute(self.authenticate(request_builder)?.json(request))
            .await
    }

    async fn pull(&self, after: i64, limit: usize) -> Result<PullResponse, TransportFailure> {
        let request_builder = self
            .client
            .get(format!("{}/v1/sync/pull", self.gateway_url))
            .query(&[("after", after.to_string()), ("limit", limit.to_string())]);
        self.execute(self.authenticate(request_builder)?).await
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
        self.execute(self.authenticate(request_builder)?).await
    }

    async fn ack(&self, request: &AckRequest) -> Result<AckResponse, TransportFailure> {
        let request_builder = self
            .client
            .post(format!("{}/v1/sync/ack", self.gateway_url));
        self.execute(self.authenticate(request_builder)?.json(request))
            .await
    }
}

#[async_trait]
impl ObjectSyncTransport for HttpSyncTransport {
    async fn list_object_changes(
        &self,
        after: i64,
        limit: usize,
    ) -> Result<ObjectChangesResponse, TransportFailure> {
        if after < 0 || !(1..=500).contains(&limit) {
            return Err(invalid_request(
                "Object change cursor or page limit is invalid",
            ));
        }
        let request_builder = self
            .client
            .get(format!("{}/v1/objects/changes", self.gateway_url))
            .query(&[("after", after.to_string()), ("limit", limit.to_string())]);
        self.execute(self.authenticate(request_builder)?).await
    }

    async fn get_object_manifest(
        &self,
        object_id: &str,
    ) -> Result<ObjectManifestResponse, TransportFailure> {
        validate_object_id(object_id)?;
        let request_builder = self.client.get(format!(
            "{}/v1/objects/manifests/{object_id}",
            self.gateway_url
        ));
        self.execute(self.authenticate(request_builder)?).await
    }

    async fn update_object_state(
        &self,
        request: &ObjectStateRequest,
    ) -> Result<ObjectStateResponse, TransportFailure> {
        validate_object_id(&request.object_id)?;
        let request_builder = self
            .client
            .post(format!("{}/v1/objects/states", self.gateway_url));
        self.execute(self.authenticate(request_builder)?.json(request))
            .await
    }
}

fn validate_object_id(value: &str) -> Result<(), TransportFailure> {
    let mut bytes = value.bytes();
    let first = bytes.next();
    let valid = value.len() <= 128
        && first.is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(invalid_request("Object ID is invalid"))
    }
}

fn invalid_request(message: &str) -> TransportFailure {
    TransportFailure {
        kind: FailureKind::Permanent,
        code: "INVALID_REQUEST".into(),
        message: message.into(),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids_are_safe_for_manifest_paths() {
        assert!(validate_object_id("knowledge:collection-1").is_ok());
        assert!(validate_object_id("").is_err());
        assert!(validate_object_id("../escape").is_err());
        assert!(validate_object_id("contains/slash").is_err());
    }
}
