import path from "node:path";

import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

const identities = [
  {
    token: "test-token-owner-a-device-a",
    ownerId: "owner-a",
    deviceId: "00000000-0000-4000-8000-000000000001",
    deviceName: "Owner A desktop",
    platform: "linux",
  },
  {
    token: "test-token-owner-a-device-b",
    ownerId: "owner-a",
    deviceId: "00000000-0000-4000-8000-000000000002",
    deviceName: "Owner A phone",
    platform: "android",
  },
  {
    token: "test-token-owner-b-device-a",
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
            AUTH_MODE: "test",
            SYNC_TEST_IDENTITIES: JSON.stringify(identities),
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
