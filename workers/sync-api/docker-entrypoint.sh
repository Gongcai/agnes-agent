#!/bin/sh
set -eu

persist_dir=/data/wrangler
mkdir -p "$persist_dir"

pnpm exec wrangler d1 migrations apply agnes-sync --local --persist-to "$persist_dir"
exec pnpm exec wrangler dev \
  --local \
  --ip 0.0.0.0 \
  --port 8787 \
  --persist-to "$persist_dir"
