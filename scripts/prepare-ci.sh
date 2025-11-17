#!/usr/bin/env bash
# CI 环境准备脚本 - 将本地依赖改为 Git 依赖

set -euo pipefail

echo "🔧 准备 gen-model-fork CI 环境..."

# 备份原始 Cargo.toml
if [ ! -f "Cargo.toml.original" ]; then
    echo "📝 备份 Cargo.toml -> Cargo.toml.original"
    cp Cargo.toml Cargo.toml.original
fi

echo "🔨 修改依赖为 Git 源..."

# 替换本地路径依赖为 Git 依赖
sed -i.bak 's|parse_pdms_db = { path = "../aios-parse-pdms" }|parse_pdms_db = { git = "https://gitee.com/happydpc/aios-parse-pdms.git" }|' Cargo.toml

sed -i.bak 's|aios_core = { path = "../rs-core"|aios_core = { git = "https://gitee.com/happydpc/rs-core.git", branch = "2.3"|' Cargo.toml

sed -i.bak 's|pdms_io = { path = "../pdms-io-fork" }|pdms_io = { git = "https://gitee.com/happydpc/pdms-io.git" }|' Cargo.toml

sed -i.bak 's|gen-xkt = { path = "../gen-xkt" }|gen-xkt = { git = "https://gitee.com/happydpc/gen-xkt.git" }|' Cargo.toml

# 注释掉可选的本地依赖
sed -i.bak 's|^story = { path|# story = { path|' Cargo.toml
sed -i.bak 's|^gpui-component = { path|# gpui-component = { path|' Cargo.toml
sed -i.bak 's|^re_ui = { path|# re_ui = { path|' Cargo.toml

echo ""
echo "📋 修改后的关键依赖:"
echo "===================="
grep -A 2 "parse_pdms_db\|aios_core\|pdms_io\|gen-xkt" Cargo.toml | head -20

echo ""
echo "✅ CI 环境准备完成！"
echo ""
echo "现在可以运行:"
echo "  cargo check --features web_server"
echo "  cargo build --release --bin web_server --features web_server"
