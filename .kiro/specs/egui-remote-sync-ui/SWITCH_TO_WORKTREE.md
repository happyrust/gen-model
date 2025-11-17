# 切换到 Worktree 继续开发

## 🎯 当前状态

你现在在主仓库的 `only-csg` 分支。egui UI 的开发应该在独立的 worktree 中进行。

## 📍 Worktree 位置

```
路径: /Volumes/DPC/work/plant-code/aios-database-egui-ui
分支: egui-ui-dev
```

## 🚀 切换命令

在终端中执行：

```bash
cd /Volumes/DPC/work/plant-code/aios-database-egui-ui
```

## ✅ 验证环境

切换后，验证你在正确的位置：

```bash
# 1. 检查当前分支
git branch
# 应该显示: * egui-ui-dev

# 2. 检查工作目录
pwd
# 应该显示: /Volumes/DPC/work/plant-code/aios-database-egui-ui

# 3. 查看 git 状态
git status
```

## 🔧 开始开发

### 1. 首先修复编译问题

worktree 中的代码需要修复以下问题：

#### 修复 `src/gui/mod.rs`

将旧的 gpui 导入替换为 egui 模块：

```rust
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
```

#### 修复 `src/gui/app.rs`

替换 eframe 的存储 API：

```rust
// 在 new() 函数中
if let Some(storage) = cc.storage {
    if let Some(page) = storage.get_string("current_page") {
        if let Ok(page) = serde_json::from_str(&page) {
            app.current_page = page;
        }
    }
    if let Some(theme) = storage.get_string("theme") {
        if let Ok(theme) = serde_json::from_str(&theme) {
            app.theme = theme;
        }
    }
}

// 在 save() 函数中
fn save(&mut self, storage: &mut dyn eframe::Storage) {
    if let Ok(page_json) = serde_json::to_string(&self.current_page) {
        storage.set_string("current_page", page_json);
    }
    if let Ok(theme_json) = serde_json::to_string(&self.theme) {
        storage.set_string("theme", theme_json);
    }
}
```

#### 修复 `src/gui/canvas/topology_canvas.rs`

更新 painter API 调用：

```rust
// 环境节点绘制
painter.rect_filled(rect, 5.0, egui::Color32::from_rgb(200, 220, 255));
painter.rect_stroke(rect, 5.0, egui::Stroke::new(2.0, egui::Color32::BLUE));

// 站点节点绘制
painter.circle_filled(pos, radius, egui::Color32::from_rgb(200, 255, 200));
painter.circle_stroke(pos, radius, egui::Stroke::new(2.0, egui::Color32::GREEN));
```

### 2. 验证编译

```bash
cargo check --bin egui_remote_sync --features gui
```

### 3. 运行程序

```bash
cargo run --bin egui_remote_sync --features gui
```

## 📝 开发流程

1. **修改代码** - 在 worktree 中进行所有修改
2. **测试** - 频繁运行程序测试功能
3. **提交** - 使用清晰的 commit message
4. **推送** - `git push origin egui-ui-dev`

## 🔄 返回主仓库

完成开发后，返回主仓库：

```bash
cd /Volumes/DPC/work/plant-code/gen-model-fork
```

## ⚠️ 重要提示

- **不要在主仓库修改 egui 相关代码**
- **所有 egui UI 开发都在 worktree 中进行**
- **定期提交和推送你的更改**
- **保持主仓库的稳定性**

## 📚 相关文档

- 快速开始: `.kiro/specs/egui-remote-sync-ui/QUICK_START.md`
- 实现进度: `.kiro/specs/egui-remote-sync-ui/IMPLEMENTATION_PROGRESS.md`
- 设计文档: `.kiro/specs/egui-remote-sync-ui/design.md`

---

现在执行：`cd /Volumes/DPC/work/plant-code/aios-database-egui-ui` 开始开发！
