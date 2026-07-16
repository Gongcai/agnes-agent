import { env, exports } from "cloudflare:workers";
import { beforeEach, describe, expect, it } from "vitest";

const DEVICE_A = "00000000-0000-4000-8000-000000000001";
const DEVICE_B = "00000000-0000-4000-8000-000000000002";
const OTHER_OWNER_DEVICE = "00000000-0000-4000-8000-000000000003";
const TOKEN_A = "test-token-owner-a-device-a";
const TOKEN_B = "test-token-owner-a-device-b";
const OTHER_OWNER_TOKEN = "test-token-owner-b-device-a";

beforeEach(async () => {
  await env.SYNC_DB.batch([
    env.SYNC_DB.prepare("DELETE FROM sync_acks"),
    env.SYNC_DB.prepare("DELETE FROM sync_changes"),
    env.SYNC_DB.prepare("DELETE FROM sync_entities"),
    env.SYNC_DB.prepare("DELETE FROM devices"),
  ]);
});

interface TestChangeOptions {
  changeId?: string;
  entityId?: string;
  entityType?: "agent" | "session" | "message" | "explicit_memory" | "memory" | "workspace";
  operation?: "upsert" | "delete";
  baseRevision?: number | null;
  deviceId?: string;
  payload?: unknown;
  payloadHash?: string;
  createdAt?: number;
}

function makeChange(options: TestChangeOptions = {}) {
  return {
    changeId: options.changeId ?? "10000000-0000-4000-8000-000000000001",
    deviceId: options.deviceId ?? DEVICE_A,
    entityType: options.entityType ?? "agent",
    entityId: options.entityId ?? "20000000-0000-4000-8000-000000000001",
    operation: options.operation ?? "upsert",
    baseRevision: options.baseRevision ?? null,
    hlc: "1784188800123-0001-device-a",
    payloadSchemaVersion: 1,
    payloadEncoding: "json",
    payload: options.payload ?? { name: "Test agent" },
    payloadHash: options.payloadHash ?? "a".repeat(64),
    keyVersion: null,
    createdAt: options.createdAt ?? 1784188800123,
  };
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
    expect(firstPage.changes[0]?.payload).toEqual({ name: "Test agent" });
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
      expect.objectContaining({ resultingRevision: 2, payload: { name: "Updated agent" } }),
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
    await push(TOKEN_A, DEVICE_A, [session, workspace, agent]);

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

    expect(entityTypes).toEqual(["agent", "workspace", "session"]);
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
