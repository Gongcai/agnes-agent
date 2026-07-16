# 同步端到端加密设计

本文冻结 Agnes Sync E2EE v1 的密码学格式、密钥边界与上线顺序。Cloudflare Worker、D1、R2
和网盘 Provider 都不持有解密密钥。当前实现状态为 **Phase 4D：安全配对、密钥轮换与恢复演练已完成**；
桌面端和线上 Worker 已启用 encrypted-only、SPAKE2 一次性配对与多版本 keyset 契约。Worker 版本
`7316feb3-48b1-4635-8363-a83e78e7dc33` 已部署，production D1 的业务表、设备表与配对表均复核为
0。本轮仍未上传真实业务数据。

## 1. 安全目标

E2EE v1 保护离开客户端后的业务 payload：

- Worker、D1、日志和对象存储无法读取聊天、角色卡、记忆或 workspace 逻辑内容；
- 密文不能被替换到另一实体、revision、设备或 key version 后仍通过认证；
- 每次加密使用独立随机 nonce，同一明文不会产生相同密文；
- 旧 key version 在轮换期间仍可解密，直到所有远端快照完成重加密和设备确认；
- 错误密钥、损坏密文或 AAD 篡改必须使整页 pull 回滚，不推进 cursor，也不发送 ack。

E2EE v1 不解决：

- 本地 SQLite 的静态加密；本地磁盘保护依赖操作系统账户、全盘加密和 Keyring；
- owner/device、实体类型和 ID、revision、时间、密文长度及访问频率等元数据泄露；
- 恶意或故障云端删除、延迟、重放旧快照；已有本地状态可拒绝 revision 回退，但首次 bootstrap 的
  新设备没有独立透明日志，不能仅凭 AEAD 证明拿到的是最新快照；
- 已解锁客户端、恶意插件、键盘记录或屏幕录制；
- 用户丢失所有已配对设备和恢复材料后的数据恢复。

## 2. 算法与依赖

| 项目 | v1 选择 |
|---|---|
| AEAD | XChaCha20-Poly1305，RustCrypto `chacha20poly1305 0.11` |
| 主密钥 | 每个账户随机 256 bit Sync Master Key |
| nonce | 每个 upsert 随机 192 bit，禁止复用 |
| tag | Poly1305 128 bit tag，由库附加在 ciphertext 尾部 |
| payload hash | `SHA-256(nonce || ciphertext || tag)`，小写十六进制 |
| 文本编码 | 解密前为紧凑 UTF-8 JSON；Worker 不解析 |
| 外层编码 | RFC 4648 Base64，无 padding |
| 内存清理 | key、keyset 临时字符串和明文缓冲区使用 `zeroize` |

不自创加密算法，不截断 tag，不使用客户端时间或实体 ID 直接派生 nonce。删除墓碑没有业务
payload，保持 `payload = null`；删除行为本身属于明确允许泄露的元数据。

## 3. 密文 payload

加密 upsert 的 envelope 字段固定为：

```json
{
  "payloadEncoding": "xchacha20poly1305-v1",
  "payload": "BASE64_NOPAD(NONCE_24 || CIPHERTEXT || TAG_16)",
  "payloadHash": "SHA256_HEX(NONCE_24 || CIPHERTEXT || TAG_16)",
  "keyVersion": 1
}
```

Hash 用于传输损坏和幂等比较，不能替代 AEAD tag。客户端必须先验证 Base64 规范形式、最小长度和
Hash，再执行 AEAD 解密。任何错误都按无效加密 payload 处理，不把密钥、明文、nonce+明文组合
或完整密文写入日志。

删除固定使用规范墓碑，不携带 key version：

```json
{
  "payloadEncoding": "tombstone-v1",
  "payload": null,
  "payloadHash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  "keyVersion": null
}
```

其中 Hash 是 `SHA-256(empty bytes)`。Worker 对 upsert 和 delete 的字段组合做严格校验，拒绝
`payloadEncoding=json`、对象 payload、缺失 key version 和非规范墓碑。

## 4. Associated Data

AAD 使用固定二进制编码，不依赖 JSON key 顺序。整数均为 big-endian；字符串字段使用
`u32_be length || UTF-8 bytes`。顺序不可修改：

| 顺序 | 字段 | 编码 |
|---:|---|---|
| 1 | domain | ASCII `agnes-sync-payload-aad-v1\0` |
| 2 | protocol version | `u8` |
| 3 | entity type | length-prefixed UTF-8 |
| 4 | entity ID | length-prefixed UTF-8 |
| 5 | resulting revision | `i64_be` |
| 6 | HLC | length-prefixed UTF-8 |
| 7 | payload schema version | `i64_be` |
| 8 | origin device ID | length-prefixed UTF-8 |
| 9 | payload encoding | length-prefixed ASCII `xchacha20poly1305-v1` |
| 10 | key version | `i64_be` |

发送端可由 `baseRevision + 1` 得到 resulting revision；pull 使用 `resultingRevision`；bootstrap
使用实体 `revision`。origin device 在 push/pull 中是 `deviceId`，在 bootstrap 中是
`changedByDeviceId`。这些值被 Worker 用于授权、CAS 或路由，但 Worker 修改其中任一值都会导致
客户端认证失败。

## 5. Keyset

账户级 keyset 使用严格 JSON，只存入 OS Keyring / Android Keystore，不进入 SQLite、renderer
IPC、同步 payload 或诊断导出。计划使用的桌面 Keyring secret ID 为 `sync:e2ee:keyset:v1`。

```json
{
  "formatVersion": 1,
  "activeKeyVersion": 2,
  "keys": [
    { "version": 1, "key": "BASE64_NOPAD_32_BYTES" },
    { "version": 2, "key": "BASE64_NOPAD_32_BYTES" }
  ]
}
```

约束：

- version 为正整数且不可重复，active version 必须存在；
- key 必须是规范无 padding Base64，解码后严格为 32 bytes；
- 最多同时保留 32 个版本，避免损坏或恶意恢复材料无限占用内存；
- 轮换只增加 version，不覆盖旧 key；达到上限时必须先完成受审计的归档/淘汰流程；
- 内部 key 类型不实现 `Debug`，所有解析早退路径也必须清零已解码或已反序列化的 key。

## 6. 固定测试向量

该向量用于 Rust、Android 和未来其他客户端做格式兼容验证：

```text
key              = 32 bytes of 0x07
nonce            = 24 bytes of 0x09
protocolVersion  = 1
entityType       = memory
entityId         = memory-1
revision         = 7
hlc              = 1784188800123-0004-device01
schemaVersion    = 1
originDeviceId   = 00000000-0000-4000-8000-000000000001
keyVersion       = 3
plaintext JSON   = {"content":"private memory"}
payload          = CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJxVCB5RLeHAyAMDIeGAFunUuH0rotBPCO1PQ6IZFo5qqHKBwKuLekgSVycXw
payloadHash      = 1cd7e8c0978997fe41774162553a5321e1335df5a38919f08712e960bbb3f15c
```

## 7. 恢复材料格式

恢复材料由两项组成，必须分别保存：

```text
Recovery Key    = agnes-recovery-key-v1.BASE64URL_NOPAD(random 32 bytes)
Recovery Bundle = agnes-recovery-bundle-v1.BASE64URL_NOPAD(bundle JSON)
```

bundle JSON 为严格结构：

```json
{
  "formatVersion": 1,
  "kdf": "hkdf-sha256",
  "cipher": "xchacha20poly1305",
  "salt": "BASE64URL_NOPAD(random 16 bytes)",
  "nonce": "BASE64URL_NOPAD(random 24 bytes)",
  "ciphertext": "BASE64URL_NOPAD(encrypted full keyset)"
}
```

Recovery Key 是随机 256-bit 高熵密钥，不是用户口令，因此 HKDF-SHA256 只用于域隔离，不承担
低熵口令拉伸。派生 info 固定为 `agnes-sync-recovery-wrap-key-v1`；恢复包 AEAD associated data
固定为 `agnes-sync-recovery-bundle-v1\0`。恢复时严格验证前缀、规范 Base64URL、字段、长度、tag
和完整 keyset，错误统一返回“不匹配”，不暴露内部 key。

每次导出都会为同一 keyset 生成新的 Recovery Key、salt 和 nonce。Renderer 只能一次性接收这两项
恢复材料，不能读取 master key 或 keyset JSON。新设备恢复后把 keyset 写入 Keyring 并读回验证；
已有 keyset 时只允许“所有旧 version 与 key 字节完全相同、active version 单调增加、至少新增一个
version”的升级，拒绝分叉、替换旧 key、删减或降级。未确认的本机 keyset 可显式丢弃后改用旧设备
恢复材料，已确认 keyset 不能通过当前 UI 清除。

## 8. 新设备安全配对

### 8.1 协议与格式

配对使用 RustCrypto `spake2 0.4` 的 SPAKE2/Ed25519Group，不自行拼装 ECDH。旧设备固定为 A 角色，
新设备固定为 B 角色；双方 identity 绑定协议域与 `sessionId`：

```text
idA = agnes-sync-pairing-initiator-v1:<sessionId>
idB = agnes-sync-pairing-responder-v1:<sessionId>
```

旧设备生成随机 256-bit pairing secret，与随机 UUID session ID 一起编码为一次性能力码：

```text
agnes-pair-v1.BASE64URL_NOPAD({ formatVersion, sessionId, secret })
```

能力码必须通过用户认可的可信通道传递。SPAKE2 共享结果再用 HKDF-SHA256 和固定 info
`agnes-sync-pairing-wrap-key-v1` 派生 256-bit wrapping key。新设备先发送使用
`agnes-sync-pairing-responder-proof-v1\0` AAD 加密的设备声明，旧设备只有成功解密且逐字段匹配
Worker 外层元数据后才批准。旧设备随后用 `agnes-sync-pairing-transfer-v1\0` AAD 加密以下严格结构：

```json
{
  "formatVersion": 1,
  "sessionId": "UUID",
  "deviceId": "UUID",
  "bearerToken": "agnes-device-token-v1.BASE64URL_NOPAD(random 32 bytes)",
  "keysetJson": "FULL_KEYSET_JSON"
}
```

Worker 仅保存 SPAKE2 消息、设备元数据、Bearer SHA-256 指纹和 XChaCha20-Poly1305 密文包，不得到
pairing secret、Bearer 明文或 keyset。新设备解密后在 Rust 内把凭证和 keyset 原子写入 Keyring，
逐项读回验证，再提交本地 `e2ee_key_version`；Renderer 只持有一次性配对码和状态。

### 8.2 服务端会话约束

- 会话 10 分钟过期，每个发起设备同时最多 5 个活动会话，一个 session 只接受第一个 join；
- create/inspect/finalize 只允许原发起设备凭证，公开 get/join/package 依赖不可猜测 session UUID 和
  高熵 pairing secret，返回内容仍由 PAKE/AEAD 保护；
- finalize 在 D1 batch 内同时登记独立设备凭证指纹和 transfer bundle，重复相同请求幂等；
- 新设备安装并认证后 consume 会立即清除 transfer bundle；Cron 每小时删除所有过期会话；
- 撤销设备时同时删除它发起或请求的未完成配对会话。

### 8.3 威胁模型

| 威胁 | 缓解 | 剩余风险 / 操作要求 |
|---|---|---|
| 恶意 Worker / 网络 MITM 替换 SPAKE2 消息 | SPAKE2 identity 绑定 session；proof 和 transfer 使用不同 AAD 的 AEAD | 可阻断或延迟配对，不能静默得到 keyset |
| 被动监听后离线猜测 | pairing secret 为随机 256 bit；SPAKE2 不暴露可验证的离线口令材料 | 用户泄露完整能力码等同授权一次配对 |
| 在线抢先 join / DoS | session UUID 不可猜；只接受一次 join；10 分钟过期；每设备限 5 个会话 | 获得能力码的攻击者可抢先 join，用户需关闭并重新生成 |
| 重放 proof / transfer | session ID、设备 ID、SPAKE2 transcript 与 AEAD 绑定；finalize/consume 状态机一次性 | finalize 响应丢失可幂等重试；已 consume/过期包不可再取 |
| Worker 读取 Bearer 或 keyset | 只接收 token SHA-256 指纹和加密 transfer bundle | Worker 仍可见设备名、平台、时间和密文长度 |
| 已攻陷旧设备 | 旧设备本就持有完整 keyset，并可在凭证有效时批准新设备 | 立即撤销该设备并在可信设备上轮换；旧数据不能追溯保密 |
| SPAKE2 实现计时与进程内存残留 | 使用成熟 RustCrypto 实现；秘密为单次随机 256 bit；原始解码 buffer 使用 `zeroize`；过期 exchange 在状态刷新/后台周期内清理 | `spake2 0.4` 自述非恒定时间且内部 state 未实现 `zeroize`，因此不用于低熵长期口令认证；已解锁进程内存读取不在 v1 防护范围 |
| Renderer / 剪贴板泄露能力码 | 配对码关闭设置页或完成后清空 React state；keyset/token 不经 IPC | 复制后的系统剪贴板由用户和操作系统负责清理 |

## 9. 密钥轮换与撤销处置

轮换是显式两阶段事务：

1. Rust 在现有 keyset 末尾生成新随机 key，先把 `activeKeyVersion + 1` 的完整 keyset 写入 Keyring
   并读回验证；SQLite 中已确认的 `e2ee_key_version` 暂不变化；
2. `activeKeyVersion != e2ee_key_version` 时状态为 `rotation_pending`，后台同步完全暂停，避免用户尚未
   保存新恢复材料时产生只有单份副本可解密的 vNext 密文；
3. 用户分别保存新的 Recovery Key/Bundle 并确认后，SQLite 原子推进 confirmed version。之后新
   outbox 首次加密使用 vNext，已固化的旧 outbox 和远端旧密文继续按 envelope version 读取；
4. 其他已配置设备导入新 Recovery Bundle 时，仅接受对本机 keyset 的单调超集升级；新配对设备
   直接获得当前完整多版本 keyset。

设备撤销只阻断其 Worker 凭证，不能抹除它已经获得的旧 key 或明文。因此疑似失控设备的标准处置
是“先撤销，再立即轮换”。新 key 不发给已撤销设备，可保护轮换后的新增/修改内容；它仍可能解密
轮换前已经持有或从云端取得的旧版本密文。当前不淘汰旧 key，也不承诺旧数据的追溯保密；未来只有
在全量远端快照重加密、所有保留设备确认和独立恢复审计完成后，才允许设计旧 key 归档/删除流程。

恢复演练由隔离的 source/target 两套 DB 与 Keyring 替身执行：target 先用 v1 Recovery Bundle 完成
新设备恢复，source 生成并确认 v2，target 再导入 v2 多版本 bundle；最终同一 target 分别成功解密
轮换前 v1 和轮换后 v2 的固定业务 payload，并验证分叉/降级材料被拒绝。

## 10. 接入顺序

### Phase 4A：密码学核心（已完成）

- XChaCha20-Poly1305 seal/open、随机 nonce、密文 Hash 和固定 AAD；
- 多版本 keyset 的生成、严格解析、序列化和轮换基础；
- round-trip、随机 nonce、坏 Hash、错误 key、AAD/密文篡改和固定向量测试。

### Phase 4B：本设备初始化与恢复材料（已完成）

- Tauri command 在 Rust 内生成或恢复 keyset、写入 Keyring 并读回验证；
- Renderer 只接收状态、版本和一次性 Recovery Key/Bundle，不读取 Sync Master Key/keyset JSON；
- 用户确认分别保存两项恢复材料后，才写入本地 `e2ee_key_version`；
- `run_once` 已启用 encrypted-only 门禁。Phase 4C 完成前业务 push/pull/bootstrap 全部暂停，设备
  查询和撤销等不含业务 payload 的管理 API 仍可使用；
- 未确认 keyset 可显式丢弃且验证删除结果；已确认 keyset 不提供清除入口，认证凭证清除保持独立。

### Phase 4C：传输接入

**已完成（2026-07-16）**：

- outbox 从 `pending` claim 为 `in_flight` 后，首次 push 使用 active key 加密，并在同一 SQLite
  事务中固化 encoding、密文、密文 Hash 和 key version；只有事务成功才构造 HTTP 请求；
- `source_payload` 仅保留在本机 outbox，供 accepted 基线和冲突合并使用，不进入协议。网络失败、
  响应丢失或进程重启后，同一 `changeId` 复用已经固化的 nonce/密文；
- pull/bootstrap 在进入 DB actor 前整页验证 envelope、密文 Hash、key version、AAD 和 AEAD，
  解密后的 JSON 再走既有严格业务白名单；任一实体失败时不写业务行、不推进 cursor、不 ack；
- Worker 只接受 `xchacha20poly1305-v1` upsert 或 `tombstone-v1` delete，把密文字符串作为不透明
  JSON 值存取，不持有密钥也不解析业务 payload；
- encrypted-only Worker 版本 `e7e09963-effe-4f12-a81d-5690b81eb853` 已部署。部署前后 production
  D1 的 `sync_entities / sync_changes / sync_acks / devices` 均为 0，未上传测试或真实业务数据。

### Phase 4D：配对、轮换与上线

**已完成（2026-07-17）**：

- 使用 RustCrypto SPAKE2 + HKDF + XChaCha20-Poly1305 完成一次性新设备配对；Worker 只做短期
  不透明中继和设备凭证指纹登记；
- 完成两阶段密钥轮换、同步暂停门禁、Recovery Bundle 单调升级与新旧 key version 共存读取；
- 完成 source/target 跨设备恢复和 v1 → v2 轮换演练，错误密钥、分叉和降级均被拒绝；
- 实时 tail 审计显示 health 和假配对 POST 仅记录 method、脱敏 URL 与 outcome，不记录 header/body；
- production D1 migration `0003_secure_pairing.sql` 已应用，Worker 版本
  `7316feb3-48b1-4635-8363-a83e78e7dc33` 已部署；五张相关表复核为 0，本轮未上传业务数据。
