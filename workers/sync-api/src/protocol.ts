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
]);

export const operationSchema = z.enum(["upsert", "delete"]);

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
    payloadEncoding: z.string().min(1).max(64),
    payload: z.json(),
    payloadHash: z.string().regex(/^[a-f0-9]{64}$/),
    keyVersion: z.int().nonnegative().nullable(),
    createdAt: z.int().nonnegative(),
  })
  .strict()
  .superRefine((change, context) => {
    if (change.operation === "upsert" && change.payload === null) {
      context.addIssue({
        code: "custom",
        path: ["payload"],
        message: "payload is required for upsert",
      });
    }
    if (change.operation === "delete" && change.payload !== null) {
      context.addIssue({
        code: "custom",
        path: ["payload"],
        message: "payload must be null for delete",
      });
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

export type SyncChange = z.infer<typeof syncChangeSchema>;

export interface ApiErrorBody {
  error: {
    code:
      | "INVALID_REQUEST"
      | "UNAUTHENTICATED"
      | "DEVICE_REVOKED"
      | "REVISION_CONFLICT"
      | "PAYLOAD_TOO_LARGE"
      | "RATE_LIMITED"
      | "SYNC_TEMPORARILY_UNAVAILABLE";
    message: string;
    details?: unknown;
  };
}
