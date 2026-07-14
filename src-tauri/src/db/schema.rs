//! 建表 DDL（rusqlite / SQLite）。
//! 注意：向量与元数据分离 —— `embedding_items` 存元数据，`vec_embeddings` 为 sqlite-vec 虚拟表（V0.2 建）。

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
  created_at TEXT,
  updated_at TEXT
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
  summary TEXT,                    -- 会话级状态，非普通消息片段
  summary_updated_at TEXT,
  created_at TEXT,
  updated_at TEXT,
  version INTEGER DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
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
  created_at TEXT,
  updated_at TEXT
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
  content TEXT,
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
  version INTEGER DEFAULT 1,
  deleted_at TEXT,
  origin_device_id TEXT
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
CREATE VIRTUAL TABLE IF NOT EXISTS vec_embeddings USING vec0(
  embedding_id TEXT PRIMARY KEY,
  vector float[1536]
);

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

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT
);
"#;
