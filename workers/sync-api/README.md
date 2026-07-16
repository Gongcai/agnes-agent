# Agnes Sync API

V0.3 云同步的 Cloudflare Worker。当前 Phase 2 只允许使用假数据验证协议；真实聊天、角色卡和记忆必须等 E2EE 完成后再上传。

当前远端资源：

- Worker：`https://agnes-sync-api.caiwengong136.workers.dev`
- D1：`agnes-sync`（APAC）

远端 Worker 使用 `AUTH_MODE=bearer`，通过 Wrangler secret `SYNC_DEVICE_IDENTITIES`
配置每台设备的 token SHA-256 指纹和 owner/device 映射，不在 Worker 配置或仓库中保存明文令牌。

## 本地验证

```bash
pnpm install
pnpm sync:typecheck
pnpm sync:test
pnpm --filter @agnes/sync-api db:migrate:local
```

`sync:test` 在 Cloudflare `workerd`/Miniflare 中应用真实 D1 migration，覆盖认证、设备撤销、幂等 push、CAS、append-only message、owner 隔离、pull、bootstrap 和 ack。

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
未完成 E2EE 前，远端数据库只保存协议假数据。

## API

- `GET /v1/health`
- `POST /v1/sync/push`
- `GET /v1/sync/pull?after={serverSeq}&limit={n}`
- `GET /v1/sync/bootstrap?cursor={token}&limit={n}`
- `POST /v1/sync/ack`

协议版本固定为 `1`，单次 push 最多 20 条 change，请求体上限 256 KiB。完整设计见 `ProjectPlan/CLOUDFLARE_SYNC.md`。
