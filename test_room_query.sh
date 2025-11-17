#!/bin/bash

# 房间查询功能测试脚本
# 专门测试 query_room_panels_by_keywords 功能

set -e

echo "🔍 房间查询功能测试"
echo "================================"

# 检查 SurrealDB
echo "📡 检查 SurrealDB 状态..."
if ! pgrep -x "surreal" > /dev/null; then
    echo "⚠️  警告: SurrealDB 似乎未运行"
    read -p "是否继续? (y/N) " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
else
    echo "✅ SurrealDB 正在运行"
fi

echo ""
echo "🚀 运行房间查询测试..."
echo ""

# 运行库测试（只测试我们的功能）
RUST_LOG=${RUST_LOG:-info} cargo test --lib --features sqlite-index \
    test_query_room_info_only \
    -- --ignored --nocapture

echo ""
echo "✅ 测试完成"
