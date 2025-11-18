#!/usr/bin/env bash
set -euo pipefail

# 自动化 BJ / SJZ 远程文件服务器同步测试
# 参考文档: docs/REMOTE_FILE_SERVER_SYNC_TEST.md

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SRC_DIR="/Volumes/DPC/work/e3d_models/AvevaMarineSample/ams000"
BJ_DIR="/Volumes/DPC/work/e3d_models/test_bj/AvevaMarineSample/ams000"
SJZ_DIR="/Volumes/DPC/work/e3d_models/test_sjz/AvevaMarineSample/ams000"

echo "[1/3] 准备测试数据目录..."
mkdir -p "$BJ_DIR" "$SJZ_DIR"

if [[ ! -f "$SRC_DIR/ams251270_0001" ]]; then
  echo "ERROR: 源文件不存在: $SRC_DIR/ams251270_0001" >&2
  exit 1
fi
if [[ ! -f "$SRC_DIR/ams251181_0001" ]]; then
  echo "ERROR: 源文件不存在: $SRC_DIR/ams251181_0001" >&2
  exit 1
fi

# 基础 DB 文件仅在不存在时复制，避免覆盖手工修改
cp -n "$SRC_DIR/ams251270_0001" "$BJ_DIR/" || true
cp -n "$SRC_DIR/ams251181_0001" "$SJZ_DIR/" || true

echo "BJ 测试目录: $BJ_DIR"
echo "SJZ 测试目录: $SJZ_DIR"

echo "[2/3] 准备 web_server 静态目录 (assets/archives, assets/temp)..."
cd "$ROOT_DIR"
mkdir -p assets/archives assets/temp

echo "ROOT_DIR: $ROOT_DIR"

echo
echo "[提示] 请确保已按 docs/REMOTE_FILE_SERVER_SYNC_TEST.md 配置好:"
echo "  - DbOption-bj.toml (project_path 指向 test_bj, location=\"bj\", location_dbs 包含 BJ dbnum)"
echo "  - DbOption-sjz.toml (project_path 指向 test_sjz, location=\"sjz\", location_dbs 包含 SJZ dbnum)"
echo

echo "[建议命令] 在两个终端中分别启动 BJ / SJZ web_server:" 
echo "  (终端1，BJ)  PORT=8081 cargo run --bin web_server --features \"web_server mqtt\" -- --config DbOption-bj"
echo "  (终端2，SJZ)  PORT=8082 cargo run --bin web_server --features \"web_server mqtt\" -- --config DbOption-sjz"
echo

echo "[3/3] 如果传入参数 'trigger'，则在 BJ 侧制造一个新增 DB 文件事件以触发 BJ→SJZ 同步。"

if [[ "${1-}" == "trigger" ]]; then
  echo "[触发模式] 在 BJ 目录创建一个新的 DB 文件副本以触发增量同步..."
  TS="$(date +%s)"
  NEW_FILE="ams251270_0001_${TS}"
  cp "$SRC_DIR/ams251270_0001" "$BJ_DIR/$NEW_FILE"
  echo "已在 BJ 测试目录创建新文件: $BJ_DIR/$NEW_FILE"
  echo "请在:"
  echo "  - BJ 日志中确认: '发现新增 db 文件，推送：$NEW_FILE'"
  echo "  - SJZ 日志中确认: 'Start delta clone db files num: 1 from http://localhost:8081/assets/archives/${NEW_FILE}.cba'"
  echo "  并检查 SJZ 测试目录和 assets/archives 是否出现对应文件。"
fi
