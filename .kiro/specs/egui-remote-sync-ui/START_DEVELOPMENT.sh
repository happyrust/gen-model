#!/bin/bash

# egui Remote Sync UI - 一键启动开发脚本
# 此脚本会自动切换到 worktree、修复编译问题并启动开发

set -e

WORKTREE_PATH="/Volumes/DPC/work/plant-code/aios-database-egui-ui"
MAIN_REPO="/Volumes/DPC/work/plant-code/gen-model-fork"

echo "🚀 egui Remote Sync UI - 开发环境启动"
echo "========================================"
echo ""

# 检查当前位置
CURRENT_DIR=$(pwd)
echo "📍 当前目录: $CURRENT_DIR"
echo ""

# 如果在主仓库，提示切换
if [ "$CURRENT_DIR" = "$MAIN_REPO" ]; then
    echo "⚠️  你当前在主仓库中"
    echo "   需要切换到 worktree 进行开发"
    echo ""
    echo "执行以下命令切换到 worktree:"
    echo ""
    echo "   cd $WORKTREE_PATH"
    echo "   bash .kiro/specs/egui-remote-sync-ui/START_DEVELOPMENT.sh"
    echo ""
    exit 0
fi

# 如果在 worktree 中
if [ "$CURRENT_DIR" = "$WORKTREE_PATH" ]; then
    echo "✅ 已在 worktree 中"
    echo ""
    
    # 检查分支
    BRANCH=$(git branch --show-current)
    echo "📌 当前分支: $BRANCH"
    
    if [ "$BRANCH" != "egui-ui-dev" ]; then
        echo "⚠️  警告: 当前不在 egui-ui-dev 分支"
        echo "   切换到正确的分支..."
        git checkout egui-ui-dev
    fi
    echo ""
    
    # 步骤 1: 修复编译问题
    echo "🔧 步骤 1: 修复编译问题"
    echo "------------------------"
    
    if [ -f ".kiro/specs/egui-remote-sync-ui/FIX_COMPILATION.sh" ]; then
        echo "运行修复脚本..."
        bash .kiro/specs/egui-remote-sync-ui/FIX_COMPILATION.sh
    else
        echo "❌ 找不到修复脚本"
        echo "   请手动参考 MANUAL_FIXES.md 进行修复"
        exit 1
    fi
    
    echo ""
    echo "✅ 编译问题已修复"
    echo ""
    
    # 步骤 2: 验证编译
    echo "🔍 步骤 2: 验证编译"
    echo "-------------------"
    echo "运行: cargo check --bin egui_remote_sync --features gui"
    echo ""
    
    if cargo check --bin egui_remote_sync --features gui 2>&1 | grep -q "Finished"; then
        echo ""
        echo "✅ 编译检查通过！"
    else
        echo ""
        echo "❌ 编译仍有错误"
        echo "   请查看上面的错误信息"
        echo "   或参考 MANUAL_FIXES.md 手动修复"
        exit 1
    fi
    
    echo ""
    
    # 步骤 3: 询问是否运行程序
    echo "🎯 步骤 3: 运行程序"
    echo "------------------"
    read -p "是否现在运行程序? (y/n) " -n 1 -r
    echo ""
    
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo ""
        echo "启动 egui Remote Sync UI..."
        echo ""
        cargo run --bin egui_remote_sync --features gui
    else
        echo ""
        echo "跳过运行程序"
        echo ""
        echo "你可以稍后手动运行:"
        echo "   cargo run --bin egui_remote_sync --features gui"
    fi
    
    echo ""
    echo "🎉 开发环境已就绪！"
    echo ""
    echo "📚 有用的命令:"
    echo "   cargo check --bin egui_remote_sync --features gui  # 检查编译"
    echo "   cargo run --bin egui_remote_sync --features gui    # 运行程序"
    echo "   cargo build --bin egui_remote_sync --features gui --release  # 发布构建"
    echo ""
    echo "📖 查看文档:"
    echo "   cat .kiro/specs/egui-remote-sync-ui/INDEX.md"
    echo ""
    
else
    echo "❌ 错误: 未知的目录位置"
    echo "   当前: $CURRENT_DIR"
    echo "   期望: $WORKTREE_PATH 或 $MAIN_REPO"
    echo ""
    echo "请切换到正确的目录:"
    echo "   cd $WORKTREE_PATH"
    exit 1
fi
