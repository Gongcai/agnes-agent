import { ApiError } from "./errors";
import type { AuthIdentity, Bindings } from "./types";

interface TestIdentity extends AuthIdentity {
  token: string;
}

interface DeviceIdentity extends AuthIdentity {
  tokenSha256: string;
}

interface StoredDeviceIdentity {
  owner_id: string;
  id: string;
  name: string;
  platform: string | null;
}

function isIdentity(candidate: Record<string, unknown>): boolean {
  return (
    typeof candidate.ownerId === "string" &&
    typeof candidate.deviceId === "string" &&
    typeof candidate.deviceName === "string" &&
    (candidate.platform === undefined || typeof candidate.platform === "string")
  );
}

function parseTestIdentities(value: string | undefined): TestIdentity[] {
  if (!value) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((entry): entry is TestIdentity => {
      if (typeof entry !== "object" || entry === null) {
        return false;
      }
      const candidate = entry as Record<string, unknown>;
      return typeof candidate.token === "string" && isIdentity(candidate);
    });
  } catch {
    return [];
  }
}

function parseDeviceIdentities(value: string | undefined): DeviceIdentity[] {
  if (!value) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((entry): entry is DeviceIdentity => {
      if (typeof entry !== "object" || entry === null) {
        return false;
      }
      const candidate = entry as Record<string, unknown>;
      return (
        typeof candidate.tokenSha256 === "string" &&
        /^[a-f0-9]{64}$/.test(candidate.tokenSha256) &&
        isIdentity(candidate)
      );
    });
  } catch {
    return [];
  }
}

async function sha256Hex(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function bearerToken(request: Request): string {
  const authorization = request.headers.get("Authorization");
  const token = authorization?.startsWith("Bearer ") ? authorization.slice(7) : null;
  if (!token || token.length > 512) {
    throw new ApiError(401, "UNAUTHENTICATED", "A bearer token is required");
  }
  return token;
}

export async function resolveIdentity(request: Request, env: Bindings): Promise<AuthIdentity> {
  if (env.AUTH_MODE !== "bearer" && env.AUTH_MODE !== "test") {
    throw new ApiError(401, "UNAUTHENTICATED", "Sync authentication is not configured");
  }
  const token = bearerToken(request);
  const tokenSha256 = await sha256Hex(token);
  const configured =
    env.AUTH_MODE === "bearer"
      ? parseDeviceIdentities(env.SYNC_DEVICE_IDENTITIES).find(
          (candidate) => candidate.tokenSha256 === tokenSha256,
        )
      : parseTestIdentities(env.SYNC_TEST_IDENTITIES).find((candidate) => candidate.token === token);
  if (configured) {
    return {
      ownerId: configured.ownerId,
      deviceId: configured.deviceId,
      deviceName: configured.deviceName,
      ...(configured.platform ? { platform: configured.platform } : {}),
      credentialFingerprint: tokenSha256,
    };
  }

  const stored = await env.SYNC_DB.prepare(
    `SELECT owner_id, id, name, platform
     FROM devices
     WHERE credential_fingerprint = ?`,
  )
    .bind(tokenSha256)
    .first<StoredDeviceIdentity>();
  if (!stored) {
    throw new ApiError(401, "UNAUTHENTICATED", "The sync credential is invalid");
  }
  return {
    ownerId: stored.owner_id,
    deviceId: stored.id,
    deviceName: stored.name,
    ...(stored.platform ? { platform: stored.platform } : {}),
    credentialFingerprint: tokenSha256,
  };
}

export async function authorizeDevice(
  database: D1Database,
  identity: AuthIdentity,
  now: number,
): Promise<void> {
  const device = await database
    .prepare("SELECT owner_id, revoked_at FROM devices WHERE id = ?")
    .bind(identity.deviceId)
    .first<{ owner_id: string; revoked_at: number | null }>();

  if (device && device.owner_id !== identity.ownerId) {
    throw new ApiError(401, "UNAUTHENTICATED", "The device credential is invalid");
  }
  if (device?.revoked_at != null) {
    throw new ApiError(403, "DEVICE_REVOKED", "This device has been revoked");
  }

  if (!device) {
    const inserted = await database
      .prepare(
        `INSERT OR IGNORE INTO devices (
          id, owner_id, name, platform, credential_fingerprint, created_at, last_seen_at, revoked_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL)`,
      )
      .bind(
        identity.deviceId,
        identity.ownerId,
        identity.deviceName,
        identity.platform ?? null,
        identity.credentialFingerprint ?? null,
        now,
        now,
      )
      .run();
    if (inserted.meta.changes > 0) {
      return;
    }

    const concurrentlyCreated = await database
      .prepare("SELECT owner_id, revoked_at FROM devices WHERE id = ?")
      .bind(identity.deviceId)
      .first<{ owner_id: string; revoked_at: number | null }>();
    if (!concurrentlyCreated || concurrentlyCreated.owner_id !== identity.ownerId) {
      throw new ApiError(401, "UNAUTHENTICATED", "The device credential is invalid");
    }
    if (concurrentlyCreated.revoked_at != null) {
      throw new ApiError(403, "DEVICE_REVOKED", "This device has been revoked");
    }
  }

  await database
    .prepare(
      `UPDATE devices
       SET name = ?, platform = ?, credential_fingerprint = ?, last_seen_at = ?
       WHERE id = ?`,
    )
    .bind(
      identity.deviceName,
      identity.platform ?? null,
      identity.credentialFingerprint ?? null,
      now,
      identity.deviceId,
    )
    .run();
}
