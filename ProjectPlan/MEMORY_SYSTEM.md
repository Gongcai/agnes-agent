# 记忆系统详细设计

> 状态：V0.2 结构化字段、基础工具、记忆提示词工程与动态维度混合检索已实现
>
> 更新日期：2026-07-16

本文定义 `agnes-agent` 的长期记忆数据模型、检索行为和 AI 可用工具。总体分层与
prompt 顺序仍以 `PROJECT.md` 为准。

## 1. 两类长期记忆

长期记忆分为两个用途不同的存储面，不能混为同一种数据：

| 类型 | 存储 | 用途 | 注入方式 |
|---|---|---|---|
| 稳定记忆 | `MEMORY.md`，SQLite 为 canonical、Markdown 为本地视图 | 少量、每轮都必须知道的事实 | 每轮直接注入 system prompt |
| 结构化记忆库 | SQLite `memory_store` | 可增长、可检索、可管理的事实集合 | AI 按需调用 `memory_search`、`memory_create`、`memory_update` |

`USER.md` 仍是用户画像，只允许用户修改；AI 可以读取注入后的内容，但没有修改
`USER.md` 的工具。

## 2. 结构化记忆实体

### 2.1 用户可见字段

每条结构化记忆当前固定包含以下字段：

| 字段 | 必填 | 产生方式 | 说明 |
|---|---|---|---|
| `name` | 是 | 用户输入或 AI 提取 | 简短、可识别的记忆名称，去除首尾空白后不能为空 |
| `keywords` | 否 | 用户输入或 AI 提取 | 字符串数组，用于辅助简单字符串匹配；保存时去空、去重 |
| `created_at` | 是 | 系统自动生成 | 创建时写入，后续编辑不改变；UI 按本地时区展示 |
| `content` | 是 | 用户输入或 AI 提取 | 完整记忆内容，去除首尾空白后不能为空 |
| `creator` | 是 | 系统按入口判断 | 仅允许 `user` 或 `ai`，创建后不可由调用方伪造或修改 |

创建人判定规则：

- 用户从记忆管理界面新建：`creator=user`；
- Python memory extractor 或后续 AI 记忆写入工具创建：`creator=ai`；
- 导入旧数据：`creator=ai`，因为历史数据来自现有自动抽取链路；
- 编辑不改变原创建人。

### 2.2 内部字段

`id`、`agent_id`、`status`、`updated_at`、`version`、`deleted_at`、
`origin_device_id` 和 `embedding_id` 是系统字段。`id` 使用稳定 UUID，所有查询和修改
必须同时受当前 `agent_id` 约束。删除采用墓碑语义，为后续同步保留版本信息。

`keywords` 在 SQLite 中以 JSON 字符串保存，对外始终表现为字符串数组。第一阶段不
为关键词建立独立关联表，避免在字段和交互尚未稳定时过早复杂化。

## 3. 写入与管理

### 3.1 用户管理

记忆设置页提供列表、新建、编辑和删除：

- 新建时用户填写名称、可选关键词和内容；
- 编辑可以修改名称、关键词和内容，不能修改创建时间和创建人；
- 删除写入 `deleted_at` 并将 `status` 设为 `deleted`；
- 列表展示名称、关键词、创建时间、创建人和内容。

### 3.2 AI 自动抽取

memory extractor 返回 `name`、`keywords`、`content`、`confidence` 和 `source`。
Rust 收到结果后强制写入 `creator=ai`。抽取失败、向量写入失败或数据库写入失败必须
记录错误，不能静默伪装成成功。

在去重策略完善前，写入至少使用规范化后的 `agent_id + name + content` 检查完全重复；
完全重复项不重复创建。

### 3.3 AI 专用写入工具

AI 可以通过两个结构化记忆专用工具写入当前 session 所属 Agent 的记忆库：

- `memory_create(name, keywords?, content)`：系统生成 UUID 和时间，Rust 固定写入当前
  `agent_id`、`creator=ai`，不接受调用方传入这些系统字段；完全重复时拒绝创建；
- `memory_update(memory_id, name?, keywords?, content?)`：至少提供一个修改字段，先按
  `memory_id + 当前 agent_id` 读取已有记录，再合并字段并更新；保留 `creator` 和
  `created_at`，刷新 `updated_at`，同时令 `version + 1`。

两个工具均为 Medium 风险写操作，并受 Agent 的 memory capability 和会话权限模式控制。
`accept_edits`、`auto`、`full_access` 可直接执行，`ask_for_approval` 需要用户批准。

工具层只提供原子操作，不强制调用顺序。system prompt 会要求 AI 在写结构化记忆前先调用
`memory_search` 获取相关记忆：已有同一事实时按稳定 `id` 使用 `memory_update`，没有合适
记录时才使用 `memory_create`；结果含糊或冲突时继续检索或询问用户，不直接覆盖。该行为
策略保留在提示词层，不与 CRUD 实现耦合。

提示词同时区分两个存储面的用途：每轮都必须知道的少量高置信事实进入 `MEMORY.md`，其余
可按需召回的长期事实进入结构化记忆库；临时任务、原始工具输出、密钥和内部推理不应写入
记忆。

## 4. 检索

AI 使用 `memory_search(query, limit?)` 检索当前 Agent 的结构化记忆库。工具不得接受
`agent_id`，Agent 范围由当前 session 在 Rust 中解析，防止跨角色读取。

字符串检索覆盖：

1. `name` 精确或包含匹配；
2. `keywords` 包含匹配；
3. `content` 包含匹配。

比较不区分 ASCII 大小写。结果返回稳定 `id`、完整的名称、关键词、创建时间、内容和
创建人，但不返回 `agent_id`；`id` 可直接交给 `memory_update`。默认最多 10 条。关键词
只用于增强召回，不替代内容搜索。

配置全局“嵌入模型”分工后，系统还会只对每条记忆的 `content` 生成向量；`name` 和
`keywords` 继续只参与字符串召回。sqlite-vec 索引按模型实际返回的维度延迟创建
`vec_embeddings_{dims}` 虚拟表，使用 cosine 距离，支持 1 到 8192 维。向量元数据保存
`model`、`dims` 和内容 SHA-256，用于模型、维度及正文变化后的失效判断。

`memory_search` 同时取得字符串候选和同一 Agent、同一嵌入模型下的向量候选；Agent 与
模型引用作为 vec0 partition key 在 KNN 前过滤，再通过 RRF 融合排序并返回 top-k。名称、
关键词和正文的普通包含匹配因此不会被语义检索替代。嵌入
模型未配置或嵌入服务调用失败时，自动降级到字符串检索，不阻断对话或记忆写入。

每次 Agent 运行前，Rust 会批量检查当前 Agent 的记忆。缺少向量、正文哈希变化或模型
引用变化的记录会通过 Python/LiteLLM 回填；正文修改或记忆删除时会立即清理旧向量。
`memory_create`、`memory_update` 和自动抽取结果在本轮可用时直接携带可信内部向量，该字段
不会暴露给模型工具 schema，也不会写入工具审计、消息历史或前端事件。

记忆管理页按当前 Agent 显示向量覆盖信息条。有效覆盖必须同时满足当前嵌入模型引用和最新
正文 SHA-256，信息条展示当前模型、`已向量化/总数`、待处理数量与覆盖率。用户可手动触发
待处理记忆的批量向量化；按钮复用运行前自动回填链路，不重复处理已是最新的记录，并在完成
后返回本次处理数量和最新状态。

向量只存在于本机，不进入云同步；其他设备同步 `memory_store` 文本后使用各自配置的嵌入
模型重新生成。

## 5. MEMORY.md 专用工具

### 5.1 `memory_md_view`

- 无业务参数；
- 只读取当前 session 所属 Agent 的 `MEMORY.md`；
- 从 SQLite canonical 值读取，同时保持本地 Markdown 视图一致；
- 返回完整内容；
- 风险等级为 Low。

### 5.2 `memory_md_edit`

工具只允许两种受控修改，不提供任意路径，也不允许修改 `USER.md`：

- `append`：追加一段非空 Markdown；
- `replace`：用 `new_text` 精确替换唯一匹配的 `old_text`。未找到或匹配多次均拒绝，
  防止 AI 修改错误区块。

工具从当前 session 解析 `agent_id`，写入 SQLite canonical 值后再更新本地 Markdown
视图。修改成功仅返回变更摘要；AI 需要确认最终内容时再次调用 `memory_md_view`。

`memory_md_edit` 风险等级为 Medium。是否弹出人工审批由会话权限模式决定：

- `ask_for_approval`：需要批准；
- `auto`、`accept_edits`、`full_access`：自动执行；
- Agent 的 memory capability 被禁用时，无论何种模式都不能执行。

## 6. 验收标准

- 用户创建的记忆自动标记 `creator=user`，AI 抽取的记忆自动标记 `creator=ai`；
- 名称或内容为空时拒绝保存，关键词可为空；
- 同一 Agent 可按名称、关键词和内容检索，不能检索到其他 Agent 的记录；
- 配置嵌入模型后可通过正文语义召回，同一索引中不能召回其他 Agent 或其他模型的记录；
- 不同维度的嵌入模型可并存，模型切换或正文修改后会自动重建对应向量；
- 嵌入服务失败时仍可完成字符串检索和记忆写入；内部向量不得进入审计与消息历史；
- 记忆管理页能显示当前 Agent 的向量覆盖率，并可手动补齐待处理记录；
- 新会话中的 AI 能通过 `memory_search` 找到已有结构化记忆；
- AI 能通过 `memory_create` 新建 `creator=ai` 的记忆，并通过稳定 `id` 部分更新已有记忆；
- `memory_update` 保留创建时间和创建人、递增版本，不能更新其他 Agent 的记忆；
- AI 只能通过专用工具查看和受控修改当前 Agent 的 `MEMORY.md`；
- `memory_md_edit` 不能修改 `USER.md`、其他 Agent 记忆或任意文件；
- 应用重启后结构化记忆和 `MEMORY.md` 修改都保持一致；
- Rust、Python 和前端测试覆盖字段校验、创建人判定、检索隔离和工具 schema。
