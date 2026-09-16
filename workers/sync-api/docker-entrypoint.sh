#!/bin/sh
set -eu

persist_dir=/data/wrangler
mkdir -p "$persist_dir"

pnpm exec wrangler d1 migrations apply agnes-sync --local --persist-to "$persist_dir"

memory_restart_mb=${SYNC_MEMORY_RESTART_MB:-768}
health_failure_limit=${SYNC_HEALTH_FAILURE_LIMIT:-5}
health_interval_seconds=${SYNC_HEALTH_INTERVAL_SECONDS:-15}

case "$memory_restart_mb" in
  ''|*[!0-9]*)
    echo "SYNC_MEMORY_RESTART_MB must be a positive integer" >&2
    exit 1
    ;;
esac
if [ "$memory_restart_mb" -le 0 ]; then
  echo "SYNC_MEMORY_RESTART_MB must be a positive integer" >&2
  exit 1
fi
case "$health_failure_limit" in
  ''|*[!0-9]*)
    echo "SYNC_HEALTH_FAILURE_LIMIT must be a positive integer" >&2
    exit 1
    ;;
esac
if [ "$health_failure_limit" -le 0 ]; then
  echo "SYNC_HEALTH_FAILURE_LIMIT must be a positive integer" >&2
  exit 1
fi
case "$health_interval_seconds" in
  ''|*[!0-9]*)
    echo "SYNC_HEALTH_INTERVAL_SECONDS must be a positive integer" >&2
    exit 1
    ;;
esac
if [ "$health_interval_seconds" -le 0 ]; then
  echo "SYNC_HEALTH_INTERVAL_SECONDS must be a positive integer" >&2
  exit 1
fi

memory_restart_bytes=$((memory_restart_mb * 1024 * 1024))
server_pid=
server_pgid=

read_memory_bytes() {
  if [ -r /sys/fs/cgroup/memory.current ]; then
    cat /sys/fs/cgroup/memory.current
    return
  fi
  if [ -r /sys/fs/cgroup/memory/memory.usage_in_bytes ]; then
    cat /sys/fs/cgroup/memory/memory.usage_in_bytes
    return
  fi
  printf '0\n'
}

stop_server() {
  if [ -n "${server_pgid:-}" ]; then
    kill -TERM -- "-$server_pgid" 2>/dev/null || true
  elif [ -n "${server_pid:-}" ]; then
    kill -TERM "$server_pid" 2>/dev/null || true
  fi
  if [ -n "${server_pid:-}" ]; then
    wait "$server_pid" 2>/dev/null || true
  fi
  server_pid=
  server_pgid=
}

shutdown() {
  stop_server
  exit 0
}

trap shutdown INT TERM HUP

while :; do
  echo "Starting local Worker (memory restart threshold: ${memory_restart_mb} MiB)"
  if command -v setsid >/dev/null 2>&1; then
    setsid pnpm exec wrangler dev \
      --local \
      --ip 0.0.0.0 \
      --port 8787 \
      --persist-to "$persist_dir" &
    server_pgid=$!
  else
    pnpm exec wrangler dev \
      --local \
      --ip 0.0.0.0 \
      --port 8787 \
      --persist-to "$persist_dir" &
    server_pgid=
  fi
  server_pid=$!
  health_failures=0
  restart_reason=

  while kill -0 "$server_pid" 2>/dev/null; do
    sleep "$health_interval_seconds"
    if ! kill -0 "$server_pid" 2>/dev/null; then
      break
    fi

    if node ./healthcheck.mjs; then
      health_failures=0
    else
      health_failures=$((health_failures + 1))
      echo "Worker health probe failed (${health_failures}/${health_failure_limit})" >&2
      if [ "$health_failures" -ge "$health_failure_limit" ]; then
        restart_reason=health
        break
      fi
    fi

    memory_bytes=$(read_memory_bytes)
    if [ "$memory_bytes" -ge "$memory_restart_bytes" ]; then
      restart_reason=memory
      echo "Restarting Worker after cgroup memory reached ${memory_bytes} bytes" >&2
      break
    fi
  done

  if [ -n "$restart_reason" ]; then
    echo "Stopping Worker for ${restart_reason} recovery" >&2
    stop_server
  else
    stop_server
  fi
  sleep 2
done
