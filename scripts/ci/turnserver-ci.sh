#!/usr/bin/env bash
# Start/stop the test TURN server (coturn) inside the CI container.
#
# Usage: turnserver-ci.sh start|stop
#
# The ci-test image ships /usr/bin/turnserver. CI jobs run inside that image and start coturn
# themselves (no Docker daemon, no sidecar). Credentials match packages/sdk/tests/turn.test.ts:
# testuser / testpass, realm test.local, 127.0.0.1:3478.
#
# --no-cli: coturn 4.6 refuses --allow-loopback-peers together with an empty CLI password.
#
# Pid file and log live under $ROOT/.test-logs/ (turnserver.pid, turnserver.log).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LOG_DIR="$ROOT/.test-logs"
PID_FILE="$LOG_DIR/turnserver.pid"
LOG="$LOG_DIR/turnserver.log"

probe() { (exec 3<>/dev/tcp/127.0.0.1/3478) 2>/dev/null; }

start() {
  if probe; then
    echo "==> TURN already listening on 127.0.0.1:3478"
    return 0
  fi
  if ! command -v turnserver >/dev/null 2>&1; then
    echo "TURN_AVAILABLE=1 but turnserver is not installed and nothing listens on 127.0.0.1:3478" >&2
    return 1
  fi
  mkdir -p "$LOG_DIR"
  turnserver -n --listening-ip=0.0.0.0 --listening-port=3478 --lt-cred-mech \
    --user=testuser:testpass --realm=test.local --min-port=49152 --max-port=65535 \
    --no-tls --no-dtls --allow-loopback-peers --external-ip=127.0.0.1/127.0.0.1 --no-cli \
    --log-file=stdout -v >"$LOG" 2>&1 &
  local pid=$!
  echo "$pid" >"$PID_FILE"

  # Bounded wait: 50 x 0.2 s = 10 s (infra.unbounded-external-wait).
  local i
  for ((i = 0; i < 50; i++)); do
    if probe; then
      echo "==> TURN listening on 127.0.0.1:3478 (pid $pid)"
      return 0
    fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
  done
  echo "turnserver did not listen on 127.0.0.1:3478 within 10 s" >&2
  tail -n 40 "$LOG" >&2 || true
  kill "$pid" 2>/dev/null || true
  rm -f "$PID_FILE"
  return 1
}

stop() {
  if [[ -f "$PID_FILE" ]]; then
    local pid
    pid="$(cat "$PID_FILE" 2>/dev/null || true)"
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      local i
      for ((i = 0; i < 25; i++)); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.2
      done
      if kill -0 "$pid" 2>/dev/null; then
        kill -9 "$pid" 2>/dev/null || true
      fi
    fi
    rm -f "$PID_FILE"
  fi
  return 0
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  *)
    echo "usage: turnserver-ci.sh start|stop" >&2
    exit 2
    ;;
esac
