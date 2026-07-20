ALTER TABLE object_replicas ADD COLUMN orphaned_at INTEGER;
ALTER TABLE object_replicas ADD COLUMN gc_started_at INTEGER;

UPDATE object_replicas
SET orphaned_at = (
  SELECT MAX(current_manifest.updated_at)
  FROM object_changes historical_change
  JOIN object_manifests current_manifest
    ON current_manifest.owner_id = historical_change.owner_id
   AND current_manifest.object_id = historical_change.object_id
  WHERE historical_change.owner_id = object_replicas.owner_id
    AND historical_change.artifact_id = object_replicas.artifact_id
    AND current_manifest.latest_artifact_id <> object_replicas.artifact_id
)
WHERE status = 'ready'
  AND NOT EXISTS (
    SELECT 1 FROM object_manifests current_reference
    WHERE current_reference.owner_id = object_replicas.owner_id
      AND current_reference.latest_artifact_id = object_replicas.artifact_id
      AND current_reference.deleted_at IS NULL
  );

CREATE INDEX idx_object_replicas_gc
  ON object_replicas(provider_kind, status, orphaned_at, gc_started_at);
