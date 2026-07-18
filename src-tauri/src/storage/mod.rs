// Write/download ports are consumed by provider adapters in the next phase.
#![allow(dead_code)]

pub mod credentials;
pub mod domain;
pub mod google_drive;
pub mod ports;
pub mod registry;
pub mod service;

pub use credentials::{KeyringProviderCredentialStore, ScopedProviderCredentialAccess};
pub use domain::*;
pub use google_drive::GoogleDriveFactory;
pub use registry::StorageProviderRegistry;
pub use service::StorageService;
