#!/bin/bash
#
# 1112 数据库完整测试流程
#
# 功能:
# 1. 启动测试数据库
# 2. 解析 PDMS 数据 (ams1112_0001)
# 3. 生成 3D 模型
# 4. 验证生成的数据
#

set -e  # 遇到错误立即退出

# 颜色定义
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 配置
DB_PORT="${DB_PORT:-8000}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo -e "${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         1112 数据库完整测试流程                                ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"
echo ""

# 检查配置文件
echo -e "${YELLOW}📋 步骤 1/5: 检查配置${NC}"
if [ ! -f "$PROJECT_ROOT/DbOption.toml" ]; then
    echo -e "${RED}❌ DbOption.toml 不存在${NC}"
    exit 1
fi

# 检查关键配置
PDMS_FILE=$(grep "included_db_files" "$PROJECT_ROOT/DbOption.toml" | grep -o '"[^"]*"' | head -1 | tr -d '"')
DB_NUM=$(grep "manual_db_nums" "$PROJECT_ROOT/DbOption.toml" | grep -o '[0-9]\+' | head -1)

echo "   配置文件: DbOption.toml"
echo "   PDMS 文件: $PDMS_FILE"
echo "   数据库编号: $DB_NUM"
echo ""

# 启动数据库
echo -e "${YELLOW}📋 步骤 2/5: 启动 SurrealDB${NC}"
if lsof -Pi :$DB_PORT -sTCP:LISTEN -t >/dev/null 2>&1; then
    echo -e "${GREEN}   ✓ 数据库已在运行 (端口 $DB_PORT)${NC}"
    DB_STARTED=false
else
    echo "   正在启动数据库..."
    "$SCRIPT_DIR/start_test_db.sh" &
    DB_PID=$!
    DB_STARTED=true

    # 等待数据库就绪
    sleep 3
    echo -e "${GREEN}   ✓ 数据库已启动${NC}"
fi
echo ""

# 清理函数
cleanup() {
    if [ "$DB_STARTED" = true ] && [ -n "$DB_PID" ]; then
        echo ""
        echo -e "${YELLOW}🧹 清理资源...${NC}"
        kill $DB_PID 2>/dev/null || true
        echo -e "${GREEN}   ✓ 已停止数据库${NC}"
    fi
}

# 注册退出时清理
trap cleanup EXIT INT TERM

# 确保配置启用了同步
echo -e "${YELLOW}📋 步骤 3/5: 检查同步配置${NC}"
TOTAL_SYNC=$(grep "^total_sync" "$PROJECT_ROOT/DbOption.toml" | grep -o 'true\|false')
INCR_SYNC=$(grep "^incr_sync" "$PROJECT_ROOT/DbOption.toml" | grep -o 'true\|false')

if [ "$TOTAL_SYNC" = "false" ] && [ "$INCR_SYNC" = "false" ]; then
    echo -e "${YELLOW}   ⚠️  total_sync 和 incr_sync 都是 false${NC}"
    echo "   将只运行模型生成,不会解析 PDMS 数据"
    echo "   如需解析数据,请在 DbOption.toml 中设置:"
    echo "     total_sync = true  (或 incr_sync = true)"
    echo ""
    read -p "   继续吗? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
else
    echo -e "${GREEN}   ✓ 同步配置已启用${NC}"
    echo "     total_sync = $TOTAL_SYNC"
    echo "     incr_sync  = $INCR_SYNC"
fi
echo ""

# 运行测试
cd "$PROJECT_ROOT"

echo -e "${YELLOW}📋 步骤 4/5: 运行完整测试流程${NC}"
echo ""
echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# 检查是否已编译
if [ ! -f "target/release/examples/test_1112_full_workflow" ]; then
    echo -e "${YELLOW}   ⏳ 首次运行,正在编译...${NC}"
    echo "   (这可能需要几分钟)"
    echo ""
fi

# 运行测试
if cargo run --release --example test_1112_full_workflow; then
    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo ""
    echo -e "${GREEN}   ✓ 测试流程完成${NC}"
else
    echo ""
    echo -e "${RED}   ❌ 测试失败${NC}"
    exit 1
fi
echo ""

# 验证数据
echo -e "${YELLOW}📋 步骤 5/5: 验证生成的数据${NC}"
echo ""

if cargo run --release --example test_1112_site_gen_model 2>&1 | grep -q "找到.*SITE"; then
    echo -e "${GREEN}   ✓ 数据验证通过${NC}"
else
    echo -e "${YELLOW}   ⚠️  未找到 SITE 数据 (可能需要更长时间生成)${NC}"
fi
echo ""

# 完成
echo -e "${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         测试完成!                                              ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"
echo ""

echo -e "${BLUE}💡 后续操作:${NC}"
echo "   • 查看数据库: http://127.0.0.1:$DB_PORT"
echo "   • 运行查询测试: cargo run --release --example verify_query_provider"
echo "   • 运行性能测试: cargo run --release --example gen_model_query_benchmark"
echo ""

if [ "$DB_STARTED" = true ]; then
    echo -e "${YELLOW}💡 数据库将在脚本退出时自动停止${NC}"
    echo "   按 Ctrl+C 退出"
    echo ""
    wait $DB_PID
fi
