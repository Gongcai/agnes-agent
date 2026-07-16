import { Hono } from "hono";

import { authorizeDevice, resolveIdentity } from "./auth";
import { ApiError, errorResponse } from "./errors";
import { PROTOCOL_VERSION } from "./protocol";
import { listDevices, revokeDevice } from "./routes/devices";
import {
  consumePairingSession,
  createPairingSession,
  finalizePairingSession,
  getPairingJoin,
  getPairingPackage,
  getPairingSession,
  joinPairingSession,
} from "./routes/pairing";
import { ack, bootstrap, pull, push } from "./routes/sync";
import type { AppEnv } from "./types";

const app = new Hono<AppEnv>();

app.use("/v1/*", async (context, next) => {
  const publicPairingPath = /^\/v1\/pairing\/sessions\/[0-9a-f-]+(?:\/package|\/join)?$/;
  const isPublicPairingRequest =
    publicPairingPath.test(context.req.path) &&
    ((context.req.method === "GET" && !context.req.path.endsWith("/join")) ||
      (context.req.method === "POST" && context.req.path.endsWith("/join")));
  if (isPublicPairingRequest) {
    await next();
    return;
  }
  const identity = await resolveIdentity(context.req.raw, context.env);
  await authorizeDevice(context.env.SYNC_DB, identity, Date.now());
  context.set("auth", identity);
  await next();
});

app.get("/v1/health", async (context) => {
  await context.env.SYNC_DB.prepare("SELECT 1 AS ok").first();
  return context.json({
    service: "agnes-sync-api",
    version: "0.1.0",
    protocolVersion: PROTOCOL_VERSION,
    serverTime: Date.now(),
  });
});

app.post("/v1/sync/push", push);
app.get("/v1/sync/pull", pull);
app.get("/v1/sync/bootstrap", bootstrap);
app.post("/v1/sync/ack", ack);
app.get("/v1/devices", listDevices);
app.post("/v1/devices/:deviceId/revoke", revokeDevice);
app.post("/v1/pairing/sessions", createPairingSession);
app.get("/v1/pairing/sessions/:sessionId", getPairingSession);
app.post("/v1/pairing/sessions/:sessionId/join", joinPairingSession);
app.get("/v1/pairing/sessions/:sessionId/join", getPairingJoin);
app.post("/v1/pairing/sessions/:sessionId/finalize", finalizePairingSession);
app.get("/v1/pairing/sessions/:sessionId/package", getPairingPackage);
app.post("/v1/pairing/sessions/:sessionId/consume", consumePairingSession);

app.notFound(() =>
  errorResponse(new ApiError(404, "INVALID_REQUEST", "The requested endpoint does not exist")),
);

app.onError((error) => {
  if (error instanceof ApiError) {
    return errorResponse(error);
  }
  console.error("sync request failed", error instanceof Error ? error.message : "unknown error");
  return errorResponse(
    new ApiError(503, "SYNC_TEMPORARILY_UNAVAILABLE", "Sync is temporarily unavailable"),
  );
});

export { app };
export default {
  fetch: app.fetch,
  scheduled(_controller, env, context) {
    context.waitUntil(
      env.SYNC_DB.prepare("DELETE FROM pairing_sessions WHERE expires_at <= ?")
        .bind(Date.now())
        .run(),
    );
  },
} satisfies ExportedHandler<AppEnv["Bindings"]>;
