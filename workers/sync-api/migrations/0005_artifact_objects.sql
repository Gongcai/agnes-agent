CREATE TABLE object_manifests (
  owner_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  object_kind TEXT NOT NULL,
  logical_version INTEGER NOT NULL CHECK(logical_version > 0),
  latest_artifact_id TEXT NOT NULL,
  ciphertext_hash TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size > 0),
  key_version INTEGER NOT NULL CHECK(key_version > 0),
  updated_hlc TEXT NOT NULL,
  deleted_at INTEGER,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(owner_id, object_id),
  UNIQUE(owner_id, latest_artifact_id)
);

CREATE TABLE object_replicas (
  owner_id TEXT NOT NULL,
  artifact_id TEXT NOT NULL,
  provider_kind TEXT NOT NULL,
  provider_account_id TEXT NOT NULL,
  opaque_server_key TEXT,
  encrypted_locator TEXT,
  provider_revision TEXT,
  etag TEXT,
  ciphertext_hash TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size > 0),
  status TEXT NOT NULL CHECK(status IN ('uploading','ready','failed','deleted')),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(owner_id, artifact_id, provider_kind, provider_account_id),
  UNIQUE(opaque_server_key)
);

CREATE TABLE object_changes (
  server_seq INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  artifact_id TEXT,
  operation TEXT NOT NULL CHECK(operation IN ('upsert','delete')),
  logical_version INTEGER NOT NULL CHECK(logical_version > 0),
  changed_at INTEGER NOT NULL
);

CREATE TABLE device_object_states (
  owner_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  observed_logical_version INTEGER NOT NULL CHECK(observed_logical_version >= 0),
  installed_artifact_id TEXT,
  local_status TEXT NOT NULL,
  verified_ciphertext_hash TEXT,
  checked_at INTEGER NOT NULL,
  error_code TEXT,
  PRIMARY KEY(owner_id, device_id, object_id),
  FOREIGN KEY(device_id) REFERENCES devices(id)
);

CREATE TABLE object_uploads (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  object_kind TEXT NOT NULL,
  logical_version INTEGER NOT NULL CHECK(logical_version > 0),
  artifact_id TEXT NOT NULL,
  opaque_server_key TEXT NOT NULL UNIQUE,
  r2_upload_id TEXT NOT NULL,
  ciphertext_hash TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size > 0),
  key_version INTEGER NOT NULL CHECK(key_version > 0),
  updated_hlc TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','completing','completed','aborted','failed')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  UNIQUE(owner_id, artifact_id)
);

CREATE TABLE object_upload_parts (
  upload_id TEXT NOT NULL REFERENCES object_uploads(id) ON DELETE CASCADE,
  part_number INTEGER NOT NULL CHECK(part_number BETWEEN 1 AND 10000),
  etag TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size > 0),
  ciphertext_hash TEXT NOT NULL,
  uploaded_at INTEGER NOT NULL,
  PRIMARY KEY(upload_id, part_number)
);

CREATE INDEX idx_object_manifests_owner_version
  ON object_manifests(owner_id, updated_at DESC, object_id);
CREATE INDEX idx_object_replicas_owner_status
  ON object_replicas(owner_id, status, updated_at DESC);
CREATE INDEX idx_object_changes_pull
  ON object_changes(owner_id, server_seq);
CREATE INDEX idx_device_object_states_device
  ON device_object_states(owner_id, device_id, checked_at DESC);
CREATE INDEX idx_object_uploads_expiry
  ON object_uploads(status, expires_at);
