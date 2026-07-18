use std::sync::Arc;

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::secrets::SecretStore;

use super::domain::{validate_account_id, ProviderError, ProviderErrorCategory, ProviderResult};
use super::ports::{ProviderCredentialAccess, ProviderCredentialStore};

const STORAGE_CREDENTIAL_PREFIX: &str = "storage";

pub struct KeyringProviderCredentialStore {
    secrets: Arc<dyn SecretStore>,
}

pub struct ScopedProviderCredentialAccess {
    account_id: String,
    store: Arc<dyn ProviderCredentialStore>,
}

impl ScopedProviderCredentialAccess {
    pub fn new(
        account_id: impl Into<String>,
        store: Arc<dyn ProviderCredentialStore>,
    ) -> ProviderResult<Self> {
        let account_id = account_id.into();
        validate_account_id(&account_id)?;
        Ok(Self { account_id, store })
    }
}

impl KeyringProviderCredentialStore {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }
}

pub fn storage_credential_secret_id(account_id: &str) -> ProviderResult<String> {
    validate_account_id(account_id)?;
    Ok(format!(
        "{STORAGE_CREDENTIAL_PREFIX}:{account_id}:credential"
    ))
}

#[async_trait]
impl ProviderCredentialStore for KeyringProviderCredentialStore {
    async fn load(&self, account_id: &str) -> ProviderResult<Option<Zeroizing<String>>> {
        let secret_id = storage_credential_secret_id(account_id)?;
        self.secrets
            .get(&secret_id)
            .await
            .map(|value| value.map(Zeroizing::new))
            .map_err(secret_error)
    }

    async fn store(&self, account_id: &str, credential: Zeroizing<String>) -> ProviderResult<()> {
        let secret_id = storage_credential_secret_id(account_id)?;
        self.secrets
            .set(&secret_id, credential.as_str())
            .await
            .map_err(secret_error)
    }

    async fn delete(&self, account_id: &str) -> ProviderResult<()> {
        let secret_id = storage_credential_secret_id(account_id)?;
        self.secrets.delete(&secret_id).await.map_err(secret_error)
    }
}

#[async_trait]
impl ProviderCredentialAccess for ScopedProviderCredentialAccess {
    async fn load(&self) -> ProviderResult<Option<Zeroizing<String>>> {
        self.store.load(&self.account_id).await
    }

    async fn store(&self, credential: Zeroizing<String>) -> ProviderResult<()> {
        self.store.store(&self.account_id, credential).await
    }

    async fn delete(&self) -> ProviderResult<()> {
        self.store.delete(&self.account_id).await
    }
}

fn secret_error(error: crate::error::AppError) -> ProviderError {
    ProviderError::new(
        ProviderErrorCategory::Authentication,
        format!("Unable to access storage credentials: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::InMemorySecretStore;

    #[tokio::test]
    async fn keyring_adapter_round_trips_without_exposing_secret_ids_to_providers() {
        let store = Arc::new(InMemorySecretStore::default());
        let credentials = KeyringProviderCredentialStore::new(store);
        credentials
            .store("account-1", Zeroizing::new("secret-cookie".into()))
            .await
            .unwrap();
        assert_eq!(
            credentials
                .load("account-1")
                .await
                .unwrap()
                .unwrap()
                .as_str(),
            "secret-cookie"
        );
        credentials.delete("account-1").await.unwrap();
        assert!(credentials.load("account-1").await.unwrap().is_none());
        assert!(storage_credential_secret_id("../../escape").is_err());

        let shared: Arc<dyn ProviderCredentialStore> = Arc::new(credentials);
        let account_one = ScopedProviderCredentialAccess::new("account-1", shared.clone()).unwrap();
        let account_two = ScopedProviderCredentialAccess::new("account-2", shared).unwrap();
        account_one
            .store(Zeroizing::new("first".into()))
            .await
            .unwrap();
        account_two
            .store(Zeroizing::new("second".into()))
            .await
            .unwrap();
        assert_eq!(account_one.load().await.unwrap().unwrap().as_str(), "first");
        assert_eq!(
            account_two.load().await.unwrap().unwrap().as_str(),
            "second"
        );
    }
}
