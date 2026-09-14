#!/usr/bin/env bash
# JSON fixtures + actual HTTP/status/envelope assertions. Node 20+ is already
# required by the cross-repository browser harness; no jq or Python shim needed.
set -euo pipefail
exec node "$(cd "$(dirname "$0")" && pwd)/verify-platform-json.mjs"
