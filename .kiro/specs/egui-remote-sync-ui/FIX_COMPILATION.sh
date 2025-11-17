#!/bin/bash

# 修复 egui UI 编译问题的脚本
# 在 worktree 中运行此脚本

set -e

WORKTREE_PATH="/Volumes/DPC/work/plant-code/aios-database-egui-ui"

echo "🔧 开始修复 egui UI 编译问题..."
echo ""

# 检查是否在 worktree 中
if [ "$(pwd)" != "$WORKTREE_PATH" ]; then
    echo "❌ 错误: 请先切换到 worktree 目录"
    echo "   执行: cd $WORKTREE_PATH"
    exit 1
fi

echo "✅ 当前在 worktree 目录"
echo ""

# 1. 修复 src/gui/mod.rs
echo "📝 修复 src/gui/mod.rs..."
cat > src/gui/mod.rs << 'EOF'
// GUI module for egui-based remote sync UI

pub mod app;
pub mod state;
pub mod api_client;
pub mod pages;
pub mod components;
pub mod canvas;
pub mod theme;

pub use app::EguiRemoteSyncApp;
pub use state::AppState;
pub use api_client::ApiClient;
EOF

echo "✅ src/gui/mod.rs 已修复"
echo ""

# 2. 修复 src/gui/app.rs 中的存储 API
echo "📝 修复 src/gui/app.rs..."

# 备份原文件
cp src/gui/app.rs src/gui/app.rs.backup

# 替换 get_value 调用
sed -i '' 's/eframe::get_value(storage, "current_page")/storage.get_string("current_page").and_then(|s| serde_json::from_str(\&s).ok())/g' src/gui/app.rs
sed -i '' 's/eframe::get_value(storage, "theme")/storage.get_string("theme").and_then(|s| serde_json::from_str(\&s).ok())/g' src/gui/app.rs

# 替换 set_value 调用
cat > /tmp/app_save_fix.txt << 'EOF'
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(page_json) = serde_json::to_string(&self.current_page) {
            storage.set_string("current_page", page_json);
        }
        if let Ok(theme_json) = serde_json::to_string(&self.theme) {
            storage.set_string("theme", theme_json);
        }
    }
EOF

# 使用 perl 进行多行替换
perl -i -0pe 's/fn save\(&mut self, storage: &mut dyn eframe::Storage\) \{[^}]*eframe::set_value[^}]*\}/`cat \/tmp\/app_save_fix.txt`/se' src/gui/app.rs

echo "✅ src/gui/app.rs 已修复"
echo ""

# 3. 修复 src/gui/canvas/topology_canvas.rs
echo "📝 修复 src/gui/canvas/topology_canvas.rs..."

# 备份原文件
cp src/gui/canvas/topology_canvas.rs src/gui/canvas/topology_canvas.rs.backup

# 替换 rect 调用
perl -i -0pe 's/painter\.rect\(\s*rect,\s*5\.0,\s*egui::Color32::from_rgb\(200, 220, 255\),\s*egui::Stroke::new\(2\.0, egui::Color32::BLUE\),\s*\);/painter.rect_filled(rect, 5.0, egui::Color32::from_rgb(200, 220, 255));\n        painter.rect_stroke(rect, 5.0, egui::Stroke::new(2.0, egui::Color32::BLUE));/g' src/gui/canvas/topology_canvas.rs

# 替换 circle 调用
perl -i -0pe 's/painter\.circle\(\s*pos,\s*radius,\s*egui::Color32::from_rgb\(200, 255, 200\),\s*egui::Stroke::new\(2\.0, egui::Color32::GREEN\),\s*\);/painter.circle_filled(pos, radius, egui::Color32::from_rgb(200, 255, 200));\n        painter.circle_stroke(pos, radius, egui::Stroke::new(2.0, egui::Color32::GREEN));/g' src/gui/canvas/topology_canvas.rs

echo "✅ src/gui/canvas/topology_canvas.rs 已修复"
echo ""

# 4. 验证编译
echo "🔍 验证编译..."
if cargo check --bin egui_remote_sync --features gui 2>&1 | grep -q "error"; then
    echo "❌ 编译仍有错误，请查看详细输出"
    cargo check --bin egui_remote_sync --features gui
    exit 1
else
    echo "✅ 编译检查通过！"
fi

echo ""
echo "🎉 所有修复完成！"
echo ""
echo "下一步:"
echo "  1. 查看修改: git diff"
echo "  2. 运行程序: cargo run --bin egui_remote_sync --features gui"
echo "  3. 提交更改: git add . && git commit -m 'fix: 修复 egui UI 编译问题'"
