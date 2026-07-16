declare namespace Cloudflare {
  interface GlobalProps {
    mainModule: typeof import("../src/index");
  }

  interface Env {
    SYNC_DB: D1Database;
    AUTH_MODE: string;
    SYNC_DEVICE_IDENTITIES: string;
    SYNC_TEST_IDENTITIES: string;
    TEST_MIGRATIONS: import("cloudflare:test").D1Migration[];
  }
}
