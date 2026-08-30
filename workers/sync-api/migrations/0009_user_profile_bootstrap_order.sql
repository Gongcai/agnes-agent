DROP INDEX IF EXISTS idx_sync_entities_bootstrap;

CREATE INDEX idx_sync_entities_bootstrap
  ON sync_entities(
    owner_id,
    CASE entity_type
      WHEN 'user_profile' THEN 0
      WHEN 'agent' THEN 1
      WHEN 'workspace' THEN 2
      WHEN 'session' THEN 3
      WHEN 'message' THEN 4
      WHEN 'memory_file' THEN 5
      WHEN 'calendar' THEN 6
      WHEN 'calendar_event' THEN 7
      WHEN 'event_exception' THEN 8
      WHEN 'task_list' THEN 9
      WHEN 'task' THEN 10
      WHEN 'reading_book' THEN 11
      WHEN 'reading_highlight' THEN 12
      ELSE 99
    END,
    entity_type,
    entity_id,
    latest_server_seq
  );
