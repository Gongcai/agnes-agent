//! 建表 DDL（rusqlite / SQLite）。
//! 注意：向量与元数据分离 —— `embedding_items` 存元数据，`vec_embeddings_{dims}` 为按维度延迟创建的 sqlite-vec 虚拟表。

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
  model TEXT,
  dims INTEGER,
  content_hash TEXT,
  created_at TEXT
);
-- vec_embeddings_{dims} tables are created lazily for each embedding dimension.

CREATE TABLE IF NOT EXISTS documents (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL REFERENCES agents(id),
  title TEXT,
  path TEXT,
  created_at TEXT
);

CREATE TABLE IF NOT EXISTS document_chunks (
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL REFERENCES documents(id),
  content TEXT,
  embedding_id TEXT
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
