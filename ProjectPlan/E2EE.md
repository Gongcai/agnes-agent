# 同步端到端加密设计

本文冻结 Agnes Sync E2EE v1 的密码学格式、密钥边界与上线顺序。Cloudflare Worker、D1、R2
和网盘 Provider 都不持有解密密钥。当前实现状态为 **Phase 4A：密码学核心已实现，尚未接入生产
同步**；在 Keyring 初始化、恢复材料、新设备配对和迁移门禁完成前，仍禁止上传真实数据。

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
已有不同 keyset 时拒绝覆盖。未确认的本机 keyset 可显式丢弃后改用旧设备恢复材料，已确认 keyset
不能通过当前 UI 清除。

## 8. 接入顺序

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

- 首次 push 前一次性加密并持久化不可变密文，重试同一 `changeId` 必须复用相同密文；
- pull/bootstrap 在进入 DB apply 前验证 Hash 和 AEAD，解密后的 JSON 再走现有严格 payload 校验；
- Worker 接受密文 JSON string 但不解密；正式模式拒绝明文 `payloadEncoding=json`；
- 使用本地/独立 staging D1 完成假数据测试，不在 production 混入真实数据。

### Phase 4D：配对、轮换与上线

- 配对协议必须使用成熟的认证密钥交换/封装库，单独完成威胁建模，不自行拼装 ECDH；
- recovery export 加密整个 keyset，覆盖多版本恢复；
- 轮换期间新写入使用 active key，读取按 envelope key version 选择旧 key；
- 清空 POC 明文库或创建 encrypted-only production D1，完成日志审计和恢复演练后再开放真实数据。
