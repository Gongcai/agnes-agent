# Cloudflare D1 云同步设计

> 状态：设计完成，Phase 0、Phase 1、Phase 2 与 Phase 3A 已完成
> 调研日期：2026-07-16  
> 适用版本：V0.3 Cloudflare 同步、V0.4 Android 轻客户端  
> 关联文档：`PROJECT.md`、`architecture.md`、`UI_DESIGN.md`、`STORAGE_AND_RAG.md`

## 1. 结论

`agnes-agent` 使用 Cloudflare 的推荐组合为：

- **Cloudflare Workers**：唯一对外的 HTTPS 同步 API；
- **D1**：保存实体当前快照、增量变更日志和设备同步状态；
- **Hono**：Worker 的轻量路由与中间件；
- **D1 prepared statements / batch**：同步热路径的数据访问；Drizzle 用于 schema、类型和普通查询；
- **R2（后续启用）**：存放头像、附件、源文件、加密备份和加密向量制品等大对象；
- **Cloudflare Access Service Token（可选）**：个人部署时为每台设备提供可吊销的机器凭证。

D1 **不替换**桌面端和 Android 端的本地 SQLite。系统继续坚持 local-first：

1. 本地 SQLite 是设备上的运行时真相源；
2. 本地写入成功即视为业务成功，不等待云端；
3. Rust 在后台将事务性 outbox 推送到 Worker，并按游标拉取其他设备的变更；
4. Worker 或 D1 不可用、网络断开、免费额度耗尽时，只影响同步，不影响聊天、记忆和工具执行；
5. React 只展示同步状态，Python sidecar 不直接访问同步 API 或云数据库。

不采用以下方案：

- Tauri 客户端直连 D1 REST API；这会迫使客户端持有 Cloudflare 管理 API Token；
- 用 D1 替换本地 SQLite；这会破坏离线能力，并让 Agent 执行依赖网络；
- 用 KV 作为同步真相源；KV 是最终一致模型，不适合事务和冲突检测；
- 首版引入 Durable Objects；个人使用量下 D1 已足够，Durable Objects 会增加状态模型和运维复杂度；
- 将本地 SQLite 文件持续上传覆盖；该方式只能做备份，无法可靠完成多设备增量合并。

---

## 2. 目标与非目标

### 2.1 目标

- 桌面端与后续 Android 端同步角色卡、会话、聊天文本和长期记忆；
- 支持离线写入、断线重试、重复请求和乱序响应；
- 原始向量行不进 D1，不上传模型密钥、本地绝对路径和原始工具审计数据；加密向量制品作为大对象使用独立 manifest 协议；
- 支持新增设备 bootstrap，不要求从第一条历史日志开始重放；
- 支持软删除、设备撤销、冲突保留和同步状态可观测；
- 云端 schema 与本地业务表解耦，避免每次本地迁移都同步修改 Worker 协议；
- 为端到端加密预留完整的数据模型，不把加密能力拖到无法兼容的后期；
- 在 Cloudflare Workers/D1 免费层内满足个人长期使用。

### 2.2 非目标

- 云端运行 Python Agent、LangGraph、shell、Git 或 MCP；
- 将 embedding/sqlite-vec 原始行复制到 D1，或在 Worker 内直接运行未加密向量检索；
- 多人实时协同编辑；
- 首版提供 Web 聊天客户端；
- 在 Worker 内执行复杂字段合并、LLM 总结或重新生成 embedding；
- 把 D1 当作可由任意客户端执行 SQL 的公共数据库。

---

## 3. 总体架构

```text
┌─────────────────────────────────────────────┐
│ Tauri Desktop / Android                     │
│                                             │
│ React UI                                    │
│   └─ 只显示同步状态、设备列表、冲突提示      │
│                                             │
│ Rust Execution + Data Plane                 │
│   ├─ Local SQLite（运行时真相源）            │
│   ├─ sync_outbox（事务性待推送队列）         │
│   ├─ SyncEngine（push / pull / bootstrap）   │
│   ├─ MergeEngine（类型化冲突处理）           │
│   ├─ Crypto（可选 E2EE）                    │
│   └─ OS Keyring（同步凭证 / 加密主密钥）     │
│                                             │
│ Python Reasoning Plane                      │
│   └─ 不参与云同步                           │
└──────────────────────┬──────────────────────┘
                       │ HTTPS JSON API
                       ▼
┌─────────────────────────────────────────────┐
│ Cloudflare Worker                           │
│   ├─ Auth                                   │
│   ├─ Hono routes                            │
│   ├─ 请求校验 / 限流 / 幂等                 │
│   └─ D1 binding                             │
└──────────────────────┬──────────────────────┘
                       ▼
┌─────────────────────────────────────────────┐
│ D1                                          │
│   ├─ devices                                │
│   ├─ sync_entities（最新实体快照）           │
│   ├─ sync_changes（append-only 变更流）      │
│   └─ sync_acks（设备游标 / 压缩依据）        │
└─────────────────────────────────────────────┘
```

同步逻辑属于 Rust Data Plane。建议未来新增：

```text
src-tauri/src/sync/
  mod.rs
  client.rs        # Worker HTTP client
  engine.rs        # 调度、push、pull、bootstrap
  outbox.rs        # 本地事务性变更队列
  merge.rs         # 类型化合并和冲突策略
  hlc.rs           # Hybrid Logical Clock
  crypto.rs        # E2EE envelope（启用时）
  types.rs         # Sync Protocol DTO
```

Worker 建议作为独立 pnpm workspace package：

```text
workers/sync-api/
  package.json
  wrangler.jsonc
  src/index.ts
  src/auth.ts
  src/routes/sync.ts
  src/db/schema.ts
  src/protocol.ts
  migrations/
  test/
```

根 `pnpm-workspace.yaml` 后续纳入 `workers/*`，但 Worker 和 React 仍是两个独立构建目标。

---

## 4. Cloudflare 产品边界

| 产品 | 本项目用途 | 首版是否使用 | 原因 |
|---|---|---:|---|
| Workers | 同步 API、认证、校验、D1 访问 | 是 | D1 binding 只应由受控服务访问 |
| D1 | 快照、变更流、设备状态 | 是 | SQLite 语义、事务、索引、按量免费层 |
| Hono | HTTP 路由 | 是 | 轻量，符合现有规划 |
| Drizzle | D1 schema、类型、迁移辅助 | 是 | 避免手写重复类型；热路径仍可直接 prepared statement |
| KV | 静态配置或极少更新的缓存 | 否 | 最终一致，不适合事务同步 |
| Durable Objects | 单实体强串行协调 | 否 | 当前 D1 单库已能满足个人使用并发 |
| R2 | 头像、附件、源文件、加密导出和向量制品 | 后续 | 大对象不应进入 D1 变更行；客户端先 E2EE |
| Vectorize | 云向量检索 | 否 | 向量在本地 sqlite-vec 检索；加密制品只作为可下载大对象 |
| Workers AI | 云端 embedding | 否 | 首版不让云端参与记忆索引 |

D1 的云端 schema 不复用 `src-tauri/src/db/schema.rs`。本地库包含 `sqlite-vec`、工具审计、本地路径和运行时状态，这些不属于云同步数据库；D1 需要独立、版本化的 SQL migrations。

---

## 5. 数据分类与同步白名单

同步必须采用**实体类型白名单**，不得以“同步整个表”或“同步所有 settings key”的方式实现。

### 5.1 同步范围

| 本地数据 | 是否同步 | 云端实体 | 说明 |
|---|---:|---|---|
| Agent 角色卡 | 是 | `agent` | persona、scenario、system prompt、模型偏好、工具策略等；文本建议加密 |
| Session | 是 | `session` | 标题、摘要、上下文设置、分支视图状态和逻辑 workspace 归属；不携带设备路径 |
| 完成的 user/assistant message | 是 | `message` | 按 revision CAS；`pending/streaming` 不推送，并发回复使用不同 message ID |
| message text parts | 是 | 包含于 `message` payload | 小实体整体同步，避免 parts 到达早于 message |
| reasoning part | 默认否 | — | 可能包含敏感推理或供应商不允许持久化的内容 |
| tool call/result part | 默认仅同步脱敏摘要 | `message` 可选字段 | 原始参数和结果可能含文件、路径、令牌和命令输出 |
| USER.md / MEMORY.md | 是 | `explicit_memory` | 从通用 settings 中独立为明确实体 |
| memory_store 文本 | 是 | `memory` | 不包含 embedding_id 的设备本地含义 |
| embedding_items / vec_embeddings_{dims} | 不进 D1 | — | 每台设备可本地重算；大型 RAG 可下载指纹匹配的加密制品 |
| model_providers | 首版否 | — | Ollama/API Base 等通常是设备配置 |
| provider API Key | 绝不 | — | 必须迁移至 OS Keyring |
| UI 最近选中的 Agent/Session | 否 | — | 设备本地体验状态 |
| workspace 逻辑信息 | 后续可同步 | `workspace` | 只同步 id/name/agent_id 等逻辑元数据 |
| workspace folder_path | 绝不 | — | 设备本地绝对路径 |
| tool_calls 审计、stdout/stderr | 默认否 | — | 体积大且高敏感 |
| knowledge collection / document metadata | 后续是 | `collection` / `document` | 只同步逻辑元数据、版本和墓碑 |
| source/chunk/vector artifact 密文 | 不进 D1 | `object_manifest` | R2/Google Drive 保存密文；D1 保存版本、Hash、replica 和 device state |
| avatar / attachment | 后续 R2 | `blob_ref` | D1 只同步对象引用与摘要 |
| 同步凭证、E2EE 主密钥 | 绝不 | — | OS Keyring / Android Keystore |

### 5.2 Workspace 拆分

Phase 0 前的 `workspaces.folder_path` 同时表达逻辑工作区和设备路径，不适合多设备；当前已拆分为：

```text
workspace                 # 可选同步
  id
  agent_id
  name
  repository_identity?    # 可选，不含凭证

workspace_binding         # 仅本地
  workspace_id
  folder_path
  last_validated_at
```

每台设备各自拥有独立的本地 SQLite，因此当前实现以 `workspace_id` 作为本机绑定主键，
设备作用域由数据库实例天然隔离；云端 workspace payload 不包含 binding。同步到新设备但
尚未绑定目录的 workspace 可以展示逻辑信息，但工具执行不得把空路径当作 cwd。

Android 可以看到某个 session 属于哪个逻辑 workspace，但没有本地目录时不得尝试执行文件工具。

Phase 3A 已将逻辑 `workspace_id` 纳入 Session payload，同时继续排除
`workspace_bindings.folder_path / last_validated_at`。Workspace 创建、重命名、删除与 outbox
在同一事务提交；远端 Workspace 只创建逻辑行，不会在新设备伪造路径绑定。bootstrap 按
`agent -> workspace -> session -> message -> explicit_memory -> memory` 排序，确保分页应用时
本地 SQLite 外键依赖先到达。

### 5.3 Settings 分类

`settings(key, value)` 目前混合了三类完全不同的数据，必须显式拆分或分类：

1. **可同步业务数据**：例如每个 Agent 的 USER.md / MEMORY.md；建议迁为独立实体；
2. **设备本地设置**：`ui:last_agent_id`、`ui:last_session_id`、窗口状态、本地模型端点；
3. **秘密**：`provider:{id}:api_key`、同步认证凭证、加密密钥。

同步引擎只接受由实体仓库显式生成的 payload，不允许扫描 `settings` 后按前缀猜测是否上传。

---

## 6. 同步实体模型

云端不复制本地每张业务表，而使用版本化实体 envelope。这样可以：

- 保持本地 schema 和 D1 schema 解耦；
- 让一次 message 与其 text parts 原子到达；
- 允许 payload 在客户端整体加密；
- 用统一的幂等、版本、墓碑和游标逻辑处理所有实体；
- 后续增加实体类型时不必重写同步核心表。

### 6.1 实体 envelope

概念结构如下：

```json
{
  "protocolVersion": 1,
  "changeId": "uuid",
  "deviceId": "uuid",
  "entityType": "message",
  "entityId": "stable-id",
  "operation": "upsert",
  "baseRevision": 12,
  "hlc": "1784188800123-0004-device-short-id",
  "payloadSchemaVersion": 1,
  "payloadEncoding": "json",
  "payload": "base64-or-json",
  "payloadHash": "sha256",
  "keyVersion": null,
  "createdAt": 1784188800123
}
```

`changeId` 和 `deviceId` 必须是 UUID；`entityId` 是最多 128 字符的稳定安全标识，推荐
使用 UUID，同时兼容内置角色的历史 ID（`agnes/nova/bard`），不得包含路径分隔符或空白。

启用 E2EE 后：

- `payloadEncoding` 改为 `xchacha20poly1305` 等确定的协议值；
- `payload` 为密文；
- envelope 中影响授权、路由和冲突判断的字段作为 AEAD associated data；
- D1 仍可看到 owner、设备、实体类型、实体 ID、版本和时间元数据，但看不到业务内容；
- `payloadHash` 对密文计算，用于传输校验和幂等比较，内容完整性由 AEAD tag 保证。

### 6.2 Payload 版本

`protocolVersion` 和 `payloadSchemaVersion` 必须分离：

- `protocolVersion`：push/pull HTTP 协议和 envelope 结构版本；
- `payloadSchemaVersion`：某一实体 JSON 内容版本。

Rust 读取旧 payload 时负责升级到当前内存结构。Worker 不解析加密 payload，也不负责业务 payload 迁移。

---

## 7. D1 概念 schema

以下 schema 用于固定职责，不是本轮要直接执行的最终 migration。

### 7.1 devices

```sql
CREATE TABLE devices (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL,
  name TEXT NOT NULL,
  platform TEXT,
  credential_fingerprint TEXT,
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER,
  revoked_at INTEGER
);

CREATE INDEX idx_devices_owner
  ON devices(owner_id, revoked_at);
```

`owner_id` 由 Worker 根据已验证凭证决定，不接受客户端任意指定。

### 7.2 sync_entities

保存每个实体的最新云端快照，供新设备 bootstrap，避免重放无限历史。

```sql
CREATE TABLE sync_entities (
  owner_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  hlc TEXT NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  payload_schema_version INTEGER NOT NULL,
  payload_encoding TEXT NOT NULL,
  payload BLOB,
  payload_hash TEXT NOT NULL,
  key_version INTEGER,
  changed_by_device_id TEXT NOT NULL,
  latest_server_seq INTEGER NOT NULL,
  latest_change_id TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(owner_id, entity_type, entity_id)
);

CREATE INDEX idx_sync_entities_bootstrap
  ON sync_entities(owner_id, entity_type, entity_id, latest_server_seq);
```

`latest_change_id` 是 Worker 内部的原子写入标记，不进入客户端协议。push 先用条件
upsert 将快照指向当前 change，再用 `INSERT ... SELECT` 只为该标记匹配的快照写入
change；`AFTER INSERT` trigger 将生成的 `server_seq` 回写到快照。整个批次由 D1
`batch()` 原子执行，因此 CAS 失败时不会产生孤立 change，日志插入失败时也不会留下
只有快照没有游标的半状态。

### 7.3 sync_changes

append-only 变更流，为增量 pull 提供全局递增游标。

```sql
CREATE TABLE sync_changes (
  server_seq INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_id TEXT NOT NULL,
  change_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  base_revision INTEGER,
  resulting_revision INTEGER NOT NULL,
  hlc TEXT NOT NULL,
  payload_schema_version INTEGER NOT NULL,
  payload_encoding TEXT NOT NULL,
  payload BLOB,
  payload_hash TEXT NOT NULL,
  key_version INTEGER,
  created_at INTEGER NOT NULL,
  accepted_at INTEGER NOT NULL,
  UNIQUE(owner_id, change_id)
);

CREATE INDEX idx_sync_changes_pull
  ON sync_changes(owner_id, server_seq);

CREATE INDEX idx_sync_changes_entity
  ON sync_changes(owner_id, entity_type, entity_id, server_seq);
```

### 7.4 sync_acks

```sql
CREATE TABLE sync_acks (
  owner_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  last_server_seq INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(owner_id, device_id)
);
```

该表用于了解活跃设备已消费到的位置，并为变更日志压缩提供依据。撤销或长期离线设备不应永久阻止压缩。

### 7.5 索引原则

D1 免费层按扫描行数计量，因此所有常用过滤条件必须由索引覆盖：

- `sync_changes(owner_id, server_seq)`：pull 热路径；
- `sync_entities(owner_id, entity_type, latest_server_seq)`：bootstrap；
- `UNIQUE(owner_id, change_id)`：push 幂等；
- 实体主键：CAS 和快照更新。

索引自身会增加写入计量，应只建立由明确查询驱动的索引，不为潜在需求提前创建大量索引。

---

## 8. Worker API 契约

所有 API 使用 `/v1` 前缀、HTTPS、JSON，并返回稳定错误码。Worker 不提供任意 SQL 接口。

### 8.1 `GET /v1/health`

用途：检查 Worker、认证和 D1 binding 是否可用。

返回内容只包含服务版本、协议版本和服务器时间，不返回数据库内部信息。

### 8.2 `POST /v1/sync/push`

请求：

```json
{
  "protocolVersion": 1,
  "deviceId": "uuid",
  "changes": []
}
```

响应：

```json
{
  "accepted": [
    {
      "changeId": "uuid",
      "serverSeq": 101,
      "revision": 13
    }
  ],
  "conflicts": [],
  "serverTime": 1784188800000
}
```

规则：

- 单批建议最多 20 个 change，且解码前请求体设置应用级体积上限；
- 同一 `(owner_id, change_id)` 重复提交必须返回第一次结果，不重复写入；
- Message 创建、正文编辑和 `selected_child_id` 更新使用 `baseRevision` CAS；并发回复必须使用不同 message ID；
- mutable 实体使用 `baseRevision` 做 compare-and-swap；
- 接受变更时，插入 `sync_changes` 与更新 `sync_entities` 必须处于同一个 D1 batch/事务；
- Worker 不依据客户端传入的 owner_id 做授权；
- 失败返回逐项可判断的错误，不以 200 + 模糊 message 隐藏冲突。

批量 push 可能同时包含成功项和 CAS 冲突项，因此合法批次使用 200 返回
`accepted[]` 与 `conflicts[]`；每个冲突项带稳定的 `reason=REVISION_CONFLICT` 和当前
revision。请求整体的认证、schema 或体积错误继续使用对应非 200 状态。

### 8.3 `GET /v1/sync/pull?after={serverSeq}&limit={n}`

响应：

```json
{
  "changes": [],
  "nextCursor": 140,
  "hasMore": false,
  "serverTime": 1784188800000
}
```

规则：

- 只返回当前已认证 owner 的数据；
- `after` 为客户端已成功落地的最后 `server_seq`；
- 建议默认 100、最大 500；同时受响应体应用级上限约束；
- 客户端必须先在本地事务中应用整页，再持久化新 cursor；
- 响应丢失时允许用旧 cursor 重拉，应用过程必须幂等。

### 8.4 `GET /v1/sync/bootstrap`

用于首次启用同步、新设备加入或客户端 cursor 已落后于日志保留窗口。

- 分页读取 `sync_entities` 当前快照；
- 每页返回稳定 continuation token；
- bootstrap 开始时返回一个高水位 `snapshotCursor`；
- 快照完成后再从 `snapshotCursor` 调用 pull，避免快照期间发生的写入丢失；
- bootstrap 不覆盖本地未推送变更，必须进入 MergeEngine。

### 8.5 `POST /v1/sync/ack`

设备在成功持久化 pull cursor 后更新 `sync_acks`。Ack 丢失不影响正确性，只会延迟日志清理。

### 8.6 设备管理

- `GET /v1/devices`：返回当前认证 owner 的设备名、平台、创建/最后在线时间、撤销状态和 ack 游标；
- `POST /v1/devices/{deviceId}/revoke`：幂等撤销同 owner 的其他设备；禁止当前设备自我撤销；
- owner 身份只取自认证上下文，其他 owner 的设备按不存在处理，避免通过 ID 探测；
- token 指纹和凭证明文均不进入响应；被撤销设备后续所有 `/v1/*` 请求立即返回 `DEVICE_REVOKED`。

### 8.7 错误模型

建议稳定错误码：

| HTTP | code | 含义 | 客户端行为 |
|---:|---|---|---|
| 400 | `INVALID_REQUEST` | schema、大小或字段不合法 | 停止重试该 change，展示诊断 |
| 401 | `UNAUTHENTICATED` | 凭证缺失或失效 | 标记需重新认证 |
| 403 | `DEVICE_REVOKED` | 设备已撤销 | 清除同步会话，不清本地数据 |
| 200/409 | `REVISION_CONFLICT` | change 项的 baseRevision 已过期；批量接口放入 `conflicts[]` | 拉取远端版本并进入合并 |
| 413 | `PAYLOAD_TOO_LARGE` | 超过应用级限制 | 保持本地，提示改用 R2/不支持同步 |
| 429 | `RATE_LIMITED` | 请求过快 | 尊重 Retry-After |
| 503 | `SYNC_TEMPORARILY_UNAVAILABLE` | D1/Worker 暂时异常或免费额度耗尽 | 指数退避，本地业务继续 |

Cloudflare 平台错误可能不符合本项目 JSON 格式，Rust client 还需根据 HTTP 状态和响应类型兜底分类。

---

## 9. 本地同步状态

现有 `sync_log` 只有基础字段，建议演进为明确的 outbox，而不是仅作审计日志。

### 9.1 sync_outbox

概念字段：

```text
change_id              PK
device_id
entity_type
entity_id
operation
base_revision
local_version
hlc
payload_schema_version
payload_encoding
payload
payload_hash
key_version
status                 pending|in_flight|synced|conflict|dead_letter
attempt_count
next_retry_at
last_error_code
last_error_message
created_at
synced_at
```

业务实体写入和 outbox insert 必须在同一个本地 SQLite transaction 中完成。禁止“业务先提交，稍后扫描数据库补日志”，否则进程崩溃会产生永久漏同步窗口。

outbox 保存变更发生时的不可变 payload。不能在实际 push 时重新读取当前业务行代替旧 payload，否则会破坏 change 顺序和 baseRevision 语义。

### 9.2 sync_entity_state

不建议把云端 revision 强行混入所有本地业务表，可增加通用状态表：

```text
entity_type + entity_id    PK
remote_revision
last_server_seq
last_payload_hash
last_synced_hlc
base_payload?              # 三方合并所需，可压缩或按类型保留
updated_at
```

本地业务 `version` 与云端 `remote_revision` 是两个概念：

- 本地 version：设备离线期间也能递增；
- remote revision：Worker 接受变更后分配，用于 CAS。

### 9.3 sync_runtime_state

至少持久化：

- `device_id`：安装时生成，除非用户明确重置设备身份，否则保持稳定；
- `last_pull_cursor`；
- `bootstrap_state`：`required`、`cursor:<snapshot-server-seq>:<opaque-token>` 或 `complete`；
- `last_success_at`；
- `last_error_code`；
- `backoff_until`；
- `e2ee_key_version`。

认证凭证和 E2EE 主密钥不进入该表，存入 OS Keyring / Android Keystore。

---

## 10. Push / Pull 算法

### 10.1 本地写入

```text
BEGIN IMMEDIATE
  写入或更新业务实体
  递增本地 version / 写墓碑
  生成 HLC
  序列化并可选加密 payload
  INSERT sync_outbox(status = pending)
COMMIT
```

对远端 pull 的落地使用独立入口，并带 `origin = remote`，不得再次生成 outbox，否则会形成同步回声循环。

### 10.2 Push 调度

触发条件：

- 应用启动并完成本地数据库初始化；
- 本地变更后短时间 debounce；
- 网络恢复；
- 用户点击“立即同步”；
- 定时低频保底。

同一设备只允许一个 SyncEngine single-flight。流程：

1. 选择到期的 pending change；
2. 按本地创建顺序组批，不跨越存在依赖关系的 change；
3. 标记 in_flight；
4. 发送 push；
5. accepted：写 remote revision、server seq、synced_at；
6. conflict：进入 MergeEngine；
7. 临时错误：恢复 pending，指数退避并加入 jitter；
8. 永久错误：进入 dead_letter，UI 可见但不阻断其他 change。

### 10.3 Pull 调度

1. 从 `last_pull_cursor` 请求一页；
2. 校验 envelope、hash、owner/device 约束；
3. 解密并按 payloadSchemaVersion 升级；
4. 在单个本地事务中幂等应用整页；
5. 同一事务更新 `last_pull_cursor`；
6. 有更多页则继续，但每轮设置最大页数，避免占用 DB Actor 太久；
7. 最终异步发送 ack。

### 10.4 同步顺序

Worker 的 `server_seq` 只表示服务端接受顺序，不等于用户语义时间。业务顺序使用：

- message 版本树：`parent_id`；
- 并发同级消息：`hlc + entity_id` 确定稳定展示顺序；
- mutable entity：remote revision + MergeEngine；
- UI 不依赖跨设备唯一的连续 `seq`。

当前消息树已经有 `parent_id / selected_child_id`，这比纯线性 `seq` 更适合并发设备。后续应把 `seq` 视为本地展示缓存，而不是跨设备一致性的唯一依据。

---

## 11. 冲突策略

Worker 只负责认证、幂等、CAS 和保存冲突所需版本，不解析或合并业务文本。类型化冲突处理位于 Rust。

### 11.1 总则

- 不使用客户端物理时间单独决定胜负；客户端时钟可能漂移；
- HLC 用于因果排序和稳定 tie-break，不替代 remote revision；
- 对可能丢失重要文本的数据，宁可保留冲突副本，不静默覆盖；
- 冲突解决本身产生新的本地 change，并以最新 remote revision 再 push；
- 所有自动合并必须是确定性的，相同输入在不同设备得到相同结果。

### 11.2 按实体策略

| 实体 | 策略 |
|---|---|
| `message` | 完成态快照使用 CAS；并发回复使用不同 ID，自然成为同一 parent 下的分支；同 ID 冲突保留 local/remote 基线，不静默覆盖正文 |
| `session` | 标题、pin、模型偏好可按字段三方合并；同字段冲突用 HLC tie-break，并记录 conflict event |
| `session.summary` | 不能简单 LWW；保留较新消息覆盖范围对应的摘要，必要时标记本地重新生成 |
| `agent` | 非重叠字段三方合并；persona/system_prompt 同字段冲突保留双方版本并提示选择 |
| `explicit_memory` | 不静默 LWW；保留 base/local/remote，尝试文本三方合并，失败则生成冲突副本 |
| `memory` | 不同 ID 自然并存；同一 ID 修改冲突保留两个 revision，用户或后续去重流程处理 |
| `workspace` | 只合并逻辑元数据；folder binding 从不进入冲突系统 |
| 设备本地 settings | 不同步，无冲突 |

### 11.3 selected_child_id

`selected_child_id` 表示当前消息选择了哪条子分支，它不是正文。当前随父 Message 快照使用
revision CAS；它的冲突不会删除任何消息，只改变默认展示分支。进入 Phase 3C 后可在确认
base/local/remote 仅该字段不同时使用 HLC 确定默认选中项，正文有变化时仍保留显式冲突。

### 11.4 流式消息

assistant 消息处于 `pending` 或 `streaming` 时不进入 outbox。完成时将 message 与全部可同步 parts 作为一个实体生成 change。

若运行失败：

- `failed/cancelled` 状态是否同步由产品层决定；
- 不同步半截 token；
- 若需要跨设备展示失败记录，推送一个最终、不可变的失败 message payload。

---

## 12. 删除、墓碑与日志压缩

### 12.1 删除规则

可同步实体禁止立即硬删除：

1. 本地写 `deleted_at` 和新 version；
2. outbox 生成 `operation = delete`；
3. Worker 写入 delete change，并在 `sync_entities` 保留 `deleted = 1` 的墓碑；
4. 其他设备 pull 后做本地软删除；
5. 达到保留期且所有活跃设备已越过相关 cursor 后，才允许物理清理。

删除 Agent 是多实体操作。不能依赖云端外键 cascade 隐式生成变更；Rust 应明确为 Agent 及需要删除的下属实体产生可重放的 tombstone，或定义“Agent 墓碑覆盖子实体可见性”的协议规则。

### 12.2 变更日志保留

建议初始策略：

- `sync_entities` 长期保存最新快照和墓碑；
- `sync_changes` 至少保留 30 天；
- 已被所有活跃设备 ack 且超过保留期的 change 可分批删除；
- 超过一定时间未同步的设备标记 stale，不阻塞压缩；它重新上线时强制 bootstrap；
- 墓碑保留期长于普通 change，避免旧设备复活已删除记录。

免费 Worker 的 Cron Trigger CPU 时间同样较小。压缩应采用小批次、可恢复的任务，或在正常请求中低频机会式执行，不能单次扫描/删除大量行。

---

## 13. 认证与设备管理

### 13.1 安全边界

- Cloudflare Account API Token 只用于部署/CI，不进入客户端；
- Worker 根据已验证凭证映射 owner 和 device；
- 客户端传入的 `ownerId` 不可信，甚至可不出现在请求中；
- 每台设备使用独立凭证，以便单独撤销；
- 同步凭证只存 OS Keyring / Android Keystore；
- 日志不得记录认证 header、payload 明文或解密密钥。

### 13.2 方案 A：Cloudflare Access Service Token

个人部署优先评估该方案：

- 每台设备创建独立 Client ID + Client Secret；
- 请求带 `CF-Access-Client-Id` 和 `CF-Access-Client-Secret`；
- token 可设置过期时间并在 Cloudflare Dashboard 单独撤销；
- Worker 仍应检查应用级 device id，不能只依赖“通过 Access”这一事实；
- 凭证初始化需要安全的人工配置或配对流程，不能编译到应用。

限制：增加 Cloudflare Zero Trust 配置依赖；在正式采用前需要用目标域名验证 Access 与 Tauri/Android HTTP client 的实际接入流程。

### 13.3 方案 B：Worker 自管 Device Token

如果不引入 Access：

- 设备注册时生成 256-bit 随机 token；
- Worker/D1 只保存 token hash/fingerprint，不保存明文；
- 明文只在注册时返回一次并进入 Keyring；
- 新设备注册由一次性 pairing code 或 owner bootstrap secret 授权；
- 支持过期、轮换、撤销和速率限制。

该方案减少外部产品依赖，但认证、注册和防暴力尝试需要自行实现和测试。

### 13.4 当前决策

V0.3 原计划优先采用 **Access Service Token per device**，同时让 Rust 认证抽象不绑定具体 header。
2026-07-16 验证时，目标 Cloudflare 账户没有任何 Zone，Wrangler OAuth 也没有 Access 管理权限，
无法给 `workers.dev` 部署建立符合安全边界的自定义域名 Access 应用。因此正式回退为
**Worker 自管 Device Token**，保留 Cloudflare Access header 客户端能力以便未来有 Zone 时切换。

远端使用 `AUTH_MODE=bearer`：客户端持有 256-bit 随机令牌明文，Worker secret
`SYNC_DEVICE_IDENTITIES` 只保存 SHA-256 指纹到 `ownerId/deviceId/deviceName/platform` 的映射，D1
`devices.credential_fingerprint` 保存当前指纹用于轮换和审计。owner 和设备身份始终由服务端映射，
请求体不能指定 owner，deviceId 必须与凭证映射交叉校验。当前设备开通仍由管理员手动生成
令牌并写入映射；配对码、自助注册、轮换和速率限制留待真实多设备上线前完成。
本地 `AUTH_MODE=test` 仍保留为未提交的开发便利入口。未完成 E2EE 前不得上传真实数据。

---

## 14. 端到端加密

D1 默认提供 Cloudflare 管理的静态加密和 TLS 传输加密，但这不等于客户端端到端加密。Agent 对话可能包含代码、个人记忆、文件摘要和工具上下文，真实数据同步建议使用应用层 E2EE。

### 14.1 分阶段策略

- 本地开发/POC：允许仅使用测试数据的明文 payload，以验证幂等、游标和冲突；
- production schema：从第一版就保留 `payload_encoding / key_version / BLOB payload`；
- 真实个人数据上线前：完成 E2EE、密钥备份和新设备配对；
- Worker 永远不承担加密主密钥托管。

### 14.2 密钥模型

建议：

- 账户级 Sync Master Key 随机生成；
- 设备本地由 OS Keyring / Android Keystore 保护；
- 每个 payload 使用随机 nonce 的 AEAD；
- envelope 元数据作为 associated data，防止密文被替换到另一实体；
- `key_version` 支持轮换；
- 新设备通过二维码/短期配对会话传递加密后的主密钥；
- 提供一次性 recovery key/恢复短语，并明确提示用户：丢失所有设备和恢复材料后，Cloudflare 备份也无法解密。

具体算法应在实现前单独安全评审；不得自创密码算法或复用 nonce。

### 14.3 元数据泄露

即使启用 E2EE，D1 仍会看到：

- owner/device 标识；
- 实体类型和实体 ID；
- 变更频率、大小、版本和时间；
- 删除行为。

本设计不以隐藏访问模式为目标。如果未来需要更强元数据隐私，需要重新评估固定大小 padding、类型隐藏和批次混合，其成本不属于 V0.3。

---

## 15. 免费层容量与设计约束

截至 2026-07-16，Cloudflare 官方文档给出的相关免费层限制：

| 项目 | Workers/D1 Free |
|---|---:|
| Worker 请求 | 100,000 / 天 |
| Worker CPU | 10 ms / invocation |
| Worker subrequests | 50 / request |
| Worker 内存 | 128 MB / isolate |
| D1 rows read | 5,000,000 / 天 |
| D1 rows written | 100,000 / 天 |
| D1 总存储 | 5 GB / 账户 |
| 免费 D1 数据库数量 | 10 |
| 单个免费 D1 数据库 | 500 MB |
| 每个 Worker invocation 的 D1 查询 | 50 |
| 单行 / 单个 string / BLOB 最大 | 2 MB |
| 单 SQL 绑定参数 | 100 |
| Time Travel | 7 天 |

这些额度对个人文本同步充足，但有以下硬约束：

1. 免费读写额度按日重置，超额后 D1 会拒绝查询，而不是自动降速；
2. 单库 500 MB 比账户 5 GB 更早成为瓶颈；
3. D1 按扫描行计量，缺少索引会快速消耗 rows read；
4. 索引更新也增加 rows written；
5. Worker 免费 CPU 只有 10 ms，不能在 Worker 内做大 JSON 合并、压缩、加解密或 LLM 工作；
6. 单行平台上限虽为 2 MB，但本项目应设置更低的应用级 payload 上限，例如 256 KB；
7. push 必须分小批次，避免 50 次查询和 100 个绑定参数的限制；
8. 原始工具输出和附件不得进入普通 D1 payload。

粗略容量原则：

- 100 条/日、平均每条同步后 4 KB，纯 payload 约 146 MB/年；
- 密文、索引、变更日志和 SQLite page 会增加实际占用；
- 保留当前快照 + 30 天变化日志比永久保留全部 change 更可控；
- 应在 D1 达到 60% / 80% 容量时显示预警，而不是等写入失败。

所有云端错误均进入后台重试。任何配额问题不得回滚已成功的本地业务事务。

---

## 16. 环境、位置、迁移与备份

### 16.1 环境

建议：

- `wrangler dev` 使用本地 D1 作为日常开发环境；
- production 使用独立远端 D1；
- 如需要真云端集成测试，再增加 staging D1；
- staging 和 production 使用不同数据库绑定、认证凭证和 Worker 环境；
- 测试不得连接 production 数据库。

免费账户允许 10 个 D1 数据库，个人项目使用 production + staging 足够。

### 16.2 数据位置

数据库创建时可使用 `apac` location hint，适合主要从亚洲写入的个人部署。需要注意：

- location hint 只在创建数据库时提供；
- 它是尽力而为，不保证固定城市或国家；
- jurisdiction 与 location hint 不是同一概念；
- 中国大陆到 `workers.dev` 或自定义域名的可达性和延迟必须在实际网络中测试；
- local-first 保证链路不稳定只影响同步体验。

### 16.3 D1 migrations

- migration 文件进入 Git，按顺序不可变；
- 本地、staging、production 使用同一组 migration 文件；
- 先在本地和 staging 应用并验证，再应用 production；
- migration 前检查 D1 Time Travel bookmark；
- 大规模 UPDATE/DELETE 分批执行；
- 不使用 `CREATE TABLE IF NOT EXISTS` 代替正式版本迁移；
- D1 migration 与本地 rusqlite migration 分开维护。

### 16.4 备份

D1 Time Travel 在免费层保留 7 天，默认启用且可按时间点恢复。它用于处理误删、错误 migration 等服务端事故，但不是 E2EE 密钥备份。

后续可将周期性 D1 导出或加密本地 SQLite 快照存入 R2，以获得更长保留期。恢复演练必须覆盖：

- D1 回滚后客户端 cursor 比服务端超前；
- 已被客户端看到的 change 在回滚后消失；
- 强制 bootstrap 和本地未推送 outbox 的再合并；
- E2EE key version 与恢复时间点匹配。

---

## 17. 可观测性与 UI

### 17.1 客户端状态

同步设置页至少展示：

- `disabled`：未配置；
- `idle`：已同步；
- `syncing`：正在 push/pull；
- `offline`：当前不可联网，本地功能正常；
- `auth_required`：凭证过期或设备被撤销；
- `error_retrying`：临时错误与下次重试时间；
- `conflict`：存在需要用户处理的冲突；
- `quota_exceeded`：平台额度或存储限制；
- pending change 数、last success、当前设备 ID/名称。

用户应能：

- 手动立即同步；
- 查看设备并撤销旧设备；
- 导出诊断信息；
- 处理文本冲突；
- 禁用云同步但保留本地数据；
- 明确选择“移除本设备凭证”或“删除云端数据”，两者不可混为一个操作。

### 17.2 日志

本地和 Worker 日志只记录：

- request/change correlation id；
- entity type、计数、字节数；
- 状态码、D1 rows read/written、耗时；
- retry/conflict 分类。

不得记录：

- Access Client Secret / bearer token；
- provider API Key；
- E2EE 主密钥、nonce+明文组合；
- message/memory 明文；
- 完整 tool params/result。

### 17.3 配额监控

利用 D1 result `meta.rows_read / rows_written`、Cloudflare Dashboard 和 Worker Logs 观察：

- pull 平均扫描行数；
- push 每 change 写入行数；
- 数据库总大小；
- 409 冲突率；
- 429/5xx/平台配额错误；
- Worker CPU 超限（1102）和免费请求超限（1027）。

---

## 18. 测试策略

### 18.1 Rust 单元测试

- HLC 单调性、时钟回拨和同毫秒计数；
- payload 序列化版本升级；
- E2EE encrypt/decrypt、associated data 篡改、错误 key/nonce；
- outbox 与业务写入原子性；
- pull 应用不产生同步回声；
- 每种实体的冲突合并；
- 退避、jitter、dead-letter；
- cursor 只在整页落地成功后前进。

### 18.2 Worker 单元/集成测试

- 认证 owner/device 隔离；
- 重复 change_id 幂等；
- stale baseRevision 返回 409；
- D1 batch 失败整体回滚；
- Message stale revision 不可覆盖，合法 CAS 编辑可生成下一 revision；
- revoked device 被拒绝；
- pull 分页不漏、不重、排序稳定；
- bootstrap 高水位期间并发写入；
- payload 大小和批量上限；
- 所有热查询使用预期索引。

### 18.3 多设备故障测试

至少模拟：

- 请求已在服务端提交，但响应在客户端丢失；
- pull 页落地一半时进程崩溃；
- 两台设备离线编辑同一 Agent；
- 两台设备从同一 parent 同时发送消息；
- 一台设备删除，另一台设备离线修改；
- 服务端 change log 已压缩，旧设备恢复上线；
- D1 暂时 5xx、429、达到每日免费额度；
- D1 Time Travel 回滚；
- 新设备拿到快照时仍有持续写入；
- 密钥轮换期间存在旧 key version payload。

### 18.4 安全测试

- 仓库和构建产物不含真实 secret；
- `settings` 中的 provider key 永不进入 payload；
- owner A 无法猜测 ID 读取 owner B 数据，即使当前部署只有一个 owner；
- 日志和错误响应不回显密文外的敏感字段；
- 被撤销设备无法 push、pull 或 bootstrap；
- 恶意大 payload、极深 JSON 和超大 batch 被尽早拒绝。

---

## 19. 分阶段实施路线

### Phase 0：同步前的数据边界修正

- [x] 将 provider API Key 从 SQLite settings 迁至 OS Keyring；
- [x] 将 settings 分为 syncable / device-local / secret；
- [x] 将 USER.md / MEMORY.md 建模为明确同步实体；
- [x] 拆分 workspace 逻辑信息与设备 folder binding；
- [x] 为所有同步实体补齐 tombstone、本地 version 和稳定 ID；
- [x] 明确 message 中 reasoning/tool content 的默认上传策略，并实现实体字段白名单投影器。

完成标准：能证明任意一次 payload 构造都不会包含 API Key、本地绝对路径或未授权工具输出。

#### Phase 0A 实现记录（2026-07-16）

- 新增统一 `SecretStore` 抽象和 OS Keyring 实现。启动时扫描旧 `provider:{id}:api_key`，仅在 Keyring 写入并读回验证成功后删除 SQLite 行；冲突或验证失败时保留旧行并报告错误。
- Provider 的新增、更新、删除均直接操作 Keyring，并在数据库操作失败时恢复原密钥；前端只接收 `has_api_key`，不再存在读取已保存密钥明文的 Tauri command。
- 主模型、任务模型、Embedding、调试提示词和模型列表拉取统一从 Keyring 取密钥。复用已保存密钥拉取模型时，Rust 会校验 Provider 类型和 API Base 与已保存配置一致，防止把密钥发送到 renderer 临时指定的 endpoint。
- `settings` 已建立 `syncable / device-local / secret / unknown` 分类边界。通用 renderer `get_setting/set_setting` 仅允许访问 `ui:*`；Phase 0B 将原 syncable 显式记忆迁成独立实体后，legacy Agent 记忆键归为 unknown。
- 新增 Agent、Session、Message、Explicit Memory、Memory 的 payload 字段白名单。Message 只投影 text parts，默认排除 reasoning、原始 tool call/result、metadata、embedding、本地路径及未知字段；Session 的 `permission_mode` 和 workspace binding 保持设备本地。
- 该投影器是同步协议的数据边界基础；Phase 2A 已将 Agent/Session 业务写入与不可变 payload 的 outbox 生成合并到同一 SQLite 事务。

#### Phase 0B 实现记录（2026-07-16）

- 新增 `explicit_memories`，以 `(agent_id, kind)` 唯一约束保存 `user_md / memory_md`，具备稳定 UUID、版本、墓碑和来源设备字段。读取与双文档保存均走专用 DB Actor 命令，保存使用单事务且内容未变化时不递增版本。
- 启动迁移将 legacy `agent:*:{user_md|memory_md}` settings 原子搬入新实体；已存在的新实体优先，孤立 Agent 的旧键保留以避免数据丢失。墓碑读取不会被残留 Markdown 视图自动复活。
- `workspaces` 只保留逻辑字段；`workspace_bindings` 保存本机 `folder_path / last_validated_at`。旧路径先复制到绑定表再删除原列，仓库通过 LEFT JOIN 保持现有桌面 UI 契约；无绑定时工具 cwd 解析为 `None`。
- Agent、Session、Message、Memory、Explicit Memory、Workspace 均已具备版本、墓碑和来源设备字段。Agent/Message/Workspace 删除改为逻辑删除，内容修改、分支选择和会话置顶会递增版本。
- Workspace payload 白名单只包含逻辑字段；本地路径和校验状态有独立测试证明不会进入 payload。
- 真实本地库迁移验证通过：迁移前后 Agent、Message、Memory、Workspace 数量一致，6 条显式记忆完整迁移，1 条 workspace 本地路径完整迁入 binding。

### Phase 1：Worker / D1 骨架

- [x] 建 `workers/sync-api` workspace；
- [x] 建本地 D1 migrations 和 schema；
- [x] 实现 auth、health、push、pull、bootstrap、ack；
- [x] 建 production D1，按需建 staging；
- [x] 添加协议 fixtures 和 Worker 集成测试；
- [x] 使用假数据验证 rows read/written 和 Worker CPU。

完成标准：单客户端可对假实体执行幂等 push/pull，重复请求不产生重复 change。

#### Phase 1A 实现记录（2026-07-16）

- Worker 使用 Hono + Zod，D1 schema 由独立 SQL migration 管理，并提供对应 Drizzle
  schema 类型；生产配置默认关闭认证，测试身份只能通过未提交的本地变量或 Wrangler
  secret 注入。
- push 限制单批 20 条、256 KiB、JSON 最大 64 层和 5 万节点，请求中的 owner 完全忽略；每个 change 的 deviceId
  必须匹配认证设备。`(owner_id, change_id)` 重放返回首次的 serverSeq/revision，不新增
  change；复用 changeId 发送不同内容会被拒绝。
- mutable entity（包括完成态 Message）使用 `baseRevision` CAS；并发回复使用不同 Message ID
  保留为消息树分支。快照条件 upsert、change insert 和 serverSeq 回写在同一
  D1 batch 中完成，20 条满批连同认证和预读仍低于免费 Worker 每次 50 条 D1 查询限制。
- pull 采用单调 `server_seq` 游标，默认 100、最大 500，并受响应体积预算约束；
  bootstrap continuation token 固定首次请求的高水位，新写入通过后续 pull 补齐；ack
  拒绝超过 owner 当前高水位的游标并保持单调不回退。
- Cloudflare Workers Vitest 池已在真实 workerd/Miniflare D1 中通过 10 项集成测试；另用
  Wrangler 成功执行本地 `0001_initial.sql` migration。

#### Phase 1B 远端验证记录（2026-07-16）

- 创建 `agnes-sync` D1，resource ID 为 `44283e54-cfec-4d16-8db8-fa572ff8a9ad`，位置为
  APAC，当前主服务 colo 为 NRT；远端 `0001_initial.sql` 的 11 条命令执行成功，共 5 张
  表，空库大小约 73.7 kB。
- 部署 Worker `agnes-sync-api`：
  `https://agnes-sync-api.caiwengong136.workers.dev`。上传 gzip 约 102 kB，部署报告
  startup time 为 12 ms；关闭认证后的 health 请求由实时 tail 观测到 wall time 3 ms、
  CPU time 3 ms、outcome ok。
- 使用随机、仅存在于 Wrangler secret 的 POC 凭证和假 Agent 实体完成远端验证：未认证
  请求返回 401；首次 push 返回 revision 1；重复 changeId 返回同一 serverSeq 且
  `idempotent=true`；pull、bootstrap 高水位和 ack 结果一致。
- 远端 `EXPLAIN QUERY PLAN` 确认 pull 使用 covering index
  `idx_sync_changes_pull`，bootstrap 使用 covering index
  `idx_sync_entities_bootstrap`；本轮 D1 查询耗时约 0.12-0.28 ms，计量元数据能正常返回
  rows read/written。
- 验证完成后已删除全部 POC devices/entities/changes/acks，四张业务表行数均为 0，并删除
  `SYNC_TEST_IDENTITIES` secret。Worker 已按仓库默认值重新部署为
  `AUTH_MODE=disabled`，真实数据必须等 Access 和 E2EE 完成后才可启用上传。

### Phase 2：Rust 事务性 outbox

- [x] 增加 HLC、sync_outbox、sync_entity_state、runtime state；
- [x] 将 Agent/Session repo 写入与 outbox 写入合并为事务；
- [x] 实现 SyncEngine single-flight、退避和手动同步；
- [x] 首先只开放 Agent/Session 测试 push；
- [x] 接入 Worker 自管 Device Token 映射与 OS Keyring 凭证，完成远端假数据端到端验证。

完成标准：任意断网或进程崩溃场景下，本地数据不丢且最终可同步。

#### Phase 2A 实现记录（2026-07-16）

- 新增 `sync_outbox / sync_entity_state / sync_runtime_state`；migration 首次启动生成安装级 Device UUID，并在重启时将遗留 `in_flight` 恢复为 `pending`。
- HLC 保存在 runtime state，能处理本机时钟回退并为后续合并远端因果时间预留接口。Agent/Session 的业务行和 outbox 在同一 `BEGIN IMMEDIATE` 事务内提交，连续离线修改按 `baseRevision = null, 1, 2...` 链接。
- outbox 实现 `pending -> in_flight -> synced/conflict/dead_letter` 状态机；网络、5xx、429 和认证失败恢复为 `pending` 并指数退避，永久请求错误进入 dead letter。accepted/conflict 结果与实体远端 revision 在单一 SQLite 事务内落库。
- `SyncService` 使用单飞锁、750 ms 去抖调度和 60 秒保底调度；提供 `get_sync_status / sync_now` Tauri command。设置页显示真实 Worker URL、Device UUID、队列/冲突计数，无凭证时显示 `auth_required` 并禁用手动同步。
- 同步凭证作为单一 JSON secret 存入 OS Keyring，支持 Bearer 和 Cloudflare Access Service Token 两种 header 策略；Renderer 仅能读取“是否已配置”，无法获取明文。
- 业务白名单证明 `permission_mode`、workspace binding 不进入 Session payload；远端来源 Session 的专用写入路径不生成同步回声。Worker 协议允许内置 Agent 使用非 UUID 稳定 ID，仍强制 changeId/deviceId 为 UUID。
- 完整验证通过：Rust 72 项、Python 15 项、前端 4 项、Worker/D1 10 项，且前端 production build 和 Worker typecheck 通过。真实本地库 migration 后 Agent/Session 数量未变。

#### Phase 2B 实现记录（2026-07-16）

- Cloudflare 账户 Zone 列表为空，Access organization API 对当前 Wrangler OAuth 返回 403，因此按预案从 Access Service Token 回退为 Worker 自管 Device Token。
- `AUTH_MODE=bearer` 对请求令牌计算 SHA-256，仅与 Wrangler secret 中的指纹映射比对；未认证、错误令牌、deviceId 不匹配和已撤销设备均被拒绝。
- 新增 `set_sync_credential` Tauri command，支持 Bearer/Access 结构的校验、写入后读回验证和失败回滚。设置页可以用密码输入一次性写入或清除 Keyring 凭证，Renderer 无读取明文接口。
- 远端 Worker 部署后，未认证和错误令牌的 health 请求返回 401，正确临时令牌返回 200。使用真实 Rust `SyncService`、临时 SQLite 和假 Agent 完成 push，本地 outbox 进入 `synced`，远端 revision 为 1。
- 验证后已按测试 owner 清除远端记录，D1 `devices/entities/changes/acks` 均为 0；一次性明文令牌已从本机删除，远端指纹映射重置为空数组。当前 Worker 认证入口已启用，但不存在任何有效设备凭证；当前远端 Version ID 为 `3439a5b9-ca5d-47ed-9145-a8a13e054495`。

### Phase 3：消息、记忆与冲突

- [x] 同步完成状态 message；
- [x] 支持分支消息和 selected child 状态；
- [x] 同步 explicit memory / memory_store 文本；
- [x] 完成基础类型化 MergeEngine 与冲突解决；
- [x] UI 增加冲突管理；
- [x] UI 增加设备管理；
- [x] 补齐 Session HLC 与 Explicit Memory 文本合并策略。

完成标准：两台桌面设备可离线产生变更并在恢复网络后确定性收敛，不静默丢失文本。

#### Phase 3A 实现记录（2026-07-16）

- Rust protocol 与 `SyncTransport` 已实现 `bootstrap / pull / ack`，HTTP 成功响应使用严格
  camelCase DTO 反序列化，统一处理认证、永久错误、限流、网络和无效响应。
- SyncEngine 顺序固定为 `bootstrap -> push -> pull -> ack`。bootstrap continuation token 和
  pull `server_seq` 持久化在 SQLite；每页业务行、`sync_entity_state`、HLC 与 cursor 在同一
  `BEGIN IMMEDIATE` 事务提交，成功提交后才发送 ack。ack 丢失时本地 cursor 不回退，下一轮
  会重新 ack。
- 首批远端应用开放 Agent、Workspace、Session。payload 使用 `deny_unknown_fields` 类型化读取，
  严格校验实体 ID、Device UUID、HLC、schema/encoding、版本、墓碑与 payload 组合；未开放的
  Message/Explicit Memory/Memory 会让整页失败且不推进 cursor，不会被静默跳过。
- 远端写入走 DB actor 专用命令，直接写业务表且不生成 outbox。相同或更旧 revision 重放只
  推进页面游标；本地存在 pending/in-flight/conflict/dead-letter 时保留本地业务内容，将待推送
  项标为明确冲突，并把远端 payload 保存为后续 MergeEngine 的基线。
- Workspace 逻辑 CRUD 已接入事务性 outbox。Session payload 同步 `workspace_id`，继续排除
  `permission_mode` 和本地路径；远端 Workspace 不创建 `workspace_bindings`。
- Worker bootstrap 使用外键依赖排序和对应 D1 expression index，continuation token 保持兼容。
  本地 `EXPLAIN QUERY PLAN` 确认使用 covering index。Rust 新增多页 bootstrap、畸形页整页回滚、
  replay 无回声、pending 冲突和 commit-before-ack 测试；Worker 集成测试覆盖逐页依赖排序。
- `0002_bootstrap_dependency_order.sql` 已应用到 production D1，Worker 已部署为 Version
  `11f5a271-fcd5-479c-a527-6b8bab81f064`。远端未认证 health 返回 401，bootstrap 查询在 NRT
  使用 `idx_sync_entities_bootstrap` covering expression index，实测 SQL 约 0.25 ms；验证后
  `devices / sync_entities / sync_changes / sync_acks` 仍全部为 0，未上传真实数据。

#### Phase 3B 实现记录（2026-07-16）

- Message 仅在 `status=complete` 时生成 outbox；pending/streaming、reasoning、原始 tool parts、
  model、token count 和 metadata 均不上传。payload 包含稳定 `seq`、消息树关系和 text parts；
  指向未完成子消息的 `selected_child_id` 暂投影为 `null`，子消息完成后再生成父 Message revision。
- Message 创建、完成、正文替换、分支切换和墓碑均与 outbox 在同一 SQLite 事务。删除叶子消息
  会原子清除父指针并写两者变更；消息树遍历增加 cycle guard，损坏数据不会造成无限循环。
- Worker 已取消与现有产品行为冲突的 Message append-only 特判，所有 Message 更新按
  `baseRevision` CAS；同一 parent 下的并发回复继续使用不同 message ID，因此不会互相覆盖。
- Explicit Memory 使用 `SHA-256(agent_id + kind)` 派生云端稳定 ID，解决两台设备离线首次创建
  USER.md/MEMORY.md 时本地 UUID 不同的问题；本地现有行 ID 无需迁移，双文档和各自 outbox
  仍在一个事务提交。
- Memory Store 创建、修改、删除与 outbox 原子提交；同步白名单只含名称、关键词、正文、创建人
  和版本状态，不含 embedding、模型、confidence、scope/source。远端正文变化或墓碑会在同一
  事务清除旧 embedding metadata/向量引用，后续按本机当前 embedding 模型重建。
- Rust 接收端已开放 Message、Explicit Memory、Memory 严格 payload。远端写入不生成 outbox，
  本地存在未解决变更时沿用 Phase 3A 的显式 conflict 路径并保存 remote base payload。
- 新增完成态过滤、私有 part 排除、稳定显式记忆 ID、六类实体同页 bootstrap、远端记忆更新使
  embedding 失效等测试。Phase 3C 继续实现字段级三方合并、冲突查看/处理 UI 和设备管理。
- Worker 已部署为 Version `b6ac4f3c-6298-4d58-9432-f4fe1fb46802`，startup time 10 ms；
  未认证 health 仍返回 401，部署后 production D1 的
  `devices / sync_entities / sync_changes / sync_acks` 仍全部为 0，未上传真实数据。

#### Phase 3C 冲突闭环实现记录（2026-07-16）

- 新增本地 `sync_conflicts`，按冲突实体同时保存 base/local/remote payload、双方 HLC、删除状态、
  远端 revision 和冲突字段。状态计数按冲突实体而非 outbox 行计算，多次离线修改不会重复显示。
- 修复 push CAS 冲突时过早覆盖 `sync_entity_state.remote_revision` 的问题。现在 push 只登记待补全
  冲突，后续 pull 同一远端 revision 可正常保存 remote payload，不会因旧 hash 被误判为数据损坏。
- Rust `MergeEngine` 使用确定性 JSON 三方规则：双方相同、仅本机变化、仅云端变化可直接选择；
  Agent、Session、Workspace 和 Message 的非重叠字段自动合并，同字段修改、删除冲突、
  Explicit Memory 与 Memory Store 同 ID 冲突继续保留人工选择，不静默覆盖文本。
- 自动合并和“保留本机”都会先以最新 remote revision 更新本地业务快照，再在同一 SQLite 事务
  生成新的 outbox；“接受云端”直接应用已验证的远端 payload 且不产生同步回声。旧冲突 change
  被明确标记为已解决，失败或进程中断时整笔事务回滚。
- 新增 `list_sync_conflicts / resolve_sync_conflict` Tauri command。设置页同步区域按实体展示冲突字段、
  本机/云端白名单内容和 revision，可选择“保留本机”或“接受云端”；remote payload 未下载完成时
  禁止解决，并每 5 秒刷新后台同步状态。
- 新增 CAS conflict -> pull 三份 payload、保留本机重新基于远端 revision 入队、接受云端无回声、
  非重叠字段自动合并等测试。Phase 3C 下一步实现 Worker 设备列表/撤销 API 与设备管理 UI，
  再补 Session 同字段 HLC tie-break 和 Explicit Memory 文本 diff3 策略。

#### Phase 3C 设备管理实现记录（2026-07-16）

- Worker 新增 `GET /v1/devices` 与 `POST /v1/devices/{deviceId}/revoke`。查询按认证 owner 隔离，
  返回设备元数据与 `sync_acks.last_server_seq`，不返回 credential fingerprint；跨 owner 目标统一
  按不存在处理。撤销幂等，服务端和客户端均禁止当前设备自我撤销。
- 设备被撤销后，既有 `authorizeDevice` 会在任何 `/v1/*` 路由执行前返回 `DEVICE_REVOKED`；
  无需修改 D1 schema，也不依赖从 Wrangler secret 中立即删除旧指纹。
- Rust HTTP client 新增严格设备 DTO 校验：拒绝非法/重复 UUID、负时间或游标、错误 current 标记
  和不匹配的撤销响应。Tauri 暴露 `list_sync_devices / revoke_sync_device`，Renderer 无法读取凭证。
- 设置页同步区域显示设备名、平台、最后在线、ack 游标、本机与撤销状态；仅其他活跃设备显示
  撤销图标，操作前再次确认。设备列表与冲突状态同样每 5 秒刷新。
- Worker 集成测试覆盖 owner 隔离、current 标记、ack 状态、幂等撤销、自我撤销保护和撤销后
  凭证失效；Rust 测试覆盖畸形设备响应。Phase 3C 剩余 Session 同字段 HLC tie-break 与
  Explicit Memory 文本 diff3，之后进入 E2EE。
- Worker 已部署为 Version `30e8771b-2a8f-4f1c-9cf1-15caaf32a175`，startup time 12 ms；
  未认证访问 `/v1/health` 与 `/v1/devices` 均返回 401。部署后直接查询 production D1，
  `devices / sync_entities / sync_changes / sync_acks` 均为 0，未上传真实数据。

#### Phase 3C 合并策略收口记录（2026-07-16）

- Session 的标题、上下文参数、模型/思考配置、workspace 和 pin 在同字段并发修改时，按 HLC 的
  `physical_ms / counter / node` 完整顺序确定胜者；极端情况下 HLC 完全相同，再按字段 JSON
  表示稳定决胜。`summary / summary_updated_at` 不使用 LWW，双方同时修改时继续保留人工冲突。
- Explicit Memory 使用 `diffy` 对共同 base、local、remote 执行行级 diff3。非重叠文本区块自动
  合并；缺少共同 base、字段类型异常或修改区块重叠时不写入冲突标记文本，继续保留三份 payload
  供设置页人工选择。
- HLC 决胜和成功 diff3 都沿用冲突解决事务：先以最新 remote revision 归一化业务快照，再生成
  新 outbox。`sync_conflicts` 保留 `auto_merge` 事件和实际自动解决字段，便于后续审计。
- Rust 完整测试为 92 项通过、1 项需临时远端凭证而忽略；新增测试覆盖摘要例外、diff3 重叠失败、
  Session/Explicit Memory 自动收敛、冲突事件记录和 `baseRevision = 2` 重新入队。Phase 3 已完成，
  下一步进入 E2EE envelope、密钥配对与恢复设计。

### Phase 4：E2EE 与真实数据上线

- [x] 冻结并实现 `xchacha20poly1305-v1`、AAD、密文 Hash 与多版本 keyset 核心；
- [x] 接入 OS Keyring 初始化、恢复材料与 encrypted-only 门禁；
- [x] 接入 push/pull/bootstrap 加解密和 Worker 密文校验；
- [ ] 实现新设备配对和密钥轮换；
- [ ] 做安全审计和恢复演练；
- [x] 确认 production D1 为空并部署 encrypted-only Worker。

完成标准：D1/Worker 日志和表中不存在可读聊天、角色卡或记忆正文。

#### Phase 4A 密码学核心记录（2026-07-16）

- 新增 Rust `sync/crypto.rs`，使用 RustCrypto XChaCha20-Poly1305、192-bit 随机 nonce 和完整
  128-bit tag；外层 payload 为无 padding Base64，`payload_hash` 对 nonce+ciphertext+tag 计算。
- AAD 使用固定 domain 和长度前缀二进制编码，认证协议版本、实体类型/ID、resulting revision、
  HLC、payload schema、origin device、encoding 和 key version。字节级格式及固定向量见
  `ProjectPlan/E2EE.md`。
- 多版本 Sync Keyset 支持随机初始 key、严格 JSON 解析和单调轮换；key 类型不实现 `Debug`，
  解码 key、keyset 临时字符串和明文 JSON buffer 在所有正常/错误路径使用 `zeroize` 清理。
- 本节只实现独立密码学核心，尚未修改线上同步行为，也未生成或保存真实账户密钥。完成 Keyring、
  recovery 和 encrypted-only 门禁前，production 继续禁止真实数据上传。

#### Phase 4B Keyring 与恢复门禁记录（2026-07-16）

- Rust 可生成初始 keyset 或用两件式 Recovery Key/Bundle 恢复完整多版本 keyset。Recovery Key 为
  随机 256 bit；HKDF-SHA256 域隔离后使用 XChaCha20-Poly1305 包裹 keyset。严格格式与算法见
  `ProjectPlan/E2EE.md`。
- keyset 只写入 OS Keyring 的 `sync:e2ee:keyset:v1`，所有安装均读回验证。新设备恢复拒绝覆盖不同
  keyset；DB 状态提交失败会回滚本轮新安装。未确认 keyset 可显式丢弃并验证删除，已确认 keyset
  禁止清除。
- 设置页支持创建、复制、确认和恢复；Renderer 只短暂持有 Recovery Key 与加密 Bundle，关闭设置、
  切换离开当前页或成功完成后立即清空 React state，不存在读取 master key/keyset JSON 的 IPC。
- `SyncStatus` 新增 E2EE 状态和 key version。`run_once` 已启用 encrypted-only 门禁，因此 Phase 4C
  接入密文传输前不会继续发送现有明文 outbox；设备列表/撤销仍可使用。Worker 和 production D1
  本轮未修改，仍未上传真实数据。
- 完整验证为 Rust 99 项通过、1 项明文远端 POC 按门禁忽略，前端 4 项通过且 production build
  成功；真实 Tauri 窄窗口下恢复区无溢出或控件重叠。

#### Phase 4C 密文传输记录（2026-07-16）

- `sync_outbox` 新增仅本机使用的 `source_payload`。首次发送在 `in_flight` 状态下批量固化密文、
  encoding、Hash 和 key version；整批要求每行从 `json` 恰好更新一次，否则事务回滚。重试和
  进程恢复直接复用固化结果，不会为同一 `changeId` 生成新 nonce。
- accepted 后 `sync_entity_state.base_payload`、push/pull 冲突中的 local/remote payload 均继续保存
  解密后的业务 JSON；D1 和 HTTP 只出现密文字符串。delete 固定使用 `tombstone-v1`、null payload、
  null key version 和 SHA-256 空字节 Hash。
- pull/bootstrap 在 DB actor 前整页校验和解密，AAD 分别使用 change 的 `resultingRevision/deviceId`
  或 snapshot entity 的 `revision/changedByDeviceId`。坏 Hash、错误 key/AAD 或密文会终止整页，
  不写业务表、不推进 cursor、不发送 ack。
- Worker Zod schema 已切换 encrypted-only：upsert 只接受 Base64 密文字符串与正 key version，
  delete 只接受规范墓碑；Worker 继续把 payload 当不透明 JSON 值存取，不执行解密。
- 本地验证为 Rust 103 项通过、1 项需要显式远端凭证而忽略；Worker 15 项通过且 typecheck 成功。
  新增覆盖密文固化事务回滚、网络重试字节级复用、有效密文拉取和篡改页原子拒绝。
- Worker 版本 `e7e09963-effe-4f12-a81d-5690b81eb853` 已部署；未认证 health 返回 401。部署前后
  production D1 的 `sync_entities / sync_changes / sync_acks / devices` 均为 0，本轮没有上传业务数据。

### Phase 5：Android 与 R2

- Android 使用相同 Sync Protocol 和 E2EE；
- Android 保持本地 SQLite 缓存；
- workspace folder binding 在 Android 为空；
- 按 `STORAGE_AND_RAG.md` 接 R2 附件/头像/源文件/加密向量制品；
- R2 对象也必须遵循认证、owner 隔离和客户端加密。

---

## 20. 验收标准

V0.3 被视为完成，至少满足：

- 关闭网络时所有本地功能正常，变更进入 pending；
- 网络恢复后无需用户干预即可追平；
- 同一 push 重放任意次数只产生一次服务端 change；
- pull 页面重复应用不会产生重复业务记录或同步回声；
- 两设备并发回复形成消息分支，不覆盖对方消息；
- 删除通过墓碑传播，长期离线设备不能复活已删除实体；
- 新设备可 bootstrap，并无缝接续 bootstrap 期间的新变更；
- API Key、同步凭证、E2EE key、本地路径、原始工具审计不进入云端；
- D1 热查询均有索引，个人典型使用量远低于每日免费额度；
- 配额耗尽或 D1 故障只显示同步错误，不影响本地业务；
- production migration 和 Time Travel 恢复流程经过演练；
- 用户能撤销单个设备，并保留其他设备正常同步。

---

## 21. 实现前需最终确认的决策

| 决策 | 推荐值 | 说明 |
|---|---|---|
| 云端角色 | 同步副本/中继 | 不作为本地 Agent 执行依赖 |
| 云端数据模型 | entity snapshot + change stream | 不复制全部本地表 |
| 首版认证 | Worker 自管 Device Token | 目标账户无 Zone，已从 Access 方案回退并完成远端验证 |
| E2EE | production 必须，POC 仅假数据可明文 | schema 从首版预留密文字段 |
| 消息同步时机 | complete 后整体同步 | 不同步 token delta |
| reasoning | 默认不上传 | 单独产品开关需安全评审 |
| tool result | 默认仅脱敏摘要 | 原始输出保持本地 |
| workspace path | 永不上传 | 只同步逻辑 workspace |
| 冲突位置 | Rust MergeEngine | Worker 只做 CAS |
| change log 保留 | 初始 30 天 | 结合 ack、容量后调整 |
| payload 应用级上限 | 初始 256 KB | 大对象转 R2 或保持本地 |
| D1 location hint | `apac` | 创建时设置，实际链路需测试 |
| 开发/生产 | local D1 + production D1 | 需要真云测试时增加 staging |

---

## 22. 官方资料

- [D1 Pricing](https://developers.cloudflare.com/d1/platform/pricing/)
- [D1 Limits](https://developers.cloudflare.com/d1/platform/limits/)
- [Workers Pricing](https://developers.cloudflare.com/workers/platform/pricing/)
- [Workers Limits](https://developers.cloudflare.com/workers/platform/limits/)
- [D1 Database Worker API](https://developers.cloudflare.com/d1/worker-api/d1-database/)
- [D1 Migrations](https://developers.cloudflare.com/d1/reference/migrations/)
- [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/)
- [D1 Data Security](https://developers.cloudflare.com/d1/reference/data-security/)
- [D1 Data Location](https://developers.cloudflare.com/d1/configuration/data-location/)
- [D1 Environments](https://developers.cloudflare.com/d1/configuration/environments/)
- [D1 Index Best Practices](https://developers.cloudflare.com/d1/best-practices/use-indexes/)
- [Workers Secrets](https://developers.cloudflare.com/workers/configuration/secrets/)
- [Cloudflare Access Service Tokens](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/)
- [Workers KV Consistency](https://developers.cloudflare.com/kv/concepts/how-kv-works/)
- [R2 Pricing](https://developers.cloudflare.com/r2/pricing/)
