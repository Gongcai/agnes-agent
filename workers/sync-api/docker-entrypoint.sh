#!/bin/sh
set -eu

persist_dir=/data/wrangler
mkdir -p "$persist_dir"

pnpm exec wrangler d1 migrations apply agnes-sync --local --persist-to "$persist_dir"

memory_restart_mb=${SYNC_MEMORY_RESTART_MB:-768}
health_failure_limit=${SYNC_HEALTH_FAILURE_LIMIT:-5}
health_interval_seconds=${SYNC_HEALTH_INTERVAL_SECONDS:-15}
stop_timeout_seconds=${SYNC_STOP_TIMEOUT_SECONDS:-5}

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
case "$stop_timeout_seconds" in
  ''|*[!0-9]*)
    echo "SYNC_STOP_TIMEOUT_SECONDS must be a positive integer" >&2
    exit 1
    ;;
esac
if [ "$stop_timeout_seconds" -le 0 ]; then
  echo "SYNC_STOP_TIMEOUT_SECONDS must be a positive integer" >&2
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

# The image only ships dash's builtin kill (no external kill binary), and dash
# rejects the "--" option terminator, so the signal is passed as a plain option.
signal_server() {
  signal=$1
  if [ -n "${server_pgid:-}" ]; then
    kill "-$signal" "-$server_pgid" 2>/dev/null || true
  elif [ -n "${server_pid:-}" ]; then
    kill "-$signal" "$server_pid" 2>/dev/null || true
  fi
}

# An exited-but-unreaped process still answers kill -0, so the process state is
# checked as well to tell a zombie apart from a live worker.
server_running() {
  kill -0 "$server_pid" 2>/dev/null || return 1
  state=$(awk '{print $3}' "/proc/$server_pid/stat" 2>/dev/null) || return 0
  [ "$state" != "Z" ]
}

wait_for_server_exit() {
  limit=$1
  elapsed=0
  while server_running; do
    if [ "$elapsed" -ge "$limit" ]; then
      return 1
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  return 0
}

stop_server() {
  if [ -z "${server_pid:-}" ]; then
    return 0
  fi

  signal_server TERM
  if ! wait_for_server_exit "$stop_timeout_seconds"; then
    echo "Worker ignored SIGTERM after ${stop_timeout_seconds}s, sending SIGKILL" >&2
    signal_server KILL
    if ! wait_for_server_exit "$stop_timeout_seconds"; then
      # Give up and let the container restart policy take over instead of
      # blocking the supervisor forever and leaving a dead listener behind.
      echo "Worker could not be stopped, exiting so the container is restarted" >&2
      exit 1
    fi
  fi

  wait "$server_pid" 2>/dev/null || true
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
  fi
  stop_server
  sleep 2
done
