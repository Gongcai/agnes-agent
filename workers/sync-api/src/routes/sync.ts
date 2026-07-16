import type { Context } from "hono";

import { ApiError } from "../errors";
import {
  ackRequestSchema,
  DEFAULT_PAGE_LIMIT,
  entityIdSchema,
  entityTypeSchema,
  MAX_PAGE_LIMIT,
  MAX_RESPONSE_BYTES,
  PROTOCOL_VERSION,
  pushRequestSchema,
  type SyncChange,
} from "../protocol";
import { parsePageNumber, readJsonBody } from "../request";
import type { AppEnv, AuthIdentity } from "../types";

interface ExistingChangeRow {
  change_id: string;
  device_id: string;
  entity_type: string;
  entity_id: string;
  operation: string;
  base_revision: number | null;
  resulting_revision: number;
  hlc: string;
  payload_schema_version: number;
  payload_encoding: string;
  payload: string | null;
  payload_hash: string;
  key_version: number | null;
  created_at: number;
  server_seq: number;
}

interface EntityStateRow {
  entity_type: string;
  entity_id: string;
  revision: number;
  hlc: string;
  payload_hash: string;
  deleted: number;
}

interface ChangeRow {
  server_seq: number;
  change_id: string;
  device_id: string;
  entity_type: string;
  entity_id: string;
  operation: "upsert" | "delete";
  base_revision: number | null;
  resulting_revision: number;
  hlc: string;
  payload_schema_version: number;
  payload_encoding: string;
  payload: string | null;
  payload_hash: string;
  key_version: number | null;
  created_at: number;
  accepted_at: number;
}

interface EntityRow {
  entity_type: string;
  entity_id: string;
  revision: number;
  hlc: string;
  deleted: number;
  payload_schema_version: number;
  payload_encoding: string;
  payload: string | null;
  payload_hash: string;
  key_version: number | null;
  changed_by_device_id: string;
  latest_server_seq: number;
  updated_at: number;
}

interface BootstrapCursor {
  version: 1;
  snapshotCursor: number;
  entityType: string;
  entityId: string;
}

const APPEND_ONLY_ENTITY_TYPES = new Set(["message"]);

function stateKey(entityType: string, entityId: string): string {
  return `${entityType}\u0000${entityId}`;
}

function encodePayload(payload: unknown): string | null {
  return payload === null ? null : JSON.stringify(payload);
}

function decodePayload(payload: string | null): unknown {
  if (payload === null) {
    return null;
  }
  try {
    return JSON.parse(payload) as unknown;
  } catch {
    throw new ApiError(503, "SYNC_TEMPORARILY_UNAVAILABLE", "Stored sync payload is invalid");
  }
}

function validateDevice(change: SyncChange, identity: AuthIdentity): void {
  if (change.deviceId !== identity.deviceId) {
    throw new ApiError(401, "UNAUTHENTICATED", "Change deviceId does not match the credential");
  }
}

function matchesExisting(change: SyncChange, existing: ExistingChangeRow): boolean {
  return (
    change.deviceId === existing.device_id &&
    change.entityType === existing.entity_type &&
    change.entityId === existing.entity_id &&
    change.operation === existing.operation &&
    change.baseRevision === existing.base_revision &&
    change.hlc === existing.hlc &&
    change.payloadSchemaVersion === existing.payload_schema_version &&
    change.payloadEncoding === existing.payload_encoding &&
    encodePayload(change.payload) === existing.payload &&
    change.payloadHash === existing.payload_hash &&
    change.keyVersion === existing.key_version &&
    change.createdAt === existing.created_at
  );
}

async function selectExistingChanges(
  database: D1Database,
  ownerId: string,
  changes: SyncChange[],
): Promise<Map<string, ExistingChangeRow>> {
  const placeholders = changes.map(() => "?").join(", ");
  const result = await database
    .prepare(
      `SELECT change_id, device_id, entity_type, entity_id, operation, base_revision,
              resulting_revision, hlc, payload_schema_version, payload_encoding, payload,
              payload_hash, key_version, created_at, server_seq
       FROM sync_changes
       WHERE owner_id = ? AND change_id IN (${placeholders})`,
    )
    .bind(ownerId, ...changes.map((change) => change.changeId))
    .all<ExistingChangeRow>();
  return new Map(result.results.map((row) => [row.change_id, row]));
}

async function selectEntityStates(
  database: D1Database,
  ownerId: string,
  changes: SyncChange[],
): Promise<Map<string, EntityStateRow>> {
  if (changes.length === 0) {
    return new Map();
  }
  const conditions = changes.map(() => "(entity_type = ? AND entity_id = ?)").join(" OR ");
  const bindings = changes.flatMap((change) => [change.entityType, change.entityId]);
  const result = await database
    .prepare(
      `SELECT entity_type, entity_id, revision, hlc, payload_hash, deleted
       FROM sync_entities
       WHERE owner_id = ? AND (${conditions})`,
    )
    .bind(ownerId, ...bindings)
    .all<EntityStateRow>();
  return new Map(result.results.map((row) => [stateKey(row.entity_type, row.entity_id), row]));
}

function canApply(change: SyncChange, state: EntityStateRow | undefined): boolean {
  if (!state) {
    return change.baseRevision === null;
  }
  if (change.baseRevision !== state.revision) {
    return false;
  }
  return !(APPEND_ONLY_ENTITY_TYPES.has(change.entityType) && change.operation === "upsert");
}

function makeEntityStatement(
  database: D1Database,
  ownerId: string,
  change: SyncChange,
  resultingRevision: number,
  acceptedAt: number,
): D1PreparedStatement {
  const deleted = change.operation === "delete" ? 1 : 0;
  const payload = encodePayload(change.payload);
  return database
    .prepare(
      `INSERT INTO sync_entities (
         owner_id, entity_type, entity_id, revision, hlc, deleted,
         payload_schema_version, payload_encoding, payload, payload_hash, key_version,
         changed_by_device_id, latest_server_seq, latest_change_id, updated_at
       )
       SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?
       WHERE ? IS NULL
          OR EXISTS (
            SELECT 1 FROM sync_entities current
            WHERE current.owner_id = ?
              AND current.entity_type = ?
              AND current.entity_id = ?
              AND current.revision = ?
          )
       ON CONFLICT(owner_id, entity_type, entity_id) DO UPDATE SET
         revision = sync_entities.revision + 1,
         hlc = excluded.hlc,
         deleted = excluded.deleted,
         payload_schema_version = excluded.payload_schema_version,
         payload_encoding = excluded.payload_encoding,
         payload = excluded.payload,
         payload_hash = excluded.payload_hash,
         key_version = excluded.key_version,
         changed_by_device_id = excluded.changed_by_device_id,
         latest_change_id = excluded.latest_change_id,
         updated_at = excluded.updated_at
       WHERE ? IS NOT NULL
         AND sync_entities.revision = ?
         AND (? != 'message' OR ? = 'delete')`,
    )
    .bind(
      ownerId,
      change.entityType,
      change.entityId,
      resultingRevision,
      change.hlc,
      deleted,
      change.payloadSchemaVersion,
      change.payloadEncoding,
      payload,
      change.payloadHash,
      change.keyVersion,
      change.deviceId,
      change.changeId,
      acceptedAt,
      change.baseRevision,
      ownerId,
      change.entityType,
      change.entityId,
      change.baseRevision,
      change.baseRevision,
      change.baseRevision,
      change.entityType,
      change.operation,
    );
}

function makeChangeStatement(
  database: D1Database,
  ownerId: string,
  change: SyncChange,
  acceptedAt: number,
): D1PreparedStatement {
  return database
    .prepare(
      `INSERT INTO sync_changes (
         owner_id, change_id, device_id, entity_type, entity_id, operation,
         base_revision, resulting_revision, hlc, payload_schema_version,
         payload_encoding, payload, payload_hash, key_version, created_at, accepted_at
       )
       SELECT ?, ?, ?, ?, ?, ?, ?, revision, ?, ?, ?, ?, ?, ?, ?, ?
       FROM sync_entities
       WHERE owner_id = ? AND entity_type = ? AND entity_id = ? AND latest_change_id = ?
       RETURNING server_seq, resulting_revision`,
    )
    .bind(
      ownerId,
      change.changeId,
      change.deviceId,
      change.entityType,
      change.entityId,
      change.operation,
      change.baseRevision,
      change.hlc,
      change.payloadSchemaVersion,
      change.payloadEncoding,
      encodePayload(change.payload),
      change.payloadHash,
      change.keyVersion,
      change.createdAt,
      acceptedAt,
      ownerId,
      change.entityType,
      change.entityId,
      change.changeId,
    );
}

export async function push(context: Context<AppEnv>): Promise<Response> {
  const rawBody = await readJsonBody(context);
  const parsed = pushRequestSchema.safeParse(rawBody);
  if (!parsed.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Push request is invalid", parsed.error.issues);
  }

  const identity = context.get("auth");
  if (parsed.data.deviceId !== identity.deviceId) {
    throw new ApiError(401, "UNAUTHENTICATED", "Request deviceId does not match the credential");
  }
  const seenChangeIds = new Set<string>();
  for (const change of parsed.data.changes) {
    validateDevice(change, identity);
    if (seenChangeIds.has(change.changeId)) {
      throw new ApiError(400, "INVALID_REQUEST", "A push batch cannot repeat a changeId");
    }
    seenChangeIds.add(change.changeId);
  }

  const database = context.env.SYNC_DB;
  const existing = await selectExistingChanges(database, identity.ownerId, parsed.data.changes);
  const freshChanges = parsed.data.changes.filter((change) => !existing.has(change.changeId));
  for (const change of parsed.data.changes) {
    const previous = existing.get(change.changeId);
    if (previous && !matchesExisting(change, previous)) {
      throw new ApiError(400, "INVALID_REQUEST", "changeId was already used for different content", {
        changeId: change.changeId,
      });
    }
  }

  const states = await selectEntityStates(database, identity.ownerId, freshChanges);
  const accepted = parsed.data.changes
    .filter((change) => existing.has(change.changeId))
    .map((change) => {
      const previous = existing.get(change.changeId)!;
      return {
        changeId: change.changeId,
        serverSeq: previous.server_seq,
        revision: previous.resulting_revision,
        idempotent: true,
      };
    });
  const conflicts: Array<{
    changeId: string;
    entityType: string;
    entityId: string;
    currentRevision: number | null;
    reason: "REVISION_CONFLICT";
  }> = [];
  const applicable: Array<{ change: SyncChange; resultingRevision: number }> = [];

  for (const change of freshChanges) {
    const key = stateKey(change.entityType, change.entityId);
    const state = states.get(key);
    if (!canApply(change, state)) {
      conflicts.push({
        changeId: change.changeId,
        entityType: change.entityType,
        entityId: change.entityId,
        currentRevision: state?.revision ?? null,
        reason: "REVISION_CONFLICT",
      });
      continue;
    }
    const resultingRevision = (state?.revision ?? 0) + 1;
    applicable.push({ change, resultingRevision });
    states.set(key, {
      entity_type: change.entityType,
      entity_id: change.entityId,
      revision: resultingRevision,
      hlc: change.hlc,
      payload_hash: change.payloadHash,
      deleted: change.operation === "delete" ? 1 : 0,
    });
  }

  if (applicable.length > 0) {
    const acceptedAt = Date.now();
    const statements = applicable.flatMap(({ change, resultingRevision }) => [
      makeEntityStatement(database, identity.ownerId, change, resultingRevision, acceptedAt),
      makeChangeStatement(database, identity.ownerId, change, acceptedAt),
    ]);
    const results = await database.batch<{ server_seq: number; resulting_revision: number }>(statements);
    for (let index = 0; index < applicable.length; index += 1) {
      const item = applicable[index]!;
      const result = results[index * 2 + 1];
      const inserted = result?.results[0];
      if (!inserted) {
        conflicts.push({
          changeId: item.change.changeId,
          entityType: item.change.entityType,
          entityId: item.change.entityId,
          currentRevision: null,
          reason: "REVISION_CONFLICT",
        });
        continue;
      }
      accepted.push({
        changeId: item.change.changeId,
        serverSeq: inserted.server_seq,
        revision: inserted.resulting_revision,
        idempotent: false,
      });
    }
  }

  const acceptedOrder = new Map(parsed.data.changes.map((change, index) => [change.changeId, index]));
  accepted.sort((left, right) => acceptedOrder.get(left.changeId)! - acceptedOrder.get(right.changeId)!);
  conflicts.sort((left, right) => acceptedOrder.get(left.changeId)! - acceptedOrder.get(right.changeId)!);
  return context.json({ accepted, conflicts, serverTime: Date.now() });
}

function serializeChange(row: ChangeRow) {
  return {
    protocolVersion: PROTOCOL_VERSION,
    serverSeq: row.server_seq,
    changeId: row.change_id,
    deviceId: row.device_id,
    entityType: row.entity_type,
    entityId: row.entity_id,
    operation: row.operation,
    baseRevision: row.base_revision,
    resultingRevision: row.resulting_revision,
    hlc: row.hlc,
    payloadSchemaVersion: row.payload_schema_version,
    payloadEncoding: row.payload_encoding,
    payload: decodePayload(row.payload),
    payloadHash: row.payload_hash,
    keyVersion: row.key_version,
    createdAt: row.created_at,
    acceptedAt: row.accepted_at,
  };
}

export async function pull(context: Context<AppEnv>): Promise<Response> {
  const after = parsePageNumber(context.req.query("after"), "after", 0, Number.MAX_SAFE_INTEGER);
  const limit = parsePageNumber(
    context.req.query("limit"),
    "limit",
    DEFAULT_PAGE_LIMIT,
    MAX_PAGE_LIMIT,
  );
  if (limit === 0) {
    throw new ApiError(400, "INVALID_REQUEST", "limit must be greater than zero");
  }

  const identity = context.get("auth");
  const result = await context.env.SYNC_DB.prepare(
    `SELECT server_seq, change_id, device_id, entity_type, entity_id, operation,
            base_revision, resulting_revision, hlc, payload_schema_version,
            payload_encoding, payload, payload_hash, key_version, created_at, accepted_at
     FROM sync_changes
     WHERE owner_id = ? AND server_seq > ?
     ORDER BY server_seq
     LIMIT ?`,
  )
    .bind(identity.ownerId, after, limit + 1)
    .all<ChangeRow>();

  const changes: ReturnType<typeof serializeChange>[] = [];
  let responseBytes = 0;
  let truncatedBySize = false;
  for (const row of result.results.slice(0, limit)) {
    const change = serializeChange(row);
    const encodedBytes = new TextEncoder().encode(JSON.stringify(change)).byteLength;
    if (changes.length > 0 && responseBytes + encodedBytes > MAX_RESPONSE_BYTES) {
      truncatedBySize = true;
      break;
    }
    changes.push(change);
    responseBytes += encodedBytes;
  }
  const nextCursor = changes.at(-1)?.serverSeq ?? after;
  return context.json({
    changes,
    nextCursor,
    hasMore: truncatedBySize || result.results.length > changes.length,
    serverTime: Date.now(),
  });
}

function encodeBootstrapCursor(cursor: BootstrapCursor): string {
  const bytes = new TextEncoder().encode(JSON.stringify(cursor));
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function decodeBootstrapCursor(value: string): BootstrapCursor {
  try {
    const base64 = value.replaceAll("-", "+").replaceAll("_", "/");
    const padded = base64.padEnd(Math.ceil(base64.length / 4) * 4, "=");
    const binary = atob(padded);
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    const parsed = JSON.parse(new TextDecoder().decode(bytes)) as Partial<BootstrapCursor>;
    if (
      parsed.version !== 1 ||
      typeof parsed.snapshotCursor !== "number" ||
      !Number.isSafeInteger(parsed.snapshotCursor) ||
      parsed.snapshotCursor < 0 ||
      typeof parsed.entityType !== "string" ||
      typeof parsed.entityId !== "string" ||
      !(
        (parsed.entityType === "" && parsed.entityId === "") ||
        (entityTypeSchema.safeParse(parsed.entityType).success &&
          entityIdSchema.safeParse(parsed.entityId).success)
      )
    ) {
      throw new Error("invalid cursor");
    }
    return parsed as BootstrapCursor;
  } catch {
    throw new ApiError(400, "INVALID_REQUEST", "Bootstrap cursor is invalid");
  }
}

function serializeEntity(row: EntityRow) {
  return {
    entityType: row.entity_type,
    entityId: row.entity_id,
    revision: row.revision,
    hlc: row.hlc,
    deleted: row.deleted === 1,
    payloadSchemaVersion: row.payload_schema_version,
    payloadEncoding: row.payload_encoding,
    payload: decodePayload(row.payload),
    payloadHash: row.payload_hash,
    keyVersion: row.key_version,
    changedByDeviceId: row.changed_by_device_id,
    latestServerSeq: row.latest_server_seq,
    updatedAt: row.updated_at,
  };
}

export async function bootstrap(context: Context<AppEnv>): Promise<Response> {
  const limit = parsePageNumber(
    context.req.query("limit"),
    "limit",
    DEFAULT_PAGE_LIMIT,
    MAX_PAGE_LIMIT,
  );
  if (limit === 0) {
    throw new ApiError(400, "INVALID_REQUEST", "limit must be greater than zero");
  }
  const identity = context.get("auth");
  const cursorValue = context.req.query("cursor");
  let cursor: BootstrapCursor;
  if (cursorValue) {
    cursor = decodeBootstrapCursor(cursorValue);
  } else {
    const highWater = await context.env.SYNC_DB.prepare(
      "SELECT COALESCE(MAX(server_seq), 0) AS snapshot_cursor FROM sync_changes WHERE owner_id = ?",
    )
      .bind(identity.ownerId)
      .first<{ snapshot_cursor: number }>();
    cursor = {
      version: 1,
      snapshotCursor: highWater?.snapshot_cursor ?? 0,
      entityType: "",
      entityId: "",
    };
  }

  const result = await context.env.SYNC_DB.prepare(
    `SELECT entity_type, entity_id, revision, hlc, deleted, payload_schema_version,
            payload_encoding, payload, payload_hash, key_version, changed_by_device_id,
            latest_server_seq, updated_at
     FROM sync_entities
     WHERE owner_id = ?
       AND latest_server_seq <= ?
       AND (entity_type > ? OR (entity_type = ? AND entity_id > ?))
     ORDER BY entity_type, entity_id
     LIMIT ?`,
  )
    .bind(
      identity.ownerId,
      cursor.snapshotCursor,
      cursor.entityType,
      cursor.entityType,
      cursor.entityId,
      limit + 1,
    )
    .all<EntityRow>();

  const entities: ReturnType<typeof serializeEntity>[] = [];
  let responseBytes = 0;
  let truncatedBySize = false;
  for (const row of result.results.slice(0, limit)) {
    const entity = serializeEntity(row);
    const encodedBytes = new TextEncoder().encode(JSON.stringify(entity)).byteLength;
    if (entities.length > 0 && responseBytes + encodedBytes > MAX_RESPONSE_BYTES) {
      truncatedBySize = true;
      break;
    }
    entities.push(entity);
    responseBytes += encodedBytes;
  }
  const lastEntity = entities.at(-1);
  const hasMore = truncatedBySize || result.results.length > entities.length;
  const nextCursor =
    hasMore && lastEntity
      ? encodeBootstrapCursor({
          version: 1,
          snapshotCursor: cursor.snapshotCursor,
          entityType: lastEntity.entityType,
          entityId: lastEntity.entityId,
        })
      : null;
  return context.json({
    entities,
    snapshotCursor: cursor.snapshotCursor,
    nextCursor,
    hasMore,
    serverTime: Date.now(),
  });
}

export async function ack(context: Context<AppEnv>): Promise<Response> {
  const rawBody = await readJsonBody(context);
  const parsed = ackRequestSchema.safeParse(rawBody);
  if (!parsed.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Ack request is invalid", parsed.error.issues);
  }
  const identity = context.get("auth");
  if (parsed.data.deviceId !== identity.deviceId) {
    throw new ApiError(401, "UNAUTHENTICATED", "Request deviceId does not match the credential");
  }

  const highWater = await context.env.SYNC_DB.prepare(
    "SELECT COALESCE(MAX(server_seq), 0) AS high_water FROM sync_changes WHERE owner_id = ?",
  )
    .bind(identity.ownerId)
    .first<{ high_water: number }>();
  if (parsed.data.cursor > (highWater?.high_water ?? 0)) {
    throw new ApiError(400, "INVALID_REQUEST", "Ack cursor is ahead of the owner change stream");
  }

  const storedAck = await context.env.SYNC_DB.prepare(
    `INSERT INTO sync_acks (owner_id, device_id, last_server_seq, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(owner_id, device_id) DO UPDATE SET
       last_server_seq = MAX(sync_acks.last_server_seq, excluded.last_server_seq),
       updated_at = excluded.updated_at
     RETURNING last_server_seq`,
  )
    .bind(identity.ownerId, identity.deviceId, parsed.data.cursor, Date.now())
    .first<{ last_server_seq: number }>();
  return context.json({
    acknowledgedCursor: storedAck?.last_server_seq ?? parsed.data.cursor,
    serverTime: Date.now(),
  });
}
