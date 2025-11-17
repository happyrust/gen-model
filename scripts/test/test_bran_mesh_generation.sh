#!/bin/bash
# 测试 BRAN/HANG Full Noun 模式 mesh 生成

set -e

echo "🚀 测试 BRAN/HANG Full Noun 模式 mesh 生成"
echo ""

# 1. 编译测试用例
echo "📦 步骤 1: 编译测试用例..."
cd "$(dirname "$0")/../.."
cargo build --example test_full_noun_bran_mesh --release
echo "✅ 编译完成"
echo ""

# 2. 清理旧的测试输出
echo "🧹 步骤 2: 清理旧的测试输出..."
rm -rf test_output/full_noun_bran_meshes
mkdir -p test_output/full_noun_bran_meshes
echo "✅ 清理完成"
echo ""

# 3. 运行测试
echo "🔨 步骤 3: 运行测试..."
export RUST_LOG=info
cargo run --example test_full_noun_bran_mesh --release
TEST_EXIT_CODE=$?
echo ""

# 4. 检查测试结果
if [ $TEST_EXIT_CODE -eq 0 ]; then
    echo "✅ 测试执行成功"
    
    # 统计生成的 mesh 文件
    MESH_DIR="test_output/full_noun_bran_meshes"
    if [ -d "$MESH_DIR" ]; then
        MESH_COUNT=$(find "$MESH_DIR" -name "*.mesh" | wc -l | tr -d ' ')
        echo "📊 统计结果:"
        echo "   - Mesh 文件总数: $MESH_COUNT"
        
        if [ "$MESH_COUNT" -gt 0 ]; then
            echo "   - Mesh 文件列表 (前 10 个):"
            find "$MESH_DIR" -name "*.mesh" | head -10 | while read -r file; do
                SIZE=$(ls -lh "$file" | awk '{print $5}')
                echo "      * $(basename "$file") ($SIZE)"
            done
            echo ""
            echo "✅ Mesh 文件生成验证通过"
        else
            echo "⚠️  警告: 未找到 mesh 文件"
            echo "可能原因："
            echo "   1. 数据库中没有 BRAN/HANG 数据"
            echo "   2. inst_relate 表中没有 BRAN/HANG 的子元素关系"
            echo "   3. 子元素收集逻辑有问题"
        fi
    else
        echo "❌ Mesh 目录不存在: $MESH_DIR"
        exit 1
    fi
else
    echo "❌ 测试执行失败 (退出码: $TEST_EXIT_CODE)"
    exit $TEST_EXIT_CODE
fi

echo ""
echo "🎉 测试完成"
