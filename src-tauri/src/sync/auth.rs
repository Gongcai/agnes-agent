use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncCredential {
    Bearer {
        token: String,
    },
    CloudflareAccess {
        client_id: String,
        client_secret: String,
    },
}

impl SyncCredential {
    pub fn parse(secret: &str) -> AppResult<Self> {
        let credential: Self = serde_json::from_str(secret)
            .map_err(|error| AppError::SecretStore(format!("invalid sync credential: {error}")))?;
        match &credential {
            Self::Bearer { token } if token.trim().is_empty() => {
                Err(AppError::SecretStore("sync bearer token is empty".into()))
            }
            Self::CloudflareAccess {
                client_id,
                client_secret,
            } if client_id.trim().is_empty() || client_secret.trim().is_empty() => Err(
                AppError::SecretStore("Cloudflare Access credential is incomplete".into()),
            ),
            _ => Ok(credential),
        }
    }

    pub fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::Bearer { token } => request.bearer_auth(token),
            Self::CloudflareAccess {
                client_id,
                client_secret,
            } => request
                .header("CF-Access-Client-Id", client_id)
                .header("CF-Access-Client-Secret", client_secret),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer_and_access_credentials_without_exposing_fields() {
        assert!(matches!(
            SyncCredential::parse(r#"{"kind":"bearer","token":"test-token"}"#).unwrap(),
            SyncCredential::Bearer { .. }
        ));
        assert!(matches!(
            SyncCredential::parse(
                r#"{"kind":"cloudflare_access","client_id":"id","client_secret":"secret"}"#
            )
            .unwrap(),
            SyncCredential::CloudflareAccess { .. }
        ));
        assert!(SyncCredential::parse(r#"{"kind":"bearer","token":""}"#).is_err());
    }
}
