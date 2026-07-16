import path from "node:path";
import { createHash } from "node:crypto";

import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

const identities = [
  {
    tokenSha256: createHash("sha256").update("test-token-owner-a-device-a").digest("hex"),
    ownerId: "owner-a",
    deviceId: "00000000-0000-4000-8000-000000000001",
    deviceName: "Owner A desktop",
    platform: "linux",
  },
  {
    tokenSha256: createHash("sha256").update("test-token-owner-a-device-b").digest("hex"),
    ownerId: "owner-a",
    deviceId: "00000000-0000-4000-8000-000000000002",
    deviceName: "Owner A phone",
    platform: "android",
  },
  {
    tokenSha256: createHash("sha256").update("test-token-owner-b-device-a").digest("hex"),
    ownerId: "owner-b",
    deviceId: "00000000-0000-4000-8000-000000000003",
    deviceName: "Owner B desktop",
    platform: "linux",
  },
];

export default defineConfig(async () => {
  const migrations = await readD1Migrations(path.join(__dirname, "migrations"));
  return {
    plugins: [
      cloudflareTest({
        wrangler: { configPath: "./wrangler.jsonc" },
        miniflare: {
          bindings: {
            AUTH_MODE: "bearer",
            SYNC_DEVICE_IDENTITIES: JSON.stringify(identities),
            TEST_MIGRATIONS: migrations,
          },
        },
      }),
    ],
    test: {
      setupFiles: ["./test/apply-migrations.ts"],
    },
  };
});
