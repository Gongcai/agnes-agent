import { env, exports } from "cloudflare:workers";
import { beforeEach, describe, expect, it } from "vitest";

const DEVICE_A = "00000000-0000-4000-8000-000000000001";
const DEVICE_B = "00000000-0000-4000-8000-000000000002";
const OTHER_OWNER_DEVICE = "00000000-0000-4000-8000-000000000003";
const PAIRED_DEVICE = "00000000-0000-4000-8000-000000000004";
const TOKEN_A = "test-token-owner-a-device-a";
const TOKEN_B = "test-token-owner-a-device-b";
const OTHER_OWNER_TOKEN = "test-token-owner-b-device-a";
const PAIRED_TOKEN = "paired-device-token-with-256-bits-of-randomness-placeholder";
const EMPTY_PAYLOAD_HASH =
  "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

beforeEach(async () => {
  await env.SYNC_DB.batch([
    env.SYNC_DB.prepare("DELETE FROM pairing_sessions"),
    env.SYNC_DB.prepare("DELETE FROM sync_acks"),
    env.SYNC_DB.prepare("DELETE FROM sync_changes"),
    env.SYNC_DB.prepare("DELETE FROM sync_entities"),
    env.SYNC_DB.prepare("DELETE FROM devices"),
  ]);
});

async function sha256Hex(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

interface TestChangeOptions {
  changeId?: string;
  entityId?: string;
  entityType?:
    | "agent"
    | "session"
    | "message"
    | "explicit_memory"
    | "memory"
    | "workspace"
    | "calendar"
    | "calendar_event"
    | "event_exception"
    | "task_list"
    | "task";
  operation?: "upsert" | "delete";
  baseRevision?: number | null;
  deviceId?: string;
  payload?: unknown;
  payloadHash?: string;
  createdAt?: number;
}

function makeChange(options: TestChangeOptions = {}) {
  const payload = encryptedPayload(options.payload ?? { name: "Test agent" });
  return {
    changeId: options.changeId ?? "10000000-0000-4000-8000-000000000001",
    deviceId: options.deviceId ?? DEVICE_A,
    entityType: options.entityType ?? "agent",
    entityId: options.entityId ?? "20000000-0000-4000-8000-000000000001",
    operation: options.operation ?? "upsert",
    baseRevision: options.baseRevision ?? null,
    hlc: "1784188800123-0001-device-a",
    payloadSchemaVersion: 1,
    payloadEncoding: "xchacha20poly1305-v1",
    payload,
    payloadHash: options.payloadHash ?? "a".repeat(64),
    keyVersion: 1,
    createdAt: options.createdAt ?? 1784188800123,
  };
}

function encryptedPayload(value: unknown): string {
  const encoded = btoa(JSON.stringify(value)).replace(/=+$/, "");
  return `${encoded}${"A".repeat(Math.max(0, 64 - encoded.length))}`;
}

function request(path: string, token?: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  if (init.body) {
    headers.set("Content-Type", "application/json");
  }
  return exports.default.fetch(`https://sync.example.test${path}`, { ...init, headers });
}

function push(token: string, deviceId: string, changes: unknown[]): Promise<Response> {
  return request("/v1/sync/push", token, {
    method: "POST",
    body: JSON.stringify({ protocolVersion: 1, deviceId, changes }),
  });
}

describe("authentication and health", () => {
  it("rejects unauthenticated requests and registers mapped devices", async () => {
    const rejected = await request("/v1/health");
    expect(rejected.status).toBe(401);
    expect(await rejected.json()).toMatchObject({ error: { code: "UNAUTHENTICATED" } });

    const response = await request("/v1/health", TOKEN_A);
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({
      service: "agnes-sync-api",
      protocolVersion: 1,
    });
    const device = await env.SYNC_DB.prepare(
      "SELECT owner_id, name, platform, credential_fingerprint FROM devices WHERE id = ?",
    )
      .bind(DEVICE_A)
      .first();
    expect(device).toEqual({
      owner_id: "owner-a",
      name: "Owner A desktop",
      platform: "linux",
      credential_fingerprint: expect.stringMatching(/^[a-f0-9]{64}$/),
    });

    const invalid = await request("/v1/health", "not-a-configured-token");
    expect(invalid.status).toBe(401);
    expect(await invalid.json()).toMatchObject({ error: { code: "UNAUTHENTICATED" } });
  });

  it("blocks a revoked device", async () => {
    await request("/v1/health", TOKEN_A);
    await env.SYNC_DB.prepare("UPDATE devices SET revoked_at = ? WHERE id = ?")
      .bind(Date.now(), DEVICE_A)
      .run();

    const response = await request("/v1/health", TOKEN_A);
    expect(response.status).toBe(403);
    expect(await response.json()).toMatchObject({ error: { code: "DEVICE_REVOKED" } });
  });
});

describe("device management", () => {
  it("lists only the authenticated owner devices with current and ack state", async () => {
    await request("/v1/health", TOKEN_A);
    await request("/v1/health", TOKEN_B);
    await request("/v1/health", OTHER_OWNER_TOKEN);
    await env.SYNC_DB.prepare(
      "INSERT INTO sync_acks (owner_id, device_id, last_server_seq, updated_at) VALUES (?, ?, ?, ?)",
    )
      .bind("owner-a", DEVICE_B, 7, Date.now())
      .run();

    const response = await request("/v1/devices", TOKEN_A);
    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      devices: Array<Record<string, unknown>>;
    };
    expect(body.devices).toHaveLength(2);
    expect(body.devices).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: DEVICE_A,
          name: "Owner A desktop",
          platform: "linux",
          current: true,
          revokedAt: null,
        }),
        expect.objectContaining({
          id: DEVICE_B,
          name: "Owner A phone",
          platform: "android",
          current: false,
          lastAckCursor: 7,
        }),
      ]),
    );
    expect(body.devices.some((device) => device.id === OTHER_OWNER_DEVICE)).toBe(false);
  });

  it("revokes another owner device idempotently and blocks its credential", async () => {
    await request("/v1/health", TOKEN_A);
    await request("/v1/health", TOKEN_B);
    await request("/v1/pairing/sessions", TOKEN_B, {
      method: "POST",
      body: JSON.stringify({
        protocolVersion: 1,
        sessionId: "30000000-0000-4000-8000-000000000002",
        initiatorMessage: "revoked_device_pairing_message",
      }),
    });

    const first = await request(`/v1/devices/${DEVICE_B}/revoke`, TOKEN_A, { method: "POST" });
    expect(first.status).toBe(200);
    expect(await first.json()).toMatchObject({
      device: { id: DEVICE_B, current: false, revokedAt: expect.any(Number) },
    });
    const pairingCount = await env.SYNC_DB.prepare(
      "SELECT COUNT(*) AS count FROM pairing_sessions WHERE initiator_device_id = ?",
    )
      .bind(DEVICE_B)
      .first<{ count: number }>();
    expect(pairingCount?.count).toBe(0);
    const repeated = await request(`/v1/devices/${DEVICE_B}/revoke`, TOKEN_A, { method: "POST" });
    expect(repeated.status).toBe(200);

    const rejected = await request("/v1/health", TOKEN_B);
    expect(rejected.status).toBe(403);
    expect(await rejected.json()).toMatchObject({ error: { code: "DEVICE_REVOKED" } });

    const self = await request(`/v1/devices/${DEVICE_A}/revoke`, TOKEN_A, { method: "POST" });
    expect(self.status).toBe(400);
    const current = await request("/v1/health", TOKEN_A);
    expect(current.status).toBe(200);
  });

  it("does not reveal or revoke another owner device", async () => {
    await request("/v1/health", TOKEN_A);
    await request("/v1/health", OTHER_OWNER_TOKEN);
    const response = await request(
      `/v1/devices/${OTHER_OWNER_DEVICE}/revoke`,
      TOKEN_A,
      { method: "POST" },
    );
    expect(response.status).toBe(404);
    expect(await request("/v1/health", OTHER_OWNER_TOKEN).then((value) => value.status)).toBe(200);
  });
});

describe("secure device pairing relay", () => {
  const sessionId = "30000000-0000-4000-8000-000000000001";

  it("relays an opaque one-time PAKE exchange and activates a dynamic credential", async () => {
    await request("/v1/health", TOKEN_A);
    const created = await request("/v1/pairing/sessions", TOKEN_A, {
      method: "POST",
      body: JSON.stringify({
        protocolVersion: 1,
        sessionId,
        initiatorMessage: "spake2_initiator_message",
      }),
    });
    expect(created.status).toBe(200);

    const publicSession = await request(`/v1/pairing/sessions/${sessionId}`);
    expect(publicSession.status).toBe(200);
    expect(await publicSession.json()).toMatchObject({
      sessionId,
      initiatorMessage: "spake2_initiator_message",
    });

    const joined = await request(`/v1/pairing/sessions/${sessionId}/join`, undefined, {
      method: "POST",
      body: JSON.stringify({
        protocolVersion: 1,
        deviceId: PAIRED_DEVICE,
        deviceName: "Paired laptop",
        platform: "linux",
        responderMessage: "spake2_responder_message",
        responderProof: "encrypted_responder_proof",
      }),
    });
    expect(joined.status).toBe(200);
    const duplicateJoin = await request(`/v1/pairing/sessions/${sessionId}/join`, undefined, {
      method: "POST",
      body: JSON.stringify({
        protocolVersion: 1,
        deviceId: PAIRED_DEVICE,
        deviceName: "Paired laptop",
        platform: "linux",
        responderMessage: "different_message",
        responderProof: "different_proof",
      }),
    });
    expect(duplicateJoin.status).toBe(409);

    const pendingJoin = await request(`/v1/pairing/sessions/${sessionId}/join`, TOKEN_A);
    expect(pendingJoin.status).toBe(200);
    expect(await pendingJoin.json()).toMatchObject({
      deviceId: PAIRED_DEVICE,
      deviceName: "Paired laptop",
      responderMessage: "spake2_responder_message",
      responderProof: "encrypted_responder_proof",
    });
    const isolated = await request(`/v1/pairing/sessions/${sessionId}/join`, OTHER_OWNER_TOKEN);
    expect(isolated.status).toBe(404);

    const finalized = await request(`/v1/pairing/sessions/${sessionId}/finalize`, TOKEN_A, {
      method: "POST",
      body: JSON.stringify({
        protocolVersion: 1,
        deviceId: PAIRED_DEVICE,
        credentialFingerprint: await sha256Hex(PAIRED_TOKEN),
        transferBundle: "encrypted_keyset_and_device_credential",
      }),
    });
    expect(finalized.status).toBe(200);
    const repeatedFinalize = await request(`/v1/pairing/sessions/${sessionId}/finalize`, TOKEN_A, {
      method: "POST",
      body: JSON.stringify({
        protocolVersion: 1,
        deviceId: PAIRED_DEVICE,
        credentialFingerprint: await sha256Hex(PAIRED_TOKEN),
        transferBundle: "encrypted_keyset_and_device_credential",
      }),
    });
    expect(repeatedFinalize.status).toBe(200);

    const packageResponse = await request(`/v1/pairing/sessions/${sessionId}/package`);
    expect(packageResponse.status).toBe(200);
    expect(await packageResponse.json()).toMatchObject({
      status: "ready",
      transferBundle: "encrypted_keyset_and_device_credential",
    });
    const authenticated = await request("/v1/health", PAIRED_TOKEN);
    expect(authenticated.status).toBe(200);

    const consumed = await request(`/v1/pairing/sessions/${sessionId}/consume`, PAIRED_TOKEN, {
      method: "POST",
    });
    expect(consumed.status).toBe(200);
    const replay = await request(`/v1/pairing/sessions/${sessionId}/package`);
    expect(replay.status).toBe(410);

    const device = await env.SYNC_DB.prepare(
      "SELECT owner_id, name, credential_fingerprint FROM devices WHERE id = ?",
    )
      .bind(PAIRED_DEVICE)
      .first();
    expect(device).toEqual({
      owner_id: "owner-a",
      name: "Paired laptop",
      credential_fingerprint: await sha256Hex(PAIRED_TOKEN),
    });
  });

  it("expires public sessions and never exposes an unencrypted transfer", async () => {
    await request("/v1/health", TOKEN_A);
    await request("/v1/pairing/sessions", TOKEN_A, {
      method: "POST",
      body: JSON.stringify({
        protocolVersion: 1,
        sessionId,
        initiatorMessage: "opaque_message_only",
      }),
    });
    const row = await env.SYNC_DB.prepare(
      "SELECT initiator_message, responder_proof, transfer_bundle FROM pairing_sessions WHERE id = ?",
    )
      .bind(sessionId)
      .first();
    expect(row).toEqual({
      initiator_message: "opaque_message_only",
      responder_proof: null,
      transfer_bundle: null,
    });
    await env.SYNC_DB.prepare("UPDATE pairing_sessions SET expires_at = 0 WHERE id = ?")
      .bind(sessionId)
      .run();
    const expired = await request(`/v1/pairing/sessions/${sessionId}`);
    expect(expired.status).toBe(410);
    expect(await expired.json()).toMatchObject({ error: { code: "PAIRING_EXPIRED" } });
  });
});

describe("push and pull", () => {
  it("replays a change idempotently without duplicating rows", async () => {
    const change = makeChange({ entityId: "agnes" });
    const first = await push(TOKEN_A, DEVICE_A, [change]);
    expect(first.status).toBe(200);
    expect(await first.json()).toMatchObject({
      accepted: [{ changeId: change.changeId, revision: 1, idempotent: false }],
      conflicts: [],
    });

    const replay = await push(TOKEN_A, DEVICE_A, [change]);
    expect(replay.status).toBe(200);
    expect(await replay.json()).toMatchObject({
      accepted: [{ changeId: change.changeId, revision: 1, idempotent: true }],
      conflicts: [],
    });

    const reusedWithDifferentMetadata = await push(TOKEN_A, DEVICE_A, [
      { ...change, hlc: "1784188800123-0002-device-a" },
    ]);
    expect(reusedWithDifferentMetadata.status).toBe(400);
    expect(await reusedWithDifferentMetadata.json()).toMatchObject({
      error: { code: "INVALID_REQUEST" },
    });

    const counts = await env.SYNC_DB.prepare(
      `SELECT
         (SELECT COUNT(*) FROM sync_entities) AS entities,
         (SELECT COUNT(*) FROM sync_changes) AS changes`,
    ).first();
    expect(counts).toEqual({ entities: 1, changes: 1 });
  });

  it("rejects plaintext JSON and accepts only the canonical encrypted tombstone", async () => {
    const plaintext = {
      ...makeChange(),
      payloadEncoding: "json",
      payload: { name: "Plaintext must not reach D1" },
      keyVersion: null,
    };
    const rejected = await push(TOKEN_A, DEVICE_A, [plaintext]);
    expect(rejected.status).toBe(400);
    expect(await rejected.json()).toMatchObject({ error: { code: "INVALID_REQUEST" } });

    const malformedBase64 = { ...makeChange(), payload: "A".repeat(57) };
    const malformed = await push(TOKEN_A, DEVICE_A, [malformedBase64]);
    expect(malformed.status).toBe(400);

    const tombstone = {
      ...makeChange({ changeId: "10000000-0000-4000-8000-000000000020" }),
      operation: "delete",
      payloadEncoding: "tombstone-v1",
      payload: null,
      payloadHash: EMPTY_PAYLOAD_HASH,
      keyVersion: null,
    };
    const accepted = await push(TOKEN_A, DEVICE_A, [tombstone]);
    expect(accepted.status).toBe(200);
    const pulled = await request("/v1/sync/pull?after=0", TOKEN_B);
    expect(await pulled.json()).toMatchObject({
      changes: [{ operation: "delete", payloadEncoding: "tombstone-v1", payload: null }],
    });
  });

  it("applies mutable CAS changes and reports stale revisions", async () => {
    const created = makeChange();
    await push(TOKEN_A, DEVICE_A, [created]);

    const stale = makeChange({
      changeId: "10000000-0000-4000-8000-000000000002",
      payload: { name: "Stale update" },
      payloadHash: "b".repeat(64),
    });
    const staleResponse = await push(TOKEN_A, DEVICE_A, [stale]);
    expect(await staleResponse.json()).toMatchObject({
      accepted: [],
      conflicts: [
        {
          changeId: stale.changeId,
          currentRevision: 1,
          reason: "REVISION_CONFLICT",
        },
      ],
    });

    const update = makeChange({
      changeId: "10000000-0000-4000-8000-000000000003",
      baseRevision: 1,
      payload: { name: "Updated agent" },
      payloadHash: "c".repeat(64),
    });
    const updateResponse = await push(TOKEN_A, DEVICE_A, [update]);
    expect(await updateResponse.json()).toMatchObject({
      accepted: [{ changeId: update.changeId, revision: 2 }],
      conflicts: [],
    });

    const pulled = await request("/v1/sync/pull?after=0&limit=1", TOKEN_B);
    const firstPage = (await pulled.json()) as {
      changes: Array<{ serverSeq: number; payload: unknown }>;
      nextCursor: number;
      hasMore: boolean;
    };
    expect(firstPage.changes).toHaveLength(1);
    expect(firstPage.changes[0]?.payload).toBe(created.payload);
    expect(firstPage.hasMore).toBe(true);

    const secondPage = await request(
      `/v1/sync/pull?after=${firstPage.nextCursor}&limit=10`,
      TOKEN_B,
    );
    const secondBody = (await secondPage.json()) as {
      changes: Array<{ resultingRevision: number; payload: unknown }>;
      hasMore: boolean;
    };
    expect(secondBody.changes).toEqual([
      expect.objectContaining({ resultingRevision: 2, payload: update.payload }),
    ]);
    expect(secondBody.hasMore).toBe(false);
  });

  it("applies message edits through revision CAS", async () => {
    const message = makeChange({
      entityType: "message",
      entityId: "20000000-0000-4000-8000-000000000010",
      payload: { text: "First" },
    });
    await push(TOKEN_A, DEVICE_A, [message]);

    const overwrite = makeChange({
      changeId: "10000000-0000-4000-8000-000000000010",
      entityType: "message",
      entityId: message.entityId,
      baseRevision: 1,
      payload: { text: "Changed" },
      payloadHash: "d".repeat(64),
    });
    const response = await push(TOKEN_A, DEVICE_A, [overwrite]);
    expect(await response.json()).toMatchObject({
      accepted: [{ changeId: overwrite.changeId, revision: 2 }],
      conflicts: [],
    });
  });

  it("keeps owners isolated and rejects a credential device mismatch", async () => {
    await push(TOKEN_A, DEVICE_A, [makeChange()]);

    const otherOwnerPull = await request("/v1/sync/pull?after=0", OTHER_OWNER_TOKEN);
    expect(await otherOwnerPull.json()).toMatchObject({ changes: [], nextCursor: 0 });

    const mismatch = await push(OTHER_OWNER_TOKEN, OTHER_OWNER_DEVICE, [
      makeChange({ deviceId: DEVICE_A }),
    ]);
    expect(mismatch.status).toBe(401);
    expect(await mismatch.json()).toMatchObject({ error: { code: "UNAUTHENTICATED" } });
  });

  it("keeps the snapshot and change stream cursor atomic", async () => {
    const response = await push(TOKEN_A, DEVICE_A, [makeChange()]);
    const body = (await response.json()) as { accepted: Array<{ serverSeq: number }> };
    const rows = await env.SYNC_DB.prepare(
      `SELECT e.latest_server_seq, c.server_seq, e.latest_change_id, c.change_id
       FROM sync_entities e
       JOIN sync_changes c
         ON c.owner_id = e.owner_id AND c.change_id = e.latest_change_id`,
    ).first();
    expect(rows).toEqual({
      latest_server_seq: body.accepted[0]?.serverSeq,
      server_seq: body.accepted[0]?.serverSeq,
      latest_change_id: "10000000-0000-4000-8000-000000000001",
      change_id: "10000000-0000-4000-8000-000000000001",
    });
  });
});

describe("bootstrap and ack", () => {
  it("orders bootstrap entities by local foreign-key dependencies", async () => {
    const agent = makeChange({
      changeId: "10000000-0000-4000-8000-000000000021",
      entityId: "20000000-0000-4000-8000-000000000021",
    });
    const workspace = makeChange({
      changeId: "10000000-0000-4000-8000-000000000022",
      entityId: "20000000-0000-4000-8000-000000000022",
      entityType: "workspace",
    });
    const session = makeChange({
      changeId: "10000000-0000-4000-8000-000000000023",
      entityId: "20000000-0000-4000-8000-000000000023",
      entityType: "session",
    });
    const calendar = makeChange({
      changeId: "10000000-0000-4000-8000-000000000024",
      entityId: "20000000-0000-4000-8000-000000000024",
      entityType: "calendar",
    });
    const calendarEvent = makeChange({
      changeId: "10000000-0000-4000-8000-000000000025",
      entityId: "20000000-0000-4000-8000-000000000025",
      entityType: "calendar_event",
    });
    const eventException = makeChange({
      changeId: "10000000-0000-4000-8000-000000000026",
      entityId: "20000000-0000-4000-8000-000000000026",
      entityType: "event_exception",
    });
    const taskList = makeChange({
      changeId: "10000000-0000-4000-8000-000000000027",
      entityId: "20000000-0000-4000-8000-000000000027",
      entityType: "task_list",
    });
    const task = makeChange({
      changeId: "10000000-0000-4000-8000-000000000028",
      entityId: "20000000-0000-4000-8000-000000000028",
      entityType: "task",
    });
    await push(TOKEN_A, DEVICE_A, [
      task,
      eventException,
      calendarEvent,
      taskList,
      calendar,
      session,
      workspace,
      agent,
    ]);

    const entityTypes: string[] = [];
    let cursor: string | null = null;
    do {
      const query = cursor
        ? `/v1/sync/bootstrap?limit=1&cursor=${encodeURIComponent(cursor)}`
        : "/v1/sync/bootstrap?limit=1";
      const response = await request(query, TOKEN_B);
      const page = (await response.json()) as {
        entities: Array<{ entityType: string }>;
        nextCursor: string | null;
      };
      entityTypes.push(...page.entities.map((entity) => entity.entityType));
      cursor = page.nextCursor;
    } while (cursor);

    expect(entityTypes).toEqual([
      "agent",
      "workspace",
      "session",
      "calendar",
      "calendar_event",
      "event_exception",
      "task_list",
      "task",
    ]);
  });

  it("uses a stable bootstrap high-water cursor", async () => {
    const first = makeChange();
    const second = makeChange({
      changeId: "10000000-0000-4000-8000-000000000002",
      entityId: "20000000-0000-4000-8000-000000000002",
      entityType: "session",
      payload: { title: "Session" },
      payloadHash: "b".repeat(64),
    });
    await push(TOKEN_A, DEVICE_A, [first, second]);

    const pageOneResponse = await request("/v1/sync/bootstrap?limit=1", TOKEN_B);
    const pageOne = (await pageOneResponse.json()) as {
      entities: Array<{ entityId: string }>;
      snapshotCursor: number;
      nextCursor: string;
      hasMore: boolean;
    };
    expect(pageOne.entities).toHaveLength(1);
    expect(pageOne.hasMore).toBe(true);

    const third = makeChange({
      changeId: "10000000-0000-4000-8000-000000000003",
      entityId: "20000000-0000-4000-8000-000000000003",
      entityType: "workspace",
      payload: { name: "Workspace" },
      payloadHash: "c".repeat(64),
    });
    await push(TOKEN_A, DEVICE_A, [third]);

    const pageTwoResponse = await request(
      `/v1/sync/bootstrap?limit=1&cursor=${encodeURIComponent(pageOne.nextCursor)}`,
      TOKEN_B,
    );
    const pageTwo = (await pageTwoResponse.json()) as {
      entities: Array<{ entityId: string }>;
      snapshotCursor: number;
      hasMore: boolean;
    };
    expect(pageTwo.snapshotCursor).toBe(pageOne.snapshotCursor);
    expect(pageTwo.entities).toHaveLength(1);
    expect(pageTwo.entities[0]?.entityId).not.toBe(third.entityId);
    expect(pageTwo.hasMore).toBe(false);

    const catchUp = await request(
      `/v1/sync/pull?after=${pageOne.snapshotCursor}`,
      TOKEN_B,
    );
    expect(await catchUp.json()).toMatchObject({
      changes: [{ changeId: third.changeId, entityId: third.entityId }],
    });
  });

  it("stores monotonic acks and rejects a cursor ahead of the stream", async () => {
    const pushed = await push(TOKEN_A, DEVICE_A, [makeChange()]);
    const body = (await pushed.json()) as { accepted: Array<{ serverSeq: number }> };
    const serverSeq = body.accepted[0]!.serverSeq;

    const acknowledged = await request("/v1/sync/ack", TOKEN_A, {
      method: "POST",
      body: JSON.stringify({ protocolVersion: 1, deviceId: DEVICE_A, cursor: serverSeq }),
    });
    expect(acknowledged.status).toBe(200);

    const older = await request("/v1/sync/ack", TOKEN_A, {
      method: "POST",
      body: JSON.stringify({ protocolVersion: 1, deviceId: DEVICE_A, cursor: 0 }),
    });
    expect(older.status).toBe(200);
    const stored = await env.SYNC_DB.prepare(
      "SELECT last_server_seq FROM sync_acks WHERE owner_id = ? AND device_id = ?",
    )
      .bind("owner-a", DEVICE_A)
      .first();
    expect(stored).toEqual({ last_server_seq: serverSeq });

    const ahead = await request("/v1/sync/ack", TOKEN_A, {
      method: "POST",
      body: JSON.stringify({ protocolVersion: 1, deviceId: DEVICE_A, cursor: serverSeq + 1 }),
    });
    expect(ahead.status).toBe(400);
    expect(await ahead.json()).toMatchObject({ error: { code: "INVALID_REQUEST" } });
  });
});

describe("request limits", () => {
  it("rejects oversized and malformed payloads", async () => {
    const oversized = await request("/v1/sync/push", TOKEN_A, {
      method: "POST",
      body: JSON.stringify({ data: "x".repeat(256 * 1024) }),
    });
    expect(oversized.status).toBe(413);
    expect(await oversized.json()).toMatchObject({ error: { code: "PAYLOAD_TOO_LARGE" } });

    const malformed = await request("/v1/sync/push", TOKEN_A, {
      method: "POST",
      body: "{not-json",
    });
    expect(malformed.status).toBe(400);
    expect(await malformed.json()).toMatchObject({ error: { code: "INVALID_REQUEST" } });

    let deeplyNested: Record<string, unknown> = { value: "end" };
    for (let depth = 0; depth < 70; depth += 1) {
      deeplyNested = { child: deeplyNested };
    }
    const nested = await request("/v1/sync/push", TOKEN_A, {
      method: "POST",
      body: JSON.stringify(deeplyNested),
    });
    expect(nested.status).toBe(400);
    expect(await nested.json()).toMatchObject({ error: { code: "INVALID_REQUEST" } });
  });
});
