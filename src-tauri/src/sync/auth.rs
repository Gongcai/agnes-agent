use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Serialize, Deserialize)]
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
        credential.validate()?;
        Ok(credential)
    }

    pub fn into_secret(self) -> AppResult<String> {
        let normalized = match self {
            Self::Bearer { token } => Self::Bearer {
                token: token.trim().to_string(),
            },
            Self::CloudflareAccess {
                client_id,
                client_secret,
            } => Self::CloudflareAccess {
                client_id: client_id.trim().to_string(),
                client_secret: client_secret.trim().to_string(),
            },
        };
        normalized.validate()?;
        serde_json::to_string(&normalized).map_err(Into::into)
    }

    fn validate(&self) -> AppResult<()> {
        match self {
            Self::Bearer { token } if token.trim().is_empty() => {
                Err(AppError::SecretStore("sync bearer token is empty".into()))
            }
            Self::Bearer { token } if token.len() > 512 => Err(AppError::SecretStore(
                "sync bearer token is too long".into(),
            )),
            Self::CloudflareAccess {
                client_id,
                client_secret,
            } if client_id.trim().is_empty() || client_secret.trim().is_empty() => Err(
                AppError::SecretStore("Cloudflare Access credential is incomplete".into()),
            ),
            Self::CloudflareAccess {
                client_id,
                client_secret,
            } if client_id.len() > 512 || client_secret.len() > 512 => Err(AppError::SecretStore(
                "Cloudflare Access credential is too long".into(),
            )),
            _ => Ok(()),
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
        assert_eq!(
            SyncCredential::Bearer {
                token: " token-with-space ".into()
            }
            .into_secret()
            .unwrap(),
            r#"{"kind":"bearer","token":"token-with-space"}"#
        );
    }
}
