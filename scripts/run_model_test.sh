#!/bin/bash

# 模型生成测试完整流程脚本
# 自动启动数据库、运行测试、清理资源

set -e

# 颜色输出
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

# 配置
DB_PORT="${DB_PORT:-8000}"
TEST_EXAMPLE="${TEST_EXAMPLE:-test_1112_site_gen_model}"
SURREAL_PID=""

# 设置 SurrealDB 连接环境变量（确保测试程序能找到数据库）
export SURREAL_HOST="${SURREAL_HOST:-127.0.0.1}"
export SURREAL_PORT="${SURREAL_PORT:-8000}"
export SURREAL_USER="${SURREAL_USER:-root}"
export SURREAL_PASS="${SURREAL_PASS:-root}"
export SURREAL_NS="${SURREAL_NS:-test}"
export SURREAL_DB="${SURREAL_DB:-aios}"

# 清理函数
cleanup() {
    if [ -n "${SURREAL_PID}" ]; then
        echo -e "\n${YELLOW}🛑 停止数据库...${NC}"
        kill ${SURREAL_PID} 2>/dev/null || true
        echo -e "${GREEN}✓ 已清理资源${NC}"
    fi
}

# 注册清理回调
trap cleanup EXIT INT TERM

echo -e "${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         模型生成测试流程                                       ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"

# 步骤1: 检查数据库是否已运行
echo -e "\n${YELLOW}📋 步骤 1/3: 检查数据库状态${NC}"
if lsof -Pi :${DB_PORT} -sTCP:LISTEN -t >/dev/null 2>&1 ; then
    echo -e "${GREEN}✓ 数据库已在运行 (端口 ${DB_PORT})${NC}"
else
    echo -e "${YELLOW}⚠️  数据库未运行，正在启动...${NC}"

    # 启动数据库
    if [ -f "./scripts/start_test_db.sh" ]; then
        # 后台启动数据库
        ./scripts/start_test_db.sh &
        SURREAL_PID=$!

        # 等待数据库就绪
        echo -e "${YELLOW}⏳ 等待数据库就绪...${NC}"
        sleep 5

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
            exit 1
        fi
    else
        echo -e "${RED}⚠️  错误: 未找到数据库启动脚本${NC}"
        echo -e "请手动启动 SurrealDB 或确保脚本存在"
        exit 1
    fi
fi

# 步骤2: 编译测试程序 (如果需要)
echo -e "\n${YELLOW}📋 步骤 2/3: 编译测试程序${NC}"
echo -e "${YELLOW}⏳ 正在编译 (release 模式)...${NC}"
cargo build --release --example ${TEST_EXAMPLE}
echo -e "${GREEN}✓ 编译完成${NC}"

# 步骤3: 运行测试
echo -e "\n${YELLOW}📋 步骤 3/3: 运行模型生成测试${NC}"
echo -e "${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         开始测试...                                            ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}\n"

cargo run --release --example ${TEST_EXAMPLE}

echo -e "\n${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         测试完成!                                              ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"
