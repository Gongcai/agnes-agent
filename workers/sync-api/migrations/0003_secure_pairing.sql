CREATE UNIQUE INDEX idx_devices_credential_fingerprint
  ON devices(credential_fingerprint)
  WHERE credential_fingerprint IS NOT NULL;

CREATE TABLE pairing_sessions (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL,
  initiator_device_id TEXT NOT NULL,
  initiator_message TEXT NOT NULL,
  responder_message TEXT,
  responder_proof TEXT,
  requested_device_id TEXT,
  requested_device_name TEXT,
  requested_platform TEXT,
  transfer_bundle TEXT,
  status TEXT NOT NULL CHECK (status IN ('open', 'joined', 'ready', 'consumed')),
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  joined_at INTEGER,
  finalized_at INTEGER,
  consumed_at INTEGER,
  FOREIGN KEY(initiator_device_id) REFERENCES devices(id)
);

CREATE INDEX idx_pairing_sessions_owner
  ON pairing_sessions(owner_id, status, expires_at);
