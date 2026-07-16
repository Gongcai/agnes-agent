export interface Bindings {
  SYNC_DB: D1Database;
  AUTH_MODE?: string;
  SYNC_TEST_IDENTITIES?: string;
}

export interface AuthIdentity {
  ownerId: string;
  deviceId: string;
  deviceName: string;
  platform?: string;
}

export interface AppEnv {
  Bindings: Bindings;
  Variables: {
    auth: AuthIdentity;
  };
}
