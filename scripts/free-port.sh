#!/usr/bin/env bash
# Stop any process listening on a TCP port before starting a dev example server.
#
# Mirrors examples/shared/free-port.ts — run from npm scripts so the port is free
# before Node loads Sherpa models or binds the socket.
#
# Usage:
#   bash scripts/free-port.sh [port] [label]
#   PORT=3004 bash scripts/free-port.sh [label]
#   bash scripts/free-port.sh 3004 my-server -- tsx src/index.ts
#
# Skip:
#   VOICE_SKIP_FREE_PORT=1 bash scripts/free-port.sh 3004
#
set -euo pipefail

if [[ "${VOICE_SKIP_FREE_PORT:-}" == "1" ]]; then
  if [[ "${1:-}" == "--" ]]; then
    shift
    exec "$@"
  fi
  exit 0
fi

_port_arg=""
_label_arg=""
_have_rest=0
if [[ "${1:-}" == "--" ]]; then
  _have_rest=1
else
  if [[ -n "${1:-}" && "${1}" =~ ^[0-9]+$ ]]; then
    _port_arg="$1"
    shift
  fi
  if [[ -n "${1:-}" && "${1}" != "--" ]]; then
    _label_arg="$1"
    shift
  fi
  if [[ "${1:-}" == "--" ]]; then
    _have_rest=1
    shift
  fi
fi

PORT="${_port_arg:-${PORT:-}}"
LABEL="${_label_arg:-${FREE_PORT_LABEL:-server}}"

if [[ -z "${PORT}" || ! "${PORT}" =~ ^[0-9]+$ || "${PORT}" -lt 1 || "${PORT}" -gt 65535 ]]; then
  echo "free-port.sh: set PORT or pass a valid port (1-65535) as the first argument" >&2
  exit 1
fi

_kill_pid() {
  local pid="$1"
  [[ -z "${pid}" || ! "${pid}" =~ ^[0-9]+$ || "${pid}" -le 0 ]] && return 0
  [[ "${pid}" -eq "$$" ]] && return 0
  kill -0 "${pid}" 2>/dev/null || return 0
  kill -9 "${pid}" 2>/dev/null
}

_collect_pids_unix() {
  local pid
  while IFS= read -r pid; do
    [[ -n "${pid}" ]] && echo "${pid}"
  done < <(lsof -ti ":${PORT}" -sTCP:LISTEN 2>/dev/null || true)
}

_collect_pids_windows() {
  netstat -ano -p tcp 2>/dev/null | awk -v port=":${PORT}" '
    $0 ~ port && $0 ~ /LISTENING/ { print $NF }
  ' || true
}

stopped=0
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN* | Windows*)
    while IFS= read -r pid; do
      if _kill_pid "${pid}"; then
        stopped=$((stopped + 1))
      fi
    done < <(_collect_pids_windows | sort -u)
    ;;
  *)
    while IFS= read -r pid; do
      if _kill_pid "${pid}"; then
        stopped=$((stopped + 1))
      fi
    done < <(_collect_pids_unix)
    ;;
esac

if [[ "${stopped}" -gt 0 ]]; then
  echo "[free-port] Stopped ${stopped} process(es) on port ${PORT} before starting ${LABEL}"
fi

if [[ "${_have_rest}" -eq 1 ]]; then
  exec "$@"
fi
