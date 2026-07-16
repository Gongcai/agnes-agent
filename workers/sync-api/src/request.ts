import type { Context } from "hono";

import { ApiError } from "./errors";
import { MAX_REQUEST_BYTES } from "./protocol";
import type { AppEnv } from "./types";

const MAX_JSON_DEPTH = 64;
const MAX_JSON_NODES = 50_000;

function validateJsonComplexity(value: unknown): void {
  const stack: Array<{ value: unknown; depth: number }> = [{ value, depth: 0 }];
  let nodes = 0;
  while (stack.length > 0) {
    const current = stack.pop()!;
    nodes += 1;
    if (nodes > MAX_JSON_NODES) {
      throw new ApiError(400, "INVALID_REQUEST", "Request JSON is too complex");
    }
    if (current.value === null || typeof current.value !== "object") {
      continue;
    }
    const children = Array.isArray(current.value)
      ? current.value
      : Object.values(current.value as Record<string, unknown>);
    if (children.length > 0 && current.depth >= MAX_JSON_DEPTH) {
      throw new ApiError(400, "INVALID_REQUEST", "Request JSON exceeds the nesting limit");
    }
    for (const child of children) {
      stack.push({ value: child, depth: current.depth + 1 });
    }
  }
}

export async function readJsonBody(context: Context<AppEnv>): Promise<unknown> {
  const declaredLength = context.req.header("Content-Length");
  if (declaredLength && Number(declaredLength) > MAX_REQUEST_BYTES) {
    throw new ApiError(413, "PAYLOAD_TOO_LARGE", "Request body exceeds 256 KiB");
  }

  const raw = await context.req.arrayBuffer();
  if (raw.byteLength > MAX_REQUEST_BYTES) {
    throw new ApiError(413, "PAYLOAD_TOO_LARGE", "Request body exceeds 256 KiB");
  }
  try {
    const parsed = JSON.parse(new TextDecoder().decode(raw)) as unknown;
    validateJsonComplexity(parsed);
    return parsed;
  } catch (error) {
    if (error instanceof ApiError) {
      throw error;
    }
    throw new ApiError(400, "INVALID_REQUEST", "Request body must be valid JSON");
  }
}

export function parsePageNumber(
  value: string | undefined,
  name: string,
  defaultValue: number,
  maximum: number,
): number {
  if (value === undefined) {
    return defaultValue;
  }
  if (!/^\d+$/.test(value)) {
    throw new ApiError(400, "INVALID_REQUEST", `${name} must be a non-negative integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed > maximum) {
    throw new ApiError(400, "INVALID_REQUEST", `${name} is outside the supported range`);
  }
  return parsed;
}
