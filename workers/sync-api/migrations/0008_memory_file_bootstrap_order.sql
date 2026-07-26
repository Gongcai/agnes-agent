DROP INDEX IF EXISTS idx_sync_entities_bootstrap;

DELETE FROM sync_changes WHERE entity_type IN ('explicit_memory', 'memory');
DELETE FROM sync_entities WHERE entity_type IN ('explicit_memory', 'memory');

CREATE INDEX idx_sync_entities_bootstrap
  ON sync_entities(
    owner_id,
    CASE entity_type
      WHEN 'agent' THEN 0
      WHEN 'workspace' THEN 1
      WHEN 'session' THEN 2
      WHEN 'message' THEN 3
      WHEN 'memory_file' THEN 4
      WHEN 'calendar' THEN 5
      WHEN 'calendar_event' THEN 6
      WHEN 'event_exception' THEN 7
      WHEN 'task_list' THEN 8
      WHEN 'task' THEN 9
      WHEN 'reading_book' THEN 10
      WHEN 'reading_highlight' THEN 11
      ELSE 99
    END,
    entity_type,
    entity_id,
    latest_server_seq
  );
