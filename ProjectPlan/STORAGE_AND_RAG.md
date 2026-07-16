# 存储、RAG 与外部服务设计

> 状态：详细设计初稿  
> 日期：2026-07-16  
> 适用范围：聊天、记忆、RAG 知识库、大文件、向量制品、多网盘、日历与待办  
> 关联文档：`PROJECT.md`、`architecture.md`、`CLOUDFLARE_SYNC.md`、`UI_DESIGN.md`

## 1. 核心结论

`agnes-agent` 继续坚持 local-first，但将云端能力明确拆分为“控制面”和“数据面”：

- **本地 SQLite 是运行时真相源**：聊天、记忆、文档元数据、日历、待办和各类索引在断网时仍然可用。
- **D1 是同步控制面**：保存结构化实体的增量变更，以及文件/制品的最新版本、密文 Hash、远端副本和设备落地状态。
- **R2 / Google Drive 是大对象数据面**：保存客户端压缩并加密后的源文件、附件、抽取文本和向量制品。
- **sqlite-vec 是本地在线检索引擎**：不从 R2/Drive 直接查向量；下载并解密便携制品后，原子导入本地 sqlite-vec。
- **Google Drive 可以存储加密向量文件**：Drive 只看到随机对象名、大小和密文，不拥有解密密钥，也不承担向量查询。
- **网盘、日历、待办是独立子功能**：它们各自有域模型和 Provider 端口，不将外部 API 细节泄漏到业务层。

R2 是对象存储，不是关系数据库或向量数据库。D1 也不存放大文件和大向量 payload。

---

## 2. 数据分层和真相源

| 数据 | 本地存储 | 云端存储 | 真相源 | 可否重建 |
|---|---|---|---|---|
| Agent / Session / Message | SQLite | D1 E2EE 实体 | SQLite + 同步协议 | 否 |
| USER.md / MEMORY.md / memory_store 文本 | SQLite | D1 E2EE 实体 | SQLite + 同步协议 | 否 |
| 日历 / 待办 | SQLite | D1 E2EE 实体 | SQLite + 同步协议 | 否 |
| 源文件 / 附件 | 本地内容库 | R2 / Drive 加密对象 | 文件版本清单 | 否 |
| 抽取文本 / chunk | SQLite 或本地制品 | 可选加密制品 | 源文件 + 解析器指纹 | 是 |
| Embedding | sqlite-vec | 可选 R2 / Drive 加密制品 | chunk + 嵌入配置指纹 | 是 |
| D1 / SQLite 备份 | 本地快照 | R2 / Drive 加密对象 | 快照 manifest | 部分 |
| Provider 凭证 / OAuth refresh token | OS Keyring | 不上传 | Keyring | 否 |

“可重建”不等于“不值得保存”。对 Qwen3-Embedding-8B 等较重模型，重建大型知识库会耗时且可能有调用费用，因此允许将向量作为“可验证的加密缓存制品”上传，但它不取代源文件。

---

## 3. 作用域设计

### 3.1 记忆继续按 Agent 隔离

- `USER.md / MEMORY.md / memory_store` 继续一对一属于 Agent。
- 记忆文本参与 D1 同步，本地向量可重建。
- 记忆条目数量通常远小于 RAG chunk，默认不为每次变化上传独立向量包；可在大规模时按 Agent + embedding profile 生成快照。

### 3.2 RAG 使用独立知识库

文档不再直接绑死单个 Agent，而是归属 `knowledge_collection`：

- `user_global`：用户全局知识库；
- `workspace`：逻辑工作区知识库；
- `agent_private`：仅指定 Agent 可用；
- `custom`：通过授权表分配给多个 Agent。

同一源文件和同一 embedding profile 只产生一份向量，查询时按当前 Agent 被授权的 collection 过滤，避免多 Agent 重复占用存储和嵌入算力。

### 3.3 日历和待办属于用户

- 日历与待办是用户级域数据，不复制到每个 Agent。
- Agent 通过 tool policy 获得只读、编辑或无权限等级。
- 标题、描述和关联信息可作为本地 RAG 来源，但向量仍是派生数据。

---

## 4. 本地数据模型

现有 `documents(id, agent_id, title, path, created_at)` 仅是占位表，正式 RAG 实现前不在其上继续堆字段，而是使用明确分层。

### 4.1 知识库与文档

```text
knowledge_collections
  id / name / scope / workspace_id? / created_at / updated_at / version / deleted_at

collection_agents
  collection_id / agent_id / permission(read|write|manage)

documents
  id / collection_id / title / media_type / current_version_id
  status(active|missing|error|deleted) / created_at / updated_at / version / deleted_at

document_sources
  id / document_id / provider_account_id / source_kind
  encrypted_locator / provider_revision / observed_at / local_binding_id?

document_versions
  id / document_id / logical_version / plaintext_hash / size / media_type
  parser_profile_id / created_at / origin_device_id

document_chunks
  id / document_version_id / ordinal / content / content_hash
  page? / section_path? / token_count / metadata
```

- `path` 是设备本地 binding，不参与同步。
- `encrypted_locator` 是 Provider 对象 ID 或路径的 E2EE envelope，不保存 OAuth token。
- `plaintext_hash` 用于本地去重和失效判定；上传 D1 时放入 E2EE payload，避免在明文索引列泄漏相同文档关系。

### 4.2 Embedding profile 与制品

```text
embedding_profiles
  id / model_ref / model_revision? / dims / normalized
  instruction_hash / tokenizer_ref? / created_at

parser_profiles
  id / parser_name / parser_version / options_hash

chunker_profiles
  id / chunker_name / chunker_version / chunk_size / overlap / options_hash

embedding_items
  id / ref_type / ref_id / collection_id / embedding_profile_id
  dims / content_hash / created_at

artifact_manifests
  id / artifact_type / source_version_id / build_fingerprint
  format_version / plaintext_hash / ciphertext_hash / size
  encryption_scheme / key_version / created_at

artifact_replicas
  artifact_id / provider_account_id / encrypted_locator
  provider_revision / etag / ciphertext_hash / status / updated_at

device_artifact_states
  device_id / artifact_id / observed_version / local_status
  verified_hash / last_checked_at / last_error_code
```

`build_fingerprint` 必须覆盖：

```text
source plaintext hash
+ parser profile fingerprint
+ chunker profile fingerprint
+ embedding model/ref/revision
+ dims + normalization + embedding instruction hash
+ artifact format version
```

任一项不同都不能复用旧制品。仅模型名称相同不足以判断兼容。

### 4.3 本地内容库

- 使用 app data 目录下的内容库，不将大文件 BLOB 直接塞入主 SQLite。
- 写入路径为 `temp -> fsync -> hash verify -> atomic rename`，进程崩溃不得留下被误认为完整的文件。
- 本地文件名使用随机 object ID 或带密钥的内容标识，原始文件名只作为加密元数据。
- 使用 SQLite 记录引用计数和保留期，只由后台 GC 删除无引用对象。

---

## 5. 便携向量制品

### 5.1 不上传运行中 SQLite 文件

不直接压缩并上传正在运行的 `agnes.db`、WAL 或 sqlite-vec 虚拟表文件，原因包括：

- 无法证明主库、WAL 和向量表是同一个原子时点；
- sqlite-vec / SQLite 版本及平台差异可能导致不兼容；
- 整库替换会覆盖设备本地状态，与增量同步冲突。

### 5.2 制品格式

每个 `document_version + build_fingerprint` 生成一个不可变分片，避免知识库中一个文件变化就重传整库：

```text
artifact/
  manifest.json              # 规范化 JSON，包含版本/指纹/记录数
  chunks.jsonl.zst           # chunk id、定位、content hash，可选包含文本
  vectors.f32le.zst          # 固定维度、little-endian float32
```

内层先压缩，外层再做流式分块 AEAD 加密。建议首版使用 XChaCha20-Poly1305 分块 envelope，每个 artifact 使用随机 DEK，DEK 由账户主密钥包装。

下载时：

1. 支持 Range/断点续传；
2. 先验证密文 Hash；
3. 流式解密并验证 AEAD tag；
4. 再验证内部 manifest 和明文 Hash；
5. 导入临时向量分区；
6. 事务性切换为当前分区。

任一步失败都不覆盖现有可用索引。

### 5.3 容量估算与维度

sqlite-vec 当前使用 float32，不含索引和元数据的原始体积约为：

| chunk 数 | 1024 维 | 2048 维 | 4096 维 |
|---:|---:|---:|---:|
| 10,000 | 39 MiB | 78 MiB | 156 MiB |
| 100,000 | 391 MiB | 781 MiB | 1.53 GiB |
| 1,000,000 | 3.81 GiB | 7.63 GiB | 15.26 GiB |

- RAG 默认先评估 1024/2048 维，不因模型支持 4096 维就无条件使用最大维度。
- 记录实际 dims，不从模型名称猜测。
- 优先用 FTS5/BM25 做文本候选、collection 和 metadata 过滤，再与向量候选以 RRF 融合。
- 达到十万级 chunk 后对插入、启动、KNN 和备份做实测；是否更换向量引擎由基准数据决定，不预先引入常驻服务。

---

## 6. D1 同步控制面

### 6.1 D1 不保存什么

- 不保存大源文件、完整抽取文本、向量数组或整个向量包；
- 不保存 Google OAuth refresh token、R2 API Token 或任何 Provider 明文凭证；
- 不保存本地绝对路径；
- 不使用明文 plaintext hash 作为公开索引列。

### 6.2 D1 清单表

```text
object_manifests
  owner_id / object_id / object_kind / logical_version
  latest_artifact_id / ciphertext_hash / size / key_version
  updated_hlc / deleted_at

object_replicas
  owner_id / artifact_id / provider_account_id / provider_kind
  opaque_server_key? / encrypted_locator? / provider_revision / etag
  ciphertext_hash / size / status / updated_at

object_changes
  server_seq / owner_id / change_id / object_id / artifact_id?
  operation / logical_version / changed_at

device_object_states
  owner_id / device_id / object_id / observed_logical_version
  installed_artifact_id / local_status
  verified_ciphertext_hash / checked_at / error_code
```

`object_manifests` 是最新逻辑版本的 CAS 控制点；`object_replicas` 允许同一不可变制品同时在 R2 和 Drive 中存在；`object_changes` 是按单调 `server_seq` 排序的小型 append-only 控制变更流；`device_object_states` 每设备/对象只保留最新一行，不当作事件日志无限增长。

对象控制面可复用现有 sync change stream 的幂等、pull、bootstrap、ack 和保留策略，但不将大对象 payload 放入 `sync_changes`。首版实现前通过基准选择“独立 `object_changes`”或“在现有变更流中只发布小型 manifest 实体”；两种实现都必须保留独立 `last_object_cursor`，避免大对象恢复状态与聊天消息 cursor 相互阻塞。

R2 的 `opaque_server_key` 是随机、不含业务语义的 object key，Worker 需要使用它访问 R2 binding，因此可作为明文控制列。Google Drive file ID 由客户端直接使用，放入 `encrypted_locator` E2EE payload。两者都不包含 OAuth token、用户文件名或明文内容 Hash。

### 6.3 设备启动决策

设备启动后不遍历网盘目录，而是以 D1 清单为准：

1. 使用本机 `last_object_cursor` 拉取逻辑对象、artifact 和 replica 变更；游标过旧时先 bootstrap manifest 快照，再接续高水位之后的新变更。
2. 将远端 `logical_version / build_fingerprint / artifact_id` 与本地状态比较。
3. 本地已有相同 artifact 且 Hash 验证成功：跳过。
4. 远端有兼容 artifact 且本机已授权对应 Provider：选择优先级最高的可用 replica 断点下载。
5. 远端 artifact 不兼容或 Provider 不可用，但本地有源文件：本地重建。
6. 源文件和 artifact 都不可用：标记 `missing`，不删除旧的可用版本。
7. 下载/重建成功后验证 Hash，原子安装，再 upsert 本设备状态。

启动只阻塞必须的小型 D1 清单同步，大文件下载和重建在后台进行。Agent 查询到未就绪 collection 时应返回“索引准备中”的结构化状态，不将应用整体卡在启动页。

### 6.4 并发构建与发布

- artifact 不可变且以指纹幂等；两台设备重复构建同一指纹不得产生两个逻辑版本。
- 上传顺序为：上传临时对象 -> 远端 Hash/大小验证 -> 标记 replica ready -> CAS 发布 manifest。
- D1 manifest 不得先指向尚未上传成功的对象。
- 发布失败的临时对象由保留期 GC，不立即删除以免误删慢请求。
- 逻辑删除先发布墓碑；只有无活跃设备引用且超过保留期时才删除远端大对象。

---

## 7. 多网盘依赖倒置

### 7.1 边界

业务层不识别 Google Drive file resource、R2 S3 key、夸克 Cookie 或任何特定 SDK 类型。它只依赖应用定义的端口：

```rust
#[async_trait]
pub trait ObjectStorageProvider: Send + Sync {
    fn kind(&self) -> StorageProviderKind;
    fn capabilities(&self) -> StorageCapabilities;
    async fn stat(&self, locator: &EncryptedLocator) -> Result<RemoteObjectState>;
    async fn download(&self, request: DownloadRequest) -> Result<DownloadStream>;
    async fn upload(&self, request: UploadRequest) -> Result<UploadSession>;
    async fn resume_upload(&self, request: ResumeUploadRequest) -> Result<RemoteObject>;
    async fn delete(&self, locator: &EncryptedLocator) -> Result<()>;
}
```

上层 `ArtifactReplicationService` 负责版本、Hash、加密、重试、副本选择和 D1 状态；Provider adapter 只负责远端对象 API。不把同步策略复制到每个 Provider 内部。

`StorageCapabilities` 至少声明：

- 是否支持 Range download；
- 是否支持 resumable/multipart upload；
- 是否支持条件写和稳定 revision/etag；
- 单对象限制和建议分片大小；
- 是否需要每设备用户授权；
- 是否可由 Worker 代理。

### 7.2 Provider 账户

```text
storage_provider_accounts
  id / kind / display_name / account_subject?
  auth_state / capabilities_json / is_enabled
  created_at / updated_at
```

- 凭证 secret ID 可由 `storage:{account_id}:credential` 派生，明文值只存 OS Keyring。
- D1 可同步不含凭证的逻辑 Provider 配置，但新设备必须自行 OAuth/授权后才能使用该 replica。
- 用户可为每个 collection 设置 `local_only / r2 / google_drive / mirrored`。
- D1 清单是查找对象的主入口；Provider list 只用于修复、对账和孤儿对象 GC。

### 7.3 R2 Provider

- R2 是默认应用托管存储，客户端不持有 Cloudflare Account/R2 API Token。
- Worker 通过 R2 binding 访问 bucket，校验 owner/device/object manifest 后执行上传或下载。
- 大文件使用 multipart/分块上传，客户端维护可恢复 upload session。
- object key 使用随机 owner/object ID，不包含用户文件名、Agent 名、工作区名或明文 Hash。
- 当前 Cloudflare 账户已开通 R2，但尚未创建 bucket；实现阶段由 Wrangler 创建 `agnes-blobs` 并写入 Worker binding，不要向 renderer 发放 R2 Token。

### 7.4 Google Drive Provider

- 只使用 Google Drive 官方 API 和 OAuth 2.0 + PKCE，优先最小 `drive.file` 权限。
- 使用 resumable upload 上传大型加密制品，使用 Range download 续传。
- Drive 中使用随机文件名，明文标题、MIME 和目录归属在客户端加密 manifest 中保存。
- D1 记录加密 file ID、Drive revision/modified time、密文 Hash 和 size，不记录 access/refresh token。
- Google refresh token 每设备保存在 OS Keyring，设备未授权该 Google 账户时不能下载 Drive replica，但可回退 R2 副本或本地重建。
- Google Drive 也可作为用户源文件连接器；对 Google Docs/Sheets/Slides 使用官方 export 格式并将 revision 纳入文档版本。
- 国内网络下 Google Drive 不能作为应用启动或本地聊天的强依赖。

### 7.5 夸克网盘 Provider

- 逆向 API 不进入核心仓库的默认功能和正常同步验收标准。
- 以可选 community adapter/plugin 提供，默认禁用且优先只读导入。
- 必须声明版本和可用性，不得在应用日志、SQLite 或 D1 保存 Cookie/token 明文。
- 失效只影响该 Provider，不得阻断本地文件、R2、Drive 或其他功能。
- 在引入前单独检查服务条款、账号风控和法律风险。

WebDAV 和通用 S3 可作为比逆向网盘 API 更稳定的后续 Provider。

### 7.6 外部源文件变更发现

D1 manifest 是 Agnes 设备间的最新状态，但 Google Drive 中的用户源文件还可能被 Drive 网页或其他应用修改。至少一台已授权设备需要充当该 Provider 的 observer：

1. 每个 Provider 账户在本地记录自己的 change cursor/page token，不与 D1 业务 cursor 混用。
2. Google Drive 使用官方 Changes API 和 `startPageToken`，在启动、恢复网络和后台周期调度时增量检查，不全盘扫描。
3. 发现 revision/modified time 变化后，先获取内容并计算明文 Hash；只有真实内容变化才创建新 `document_version`。
4. 新版本先写入本地 SQLite/outbox，再由普通 D1 同步协议告知其他设备。
5. 多台 observer 同时发现同一 Drive revision 时，使用 provider account + file ID + provider revision + content hash 幂等去重。
6. 未授权 Google 账户的设备只消费 D1 中已发布的结果，不猜测 Drive 最新状态。

R2 中的对象由 Agnes 以不可变制品管理，不将 bucket list 当作外部编辑源；与 D1 manifest 不符的对象进入对账/GC，不自动变成新文档版本。

---

## 8. RAG 处理链路

```text
源文件/网盘变更
  -> 生成 document_version
  -> MIME 检测和安全限制
  -> parser 抽取文本/结构
  -> chunker 分块
  -> FTS5 索引
  -> embedding batch
  -> sqlite-vec 分区
  -> 生成便携 artifact
  -> 压缩 + E2EE
  -> 上传 Provider replica
  -> D1 CAS 发布 manifest
```

### 8.1 索引分区

- 记忆使用 `agent_id + embedding_profile_id`。
- RAG 使用 `collection_id + embedding_profile_id`。
- vector table 仍可按 dims 动态建表，但 partition key 需从仅 `agent_id` 抽象为 namespace type/id。
- 同一 collection 下按 document version 分片构建，manifest 决定当前有效分片集。

### 8.2 检索

1. 根据 Agent 和 tool policy 计算允许访问的 collection。
2. FTS5/BM25 搜索名称、标题、正文和 metadata。
3. 在完全相同的 embedding profile 分区中做 vector KNN。
4. 用 RRF 融合文本和向量候选，再按文档/章节去重和扩展相邻 chunk。
5. 返回稳定 document/version/chunk ID、引用位置和可见来源。

RAG 内容始终是不可信数据；即使文档声称“忽略系统提示词”，也不得提升为 system/developer 指令。

### 8.3 失效和重建

- 源文件版本变化：只失效该 document version 的 chunk 和向量分片。
- parser/chunker 变化：重建影响文档的 chunk、FTS 和向量。
- embedding profile 变化：保留旧 profile 分区直到新分区完整可用，再延迟 GC。
- artifact 下载失败：保留本地旧索引，按 Provider 重试/切换副本/本地重建的顺序处理。

---

## 9. 日历与待办

### 9.1 本地域模型

```text
calendars
  id / name / color / timezone / provider_account_id? / version / deleted_at

calendar_events
  id / calendar_id / title / description / location
  starts_at / ends_at / timezone / all_day
  recurrence_rule? / recurrence_id? / status
  created_at / updated_at / version / deleted_at

event_exceptions
  event_id / original_occurrence / replacement_event_id? / is_cancelled

task_lists
  id / name / color / provider_account_id? / version / deleted_at

tasks
  id / task_list_id / parent_id? / title / description
  status / priority / starts_at? / due_at? / completed_at?
  recurrence_rule? / sort_order / created_at / updated_at / version / deleted_at
```

- 重复规则使用明确的 RRULE + timezone + exception，不预生成无限 occurrence 行。
- 时间存储明确区分 UTC instant、用户时区和 all-day local date。
- 任务完成是结构化状态，不依赖聊天文本或记忆抽取推断。

### 9.2 Provider 端口

不用一个包罗万象的 `Provider` trait 同时承担网盘、日历和任务。分别定义：

- `ObjectStorageProvider`：R2 / Google Drive / WebDAV / S3 / community adapter；
- `CalendarProvider`：Google Calendar / CalDAV / Local；
- `TaskProvider`：Google Tasks / CalDAV VTODO / Local。

应用服务层完成本地实体与远端 ID、etag/revision、cursor 和冲突的映射。Provider adapter 不直接修改聊天、Agent 记忆或 D1 表。

### 9.3 Agent 工具

- `calendar_list / calendar_create / calendar_update`；
- `task_list / task_create / task_update / task_complete`；
- 写操作遵守 Agent tool policy 和人工审批模式；
- 工具结果返回结构化 ID 和时区结果，不仅返回自然语言文本；
- 外部 Provider 不可用时，本地新增/修改先成功并进入待同步状态。

---

## 10. 主界面信息架构

主界面仍保持两栏，但侧边栏从“单一聊天会话列表”升级为“子功能导航 + 可折叠会话”：

```text
当前 Agent / 账户

子功能
  聊天
  记忆
  知识库
  网盘
  日历
  待办

聊天会话                  [折叠] [+]
  普通会话 A
  普通会话 B

工作区会话                [折叠] [+]
  Project Alpha
    会话 1
    会话 2

同步状态 / 设置
```

- 子功能是路由/视图选择，不是在卡片中嵌套卡片。
- “聊天会话”只显示未绑定工作区的当前 Agent 会话。
- “工作区会话”按逻辑 workspace 分组，本机未绑定路径时仍可查看会话，但文件工具不可执行。
- 工作区分组依赖可同步的逻辑 workspace ID；当前 Session payload 暂未同步 `workspace_id`，Phase 3 实现 pull/merge 时需将“逻辑归属”与“本地 folder binding”完全分离，只将前者纳入协议。
- 两组折叠状态是设备本地 UI 偏好，不需要进入云同步冲突模型。
- 侧边栏整体折叠时保留子功能图标轨，不展示会话文本。
- 手机端使用 drawer，但信息层级与桌面端一致。

详细视觉和交互约束同步写入 `UI_DESIGN.md`。

---

## 11. 安全与隐私

- 所有大对象在离开客户端前先压缩、后加密；Provider 不得获得主密钥或 artifact DEK 明文。
- 加密后 Google Drive/R2 仍可见对象数量、大小和访问时间；文档不得宣称可隐藏所有侧信道。
- 不用明文文件名或 plaintext SHA-256 作为远端 object key。
- OAuth access/refresh token、网盘 Cookie、R2/S3 凭证和 E2EE 主密钥只进入 OS Keyring / Android Keystore。
- 前端只看到 Provider 是否已授权、账户显示名和错误状态，不存在读取已保存 secret 的 IPC。
- 文档解析器作为不可信输入边界：限制文件大小、解压比、嵌套层数、页数、总字符和处理时间。
- RAG 返回必须带来源引用，不将召回文本作为高优先级 prompt。

---

## 12. 故障处理

| 故障 | 行为 |
|---|---|
| D1 不可用 | 使用本地已知 manifest 和索引，暂停发布新远端版本 |
| R2 不可用 | 切换 Drive 副本或保持本地待上传 |
| Drive 授权失效 | 标记 auth_required，不删本地数据，可切换 R2 |
| 所有 Provider 不可用 | 本地功能正常，数据进入待复制状态 |
| 下载 Hash 不匹配 | 隔离临时文件，不安装，更换 replica 重试 |
| artifact 格式不兼容 | 从源文件重建，保留旧可用索引 |
| 磁盘不足 | 停止大对象下载，提供按 collection 清理本地缓存 |
| 两设备并发构建 | 指纹幂等 + manifest CAS，允许重复物理上传后 GC |

---

## 13. 实施阶段

### Phase A：数据模型与本地 RAG

- [x] 将占位 `documents/document_chunks` 迁移为 collection/document/version/source 模型，并保留旧本地数据、分块和路径绑定；
- [x] 落地 embedding/parser/chunker profile 与本地 FTS5 的数据表；
- [ ] 为向量表增加 namespace type/id；
- [x] 实现本地 UTF-8 文本文档导入、版本去重、确定性分块与 FTS5 检索；导入限制为 Markdown、文本、CSV、JSON，单文件最大 10 MiB；
- [ ] 接入 sqlite-vec 分区、嵌入批处理与 RRF 混合检索；
- 先用中小数据集做质量和性能基准。

### Phase B：加密制品与 D1 清单

- 定稿分块 E2EE envelope 和 artifact format v1；
- 实现 object/artifact/device manifest 和启动决策器；
- 实现断点下载、Hash 验证、原子安装和失败回退；
- 加入本地磁盘配额和 GC。

### Phase C：R2 Provider

- Wrangler 创建 `agnes-blobs` bucket 并绑定 Worker；
- 实现 Worker 授权的分块上传/下载；
- 实现 owner 隔离、配额、保留期和孤儿对象 GC；
- 只使用加密假数据完成故障测试后再开放真实数据。

### Phase D：Google Drive Provider

- 创建 Google Cloud OAuth Desktop Client，完成 PKCE 和 Keyring token 轮换；
- 实现 resumable upload、Range download、revision 与限流重试；
- 实现加密制品副本和 Drive 源文件连接器；
- 用两台设备测试自动决策、授权丢失和 R2/Drive 切换。

### Phase E：子功能 UI

- [x] 改造侧边栏为子功能列表 + 可折叠聊天会话 + 可折叠工作区会话；整体折叠时保留图标轨，折叠偏好只存本机；
- [x] 建立子功能视图宿主和功能注册表；当前只开放已实现的聊天入口，不展示空白功能页；
- [ ] 增加知识库、网盘、日历、待办页面路由；
- 网盘页显示 Provider 账户、配额、同步队列、副本和错误；
- 知识库页显示源文件、抽取/分块/向量进度和设备覆盖状态。

### Phase F：日历、待办与外部适配器

- 先完成 Local Provider 和 D1 E2EE 同步；
- 再接 Google Calendar / Google Tasks / CalDAV；
- 最后向 Agent 开放受 tool policy 约束的读写工具。

---

## 14. 验收标准

- 不配置任何网盘时，聊天、记忆、RAG、日历和待办的本地功能均可用。
- D1 仅根据清单 + object cursor/bootstrap 即可让设备决定 skip/download/rebuild/missing，不必全量扫描网盘。
- 修改单个文档只重建并上传对应分片，不重传整个知识库。
- 完全相同指纹的制品可在多设备复用；任一 profile 不同时必须重建。
- R2 / Drive 中不存在明文文件名、正文、chunk、向量、OAuth token 或密钥。
- 下载中断、Hash 不匹配、解密失败和导入崩溃都不破坏现有本地索引。
- 任一 Provider 授权失效只影响该 Provider，不阻塞应用或其他 Provider。
- 日历/待办在时区、重复事件、离线修改和冲突场景下可确定性收敛。
- 侧边栏的子功能、两类会话列表和整体折叠状态在桌面/移动端都不发生文字溢出或交互重叠。

---

## 15. 用户侧后续配置

当前不需要用户继续在 Cloudflare 控制台操作。

- R2 实现开始时：由 Wrangler 创建 bucket 和 Worker binding，不要手动创建或粘贴 R2 API Token。
- Google Drive 实现开始时：需用户在 Google Cloud Console 创建项目、启用 Drive API、配置 OAuth consent screen 和 Desktop Client ID；到该阶段再提供逐步操作。
- Google Calendar / Tasks 开始时：在同一 Google Cloud 项目按最小权限增加对应 API 和 scope。
- 夸克接入前：由用户明确接受社区逆向 adapter 的失效、风控和条款风险，不在核心应用中隐式启用。
