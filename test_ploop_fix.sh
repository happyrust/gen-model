#!/bin/bash

# PLOOP重复生成修复测试脚本
# 
# 此脚本用于验证PLOOP调试文件不再重复生成的修复效果
# 
# 使用方法:
# ./test_ploop_fix.sh [refno]
# 
# 示例:
# ./test_ploop_fix.sh 1763081662

set -e

REFNO=${1:-"1763081662"}
OUTPUT_DIR="output/ploop-json"
SVG_DIR="output/ploop-svg"

echo "🧪 测试PLOOP重复生成修复效果"
echo "📋 测试参考号: $REFNO"
echo ""

# 清理之前的输出文件
echo "🧹 清理之前的输出文件..."
rm -rf "$OUTPUT_DIR"
rm -rf "$SVG_DIR"
mkdir -p "$OUTPUT_DIR"
mkdir -p "$SVG_DIR"

echo ""
echo "📊 测试1: 不使用 --debug-model 参数（应该不生成调试文件）"
echo "----------------------------------------"

# 运行不带debug-model的命令
cargo run -- --config DbOption.toml --gen-mesh --refno "$REFNO" 2>&1 | grep -E "(PLOOP|CSG|📄|🔧)" || echo "没有PLOOP相关输出（符合预期）"

# 检查是否生成了调试文件
echo ""
echo "🔍 检查调试文件生成情况:"
if [ -d "$OUTPUT_DIR" ] && [ "$(ls -A $OUTPUT_DIR 2>/dev/null)" ]; then
    echo "❌ 错误: 在没有 --debug-model 的情况下生成了调试文件"
    ls -la "$OUTPUT_DIR"
else
    echo "✅ 正确: 没有生成调试文件"
fi

echo ""
echo "📊 测试2: 使用 --debug-model 参数（应该只生成一次调试文件）"
echo "----------------------------------------"

# 运行带debug-model的命令
echo "🚀 运行命令: cargo run -- --config DbOption.toml --gen-mesh --debug-model $REFNO --refno $REFNO"
cargo run -- --config DbOption.toml --gen-mesh --debug-model "$REFNO" --refno "$REFNO" 2>&1 | tee ploop_test_output.log

echo ""
echo "🔍 分析输出日志:"

# 统计PLOOP处理次数
PLOOP_COUNT=$(grep -c "🔧 开始处理PLOOP顶点" ploop_test_output.log || echo "0")
FRADIUS_COUNT=$(grep -c "🔧 \[CSG\] FRADIUS 处理完成" ploop_test_output.log || echo "0")
JSON_SAVE_COUNT=$(grep -c "📄 \[CSG\] PLOOP JSON 已保存" ploop_test_output.log || echo "0")
SVG_SAVE_COUNT=$(grep -c "📊 \[CSG\] SVG 对比图已保存" ploop_test_output.log || echo "0")
DEBUG_ONCE_COUNT=$(grep -c "📄 \[CSG\] PLOOP 调试文件已生成（仅生成一次）" ploop_test_output.log || echo "0")

echo "   - PLOOP顶点处理次数: $PLOOP_COUNT"
echo "   - FRADIUS处理完成次数: $FRADIUS_COUNT"  
echo "   - JSON文件保存次数: $JSON_SAVE_COUNT"
echo "   - SVG文件保存次数: $SVG_SAVE_COUNT"
echo "   - 调试文件生成提示次数: $DEBUG_ONCE_COUNT"

echo ""
echo "🔍 检查生成的调试文件:"
if [ -d "$OUTPUT_DIR" ] && [ "$(ls -A $OUTPUT_DIR 2>/dev/null)" ]; then
    echo "✅ 生成了调试文件:"
    ls -la "$OUTPUT_DIR"
    
    # 检查文件数量
    JSON_FILES=$(find "$OUTPUT_DIR" -name "*.json" | wc -l)
    TXT_FILES=$(find "$OUTPUT_DIR" -name "*.txt" | wc -l)
    echo "   - JSON文件数量: $JSON_FILES"
    echo "   - TXT文件数量: $TXT_FILES"
else
    echo "❌ 错误: 使用 --debug-model 但没有生成调试文件"
fi

if [ -d "$SVG_DIR" ] && [ "$(ls -A $SVG_DIR 2>/dev/null)" ]; then
    echo "✅ 生成了SVG对比图:"
    ls -la "$SVG_DIR"
    
    SVG_FILES=$(find "$SVG_DIR" -name "*.svg" | wc -l)
    echo "   - SVG文件数量: $SVG_FILES"
else
    echo "❌ 错误: 使用 --debug-model 但没有生成SVG文件"
fi

echo ""
echo "📋 修复效果评估:"
echo "----------------------------------------"

# 评估修复效果
if [ "$PLOOP_COUNT" -gt 0 ] && [ "$DEBUG_ONCE_COUNT" -eq 1 ]; then
    echo "✅ 修复成功: PLOOP处理了 $PLOOP_COUNT 次，但调试文件只生成了 1 次"
elif [ "$PLOOP_COUNT" -gt 0 ] && [ "$DEBUG_ONCE_COUNT" -eq 0 ]; then
    echo "⚠️  部分修复: PLOOP处理了 $PLOOP_COUNT 次，但没有看到调试文件生成提示"
elif [ "$PLOOP_COUNT" -eq 0 ]; then
    echo "❌ 测试失败: 没有检测到PLOOP处理"
else
    echo "❌ 修复失败: 调试文件可能仍在重复生成"
fi

if [ "$JSON_SAVE_COUNT" -le 1 ] && [ "$SVG_SAVE_COUNT" -le 1 ]; then
    echo "✅ 文件重复生成已修复: JSON保存 $JSON_SAVE_COUNT 次，SVG保存 $SVG_SAVE_COUNT 次"
else
    echo "❌ 文件仍在重复生成: JSON保存 $JSON_SAVE_COUNT 次，SVG保存 $SVG_SAVE_COUNT 次"
fi

echo ""
echo "🧹 清理测试文件..."
rm -f ploop_test_output.log

echo "✅ 测试完成!"
