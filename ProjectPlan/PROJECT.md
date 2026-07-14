# PROJECT.md

项目规划与设计文档：技术栈、架构、数据模型、AGENTS 角色卡、记忆系统、版本路线图与关键决策。开发规范（语言、命令、代码质量）见 `CODEBUDDY.md`。

# 项目定位

`agnes-agent` 是一个**带 Agent 能力的酒馆式多角色聊天应用**（更接近 SillyTavern + Agents，而非纯 Agent 工具）。核心是可创建/管理多个 **AGENTS（角色卡）**，每个 Agent 有独立人设、系统提示词、工具权限与长期记忆；用户与某个 Agent 在 session 中对话，Agent 背后挂 Python LangGraph 运行时执行工具。**桌面端（Tauri）是真正的执行器**，负责本地文件、终端、Git、SSH、工具调用；**安卓端优先做轻客户端 + SSH 控制器**，不在 Android 内置 Termux/Ubuntu 跑完整 Agent（维护成本、权限、后台存活、依赖安装都太烦）。

# 技术栈

| 层 | 选型 |
|----|------|
| 客户端 | Tauri 2 + SvelteKit/React + TypeScript + Tailwind（shadcn/ui 或 Skeleton UI） |
| 本地核心 | Rust + Tokio + rusqlite/sqlx + portable-pty + ssh2/russh |
| Agent 运行时 | Python 3.12+ + LangGraph + LangChain tools + LiteLLM + MCP + FastAPI/WebSocket |
| 向量库 | 本地 SQLite + sqlite-vec（关注官方 Vec1 作为后续替换） |
| 云端同步 | Cloudflare Workers + Hono + D1 + Drizzle ORM |
| Embedding | BGE-M3 / Qwen Embedding / Ollama embeddinggemma（本地优先），Cloudflare Workers AI 也提供 BGE-M3 |
| 安卓 | Tauri Android（聊天/历史/记忆 → 后续 SSH 控制桌面 Agent） |

# 架构

```text
┌─────────────────────────────┐
│ Win/Linux Tauri Desktop App  │
│  Svelte/React UI             │
│  Rust Core                   │
│  SQLite + sqlite-vec         │
│  Python LangGraph Agent      │
│  PTY / Shell / Git / MCP     │
└──────────────┬──────────────┘
               │ sync（增量日志，不传向量）
               ▼
┌─────────────────────────────┐
│ Cloudflare Worker API        │
│  Hono + D1 + Auth            │
│  sync_log 冲突解决           │
└──────────────┬──────────────┘
               │ sync
               ▼
┌─────────────────────────────┐
│ Android Tauri App            │
│  Chat UI + 本地 SQLite 缓存  │
│  Optional: SSH 控制桌面 Agent │
└─────────────────────────────┘
```

数据流向：Tauri UI → (WebSocket/localhost) → Rust Core → (启动/管理 sidecar) → Python Agent Daemon → LiteLLM → 各模型厂商 → Tools（shell/file/git/browser/ssh/MCP）。

**工具层直接围绕 MCP 设计**，分内置工具（shell/file_read/file_write/git/ripgrep/python_exec/terminal_session/ssh_exec/browser_open）与外部工具（MCP server / OpenAPI adapter / 用户脚本），避免自造协议。

# 本地数据库（SQLite 表）

`agents / sessions / messages / message_parts / memory_store / memory_sources / embeddings / documents / document_chunks / tool_calls / sync_log / settings`

- `agents`：角色卡（人设/指令/默认模型/工具权限），见"AGENTS / 角色卡"。
- `sessions.agent_id` → `agents.id`（多对一：一个 Agent 可有多个 session，但每个 session 只属于一个 Agent）。
- `memory_store.agent_id`：长期记忆库按 Agent 隔离。

**同步策略**：云端只同步文本、设置、用户数据（agents/sessions/messages/memory_store/settings）；**向量不跨端同步，每台设备本地重新生成 embedding**（避免模型版本不一致、避免绑定 Cloudflare Vectorize）。`sync_log` 用 `device_id + lamport/vector clock` 做增量 push/pull + 冲突解决；聊天消息 append-only 很少冲突，记忆/设置才需 resolution。

# AGENTS / 角色卡

每个 Agent 是一张可编辑的"角色卡"（模仿 SillyTavern character card），用户定义人设等信息，运行时注入系统提示词。

`agents` 表建议字段：
```sql
id
name
persona            -- 人设 / description（自由文本）
scenario           -- 场景 / 世界观
system_prompt      -- 指令前缀，注入 system prompt
greeting           -- 开场白 first_mes
example_dialogue   -- 示例对话 mes_example
model              -- 该 Agent 默认使用的模型
tool_policy        -- JSON：启用的工具列表 + human-approval 模式
avatar
tags
created_at / updated_at
```

- **session ↔ agent**：多对一。一个 Agent 可有多个 session，每个 session 只属于一个 Agent（`sessions.agent_id`）。
- **长期记忆 ↔ agent**：每个 Agent 独立的 USER.md / MEMORY.md / memory_store（见记忆系统）。
- **注入顺序**：角色卡（persona + system_prompt + tool_policy）拼在 system prompt 最前，决定"这个 Agent 是谁、能做什么"。

# 记忆系统（四层）

**与 AGENTS 的关系**：一个 session 只属于一个具体 AGENTS（角色卡）；长期记忆（③+④）也按 AGENTS 隔离——每个 Agent 有自己独立的 USER.md / MEMORY.md / memory_store。因此"记忆"是 per-Agent 的，不是全局单一的。

```text
① Recent Context        — session 内，原文，短期（靠预算+压缩约束）
② Conversation Summary   — session 内，滚动压缩，短期
③ 必注入记忆(本地文本)    — USER.md + MEMORY.md，按 Agent 隔离，每次 session 直接进 prompt
④ 按需记忆库(向量+字符串) — 按 Agent 隔离，通过工具检索，不每次注入
```

## ①+② Session 内记忆（短期，切换 session 即消失）

长度靠"模型上下文能力 + 用户设置阈值"双重约束：
- 预算：`model_context_limit`（模型注册表，如 1M）与 `user_context_limit`（用户可选覆盖，默认 null）取有效上限；再扣掉 `reserved`（system+工具schema+输出预留）得 `usable_budget`。用户设 256K → 按 256K；用户不设 → 按模型能力。
- 触发：每个 turn 边界估算 `ratio = session_tokens / usable_budget`，超过 `compress_threshold`（默认 0.85）即压缩。
- 压缩：保留最近 `recency_window`（默认 20）条原文，把更老的消息 + 旧摘要交给 summarizer 模型滚动生成新摘要（②）。用户 `is_pinned` 消息、含"记住…"的消息受保护不被压。
- 长工具输出（大文件/长终端）落 session 前先 size cap：截断标注或写本地文件只留引用，避免爆窗。
- 切换 session：working_set 直接丢弃，原始 messages 仍在库；重载时重建，超预算立刻压缩。并发多 session 各占独立预算。
- 配置字段：`context_limit / compress_threshold / recency_window / reserved_output_tokens / summarizer_model`，模型注册表需 `max_context_tokens`。

## ③ 必注入记忆（本地文本文件，直接注入 prompt）

- 存储：`USER.md`（用户基础信息，**仅用户可改，AI 只读**）+ `MEMORY.md`（需每次都记住的事实，**AI 和用户都能改**）。纯文本 Markdown，人类可读、可 git、用户能直接手改。
- 作用域：按 Agent 隔离，路径 `~/.agnes/agents/{agent_id}/memory/USER.md` + `MEMORY.md`。可选：全局用户画像 `~/.agnes/memory/USER.md` 作为跨 Agent 共享基底，注入时与 per-Agent 的合并（若冲突以 per-Agent 为准）。
- 注入：session 启动读取，直接拼进 system prompt 每轮都在，作为稳定基底。
- 修改：用户手改或说"记住…" → AI 调 `memory_write` append 到 MEMORY.md（仅 append / 按区块编辑，不整体覆盖）。AI 改 USER.md 被工具层写保护禁止；需更新时提示用户。
- 原"Explicit Memory"即收敛为此处的 MEMORY.md：显式、高置信、每次必注入，无独立表。

## ④ 按需记忆库（SQLite + 向量 + 字符串，工具检索）

- 存储：`memory_store` 表（区别于 MEMORY.md 文件）：
  `id, agent_id, content, type(fact/project/preference/note/source), scope(global/agent), source, confidence, created_at, updated_at, embedding_id`；向量存 `embeddings`（sqlite-vec）。`agent_id` 保证记忆库按 Agent 隔离；`memory_search` 默认只在当前 Agent 范围内检索（可选并入 global 基底）。
- 检索（混合）：AI 按需调 `memory_search(q)` —— `A = sqlite-vec cosine top-k`（语义）+ `B = FTS5/LIKE 子串匹配 top-k`（精确 token）→ `fuse(A,B)`（RRF 或加权）返回 top-k，作为工具结果进上下文。
- 写入：后台 memory extractor 从对话抽事实入库 + 算 embedding；或用户"存进记忆库" → `memory_store_write`。推断项带 `confidence` + `source`，**可编辑、可删除、可解释来源**（量大且 AI 生成，是防记错的主战场）。

## Prompt 拼装顺序

```text
System Prompt
↓ Agent 角色卡（人设 / system_prompt / tool_policy）   ← 决定"这个 Agent 是谁、能做什么"
↓ 安全规则（基于 Agent 的 tool_policy）
↓ USER.md                （per-Agent，每次必注入，仅用户可改）
↓ MEMORY.md              （per-Agent，每次必注入，ai+用户可改）
↓ 当前项目上下文
↓ ② Conversation Summary
↓ ① Recent Context
↓ [按需] AI 调 memory_search（限定 agent_id）→ 检索结果作工具返回拼入
↓ 用户本轮输入
```

## 同步影响（沿用"向量不跨端同步"规则）

- USER.md / MEMORY.md：纯文本，随其他文本全量同步上云（D1）。
- memory_store 文本同步；embedding 各端本地重算，不跨端传向量（避免模型版本漂移）。

# 版本路线图

- **V0.1 先跑起来**：Tauri 2 桌面端 + SvelteKit 聊天 UI + 本地 SQLite + Python FastAPI Agent sidecar + LiteLLM + shell/file/git 三工具
- **V0.2 记忆与 RAG**：message summary + memory extractor + sqlite-vec + embedding provider 抽象 + prompt assembler
- **V0.3 Cloudflare 同步**：Workers + Hono + D1 schema + device_id + sync_log 增量 push/pull（端到端加密可选）
- **V0.4 Android**：Tauri Android 聊天/历史/记忆 + 云同步 + SSH 控制桌面 Agent
- **V0.5 Agent 增强**：MCP + human approval + diff review + workspace sandbox + tool audit log + 多模型 fallback

# 关键决策约束

- 项目定位是"带 Agent 能力的酒馆式多角色聊天"，不是纯 Agent 工具：核心是可创建多个 AGENTS（角色卡），每个有独立人设/系统提示词/工具权限/长期记忆。
- session 与 AGENTS 多对一；长期记忆（USER.md / MEMORY.md / memory_store）按 AGENTS 隔离，不是全局单一记忆。
- 桌面端是执行器，安卓端轻交互，不要在 Android 内置完整 Agent 作为 MVP。
- 向量库本地优先，不跨端同步；云端只同步文本与用户数据。
- 工具系统 MCP 优先，复用现有 MCP server 而非自造协议。
- LangGraph 用于多步 Agent / 检查点 / human-in-the-loop（敏感工具调用前暂停让用户审核）。
