#!/bin/bash

# 测试 XKT 模型生成和验证
set -e

REFNO="21491_18957"
API_BASE="http://localhost:8080"

echo "=========================================="
echo "XKT 模型生成和验证测试"
echo "参考号: $REFNO"
echo "=========================================="
echo ""

# 1. 生成 XKT 模型
echo "📝 步骤 1: 生成 XKT 模型..."
RESPONSE=$(curl -s -X POST "$API_BASE/api/xkt/generate" \
  -H "Content-Type: application/json" \
  -d "{
    \"refnos\": \"$REFNO\",
    \"compress\": true,
    \"include_descendants\": true,
    \"skip_mesh\": false
  }")

echo "生成响应: $RESPONSE"
echo ""

# 检查是否成功
SUCCESS=$(echo "$RESPONSE" | grep -o '"success":true' || echo "")
FILE_PATH=$(echo "$RESPONSE" | grep -o '"file_path":"[^"]*"' | cut -d'"' -f4 || echo "")

if [ -z "$SUCCESS" ]; then
    echo "❌ 模型生成失败"
    exit 1
fi

echo "✅ 模型生成成功"
echo "文件路径: $FILE_PATH"
echo ""

# 2. 验证 XKT 模型
echo "📝 步骤 2: 验证 XKT 模型..."
VALIDATION=$(curl -s "$API_BASE/api/xkt/validate?file=${FILE_PATH}")

echo "验证响应: $VALIDATION"
echo ""

# 检查验证结果
VALID=$(echo "$VALIDATION" | grep -o '"valid":true' || echo "")

if [ -z "$VALID" ]; then
    echo "❌ 模型验证失败"
    echo "错误详情:"
    echo "$VALIDATION" | grep -o '"errors":\[[^]]*\]' || echo "无错误信息"
    exit 1
fi

echo "✅ 模型验证通过"
echo ""

# 3. 显示统计信息
echo "📊 模型统计信息:"
echo "$VALIDATION" | grep -o '"statistics":{[^}]*}' || echo "无统计信息"
echo ""

# 4. 列出文件
echo "📁 生成的文件:"
ls -lh "$(echo "$FILE_PATH" | sed 's|output/xkt_test/|./output/xkt_test/|')" 2>/dev/null || echo "文件未找到"
echo ""

echo "=========================================="
echo "✅ 测试完成！"
echo "=========================================="


