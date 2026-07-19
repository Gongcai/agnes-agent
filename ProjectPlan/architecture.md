# 架构方案 v2（agnes-agent，经审计修正的主方案）

> 前端框架：**React**（Vite + TypeScript + Tailwind + shadcn/ui）。
> 设计原则：**架构优良设计优先于敏捷/MVP**。本版为经用户审计通过的三平面架构，已修正协议方向、WS 主从、DB 驱动、记忆真相源、工具 sandbox 和事务性 outbox 等关键边界。
> 总览见 `PROJECT.md`；本文件为其细化落地。RAG、大文件、向量制品和外部 Provider 见 `STORAGE_AND_RAG.md`。

---

## 1. 核心架构（三平面，最终版）

```text
Presentation Plane  (React)
  只负责 UI / 流式渲染 / 工具审批卡 / 记忆编辑
  通过 Tauri IPC 与 Rust 通信，绝不碰本地系统

Execution + Data Plane (Rust / src-tauri)   ← 唯一资源拥有者
  唯一拥有：SQLite / 文件 / shell / PTY / SSH / 工具执行 / sidecar 生命周期 / 同步
  内部：DbActor（DB 单写者）、ToolExecutor（工具执行+审计）、AgentManager（sidecar 生命周期）

Reasoning Plane (Python sidecar / agent)
  LangGraph + LiteLLM
  只做：prompt 拼装 / 模型推理 / 工具*选择* / 记忆抽取*建议*
  绝不直接访问 DB / 文件系统 / shell
```

**不可动摇的边界**：React 不碰本地系统；Python 不直接碰 DB/文件/shell；Rust 统一拥有本地资源与数据。这样权限、审计、冲突、测试才有单一归口。

---

## 2. 仓库结构（标准 Tauri 单应用）

```text
agnes-agent/
├── src/                  # React 前端源码（Vite 根：index.html + src/ + public/）
├── public/
├── src-tauri/            # Rust Tauri 应用
│   ├── Cargo.toml
│   ├── capabilities/     # Tauri 权限声明
│   ├── migrations/       # DB 迁移（rusqlite）
│   ├── icons/
│   └── src/
│       ├── main.rs
│       ├── error.rs            # 统一错误 AppError
│       ├── state.rs            # 托管 DbActor / AgentManager / Config
│       ├── commands/           # Tauri 命令（IPC 边界：仅校验+委托）
│       ├── agent/              # AgentManager + WS Server + Agent Protocol
│       ├── db/                 # rusqlite + DbActor + repo/
│       ├── tools/              # ToolExecutor + 各工具实现 + tool_policy
│       ├── fs.rs pty.rs ssh.rs
│       ├── memory/             # memory_search 实现 / USER·MEMORY.md 读写
│       └── sync/               # 增量同步（V0.3，先留接口）
├── agent/                # Python sidecar（uv 管理）
│   ├── pyproject.toml
│   ├── app/
│   │   ├── main.py           # WS Client，连接 Rust
│   │   ├── protocol.py       # Agent Protocol 消息（pydantic 镜像）
│   │   ├── graph.py          # LangGraph 状态机
│   │   ├── prompt.py         # 用 ContextSnapshot 拼装 prompt（含 ①② 预算/压缩）
│   │   ├── tools.py          # 工具*定义*（执行走 Rust）
│   │   ├── memory_extract.py # 记忆抽取*建议*（写回 memory_store）
│   │   └── models.py         # LiteLLM + 模型注册表
│   └── tests/
├── shared/               # 协议文档 / schema（Agent Protocol 唯一真相源文档）
│   └── protocol.md
├── workers/              # Cloudflare Worker（V0.3 再加）
├── ProjectPlan/
├── package.json          # 根：前端 + tauri 脚本（pnpm）
└── pnpm-workspace.yaml   # packages: agent / workers（不用 turborepo）
```

- 不用 `src/src` 嵌套；`src/` 即 React 源码根（与 `index.html` 同级）。
- 包管理：**pnpm workspace**（不用 npm workspace，不用 turborepo）。注：当前环境尚未安装 pnpm，脚手架时需先装。
- `agent/` 用 `uv` 单独管；`workers/` 后续加。

---

## 3. 通信契约

### 3.1 React ↔ Rust（Tauri IPC）
- 命令（请求/响应）：`send_message`、`list_agents`、`create_agent`、`approve_tool`、`cancel_run` 等。
- 事件（Rust → React 推送，流式）：`agent://token`（流式 token）、`agent://tool_call_pending`（待审批）、`agent://tool_result`、`agent://error`。
- **类型生成不写死单一工具**：首选 `specta` + `tauri-specta`，或 `tauri-typegen` / `ts-rs`，总之不手写重复类型；不把某个生态工具当成不可替代的架构核心。`shared/protocol.md` 描述协议，Rust 结构体为真相源。

### 3.2 Rust ↔ Python（Agent Protocol，**Rust 为 WS Server，Python 为 Client**）

**主从必须定死**：Rust 开 WebSocket Server（仅 `127.0.0.1`，**绝不 `0.0.0.0`**），Python sidecar 作为 Client 连接。

启动流程：
```text
1. Rust 绑定 127.0.0.1:随机端口
2. Rust 生成一次性 AGENT_PROTOCOL_TOKEN
3. Rust spawn Python sidecar（token + 端口经环境变量传入）
4. Python 主动连接 Rust WS
5. Rust 校验 token
6. 建立双向协议
```
Python 崩了 Rust 可重启；Python 不暴露任何本地服务。

**每帧基础字段（所有消息必备）**：
```json
{
  "protocolVersion": 1,
  "id": "uuid",
  "runId": "uuid",
  "sessionId": "uuid",
  "type": "...",
  "createdAt": "ISO8601",
  "payload": {}
}
```

**消息类型（方向已修正，杜绝歧义）**：
```text
Rust → Python:
  run_request        启动一轮；payload 含 input + context(ContextSnapshot)
  tool_result        工具执行结果（审批后）
  approval_result    用户对工具调用的批准/拒绝
  run_cancel         取消本轮（用户点停止）
  user_message       带外用户输入/纠正（可选）

Python → Rust:
  assistant_delta    流式推理 token
  tool_call_request  请求执行某工具
  memory_query_request  推理中二次记忆检索
  run_finished       本轮结束
  run_error          本轮出错

双向握手/保活：
  hello / ready / ping / pong
```

**关键语义厘清（原文档矛盾点）**：
- `tool_call_request`：**Python → Rust**，表示"我要调这个工具"。
- `tool_call_pending`：**Rust → React（IPC 事件，非 Agent Protocol）**，表示"有工具调用在等审批"。
- `tool_result`：**Rust → Python**，Rust 执行完后返回。
三者分属两条不同通道，不可混。

**ContextSnapshot（消除 Python 频繁零碎 DB 请求）**：每轮开始时 Rust 一次性生成快照随 `run_request` 下发，Python 本轮内不再像 ORM 一样远程查 DB：
```json
{
  "type": "run_request",
  "sessionId": "uuid", "agentId": "uuid",
  "payload": {
    "input": "用户本轮输入",
    "context": {
      "agent": {}, "settings": {}, "recentMessages": [],
      "summary": "...", "explicitMemories": [],
      "retrievedMemories": [], "projectContext": []
    }
  }
}
```
Python 本轮内只做：prompt 拼装 / 模型调用 / 工具决策 / 记忆抽取建议。若推理中确需二次检索，再发 `memory_query_request`（单一往返，不批量拉表）。

### 3.3 工具调用 + 人类确认 + capability sandbox
1. Python 发 `tool_call_request` → Rust。
2. Rust 依该 Agent 的 `tool_policy` 判断是否需要审批：需审批则**挂起**，向 React 发 `agent://tool_call_pending` 事件（UI 出审批卡）；免审批则直接执行。
3. 用户批准/拒绝 → React `approve_tool` → Rust 回 Python `approval_result`（拒绝则附 `ok:false`）。
4. Rust `ToolExecutor` 执行 → 回 Python `tool_result`。

审批是**最后一关，不是唯一防线**。工具权限 = capability + approval + sandbox + audit（见 §6）。

---

## 4. Rust Core 模块

- `commands/`：IPC 边界，仅参数校验 + 委托，返回 `Result<T, AppError>`。
- `db/`：**rusqlite + 专用 DbActor**（见 §7），保证 SQLite 写入顺序与事务边界，且不阻塞 async runtime。
  ```text
  db/
    mod.rs  actor.rs  schema.rs  migrations.rs
    repo/ agents.rs sessions.rs messages.rs memory.rs tools.rs
  ```
- `agent/`：`AgentManager` 负责 spawn/kill Python、维护 WS Server、token 校验、健康检查与崩溃重启、随应用退出清理。启动走 `AgentLauncher` trait：
  ```text
  AgentLauncher trait
    DevUvLauncher      → std::process::Command 启动 `uv run python -m app.main`
    BundledBinLauncher → 发布态加载 externalBin（agentd 二进制）
  ```
- `tools/`：`ToolExecutor` + 每工具模块（shell/file/git/ssh/memory_search/browser），统一 trait；MCP 外部工具 V0.5 以同类 trait 接入。
- `storage/`：网盘领域 DTO、`FileSourceProvider / FileUploadProvider / FileManagementProvider / ObjectStorageProvider / QuotaProvider / ProviderFactory` 窄端口、Keyring 凭证 adapter、开放注册表与 `StorageService`。`ProviderFactory` 同时提供可选的一次性授权和 challenge/poll 两阶段授权，凭证结果不经过 renderer。`google_drive` 通过官方 API 实现 Desktop OAuth + PKCE、Drive v3 文件源/断点上传、用户文件移入回收站及 `appDataFolder` 对象存储；`quark_drive` 通过 community HTTP adapter 实现文本/JSON Cookie 与二维码授权、文件源、分片上传、配额和移入回收站，但不实现应用对象存储。业务层和 renderer 不依赖任一服务商 API 类型。
- `memory/`：实现 `memory_search`（sqlite-vec cosine 向量 + 字符串包含匹配，RRF 融合）；负责 `USER.md`/`MEMORY.md` 读写（DB 为真相源，见 §8）。
- `state.rs`：Tauri `State` 托管 `DbActor` 句柄 / `AgentManager` / `Config`。

---

## 5. Python Agent 模块

- `graph.py`：LangGraph 状态机（多步、检查点、可中断等待 approval）。
- `prompt.py`：**会话内记忆 ①② 的实现点**——依据 `run_request.context` 中的 `recentMessages`/`summary` 与预算（`model_context_limit` ∩ `user_context_limit` − reserved）保 `recency_window` 原文、滚动压缩；按 `PROJECT.md` 拼装顺序组装最终 prompt。**不直接读 DB**，数据全来自 ContextSnapshot。
- `tools.py`：给 LLM 看的工具*定义*（schema），实际执行经 Agent Protocol 委托 Rust。
- `memory_extract.py`：后台从对话抽取事实，建议写入 `memory_store`（带 `confidence`/`source`/`agent_id`），经 Rust 落地。
- `models.py`：LiteLLM 统一接入 + 模型注册表（含 `max_context_tokens` 驱动预算）。

---

## 6. 工具权限模型（capability + approval + sandbox + audit）

`tool_policy` 为**结构化**配置（非任意 JSON），示例：
```json
{
  "shell": {
    "enabled": true, "approval": "always",
    "allowedCwd": ["~/Projects"], "denyWriteOutsideWorkspace": true,
    "timeoutSec": 30, "maxOutputBytes": 200000, "envAllowlist": ["PATH", "HOME"]
  },
  "file": {
    "enabled": true, "approval": "write",
    "allowedRoots": ["~/Projects", "~/.agnes"]
  },
  "git": { "enabled": true, "approval": "push" }
}
```
维度：`enabled`（是否可用）、`approval`（always / write / push / never）、`allowedCwd`/`allowedRoots`（可访问范围）、`timeoutSec`、`maxOutputBytes`、`envAllowlist`（环境变量白名单）。审批只决定"能不能跑"，capability 决定"能跑在哪、跑多久、出多少"——双保险。每次执行落 `tool_calls` 审计（status/risk_level/cwd/exit_code/stdout/stderr/时间戳/策略快照）。

---

## 7. 数据层（SQLite 细化 schema，rusqlite）

```sql
agents(
  id TEXT PK, name TEXT, persona TEXT, scenario TEXT,
  system_prompt TEXT, greeting TEXT, example_dialogue TEXT,
  model TEXT, tool_policy TEXT /*JSON 结构化*/, avatar TEXT, tags TEXT,
  created_at TEXT, updated_at TEXT
)
sessions(
  id TEXT PK, agent_id FK→agents.id, title TEXT,
  context_limit INT NULL, compress_threshold REAL DEFAULT 0.85,
  recency_window INT DEFAULT 20, reserved_output_tokens INT,
  summarizer_model TEXT, summary TEXT, summary_updated_at TEXT,   -- summary 是会话级状态，不塞 message_parts
  created_at TEXT, updated_at TEXT
)
messages(
  id TEXT PK, session_id FK, role TEXT, seq INTEGER,
  status TEXT /*pending|streaming|complete|failed|cancelled*/,
  model TEXT, token_count INTEGER, metadata TEXT,
  created_at TEXT, updated_at TEXT
)
message_parts(
  id PK, message_id FK, kind TEXT /*text|tool_call|tool_result|reasoning*/,
  ordinal INTEGER, mime_type TEXT, tool_call_id TEXT,
  content TEXT, metadata TEXT
)
memory_store(
  id PK, agent_id FK, content TEXT, type TEXT, scope TEXT,
  source TEXT, confidence REAL, status TEXT /*active|archived|deleted*/,
  expires_at TEXT, pinned INTEGER, source_message_id TEXT, last_used_at TEXT,
  created_at TEXT, updated_at TEXT, embedding_id FK
)
embedding_items(                     -- 元数据表（与向量索引分离）
  id TEXT PK, ref_type TEXT, ref_id TEXT,
  model TEXT, dims INTEGER, content_hash TEXT, created_at TEXT
)
-- 向量虚拟表（sqlite-vec）：vec_embeddings_{dims}(embedding_id, vector)
-- 按实际维度延迟创建，cosine 距离，按 embedding_id 关联 embedding_items
knowledge_collections(id PK, name, scope, workspace_id, version, deleted_at)
collection_agents(collection_id FK, agent_id FK, permission)
documents(id PK, collection_id FK, title, media_type, current_version_id, version, deleted_at)
document_sources(id PK, document_id FK, provider_account_id, encrypted_locator, provider_revision)
document_versions(id PK, document_id FK, logical_version, plaintext_hash, size, parser_profile_id)
document_chunks(id PK, document_version_id FK, ordinal, content, content_hash, metadata)
tool_calls(
  id PK, session_id FK, message_id FK, tool TEXT,
  params TEXT, result TEXT, status TEXT
    /*pending_approval|running|done|rejected|failed|cancelled*/,
  risk_level TEXT, cwd TEXT, exit_code INTEGER,
  stdout TEXT, stderr TEXT, started_at TEXT, completed_at TEXT,
  approval_policy_snapshot TEXT, created_at TEXT
)
sync_log(
  id TEXT PK, device_id TEXT, entity_type TEXT, entity_id TEXT,
  operation TEXT, payload TEXT, payload_hash TEXT,
  entity_version INTEGER, created_at TEXT, hlc TEXT, synced_at TEXT NULL
)
settings(key PK, value TEXT)
```

**embeddings 拆分原因**：sqlite-vec 需要专门的向量虚拟表/索引表，不与普通 metadata 混在一张表。`embedding_items` 保留 model revision、dims、normalization、instruction/content hash 等 profile 信息，用于重算、去重和失效判定。当前记忆嵌入允许 1 到 8192 维，查询按 Agent 和 profile 隔离；RAG 扩展后改用 collection/profile namespace，避免多 Agent 重复索引。

**向量制品边界**：原始向量行、sqlite-vec 虚拟表和运行中 SQLite 文件不进 D1。大型 RAG 可按 document version + embedding/parser/chunker fingerprint 导出便携分片，客户端压缩、E2EE 后存入 R2/Google Drive；其他设备验证指纹与 Hash 后导入本地 sqlite-vec。

**可同步实体的版本/墓碑**：`agents/sessions/messages/explicit_memories/memory_store/workspaces` 均具备 `updated_at`、`deleted_at`、`version`、`origin_device_id`，配合后续事务性 outbox（HLC + `entity_version`）支撑离线同步与冲突解决。聊天消息正文完成后不可变；记忆、workspace 逻辑信息和 Agent 配置必须有版本。通用 settings 与 workspace 本地路径不作为同步实体。

迁移用 `rusqlite` + 自维护 `migrations/`（或 refinery）。

---

## 8. 记忆子系统落点（DB canonical + Markdown view）

| 层 | 实现位置 | 存储 |
|----|----------|------|
| ① Recent Context | Python `prompt.py`（用 ContextSnapshot.recentMessages） | SQLite `messages` |
| ② Conversation Summary | Python `prompt.py`（滚动压缩）→ Rust 写 `sessions.summary` | SQLite `sessions.summary` |
| ③ USER.md / MEMORY.md | Rust `memory/` 读写 | **DB 为真相源**；md 为导出给人看的 materialized view |
| ④ memory_store | Rust `memory_search` 混合检索；Python `memory_extract` 建议写与 LiteLLM embedding | SQLite `memory_store` + `embedding_items`/`vec_embeddings_{dims}` |

**本地索引维护规则**：每次 Agent 运行前，Rust 按 `embedding_id`、模型引用和正文 SHA-256
批量找出缺失或失效记录，再通过 Agent Protocol 请求 Python/LiteLLM 生成向量。写入使用动态
维度 sqlite-vec 表与 cosine 距离，Agent 和模型引用作为 partition key 在 KNN 前过滤；
字符串和向量候选以 RRF 融合。模型未配置或嵌入失败时
降级为字符串检索，不阻断 Agent 运行。向量及可信内部工具参数不会进入同步、审计或消息历史。

**USER.md / MEMORY.md 真相源规则**：SQLite（`settings` / `memory_store` / agent 记忆）是 canonical；`USER.md`/`MEMORY.md` 是其可读写视图。用户编辑 md → Rust 解析并**回写 DB**。这样同步与冲突解决才有单一归口，不被文件路径/mtime 干扰。

**记忆优先级（拼装顺序）**：
```text
1. 本轮用户输入
2. 当前会话 recent context
3. 当前会话 summary
4. 当前 Agent persona / memory
5. 当前 workspace / project memory
6. 全局 USER profile
7. 向量召回 memory_store
```
全局用户画像（`USER.md` 基底）**可关闭**：Agent 设置里可关掉"继承全局用户画像"，仅用 per-Agent 记忆。

---

## 9. 配置与密钥

- 用户设置存 `settings` 表；模型 API Key 等敏感信息存 **OS Keyring**（Tauri keyring 插件），不落明文、不进同步。
- 构建配置 `src-tauri/tauri.conf.json`；sidecar 发布态走 `bundle.externalBin`。

---

## 10. 同步层

- Rust `sync/` 使用事务性 `sync_outbox`，push 到 Cloudflare Worker（Hono/D1）并按 cursor 拉取远端变更；本地写入不等待网络。
- 规则：聊天消息 append-only；记忆、Agent 配置和日历/待办等 mutable entity 使用 version/HLC/墓碑做冲突解决；`device_id` 按安装生成。
- D1 另作为大对象和向量制品的控制面，保存 manifest/replica/device state；密文对象位于 R2/Google Drive，具体决策见 `STORAGE_AND_RAG.md`。

---

## 11. 错误处理与类型契约

- Rust 统一 `AppError`（kind + message），Tauri 序列化给前端；前端统一 toast/错误边界。
- Agent Protocol 消息以 Rust 结构体为真相源，`shared/protocol.md` 描述；TS 经 specta/tauri-specta 或 tauri-typegen 生成，Python 经 pydantic 镜像；三方不得各写各的。

---

## 12. 测试策略

- Rust：`cargo test` 覆盖 DbActor、Agent Protocol 编解码、ToolExecutor（含 policy 判定）、memory_search。
- Python：`pytest` 覆盖 graph、prompt 预算/压缩、memory_extract、tools schema。
- 前端：`vitest` + RTL 覆盖组件与 IPC 封装。
- 契约测试：Agent Protocol 消息 Rust⇄Python 往返一致性。

---

## 13. 构建 / 开发命令

```bash
# 前端 + Rust + 启动 Python sidecar（dev，AgentLauncher=DevUvLauncher）
pnpm tauri dev
# 打包（release，自动冻结并验证 agentd → Tauri externalBin）
pnpm tauri build
# 仅冻结并执行 sidecar 协议握手测试
pnpm build:sidecar
# 仅 Rust
cargo build / cargo test          # src-tauri
# Python sidecar（uv）
cd agent && uv sync && uv run python -m app.main
uv run pytest                     # agent/tests
# 前端单测
pnpm test                         # vitest
```
> 注：`agentd` 由 PyInstaller 在目标平台原生构建，不能直接跨平台冻结；交叉编译 Tauri 时需通过 `AGNES_SIDECAR_BINARY` 提供与目标 triple 匹配的预构建文件。

---

## 14. 最终决策表

| 决策点 | 选择 | 理由 |
|--------|------|------|
| sidecar dev | Rust 启动 `uv run python -m app.main` | 开发舒服，不污染前端权限 |
| sidecar release | Python 打成 externalBin（agentd） | 分发稳定，用户不用装 Python |
| Rust ↔ Python | Rust WS Server（127.0.0.1），Python Client | Rust 管生命周期/权限/端口/token |
| DB 驱动 | `rusqlite` + DbActor | 更适合本地 SQLite、FTS5、sqlite-vec |
| 前端状态 | Zustand + TanStack Query（+ RHF + Zod 表单） | 轻量够用，不上 Redux |
| UI | shadcn/ui + Tailwind | 现成可改组件，适配桌面工具 |
| USER.md | 保留，但 **DB 为真相源** | 方便同步与冲突解决 |
| memory summary | `sessions.summary` | summary 是会话级状态 |
| 工具权限 | capability + approval + sandbox + audit | 审批非唯一防线 |
| sync | append-only + version/tombstone | 为离线同步准备 |
| Android | 不跑 Python sidecar | 先做轻客户端 / SSH 控制器 |
| 包管理 | pnpm workspace（不用 turborepo） | 轻量 monorepo |

---

## 15. 后续扩展（不推翻本架构）

Android（V0.4 轻客户端+SSH）、Cloudflare 同步（V0.3）、MCP 外部工具（V0.5）、局域网 Web 控制台——均挂载在现有三平面边界之上，不重构核心。
