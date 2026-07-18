import { z } from "zod";

export const PROTOCOL_VERSION = 1;
export const MAX_REQUEST_BYTES = 256 * 1024;
export const MAX_PUSH_CHANGES = 20;
export const DEFAULT_PAGE_LIMIT = 100;
export const MAX_PAGE_LIMIT = 500;
export const MAX_RESPONSE_BYTES = 256 * 1024;
export const uuidSchema = z.uuid();
export const entityIdSchema = z
  .string()
  .min(1)
  .max(128)
  .regex(/^[A-Za-z0-9][A-Za-z0-9._:-]*$/);

export const entityTypeSchema = z.enum([
  "agent",
  "session",
  "message",
  "explicit_memory",
  "memory",
  "workspace",
  "calendar",
  "calendar_event",
  "event_exception",
  "task_list",
  "task",
]);

export const operationSchema = z.enum(["upsert", "delete"]);
export const PAYLOAD_ENCODING = "xchacha20poly1305-v1";
export const TOMBSTONE_ENCODING = "tombstone-v1";
export const EMPTY_PAYLOAD_HASH =
  "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const encryptedPayloadSchema = z
  .string()
  .min(54)
  .max(MAX_REQUEST_BYTES)
  .regex(/^[A-Za-z0-9+/]+$/)
  .refine((value) => {
    if (value.length % 4 === 1) {
      return false;
    }
    try {
      const padded = value.padEnd(Math.ceil(value.length / 4) * 4, "=");
      return btoa(atob(padded)).replace(/=+$/, "") === value;
    } catch {
      return false;
    }
  }, "payload must be canonical unpadded Base64");

export const syncChangeSchema = z
  .object({
    protocolVersion: z.literal(PROTOCOL_VERSION).optional(),
    changeId: uuidSchema,
    deviceId: uuidSchema,
    entityType: entityTypeSchema,
    entityId: entityIdSchema,
    operation: operationSchema,
    baseRevision: z.int().positive().nullable(),
    hlc: z.string().min(1).max(160),
    payloadSchemaVersion: z.int().positive(),
    payloadEncoding: z.enum([PAYLOAD_ENCODING, TOMBSTONE_ENCODING]),
    payload: encryptedPayloadSchema.nullable(),
    payloadHash: z.string().regex(/^[a-f0-9]{64}$/),
    keyVersion: z.int().positive().nullable(),
    createdAt: z.int().nonnegative(),
  })
  .strict()
  .superRefine((change, context) => {
    if (
      change.operation === "upsert" &&
      (change.payloadEncoding !== PAYLOAD_ENCODING || change.payload === null || change.keyVersion === null)
    ) {
      context.addIssue({ code: "custom", message: "upsert requires an encrypted payload" });
    }
    if (
      change.operation === "delete" &&
      (change.payloadEncoding !== TOMBSTONE_ENCODING ||
        change.payload !== null ||
        change.keyVersion !== null ||
        change.payloadHash !== EMPTY_PAYLOAD_HASH)
    ) {
      context.addIssue({ code: "custom", message: "delete requires a canonical tombstone" });
    }
  });

export const pushRequestSchema = z
  .object({
    protocolVersion: z.literal(PROTOCOL_VERSION),
    deviceId: uuidSchema,
    changes: z.array(syncChangeSchema).min(1).max(MAX_PUSH_CHANGES),
  })
  .strict();

export const ackRequestSchema = z
  .object({
    protocolVersion: z.literal(PROTOCOL_VERSION),
    deviceId: uuidSchema,
    cursor: z.int().nonnegative(),
  })
  .strict();

const pairingOpaqueSchema = (maximum: number) =>
  z.string().min(1).max(maximum).regex(/^[A-Za-z0-9_-]+$/);

export const createPairingSessionSchema = z
  .object({
    protocolVersion: z.literal(PROTOCOL_VERSION),
    sessionId: uuidSchema,
    initiatorMessage: pairingOpaqueSchema(256),
  })
  .strict();

export const joinPairingSessionSchema = z
  .object({
    protocolVersion: z.literal(PROTOCOL_VERSION),
    deviceId: uuidSchema,
    deviceName: z.string().trim().min(1).max(80),
    platform: z.string().trim().min(1).max(40).nullable(),
    responderMessage: pairingOpaqueSchema(256),
    responderProof: pairingOpaqueSchema(2_048),
  })
  .strict();

export const finalizePairingSessionSchema = z
  .object({
    protocolVersion: z.literal(PROTOCOL_VERSION),
    deviceId: uuidSchema,
    credentialFingerprint: z.string().regex(/^[a-f0-9]{64}$/),
    transferBundle: pairingOpaqueSchema(32 * 1_024),
  })
  .strict();

export type SyncChange = z.infer<typeof syncChangeSchema>;

export interface ApiErrorBody {
  error: {
    code:
      | "INVALID_REQUEST"
      | "UNAUTHENTICATED"
      | "DEVICE_REVOKED"
      | "REVISION_CONFLICT"
      | "PAYLOAD_TOO_LARGE"
      | "PAIRING_EXPIRED"
      | "PAIRING_NOT_READY"
      | "PAIRING_CONFLICT"
      | "RATE_LIMITED"
      | "SYNC_TEMPORARILY_UNAVAILABLE";
    message: string;
    details?: unknown;
  };
}
