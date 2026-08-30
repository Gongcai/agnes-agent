FROM node:22-bookworm-slim

WORKDIR /app

RUN corepack enable

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY workers/sync-api/package.json workers/sync-api/package.json
RUN pnpm install --frozen-lockfile --filter @agnes/sync-api...

COPY workers/sync-api workers/sync-api
RUN chmod +x workers/sync-api/docker-entrypoint.sh

WORKDIR /app/workers/sync-api
ENTRYPOINT ["./docker-entrypoint.sh"]
