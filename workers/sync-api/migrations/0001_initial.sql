PRAGMA foreign_keys = ON;

CREATE TABLE devices (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL,
  name TEXT NOT NULL,
  platform TEXT,
  credential_fingerprint TEXT,
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER,
  revoked_at INTEGER
);

CREATE INDEX idx_devices_owner
  ON devices(owner_id, revoked_at);

CREATE TABLE sync_entities (
  owner_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  revision INTEGER NOT NULL CHECK (revision > 0),
  hlc TEXT NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
  payload_schema_version INTEGER NOT NULL CHECK (payload_schema_version > 0),
  payload_encoding TEXT NOT NULL,
  payload BLOB,
  payload_hash TEXT NOT NULL,
  key_version INTEGER,
  changed_by_device_id TEXT NOT NULL,
  latest_server_seq INTEGER NOT NULL,
  latest_change_id TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(owner_id, entity_type, entity_id),
  FOREIGN KEY(changed_by_device_id) REFERENCES devices(id)
);

CREATE INDEX idx_sync_entities_bootstrap
  ON sync_entities(owner_id, entity_type, entity_id, latest_server_seq);

CREATE TABLE sync_changes (
  server_seq INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_id TEXT NOT NULL,
  change_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
  base_revision INTEGER,
  resulting_revision INTEGER NOT NULL CHECK (resulting_revision > 0),
  hlc TEXT NOT NULL,
  payload_schema_version INTEGER NOT NULL CHECK (payload_schema_version > 0),
  payload_encoding TEXT NOT NULL,
  payload BLOB,
  payload_hash TEXT NOT NULL,
  key_version INTEGER,
  created_at INTEGER NOT NULL,
  accepted_at INTEGER NOT NULL,
  UNIQUE(owner_id, change_id),
  FOREIGN KEY(device_id) REFERENCES devices(id)
);

CREATE INDEX idx_sync_changes_pull
  ON sync_changes(owner_id, server_seq);

CREATE INDEX idx_sync_changes_entity
  ON sync_changes(owner_id, entity_type, entity_id, server_seq);

CREATE TRIGGER sync_changes_set_entity_server_seq
AFTER INSERT ON sync_changes
BEGIN
  UPDATE sync_entities
  SET latest_server_seq = NEW.server_seq
  WHERE owner_id = NEW.owner_id
    AND entity_type = NEW.entity_type
    AND entity_id = NEW.entity_id
    AND latest_change_id = NEW.change_id;
END;

CREATE TABLE sync_acks (
  owner_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  last_server_seq INTEGER NOT NULL CHECK (last_server_seq >= 0),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(owner_id, device_id),
  FOREIGN KEY(device_id) REFERENCES devices(id)
);
