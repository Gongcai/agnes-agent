use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use super::domain::{
    validate_provider_id, ProviderDescriptor, ProviderError, ProviderErrorCategory, ProviderResult,
};
use super::ports::ProviderFactory;

#[derive(Default)]
pub struct StorageProviderRegistry {
    factories: RwLock<BTreeMap<String, RegisteredProvider>>,
}

struct RegisteredProvider {
    descriptor: ProviderDescriptor,
    factory: Arc<dyn ProviderFactory>,
}

impl StorageProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn register(&self, factory: Arc<dyn ProviderFactory>) -> ProviderResult<()> {
        let descriptor = factory.descriptor();
        descriptor.validate()?;
        let mut factories = self.factories.write().map_err(lock_error)?;
        if factories.contains_key(&descriptor.id) {
            return Err(ProviderError::new(
                ProviderErrorCategory::Conflict,
                format!("Storage provider `{}` is already registered", descriptor.id),
            ));
        }
        factories.insert(
            descriptor.id.clone(),
            RegisteredProvider {
                descriptor,
                factory,
            },
        );
        Ok(())
    }

    pub fn descriptors(&self) -> ProviderResult<Vec<ProviderDescriptor>> {
        let factories = self.factories.read().map_err(lock_error)?;
        Ok(factories
            .values()
            .map(|provider| provider.descriptor.clone())
            .collect())
    }

    pub fn descriptor(&self, provider_id: &str) -> ProviderResult<ProviderDescriptor> {
        validate_provider_id(provider_id)?;
        self.factories
            .read()
            .map_err(lock_error)?
            .get(provider_id)
            .map(|provider| provider.descriptor.clone())
            .ok_or_else(|| provider_missing(provider_id))
    }

    pub fn factory(&self, provider_id: &str) -> ProviderResult<Arc<dyn ProviderFactory>> {
        validate_provider_id(provider_id)?;
        self.factories
            .read()
            .map_err(lock_error)?
            .get(provider_id)
            .map(|provider| provider.factory.clone())
            .ok_or_else(|| provider_missing(provider_id))
    }
}

fn provider_missing(provider_id: &str) -> ProviderError {
    ProviderError::new(
        ProviderErrorCategory::Unsupported,
        format!("Storage provider `{provider_id}` is not installed"),
    )
}

fn lock_error<T>(_error: std::sync::PoisonError<T>) -> ProviderError {
    ProviderError::new(
        ProviderErrorCategory::RemoteUnavailable,
        "Storage provider registry is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::domain::{
        ProviderAuthKind, ProviderStability, StorageCapabilities, StorageProviderAccount,
    };
    use crate::storage::ports::{ProviderCredentialAccess, ProviderFactory, ProviderSession};
    use async_trait::async_trait;

    struct FakeFactory(&'static str);
    struct FakeSession;

    impl ProviderSession for FakeSession {}

    #[async_trait]
    impl ProviderFactory for FakeFactory {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: self.0.into(),
                display_name: "Fake Drive".into(),
                auth_kind: ProviderAuthKind::Managed,
                stability: ProviderStability::Experimental,
                implementation_version: "test-v1".into(),
                capabilities: StorageCapabilities::default(),
            }
        }

        async fn connect(
            &self,
            _account: &StorageProviderAccount,
            _credentials: Arc<dyn ProviderCredentialAccess>,
        ) -> ProviderResult<Arc<dyn ProviderSession>> {
            Ok(Arc::new(FakeSession))
        }
    }

    #[test]
    fn registry_is_open_for_new_providers_and_rejects_duplicate_ids() {
        let registry = StorageProviderRegistry::new();
        registry
            .register(Arc::new(FakeFactory("future_drive")))
            .unwrap();
        assert_eq!(registry.descriptors().unwrap()[0].id, "future_drive");
        assert!(registry.factory("future_drive").is_ok());
        assert!(registry
            .register(Arc::new(FakeFactory("future_drive")))
            .is_err());
        assert!(registry.factory("missing").is_err());
    }
}
