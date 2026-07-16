import type { Context } from "hono";

import { ApiError } from "../errors";
import { uuidSchema } from "../protocol";
import type { AppEnv } from "../types";

interface DeviceRow {
  id: string;
  name: string;
  platform: string | null;
  created_at: number;
  last_seen_at: number | null;
  revoked_at: number | null;
  last_ack_cursor: number | null;
}

function serializeDevice(row: DeviceRow, currentDeviceId: string) {
  return {
    id: row.id,
    name: row.name,
    platform: row.platform,
    createdAt: row.created_at,
    lastSeenAt: row.last_seen_at,
    revokedAt: row.revoked_at,
    lastAckCursor: row.last_ack_cursor ?? 0,
    current: row.id === currentDeviceId,
  };
}

async function selectDevice(
  database: D1Database,
  ownerId: string,
  deviceId: string,
): Promise<DeviceRow | null> {
  return database
    .prepare(
      `SELECT d.id, d.name, d.platform, d.created_at, d.last_seen_at, d.revoked_at,
              a.last_server_seq AS last_ack_cursor
       FROM devices d
       LEFT JOIN sync_acks a ON a.owner_id = d.owner_id AND a.device_id = d.id
       WHERE d.owner_id = ? AND d.id = ?`,
    )
    .bind(ownerId, deviceId)
    .first<DeviceRow>();
}

export async function listDevices(context: Context<AppEnv>): Promise<Response> {
  const identity = context.get("auth");
  const result = await context.env.SYNC_DB.prepare(
    `SELECT d.id, d.name, d.platform, d.created_at, d.last_seen_at, d.revoked_at,
            a.last_server_seq AS last_ack_cursor
     FROM devices d
     LEFT JOIN sync_acks a ON a.owner_id = d.owner_id AND a.device_id = d.id
     WHERE d.owner_id = ?
     ORDER BY (d.revoked_at IS NOT NULL), d.last_seen_at DESC, d.id`,
  )
    .bind(identity.ownerId)
    .all<DeviceRow>();
  return context.json({
    devices: result.results.map((device) => serializeDevice(device, identity.deviceId)),
    serverTime: Date.now(),
  });
}

export async function revokeDevice(context: Context<AppEnv>): Promise<Response> {
  const identity = context.get("auth");
  const parsedDeviceId = uuidSchema.safeParse(context.req.param("deviceId"));
  if (!parsedDeviceId.success) {
    throw new ApiError(400, "INVALID_REQUEST", "Device id is invalid");
  }
  const deviceId = parsedDeviceId.data;
  if (deviceId === identity.deviceId) {
    throw new ApiError(400, "INVALID_REQUEST", "The current device cannot revoke itself");
  }

  const existing = await selectDevice(context.env.SYNC_DB, identity.ownerId, deviceId);
  if (!existing) {
    throw new ApiError(404, "INVALID_REQUEST", "Device was not found");
  }
  if (existing.revoked_at == null) {
    const revokedAt = Date.now();
    await context.env.SYNC_DB.batch([
      context.env.SYNC_DB.prepare(
        "UPDATE devices SET revoked_at = ? WHERE owner_id = ? AND id = ? AND revoked_at IS NULL",
      ).bind(revokedAt, identity.ownerId, deviceId),
      context.env.SYNC_DB.prepare(
        `DELETE FROM pairing_sessions
         WHERE owner_id = ? AND (initiator_device_id = ? OR requested_device_id = ?)`,
      ).bind(identity.ownerId, deviceId, deviceId),
    ]);
  }
  const revoked = await selectDevice(context.env.SYNC_DB, identity.ownerId, deviceId);
  if (!revoked?.revoked_at) {
    throw new ApiError(503, "SYNC_TEMPORARILY_UNAVAILABLE", "Device revocation was not persisted");
  }
  return context.json({
    device: serializeDevice(revoked, identity.deviceId),
    serverTime: Date.now(),
  });
}
