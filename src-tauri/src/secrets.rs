use std::sync::Arc;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

use async_trait::async_trait;

use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

const KEYRING_SERVICE: &str = "com.agnes.agent";
const HEALTHCHECK_SECRET_ID: &str = "internal:keyring-healthcheck";

pub fn provider_api_key_secret_id(provider_id: &str) -> String {
    format!("provider:{provider_id}:api_key")
}

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, secret_id: &str) -> AppResult<Option<String>>;
    async fn set(&self, secret_id: &str, value: &str) -> AppResult<()>;
    async fn delete(&self, secret_id: &str) -> AppResult<()>;
}

#[derive(Debug, Default)]
pub struct OsSecretStore;

impl OsSecretStore {
    pub fn new() -> Self {
        Self
    }
}

fn keyring_error(operation: &str, error: keyring::Error) -> AppError {
    AppError::SecretStore(format!("{operation} failed: {error}"))
}

#[async_trait]
impl SecretStore for OsSecretStore {
    async fn get(&self, secret_id: &str) -> AppResult<Option<String>> {
        let secret_id = secret_id.to_string();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(KEYRING_SERVICE, &secret_id)
                .map_err(|error| keyring_error("open", error))?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(keyring_error("read", error)),
            }
        })
        .await
        .map_err(|error| AppError::SecretStore(format!("keyring task failed: {error}")))?
    }

    async fn set(&self, secret_id: &str, value: &str) -> AppResult<()> {
        let secret_id = secret_id.to_string();
        let value = value.to_string();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(KEYRING_SERVICE, &secret_id)
                .map_err(|error| keyring_error("open", error))?;
            entry
                .set_password(&value)
                .map_err(|error| keyring_error("write", error))
        })
        .await
        .map_err(|error| AppError::SecretStore(format!("keyring task failed: {error}")))?
    }

    async fn delete(&self, secret_id: &str) -> AppResult<()> {
        let secret_id = secret_id.to_string();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(KEYRING_SERVICE, &secret_id)
                .map_err(|error| keyring_error("open", error))?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(keyring_error("delete", error)),
            }
        })
        .await
        .map_err(|error| AppError::SecretStore(format!("keyring task failed: {error}")))?
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
pub struct InMemorySecretStore {
    values: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
#[async_trait]
impl SecretStore for InMemorySecretStore {
    async fn get(&self, secret_id: &str) -> AppResult<Option<String>> {
        Ok(self.values.lock().unwrap().get(secret_id).cloned())
    }

    async fn set(&self, secret_id: &str, value: &str) -> AppResult<()> {
        self.values
            .lock()
            .unwrap()
            .insert(secret_id.to_string(), value.to_string());
        Ok(())
    }

    async fn delete(&self, secret_id: &str) -> AppResult<()> {
        self.values.lock().unwrap().remove(secret_id);
        Ok(())
    }
}

pub async fn verify_secret_store(store: &dyn SecretStore) -> AppResult<()> {
    store.get(HEALTHCHECK_SECRET_ID).await.map(|_| ())
}

pub async fn migrate_legacy_provider_api_keys(
    db: &DbActorHandle,
    store: &dyn SecretStore,
) -> AppResult<usize> {
    let legacy_settings = db.list_settings_with_prefix("provider:".into()).await?;
    let mut migrated = 0;
    for (secret_id, legacy_value) in legacy_settings {
        if crate::sync::settings::classify(&secret_id)
            != crate::sync::settings::SettingClass::Secret
        {
            continue;
        }
        if legacy_value.is_empty() {
            db.delete_setting(secret_id).await?;
            continue;
        }

        match store.get(&secret_id).await? {
            None => store.set(&secret_id, &legacy_value).await?,
            Some(current) if current == legacy_value => {}
            Some(_) => {
                return Err(AppError::SecretStore(format!(
                    "credential conflict for `{secret_id}`; legacy value was preserved"
                )))
            }
        }
        let verified = store.get(&secret_id).await?;
        if verified.as_deref() != Some(legacy_value.as_str()) {
            return Err(AppError::SecretStore(format!(
                "credential verification failed for `{secret_id}`; legacy value was preserved"
            )));
        }
        db.delete_setting(secret_id).await?;
        migrated += 1;
    }
    Ok(migrated)
}

pub type SharedSecretStore = Arc<dyn SecretStore>;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("agnes-secret-{label}-{}.db", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn in_memory_store_round_trips_and_deletes_values() {
        let store = InMemorySecretStore::default();
        assert_eq!(store.get("provider:test:api_key").await.unwrap(), None);
        store.set("provider:test:api_key", "secret").await.unwrap();
        assert_eq!(
            store.get("provider:test:api_key").await.unwrap().as_deref(),
            Some("secret")
        );
        store.delete("provider:test:api_key").await.unwrap();
        assert_eq!(store.get("provider:test:api_key").await.unwrap(), None);
    }

    #[tokio::test]
    async fn migrates_and_removes_verified_legacy_provider_keys() {
        let path = test_db_path("migration");
        let db = crate::db::spawn_db_actor(path.clone());
        let legacy_id = provider_api_key_secret_id("openai");
        db.set_setting(legacy_id.clone(), "legacy-secret".into())
            .await
            .unwrap();
        let store = InMemorySecretStore::default();

        assert_eq!(
            migrate_legacy_provider_api_keys(&db, &store).await.unwrap(),
            1
        );
        assert_eq!(
            store.get(&legacy_id).await.unwrap().as_deref(),
            Some("legacy-secret")
        );
        assert_eq!(db.get_setting(legacy_id).await.unwrap(), None);

        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn preserves_legacy_value_when_keyring_has_a_conflict() {
        let path = test_db_path("conflict");
        let db = crate::db::spawn_db_actor(path.clone());
        let legacy_id = provider_api_key_secret_id("openai");
        db.set_setting(legacy_id.clone(), "legacy-secret".into())
            .await
            .unwrap();
        let store = InMemorySecretStore::default();
        store.set(&legacy_id, "keyring-secret").await.unwrap();

        let error = migrate_legacy_provider_api_keys(&db, &store)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("credential conflict"));
        assert_eq!(
            db.get_setting(legacy_id).await.unwrap().as_deref(),
            Some("legacy-secret")
        );

        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
