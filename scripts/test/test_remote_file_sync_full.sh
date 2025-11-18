#!/usr/bin/env bash
set -euo pipefail

# 完整自动化 BJ / SJZ 远程文件服务器同步测试
# 步骤：
# 1. 启动 SurrealDB（带健康检查与 NS/DB 初始化）
# 2. 准备测试数据目录与 assets/archives（调用 test_remote_file_sync.sh）
# 3. 构建 web_server 二进制
# 4. 启动 BJ / SJZ 两个 web_server 实例（不同 PORT / DbOption）
# 5. 等待 HTTP 就绪，启动内部 MQTT 服务器
# 6. 通过 API 从 DbOption 导入环境并激活 runtime
# 7. 调用脚本触发一次 BJ→SJZ 新增 DB 文件同步事件

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "[0/7] 项目根目录: $ROOT_DIR"

#############################
# 1. 启动 SurrealDB
#############################

DB_PORT="${DB_PORT:-8020}"
DB_FILE="${DB_FILE:-ams-demo.db}"
DB_BIND="${DB_BIND:-0.0.0.0}"
DB_USER="${DB_USER:-root}"
DB_PASS="${DB_PASS:-root}"

echo "[1/7] 启动 SurrealDB (port=$DB_PORT, file=$DB_FILE)..."
DB_ENV="DB_PORT=$DB_PORT DB_FILE=$DB_FILE DB_BIND=$DB_BIND DB_USER=$DB_USER DB_PASS=$DB_PASS"

# 使用带健康检查的启动脚本；如果已运行则脚本会直接返回成功
if ! eval "$DB_ENV scripts/db/start_surreal_with_check.sh start"; then
  echo "[ERROR] 启动 SurrealDB 失败" >&2
  exit 1
fi

#############################
# 2. 准备测试数据与静态目录
#############################

echo "[2/7] 准备 BJ / SJZ 测试目录与 assets/archives..."
# 复用已有脚本：创建 test_bj/test_sjz 目录并拷贝基础 DB 文件
scripts/test/test_remote_file_sync.sh

#############################
# 3. 构建 web_server 二进制
#############################

echo "[3/7] 构建 web_server（带 web_server + mqtt 特性）..."
cargo build --bin web_server --features "web_server mqtt"

WEB_BIN_DEBUG="target/debug/web_server"
WEB_BIN_RELEASE="target/release/web_server"

if [[ -x "$WEB_BIN_DEBUG" ]]; then
  WEB_BIN="$WEB_BIN_DEBUG"
elif [[ -x "$WEB_BIN_RELEASE" ]]; then
  WEB_BIN="$WEB_BIN_RELEASE"
else
  echo "[ERROR] 未找到 web_server 可执行文件" >&2
  exit 1
fi

echo "使用 web_server 可执行文件: $WEB_BIN"

#############################
# 4. 启动 BJ / SJZ 两个 web_server 实例
#############################

PORT_BJ="${PORT_BJ:-8081}"
PORT_SJZ="${PORT_SJZ:-8082}"

echo "[4/7] 启动 BJ Web Server (PORT=$PORT_BJ, config=config/remote_file_sync_test/DbOption-bj)..."
PORT="$PORT_BJ" "$WEB_BIN" --config config/remote_file_sync_test/DbOption-bj > /tmp/web_bj.log 2>&1 &
BJ_PID=$!

echo "[4/7] 启动 SJZ Web Server (PORT=$PORT_SJZ, config=config/remote_file_sync_test/DbOption-sjz)..."
PORT="$PORT_SJZ" "$WEB_BIN" --config config/remote_file_sync_test/DbOption-sjz > /tmp/web_sjz.log 2>&1 &
SJZ_PID=$!

cleanup() {
  echo "\n[清理] 停止 BJ / SJZ Web Server..."
  kill "$BJ_PID" "$SJZ_PID" 2>/dev/null || true
}
trap cleanup EXIT

#############################
# 5. 等待 HTTP 服务就绪 & 启动内部 MQTT
#############################

wait_http() {
  local port="$1"
  local path="$2"
  local max_retry=60
  echo "[wait] http://localhost:${port}${path}"
  for ((i=1; i<=max_retry; i++)); do
    if curl -fsS "http://localhost:${port}${path}" > /dev/null 2>&1; then
      echo "  -> OK (第 ${i} 次重试)"
      return 0
    fi
    sleep 1
  done
  echo "  -> 超时：无法访问 http://localhost:${port}${path}" >&2
  return 1
}

echo "[5/7] 等待 BJ / SJZ /remote-sync 页面就绪..."
wait_http "$PORT_BJ" "/remote-sync" || exit 1
wait_http "$PORT_SJZ" "/remote-sync" || exit 1

echo "[5/7] 通过 BJ 实例启动内部 MQTT 服务器 (port=1883)..."
MQTT_PORT="${MQTT_PORT:-1883}"

curl -fsS -X POST "http://localhost:${PORT_BJ}/api/sync/mqtt/start" \
  -H 'Content-Type: application/json' \
  -d "{\"port\":${MQTT_PORT}}" \
  || echo "[WARN] 启动内部 MQTT 服务器失败，可能已在运行或未启用该 API。"

#############################
# 6. 从 DbOption 导入环境并激活 runtime
#############################

import_and_activate() {
  local port="$1"
  echo "[6/7] 在端口 ${port} 从 DbOption 导入环境并激活..."

  local json
  json=$(curl -fsS -X POST "http://localhost:${port}/api/remote-sync/envs/import-from-dboption" \
    -H 'Content-Type: application/json' || echo '{}')

  local env_id
  env_id=$(python3 - "$json" << 'EOF'
import sys, json
raw = sys.argv[1]
try:
    data = json.loads(raw)
    print(data.get("id", ""))
except Exception:
    print("")
EOF
  )

  if [[ -z "${env_id}" ]]; then
    echo "  -> [ERROR] 无法解析导入环境的 id，响应: ${json}" >&2
    return 1
  fi

  echo "  -> 导入环境 id=${env_id}"

  # 激活环境：写入 DbOption.toml 并启动 watcher + MQTT 订阅
  curl -fsS -X POST "http://localhost:${port}/api/remote-sync/envs/${env_id}/activate" \
    -H 'Content-Type: application/json' > /dev/null || {
      echo "  -> [ERROR] 激活环境失败" >&2
      return 1
    }

  echo "  -> 环境已激活 (port=${port}, env_id=${env_id})"
}

import_and_activate "$PORT_BJ"
import_and_activate "$PORT_SJZ"

# 给 runtime 一点时间完成初始化
sleep 3

#############################
# 7. 触发 BJ→SJZ 新增 DB 文件同步事件
#############################

echo "[7/7] 触发 BJ→SJZ 新增 DB 文件同步事件..."
scripts/test/test_remote_file_sync.sh trigger

echo
echo "=== 自动化测试执行完毕 ==="
echo "- SurrealDB 端口: $DB_PORT"
echo "- BJ Web Server 日志: /tmp/web_bj.log"
echo "- SJZ Web Server 日志: /tmp/web_sjz.log"
echo "请在日志中查找关键信息，例如："
echo "  - BJ: '发现新增 db 文件，推送：...'"
echo "  - SJZ: 'Start delta clone db files num: 1 from http://localhost:${PORT_BJ}/assets/archives/... .cba'"
