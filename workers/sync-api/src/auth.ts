import { ApiError } from "./errors";
import type { AuthIdentity, Bindings } from "./types";

interface TestIdentity extends AuthIdentity {
  token: string;
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
      return (
        typeof candidate.token === "string" &&
        typeof candidate.ownerId === "string" &&
        typeof candidate.deviceId === "string" &&
        typeof candidate.deviceName === "string" &&
        (candidate.platform === undefined || typeof candidate.platform === "string")
      );
    });
  } catch {
    return [];
  }
}

export function resolveIdentity(request: Request, env: Bindings): AuthIdentity {
  if (env.AUTH_MODE !== "test") {
    throw new ApiError(401, "UNAUTHENTICATED", "Sync authentication is not configured");
  }

  const authorization = request.headers.get("Authorization");
  const token = authorization?.startsWith("Bearer ") ? authorization.slice(7) : null;
  if (!token) {
    throw new ApiError(401, "UNAUTHENTICATED", "A bearer token is required");
  }

  const identity = parseTestIdentities(env.SYNC_TEST_IDENTITIES).find(
    (candidate) => candidate.token === token,
  );
  if (!identity) {
    throw new ApiError(401, "UNAUTHENTICATED", "The sync credential is invalid");
  }
  return {
    ownerId: identity.ownerId,
    deviceId: identity.deviceId,
    deviceName: identity.deviceName,
    ...(identity.platform ? { platform: identity.platform } : {}),
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
        ) VALUES (?, ?, ?, ?, NULL, ?, ?, NULL)`,
      )
      .bind(
        identity.deviceId,
        identity.ownerId,
        identity.deviceName,
        identity.platform ?? null,
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
    .prepare("UPDATE devices SET last_seen_at = ? WHERE id = ?")
    .bind(now, identity.deviceId)
    .run();
}
