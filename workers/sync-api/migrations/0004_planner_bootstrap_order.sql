DROP INDEX IF EXISTS idx_sync_entities_bootstrap;

CREATE INDEX idx_sync_entities_bootstrap
  ON sync_entities(
    owner_id,
    CASE entity_type
      WHEN 'agent' THEN 0
      WHEN 'workspace' THEN 1
      WHEN 'session' THEN 2
      WHEN 'message' THEN 3
      WHEN 'explicit_memory' THEN 4
      WHEN 'memory' THEN 5
      WHEN 'calendar' THEN 6
      WHEN 'calendar_event' THEN 7
      WHEN 'event_exception' THEN 8
      WHEN 'task_list' THEN 9
      WHEN 'task' THEN 10
      ELSE 99
    END,
    entity_type,
    entity_id,
    latest_server_seq
  );
