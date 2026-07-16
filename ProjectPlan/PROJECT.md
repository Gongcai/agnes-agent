# PROJECT.md

项目规划与设计文档：技术栈、架构、数据模型、AGENTS 角色卡、记忆系统、版本路线图与关键决策。开发规范（语言、命令、代码质量）见 `CODEBUDDY.md`。

记忆系统的实体字段、检索行为与 AI 工具契约详见 `ProjectPlan/MEMORY_SYSTEM.md`。

模型能力标签与任务分工详见 `ProjectPlan/MODEL_ROUTING.md`。

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

`agents / sessions / messages / message_parts / explicit_memories / memory_store / workspace_bindings / embeddings / documents / document_chunks / tool_calls / sync_log / settings`

- `agents`：角色卡（人设/指令/默认模型/工具权限），见"AGENTS / 角色卡"。
- `sessions.agent_id` → `agents.id`（多对一：一个 Agent 可有多个 session，但每个 session 只属于一个 Agent）。
- `memory_store.agent_id`：长期记忆库按 Agent 隔离。

**同步策略**：云端只同步白名单内的文本与用户实体（agents/sessions/messages/explicit_memories/memory_store/workspaces）；通用 settings 保持设备本地。**向量不跨端同步，每台设备本地重新生成 embedding**（避免模型版本不一致、避免绑定 Cloudflare Vectorize）。后续事务性 outbox 使用 `device_id + HLC` 做增量 push/pull 与冲突解决；聊天消息正文完成后不可变，记忆和角色配置才需字段级冲突处理。

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

- 存储：SQLite `explicit_memories` 是 canonical 真相源，每个 Agent 固定以 `user_md / memory_md` 两种 kind 保存 `USER.md`（用户基础信息，**仅用户可改，AI 只读**）和 `MEMORY.md`（需每次都记住的事实，**AI 和用户都能改**）；本地 Markdown 文件是可读、可编辑的物化视图。
- 作用域：按 Agent 隔离，路径 `~/.agnes/agents/{agent_id}/memory/USER.md` + `MEMORY.md`。可选：全局用户画像 `~/.agnes/memory/USER.md` 作为跨 Agent 共享基底，注入时与 per-Agent 的合并（若冲突以 per-Agent 为准）。
- 注入：session 启动读取，直接拼进 system prompt 每轮都在，作为稳定基底。
- 修改：用户手改或说"记住…" → AI 调 `memory_md_edit` 追加或精确替换，并可用 `memory_md_view` 再次查看。AI 改 USER.md 被工具层写保护禁止；需更新时提示用户。
- 原"Explicit Memory"即收敛为 `explicit_memories` 中的 USER.md/MEMORY.md 文档实体：使用稳定 UUID、版本和墓碑参与同步，不再混入通用 settings。

## ④ 按需记忆库（SQLite + 向量 + 字符串，工具检索）

- 存储：`memory_store` 表（区别于 MEMORY.md 文件）：
  用户可见字段固定为 `name / keywords? / created_at / content / creator(user|ai)`；另有 `id / agent_id / status / version / embedding_id` 等内部字段。`agent_id` 保证记忆库按 Agent 隔离，详细约束见 `MEMORY_SYSTEM.md`。
- 检索（混合）：AI 按需调 `memory_search(q)`。名称、关键词和内容参与字符串匹配；配置嵌入模型后，仅对 `content` 建立按实际维度分表的 sqlite-vec cosine 索引，并以 RRF 融合字符串与同 Agent、同模型的向量候选，返回 top-k 作为工具结果进入上下文。模型未配置或向量调用失败时自动降级为字符串检索。
- 索引维护：每次 Agent 运行前批量回填缺失、正文变化或模型切换后的记忆向量；正文修改和删除会清理旧向量。记忆管理页按当前 Agent 展示当前模型、覆盖率和待处理数量，并允许手动触发同一批量回填链路。sqlite-vec 支持的维度范围为 1 到 8192，向量仅保存在本机，不参与云同步。
- 写入：后台 memory extractor 从对话抽取，或 AI 调用 `memory_create` / `memory_update` 写入当前 Agent 的结构化记忆；Rust 强制 AI 创建入口标记 `creator=ai`，用户从记忆管理界面创建时强制标记 `creator=user`。创建时间和创建人均由系统生成，不接受调用方伪造，更新时保持不变。
- AI 工具边界：`memory_search` 返回稳定 `id` 供 `memory_update` 使用，但不返回 `agent_id`；`memory_create` 和 `memory_update` 都从当前 session 解析 Agent，不能跨 Agent 写入。提示词已要求 AI 写入前先检索相关记忆，再判断新增或更新；基础工具层不强制该调用顺序。
- `MEMORY.md`：AI 使用 `memory_md_view` 再次查看，使用 `memory_md_edit` 进行追加或唯一精确替换；工具只能操作当前 Agent 的 `MEMORY.md`，不能修改 `USER.md` 或任意文件。

## Prompt 拼装顺序

```text
System Prompt
↓ Agent 角色卡（人设 / system_prompt / tool_policy）   ← 决定"这个 Agent 是谁、能做什么"
↓ 安全规则（基于 Agent 的 tool_policy）
↓ 记忆决策规则（先检索再新增/更新，区分 MEMORY.md 与结构化记忆库）
↓ USER.md                （per-Agent，每次必注入，仅用户可改）
↓ MEMORY.md              （per-Agent，每次必注入，ai+用户可改）
↓ 当前项目上下文
↓ ② Conversation Summary
↓ ① Recent Context
↓ [按需] AI 调 memory_search（限定 agent_id）→ 检索结果作工具返回拼入
↓ 用户本轮输入
```

提示词调试面板按实际模型请求结构分别展示 `system prompt`、`tools` schema 和 `messages`；
工具定义通过模型 API 的 `tools` 参数发送，不重复拼进 system prompt。

## 同步影响（沿用"向量不跨端同步"规则）

- USER.md / MEMORY.md：纯文本，随其他文本全量同步上云（D1）。
- memory_store 文本同步；embedding 各端本地重算，不跨端传向量（避免模型版本漂移）。

# 版本路线图与当前进度

| 版本 | 范围 | 当前状态（2026-07-16） |
|---|---|---|
| V0.1 | Tauri 2 + React 聊天 UI + SQLite + Python LangGraph sidecar + LiteLLM | 主链路已完成；发布态 sidecar 打包待收口 |
| V0.2 | message summary + memory extractor + 结构化记忆库 + sqlite-vec + prompt assembler | 已完成：摘要、抽取、结构化字段、AI 创建/更新、记忆决策提示词、`MEMORY.md` 专用工具、动态维度 sqlite-vec + RRF 混合检索；已使用 Qwen3-Embedding-8B 完成真实服务端到端验证，手动向量化、覆盖率统计与检索链路均可用 |
| V0.3 | Cloudflare Workers + D1 + 事务性 outbox + 增量同步 + E2EE | 进行中：Phase 0 数据边界、Phase 1 Worker/D1 和 Phase 2 Agent/Session 事务性 push 已完成；远端启用每设备 Bearer 指纹映射，真实 Rust SyncService 假 Agent 端到端验证通过并已清空远端业务表。下一步进入 Phase 3 pull/bootstrap、消息/记忆同步与类型化冲突合并 |
| V0.4 | Tauri Android 聊天/历史/记忆 + 云同步 + SSH 控制桌面 Agent | 未开始 |
| V0.5 | MCP + diff review + workspace sandbox + tool audit + 多模型 fallback | 工具、审批、Linux 沙箱、审计和模型路由已提前实现；MCP 等能力待后续补齐 |

# 关键决策约束

- 项目定位是"带 Agent 能力的酒馆式多角色聊天"，不是纯 Agent 工具：核心是可创建多个 AGENTS（角色卡），每个有独立人设/系统提示词/工具权限/长期记忆。
- session 与 AGENTS 多对一；长期记忆（USER.md / MEMORY.md / memory_store）按 AGENTS 隔离，不是全局单一记忆。
- 桌面端是执行器，安卓端轻交互，不要在 Android 内置完整 Agent 作为 MVP。
- 向量库本地优先，不跨端同步；云端只同步文本与用户数据。
- Provider API Key、同步凭证和 E2EE 主密钥只进入 OS Keyring / Android Keystore，不进入 SQLite、renderer 明文 IPC 或同步 payload。
- 同步 payload 必须由 Rust 按实体字段白名单投影；不得直接序列化业务表，也不得扫描全部 settings 猜测同步范围。
- 工具系统 MCP 优先，复用现有 MCP server 而非自造协议。
- LangGraph 用于多步 Agent / 检查点 / human-in-the-loop（敏感工具调用前暂停让用户审核）。
