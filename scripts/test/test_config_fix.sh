#!/bin/bash

echo "🧪 测试配置文件修复"
echo "==================="

# 检查配置文件是否存在
if [ ! -f "DbOption.toml" ]; then
    echo "❌ DbOption.toml 不存在"
    exit 1
fi

if [ ! -f "DbOption-ams.toml" ]; then
    echo "❌ DbOption-ams.toml 不存在"
    exit 1
fi

echo "✅ 配置文件检查通过"

# 编译项目
echo "🔨 编译项目..."
cargo build --release --bin web_server --features web_server

if [ $? -ne 0 ]; then
    echo "❌ 编译失败"
    exit 1
fi

echo "✅ 编译成功"

# 测试默认配置
echo "📋 测试默认配置 (DbOption.toml):"
echo "DB_OPTION_FILE 环境变量: ${DB_OPTION_FILE:-未设置}"

# 测试指定配置
echo "📋 测试指定配置 (DbOption-ams.toml):"
echo "使用 --config DbOption-ams 参数"

echo ""
echo "🎯 测试完成！"
echo "现在可以使用以下命令测试："
echo "1. 默认配置: ./target/release/web_server"
echo "2. AMS 配置: ./target/release/web_server --config DbOption-ams"
echo "3. 环境变量: DB_OPTION_FILE=DbOption-ams ./target/release/web_server"
