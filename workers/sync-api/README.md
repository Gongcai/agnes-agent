# Agnes Sync API

V0.3 云同步的 Cloudflare Worker。Phase 0-4 已完成，业务 payload 只接受 E2EE 密文；本轮部署和
验证未上传真实聊天、角色卡或记忆。

当前远端资源：

- Worker：`https://agnes-sync-api.caiwengong136.workers.dev`
- D1：`agnes-sync`（APAC）

远端 Worker 使用 `AUTH_MODE=bearer`。首台设备通过 Wrangler secret `SYNC_DEVICE_IDENTITIES`
配置 token SHA-256 指纹和 owner/device 映射；后续设备经 SPAKE2 配对后把唯一 fingerprint 登记到
D1。Worker 配置、D1 和仓库均不保存令牌明文或 E2EE keyset。

## 本地验证

```bash
pnpm install
pnpm sync:typecheck
pnpm sync:test
pnpm --filter @agnes/sync-api db:migrate:local
```

`sync:test` 在 Cloudflare `workerd`/Miniflare 中应用真实 D1 migration，覆盖认证、设备撤销、
SPAKE2 不透明配对中继、动态凭证、幂等 push、CAS、append-only message、owner 隔离、pull、
bootstrap 和 ack。

如需手动启动本地 Worker，在未提交的 `workers/sync-api/.dev.vars` 中配置测试身份：

```dotenv
AUTH_MODE="test"
SYNC_TEST_IDENTITIES='[{"token":"replace-with-a-random-token","ownerId":"local-owner","deviceId":"00000000-0000-4000-8000-000000000001","deviceName":"Local desktop","platform":"linux"}]'
```

然后运行：

```bash
pnpm sync:dev
```

测试请求使用 `Authorization: Bearer <token>`。`ownerId` 和 `deviceId` 来自 Worker 端的凭证映射，客户端不能自行指定 owner；请求中的 `deviceId` 只用于与已认证设备交叉校验。

## 远端资源

```bash
pnpm --filter @agnes/sync-api exec wrangler login
pnpm --filter @agnes/sync-api exec wrangler d1 create agnes-sync --location=apac
pnpm --filter @agnes/sync-api db:migrate:remote
pnpm --filter @agnes/sync-api run deploy
```

D1 创建后需要将 Wrangler 返回的 `database_id` 写入 `wrangler.jsonc`。本地 POC 使用
`SYNC_TEST_IDENTITIES`；远端 `SYNC_DEVICE_IDENTITIES` 条目格式为
`{"tokenSha256":"...","ownerId":"...","deviceId":"...","deviceName":"...","platform":"..."}`，
两者均只通过未提交的本地变量或 Wrangler secret 配置。当前 Cloudflare 账户没有可用
Zone，无法为 Worker 绑定自定义域名 Access，因此按设计回退为 Worker 自管设备令牌；
E2EE、配对和轮换均已完成，production D1 在部署后复核为空。

## API

- `GET /v1/health`
- `POST /v1/sync/push`
- `GET /v1/sync/pull?after={serverSeq}&limit={n}`
- `GET /v1/sync/bootstrap?cursor={token}&limit={n}`
- `POST /v1/sync/ack`
- `GET /v1/devices`
- `POST /v1/devices/{deviceId}/revoke`
- `POST /v1/pairing/sessions`（旧设备认证）
- `GET /v1/pairing/sessions/{sessionId}`（公开；仅返回 SPAKE2 A 消息）
- `POST /v1/pairing/sessions/{sessionId}/join`（公开；仅接收 SPAKE2 B 消息与加密 proof）
- `GET /v1/pairing/sessions/{sessionId}/join`（旧设备认证）
- `POST /v1/pairing/sessions/{sessionId}/finalize`（旧设备认证）
- `GET /v1/pairing/sessions/{sessionId}/package`（公开；仅返回加密 transfer bundle）
- `POST /v1/pairing/sessions/{sessionId}/consume`（新设备认证）

协议版本固定为 `1`，单次 push 最多 20 条 change，请求体上限 256 KiB。配对会话 10 分钟过期，
每小时清理。完整协议与威胁模型见 `ProjectPlan/E2EE.md` 和 `ProjectPlan/CLOUDFLARE_SYNC.md`。
