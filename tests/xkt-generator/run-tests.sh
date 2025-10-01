#!/bin/bash

# XKT生成器自动化测试运行脚本
# 该脚本会启动必要的服务并运行所有测试

set -e

echo "=============================================="
echo "🚀 XKT生成器自动化测试"
echo "=============================================="

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# 检查服务状态
check_service() {
    local url=$1
    local service=$2

    if curl -s -o /dev/null -w "%{http_code}" "$url" | grep -q "200\|404"; then
        echo -e "${GREEN}✅ $service 正在运行${NC}"
        return 0
    else
        echo -e "${RED}❌ $service 未运行${NC}"
        return 1
    fi
}

# 清理函数
cleanup() {
    echo -e "\n${YELLOW}🧹 清理测试环境...${NC}"
    # 如果需要停止服务，在这里添加
}

# 设置清理钩子
trap cleanup EXIT

# 1. 检查后端服务
echo -e "\n${YELLOW}1. 检查后端服务...${NC}"
if ! check_service "http://localhost:8080" "后端API (端口8080)"; then
    echo "请先启动后端服务:"
    echo "  cargo run --features=\"web_ui\" --bin web_ui"
    exit 1
fi

# 2. 检查前端服务
echo -e "\n${YELLOW}2. 检查前端服务...${NC}"
if ! check_service "http://localhost:3001" "前端应用 (端口3001)"; then
    echo "请先启动前端服务:"
    echo "  cd frontend/v0-aios-database-management && pnpm dev"
    exit 1
fi

# 3. 安装测试依赖
echo -e "\n${YELLOW}3. 安装测试依赖...${NC}"
if [ ! -d "node_modules" ]; then
    npm install
fi
echo -e "${GREEN}✅ 依赖已安装${NC}"

# 4. 运行API测试
echo -e "\n${YELLOW}4. 运行API测试...${NC}"
echo "----------------------------------------------"
if npm run test:api; then
    API_TEST_RESULT="${GREEN}✅ API测试通过${NC}"
    API_TEST_EXIT=0
else
    API_TEST_RESULT="${RED}❌ API测试失败${NC}"
    API_TEST_EXIT=1
fi

# 5. 运行E2E测试
echo -e "\n${YELLOW}5. 运行端到端测试...${NC}"
echo "----------------------------------------------"
if npm run test:e2e; then
    E2E_TEST_RESULT="${GREEN}✅ E2E测试通过${NC}"
    E2E_TEST_EXIT=0
else
    E2E_TEST_RESULT="${RED}❌ E2E测试失败${NC}"
    E2E_TEST_EXIT=1
fi

# 6. 生成测试报告
echo -e "\n${YELLOW}6. 测试报告${NC}"
echo "=============================================="
echo -e "API测试: $API_TEST_RESULT"
echo -e "E2E测试: $E2E_TEST_RESULT"
echo "=============================================="

# 计算总体结果
TOTAL_EXIT=$((API_TEST_EXIT + E2E_TEST_EXIT))

if [ $TOTAL_EXIT -eq 0 ]; then
    echo -e "\n${GREEN}🎉 所有测试通过！${NC}"
else
    echo -e "\n${RED}⚠️ 部分测试失败${NC}"
fi

exit $TOTAL_EXIT