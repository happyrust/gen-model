#!/usr/bin/env bash
# 恢复原始 Cargo.toml（本地开发用）

set -euo pipefail

echo "🔄 恢复本地开发配置..."

if [ -f "Cargo.toml.original" ]; then
    mv Cargo.toml.original Cargo.toml
    echo "✅ 已恢复 Cargo.toml"
    
    # 清理备份文件
    rm -f Cargo.toml.bak
    echo "🧹 已清理备份文件"
else
    echo "⚠️  未找到 Cargo.toml.original，无需恢复"
fi

echo "✅ 恢复完成！"
