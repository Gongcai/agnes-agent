# UI 设计文档（agnes-agent）

> 范围：桌面端（Tauri）主界面设计，覆盖 V0.1 骨架到 V0.5 的界面形态。
> 定位依据：`PROJECT.md`（酒馆式多角色 + Agent 能力）；架构依据：`architecture.md`（三平面、AGENTS/记忆/工具协议、Agent Protocol）。
> 本文档只定义**界面形态、信息架构、交互流、前端状态与 IPC 契约**；不改动后端架构。

---

## 1. 设计原则

1. **三平面边界在 UI 上可见**：UI 只通过 Tauri IPC 与 Rust 通信；凡涉及本地文件/Shell/Git/SSH/记忆库的读写，一律走 Rust 命令，UI 不直接碰系统。
2. **Agent 是中心对象**：界面围绕"AGENTS（角色卡）↔ session ↔ 长期记忆"展开，而非纯聊天框。
3. **Agent 行为透明**：工具调用、记忆检索、推理过程在右侧"活动"面板可追、可审、可中断（human-in-the-loop）。
4. **深色优先、可读、可改**：shadcn/ui + Tailwind，CSS 变量令牌化；Markdown/代码高亮原生支持。
5. **骨架先行、逐步填实**：V0.1 先把布局、导航、聊天流、审批卡、记忆视图的**外壳与 IPC 契约**落地；V0.2+ 再逐层接实数据。

---

## 2. 信息架构（IA）

```text
AppShell
├── 左栏 Sidebar（窄）
│   ├── 顶部：应用名 / 全局用户画像(USER.md 基底) / 设置
│   ├── AGENTS 列表（角色卡：头像+名+一句话人设）
│   │   └── 选中某 Agent → 展开其 Sessions 列表
│   └── + 新建 Agent / 导入角色卡
│
├── 主区 Main（宽）
│   ├── ChatHeader：Agent 名/头像 + 当前 session 标题(可改) + 模型徽标 + 活动/记忆开关
│   ├── MessageList：消息流（用户 / Agent / 工具卡 / 推理块）
│   └── Composer：多行输入 + 模型选择 + 发送 / 停止
│
└── 右栏 RightPanel（可折叠，Tab 切换）
    ├── 活动 Activity：当前 run 的时间线（推理 → 工具调用 → 记忆检索 → 结果）
    ├── 记忆 Memory：USER.md / MEMORY.md / memory_store 浏览器 / 会话摘要
    └── 上下文 Context：当前 Agent 注入的 projectContext（V0.2）
```

**默认布局**：三栏（侧栏 ~260px / 主区自适应 / 右栏 ~360px，可折叠）。窄屏下右栏改为抽屉，侧栏可收起为图标条。

```
┌──────┬───────────────────────────────┬──────────────┐
│ AGENTS│  ChatHeader: Alice · 会话1    │ 活动 | 记忆  │
│ ───── │ ───────────────────────────── │ ──────────── │
│ ● Alice│  User: 帮我看下 main.rs      │ ● 推理(隐)   │
│ ● Bob  │  Alice: 好的，我来读…        │ ● tool: read │
│       │  ┌─ ToolCall: file_read ──┐  │   审批: 允许 │
│ Sessions│ │ params: main.rs       │  │ ● memory_q  │
│  - 会话1│ │ [允许][拒绝][改参]     │  │             │
│  - 会话2│ └───────────────────────┘  │ USER.md     │
│ [+会话] │  Alice: 这里是 …（流式）    │ MEMORY.md   │
│       │                               │ 记忆库(12)  │
│ [+Agent]│  ┌────────────────────────┐ │ 摘要        │
│ 设置 ⚙ │  │ 输入框…            [发送]│ └────────────┘
└──────┴──┴────────────────────────┴──┴──────────────┘
```

---

## 3. 布局与导航

- **侧栏（Sidebar）**
  - AGENTS 列表项：头像 + 名称 + `persona` 前 1 行摘要；hover 出"编辑/删除"。
  - 选中 Agent 后，下方出现该 Agent 的 **Sessions 二级列表**（`sessions.agent_id` 多对一，见 `PROJECT.md`）：最近会话在上，底部"＋新建会话"。
  - 顶部"设置 ⚙"进入全局设置；"全局用户画像"按钮打开全局 `USER.md` 基底编辑器。
- **主区导航**：单 Agent 单 session 视图（V0.1）。切换 Agent = 切换左侧选中；切换 session = 左侧二级列表。后续 V0.x 支持顶部多 session 标签 / 多 Agent 分屏。
- **右栏**：默认显示"活动"；发送消息/有 run 时自动聚焦"活动"。用户可手动切"记忆""上下文"。可整体折叠（快捷键或 ChatHeader 开关）。

---

## 4. 视图详细设计

### 4.1 聊天主区（Chat）

- **消息项（MessageItem）**：按 `role` 区分。采用酒馆式左头像布局（Agent 头像在左，用户名+内容；用户头像在右或同样左对齐但用不同底色）。每条消息可为多 `message_parts`（`kind`: `text|reasoning|tool_call|tool_result`）。
- **流式（assistant_delta）**：`text` 部件实时追加光标；`reasoning` 部件默认折叠为"思考中…"，展开看完整思维链。
- **工具卡（ToolCallCard，内联）**：在消息流中**调用点原位**渲染（比模态更连贯），状态机：
  `pending_approval`（黄，出审批卡）→ `running`（蓝，转圈）→ `done`（绿，可展开 I/O）→ `rejected`（灰）→ `failed`（红，展开 stderr）。
  展示：工具名 + 一句话参数摘要 + sandbox 范围（cwd / risk / 超时 / 输出上限）。
- **停止**：run 进行中 Composer 发送按钮变"停止"（`cancel_run`）。
- **空状态**：显示 Agent `greeting`（first_mes）+ 几条建议提问气泡。

### 4.2 工具审批卡（Human-in-the-loop）

对应协议 `tool_call_request`(Py→Rust) → `tool_call_pending`(Rust→React 事件) → `approve_tool`(React→Rust) → `approval_result`(Rust→Py)。UI 提供：
- **允许 / 拒绝** 两个主操作。
- **改参**（可选）：展开参数表单（按工具 schema 生成），用户修改后再允许。
- **展示策略快照**：`cwd`、`allowedCwd`、`approval` 模式、`timeoutSec`、`maxOutputBytes`、`risk_level`——让用户知道这次执行会被限制在哪、多久、出多少（capability + approval 双保险，见 `architecture.md` §6）。
- 免审批工具（`approval: never`）不弹卡，直接 `running`。

### 4.3 右栏 · 活动（Activity Timeline）

当前 `runId` 的纵向时间线，实时随事件刷新：
- 推理步骤（折叠）
- 工具调用节点（pending/approved/running/done/failed，点开看参数与 I/O）
- 记忆检索节点（`memory_query_request`：查询词 + 返回条数）
- 来源：`agent://assistant_delta` / `agent://tool_call_pending` / `agent://tool_result` / `agent://run_finished` 等事件驱动。
- 价值：让 Agent 的"黑盒"变成可追、可中断、可审计（对应 `tool_calls` 审计表）。

### 4.4 右栏 · 记忆（Memory）

四个子区，对应"四层记忆"（见 `PROJECT.md`）中**可用户操作**的部分：

| 子区 | 对应层 | 权限 | 说明 |
|------|--------|------|------|
| USER.md | ③ 必注入·用户基底 | **仅用户可改**，AI 只读 | 全局基底 + per-Agent；可关"继承全局"（Agent 设置里） |
| MEMORY.md | ③ 必注入·事实 | 用户 + AI 都能改 | 高置信、每次必注入；AI 经 `memory_write` 仅 append/区块编辑 |
| 记忆库 browser | ④ 按需 memory_store | 可浏览/搜/增/改/删/归档 | 类型 fact/project/preference/note/source；显示 `confidence`/`source`/`last_used_at`；搜索走混合检索 |
| 会话摘要 | ② Conversation Summary | 可查看/手动重算 | 显示 `sessions.summary`，可"重新压缩" |

- 记忆库 browser：搜索框（走 `search_memory` 混合检索：sqlite-vec 向量 + FTS5 字符串，RRF 融合）→ 结果列表；每条可展开看完整 content、来源消息、置信度、删除/归档。
- 编辑 USER.md / MEMORY.md 实时 `save_user_md` / `save_memory_md`（DB 为真相源，见 `architecture.md` §8）。

### 4.5 AGENT 角色卡编辑器

独立视图（路由或模态），字段直接映射 `agents` 表（`PROJECT.md` §AGENTS/角色卡）：
- 基础：`name`、`avatar`（上传/生成）、`tags`、`persona`、`scenario`
- 提示词：`system_prompt`、`greeting`(first_mes)、`example_dialogue`(mes_example)
- 运行时：`model`（从模型注册表下拉）、`tool_policy`（结构化编辑器，非裸 JSON，见下）
- **tool_policy 编辑器**：按工具（shell/file/git/ssh/browser/memory_search…）逐项配置 `enabled` / `approval`(always|write|push|never) / `allowedCwd` / `allowedRoots` / `timeoutSec` / `maxOutputBytes` / `envAllowlist`。结构化表单，避免手写 JSON 出错。

### 4.6 设置（Settings）

- **模型供应商**：LiteLLM 支持的 provider 列表；API Key 经 **OS Keyring**（Tauri keyring 插件）存取，UI 只显示"已配置/未配置"，不回显明文（`architecture.md` §9）。
- **全局工具策略**：作为新 Agent 的默认 `tool_policy` 模板。
- **上下文预算默认**：`context_limit` / `compress_threshold`(默认 0.85) / `recency_window`(默认 20) / `reserved_output_tokens` / `summarizer_model`。
- **同步**（V0.3 占位）：Cloudflare/D1 开关与设备名，当前禁用并标注"即将到来"。
- **外观**：明暗主题、字号、密度（comfortable/compact）。

### 4.7 全局用户画像（Global USER.md）

`~/.agnes/memory/USER.md` 基底编辑器（跨 Agent 共享，注入时与 per-Agent 合并，冲突以 per-Agent 为准）。入口在侧栏顶部。可整体关闭继承（Agent 设置内）。

---

## 5. 关键交互流（时序）

### 5.1 发送一条消息（完整 run）

```text
用户 Composer 发送
  → React send_message(session_id, content)            [命令]
  → Rust: 落 messages(role=user) + 生成 ContextSnapshot + 发 run_request(Py)
  → Rust 事件 agent://run_started {runId}
  → React 新建 run 时间线节点（右栏·活动）
  → Py: 推理 → 流式 assistant_delta
      → Rust 事件 agent://assistant_delta {messageId, delta}
      → React 实时追加到对应 message 的 text 部件
  → Py: 需要工具 → tool_call_request
      → Rust 按 tool_policy 判审批
         需审批 → 事件 agent://tool_call_pending {toolCall}
                  → React 在消息流原位渲染审批卡（§4.2）
                  → 用户批准 → approve_tool → Rust 回 approval_result(Py)
         免审批 → 直接执行
      → Rust ToolExecutor 执行 → 事件 agent://tool_result {toolCallId, result}
                  → React 工具卡变 done，右栏时间线更新
  → Py: 继续推理…可能 memory_query_request（右栏记一笔）
  → Py: run_finished → 事件 agent://run_finished
  → Rust 落 messages(role=assistant, status=complete) + 触发 memory_extract 建议
  → React 收尾（停止按钮复位）
```

### 5.2 记忆检索（推理中按需）

```text
Py memory_query_request {q, topK}
  → Rust memory_search（向量+FTS5 融合）→ 结果回 Py 作工具返回
  → React 右栏活动记一条"记忆检索：q → N 条"
```

### 5.3 停止

Composer"停止" → `cancel_run(runId)` → Rust 发 `run_cancel`(Py) → Py 中断 LangGraph → 事件 `agent://run_finished`（带 cancelled 标记）→ React 复位。

---

## 6. 前端状态管理

- **Zustand**（ ephemeral / 流式 / UI 状态）：
  - `useAppStore`：theme、sidebarCollapsed、rightPanelTab、currentView
  - `useAgentsStore`：agents、currentAgentId
  - `useSessionsStore`：byAgent 映射、currentSessionId
  - `useChatStore`：messages、streaming、runStatus、toolCalls（id→状态）、composerDraft
  - `useMemoryStore`：userMd、memoryMd、memoryEntries、summary、加载态
- **TanStack Query**（异步取数，缓存+失效）：`list_agents` / `get_messages` / `list_memory` / `search_memory` / `get_settings` 等命令的封装；Rust 推送 `agent://memory_changed` / `agent://agents_changed` 时 `invalidate` 对应查询。
- **表单**：react-hook-form + zod（角色卡编辑、tool_policy 编辑、设置）。

> 不引入 Redux（决策表 `architecture.md` §14）。

---

## 7. IPC 接口契约（React ↔ Rust）

> 命名沿用 `architecture.md` §3.1。命令用 Tauri `invoke`；事件用 `listen`（`tauri://` 或 `agent://` 前缀）。Rust 结构体为类型真相源，TS 经 `tauri-specta`/`tauri-typegen` 生成，避免手写重复类型。

### 7.1 命令（React → Rust）

**Agents**
- `list_agents() → AgentSummary[]`
- `get_agent(id) → Agent`
- `create_agent(payload) → Agent`
- `update_agent(id, payload) → Agent`
- `delete_agent(id)`
- `import_agent_card(file) → Agent`  （SillyTavern 卡格式兼容，V0.2）

**Sessions**
- `list_sessions(agent_id) → SessionSummary[]`
- `get_session(id) → Session`
- `create_session(agent_id, title?) → Session`
- `rename_session(id, title)`
- `delete_session(id)`
- `get_messages(session_id, before?, limit?) → Message[]`

**Chat / Run**
- `send_message(session_id, content) → { runId }`
- `cancel_run(run_id)`

**Memory**
- `get_user_md(agent_id, scope?) → string`
- `save_user_md(agent_id, content, scope?)`
- `get_memory_md(agent_id) → string`
- `save_memory_md(agent_id, content)`
- `list_memory(agent_id, filter?) → MemoryEntry[]`
- `search_memory(agent_id, query, top_k?) → MemoryEntry[]`
- `create_memory(agent_id, payload) → MemoryEntry`
- `update_memory(id, payload)`
- `delete_memory(id)`
- `get_summary(session_id) → string`
- `regenerate_summary(session_id)`

**Tools / Policy**
- `get_tool_policy(agent_id) → ToolPolicy`
- `update_tool_policy(agent_id, policy)`
- `get_tool_call(id) → ToolCall`
- `list_tool_calls(session_id) → ToolCall[]`

**Model / Settings**
- `get_settings() → Settings`
- `update_settings(patch)`
- `list_models() → ModelInfo[]`            （模型注册表，含 max_context_tokens）
- `get_provider_status() → {provider: bool}[]`
- `set_provider_key(provider, key)`        （写 OS Keyring，不回显）

**App**
- `ping() → string`
- `get_app_info() → {version, db_path, ...}`

### 7.2 事件（Rust → React）

沿用架构命名并补全（括号内为驱动来源）：

| 事件 | payload | 用途 |
|------|---------|------|
| `agent://run_started` | `{runId, sessionId}` | 新建时间线节点 |
| `agent://token` | `{runId, messageId, delta}` | 流式文本（架构 §3.1 原 `agent://token`） |
| `agent://message_part` | `{messageId, part}` | 追加 reasoning/tool_call/tool_result 部件 |
| `agent://tool_call_pending` | `{sessionId, runId, toolCall}` | **弹审批卡**（架构 §3.1） |
| `agent://tool_result` | `{toolCallId, status, result?}` | 工具卡/时间线状态更新（架构 §3.1） |
| `agent://memory_changed` | `{agentId}` | 失效记忆查询，刷新记忆面板 |
| `agent://run_finished` | `{runId, sessionId, cancelled?}` | 收尾、复位停止键 |
| `agent://run_error` | `{runId, error}` | 错误条 |
| `agent://error` | `{message}` | 全局错误 toast（架构 §3.1） |
| `agent://agents_changed` | — | 失效 `list_agents` |
| `agent://sessions_changed` | `{agentId}` | 失效该 Agent 的 sessions |

> 注：`agent://token` 即架构中的流式 token 事件；本文档为清晰在内部称为 `assistant_delta`，对外事件名保持 `agent://token` 以兼容协议文档。

---

## 8. 设计系统

- **基础**：shadcn/ui + Tailwind。`tailwind.config.js` 接 CSS 变量令牌（`background`/`foreground`/`muted`/`border`/`accent`/`destructive`/`ring`/`radius`）。
- **主题**：深色优先（SillyTavern 风），提供浅色变体；令牌化便于切换。
- **字体**：UI 用 Inter / system-ui；代码与工具 I/O 用等宽（JetBrains Mono / `ui-monospace`）。
- **Markdown 渲染**：`react-markdown` + `remark-gfm`；代码高亮 `rehype-highlight`（或 shiki，V0.2 升级）。
- **图标**：`lucide-react`（已在依赖中）。
- **密度**：默认 comfortable；设置内可切 compact。
- **头像**：图片优先，缺省用首字母 + 渐变底色生成。

---

## 9. 组件清单（树）

```text
<AppShell>
├─ <TitleBar>                # 自定义标题栏（应用名 / 全局用户 / 设置）
├─ <Sidebar>
│  ├─ <AgentList> → <AgentListItem>
│  ├─ <SessionList> → <SessionListItem>
│  └─ <NewAgentButton> / <ImportAgentButton>
├─ <MainArea>
│  ├─ <ChatHeader>           # Agent/会话/模型/活动·记忆开关
│  ├─ <MessageList> → <MessageItem>
│  │  ├─ <MessagePartText>   # Markdown 渲染
│  │  ├─ <ReasoningBlock>    # 折叠思维链
│  │  └─ <ToolCallCard>      # 内联审批卡 + 状态机（§4.2）
│  └─ <Composer>             # 输入 + 模型选择 + 发送/停止
├─ <RightPanel>
│  ├─ <ActivityTimeline>     # run 时间线（§4.3）
│  └─ <MemoryPanel>
│     ├─ <UserMdEditor>      # 仅用户可改
│     ├─ <MemoryMdEditor>    # 用户+AI 可改
│     ├─ <MemoryStoreBrowser># 搜索/列表/增删改（§4.4）
│     └─ <SessionSummaryView>
├─ <AgentEditor>             # 角色卡 + tool_policy 编辑（§4.5，路由/模态）
├─ <SettingsDialog>          # 模型/Keyring/预算/同步/外观（§4.6）
└─ <Toaster>                 # 错误边界与 toast
```

---

## 10. 响应式与 Android 轻客户端

- **桌面优先**（V0.1 主目标，Tauri 桌面端是执行器）。
- 断点：≥1280px 三栏完整；768–1280px 右栏转抽屉、侧栏可收；<768px 单栏 + 底部 Tab（聊天/记忆/设置）。
- **Android（V0.4 轻客户端）**：本设计的前端组件**天然可复用**为轻客户端 UI；但 Android 不跑 Python sidecar（架构约束），其聊天/历史/记忆走云端同步缓存，工具执行经 SSH 控制桌面 Agent。故 Android 视图是"只读+触发"形态，不在本端执行工具——组件层用同一套，数据层走同步缓存而非本地 Rust。

---

## 11. 实施分阶段（前端）

- **V0.1（本次骨架）**：AppShell + Sidebar（AGENTS/Sessions 列表，接 `list_agents`/`list_sessions`） + ChatHeader + MessageList（text 流式，接 `agent://token`） + Composer（`send_message`/`cancel_run`） + 右栏外壳（Activity/Memory Tab 占位） + 错误 toast。先把 IPC 封装（`src/lib/ipc.ts`）与 Zustand/TanStack 骨架就位。
- **V0.2 记忆与 RAG**：MemoryPanel 四个子区全接实；Activity 时间线接 `tool_call_pending`/`tool_result`/`run_finished`；`search_memory` 混合检索 UI。
- **V0.3 同步**：设置里同步面板；Android 轻客户端复用组件。
- **V0.4 Agent 增强**：tool_policy 编辑器、审批卡"改参"、MCP 工具、diff review、审计日志视图。

---

## 12. 开放问题 / 待确认

1. **默认明暗主题**：提议深色（酒馆习惯）。是否要浅色默认或跟随系统？
2. **右栏默认 Tab**：提议"活动"（run 时自动聚焦）。还是默认"记忆"？
3. **会话切换形态**：侧栏二级列表（当前提议） vs 主区顶部标签栏？多 session 多了之后哪种更顺手？
4. **多 Agent 同屏**：V0.1 单 Agent 单 session。是否需要在 V0.x 支持 @切换 / 分屏多 Agent 对话？
5. **工具审批形态**：内联卡（提议）+ 右栏时间线，是否够？还是也要模态强阻断？
6. **移动端时机**：桌面优先到 V0.4 再出 Android，还是提前做响应式骨架？

请就以上 1–6 拍板，确认后我再据此进入具体组件实现（从 V0.1 骨架开始）。
