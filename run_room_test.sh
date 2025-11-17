#!/bin/bash

# 房间集成测试运行脚本
# 用法: ./run_room_test.sh [test_name]
# 示例: ./run_room_test.sh test_query_room_info_only

set -e

echo "🏗️  房间集成测试运行脚本"
echo "================================"

# 检查 SurrealDB 是否运行
echo "📡 检查 SurrealDB 状态..."
if ! pgrep -x "surreal" > /dev/null; then
    echo "⚠️  警告: SurrealDB 似乎未运行"
    echo "提示: 请先启动 SurrealDB"
    echo ""
    read -p "是否继续运行测试? (y/N) " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "❌ 测试已取消"
        exit 1
    fi
else
    echo "✅ SurrealDB 正在运行"
fi

# 设置日志级别（可选）
export RUST_LOG=${RUST_LOG:-info}

# 选择测试案例
TEST_NAME=${1:-test_room_integration_complete}

echo ""
echo "🎯 测试案例: $TEST_NAME"
echo "📝 日志级别: $RUST_LOG"
echo ""

# 可用的测试案例
case $TEST_NAME in
    "complete"|"test_room_integration_complete")
        echo "🚀 运行完整集成测试..."
        TEST_NAME="test_room_integration_complete"
        ;;
    "query"|"test_query_room_info_only")
        echo "🔍 运行房间查询测试..."
        TEST_NAME="test_query_room_info_only"
        ;;
    "rebuild"|"test_rebuild_specific_rooms")
        echo "🔄 运行特定房间重建测试..."
        TEST_NAME="test_rebuild_specific_rooms"
        ;;
    "limited"|"test_limited_room_integration")
        echo "📊 运行限制房间数量测试..."
        TEST_NAME="test_limited_room_integration"
        ;;
    "all")
        echo "🎪 运行所有测试..."
        TEST_NAME=""
        ;;
    *)
        echo "⚠️  使用自定义测试名称: $TEST_NAME"
        ;;
esac

echo ""
echo "⚙️  编译测试..."
echo ""

# 构建测试命令
if [ -z "$TEST_NAME" ]; then
    # 运行所有测试
    CMD="cargo test --test test_room_integration --features sqlite-index -- --ignored --nocapture"
else
    # 运行指定测试
    CMD="cargo test --test test_room_integration --features sqlite-index $TEST_NAME -- --ignored --nocapture"
fi

echo "执行命令: $CMD"
echo ""

# 运行测试
eval $CMD

echo ""
echo "✅ 测试完成"
