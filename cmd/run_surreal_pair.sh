#!/bin/bash
set -euo pipefail

# Resolve repo root (script is in cmd/)
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DB_OPTION_TOML="$REPO_ROOT/DbOption.toml"
SURREAL_BIN="/usr/local/bin/surreal"

if [[ ! -x "$SURREAL_BIN" ]]; then
  echo "Error: surreal binary not found or not executable at: $SURREAL_BIN" >&2
  exit 1
fi

# Helper to extract simple key = value (supports quoted strings) from TOML
get_toml_value() {
  local key="$1"
  local default_value="${2:-}"
  local raw
  raw="$(grep -E "^${key}[[:space:]]*=" "$DB_OPTION_TOML" | tail -n1 | awk -F'=' '{print $2}' | xargs)" || true
  if [[ -z "$raw" ]]; then
    echo "$default_value"
    return
  fi
  # Strip surrounding quotes if present
  raw="${raw%\"}"
  raw="${raw#\"}"
  echo "$raw"
}

# Return PIDs listening on a TCP port (macOS compatible)
get_listen_pids_for_port() {
  local port="$1"
  lsof -n -P -t -iTCP:"$port" -sTCP:LISTEN 2>/dev/null || true
}

# Stop process from pid file gracefully, then force if needed
stop_by_pidfile() {
  local pidfile="$1"
  local label="$2"
  if [[ -f "$pidfile" ]]; then
    local pid
    pid="$(cat "$pidfile" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      echo "Stopping $label (pid=$pid) ..."
      kill "$pid" 2>/dev/null || true
      for _ in {1..20}; do
        if kill -0 "$pid" 2>/dev/null; then
          sleep 0.25
        else
          break
        fi
      done
      if kill -0 "$pid" 2>/dev/null; then
        echo "Force killing $label (pid=$pid) ..."
        kill -9 "$pid" 2>/dev/null || true
      fi
    fi
    rm -f "$pidfile" || true
  fi
}

# Ensure a TCP port is free: try pidfile, then any listeners on the port
ensure_port_free() {
  local port="$1"
  local label="$2"
  local pidfile="$3"
  stop_by_pidfile "$pidfile" "$label"
  local pids
  pids="$(get_listen_pids_for_port "$port" | tr '\n' ' ')"
  if [[ -n "$pids" ]]; then
    echo "$label port $port is in use by PID(s): $pids. Attempting to stop them..."
    for pid in $pids; do
      if kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
      fi
    done
    sleep 0.5
    pids="$(get_listen_pids_for_port "$port" | tr '\n' ' ')"
    if [[ -n "$pids" ]]; then
      for pid in $pids; do
        if kill -0 "$pid" 2>/dev/null; then
          kill -9 "$pid" 2>/dev/null || true
        fi
      done
    fi
  fi
}

# Read configs from DbOption.toml
V_IP="$(get_toml_value v_ip "127.0.0.1")"
V_PORT="$(get_toml_value v_port "8009")"
V_USER="$(get_toml_value v_user "root")"
V_PASS="$(get_toml_value v_password "root")"

KV_IP="$(get_toml_value kv_ip "127.0.0.1")"
KV_PORT="$(get_toml_value kv_port "8010")"

# Normalize hostnames not accepted by Surreal bind syntax
if [[ "$V_IP" == "localhost" ]]; then V_IP="127.0.0.1"; fi
if [[ "$KV_IP" == "localhost" ]]; then KV_IP="127.0.0.1"; fi

# Database file locations
ROCKS_PATH="$REPO_ROOT/ams-${V_PORT}-test.db"
KV_FILE_PATH="$REPO_ROOT/surreal-${KV_PORT}.kv"

# Logs and PIDs
LOG_DIR="$REPO_ROOT"
V_LOG="$LOG_DIR/surreal-${V_PORT}.log"
KV_LOG="$LOG_DIR/surreal-${KV_PORT}.log"
PID_DIR="$REPO_ROOT"
V_PID="$PID_DIR/surreal-${V_PORT}.pid"
KV_PID="$PID_DIR/surreal-${KV_PORT}.pid"

ensure_port_free "$V_PORT" "SurrealDB (RocksDB)" "$V_PID"
ensure_port_free "$KV_PORT" "SurrealDB (KV)" "$KV_PID"

echo "Starting SurrealDB (RocksDB) on ${V_IP}:${V_PORT} ..."
nohup "$SURREAL_BIN" start \
  --log info \
  --user "$V_USER" \
  --pass "$V_PASS" \
  --bind "${V_IP}:${V_PORT}" \
  "rocksdb://$ROCKS_PATH" \
  >"$V_LOG" 2>&1 &
echo $! > "$V_PID"
echo "  - log: $V_LOG"
echo "  - pid: $V_PID"

echo "Starting SurrealDB (KV) on ${KV_IP}:${KV_PORT} ..."
nohup "$SURREAL_BIN" start \
  --log info \
  --user "$V_USER" \
  --pass "$V_PASS" \
  --bind "${KV_IP}:${KV_PORT}" \
  "surrealkv://$KV_FILE_PATH" \
  >"$KV_LOG" 2>&1 &
echo $! > "$KV_PID"
echo "  - log: $KV_LOG"
echo "  - pid: $KV_PID"

echo "Done. Two SurrealDB instances are starting in the background."


