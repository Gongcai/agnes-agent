// Write/download ports are consumed by provider adapters in the next phase.
#![allow(dead_code)]

pub mod artifact_cache;
pub mod artifact_transfer;
pub mod credentials;
pub mod domain;
pub mod google_drive;
pub mod ports;
pub mod quark_drive;
pub mod r2;
pub mod registry;
pub mod service;

pub use credentials::{
    KeyringProviderCredentialStore, ScopedProviderCredentialAccess, SyncProviderCredentialAccess,
};
pub use domain::*;
pub use google_drive::GoogleDriveFactory;
pub use quark_drive::QuarkDriveFactory;
pub use r2::{R2Factory, MANAGED_R2_ACCOUNT_ID, R2_PROVIDER_ID, SYNC_CREDENTIAL_SOURCE};
pub use registry::StorageProviderRegistry;
pub use service::{StorageImportKind, StorageService};
