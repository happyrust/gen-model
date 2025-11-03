#!/bin/bash

# 数据库测试启动脚本
# 用于启动 SurrealDB 进行模型生成测试

set -e

# 颜色输出
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# 配置
SURREAL_PATH="${SURREAL_PATH:-surreal}"
DB_PATH="${DB_PATH:-./data/test_db}"
DB_PORT="${DB_PORT:-8000}"
DB_USER="${DB_USER:-root}"
DB_PASS="${DB_PASS:-root}"
DB_NS="${DB_NS:-test}"
DB_NAME="${DB_NAME:-aios}"

echo -e "${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         启动 SurrealDB 测试数据库                              ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"

echo -e "\n${YELLOW}📋 配置信息:${NC}"
echo -e "  - 数据库路径: ${DB_PATH}"
echo -e "  - 监听端口:   ${DB_PORT}"
echo -e "  - 命名空间:   ${DB_NS}"
echo -e "  - 数据库名:   ${DB_NAME}"
echo -e "  - 用户名:     ${DB_USER}"

# 检查 SurrealDB 是否已安装
if ! command -v ${SURREAL_PATH} &> /dev/null; then
    echo -e "\n${RED}⚠️  错误: 未找到 SurrealDB 可执行文件${NC}"
    echo -e "请先安装 SurrealDB:"
    echo -e "  curl -sSf https://install.surrealdb.com | sh"
    exit 1
fi

# 检查端口是否被占用
if lsof -Pi :${DB_PORT} -sTCP:LISTEN -t >/dev/null 2>&1 ; then
    echo -e "\n${YELLOW}⚠️  警告: 端口 ${DB_PORT} 已被占用${NC}"
    echo -e "尝试关闭现有进程..."
    PID=$(lsof -ti:${DB_PORT})
    kill -9 ${PID} 2>/dev/null || true
    sleep 2
fi

# 创建数据目录
mkdir -p "$(dirname ${DB_PATH})"

echo -e "\n${GREEN}🚀 启动 SurrealDB...${NC}"

# 启动 SurrealDB (使用 file 存储)
${SURREAL_PATH} start \
    --bind 0.0.0.0:${DB_PORT} \
    --user ${DB_USER} \
    --pass ${DB_PASS} \
    file://${DB_PATH} &

SURREAL_PID=$!

echo -e "${GREEN}✓ SurrealDB 已启动 (PID: ${SURREAL_PID})${NC}"

# 等待数据库就绪
echo -e "\n${YELLOW}⏳ 等待数据库就绪...${NC}"
MAX_RETRIES=30
RETRY_COUNT=0

while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
    if curl -s http://localhost:${DB_PORT}/health > /dev/null 2>&1; then
        echo -e "${GREEN}✓ 数据库已就绪!${NC}"
        break
    fi

    RETRY_COUNT=$((RETRY_COUNT + 1))
    echo -n "."
    sleep 1
done

if [ $RETRY_COUNT -eq $MAX_RETRIES ]; then
    echo -e "\n${RED}⚠️  错误: 数据库启动超时${NC}"
    kill ${SURREAL_PID} 2>/dev/null || true
    exit 1
fi

echo -e "\n${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         数据库已就绪，可以开始测试!                            ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"

echo -e "\n${YELLOW}💡 测试命令:${NC}"
echo -e "  cargo run --release --example test_1112_site_gen_model"
echo -e "\n${YELLOW}💡 停止数据库:${NC}"
echo -e "  kill ${SURREAL_PID}"
echo -e "  或按 Ctrl+C 退出\n"

# 保持进程运行并捕获信号
trap "echo -e '\n${YELLOW}🛑 停止数据库...${NC}'; kill ${SURREAL_PID} 2>/dev/null; echo -e '${GREEN}✓ 已停止${NC}'; exit 0" INT TERM

# 等待数据库进程
wait ${SURREAL_PID}
