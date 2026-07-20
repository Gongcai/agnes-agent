export interface Bindings {
  SYNC_DB: D1Database;
  ARTIFACT_OBJECTS: R2Bucket;
  AUTH_MODE?: string;
  SYNC_DEVICE_IDENTITIES?: string;
  SYNC_TEST_IDENTITIES?: string;
  R2_OWNER_QUOTA_BYTES?: string;
  R2_ORPHAN_GRACE_MS?: string;
  R2_ORPHAN_GC_BATCH_SIZE?: string;
}

export interface AuthIdentity {
  ownerId: string;
  deviceId: string;
  deviceName: string;
  platform?: string;
  credentialFingerprint?: string;
}

export interface AppEnv {
  Bindings: Bindings;
  Variables: {
    auth: AuthIdentity;
  };
}
