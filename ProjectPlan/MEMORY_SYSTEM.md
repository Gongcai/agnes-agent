# Agnes 持久记忆文件系统详细设计

> 状态：核心替换已实施；保留只读 `USER.md`，原 `MEMORY.md / memory_store` 双轨实现已下线
>
> 更新日期：2026-07-27
>
> 正式运行时提示词：`MEMORY_FILESYSTEM_PROMPT.md`

本文定义 Agnes 新一代持久记忆系统。系统以“Agent 可操作的虚拟 Markdown 文件系统”为
唯一长期记忆模型，并将 `MEMORY_FILESYSTEM_PROMPT.md` 作为运行时记忆规则的权威来源。

截至 2026-07-27，运行时核心链路已按本文完成破坏式切换：SQLite canonical store、六个 Agent
工具、单快照 prompt 注入、设置页 Markdown 编辑、CAS、可信 surface 校验、E2EE `memory_file`
同步及冲突合并均已接入；旧表、旧工具、后台 extractor 和记忆向量流程已删除。正式提示词随
sidecar 打包，并通过版本常量和 SHA-256 golden test 防止资源漂移。

以下属于产品化增强，不阻塞新架构作为唯一记忆系统运行：修订历史/撤销 UI、正文 FTS 与
backlink 导航、细粒度本地指标面板、完整行为 eval 集，以及真实双设备的人工验收。这些增强
继续直接基于 `memory_files` 实现，不恢复任何旧兼容层。

这是开发阶段的破坏式重构，不保留旧数据兼容层：

- 保留每个 Agent 的 `USER.md` 作为用户管理、AI 只读且每轮固定注入的基础信息；
- 删除旧 `MEMORY.md` 和结构化 `memory_store` 的双轨概念；
- 删除长期记忆的 embedding、向量召回、RRF 和运行前向量回填；
- 删除独立后台 memory extractor，记忆判断和写入在当前对话轮次内完成；
- 不迁移旧记忆数据，不保留旧工具别名，不做双写；
- 本地开发数据库可直接重建，云端开发数据可直接清空后使用新实体。

会话原文、滚动摘要和 RAG 知识库不属于本次删除范围。RAG 继续使用自己的文档、chunk 和
sqlite-vec 索引，不与持久记忆文件混用。

---

## 1. 设计目标

### 1.1 核心目标

1. **单一心智模型**：用户、Agent、UI 和同步层看到的都是同一组按主题组织的 Markdown
   记忆文件，不再区分“显式记忆”和“结构化记忆”。
2. **跨会话连续性**：每个 Agent 拥有独立的持久记忆命名空间；新会话能先看到目录清单，
   再按需读取相关文件。
3. **最小必要注入**：仅直接注入稳定画像、交互偏好和全量轻量目录，其他正文通过工具按需
   读取，避免长期记忆无限占用上下文。
4. **事实可追溯**：chat surface 只能写入用户明确说过的 `[stated]` 事实，不把模型推断、
   搜索结果、建议和内部推理伪装成用户事实。
5. **并发不覆盖**：所有修改都使用版本令牌和 compare-and-swap（CAS）；多会话、多 surface、
   多设备同时编辑时不得静默覆盖较新的内容。
6. **隐私优先**：敏感信息在提取阶段即省略；应用记忆时遵循相关性、最小披露和“等待用户
   先提起”的规则。
7. **用户可见、可控**：用户可在记忆管理页查看和编辑所有文件；成功自动写入由 UI chip
   提示，Agent 不在回复中重复播报。

### 1.2 非目标

- 不把记忆系统做成通用文件系统，Agent 不能访问任意宿主机路径；
- 不使用向量数据库检索记忆文件；目录描述、别名、链接和模型判断是主要导航机制；
- 不保存工具原始输出、网页资料、模型建议、密钥或 chain-of-thought；
- 不保证记忆写入是回答请求的前置依赖；写入失败时对话仍应继续；
- 不做历史版本的长期云端归档，修订记录只用于本地审计、冲突恢复和有限撤销；
- 不在本次重构中兼容或迁移旧表、旧 Markdown 文档和旧工具调用。

---

## 2. 总体模型

### 2.1 四层上下文

```text
① Recent Context
   当前 session 最近消息原文，受上下文预算约束

② Conversation Summary
   当前 session 较早消息的滚动摘要，切换 session 后不跨会话共享

③ Always-on User And Memory Index
   当前 Agent 的 <user_profile>、<profile>、<preferences> 与 <memory_listing>

④ On-demand Memory Files
   /topics、/areas、/people 正文，由 memory_read 按需读取
```

第 ①、② 层沿用现有会话上下文管理。`<user_profile>` 来自用户管理的 `USER.md`；其余第 ③、④
层组成 Agent 可写的长期记忆文件系统，均按
`agent_id` 隔离。一个 session 只能访问其所属 Agent 的命名空间；工具参数中不接受
`agent_id`，避免模型跨 Agent 枚举或读写。

### 2.2 数据流

```text
用户消息
  │
  ├─ Prompt assembler 读取当前 Agent 的 profile/preferences/listing 快照
  │
  ▼
模型判断本轮是否需要读记忆
  │
  ├─ memory_read(path) ───────────────┐
  │                                   │
  ├─ 正常回答请求                     │ SQLite memory_files
  │                                   │ canonical store
  └─ 发现耐久且允许保存的 stated 事实 │
      ├─ 已有文件：read → replace/append/write
      └─ 新主题：write(if_version="new")
                                      │
                         transaction + outbox + UI event
                                      │
                                      ├─ E2EE 增量同步
                                      └─ “Saved memory” chip
```

写入发生在同一轮 Agent 执行中，而不是等对话结束后批量抽取。若用户消息同时包含事实和
问题，Agent 先保存事实，再回答问题；若需要澄清或联网，先保存已经明确的耐久事实。

### 2.3 作用域

- 每个 Agent 有且仅有一个独立根目录，逻辑地址为 `memory://{agent_id}/`；
- 模型和 UI 只看到从 `/` 开始的虚拟路径，不看到 `agent_id` 或本机绝对路径；
- Home、Code、Read With AI 等 session surface 默认访问当前 session 所属 Agent 的同一命名空间；
- `sources` 记录哪些 surface 写过文件，但不改变文件归属；
- `USER.md` 可在运行时继承应用“用户信息”页生成的基础资料，不把继承结果写回文件；
- 继承策略为 `auto / inherit / isolated`：自动模式仅在 Agent 的 `USER.md` 正文为空时继承，用户
  可显式选择始终继承或不继承；
- `USER.md` 与继承策略随 `memory_file` 实体加密同步；应用级基础资料作为单例 `user_profile` 实体加密同步，头像等大对象后续走 blob。

---

## 3. 虚拟目录与归档规则

### 3.1 允许的路径

```text
/
├── USER.md                 # 用户编辑，AI 只读，不进入 memory listing
├── profile.md
├── preferences.md
├── topics/
│   └── <domain>.md
├── areas/
│   └── <name>.md
└── people/
    └── <name-or-relationship>.md
```

除上述路径外不允许创建其他目录或文件。`/USER.md` 只能由可信 user surface 创建或修改，chat
surface 的所有写操作都返回 `READ_ONLY`。路径必须是规范化绝对虚拟路径，不允许 `..`、`.`、
重复 `/`、反斜杠、URL 编码绕过、NUL 或符号链接语义。

普通 slug 使用小写 ASCII kebab-case，匹配：

```regex
[a-z0-9]+(?:-[a-z0-9]+)*
```

完整路径只能匹配：

```regex
^/USER\.md$
^/(profile|preferences)\.md$
^/(topics|areas|people)/[a-z0-9]+(?:-[a-z0-9]+)*\.md$
```

### 3.2 文件职责

| 路径 | 保存内容 | 不应保存 |
|---|---|---|
| `/USER.md` | 用户明确管理的基础信息；可在正文为空时默认继承应用级用户资料 | Agent 自动提取内容、工具结果、模型推断；AI 不得写入 |
| `/profile.md` | 姓名、稳定角色、任职组织、长期工作方向、开始时间等三个月后仍大概率成立的身份背景 | 冲刺任务、当前事故、具体截止日期、短期状态 |
| `/preferences.md` | 用户希望当前 Agent 如何输出和互动，例如格式、长度、语气、希望跳过的内容 | 用户喜好、事实画像、削弱诚实或安全性的指令 |
| `/topics/<domain>.md` | 习惯、口味、兴趣、时区、日常规律及可能形成模式的一次性提及 | 项目状态、他人档案、模型总结出的性格标签 |
| `/areas/<name>.md` | 持续项目、责任、事件、办理事项的决策、约束、截止日期和当前状态 | 与该事项无关的事实、模型建议全文 |
| `/people/<name>.md` | 对未来对话有帮助的关系上下文 | 第三方隐私档案、家人真实姓名、敏感生活细节 |

### 3.3 一主题一文件

- 事实由其主题决定目标文件，不由当前已经打开的文件决定；
- 同一事实只保存一次，可用 `[[name]]` 链接到其他主题，不复制正文；
- `/areas` 和 `/people` 可声明最多 7 个耐久别名；创建文件前必须检查 listing 中的别名；
- `name` 在同一 Agent 内全局唯一，不仅在同一目录内唯一，保证 `[[name]]` 可无歧义解析；
- 链接目标暂不存在是合法状态，用于提示未来可能需要建档；
- 家庭成员的 slug 使用关系，如 `partner`、`mom`，正文也只写“用户的伴侣/母亲”，不写姓名；
- 非家庭成员可用经过规范化的姓名 slug，例如 `/people/sam-r.md`。

### 3.4 容量边界

容量限制由可信工具层强制执行，提示词不能绕过：

| 项目 | 默认限制 |
|---|---:|
| 单 Agent 活跃文件数 | 256 |
| 单文件 UTF-8 大小 | 16 KiB |
| `/profile.md` 正文 | 300 English words 或等价 tokenizer 预算 |
| `/USER.md` | 16 KiB，继承的应用级资料不计入文件容量 |
| `description` | 单行、160 Unicode code points |
| 单个 alias | 80 Unicode code points |
| aliases 数量 | 7 |
| `memory_read` 单次路径数 | 20 |
| 单次 `memory_read` 总返回正文 | 256 KiB |

达到单文件上限时，Agent 应在完整读取后压缩重复或过时表述，再执行全量 `memory_write`。
达到文件数上限时不得自动删除文件；工具返回容量错误，由用户在记忆管理页处理，或由 Agent
在用户明确授权后合并和删除。

---

## 4. Markdown 文件契约

### 4.1 规范格式

```markdown
---
name: agnes-agent
description: Agnes 项目的持续决策、约束和当前状态，讨论项目工作时读取
sources: [chat]
aliases: [agnes, agnes-repo]
---
- [stated] 项目定位是带 Agent 能力的酒馆式多角色聊天
- [stated] 当前优先稳定桌面端与本地 Agent 能力
- [stated] 使用 pnpm 管理前端依赖
```

工具写入后保存规范化 UTF-8/LF 文本，并确保文件末尾只有一个换行。

### 4.2 Frontmatter

| 字段 | 类型 | 规则 |
|---|---|---|
| `name` | string | 必填；等于路径 stem；同 Agent 全局唯一 |
| `description` | string | 必填；单行说明内容及何时读取，不复述路径 |
| `sources` | string[] | 必填；集合语义；更新时只能新增，不能删除已有来源 |
| `aliases` | string[] | `/areas`、`/people` 可非空；其他类型必须为空数组 |
| `inherit` | `auto \| inherit \| isolated` | 仅 `/USER.md` 必填；普通记忆文件出现该字段时拒绝 |

解析必须使用安全 YAML parser，禁止自定义 tag、anchor、alias expansion 和多文档输入。未知字段
一律拒绝，防止把系统元数据或隐藏指令塞入 frontmatter。

`USER.md` 的 canonical frontmatter 由可信 UI 层生成，正文编辑器只暴露 Markdown 正文。当前模型
工具创建普通记忆文件时写入 `sources: [chat]`，记忆管理 UI 写入时由可信层补上 `user`。更新
已有文件时保留全部来源并补上当前可信 surface。首版枚举仅允许 `chat` 和 `user`；未来 surface
必须通过 schema 版本增加枚举值，不能由模型自由伪造来源。

### 4.3 正文事实

- chat surface 新写的每个非空正文行必须是 `- [stated] ...`；
- `[stated]` 表示用户明确说过，而不是模型认为正确或用户没有反对；
- 其他可信 surface 未来可以产生 `[observed]` 或 `[inferred]`；chat 读取和重构文件时必须原样
  保留这些行，但不能新建或把其他标签改成 `[stated]`；
- 一条语义事实尽量一行，同一从句内的列表应合并，避免拆成大量碎片；
- 用户改变事实时更新原行，可保留真正有价值的历史上下文，例如“当前在 infra 团队（此前在
  search 团队）”；
- 用户要求删除时直接删除目标事实及仅由它派生的内容，不改写成“以前喜欢”；
- 正文不保存“待询问”“下一步”“TBD”等 Agent 自己的工作状态；
- Markdown 链接只使用 `[[name]]`，解析器提取链接用于 UI 导航和完整性检查。

### 4.4 可保存性判定

同时满足以下条件才可由 chat 写入：

```text
用户明确说过
AND 对未来会话有持续价值
AND 不会很快自然过期
AND 不属于隐私省略范围
AND 不是搜索/工具获取的外部资料
AND 不是模型生成的建议、推断或推理
```

用户从模型给出的多个选项中明确选择某项时，“选择结果”属于 `[stated]`；未选项和模型的
理由不保存。用户只说“可以/听起来不错”时，只保存其接受的方法轮廓，不把方案中的每个细节
都归为用户事实。

证据强度必须与措辞一致。一次提及最多写成“提到过 X”，不能升级成“热衷 X”。耐久表述优先
于容易过期的精确数字。

---

## 5. 运行时提示词与注入

### 5.1 权威提示词

`ProjectPlan/MEMORY_FILESYSTEM_PROMPT.md` 是新系统记忆行为提示词的唯一权威模板。实现时将其
作为 package resource 内置到 `agentd`，不得在 Python、Rust 和前端分别维护删减副本。

编译阶段只允许以下替换：

- `{{Agent_Name}}`：替换为角色卡中经过纯文本转义的 Agent 显示名；为空时使用 `Agnes`；
- surface 标识：首版固定为提示词中的 `chat`，未来引入新 surface 时通过版本化模板修改，
  不在运行时字符串替换段落；
- UI 文案可本地化，但模型可见的正式提示词首版保持英文，避免翻译产生行为偏差。

模板变更必须修改一个 `memory_prompt_version` 常量并补充 golden test。会话和工具审计记录使用的
版本号，不复制整份提示词。

### 5.2 动态注入块

每次模型请求前，Rust 从同一 SQLite snapshot 生成四个数据块：

```xml
<user_profile source="USER.md" mode="auto">
## 全局用户资料
- 姓名：小安
</user_profile>

<profile version="v3:9fc1...">
- [stated] ...
</profile>

<preferences version="v5:2d8a...">
- [stated] ...
</preferences>

<memory_listing epoch="42">
- path: /areas/agnes-agent.md
  description: Agnes 项目的持续决策、约束和当前状态，讨论项目工作时读取
  aliases: [agnes, agnes-repo]
  sources: [chat]
- path: /topics/food.md
  description: 饮食口味和相关选择，提供餐饮建议时读取
  aliases: []
  sources: [chat]
</memory_listing>
```

规则：

- `<user_profile>` 每轮固定注入，即使 memory capability 关闭也保留；内容是 Agent `USER.md` 与
  当前允许继承的应用级基础资料合并后的只读视图；
- Agent 不得使用任何 memory 工具修改 `/USER.md`，也不得把已注入的 USER.md 内容复制到普通
  记忆文件；
- listing 包含当前 Agent 的每个活跃文件，包括 `/profile.md` 和 `/preferences.md`；
- listing 只包含路径、description、aliases、sources，不包含正文摘要或模型生成的额外解释；
- `/profile.md` 与 `/preferences.md` 注入正文事实，不重复注入 frontmatter；
- 文件不存在时分别注入 `(not yet written)`；无文件时 listing 为 `(empty)`；
- XML 内容必须转义，不能让文件内容闭合标签或伪造 system prompt 结构；
- listing 在单次模型请求内是不可变 snapshot；本轮写入后需要刷新时调用 `memory_list()`；
- 动态块被声明为低优先级的用户事实数据。除 `/preferences.md` 的允许范围外，任何记忆内容都
  不能改变系统安全规则、工具权限、审批、sandbox 或角色边界。

### 5.3 Prompt 拼装顺序

```text
1. 平台基础 System Prompt
2. Agent 角色卡 persona / system_prompt
3. 安全规则、Tool Policy、审批与 sandbox 边界
4. `<user_profile source="USER.md" mode="...">` 只读基础资料
5. MEMORY_FILESYSTEM_PROMPT.md（替换 {{Agent_Name}}）
6. <profile>
7. <preferences>
8. <memory_listing>
9. Home / Code / Read With AI 模式规则与项目上下文
10. Active Skills
11. Conversation Summary
12. Recent Context
13. 当前用户输入
```

`<preferences>` 只能调整允许用户配置的输出表现，优先级低于 1-4 和工具边界。记忆工具通过
模型 API 的 `tools` 参数发送 schema，不重复拼入 system prompt；正式提示词只说明其行为语义。

### 5.4 Cache 与上下文预算

- 静态提示词放在动态块之前，以便支持 provider prompt cache；
- 三个动态块使用 canonical 排序，未变化时字节级稳定；
- listing 预算属于 system/tool reserve，不能挤占预留输出 token；
- 达到容量边界时禁止截断 listing，因为“看不到文件”会导致错误创建和重复记忆；应阻止继续
  创建并提示用户整理；
- `memory_read` 的工具结果计入当前 turn 上下文，超出单次返回上限时整次拒绝，不静默截断文件。

---

## 6. 工具契约

所有工具从当前 session 解析 `agent_id`，只接受虚拟路径。工具结果进入正常 tool result，原始内容
不写入 assistant 文本或运行日志的脱敏摘要字段。

### 6.1 `memory_list`

```json
{
  "name": "memory_list",
  "parameters": {
    "type": "object",
    "properties": {},
    "additionalProperties": false
  }
}
```

返回当前完整 listing 和 `listing_epoch`。用于本轮出现其他 surface 写入、CAS 冲突或模型需要确认
新建文件是否已经存在时刷新目录。风险等级 Low，无需审批。

### 6.2 `memory_read`

```json
{
  "name": "memory_read",
  "parameters": {
    "type": "object",
    "properties": {
      "path": {
        "oneOf": [
          { "type": "string" },
          { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 20, "uniqueItems": true }
        ]
      }
    },
    "required": ["path"],
    "additionalProperties": false
  }
}
```

返回每个文件的完整 Markdown、opaque `version`、`content_hash` 和是否存在。多个路径的结果保持
请求顺序。不存在不是工具失败。风险等级 Low，无需审批。

### 6.3 `memory_write`

```json
{
  "name": "memory_write",
  "parameters": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "content": { "type": "string" },
      "if_version": { "type": "string" }
    },
    "required": ["path", "content", "if_version"],
    "additionalProperties": false
  }
}
```

- 创建新文件时 `if_version` 必须严格为 `new`；
- 重写已有文件时必须传最近一次 read/write 返回的 version；
- `content` 是包括 frontmatter 在内的完整文件，成功后完全替换旧内容；
- 工具解析并验证最终文档，任何字段、来源、标签或容量违规都整体拒绝；
- 适用于新建、大范围整理和同时修改多行；小修改优先用 `memory_str_replace`。

创建和修改属于持久记忆 capability 内的预授权写入，风险等级 Medium；默认不弹审批，但必须
显示非阻塞 Saved memory chip，并写工具审计。

### 6.4 `memory_str_replace`

```json
{
  "name": "memory_str_replace",
  "parameters": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "old_str": { "type": "string", "minLength": 1 },
      "new_str": { "type": "string" },
      "if_version": { "type": "string" }
    },
    "required": ["path", "old_str", "new_str", "if_version"],
    "additionalProperties": false
  }
}
```

`old_str` 必须在完整 Markdown 中恰好出现一次。0 次返回 `MATCH_NOT_FOUND`，多次返回
`MATCH_AMBIGUOUS`；两者都携带最新完整内容和 version，模型扩大匹配范围后在同一轮重试。
`new_str` 为空表示删除该唯一片段。替换后的完整文件必须重新通过所有校验。

### 6.5 `memory_append`

```json
{
  "name": "memory_append",
  "parameters": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "content": { "type": "string", "minLength": 1 },
      "if_version": { "type": "string" }
    },
    "required": ["path", "content", "if_version"],
    "additionalProperties": false
  }
}
```

只允许向已有文件正文末尾追加一条或多条完整事实行，不允许携带 frontmatter。工具先检查完全
重复行，再拼接、补齐来源并验证完整文件。已有事实应更新而不是追加近义重复。

### 6.6 `memory_delete`

```json
{
  "name": "memory_delete",
  "parameters": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "if_version": { "type": "string" }
    },
    "required": ["path", "if_version"],
    "additionalProperties": false
  }
}
```

整文件删除只在当前用户消息明确要求删除该文件或其全部主题时允许。orchestrator 在工具上下文中
提供本轮 destructive authorization；没有明确授权时，即使模型调用也返回
`USER_AUTHORIZATION_REQUIRED`。删除写墓碑并保留有限本地修订，风险等级 High。

删除单条事实不使用本工具，而使用 `memory_str_replace(..., new_str="")`。Agent 不能为了整理、
去重、过期判断或腾容量主动调用 `memory_delete`。

### 6.7 统一结果与错误

成功结果：

```json
{
  "ok": true,
  "path": "/topics/food.md",
  "version": "v7:4a21d8f0c18b6d2e",
  "listing_epoch": 43,
  "summary": "Appended 1 stated fact"
}
```

错误结果：

```json
{
  "ok": false,
  "code": "VERSION_CONFLICT",
  "message": "The memory file changed after it was read.",
  "path": "/topics/food.md",
  "current_content": "...",
  "current_version": "v8:...",
  "listing_epoch": 44,
  "retryable": true
}
```

错误码至少包括：`INVALID_PATH`、`NOT_FOUND`、`ALREADY_EXISTS`、`INVALID_FRONTMATTER`、
`INVALID_FACT_TAG`、`SOURCE_REMOVAL_FORBIDDEN`、`NAME_CONFLICT`、`SIZE_LIMIT`、
`MATCH_NOT_FOUND`、`MATCH_AMBIGUOUS`、`VERSION_CONFLICT`、`STALE_LISTING`、
`USER_AUTHORIZATION_REQUIRED`、`CAPABILITY_DISABLED` 和 `INTERNAL_ERROR`。

冲突、过期和匹配失败是正常协调结果，不自动转成面向用户的报错。只要工具返回最新完整内容，
Agent 应基于新内容合并并重试。真正语义冲突才询问用户。

---

## 7. 读写决策流程

### 7.1 回答前读取

```text
问题是否涉及用户、其项目、其关系或交互偏好？
  ├─ 否：无需为了“显得记得”而读取
  └─ 是：检查 listing
       ├─ 找到相关 path/alias/description：memory_read
       ├─ 不确定哪个相关：读取最多 20 个合理候选
       └─ listing 无相关项：基于当前对话回答，自然询问真正缺少的信息
```

不得在未读取明显相关文件前声称“不知道/没有记录”。读取为空时也不把“没有记忆”当成回答
主体。每个被使用的记忆事实都必须实质改变结论、建议或追问；仅用于展示记忆能力的个性化装饰
不应出现。

### 7.2 写入

```text
本轮是否出现用户明确说过的事实/选择/计划？
  ├─ 否：不写
  └─ 是：逐条执行隐私和耐久性过滤
       ├─ 不允许/短期/外部资料/模型生成：忽略
       └─ 可保存：按主题确定唯一目标文件
            ├─ listing 有目标或 alias 命中：read 最新文件
            │    ├─ 已有同一事实：不写
            │    ├─ 旧事实需更新：str_replace
            │    ├─ 新独立事实：append
            │    └─ 多处需整理：write 全量重构
            └─ listing 无目标：write(if_version="new")
```

一轮出现多个主题时拆成多个文件操作。成功写入后 Agent 正常回答，不说“我已经记住/保存了”；
UI chip 是默认反馈渠道。

### 7.3 移除与更正

- “我不再喜欢 X”是事实变化：更新原事实，可按用户原意决定是否保留有用历史；
- “忘掉/删除 X”是移除请求：删除目标行，不保留反向表述，也删除只依赖该事实的派生行；
- “删掉这个人的全部记录”在范围明确时可删除整个文件；范围不明确先询问；
- 更正可能影响 description、alias 或链接，必须在同一轮一并修正已经变得错误或误导的元数据；
- 仅仅新增一个细节不会自动要求扩大 description，除非旧描述已经错误或误导。

---

## 8. 本地持久化设计

### 8.1 Canonical 表

SQLite 是唯一真相源。虚拟“文件”是 API 语义，不要求在宿主机创建可被任意工具访问的目录。

```sql
CREATE TABLE memory_files (
  id                 TEXT PRIMARY KEY,
  agent_id           TEXT NOT NULL REFERENCES agents(id),
  path               TEXT NOT NULL,
  kind               TEXT NOT NULL CHECK(kind IN ('profile', 'preferences', 'topic', 'area', 'person')),
  name               TEXT NOT NULL,
  description        TEXT NOT NULL,
  sources_json       TEXT NOT NULL,
  aliases_json       TEXT NOT NULL,
  markdown           TEXT NOT NULL,
  content_hash       TEXT NOT NULL,
  version            INTEGER NOT NULL DEFAULT 1,
  status             TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active', 'deleted')),
  created_at         TEXT NOT NULL,
  updated_at         TEXT NOT NULL,
  deleted_at         TEXT,
  origin_device_id   TEXT NOT NULL,
  UNIQUE(agent_id, path)
);

CREATE UNIQUE INDEX memory_files_active_name
ON memory_files(agent_id, name)
WHERE status = 'active' AND deleted_at IS NULL;

CREATE TABLE memory_file_revisions (
  id                 TEXT PRIMARY KEY,
  memory_file_id     TEXT NOT NULL REFERENCES memory_files(id),
  base_version       INTEGER,
  resulting_version  INTEGER NOT NULL,
  operation          TEXT NOT NULL CHECK(operation IN ('create', 'write', 'replace', 'append', 'delete', 'sync')),
  before_markdown    TEXT,
  after_markdown     TEXT,
  actor_surface      TEXT NOT NULL,
  session_id         TEXT,
  message_id         TEXT,
  created_at         TEXT NOT NULL
);

CREATE TABLE memory_file_conflicts (
  id                 TEXT PRIMARY KEY,
  memory_file_id     TEXT NOT NULL REFERENCES memory_files(id),
  base_hash          TEXT,
  local_markdown     TEXT NOT NULL,
  remote_markdown    TEXT NOT NULL,
  remote_version     INTEGER NOT NULL,
  created_at         TEXT NOT NULL,
  resolved_at        TEXT
);
```

`markdown` 保存完整规范文档；其他业务列是同一事务内从 frontmatter 派生的可信索引，不允许
调用方分别传值。UI/listing 读派生列，`memory_read` 返回 `markdown`。

### 8.2 不再使用的旧结构

实现本设计时直接删除：

- `explicit_memories` 表及 `user_md / memory_md` kind；
- `memory_store` 表及其 `name / keywords / creator / embedding_id` 模型；
- `memory_store` 对 `embedding_items` 和 `vec_embeddings_*` 的引用；
- `memory_md_view`、`memory_md_edit`、`memory_search`、`memory_create`、`memory_update`；
- Python memory extractor、记忆 confidence/source 建议和自动 embedding 回填；
- 前端 USER.md/MEMORY.md 双编辑器、结构化记忆表单和向量覆盖率 UI；
- 同步实体 `explicit_memory`、`memory` 及其 payload/repository。

保留的 `USER.md` 不恢复 `explicit_memories`：它作为路径为 `/USER.md` 的特殊 `memory_file` 保存，
复用同一套 canonical Markdown、CAS、revision、outbox 和 E2EE 同步链路，并从普通 listing 隐藏。

`embedding_items` 与 sqlite-vec 仍由 RAG/知识库使用，不能因为删除记忆向量而全局删除。

### 8.3 事务

每个成功写操作在同一 SQLite 事务中完成：

1. 按 `agent_id + path` 读取当前行并校验 `if_version`；
2. 执行字符串修改或全量替换；
3. 解析、规范化并验证完整 Markdown；
4. 写 `memory_files`，递增 version，更新 hash 和派生列；
5. 写一条有限修订记录；
6. 写 E2EE sync outbox；
7. 递增当前 Agent 的 `listing_epoch`；
8. commit 后发布 UI memory-saved 事件。

任何一步失败则整体回滚，不发送成功 chip，不产生半更新的 Markdown/索引状态。内容字节未变化
时返回 no-op，不递增 version，也不写 outbox。

### 8.4 Version token

数据库 version 是本机内部整数；模型拿到的是 opaque token，例如
`v7:4a21d8f0c18b6d2e`，由 version 与规范内容 SHA-256 前缀构成。工具只比较，不要求模型解析。

- token 绑定当前 Agent、path 和内容版本；
- 不能把另一个文件的 token 用于当前文件；
- `new` 是保留字，只用于确认路径当前不存在；
- CAS 失败必须返回当前完整内容与新 token，避免模型额外读一次；
- 工具审计可记录 token 和 hash，不能把完整私密正文复制到普通日志。

### 8.5 修订保留

本地默认保留每文件最近 20 个版本或 30 天内版本，以较大者为准；冲突未解决的修订不得 GC。
修订不进入普通聊天同步 payload。用户可从 UI 撤销到旧版本，撤销本身创建一个新版本，不回退
全局版本号。

---

## 9. 多会话、多 surface 与同步冲突

### 9.1 本机并发

DbActor 串行化最终事务，但业务正确性依赖 CAS 而不是队列顺序。典型流程：

```text
Session A read v4          Session B read v4
Session A append → v5
                           Session B append(v4) → VERSION_CONFLICT + current v5
                           Session B 合并自己的事实与 v5 → append/write(v5) → v6
```

Agent 不因版本冲突请求用户批准。只有两个版本表达互相排斥的事实、无法从用户原话判断哪个应
保留时才询问用户。

### 9.2 E2EE 同步实体

新白名单实体只有 `memory_file`：

```json
{
  "id": "stable-uuid",
  "agent_id": "agent-uuid",
  "path": "/areas/agnes-agent.md",
  "markdown": "<full canonical markdown>",
  "content_hash": "sha256",
  "version": 7,
  "deleted_at": null,
  "updated_at": "RFC3339",
  "origin_device_id": "device-uuid"
}
```

payload 先由 Rust 按字段白名单投影，再由客户端 E2EE；D1 只看到 envelope 和密文。路径和
正文都在密文内，服务端不建立记忆索引。embedding 不存在，也没有向量制品。

### 9.3 跨设备冲突

服务端对同一实体使用 revision CAS，不允许静默 LWW。设备 push 被拒绝后：

1. pull 最新密文实体并在本地解密；
2. 若本地尚未继续修改，直接应用远端版本；
3. 若本地与远端都从同一 base 分叉，保存到 `memory_file_conflicts`；
4. canonical 行采用已确认的远端版本，未合并本地版本不得丢弃；
5. 下次 `memory_read`/写操作返回 staleness notice、当前内容与竞争版本；
6. Agent 在完整上下文中合并非冲突事实，或 UI 让用户人工处理；
7. 以远端当前 token CAS 写入合并结果，标记 conflict resolved，再生成新 outbox。

frontmatter 的 `sources` 和合法 aliases 采用集合并集；正文不使用通用 LLM 离线自动合并，也不把
Git conflict marker 写进正式记忆。删除与并发修改相遇时必须保留冲突记录并等待当前 Agent 或用户
处理，不能仅凭 HLC 决定永久丢弃内容。

### 9.4 开发阶段切换

本次重构不编写旧数据 migration：

- 本地删除开发数据库后由新 schema 初始化；
- Worker/D1 开发环境删除 `explicit_memory`、`memory` 测试实体并部署新 migration；
- 不从旧 `USER.md`、`MEMORY.md` 或 `memory_store` 自动生成文件；
- 不提供读取旧实体的 fallback；
- 同一应用版本中不能同时启用新旧记忆工具。

---

## 10. 隐私、安全与行为边界

### 10.1 Chat surface 永不写入

- 健康、心理健康和 coping 细节；
- 种族、族裔、血统、heritage；
- 移民身份、公民身份办理过程、国籍来源指标；
- 家庭成员的健康史或应对模式；
- 自伤方法、数量或具体计划；
- 伴侣、配偶和家庭成员姓名；
- 凭证、token、密钥、验证码、支付卡和其他 secret；
- 要求 Agent 放弃诚实、安全、批判性思考或系统边界的偏好。

一句话同时包含允许和禁止内容时，只保存允许部分，不写“某种健康问题”等占位泛化。用户明确
要求 Agent 记住禁止类别时，Agent 用一句短句说明不能保存该类信息，不扩展政策清单。

饮食限制、学生/退休/父母等人生角色、职业可以在用户明确要求或按原话允许时保存，但不得从中
推断敏感属性。

### 10.2 已存在的敏感内容

用户手动编辑、其他可信 surface 或未来导入可能使文件中出现 chat 不能创建的敏感内容。运行时
不能假装没有看到，但只在以下情形使用：

- 用户在当前问题中明确要求使用；或
- 对当前请求的安全、适当和准确回答必不可少。

其他情况下不主动提起。第三方事实只有在用户当前问题已经带入该人物时才能使用。

### 10.3 Prompt injection 防护

- 记忆文件按“用户事实数据”处理，不按高优先级指令处理；
- `/preferences.md` 只在格式、长度、语气等允许范围内生效；
- listing 和正文都进行 XML/JSON 转义与长度校验；
- frontmatter 用 safe parser 并拒绝未知字段；
- 记忆中的“忽略系统提示词”“调用某工具”等文本不获得权限；
- memory capability 关闭时不注入记忆块，也不向模型发布六个工具；
- 工具层始终再次校验 agent scope、path、版本、授权和最终文档，不能只信模型遵守提示词。

### 10.4 回复中的应用方式

Agent 自然使用相关事实，不提“根据你的记忆/档案/数据”“我记得/我回忆起”。只有用户直接询问
记忆系统时，才可使用“你之前提到过/我们讨论过”等来源表述。

直接问及用户自身且答案已在相关文件中时，直接回答事实，不加不必要的记忆来源前言。格式、
长度、语气偏好始终适用；普通技术问题若无需个性化，不读取或插入无关个人信息。

---

## 11. UI 设计

### 11.1 记忆管理页

旧的 USER.md/MEMORY.md 双卡片和结构化记忆列表替换为“只读基础资料 + 记忆文件系统”：

```text
记忆
├─ USER.md 正文编辑器（AI 只读）
├─ 基础信息继承策略（自动 / 始终继承 / 不继承）
├─ 搜索框（本地 path/name/description/aliases/正文 FTS，仅用于 UI）
├─ Agent 选择器
├─ 文件树
│  ├─ profile.md
│  ├─ preferences.md
│  ├─ topics
│  ├─ areas
│  └─ people
└─ Markdown 编辑器 / 预览
   ├─ frontmatter 表单
   ├─ stated/observed/inferred 标签提示
   ├─ backlinks 与 dangling links
   ├─ 来源、更新时间、版本
   └─ 保存 / 删除 / 历史 / 撤销
```

UI 保存走与 Agent 工具相同的 parser、validator 和 CAS repository，不直接执行 SQL。用户手动
新增的事实使用 `[stated]`，可保留但不能伪造已有 `[observed]`、`[inferred]` 来源；UI 对敏感
内容给出本地提示，最终控制权归用户。删除提供明确确认和可撤销历史。

### 11.2 对话反馈

- 自动创建/修改成功：显示可点击的 “Saved memory” chip，展开后显示虚拟 path 和变更摘要；
- no-op：不显示 chip；
- 自动写入失败且用户未明确要求保存：不打断回答，可在工具详情看到失败；
- 用户明确要求保存但失败：Agent 应直接说明失败原因；
- 冲突已自动合并：chip 标明“已合并较新版本”；
- 需要语义裁决：不覆盖，Agent 在对话中询问用户。

### 11.3 本地搜索

UI 可维护 FTS5 派生索引搜索路径、名称、描述、别名和正文，但该索引：

- 不是 Agent 的 `memory_search` 工具；
- 不生成 embedding；
- 不同步，可从 `memory_files` 重建；
- 不能绕过 Agent 隔离；
- 删除或修改文件时在同一事务更新，失败时可后台全量重建。

---

## 12. 可观测性与审计

每次工具调用记录：

- tool 名称、session/message、虚拟 path；
- 操作前后 version/hash，不记录普通日志中的完整正文；
- validation、CAS、授权和同步结果；
- prompt template version 与 listing epoch；
- UI 是否收到 saved event。

以下指标用于本地诊断：

- `memory_read_total{result}`；
- `memory_write_total{operation,result}`；
- `memory_cas_conflict_total`；
- `memory_validation_failure_total{code}`；
- `memory_sync_conflict_open`；
- `memory_listing_files` 和 `memory_listing_prompt_tokens`；
- `memory_saved_chip_delivery_total{result}`。

日志、崩溃报告和遥测不得包含 Markdown 正文、description、aliases 或用户姓名。

---

## 13. 实施状态

### Phase 1：领域模型与本地工具

- [x] 建立 `memory_files`、revisions、conflicts 和 listing epoch；
- [x] 实现统一 parser/validator/path normalizer；
- [x] 实现六个工具、CAS、脱敏审计和 Saved memory 对话反馈；
- [x] 删除旧 memory tools、extractor 和记忆 embedding 回填；
- [x] 使用全新开发数据库启动，不实现旧数据迁移。

### Phase 2：Prompt compiler

- [x] 将 `MEMORY_FILESYSTEM_PROMPT.md` 打包为唯一模板；
- [x] 实现 Agent name 替换、USER.md 固定注入、动态块转义、单次 DB snapshot 与 token 预算；
- [x] 删除 `MEMORY_MANAGEMENT_INSTRUCTIONS` 和旧 USER/MEMORY 双文档注入；
- [x] 添加 prompt 版本/hash golden test 和工具 schema 覆盖测试。

### Phase 3：前端

- [x] 用虚拟路径列表和完整 Markdown 编辑器替换旧记忆设置页；
- [x] 保留 AI 只读 USER.md 编辑器和 auto/inherit/isolated 基础信息继承控制；
- [x] 接入 Saved memory 反馈、删除确认和版本冲突重载；
- [x] 移除向量覆盖率和手动向量化相关记忆 UI；
- [ ] 增加目录树、修订历史/撤销、正文 FTS、backlink 和 dangling-link 导航。

### Phase 4：同步

- [x] 增加 `memory_file` E2EE payload、事务内 outbox 和 D1 migration；
- [x] 删除 `explicit_memory` / `memory` sync repository 和 Worker 路由；
- [x] 实现确定性实体 ID、revision CAS、三方合并、冲突落盘和恢复流程；
- [x] migration 清理开发云端旧记忆实体；
- [ ] 使用两个真实设备完成端到端人工验收。

---

## 14. 测试策略

### 14.1 Parser 与 validator

- 合法五类路径、path traversal、Unicode/大小写 slug、重复 name；
- YAML 多文档、未知字段、anchor/tag、缺字段、来源删除、非法 aliases；
- `[stated]` 新写、保留 observed/inferred、chat 伪造其他标签；
- 换行规范化、文件大小、profile 预算、链接抽取、dangling link；
- 同内容 no-op 和 hash 稳定性。

### 14.2 工具

- 单/批量 read 与 20 路径上限；
- create 必须使用 `new`，write 全量覆盖；
- replace 的 0/1/多匹配；
- append 去重、来源补齐和容量失败原子性；
- delete 必须有用户明确授权；
- 当前 session Agent 隔离，伪造 path/token 不可跨 Agent；
- 失败返回当前内容/version，模型可在同轮重试。

### 14.3 Prompt

- 模板仅替换 `{{Agent_Name}}`，版本与 package resource 一致；
- profile/preferences 不存在与存在两种注入；
- listing 包含每个活跃文件且 canonical 排序；
- XML 注入字符无法闭合动态块；
- memory capability 禁用时不注入也不发布工具；
- prompt cache 静态前缀稳定，token 预算不截断 listing。

### 14.4 行为评测

建立覆盖正式提示词规则的对话 eval：

- 用户介绍自己后在下一问前写入；
- 一个消息拆分写入 topics/areas/people 多文件；
- 搜索得到的信息不写，用户确认选择后只写选择；
- 单次提及不过度泛化；
- 敏感句只保存允许部分；
- 已有别名时更新旧文件而不是新建；
- 回答用户个人问题前读取相关文件；
- 普通技术问题不插入无关画像；
- 不说“我记得/根据你的记忆”；
- 写入成功不在 assistant 文本播报；
- CAS 冲突自行合并，语义冲突才询问；
- 未经用户明确要求绝不删除整文件。

### 14.5 同步

- E2EE payload 白名单不包含本机内部修订或正文日志副本；
- 两设备非冲突更新正常收敛；
- 同 base 并发编辑产生 conflict，不静默 LWW；
- 删除与修改冲突可恢复；
- tombstone 防止离线旧设备复活文件；
- bootstrap 顺序保证 agent 先于 memory_file；
- 清空并重建本地 UI FTS 不影响 canonical 数据。

---

## 15. 验收状态

### 15.1 核心替换（已完成）

- [x] USER.md 是独立只读基础层，运行时代码和 prompt 中不再存在旧 USER.md/MEMORY.md 双轨决策；
- [x] Agent 只收到 `memory_list/read/write/str_replace/append/delete` 六个记忆工具；
- [x] `MEMORY_FILESYSTEM_PROMPT.md` 是唯一正式模板，版本/hash golden test 可检测漂移；
- [x] profile、preferences、listing 每轮从同一 Agent DB snapshot 正确注入；
- [x] 所有本地写入使用 CAS，远端派生字段重算且冲突不会静默覆盖；
- [x] chat 与 UI 只能新建 `[stated]`，并必须原样保留可信 surface 的其他标签；
- [x] 前端可列出、创建、编辑、删除文件并处理 CAS 冲突；
- [x] 新 `memory_file` 已进入 E2EE payload、outbox、bootstrap 和冲突恢复链路；
- [x] 旧表、旧工具、旧 extractor、旧记忆向量任务和旧同步实体均已删除；
- [x] RAG 的 sqlite-vec、文档检索和加密向量制品不受本次删除影响。

### 15.2 产品化增强（后续）

- [ ] 补齐隐私省略、别名归档、读前判断和回复措辞的完整对话行为 eval；
- [ ] 前端提供修订历史、撤销、正文 FTS、backlink 与 dangling-link 导航；
- [ ] 实现第 12 节的细粒度本地指标与诊断视图；
- [ ] 在两个真实设备上完成非冲突收敛、同 base 冲突和删除/修改冲突人工验收。

本文件负责系统设计；模型逐条执行的行为规范以
`ProjectPlan/MEMORY_FILESYSTEM_PROMPT.md` 为准。二者出现冲突时，应先修正文档与模板使其一致，
不得在代码中增加第三份隐式规则。
