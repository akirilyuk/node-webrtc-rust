#!/usr/bin/env bash
# Deprecated alias — use run-pre-push-gates.sh (lint + helpers vitest).
exec "$(dirname "$0")/run-pre-push-gates.sh" "$@"
