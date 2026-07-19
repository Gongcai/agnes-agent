import { blob, index, integer, primaryKey, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

export const devices = sqliteTable(
  "devices",
  {
    id: text("id").primaryKey(),
    ownerId: text("owner_id").notNull(),
    name: text("name").notNull(),
    platform: text("platform"),
    credentialFingerprint: text("credential_fingerprint"),
    createdAt: integer("created_at").notNull(),
    lastSeenAt: integer("last_seen_at"),
    revokedAt: integer("revoked_at"),
  },
  (table) => [
    index("idx_devices_owner").on(table.ownerId, table.revokedAt),
    uniqueIndex("idx_devices_credential_fingerprint").on(table.credentialFingerprint),
  ],
);

export const pairingSessions = sqliteTable(
  "pairing_sessions",
  {
    id: text("id").primaryKey(),
    ownerId: text("owner_id").notNull(),
    initiatorDeviceId: text("initiator_device_id").notNull(),
    initiatorMessage: text("initiator_message").notNull(),
    responderMessage: text("responder_message"),
    responderProof: text("responder_proof"),
    requestedDeviceId: text("requested_device_id"),
    requestedDeviceName: text("requested_device_name"),
    requestedPlatform: text("requested_platform"),
    transferBundle: text("transfer_bundle"),
    status: text("status").notNull(),
    createdAt: integer("created_at").notNull(),
    expiresAt: integer("expires_at").notNull(),
    joinedAt: integer("joined_at"),
    finalizedAt: integer("finalized_at"),
    consumedAt: integer("consumed_at"),
  },
  (table) => [index("idx_pairing_sessions_owner").on(table.ownerId, table.status, table.expiresAt)],
);

export const syncEntities = sqliteTable(
  "sync_entities",
  {
    ownerId: text("owner_id").notNull(),
    entityType: text("entity_type").notNull(),
    entityId: text("entity_id").notNull(),
    revision: integer("revision").notNull(),
    hlc: text("hlc").notNull(),
    deleted: integer("deleted", { mode: "boolean" }).notNull().default(false),
    payloadSchemaVersion: integer("payload_schema_version").notNull(),
    payloadEncoding: text("payload_encoding").notNull(),
    payload: blob("payload"),
    payloadHash: text("payload_hash").notNull(),
    keyVersion: integer("key_version"),
    changedByDeviceId: text("changed_by_device_id").notNull(),
    latestServerSeq: integer("latest_server_seq").notNull(),
    latestChangeId: text("latest_change_id").notNull(),
    updatedAt: integer("updated_at").notNull(),
  },
  (table) => [
    primaryKey({ columns: [table.ownerId, table.entityType, table.entityId] }),
    index("idx_sync_entities_bootstrap").on(
      table.ownerId,
      table.entityType,
      table.entityId,
      table.latestServerSeq,
    ),
  ],
);

export const syncChanges = sqliteTable(
  "sync_changes",
  {
    serverSeq: integer("server_seq").primaryKey({ autoIncrement: true }),
    ownerId: text("owner_id").notNull(),
    changeId: text("change_id").notNull(),
    deviceId: text("device_id").notNull(),
    entityType: text("entity_type").notNull(),
    entityId: text("entity_id").notNull(),
    operation: text("operation").notNull(),
    baseRevision: integer("base_revision"),
    resultingRevision: integer("resulting_revision").notNull(),
    hlc: text("hlc").notNull(),
    payloadSchemaVersion: integer("payload_schema_version").notNull(),
    payloadEncoding: text("payload_encoding").notNull(),
    payload: blob("payload"),
    payloadHash: text("payload_hash").notNull(),
    keyVersion: integer("key_version"),
    createdAt: integer("created_at").notNull(),
    acceptedAt: integer("accepted_at").notNull(),
  },
  (table) => [
    uniqueIndex("sync_changes_owner_change_unique").on(table.ownerId, table.changeId),
    index("idx_sync_changes_pull").on(table.ownerId, table.serverSeq),
    index("idx_sync_changes_entity").on(
      table.ownerId,
      table.entityType,
      table.entityId,
      table.serverSeq,
    ),
  ],
);

export const syncAcks = sqliteTable(
  "sync_acks",
  {
    ownerId: text("owner_id").notNull(),
    deviceId: text("device_id").notNull(),
    lastServerSeq: integer("last_server_seq").notNull(),
    updatedAt: integer("updated_at").notNull(),
  },
  (table) => [primaryKey({ columns: [table.ownerId, table.deviceId] })],
);

export const objectManifests = sqliteTable(
  "object_manifests",
  {
    ownerId: text("owner_id").notNull(),
    objectId: text("object_id").notNull(),
    objectKind: text("object_kind").notNull(),
    logicalVersion: integer("logical_version").notNull(),
    latestArtifactId: text("latest_artifact_id").notNull(),
    ciphertextHash: text("ciphertext_hash").notNull(),
    size: integer("size").notNull(),
    keyVersion: integer("key_version").notNull(),
    updatedHlc: text("updated_hlc").notNull(),
    deletedAt: integer("deleted_at"),
    updatedAt: integer("updated_at").notNull(),
  },
  (table) => [
    primaryKey({ columns: [table.ownerId, table.objectId] }),
    uniqueIndex("object_manifests_owner_artifact_unique").on(
      table.ownerId,
      table.latestArtifactId,
    ),
    index("idx_object_manifests_owner_version").on(
      table.ownerId,
      table.updatedAt,
      table.objectId,
    ),
  ],
);

export const objectReplicas = sqliteTable(
  "object_replicas",
  {
    ownerId: text("owner_id").notNull(),
    artifactId: text("artifact_id").notNull(),
    providerKind: text("provider_kind").notNull(),
    providerAccountId: text("provider_account_id").notNull(),
    opaqueServerKey: text("opaque_server_key"),
    encryptedLocator: text("encrypted_locator"),
    providerRevision: text("provider_revision"),
    etag: text("etag"),
    ciphertextHash: text("ciphertext_hash").notNull(),
    size: integer("size").notNull(),
    status: text("status").notNull(),
    updatedAt: integer("updated_at").notNull(),
  },
  (table) => [
    primaryKey({
      columns: [table.ownerId, table.artifactId, table.providerKind, table.providerAccountId],
    }),
    uniqueIndex("object_replicas_opaque_key_unique").on(table.opaqueServerKey),
    index("idx_object_replicas_owner_status").on(table.ownerId, table.status, table.updatedAt),
  ],
);

export const objectChanges = sqliteTable(
  "object_changes",
  {
    serverSeq: integer("server_seq").primaryKey({ autoIncrement: true }),
    ownerId: text("owner_id").notNull(),
    objectId: text("object_id").notNull(),
    artifactId: text("artifact_id"),
    operation: text("operation").notNull(),
    logicalVersion: integer("logical_version").notNull(),
    changedAt: integer("changed_at").notNull(),
  },
  (table) => [index("idx_object_changes_pull").on(table.ownerId, table.serverSeq)],
);

export const deviceObjectStates = sqliteTable(
  "device_object_states",
  {
    ownerId: text("owner_id").notNull(),
    deviceId: text("device_id").notNull(),
    objectId: text("object_id").notNull(),
    observedLogicalVersion: integer("observed_logical_version").notNull(),
    installedArtifactId: text("installed_artifact_id"),
    localStatus: text("local_status").notNull(),
    verifiedCiphertextHash: text("verified_ciphertext_hash"),
    checkedAt: integer("checked_at").notNull(),
    errorCode: text("error_code"),
  },
  (table) => [
    primaryKey({ columns: [table.ownerId, table.deviceId, table.objectId] }),
    index("idx_device_object_states_device").on(table.ownerId, table.deviceId, table.checkedAt),
  ],
);

export const objectUploads = sqliteTable(
  "object_uploads",
  {
    id: text("id").primaryKey(),
    ownerId: text("owner_id").notNull(),
    deviceId: text("device_id").notNull(),
    objectId: text("object_id").notNull(),
    objectKind: text("object_kind").notNull(),
    logicalVersion: integer("logical_version").notNull(),
    artifactId: text("artifact_id").notNull(),
    opaqueServerKey: text("opaque_server_key").notNull(),
    r2UploadId: text("r2_upload_id").notNull(),
    ciphertextHash: text("ciphertext_hash").notNull(),
    size: integer("size").notNull(),
    keyVersion: integer("key_version").notNull(),
    updatedHlc: text("updated_hlc").notNull(),
    status: text("status").notNull(),
    createdAt: integer("created_at").notNull(),
    updatedAt: integer("updated_at").notNull(),
    expiresAt: integer("expires_at").notNull(),
  },
  (table) => [
    uniqueIndex("object_uploads_owner_artifact_unique").on(table.ownerId, table.artifactId),
    uniqueIndex("object_uploads_opaque_key_unique").on(table.opaqueServerKey),
    index("idx_object_uploads_expiry").on(table.status, table.expiresAt),
  ],
);

export const objectUploadParts = sqliteTable(
  "object_upload_parts",
  {
    uploadId: text("upload_id").notNull(),
    partNumber: integer("part_number").notNull(),
    etag: text("etag").notNull(),
    size: integer("size").notNull(),
    ciphertextHash: text("ciphertext_hash").notNull(),
    uploadedAt: integer("uploaded_at").notNull(),
  },
  (table) => [primaryKey({ columns: [table.uploadId, table.partNumber] })],
);
