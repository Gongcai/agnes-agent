# Auto-backup Provider Limits and Design

Status: design note, checked 2026-08-25.

## Current upload limits

### Google Drive

Google publishes two different classes of limits. They must not be conflated:

- Drive API quota: 1,000,000 quota units per minute per project, 325,000 quota units per minute per user per project, and a 400,000,000 quota-unit daily billing threshold. The current page says quota is measured in method-dependent units, not upload bytes. It reports `files.get` as 5 units, `files.list` as 100, `files.update` as 50, and downloads as 200.
- Account upload capacity: Google Workspace users can upload or copy 750 GB per day across My Drive and shared drives. After reaching that limit, or attempting a file larger than 750 GB, additional uploads/copies are blocked for 24 hours. The maximum uploaded file size is 5 TB.
- Resumable uploads: the session URI expires after one week. Non-final chunks must be multiples of 256 KiB. The provider currently recommends 8 MiB chunks.

Sources:

- https://developers.google.com/workspace/drive/api/guides/limits
- https://developers.google.com/workspace/drive/api/guides/manage-uploads
- https://support.google.com/drive/answer/37603?hl=en

The application already uses Google resumable uploads and retries 429/5xx responses. The adapter now preserves `Retry-After` delays up to 60 seconds; a background backup queue should own longer pauses and retry the job later instead of treating the upload as permanently failed.

### Quark Drive

No public, stable numeric upload request-rate, bandwidth, or daily-upload quota was found for the current Quark Open API integration. The API root and documentation paths require signed request parameters and do not expose a public quota table. Quark can still apply account, device, IP, endpoint, or service-level throttling.

What is observable in the current adapter:

- The official adapter requests an upload part size from Quark and falls back to 4 MiB (`quark_drive_official.rs`). The server response is authoritative for `part_size`.
- The pre-upload request sets `parallel_upload: true`, but the application-level `StorageService` awaits one chunk before sending the next. There is currently one active chunk per upload job.
- HTTP 429 is mapped to `RateLimit`, and `Retry-After` is now preserved on API, download, and part-upload errors. The official adapter still does not retry a failed signed PUT or classify every possible provider-specific throttle code.
- The legacy adapter recognizes rate-limit words in API messages, but it is not the factory registered by `main.rs`.

Therefore the product must not claim a fixed Quark upload rate. It should begin with one upload worker per account, use the server-selected part size, and reduce concurrency or delay retries after the first 429/frequency error.

## Recommended auto-backup contract

The default backup unit should be an immutable, compressed, client-encrypted artifact, not a live SQLite database, WAL, or `sqlite-vec` file. The existing artifact manifest, replica, and transfer-job tables are suitable for this contract.

1. Build a consistent snapshot or domain artifact locally.
2. Compress, encrypt, and hash it before leaving the device.
3. Deduplicate by artifact ID and ciphertext hash.
4. Enqueue one upload job per provider account; keep at most one active upload per account initially.
5. Verify remote size and hash before marking the replica `ready`.
6. Persist the remote locator and resume metadata. Never delete the only local copy because a provider is throttling.
7. Retry rate-limit failures with exponential backoff and jitter. Honor `Retry-After` when present; otherwise use 1 minute, 5 minutes, 30 minutes, then hourly retry intervals with a daily cap.

Provider-specific defaults for the first version:

| Provider | Backup surface | Chunk | Initial concurrency | Daily safety budget |
| --- | --- | ---: | ---: | ---: |
| Google Drive | `appDataFolder` encrypted object replica | 8 MiB | 1/account | 700 GiB/account, configurable |
| Quark Drive | Dedicated user-visible backup folder until object storage is implemented | server `part_size` (fallback 4 MiB) | 1/account | adaptive; no hard numeric claim |

The Google 700 GiB budget leaves margin below the documented 750 GiB account limit. The Quark budget should be measured in successful bytes and 429/frequency events, with a circuit breaker that pauses the account after repeated throttling.

## Implementation gaps

- Add a persistent backup policy containing enabled source kinds, provider account IDs, schedule, retention, daily byte budget, and paused-until timestamp.
- Add a backup coordinator that creates immutable artifacts and feeds the existing transfer queue. It should be account-scoped so normal interactive uploads are not starved.
- Extend `ObjectStorageProvider` to Quark, or explicitly model Quark backups as ordinary files. The current Quark provider exposes `FileUploadProvider` only, so it cannot yet participate in the existing encrypted object-replica coordinator.
- Add safe retry for signed part PUTs and tests for 429 and provider-status throttling.
- Move Google delays longer than 60 seconds to the persisted job scheduler.
- Expose backup state, last successful snapshot, bytes used today, next retry time, and the reason for a pause in the storage settings UI.

Auto-backup should be opt-in, pause on authentication or quota errors, and remain resumable after application restart. Provider credentials stay in the OS keyring; the backup manifest must contain only encrypted locators and non-secret transfer metadata.
