#!/bin/bash
# 测试 CTorus CSG 网格生成

set -e

echo "=========================================="
echo "测试 CTorus CSG 网格生成"
echo "=========================================="
echo ""

cd /Volumes/DPC/work/plant-code/gen-model

# 1. 创建输出目录
mkdir -p output/capture-L1
echo "✓ 已创建输出目录"

# 2. 清理旧文件
rm -f output/capture-L1/*.obj output/capture-L1/*.png
echo "✓ 已清理旧文件"

# 3. 检查编译
echo ""
echo "检查代码编译..."
cd /Volumes/DPC/work/plant-code/rs-core
if cargo check --lib > /dev/null 2>&1; then
    echo "✓ rs-core 编译成功"
else
    echo "✗ rs-core 编译失败"
    cargo check --lib
    exit 1
fi

cd /Volumes/DPC/work/plant-code/gen-model
if cargo check --bin aios-database > /dev/null 2>&1; then
    echo "✓ gen-model 编译成功"
else
    echo "✗ gen-model 编译失败"
    cargo check --bin aios-database
    exit 1
fi

# 4. 运行测试
echo ""
echo "运行模型生成测试..."
echo "命令: cargo run --bin aios-database -- --debug-model 21491/18957 --capture output/capture-L1 --capture-include-descendants"
echo ""

cargo run --bin aios-database -- --debug-model 21491/18957 --capture output/capture-L1 --capture-include-descendants 2>&1 | tee /tmp/test_run.log

# 5. 检查结果
echo ""
echo "=========================================="
echo "检查测试结果"
echo "=========================================="

# 检查是否有 CTorus 警告
if grep -qi "CSG mesh generation not supported.*CTorus" /tmp/test_run.log; then
    echo "✗ 仍然存在 CTorus 未支持的警告"
else
    echo "✓ 没有 CTorus 未支持的警告（成功！）"
fi

# 检查输出文件
if [ -f "output/capture-L1/VALV_21491_18957.obj" ]; then
    echo "✓ OBJ 文件已生成"
    OBJ_SIZE=$(wc -l < output/capture-L1/VALV_21491_18957.obj)
    echo "  - OBJ 文件行数: $OBJ_SIZE"
else
    echo "✗ OBJ 文件未生成"
fi

if [ -f "output/capture-L1/VALV_21491_18957.png" ]; then
    echo "✓ PNG 截图已生成"
else
    echo "✗ PNG 截图未生成"
fi

echo ""
echo "=========================================="
echo "测试完成"
echo "=========================================="


