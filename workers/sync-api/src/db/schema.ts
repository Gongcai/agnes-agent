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
  (table) => [index("idx_devices_owner").on(table.ownerId, table.revokedAt)],
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
