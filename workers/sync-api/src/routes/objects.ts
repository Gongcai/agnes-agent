import type { Context } from "hono";
import { z } from "zod";

import { ApiError } from "../errors";
import { parsePageNumber, readJsonBody } from "../request";
import type { AppEnv } from "../types";

const MAX_ARTIFACT_BYTES = 5_000_000_000_000;
const MAX_PART_BYTES = 64 * 1024 * 1024;
const MIN_MULTIPART_PART_BYTES = 5 * 1024 * 1024;
const UPLOAD_TTL_MS = 60 * 60 * 1000;
const DEFAULT_QUOTA_BYTES = 5_000_000_000_000;
const DEFAULT_ORPHAN_GRACE_MS = 7 * 24 * 60 * 60 * 1000;
const MIN_ORPHAN_GRACE_MS = 24 * 60 * 60 * 1000;
const MAX_ORPHAN_GRACE_MS = 90 * 24 * 60 * 60 * 1000;
const DEFAULT_ORPHAN_GC_BATCH_SIZE = 50;
const MAX_ORPHAN_GC_BATCH_SIZE = 100;
const GC_CLAIM_RETRY_MS = 60 * 60 * 1000;
const opaqueIdSchema = z.string().min(1).max(128).regex(/^[A-Za-z0-9][A-Za-z0-9._:-]*$/);
const hashSchema = z.string().regex(/^[a-f0-9]{64}$/);
const uploadRequestSchema = z
  .object({
    protocolVersion: z.literal(1),
    objectId: opaqueIdSchema,
    objectKind: z.string().min(1).max(64).regex(/^[a-z][a-z0-9._-]*$/),
    logicalVersion: z.int().positive(),
    artifactId: opaqueIdSchema,
    ciphertextHash: hashSchema,
    size: z.number().int().positive().max(MAX_ARTIFACT_BYTES),
    keyVersion: z.int().positive(),
    updatedHlc: z.string().min(1).max(160),
  })
  .strict();
const objectStateSchema = z
  .object({
    protocolVersion: z.literal(1),
    deviceId: z.uuid(),
    objectId: opaqueIdSchema,
    observedLogicalVersion: z.int().nonnegative(),
    installedArtifactId: opaqueIdSchema.nullable(),
    localStatus: z.enum(["missing", "downloading", "verifying", "installed", "failed", "incompatible"]),
    verifiedCiphertextHash: hashSchema.nullable(),
    errorCode: z.string().min(1).max(80).nullable(),
  })
  .strict();

interface UploadRow {
  id: string;
  owner_id: string;
  device_id: string;
  object_id: string;
  object_kind: string;
  logical_version: number;
  artifact_id: string;
  opaque_server_key: string;
  r2_upload_id: string;
  ciphertext_hash: string;
  size: number;
  key_version: number;
  updated_hlc: string;
  status: "pending" | "completing" | "completed" | "aborted" | "failed";
  expires_at: number;
}

interface PartRow {
  part_number: number;
  etag: string;
  size: number;
  ciphertext_hash: string;
}

interface ManifestRow {
  owner_id: string;
  object_id: string;
  object_kind: string;
  logical_version: number;
  latest_artifact_id: string;
  ciphertext_hash: string;
  size: number;
  key_version: number;
  updated_hlc: string;
  deleted_at: number | null;
  updated_at: number;
}

interface ReplicaRow {
  artifact_id: string;
  provider_kind: string;
  provider_account_id: string;
  opaque_server_key: string | null;
  encrypted_locator: string | null;
  provider_revision: string | null;
  etag: string | null;
  ciphertext_hash: string;
  size: number;
  status: string;
  updated_at: number;
}

interface DeviceStateRow {
  device_id: string;
  observed_logical_version: number;
  installed_artifact_id: string | null;
  local_status: string;
  verified_ciphertext_hash: string | null;
  checked_at: number;
  error_code: string | null;
}

interface OrphanReplicaRow {
  owner_id: string;
  artifact_id: string;
  opaque_server_key: string;
  orphaned_at: number;
}

export interface ObjectGcReport {
  candidates: number;
  claimed: number;
  deleted: number;
  failed: number;
  skipped: number;
}

function ownerNotFound(): ApiError {
  return new ApiError(404, "OBJECT_NOT_FOUND", "The requested object does not exist");
}

function quotaBytes(value: string | undefined): number {
  if (!value || !/^\d+$/.test(value)) {
    return DEFAULT_QUOTA_BYTES;
  }
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : DEFAULT_QUOTA_BYTES;
}

function boundedInteger(
  value: string | undefined,
  fallback: number,
  minimum: number,
  maximum: number,
): number {
  if (!value || !/^\d+$/.test(value)) {
    return fallback;
  }
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= minimum && parsed <= maximum
    ? parsed
    : fallback;
}

function routeParam(context: Context<AppEnv>, name: string): string {
  const value = context.req.param(name);
  if (!value) {
    throw new ApiError(400, "INVALID_REQUEST", `${name} is required`);
  }
  return value;
}

async function getUpload(context: Context<AppEnv>, uploadId: string): Promise<UploadRow> {
  const row = await context.env.SYNC_DB.prepare(
    `SELECT id, owner_id, device_id, object_id, object_kind, logical_version, artifact_id,
            opaque_server_key, r2_upload_id, ciphertext_hash, size, key_version, updated_hlc,
            status, expires_at
     FROM object_uploads WHERE owner_id = ? AND id = ?`,
  )
    .bind(context.get("auth").ownerId, uploadId)
    .first<UploadRow>();
  if (!row) {
    throw ownerNotFound();
  }
  if (row.expires_at <= Date.now() && (row.status === "pending" || row.status === "completing")) {
    try {
      await context.env.ARTIFACT_OBJECTS.resumeMultipartUpload(
        row.opaque_server_key,
        row.r2_upload_id,
      ).abort();
    } catch {
      // The session may already have been discarded by R2.
    }
    await context.env.SYNC_DB.prepare(
      "UPDATE object_uploads SET status = 'aborted', updated_at = ? WHERE id = ? AND owner_id = ?",
    )
      .bind(Date.now(), row.id, row.owner_id)
      .run();
    throw new ApiError(410, "UPLOAD_EXPIRED", "The artifact upload session has expired");
  }
  return row;
}

export async function createObjectUpload(context: Context<AppEnv>): Promise<Response> {
  const parsed = uploadRequestSchema.safeParse(await readJsonBody(context));
  if (!parsed.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Artifact upload metadata is invalid", parsed.error.issues);
  }
  const identity = context.get("auth");
  const database = context.env.SYNC_DB;
  const artifactOwner = await database
    .prepare(
      "SELECT object_id, ciphertext_hash, size, key_version FROM object_manifests WHERE owner_id = ? AND latest_artifact_id = ?",
    )
    .bind(identity.ownerId, parsed.data.artifactId)
    .first<{ object_id: string; ciphertext_hash: string; size: number; key_version: number }>();
  if (artifactOwner) {
    if (
      artifactOwner.object_id !== parsed.data.objectId ||
      artifactOwner.ciphertext_hash !== parsed.data.ciphertextHash ||
      artifactOwner.size !== parsed.data.size ||
      artifactOwner.key_version !== parsed.data.keyVersion
    ) {
      throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact ID is already bound to another object");
    }
  }
  const existingReplica = await database
    .prepare(
      `SELECT ciphertext_hash, size
       FROM object_replicas WHERE owner_id = ? AND artifact_id = ? AND status = 'ready' LIMIT 1`,
    )
    .bind(identity.ownerId, parsed.data.artifactId)
    .first<{ ciphertext_hash: string; size: number }>();
  if (
    existingReplica &&
    (existingReplica.ciphertext_hash !== parsed.data.ciphertextHash ||
      existingReplica.size !== parsed.data.size)
  ) {
    throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact ID is already used for different ciphertext");
  }
  const existingManifest = await database
    .prepare("SELECT * FROM object_manifests WHERE owner_id = ? AND object_id = ?")
    .bind(identity.ownerId, parsed.data.objectId)
    .first<ManifestRow>();
  if (existingManifest) {
    if (parsed.data.logicalVersion < existingManifest.logical_version) {
      throw new ApiError(409, "UPLOAD_CONFLICT", "The object logical version is stale");
    }
    if (
      parsed.data.logicalVersion === existingManifest.logical_version &&
      parsed.data.artifactId !== existingManifest.latest_artifact_id
    ) {
      throw new ApiError(409, "UPLOAD_CONFLICT", "The object logical version already has another artifact");
    }
    if (
      parsed.data.logicalVersion === existingManifest.logical_version &&
      parsed.data.artifactId === existingManifest.latest_artifact_id
    ) {
      if (
        parsed.data.objectKind !== existingManifest.object_kind ||
        parsed.data.ciphertextHash !== existingManifest.ciphertext_hash ||
        parsed.data.size !== existingManifest.size ||
        parsed.data.keyVersion !== existingManifest.key_version ||
        parsed.data.updatedHlc !== existingManifest.updated_hlc
      ) {
        throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact ID was reused for different content");
      }
      const replica = await database
        .prepare(
          `SELECT artifact_id, provider_kind, provider_account_id, opaque_server_key, encrypted_locator,
                  provider_revision, etag, ciphertext_hash, size, status, updated_at
           FROM object_replicas
           WHERE owner_id = ? AND artifact_id = ? AND provider_kind = 'r2' AND status = 'ready'`,
        )
        .bind(identity.ownerId, parsed.data.artifactId)
        .first<ReplicaRow>();
      if (replica) {
        return context.json({
          status: "ready",
          uploadSessionId: null,
          artifactId: replica.artifact_id,
          size: replica.size,
          ciphertextHash: replica.ciphertext_hash,
        });
      }
    }
  }

  const pending = await database
    .prepare(
      `SELECT id, owner_id, device_id, object_id, object_kind, logical_version, artifact_id,
              opaque_server_key, r2_upload_id, ciphertext_hash, size, key_version, updated_hlc,
              status, expires_at
       FROM object_uploads
       WHERE owner_id = ? AND artifact_id = ? AND status IN ('pending','completing')
       ORDER BY created_at DESC LIMIT 1`,
    )
    .bind(identity.ownerId, parsed.data.artifactId)
    .first<UploadRow>();
  if (pending && pending.expires_at > Date.now()) {
    if (
      pending.object_id !== parsed.data.objectId ||
      pending.object_kind !== parsed.data.objectKind ||
      pending.logical_version !== parsed.data.logicalVersion ||
      pending.ciphertext_hash !== parsed.data.ciphertextHash ||
      pending.size !== parsed.data.size ||
      pending.key_version !== parsed.data.keyVersion
    ) {
      throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact ID is already used by another upload");
    }
    return context.json({
      status: "pending",
      uploadSessionId: pending.id,
      artifactId: pending.artifact_id,
      size: pending.size,
      ciphertextHash: pending.ciphertext_hash,
    });
  }

  const usage = await database
    .prepare(
      `SELECT COALESCE((SELECT SUM(size) FROM object_replicas WHERE owner_id = ? AND status = 'ready'), 0)
              + COALESCE((SELECT SUM(size) FROM object_uploads WHERE owner_id = ? AND status IN ('pending','completing')), 0)
              AS used_bytes`,
    )
    .bind(identity.ownerId, identity.ownerId)
    .first<{ used_bytes: number }>();
  const quota = quotaBytes(context.env.R2_OWNER_QUOTA_BYTES);
  if (Number(usage?.used_bytes ?? 0) + parsed.data.size > quota) {
    throw new ApiError(413, "QUOTA_EXCEEDED", "The artifact quota would be exceeded", {
      quotaBytes: quota,
    });
  }

  const opaqueServerKey = `o/${crypto.randomUUID()}`;
  const multipart = await context.env.ARTIFACT_OBJECTS.createMultipartUpload(opaqueServerKey, {
    httpMetadata: { contentType: "application/octet-stream" },
    customMetadata: {
      artifactId: parsed.data.artifactId,
      ciphertextHash: parsed.data.ciphertextHash,
    },
  });
  const uploadId = crypto.randomUUID();
  const now = Date.now();
  try {
    await database
      .prepare(
        `INSERT INTO object_uploads
         (id, owner_id, device_id, object_id, object_kind, logical_version, artifact_id,
          opaque_server_key, r2_upload_id, ciphertext_hash, size, key_version, updated_hlc,
          status, created_at, updated_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)`,
      )
      .bind(
        uploadId,
        identity.ownerId,
        identity.deviceId,
        parsed.data.objectId,
        parsed.data.objectKind,
        parsed.data.logicalVersion,
        parsed.data.artifactId,
        opaqueServerKey,
        multipart.uploadId,
        parsed.data.ciphertextHash,
        parsed.data.size,
        parsed.data.keyVersion,
        parsed.data.updatedHlc,
        now,
        now,
        now + UPLOAD_TTL_MS,
      )
      .run();
  } catch (error) {
    await multipart.abort();
    throw error;
  }
  return context.json(
    {
      status: "pending",
      uploadSessionId: uploadId,
      artifactId: parsed.data.artifactId,
      size: parsed.data.size,
      ciphertextHash: parsed.data.ciphertextHash,
    },
    201,
  );
}

export async function uploadObjectPart(context: Context<AppEnv>): Promise<Response> {
  const upload = await getUpload(context, routeParam(context, "uploadId"));
  if (upload.status !== "pending") {
    throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact upload session is no longer writable");
  }
  const partNumber = Number(context.req.param("partNumber"));
  if (!Number.isInteger(partNumber) || partNumber < 1 || partNumber > 10_000) {
    throw new ApiError(400, "INVALID_REQUEST", "partNumber is outside the supported range");
  }
  const declaredLength = Number(context.req.header("Content-Length") ?? "0");
  if (!Number.isSafeInteger(declaredLength) || declaredLength <= 0 || declaredLength > MAX_PART_BYTES) {
    throw new ApiError(413, "PAYLOAD_TOO_LARGE", "An artifact part must be between 1 and 64 MiB");
  }
  const bytes = new Uint8Array(await context.req.arrayBuffer());
  if (bytes.byteLength !== declaredLength) {
    throw new ApiError(400, "INVALID_REQUEST", "Content-Length does not match the artifact part");
  }
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  const checksum = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
  if (context.req.header("X-Agnes-Part-Sha256") !== checksum) {
    throw new ApiError(422, "CHECKSUM_MISMATCH", "Artifact part checksum does not match");
  }
  const multipart = context.env.ARTIFACT_OBJECTS.resumeMultipartUpload(
    upload.opaque_server_key,
    upload.r2_upload_id,
  );
  const uploaded = await multipart.uploadPart(partNumber, bytes);
  await context.env.SYNC_DB.prepare(
    `INSERT INTO object_upload_parts (upload_id, part_number, etag, size, ciphertext_hash, uploaded_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT(upload_id, part_number) DO UPDATE SET etag = excluded.etag, size = excluded.size,
       ciphertext_hash = excluded.ciphertext_hash, uploaded_at = excluded.uploaded_at`,
  )
    .bind(upload.id, partNumber, uploaded.etag, bytes.byteLength, checksum, Date.now())
    .run();
  return context.json({ partNumber, etag: uploaded.etag, size: bytes.byteLength, checksum });
}

export async function completeObjectUpload(context: Context<AppEnv>): Promise<Response> {
  const upload = await getUpload(context, routeParam(context, "uploadId"));
  if (upload.status === "completed") {
    return context.json({ status: "completed", artifactId: upload.artifact_id });
  }
  if (upload.status !== "pending") {
    throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact upload session cannot be completed");
  }
  const parts = await context.env.SYNC_DB.prepare(
    "SELECT part_number, etag, size, ciphertext_hash FROM object_upload_parts WHERE upload_id = ? ORDER BY part_number",
  )
    .bind(upload.id)
    .all<PartRow>();
  if (parts.results.length === 0 || parts.results.some((part, index) => part.part_number !== index + 1)) {
    throw new ApiError(400, "INVALID_REQUEST", "Artifact upload parts must be contiguous from part 1");
  }
  if (parts.results.slice(0, -1).some((part) => part.size < MIN_MULTIPART_PART_BYTES)) {
    throw new ApiError(400, "INVALID_REQUEST", "Non-final artifact parts must be at least 5 MiB");
  }
  const totalSize = parts.results.reduce((total, part) => total + part.size, 0);
  if (totalSize !== upload.size) {
    throw new ApiError(400, "INVALID_REQUEST", "Artifact upload parts do not match the declared size");
  }
  const claim = await context.env.SYNC_DB.prepare(
    "UPDATE object_uploads SET status = 'completing', updated_at = ? WHERE id = ? AND status = 'pending'",
  )
    .bind(Date.now(), upload.id)
    .run();
  if (claim.meta.changes !== 1) {
    const current = await getUpload(context, upload.id);
    if (current.status === "completed") {
      return context.json({ status: "completed", artifactId: current.artifact_id });
    }
    throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact upload is already being completed");
  }
  const multipart = context.env.ARTIFACT_OBJECTS.resumeMultipartUpload(
    upload.opaque_server_key,
    upload.r2_upload_id,
  );
  let object: R2Object;
  try {
    object = await multipart.complete(
      parts.results.map((part) => ({ partNumber: part.part_number, etag: part.etag })),
    );
  } catch (error) {
    await context.env.SYNC_DB.prepare(
      "UPDATE object_uploads SET status = 'failed', updated_at = ? WHERE id = ?",
    )
      .bind(Date.now(), upload.id)
      .run();
    throw error;
  }
  const head = await context.env.ARTIFACT_OBJECTS.head(upload.opaque_server_key);
  if (
    !head ||
    head.size !== upload.size ||
    head.customMetadata?.ciphertextHash !== upload.ciphertext_hash
  ) {
    await context.env.ARTIFACT_OBJECTS.delete(upload.opaque_server_key);
    await context.env.SYNC_DB.prepare(
      "UPDATE object_uploads SET status = 'failed', updated_at = ? WHERE id = ?",
    )
      .bind(Date.now(), upload.id)
      .run();
    throw new ApiError(422, "CHECKSUM_MISMATCH", "R2 object metadata or size does not match the artifact");
  }

  const current = await context.env.SYNC_DB.prepare(
    "SELECT * FROM object_manifests WHERE owner_id = ? AND object_id = ?",
  )
    .bind(upload.owner_id, upload.object_id)
    .first<ManifestRow>();
  const artifactBinding = await context.env.SYNC_DB.prepare(
    "SELECT object_id, ciphertext_hash, size, key_version FROM object_manifests WHERE owner_id = ? AND latest_artifact_id = ?",
  )
    .bind(upload.owner_id, upload.artifact_id)
    .first<{ object_id: string; ciphertext_hash: string; size: number; key_version: number }>();
  if (
    artifactBinding &&
    (artifactBinding.object_id !== upload.object_id ||
      artifactBinding.ciphertext_hash !== upload.ciphertext_hash ||
      artifactBinding.size !== upload.size ||
      artifactBinding.key_version !== upload.key_version)
  ) {
    await context.env.ARTIFACT_OBJECTS.delete(upload.opaque_server_key);
    await context.env.SYNC_DB.prepare(
      "UPDATE object_uploads SET status = 'failed', updated_at = ? WHERE id = ?",
    )
      .bind(Date.now(), upload.id)
      .run();
    throw new ApiError(409, "UPLOAD_CONFLICT", "The artifact ID is already bound to another object");
  }
  if (
    current &&
    (upload.logical_version < current.logical_version ||
      (upload.logical_version === current.logical_version && upload.artifact_id !== current.latest_artifact_id))
  ) {
    await context.env.ARTIFACT_OBJECTS.delete(upload.opaque_server_key);
    await context.env.SYNC_DB.prepare(
      "UPDATE object_uploads SET status = 'failed', updated_at = ? WHERE id = ?",
    )
      .bind(Date.now(), upload.id)
      .run();
    throw new ApiError(409, "UPLOAD_CONFLICT", "The object logical version is stale or already published");
  }

  const now = Date.now();
  const manifestChanged = !current || upload.logical_version > current.logical_version;
  const statements: D1PreparedStatement[] = [
    context.env.SYNC_DB.prepare(
      `INSERT INTO object_replicas
       (owner_id, artifact_id, provider_kind, provider_account_id, opaque_server_key,
        encrypted_locator, provider_revision, etag, ciphertext_hash, size, status, updated_at,
        orphaned_at, gc_started_at)
       VALUES (?, ?, 'r2', 'r2', ?, NULL, ?, ?, ?, ?, 'ready', ?, NULL, NULL)
       ON CONFLICT(owner_id, artifact_id, provider_kind, provider_account_id) DO UPDATE SET
         opaque_server_key = excluded.opaque_server_key, provider_revision = excluded.provider_revision,
         etag = excluded.etag, ciphertext_hash = excluded.ciphertext_hash, size = excluded.size,
         status = 'ready', updated_at = excluded.updated_at, orphaned_at = NULL,
         gc_started_at = NULL`,
    ).bind(
      upload.owner_id,
      upload.artifact_id,
      upload.opaque_server_key,
      object.version,
      object.httpEtag,
      upload.ciphertext_hash,
      upload.size,
      now,
    ),
    context.env.SYNC_DB.prepare(
      "UPDATE object_uploads SET status = 'completed', updated_at = ? WHERE id = ?",
    ).bind(now, upload.id),
  ];
  if (manifestChanged) {
    statements.push(
      context.env.SYNC_DB.prepare(
        `INSERT INTO object_manifests
         (owner_id, object_id, object_kind, logical_version, latest_artifact_id, ciphertext_hash,
          size, key_version, updated_hlc, deleted_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)
         ON CONFLICT(owner_id, object_id) DO UPDATE SET object_kind = excluded.object_kind,
           logical_version = excluded.logical_version, latest_artifact_id = excluded.latest_artifact_id,
           ciphertext_hash = excluded.ciphertext_hash, size = excluded.size,
           key_version = excluded.key_version, updated_hlc = excluded.updated_hlc,
           deleted_at = NULL, updated_at = excluded.updated_at
         WHERE object_manifests.logical_version < excluded.logical_version`,
      ).bind(
        upload.owner_id,
        upload.object_id,
        upload.object_kind,
        upload.logical_version,
        upload.artifact_id,
        upload.ciphertext_hash,
        upload.size,
        upload.key_version,
        upload.updated_hlc,
        now,
      ),
    );
    statements.push(
      context.env.SYNC_DB.prepare(
        `INSERT INTO object_changes
         (owner_id, object_id, artifact_id, operation, logical_version, changed_at)
       VALUES (?, ?, ?, 'upsert', ?, ?)`,
      ).bind(upload.owner_id, upload.object_id, upload.artifact_id, upload.logical_version, now),
    );
    statements.push(
      context.env.SYNC_DB.prepare(
        `UPDATE object_replicas SET orphaned_at = COALESCE(orphaned_at, ?), gc_started_at = NULL
         WHERE owner_id = ? AND status = 'ready'
           AND EXISTS (
             SELECT 1 FROM object_changes historical_change
             WHERE historical_change.owner_id = object_replicas.owner_id
               AND historical_change.object_id = ?
               AND historical_change.artifact_id = object_replicas.artifact_id
           )
           AND NOT EXISTS (
             SELECT 1 FROM object_manifests current_manifest
             WHERE current_manifest.owner_id = object_replicas.owner_id
               AND current_manifest.object_id = ?
               AND current_manifest.latest_artifact_id = object_replicas.artifact_id
               AND current_manifest.deleted_at IS NULL
           )`,
      ).bind(now, upload.owner_id, upload.object_id, upload.object_id),
    );
  }
  await context.env.SYNC_DB.batch(statements);
  return context.json({
    status: "completed",
    artifactId: upload.artifact_id,
    objectId: upload.object_id,
    logicalVersion: upload.logical_version,
    ciphertextHash: upload.ciphertext_hash,
    size: upload.size,
  });
}

export async function abortObjectUpload(context: Context<AppEnv>): Promise<Response> {
  const upload = await getUpload(context, routeParam(context, "uploadId"));
  if (upload.status === "completed") {
    throw new ApiError(409, "UPLOAD_CONFLICT", "A completed artifact upload cannot be aborted");
  }
  if (upload.status === "aborted" || upload.status === "failed") {
    return context.json({ status: upload.status, uploadSessionId: upload.id });
  }
  await context.env.ARTIFACT_OBJECTS.resumeMultipartUpload(
    upload.opaque_server_key,
    upload.r2_upload_id,
  ).abort();
  await context.env.SYNC_DB.prepare(
    "UPDATE object_uploads SET status = 'aborted', updated_at = ? WHERE id = ?",
  )
    .bind(Date.now(), upload.id)
    .run();
  return context.json({ status: "aborted", uploadSessionId: upload.id });
}

export async function updateObjectState(context: Context<AppEnv>): Promise<Response> {
  const parsed = objectStateSchema.safeParse(await readJsonBody(context));
  if (!parsed.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Device object state is invalid", parsed.error.issues);
  }
  const identity = context.get("auth");
  if (parsed.data.deviceId !== identity.deviceId) {
    throw new ApiError(401, "UNAUTHENTICATED", "State deviceId does not match the credential");
  }
  const manifest = await context.env.SYNC_DB.prepare(
    "SELECT logical_version, latest_artifact_id, ciphertext_hash FROM object_manifests WHERE owner_id = ? AND object_id = ?",
  )
    .bind(identity.ownerId, parsed.data.objectId)
    .first<{ logical_version: number; latest_artifact_id: string; ciphertext_hash: string }>();
  if (!manifest) {
    throw ownerNotFound();
  }
  if (
    parsed.data.localStatus === "installed" &&
    (parsed.data.installedArtifactId !== manifest.latest_artifact_id ||
      parsed.data.verifiedCiphertextHash !== manifest.ciphertext_hash)
  ) {
    throw new ApiError(409, "UPLOAD_CONFLICT", "Installed state does not match the current manifest");
  }
  await context.env.SYNC_DB.prepare(
    `INSERT INTO device_object_states
     (owner_id, device_id, object_id, observed_logical_version, installed_artifact_id,
      local_status, verified_ciphertext_hash, checked_at, error_code)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(owner_id, device_id, object_id) DO UPDATE SET
       observed_logical_version = excluded.observed_logical_version,
       installed_artifact_id = excluded.installed_artifact_id,
       local_status = excluded.local_status,
       verified_ciphertext_hash = excluded.verified_ciphertext_hash,
       checked_at = excluded.checked_at,
       error_code = excluded.error_code
     WHERE device_object_states.observed_logical_version <= excluded.observed_logical_version`,
  )
    .bind(
      identity.ownerId,
      identity.deviceId,
      parsed.data.objectId,
      parsed.data.observedLogicalVersion,
      parsed.data.installedArtifactId,
      parsed.data.localStatus,
      parsed.data.verifiedCiphertextHash,
      Date.now(),
      parsed.data.errorCode,
    )
    .run();
  return context.json({ status: "recorded", objectId: parsed.data.objectId });
}

export async function getObjectManifest(context: Context<AppEnv>): Promise<Response> {
  const identity = context.get("auth");
  const manifest = await context.env.SYNC_DB.prepare(
    "SELECT * FROM object_manifests WHERE owner_id = ? AND object_id = ?",
  )
    .bind(identity.ownerId, routeParam(context, "objectId"))
    .first<ManifestRow>();
  if (!manifest) {
    throw ownerNotFound();
  }
  const [replicas, deviceStates] = await Promise.all([
    context.env.SYNC_DB.prepare(
      `SELECT artifact_id, provider_kind, provider_account_id, opaque_server_key, encrypted_locator,
              provider_revision, etag, ciphertext_hash, size, status, updated_at
       FROM object_replicas WHERE owner_id = ? AND artifact_id = ? ORDER BY status = 'ready' DESC, updated_at DESC`,
    )
      .bind(identity.ownerId, manifest.latest_artifact_id)
      .all<ReplicaRow>(),
    context.env.SYNC_DB.prepare(
      `SELECT device_id, observed_logical_version, installed_artifact_id, local_status,
              verified_ciphertext_hash, checked_at, error_code
       FROM device_object_states WHERE owner_id = ? AND object_id = ?
       ORDER BY checked_at DESC, device_id`,
    )
      .bind(identity.ownerId, manifest.object_id)
      .all<DeviceStateRow>(),
  ]);
  return context.json({
    manifest: {
      objectId: manifest.object_id,
      objectKind: manifest.object_kind,
      logicalVersion: manifest.logical_version,
      artifactId: manifest.latest_artifact_id,
      ciphertextHash: manifest.ciphertext_hash,
      size: manifest.size,
      keyVersion: manifest.key_version,
      updatedHlc: manifest.updated_hlc,
      deletedAt: manifest.deleted_at,
      updatedAt: manifest.updated_at,
    },
    replicas: replicas.results.map((replica) => ({
      providerKind: replica.provider_kind,
      providerAccountId: replica.provider_account_id,
      providerRevision: replica.provider_revision,
      etag: replica.etag,
      ciphertextHash: replica.ciphertext_hash,
      size: replica.size,
      status: replica.status,
      updatedAt: replica.updated_at,
    })),
    deviceStates: deviceStates.results.map((state) => ({
      deviceId: state.device_id,
      observedLogicalVersion: state.observed_logical_version,
      installedArtifactId: state.installed_artifact_id,
      localStatus: state.local_status,
      verifiedCiphertextHash: state.verified_ciphertext_hash,
      checkedAt: state.checked_at,
      errorCode: state.error_code,
    })),
  });
}

export async function listObjectChanges(context: Context<AppEnv>): Promise<Response> {
  const after = parsePageNumber(context.req.query("after"), "after", 0, Number.MAX_SAFE_INTEGER);
  const limit = parsePageNumber(context.req.query("limit"), "limit", 100, 500);
  const rows = await context.env.SYNC_DB.prepare(
    `SELECT server_seq, object_id, artifact_id, operation, logical_version, changed_at
     FROM object_changes WHERE owner_id = ? AND server_seq > ? ORDER BY server_seq LIMIT ?`,
  )
    .bind(context.get("auth").ownerId, after, limit + 1)
    .all<{ server_seq: number; object_id: string; artifact_id: string | null; operation: string; logical_version: number; changed_at: number }>();
  const hasMore = rows.results.length > limit;
  const changes = (hasMore ? rows.results.slice(0, limit) : rows.results).map((row) => ({
    serverSeq: row.server_seq,
    objectId: row.object_id,
    artifactId: row.artifact_id,
    operation: row.operation,
    logicalVersion: row.logical_version,
    changedAt: row.changed_at,
  }));
  return context.json({
    changes,
    nextCursor: changes.at(-1)?.serverSeq ?? after,
    hasMore,
  });
}

function parseRange(value: string | undefined, size: number): { offset: number; length: number } | null {
  if (!value) {
    return null;
  }
  const match = /^bytes=(\d*)-(\d*)$/.exec(value.trim());
  if (!match || (!match[1] && !match[2])) {
    throw new ApiError(416, "INVALID_REQUEST", "Only one byte range is supported");
  }
  if (!match[1]) {
    const suffix = Number(match[2]);
    if (!Number.isSafeInteger(suffix) || suffix <= 0) {
      throw new ApiError(416, "INVALID_REQUEST", "The byte range is invalid");
    }
    return { offset: Math.max(0, size - suffix), length: Math.min(size, suffix) };
  }
  const offset = Number(match[1]);
  const end = match[2] ? Number(match[2]) : size - 1;
  if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(end) || offset < 0 || end < offset || offset >= size) {
    throw new ApiError(416, "INVALID_REQUEST", "The byte range is invalid");
  }
  return { offset, length: Math.min(size - offset, end - offset + 1) };
}

async function findReadyReplica(context: Context<AppEnv>, artifactId: string): Promise<ReplicaRow> {
  const replica = await context.env.SYNC_DB.prepare(
    `SELECT artifact_id, provider_kind, provider_account_id, opaque_server_key, encrypted_locator,
            provider_revision, etag, ciphertext_hash, size, status, updated_at
     FROM object_replicas WHERE owner_id = ? AND artifact_id = ? AND provider_kind = 'r2' AND status = 'ready'`,
  )
    .bind(context.get("auth").ownerId, artifactId)
    .first<ReplicaRow>();
  if (!replica?.opaque_server_key) {
    throw ownerNotFound();
  }
  return replica;
}

export async function downloadObject(context: Context<AppEnv>): Promise<Response> {
  const replica = await findReadyReplica(context, routeParam(context, "artifactId"));
  const range = parseRange(context.req.header("Range"), replica.size);
  const object = await context.env.ARTIFACT_OBJECTS.get(replica.opaque_server_key!, range ? { range } : undefined);
  if (!object) {
    throw ownerNotFound();
  }
  const headers = new Headers({
    "Accept-Ranges": "bytes",
    "Content-Type": "application/octet-stream",
    "Content-Length": String(range?.length ?? replica.size),
    "X-Agnes-Ciphertext-Hash": replica.ciphertext_hash,
    ETag: replica.etag ?? object.httpEtag,
  });
  if (range) {
    headers.set("Content-Range", `bytes ${range.offset}-${range.offset + range.length - 1}/${replica.size}`);
  }
  return new Response(object.body, { status: range ? 206 : 200, headers });
}

export async function headObject(context: Context<AppEnv>): Promise<Response> {
  const replica = await findReadyReplica(context, routeParam(context, "artifactId"));
  const object = await context.env.ARTIFACT_OBJECTS.head(replica.opaque_server_key!);
  if (!object) {
    throw ownerNotFound();
  }
  return new Response(null, {
    status: 200,
    headers: {
      "Accept-Ranges": "bytes",
      "Content-Length": String(object.size),
      "X-Agnes-Ciphertext-Hash": replica.ciphertext_hash,
      ETag: replica.etag ?? object.httpEtag,
    },
  });
}

export async function deleteObject(context: Context<AppEnv>): Promise<Response> {
  const identity = context.get("auth");
  const artifactId = routeParam(context, "artifactId");
  const current = await context.env.SYNC_DB.prepare(
    "SELECT 1 AS found FROM object_manifests WHERE owner_id = ? AND latest_artifact_id = ? LIMIT 1",
  )
    .bind(identity.ownerId, artifactId)
    .first<{ found: number }>();
  if (current) {
    throw new ApiError(409, "UPLOAD_CONFLICT", "The current artifact cannot be deleted");
  }
  const replica = await findReadyReplica(context, artifactId);
  await context.env.ARTIFACT_OBJECTS.delete(replica.opaque_server_key!);
  await context.env.SYNC_DB.prepare(
    `UPDATE object_replicas SET status = 'deleted', updated_at = ?, gc_started_at = NULL
     WHERE owner_id = ? AND artifact_id = ? AND provider_kind = 'r2'`,
  )
    .bind(Date.now(), identity.ownerId, artifactId)
    .run();
  return context.json({ status: "deleted", artifactId });
}

export async function cleanupObjectUploads(env: AppEnv["Bindings"]): Promise<void> {
  const expired = await env.SYNC_DB.prepare(
    `SELECT id, opaque_server_key, r2_upload_id FROM object_uploads
     WHERE status IN ('pending','completing') AND expires_at <= ? LIMIT 100`,
  )
    .bind(Date.now())
    .all<{ id: string; opaque_server_key: string; r2_upload_id: string }>();
  for (const row of expired.results) {
    try {
      await env.ARTIFACT_OBJECTS.resumeMultipartUpload(row.opaque_server_key, row.r2_upload_id).abort();
    } catch {
      // Expiration is authoritative even if R2 already discarded the multipart session.
    }
    await env.SYNC_DB.prepare(
      "UPDATE object_uploads SET status = 'aborted', updated_at = ? WHERE id = ?",
    )
      .bind(Date.now(), row.id)
      .run();
  }
  await env.SYNC_DB.prepare(
    `DELETE FROM object_uploads
     WHERE status IN ('completed','aborted','failed') AND updated_at < ?`,
  )
    .bind(Date.now() - 7 * 24 * 60 * 60 * 1000)
    .run();
}

export async function cleanupOrphanedObjects(
  env: AppEnv["Bindings"],
  options: {
    now?: number;
    graceMs?: number;
    batchSize?: number;
    claimRetryMs?: number;
  } = {},
): Promise<ObjectGcReport> {
  const now = options.now ?? Date.now();
  const graceMs = options.graceMs ?? boundedInteger(
    env.R2_ORPHAN_GRACE_MS,
    DEFAULT_ORPHAN_GRACE_MS,
    MIN_ORPHAN_GRACE_MS,
    MAX_ORPHAN_GRACE_MS,
  );
  const batchSize = options.batchSize ?? boundedInteger(
    env.R2_ORPHAN_GC_BATCH_SIZE,
    DEFAULT_ORPHAN_GC_BATCH_SIZE,
    1,
    MAX_ORPHAN_GC_BATCH_SIZE,
  );
  const claimRetryMs = options.claimRetryMs ?? GC_CLAIM_RETRY_MS;
  const orphanCutoff = now - graceMs;
  const staleClaimCutoff = now - claimRetryMs;
  const candidates = await env.SYNC_DB.prepare(
    `SELECT owner_id, artifact_id, opaque_server_key, orphaned_at
     FROM object_replicas replica
     WHERE provider_kind = 'r2' AND status = 'ready' AND opaque_server_key IS NOT NULL
       AND orphaned_at IS NOT NULL AND orphaned_at <= ?
       AND (gc_started_at IS NULL OR gc_started_at <= ?)
       AND NOT EXISTS (
         SELECT 1 FROM object_manifests manifest
         WHERE manifest.owner_id = replica.owner_id
           AND manifest.latest_artifact_id = replica.artifact_id
           AND manifest.deleted_at IS NULL
       )
     ORDER BY orphaned_at, owner_id, artifact_id
     LIMIT ?`,
  )
    .bind(orphanCutoff, staleClaimCutoff, Math.min(batchSize, MAX_ORPHAN_GC_BATCH_SIZE))
    .all<OrphanReplicaRow>();
  const report: ObjectGcReport = {
    candidates: candidates.results.length,
    claimed: 0,
    deleted: 0,
    failed: 0,
    skipped: 0,
  };
  for (const candidate of candidates.results) {
    const claim = await env.SYNC_DB.prepare(
      `UPDATE object_replicas SET gc_started_at = ?
       WHERE owner_id = ? AND artifact_id = ? AND provider_kind = 'r2'
         AND status = 'ready' AND opaque_server_key = ?
         AND orphaned_at IS NOT NULL AND orphaned_at <= ?
         AND (gc_started_at IS NULL OR gc_started_at <= ?)
         AND NOT EXISTS (
           SELECT 1 FROM object_manifests manifest
           WHERE manifest.owner_id = object_replicas.owner_id
             AND manifest.latest_artifact_id = object_replicas.artifact_id
             AND manifest.deleted_at IS NULL
         )`,
    )
      .bind(
        now,
        candidate.owner_id,
        candidate.artifact_id,
        candidate.opaque_server_key,
        orphanCutoff,
        staleClaimCutoff,
      )
      .run();
    if (claim.meta.changes !== 1) {
      report.skipped += 1;
      continue;
    }
    report.claimed += 1;
    try {
      await env.ARTIFACT_OBJECTS.delete(candidate.opaque_server_key);
      const finalized = await env.SYNC_DB.prepare(
        `UPDATE object_replicas SET status = 'deleted', updated_at = ?, gc_started_at = NULL
         WHERE owner_id = ? AND artifact_id = ? AND provider_kind = 'r2'
           AND status = 'ready' AND opaque_server_key = ? AND gc_started_at = ?
           AND NOT EXISTS (
             SELECT 1 FROM object_manifests manifest
             WHERE manifest.owner_id = object_replicas.owner_id
               AND manifest.latest_artifact_id = object_replicas.artifact_id
               AND manifest.deleted_at IS NULL
           )`,
      )
        .bind(
          now,
          candidate.owner_id,
          candidate.artifact_id,
          candidate.opaque_server_key,
          now,
        )
        .run();
      if (finalized.meta.changes === 1) {
        report.deleted += 1;
      } else {
        report.skipped += 1;
      }
    } catch {
      report.failed += 1;
    }
  }
  return report;
}
