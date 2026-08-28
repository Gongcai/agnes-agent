# PROJECT.md

项目规划与设计文档：技术栈、架构、数据模型、AGENTS 角色卡、记忆系统、版本路线图与关键决策。开发规范（语言、命令、代码质量）见 `CODEBUDDY.md`。

记忆系统的实体字段、文件导航行为与 AI 工具契约详见 `ProjectPlan/MEMORY_SYSTEM.md`；正式运行时规则见 `ProjectPlan/MEMORY_FILESYSTEM_PROMPT.md`。

Read With AI 的 EPUB、划线与讨论上下文边界详见 `ProjectPlan/READ_WITH_AI.md`。

模型能力标签与任务分工详见 `ProjectPlan/MODEL_ROUTING.md`。

联网搜索、网页读取与 MCP 分阶段设计详见 `ProjectPlan/WEB_AND_MCP.md`。

Skills 的目录格式、安装、运行时加载与安全边界详见 `ProjectPlan/SKILLS.md`。

DOCX、PPTX、XLSX、PDF 的结构化解析、独立 sidecar 与 PDF 可选模型包详见 `ProjectPlan/DOCUMENT_PARSING.md`。

大文件、RAG、加密向量制品、多网盘、日历和待办详见 `ProjectPlan/STORAGE_AND_RAG.md`。

Linux 桌面端的标准发行构建、Arch Linux AppImage 兼容处理与产物验收详见 `ProjectPlan/LINUX_RELEASE.md`。

同步 E2EE 的密码学格式、AAD、keyset 与上线门禁详见 `ProjectPlan/E2EE.md`。

Android 本地完整 Agent、PRoot/Ubuntu Runtime、Android Service、工作区互通与分阶段验收详见 `ProjectPlan/ANDROID_RUNTIME.md`。

# 项目定位

`agnes-agent` 是一个**带 Agent 能力的酒馆式多角色聊天应用**（更接近 SillyTavern + Agents，而非纯 Agent 工具）。核心是可创建/管理多个 **AGENTS（角色卡）**，每个 Agent 有独立人设、系统提示词、工具权限与长期记忆；用户与某个 Agent 在 session 中对话，Agent 背后挂 Python LangGraph 运行时执行工具。**桌面端和安卓端都是执行器**：桌面端在宿主操作系统运行 Agent，安卓端通过独立 Linux Runtime 在本地运行完整 Agent。SSH 作为连接桌面 Agent 的兼容能力保留，但不是安卓端的唯一执行路径。

# 技术栈

| 层 | 选型 |
|----|------|
| 客户端 | Tauri 2 + SvelteKit/React + TypeScript + Tailwind（shadcn/ui 或 Skeleton UI） |
| 本地核心 | Rust + Tokio + rusqlite/sqlx + portable-pty + ssh2/russh |
| Agent 运行时 | Python 3.12+ + LangGraph + LangChain tools + LiteLLM + MCP + FastAPI/WebSocket |
| 向量库 | 本地 SQLite + sqlite-vec（关注官方 Vec1 作为后续替换） |
| 云端同步 | Cloudflare Workers + Hono + D1 + Drizzle ORM |
| 大对象存储 | 本地内容库 + Cloudflare R2 / Google Drive 加密副本 |
| Embedding | BGE-M3 / Qwen Embedding / Ollama embeddinggemma（本地优先），Cloudflare Workers AI 也提供 BGE-M3 |
| 安卓 | Tauri Android + Android Service + PRoot/Ubuntu ARM64（本地完整 Agent；SSH 作为桌面远程 adapter） |

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
               │ sync（结构化增量 + 大对象清单）
               ▼
┌─────────────────────────────┐
│ Cloudflare Worker API        │
│  Hono + D1 + Auth            │
│  sync_changes / manifest CAS │
└──────────────┬──────────────┘
               │ sync
               ▼
┌─────────────────────────────┐
│ Android Tauri App            │
│  Chat UI + Rust Core         │
│  SQLite + Sync + E2EE        │
│  Android Runtime Service     │
│  PRoot + Ubuntu + Python Agent│
│  Optional: SSH 桌面远程模式  │
└─────────────────────────────┘
```

数据流向：Tauri UI → (WebSocket/localhost) → Rust Core → (启动/管理 sidecar) → Python Agent Daemon → LiteLLM → 各模型厂商 → Tools（shell/file/git/browser/ssh/MCP）。

**工具层直接围绕 MCP 设计**，分内置工具（shell/file_read/file_write/git/ripgrep/python_exec/terminal_session/ssh_exec/browser_open）与外部工具（MCP server / OpenAPI adapter / 用户脚本），避免自造协议。

结构化小数据通过 Worker/D1 增量同步；源文件、附件和向量制品在客户端压缩/E2EE 后存入 R2 或 Google Drive。D1 只保存这些大对象的 manifest、密文 Hash、Provider 副本和设备落地状态。

# 本地数据库（SQLite 表）

`agents / sessions / messages / message_parts / memory_files / memory_file_revisions / memory_file_conflicts / workspace_bindings / embeddings / knowledge_collections / documents / document_versions / document_chunks / blob_objects / calendars / calendar_events / task_lists / tasks / tool_calls / sync_outbox / settings`

- `agents`：角色卡（人设/指令/默认模型/工具权限），见"AGENTS / 角色卡"。
- `sessions.agent_id` → `agents.id`（多对一：一个 Agent 可有多个 session，但每个 session 只属于一个 Agent）。
- `memory_files.agent_id`：长期记忆虚拟文件系统按 Agent 隔离；SQLite 是唯一真相源。

**同步策略**：云端只同步白名单内的结构化用户实体（agents/sessions/messages/memory_files/workspaces/calendars/calendar_events/event_exceptions/task_lists/tasks，文件清单后续扩展）；通用 settings 和本地记忆修订保持设备本地。**原始向量行和 sqlite-vec 表不进 D1**：每台设备可本地重建，也可在 embedding/parser/chunker 指纹完全一致时从 R2/Google Drive 下载客户端加密的便携 RAG 向量制品。D1 只保存最新版本、密文 Hash、Provider 副本和设备落地状态。事务性 outbox 使用 revision CAS、`device_id + HLC` 做增量 push/pull；聊天消息正文完成后不可变，记忆文件并发修改不静默 LWW，而是保留 base/local/remote 后合并。

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
- **长期记忆 ↔ agent**：每个 Agent 有独立的持久记忆虚拟文件系统（见 `MEMORY_SYSTEM.md`）。
- **注入顺序**：角色卡（persona + system_prompt + tool_policy）拼在 system prompt 最前，决定"这个 Agent 是谁、能做什么"。

# 记忆系统（四层）

**与 AGENTS 的关系**：一个 session 只属于一个具体 Agent；长期记忆（③+④）也按 Agent 隔离。每个 Agent 拥有一套 SQLite-backed 虚拟 Markdown 文件系统，不存在全局画像隐式继承，也不与其他 Agent 共用记忆。

```text
① Recent Context          — session 内最近消息原文，受预算约束
② Conversation Summary    — session 内较早消息的滚动摘要
③ Always-on Memory Index  — <profile> + <preferences> + <memory_listing>
④ On-demand Memory Files  — /topics、/areas、/people 正文，通过工具按需读取
```

## ①+② Session 内记忆（短期，切换 session 即消失）

长度靠"模型上下文能力 + 用户设置阈值"双重约束：
- 预算：`model_context_limit`（模型注册表，如 1M）与 `user_context_limit`（用户可选覆盖，默认 null）取有效上限；再扣掉 `reserved`（system+工具schema+输出预留）得 `usable_budget`。用户设 256K → 按 256K；用户不设 → 按模型能力。
- 触发：每个 turn 边界估算 `ratio = session_tokens / usable_budget`，超过 `compress_threshold`（默认 0.85）即压缩。
- 压缩：保留最近 `recency_window`（默认 20）条原文，把更老的消息 + 旧摘要交给 summarizer 模型滚动生成新摘要（②）。用户 `is_pinned` 消息、含"记住…"的消息受保护不被压。
- 长工具输出（大文件/长终端）落 session 前先 size cap：截断标注或写本地文件只留引用，避免爆窗。
- 切换 session：working_set 直接丢弃，原始 messages 仍在库；重载时重建，超预算立刻压缩。并发多 session 各占独立预算。
- 配置字段：`context_limit / compress_threshold / recency_window / reserved_output_tokens / summarizer_model`，模型注册表需 `max_context_tokens`。

## ③ Always-on Memory Index

- `USER.md` 是每个 Agent 的用户管理、AI 只读基础资料，每轮以 `<user_profile>` 固定注入；正文为空时默认继承“用户信息”页生成的基础资料，用户可选择始终继承或不继承。
- `/profile.md` 保存稳定身份背景，正文以 `<profile>` 注入；`/preferences.md` 保存合法的交互偏好，正文以 `<preferences>` 注入。
- `<memory_listing>` 注入当前 Agent 每个活跃文件的虚拟 path、description、aliases 和 sources，不注入其他正文。
- USER/profile/preferences/listing 四个动态块每轮从同一 SQLite snapshot 生成；不存在时明确标记 `(not provided)`、`(not yet written)` 或 `(empty)`。
- listing 不得因上下文预算截断；通过单 Agent 文件数和元数据长度上限控制规模。

## ④ On-demand Memory Files

- 路径固定为 `/topics/<domain>.md`、`/areas/<name>.md` 和 `/people/<name>.md`；每个主题只归档一次，通过 `[[name]]` 交叉引用。
- AI 通过 `memory_list / memory_read / memory_write / memory_str_replace / memory_append / memory_delete` 操作，不接触宿主机路径，也不能传 `agent_id`。
- Markdown frontmatter 固定为 `name / description / sources / aliases`；chat surface 只能新写用户明确说过的 `[stated]` 事实。
- 所有修改必须带 read 返回的 opaque version token，以 CAS 防止多会话和多 surface 覆盖；整文件删除还必须有本轮用户明确授权。
- 不提供向量检索，不运行后台 extractor。模型在当前轮按 listing 读取相关主题，并在事实出现时立即写入；写入失败不阻断主要回答。
- SQLite `memory_files` 是 canonical 真相源；有限 revisions/conflicts 用于本地撤销和冲突恢复。详细 schema、工具契约、隐私边界和测试见 `MEMORY_SYSTEM.md`，逐条模型行为以 `MEMORY_FILESYSTEM_PROMPT.md` 为准。

## Prompt 拼装顺序

```text
System Prompt
↓ Agent 角色卡（人设 / system_prompt / tool_policy）   ← 决定"这个 Agent 是谁、能做什么"
↓ 安全规则（基于 Agent 的 tool_policy）
↓ MEMORY_FILESYSTEM_PROMPT.md（正式记忆行为规则）
↓ <profile> + <preferences> + <memory_listing>（per-Agent snapshot）
↓ Home 注入通用共享工作区纪律；Code 注入编码工作流（工作区范围、先读项目指令、工具与验证纪律）
↓ 当前项目上下文
↓ [按本轮选择] Active Skills（低于系统安全规则与 Tool Policy）
↓ ② Conversation Summary
↓ ① Recent Context
↓ [按需] AI 调 memory_read（agent_id 由 session 解析）→ 文件正文作为工具结果
↓ 用户本轮输入
```

提示词调试面板按实际模型请求结构分别展示 `system prompt`、`tools` schema 和 `messages`；
工具定义通过模型 API 的 `tools` 参数发送，不重复拼进 system prompt。

Home 普通会话共享一个设备本地的通用工作区，首选系统 Documents 下的 `Agnes/Home`（Linux 尊重 XDG Documents，Windows 使用系统 Documents/可能的 OneDrive 重定向路径），不可用时回退到应用本机数据目录；它面向文档、表格、下载与转换产物，不默认假设软件仓库。Code 的编码工作流仅在 `session.workspace_id` 指向有效逻辑工作区时注入。两种模式都不向模型暴露本机绝对路径；未绑定目录的同步 Code 工作区会明确禁止本地文件、终端和 Git 操作，并提示用户先完成本机绑定。

## 同步影响

- `memory_file` 是唯一记忆同步实体，path 与 Markdown 正文均经客户端 E2EE 后进入 D1 增量协议。
- 服务端使用 revision CAS；跨设备并发修改保留 base/local/remote，不静默 LWW。有限修订和 UI FTS 只保存在本机。
- 记忆系统不再生成 embedding；知识库/RAG 的原始向量仍默认各端本地重算，大型 RAG 向量可作为指纹严格匹配的加密制品在 R2/Google Drive 间复用，但不进入 D1 payload。

# 知识库、网盘与个人效率子功能

- RAG 文档归属独立 `knowledge_collection`，通过授权分配给一个或多个 Agent，不按 Agent 重复存储源文件和向量。
- D1 是文件/制品同步控制面；R2 / Google Drive 是加密大对象数据面；sqlite-vec 是本地检索引擎。
- 网盘通过窄化的文件源、上传、文件管理、配额和对象存储 Provider 端口接入；Google Drive 使用官方 API，夸克通过可替换的 `quark_drive` community adapter 接入并要求用户显式启用。Google Drive 和夸克均已完成真实账户授权、目录层级、上传下载、移入回收站和知识库/书架导入验收；Google Drive 支持 Workspace 文档导出，夸克另支持单项或批量移动。R2 继续作为应用托管的加密对象副本。
- 日历和待办是用户级结构化数据，默认仅使用 Local Provider，Agent 读写受 tool policy 约束；外部 `CalendarProvider / TaskProvider` adapter 保留为未来按实际需求启用的扩展，不属于当前自用、国内本地优先路线。
- 主界面侧边栏将调整为子功能列表 + 可折叠聊天会话 + 可折叠工作区会话，详见 `UI_DESIGN.md`。

# 版本路线图与当前进度

| 版本 | 范围 | 当前状态（2026-07-27） |
|---|---|---|
| V0.1 | Tauri 2 + React 聊天 UI + SQLite + Python LangGraph sidecar + LiteLLM | 已完成：开发态通过 `uv` 启动，发布态使用 PyInstaller 冻结为按 target triple 命名的 `agentd` 并由 Tauri `externalBin` 内置；发布构建会自动执行独立 WebSocket 协议握手测试，安装后不依赖 Python 或 `uv` |
| V0.2 | message summary + 持久记忆文件系统 + prompt assembler | 会话摘要已完成；旧结构化/向量记忆实现将直接删除并按 `MEMORY_SYSTEM.md` 重构为虚拟 Markdown 文件系统，不做数据迁移。正式行为提示词为 `MEMORY_FILESYSTEM_PROMPT.md` |
| V0.3 | Cloudflare Workers + D1 + 事务性 outbox + 增量同步 + E2EE | Phase 0-4 已完成：密文传输、SPAKE2 新设备配对、两阶段密钥轮换、Recovery Bundle 多版本恢复和线上日志审计均已完成；Worker 已部署，真实桌面端已用测试数据完成 E2EE 同步、手动排空和断点续推验收。远端当前仅保留测试设备与测试同步数据，未上传真实业务内容 |
| V0.4 | Android 基础客户端 + 云同步 + E2EE + AgentRuntime 接口 | 开发中：Tauri Android scaffold、ARM64 debug APK、移动 drawer、Android library target、Rustls 和 Runtime 抽象已完成；真机/双设备验收、Keystore、SAF 绑定待完成 |
| V0.5 | MCP + Skills + diff review + workspace sandbox + tool audit + 多模型 fallback | 已完成桌面端基础闭环：工具、审批、Linux 沙箱、审计、模型路由、内置 `web_search/web_fetch/browser_open`、可配置 SearXNG/Brave Search Provider、MCP Client、主模型零输出故障回退、兼容 `SKILL.md` 的本地/Git 安装与运行时资源边界，以及 `file_write/file_edit/apply_patch` 批准前变更预览均已接入；沙箱支持规范化软链接的额外可写根目录，`uv`/pnpm/Cargo 等开发工具可复用真实用户缓存和工具链目录；浏览器为隔离无登录的严格只读渲染能力。diff review 当前按整次文件工具调用批准或拒绝，不预测 Shell/Git/MCP 的任意副作用，也不提供逐 hunk 选择 |
| V0.6 | 侧边栏子功能导航 + 知识库 + 本地 RAG + 加密向量制品 + 通用网盘 Provider | 代码阶段已完成：本地 RAG、chunks/embeddings artifact 导出与事务性 sqlite-vec 导入、artifact v1 加密/校验/原子安装、SQLite manifest/replica/device 状态、Google Drive 对象副本编排、R2 Worker Multipart/Range 数据面、Rust object 控制面客户端及独立 artifact replication coordinator 已完成；知识库已通过独立轻量 `document-parserd` 支持 DOCX/PPTX/XLSX 结构化解析，并通过显式安装的离线模型包支持 PDF 版面、表格和中英文 OCR，保留标题、页码、幻灯片、工作表、表格范围和解析器指纹；知识库支持按文档构建、缓存和发布 R2 加密向量制品，失败重试复用同一 artifact，并在页面展示向量覆盖、ready 副本和远端设备安装状态；本地制品存储提供默认 2 GiB 配额、容量统计、手动清理和定时安全 GC，只回收已有 ready 远端副本的缓存或旧安装目录；聊天 pull cursor 与 object cursor 已分离。Worker 已接入 R2 孤儿副本对账、7 天宽限期、有界定时 GC 与中断重试，契约测试覆盖分片校验、owner 隔离、逻辑版本 CAS、幂等重放、设备落地状态、过期上传和孤儿回收。Google Drive 与夸克真实账户基础链路、文件管理和知识库/书架导入均已验收，Google Workspace 文档导出与 token 刷新也已通过；真实 R2 bucket、D1 migration、Worker 部署、知识制品发布、重复复用、密文对象下载 Hash 和本机设备状态均已验收；跨设备 Range 下载与原子安装仍待安卓端完成后统一验证 |
| V0.7 | 本地日历 + 待办 | 已完成：本地域模型、Local Provider、受审批约束的 Agent 工具、完整桌面工作区、本地统一通知服务和 D1 E2EE 同步均已完成。日历支持月/周/日/议程、多日历叠加、当天议程、待办图层及重复 occurrence 例外；待办支持五类智能视图、自定义列表、重要/我的一天、日期或精确时间、步骤和完成后生成下一重复实例；通知中心覆盖 AI 回复/许可、任务到期和日历事件。Google Calendar / Google Tasks / CalDAV 不纳入默认路线图，未来仅按实际需求作为可选扩展评估。 |
| V0.8 | Read With AI | 已完成本地优先 v1：EPUB 2/3 导入与原样渲染、阅读进度、CFI 划线、用户级书架、按书籍 × Agent 隔离的连续讨论、已知书籍模式，以及未知书籍首次书内检索授权。现已接入加密 `reading_epub` artifact：书架可发布 EPUB 到 R2，`reading_book`/`reading_highlight` 通过 E2EE 同步，远端校验并原子安装原书后重建本机检索索引；未安装书籍显示元数据和划线但不可打开。真实跨设备安装验收仍待第二设备。 |

# 关键决策约束

- 项目定位是"带 Agent 能力的酒馆式多角色聊天"，不是纯 Agent 工具：核心是可创建多个 AGENTS（角色卡），每个有独立人设/系统提示词/工具权限/长期记忆。
- session 与 AGENTS 多对一；长期记忆虚拟文件系统按 Agent 隔离，不是全局单一记忆，也不隐式继承全局画像。
- 桌面端和安卓端都可以是执行器；安卓端通过独立 Linux Runtime 运行完整 Agent，第一版可以先交付轻客户端能力。
- Android 的 PRoot/Ubuntu Runtime、Service 生命周期、工作区互通和验收按 `ANDROID_RUNTIME.md` 分阶段实施。
- 向量库本地优先，不跨端同步；云端只同步文本与用户数据。
- 上一条中“向量不跨端同步”专指原始向量行不进 D1/服务端检索；大型 RAG 允许以客户端加密、指纹完全匹配的便携制品存入 R2/Google Drive。
- Provider API Key、同步凭证和 E2EE 主密钥只进入 OS Keyring / Android Keystore，不进入 SQLite、renderer 明文 IPC 或同步 payload。
- 外部存储/日历/待办使用应用定义的端口和 Provider adapter；域服务不直接依赖 Google/R2/夸克/CalDAV SDK 类型。
- 同步 payload 必须由 Rust 按实体字段白名单投影；不得直接序列化业务表，也不得扫描全部 settings 猜测同步范围。
- 工具系统 MCP 优先，复用现有 MCP server 而非自造协议。
- LangGraph 用于多步 Agent / 检查点 / human-in-the-loop（敏感工具调用前暂停让用户审核）。
