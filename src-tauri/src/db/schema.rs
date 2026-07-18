//! Database DDL for rusqlite / SQLite.
//! Embedding metadata is separated from sqlite-vec virtual tables, which are
//! created lazily for each embedding dimension.

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agents (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  persona TEXT,
  scenario TEXT,
  system_prompt TEXT,
  greeting TEXT,
  example_dialogue TEXT,
  model TEXT,
  tool_policy TEXT,                -- 结构化 JSON
  avatar TEXT,
  tags TEXT,
  thinking_mode TEXT,              -- 思考模式/强度: off|auto|low|medium|high
  thinking_budget INTEGER,         -- 思考预算(token)，Claude 的 budget_tokens，0 = 按强度预设
  created_at TEXT,
  updated_at TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL REFERENCES agents(id),
  title TEXT,
  context_limit INTEGER,          -- NULL = 跟随模型能力
  compress_threshold REAL DEFAULT 0.85,
  recency_window INTEGER DEFAULT 20,
  reserved_output_tokens INTEGER,
  summarizer_model TEXT,
  model TEXT,                      -- 会话级模型覆盖（空 = 沿用角色卡默认）
  thinking_mode TEXT,              -- 会话级思考模式（空 = 沿用角色卡默认）
  thinking_budget INTEGER,         -- 会话级思考预算(token)
  permission_mode TEXT NOT NULL DEFAULT 'auto', -- Session-level tool permission mode
  summary TEXT,                    -- 会话级状态，非普通消息片段
  summary_updated_at TEXT,
  created_at TEXT,
  updated_at TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT,
  pinned INTEGER DEFAULT 0,
  workspace_id TEXT REFERENCES workspaces(id)  -- NULL=普通对话，非空=归属某工作区
);

CREATE TABLE IF NOT EXISTS workspaces (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL REFERENCES agents(id),
  name TEXT,
  created_at TEXT,
  updated_at TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS workspace_bindings (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  folder_path TEXT NOT NULL,
  created_at TEXT,
  updated_at TEXT,
  last_validated_at TEXT
);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  role TEXT,
  seq INTEGER,
  status TEXT,                     -- pending|streaming|complete|failed|cancelled
  model TEXT,
  token_count INTEGER,
  metadata TEXT,
  parent_id TEXT,                  -- 父消息 id（版本树），NULL=根
  selected_child_id TEXT,          -- 当前活动路径选中的子消息 id，NULL=叶子
  created_at TEXT,
  updated_at TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS message_parts (
  id TEXT PRIMARY KEY,
  message_id TEXT NOT NULL REFERENCES messages(id),
  kind TEXT,                       -- text|tool_call|tool_result|reasoning
  ordinal INTEGER,
  mime_type TEXT,
  tool_call_id TEXT,
  content TEXT,
  metadata TEXT
);

CREATE TABLE IF NOT EXISTS memory_store (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL REFERENCES agents(id),
  name TEXT NOT NULL DEFAULT '',
  keywords TEXT,
  content TEXT,
  creator TEXT NOT NULL DEFAULT 'ai', -- user|ai, assigned by the trusted entry point
  type TEXT,
  scope TEXT,
  source TEXT,
  confidence REAL,
  status TEXT DEFAULT 'active',    -- active|archived|deleted
  expires_at TEXT,
  pinned INTEGER DEFAULT 0,
  source_message_id TEXT,
  last_used_at TEXT,
  created_at TEXT,
  updated_at TEXT,
  embedding_id TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS explicit_memories (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL REFERENCES agents(id),
  kind TEXT NOT NULL CHECK(kind IN ('user_md', 'memory_md')),
  content TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT,
  UNIQUE(agent_id, kind)
);

CREATE TABLE IF NOT EXISTS embedding_items (
  id TEXT PRIMARY KEY,
  ref_type TEXT,
  ref_id TEXT,
  collection_id TEXT,
  embedding_profile_id TEXT,
  model TEXT,
  dims INTEGER,
  content_hash TEXT,
  created_at TEXT
);
-- vec_embeddings_{dims} tables are created lazily for each embedding dimension.

CREATE TABLE IF NOT EXISTS calendars (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  color TEXT,
  timezone TEXT NOT NULL,
  provider_account_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS calendar_events (
  id TEXT PRIMARY KEY,
  calendar_id TEXT NOT NULL REFERENCES calendars(id),
  title TEXT NOT NULL,
  description TEXT,
  location TEXT,
  starts_at TEXT NOT NULL,
  ends_at TEXT NOT NULL,
  timezone TEXT NOT NULL,
  all_day INTEGER NOT NULL DEFAULT 0 CHECK(all_day IN (0, 1)),
  recurrence_rule TEXT,
  recurrence_id TEXT,
  status TEXT NOT NULL DEFAULT 'confirmed' CHECK(status IN ('confirmed', 'tentative', 'cancelled')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS event_exceptions (
  event_id TEXT NOT NULL REFERENCES calendar_events(id) ON DELETE CASCADE,
  original_occurrence TEXT NOT NULL,
  replacement_event_id TEXT REFERENCES calendar_events(id),
  is_cancelled INTEGER NOT NULL DEFAULT 0 CHECK(is_cancelled IN (0, 1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT,
  PRIMARY KEY(event_id, original_occurrence)
);

CREATE TABLE IF NOT EXISTS task_lists (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  color TEXT,
  provider_account_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY,
  task_list_id TEXT NOT NULL REFERENCES task_lists(id),
  parent_id TEXT REFERENCES tasks(id),
  title TEXT NOT NULL,
  description TEXT,
  status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open', 'completed', 'cancelled')),
  priority INTEGER NOT NULL DEFAULT 0 CHECK(priority BETWEEN 0 AND 4),
  starts_at TEXT,
  due_date TEXT,
  due_at TEXT,
  due_timezone TEXT,
  is_important INTEGER NOT NULL DEFAULT 0 CHECK(is_important IN (0, 1)),
  my_day_date TEXT,
  completed_at TEXT,
  recurrence_rule TEXT,
  recurrence_anchor TEXT,
  recurrence_source_id TEXT REFERENCES tasks(id),
  sort_order REAL NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_calendar_events_range
  ON calendar_events(calendar_id, starts_at, ends_at) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_calendar_events_recurrence
  ON calendar_events(recurrence_id) WHERE recurrence_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tasks_list_status_due
  ON tasks(task_list_id, status, due_at, sort_order) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_tasks_parent
  ON tasks(parent_id, sort_order) WHERE parent_id IS NOT NULL AND deleted_at IS NULL;

-- Keeps a remote task's requested parent until that parent is present locally.
-- The Planner payload is encrypted, so bootstrap cannot sort task rows by it.
CREATE TABLE IF NOT EXISTS task_sync_parents (
  task_id TEXT PRIMARY KEY,
  parent_id TEXT
);

CREATE TABLE IF NOT EXISTS tool_calls (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  message_id TEXT REFERENCES messages(id),
  tool TEXT,
  params TEXT,
  result TEXT,
  status TEXT,                     -- pending_approval|running|done|rejected|failed|cancelled
  risk_level TEXT,
  cwd TEXT,
  exit_code INTEGER,
  stdout TEXT,
  stderr TEXT,
  started_at TEXT,
  completed_at TEXT,
  approval_policy_snapshot TEXT,
  created_at TEXT
);

-- Notifications are intentionally device-local. They are derived from local agent
-- activity and scheduled planner data, so they do not participate in D1 sync.
CREATE TABLE IF NOT EXISTS notifications (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('agent_completed', 'approval_requested', 'task_due', 'event_start')),
  title TEXT NOT NULL,
  body TEXT,
  target_kind TEXT NOT NULL CHECK(target_kind IN ('chat', 'task', 'calendar', 'none')),
  target_id TEXT,
  source_kind TEXT NOT NULL,
  source_id TEXT NOT NULL,
  dedupe_key TEXT NOT NULL UNIQUE,
  scheduled_at TEXT,
  delivered_at TEXT NOT NULL,
  read_at TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notifications_inbox
  ON notifications(read_at, delivered_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_notifications_source
  ON notifications(source_kind, source_id);

CREATE TABLE IF NOT EXISTS sync_log (
  id TEXT PRIMARY KEY,
  device_id TEXT,
  entity_type TEXT,
  entity_id TEXT,
  operation TEXT,
  payload TEXT,
  payload_hash TEXT,
  entity_version INTEGER,
  created_at TEXT,
  hlc TEXT,
  synced_at TEXT
);

CREATE TABLE IF NOT EXISTS sync_outbox (
  change_id TEXT PRIMARY KEY,
  device_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  operation TEXT NOT NULL CHECK(operation IN ('upsert', 'delete')),
  base_revision INTEGER,
  local_version INTEGER NOT NULL,
  hlc TEXT NOT NULL,
  payload_schema_version INTEGER NOT NULL DEFAULT 1,
  payload_encoding TEXT NOT NULL DEFAULT 'json',
  payload TEXT,
  source_payload TEXT,
  payload_hash TEXT NOT NULL,
  key_version INTEGER,
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK(status IN ('pending', 'in_flight', 'synced', 'conflict', 'dead_letter')),
  attempt_count INTEGER NOT NULL DEFAULT 0,
  next_retry_at INTEGER,
  last_error_code TEXT,
  last_error_message TEXT,
  created_at INTEGER NOT NULL,
  synced_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sync_outbox_ready
  ON sync_outbox(status, next_retry_at, created_at, change_id);

CREATE INDEX IF NOT EXISTS idx_sync_outbox_entity
  ON sync_outbox(entity_type, entity_id, status, created_at, change_id);

CREATE TABLE IF NOT EXISTS sync_entity_state (
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  remote_revision INTEGER,
  last_server_seq INTEGER,
  last_payload_hash TEXT,
  last_synced_hlc TEXT,
  base_payload TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(entity_type, entity_id)
);

CREATE TABLE IF NOT EXISTS sync_conflicts (
  id TEXT PRIMARY KEY,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  base_revision INTEGER,
  remote_revision INTEGER,
  base_payload TEXT,
  local_payload TEXT,
  remote_payload TEXT,
  local_deleted INTEGER NOT NULL DEFAULT 0 CHECK(local_deleted IN (0, 1)),
  remote_deleted INTEGER NOT NULL DEFAULT 0 CHECK(remote_deleted IN (0, 1)),
  remote_ready INTEGER NOT NULL DEFAULT 0 CHECK(remote_ready IN (0, 1)),
  local_version INTEGER NOT NULL,
  local_hlc TEXT NOT NULL,
  remote_hlc TEXT,
  remote_payload_hash TEXT,
  remote_origin_device_id TEXT,
  remote_server_seq INTEGER,
  remote_updated_at INTEGER,
  conflicting_fields TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'resolved')),
  resolution TEXT CHECK(resolution IN ('auto_merge', 'keep_local', 'keep_remote')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  resolved_at INTEGER
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_conflicts_pending_entity
  ON sync_conflicts(entity_type, entity_id) WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_sync_conflicts_status
  ON sync_conflicts(status, updated_at DESC, id);

CREATE TABLE IF NOT EXISTS sync_runtime_state (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  device_id TEXT NOT NULL UNIQUE,
  last_hlc TEXT,
  last_pull_cursor INTEGER NOT NULL DEFAULT 0,
  bootstrap_state TEXT NOT NULL DEFAULT 'required',
  last_success_at INTEGER,
  last_error_code TEXT,
  backoff_until INTEGER,
  e2ee_key_version INTEGER
);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT
);

CREATE TABLE IF NOT EXISTS model_providers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  api_base TEXT,
  is_default INTEGER DEFAULT 0,
  models_json TEXT,
  extra_config TEXT,
  created_at TEXT,
  updated_at TEXT
);
"#;

/// Knowledge base tables are kept separate so legacy placeholder document
/// tables can be renamed before this schema is installed.
pub const KNOWLEDGE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_collections (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  scope TEXT NOT NULL CHECK(scope IN ('user_global', 'workspace', 'agent_private', 'custom')),
  workspace_id TEXT REFERENCES workspaces(id),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS collection_agents (
  collection_id TEXT NOT NULL REFERENCES knowledge_collections(id) ON DELETE CASCADE,
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  permission TEXT NOT NULL CHECK(permission IN ('read', 'write', 'manage')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(collection_id, agent_id)
);

CREATE TABLE IF NOT EXISTS documents (
  id TEXT PRIMARY KEY,
  collection_id TEXT NOT NULL REFERENCES knowledge_collections(id),
  title TEXT NOT NULL,
  media_type TEXT NOT NULL,
  current_version_id TEXT,
  status TEXT NOT NULL CHECK(status IN ('active', 'missing', 'error', 'deleted')) DEFAULT 'active',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
);

CREATE TABLE IF NOT EXISTS document_sources (
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  provider_account_id TEXT,
  source_kind TEXT NOT NULL,
  encrypted_locator TEXT,
  provider_revision TEXT,
  observed_at TEXT NOT NULL,
  local_binding_id TEXT
);

CREATE TABLE IF NOT EXISTS document_local_bindings (
  source_id TEXT PRIMARY KEY REFERENCES document_sources(id) ON DELETE CASCADE,
  local_path TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS document_versions (
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  logical_version INTEGER NOT NULL,
  plaintext_hash TEXT NOT NULL,
  size INTEGER NOT NULL,
  media_type TEXT NOT NULL,
  parser_profile_id TEXT,
  created_at TEXT NOT NULL,
  origin_device_id TEXT,
  UNIQUE(document_id, logical_version)
);

CREATE TABLE IF NOT EXISTS document_chunks (
  id TEXT PRIMARY KEY,
  document_version_id TEXT NOT NULL REFERENCES document_versions(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL,
  content TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  page INTEGER,
  section_path TEXT,
  token_count INTEGER NOT NULL,
  metadata TEXT NOT NULL DEFAULT '{}',
  UNIQUE(document_version_id, ordinal)
);

CREATE VIRTUAL TABLE IF NOT EXISTS document_chunks_fts USING fts5(
  chunk_id UNINDEXED,
  document_id UNINDEXED,
  document_version_id UNINDEXED,
  title,
  content,
  tokenize='unicode61'
);

CREATE TABLE IF NOT EXISTS embedding_profiles (
  id TEXT PRIMARY KEY,
  model_ref TEXT NOT NULL,
  model_revision TEXT,
  dims INTEGER NOT NULL,
  normalized INTEGER NOT NULL DEFAULT 1,
  instruction_hash TEXT NOT NULL,
  tokenizer_ref TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS parser_profiles (
  id TEXT PRIMARY KEY,
  parser_name TEXT NOT NULL,
  parser_version TEXT NOT NULL,
  options_hash TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunker_profiles (
  id TEXT PRIMARY KEY,
  chunker_name TEXT NOT NULL,
  chunker_version TEXT NOT NULL,
  chunk_size INTEGER NOT NULL,
  overlap INTEGER NOT NULL,
  options_hash TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_collection_agents_agent
  ON collection_agents(agent_id, collection_id);
CREATE INDEX IF NOT EXISTS idx_documents_collection
  ON documents(collection_id, status, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_document_sources_document
  ON document_sources(document_id, observed_at DESC);
CREATE INDEX IF NOT EXISTS idx_document_versions_document
  ON document_versions(document_id, logical_version DESC);
CREATE INDEX IF NOT EXISTS idx_document_chunks_version
  ON document_chunks(document_version_id, ordinal);
CREATE INDEX IF NOT EXISTS idx_embedding_items_collection_profile
  ON embedding_items(collection_id, embedding_profile_id, ref_type, ref_id);
"#;
