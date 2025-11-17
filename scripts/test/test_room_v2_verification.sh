#!/bin/bash

# 房间计算 V2 改进验证测试脚本
# 
# 用法：
#   ./scripts/test/test_room_v2_verification.sh

set -e

echo "🔬 房间计算 V2 改进验证测试"
echo "========================================"
echo ""

# 颜色定义
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# 检查环境
echo "📋 步骤 1: 检查环境"
echo "----------------------------------------"

# 检查配置文件
if [ ! -f "DbOption.toml" ]; then
    echo -e "${RED}❌ 错误: 未找到 DbOption.toml${NC}"
    exit 1
fi

echo -e "${GREEN}✅ 配置文件存在${NC}"

# 检查 L0 LOD 目录
MESH_DIR=$(grep -A 3 '\[output\]' DbOption.toml | grep 'meshes_path' | cut -d'"' -f2 | sed 's|~|'$HOME'|')
if [ -z "$MESH_DIR" ]; then
    MESH_DIR="assets/meshes"
fi

LOD_L0_DIR="$MESH_DIR/lod_L0"
if [ -d "$LOD_L0_DIR" ]; then
    L0_COUNT=$(find "$LOD_L0_DIR" -name "*.mesh" 2>/dev/null | wc -l | tr -d ' ')
    echo -e "${GREEN}✅ L0 LOD 目录存在: $LOD_L0_DIR${NC}"
    echo "   L0 mesh 文件数: $L0_COUNT"
    
    if [ "$L0_COUNT" -eq 0 ]; then
        echo -e "${YELLOW}⚠️  警告: L0 mesh 文件为空，需要先生成模型${NC}"
    fi
else
    echo -e "${YELLOW}⚠️  警告: L0 LOD 目录不存在: $LOD_L0_DIR${NC}"
    echo "   请先运行模型生成以创建 L0 mesh 文件"
fi

# 检查 SQLite 空间索引
SQLITE_INDEX=$(grep -A 3 '\[output\]' DbOption.toml | grep 'sqlite_index_path' | cut -d'"' -f2 | sed 's|~|'$HOME'|')
if [ -z "$SQLITE_INDEX" ]; then
    SQLITE_INDEX="test-room-build.db"
fi

if [ -f "$SQLITE_INDEX" ]; then
    echo -e "${GREEN}✅ SQLite 空间索引存在: $SQLITE_INDEX${NC}"
    SIZE=$(du -h "$SQLITE_INDEX" | cut -f1)
    echo "   文件大小: $SIZE"
else
    echo -e "${YELLOW}⚠️  警告: SQLite 空间索引不存在: $SQLITE_INDEX${NC}"
    echo "   粗算阶段可能失败"
fi

echo ""

# 运行快速单元测试
echo "📋 步骤 2: 运行关键点提取基础测试"
echo "----------------------------------------"
cargo test --lib --features sqlite-index test_key_points_extraction -- --nocapture || true
echo ""

# 运行完整验证测试
echo "📋 步骤 3: 运行完整验证测试"
echo "----------------------------------------"
echo -e "${YELLOW}⚠️  此测试需要数据库连接，请确保配置正确${NC}"
echo ""

# 设置日志级别
export RUST_LOG=debug

echo "运行命令:"
echo "  cargo test --features sqlite-index test_room_v2_with_lod_verification -- --ignored --nocapture"
echo ""

cargo test --features sqlite-index test_room_v2_with_lod_verification -- --ignored --nocapture

echo ""
echo "========================================"
echo -e "${GREEN}🎉 验证测试完成${NC}"
echo ""
echo "💡 验证要点检查："
echo "   1. 查看日志中是否有 '🔍 粗算完成' 和 '✅ 细算完成'"
echo "   2. 确认耗时统计正常"
echo "   3. 检查是否有 L0 mesh 加载错误"
echo "   4. 验证计算结果数量合理"
echo ""
