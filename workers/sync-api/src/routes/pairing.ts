import type { Context } from "hono";

import { ApiError } from "../errors";
import {
  createPairingSessionSchema,
  finalizePairingSessionSchema,
  joinPairingSessionSchema,
  PROTOCOL_VERSION,
  uuidSchema,
} from "../protocol";
import { readJsonBody } from "../request";
import type { AppEnv } from "../types";

const PAIRING_TTL_MS = 10 * 60 * 1_000;
const MAX_OPEN_SESSIONS = 5;

interface PairingRow {
  id: string;
  owner_id: string;
  initiator_device_id: string;
  initiator_message: string;
  responder_message: string | null;
  responder_proof: string | null;
  requested_device_id: string | null;
  requested_device_name: string | null;
  requested_platform: string | null;
  transfer_bundle: string | null;
  status: "open" | "joined" | "ready" | "consumed";
  created_at: number;
  expires_at: number;
}

function pairingId(context: Context<AppEnv>): string {
  const parsed = uuidSchema.safeParse(context.req.param("sessionId"));
  if (!parsed.success) {
    throw new ApiError(404, "INVALID_REQUEST", "Pairing session was not found");
  }
  return parsed.data;
}

async function selectPairing(database: D1Database, id: string): Promise<PairingRow | null> {
  return database
    .prepare(
      `SELECT id, owner_id, initiator_device_id, initiator_message, responder_message,
              responder_proof, requested_device_id, requested_device_name, requested_platform,
              transfer_bundle, status, created_at, expires_at
       FROM pairing_sessions
       WHERE id = ?`,
    )
    .bind(id)
    .first<PairingRow>();
}

function requireLivePairing(row: PairingRow | null, now: number): PairingRow {
  if (!row) {
    throw new ApiError(404, "INVALID_REQUEST", "Pairing session was not found");
  }
  if (row.expires_at <= now || row.status === "consumed") {
    throw new ApiError(410, "PAIRING_EXPIRED", "Pairing session expired or was already used");
  }
  return row;
}

function requireInitiator(context: Context<AppEnv>, row: PairingRow): void {
  const identity = context.get("auth");
  if (row.owner_id !== identity.ownerId || row.initiator_device_id !== identity.deviceId) {
    throw new ApiError(404, "INVALID_REQUEST", "Pairing session was not found");
  }
}

export async function createPairingSession(context: Context<AppEnv>): Promise<Response> {
  const parsed = createPairingSessionSchema.safeParse(await readJsonBody(context));
  if (!parsed.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Pairing session request is invalid");
  }
  const identity = context.get("auth");
  const now = Date.now();
  const count = await context.env.SYNC_DB.prepare(
    `SELECT COUNT(*) AS count
     FROM pairing_sessions
     WHERE owner_id = ? AND initiator_device_id = ? AND expires_at > ? AND status != 'consumed'`,
  )
    .bind(identity.ownerId, identity.deviceId, now)
    .first<{ count: number }>();
  if ((count?.count ?? 0) >= MAX_OPEN_SESSIONS) {
    throw new ApiError(429, "RATE_LIMITED", "Too many pairing sessions are active");
  }
  const expiresAt = now + PAIRING_TTL_MS;
  try {
    await context.env.SYNC_DB.prepare(
      `INSERT INTO pairing_sessions (
         id, owner_id, initiator_device_id, initiator_message, status, created_at, expires_at
       ) VALUES (?, ?, ?, ?, 'open', ?, ?)`,
    )
      .bind(
        parsed.data.sessionId,
        identity.ownerId,
        identity.deviceId,
        parsed.data.initiatorMessage,
        now,
        expiresAt,
      )
      .run();
  } catch {
    throw new ApiError(409, "PAIRING_CONFLICT", "Pairing session already exists");
  }
  return context.json({
    sessionId: parsed.data.sessionId,
    expiresAt,
    serverTime: now,
  });
}

export async function getPairingSession(context: Context<AppEnv>): Promise<Response> {
  const now = Date.now();
  const row = requireLivePairing(await selectPairing(context.env.SYNC_DB, pairingId(context)), now);
  return context.json({
    protocolVersion: PROTOCOL_VERSION,
    sessionId: row.id,
    initiatorMessage: row.initiator_message,
    expiresAt: row.expires_at,
    serverTime: now,
  });
}

export async function joinPairingSession(context: Context<AppEnv>): Promise<Response> {
  const id = pairingId(context);
  const parsed = joinPairingSessionSchema.safeParse(await readJsonBody(context));
  if (!parsed.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Pairing join request is invalid");
  }
  const now = Date.now();
  const row = requireLivePairing(await selectPairing(context.env.SYNC_DB, id), now);
  if (
    (row.status === "joined" || row.status === "ready") &&
    row.responder_message === parsed.data.responderMessage &&
    row.responder_proof === parsed.data.responderProof &&
    row.requested_device_id === parsed.data.deviceId &&
    row.requested_device_name === parsed.data.deviceName &&
    row.requested_platform === parsed.data.platform
  ) {
    return context.json({ status: "joined", expiresAt: row.expires_at, serverTime: now });
  }
  const result = await context.env.SYNC_DB.prepare(
    `UPDATE pairing_sessions
     SET responder_message = ?, responder_proof = ?, requested_device_id = ?,
         requested_device_name = ?, requested_platform = ?, status = 'joined', joined_at = ?
     WHERE id = ? AND status = 'open' AND expires_at > ?`,
  )
    .bind(
      parsed.data.responderMessage,
      parsed.data.responderProof,
      parsed.data.deviceId,
      parsed.data.deviceName,
      parsed.data.platform,
      now,
      id,
      now,
    )
    .run();
  if (result.meta.changes !== 1) {
    throw new ApiError(409, "PAIRING_CONFLICT", "Pairing session was already joined");
  }
  return context.json({ status: "joined", expiresAt: row.expires_at, serverTime: now });
}

export async function getPairingJoin(context: Context<AppEnv>): Promise<Response> {
  const now = Date.now();
  const row = requireLivePairing(await selectPairing(context.env.SYNC_DB, pairingId(context)), now);
  requireInitiator(context, row);
  if (
    row.status !== "joined" ||
    !row.responder_message ||
    !row.responder_proof ||
    !row.requested_device_id ||
    !row.requested_device_name
  ) {
    throw new ApiError(409, "PAIRING_NOT_READY", "No device is waiting for approval");
  }
  return context.json({
    deviceId: row.requested_device_id,
    deviceName: row.requested_device_name,
    platform: row.requested_platform,
    responderMessage: row.responder_message,
    responderProof: row.responder_proof,
    expiresAt: row.expires_at,
    serverTime: now,
  });
}

export async function finalizePairingSession(context: Context<AppEnv>): Promise<Response> {
  const id = pairingId(context);
  const parsed = finalizePairingSessionSchema.safeParse(await readJsonBody(context));
  if (!parsed.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Pairing finalization request is invalid");
  }
  const now = Date.now();
  const row = requireLivePairing(await selectPairing(context.env.SYNC_DB, id), now);
  requireInitiator(context, row);
  if (row.status === "ready" && row.requested_device_id === parsed.data.deviceId) {
    const device = await context.env.SYNC_DB.prepare(
      "SELECT credential_fingerprint FROM devices WHERE owner_id = ? AND id = ?",
    )
      .bind(row.owner_id, parsed.data.deviceId)
      .first<{ credential_fingerprint: string | null }>();
    if (
      device?.credential_fingerprint === parsed.data.credentialFingerprint &&
      row.transfer_bundle === parsed.data.transferBundle
    ) {
      return context.json({ status: "ready", deviceId: parsed.data.deviceId, serverTime: now });
    }
  }
  if (row.status !== "joined" || row.requested_device_id !== parsed.data.deviceId) {
    throw new ApiError(409, "PAIRING_NOT_READY", "Pairing session is not ready to finalize");
  }
  try {
    const results = await context.env.SYNC_DB.batch([
      context.env.SYNC_DB.prepare(
        `INSERT INTO devices (
           id, owner_id, name, platform, credential_fingerprint, created_at, last_seen_at, revoked_at
         )
         SELECT requested_device_id, owner_id, requested_device_name, requested_platform, ?, ?, NULL, NULL
         FROM pairing_sessions
         WHERE id = ? AND owner_id = ? AND initiator_device_id = ?
           AND status = 'joined' AND expires_at > ?`,
      ).bind(
        parsed.data.credentialFingerprint,
        now,
        id,
        row.owner_id,
        row.initiator_device_id,
        now,
      ),
      context.env.SYNC_DB.prepare(
        `UPDATE pairing_sessions
         SET transfer_bundle = ?, status = 'ready', finalized_at = ?
         WHERE id = ? AND status = 'joined' AND expires_at > ?`,
      ).bind(parsed.data.transferBundle, now, id, now),
    ]);
    if (results.some((result) => result.meta.changes !== 1)) {
      throw new Error("pairing transaction did not update exactly one row");
    }
  } catch {
    throw new ApiError(409, "PAIRING_CONFLICT", "The requested device is already registered");
  }
  return context.json({ status: "ready", deviceId: parsed.data.deviceId, serverTime: now });
}

export async function getPairingPackage(context: Context<AppEnv>): Promise<Response> {
  const now = Date.now();
  const row = requireLivePairing(await selectPairing(context.env.SYNC_DB, pairingId(context)), now);
  return context.json({
    status: row.status === "ready" ? "ready" : "pending",
    transferBundle: row.status === "ready" ? row.transfer_bundle : null,
    expiresAt: row.expires_at,
    serverTime: now,
  });
}

export async function consumePairingSession(context: Context<AppEnv>): Promise<Response> {
  const id = pairingId(context);
  const identity = context.get("auth");
  const now = Date.now();
  const result = await context.env.SYNC_DB.prepare(
    `UPDATE pairing_sessions
     SET status = 'consumed', consumed_at = ?, transfer_bundle = NULL
     WHERE id = ? AND owner_id = ? AND requested_device_id = ?
       AND status = 'ready' AND expires_at > ?`,
  )
    .bind(now, id, identity.ownerId, identity.deviceId, now)
    .run();
  if (result.meta.changes !== 1) {
    throw new ApiError(409, "PAIRING_NOT_READY", "Pairing package is not ready or was already used");
  }
  return context.json({ status: "consumed", serverTime: now });
}
