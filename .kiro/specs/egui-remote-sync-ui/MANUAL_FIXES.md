# 手动修复编译问题指南

在 worktree 中执行以下修复步骤。

## 前提条件

```bash
cd /Volumes/DPC/work/plant-code/aios-database-egui-ui
```

## 修复 1: src/gui/mod.rs

**完全替换文件内容为：**

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

## 修复 2: src/gui/app.rs

### 2.1 修复 new() 函数中的存储加载

**查找：**
```rust
        // Load saved state
        if let Some(storage) = cc.storage {
            if let Some(page) = eframe::get_value(storage, "current_page") {
                app.current_page = page;
            }
            if let Some(theme) = eframe::get_value(storage, "theme") {
                app.theme = theme;
            }
        }
```

**替换为：**
```rust
        // Load saved state
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
```

### 2.2 修复 save() 函数

**查找：**
```rust
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "current_page", &self.current_page);
        eframe::set_value(storage, "theme", &self.theme);
    }
```

**替换为：**
```rust
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(page_json) = serde_json::to_string(&self.current_page) {
            storage.set_string("current_page", page_json);
        }
        if let Ok(theme_json) = serde_json::to_string(&self.theme) {
            storage.set_string("theme", theme_json);
        }
    }
```

## 修复 3: src/gui/canvas/topology_canvas.rs

### 3.1 修复环境节点绘制

**查找：**
```rust
        // Draw rectangle
        painter.rect(
            rect,
            5.0,
            egui::Color32::from_rgb(200, 220, 255),
            egui::Stroke::new(2.0, egui::Color32::BLUE),
        );
```

**替换为：**
```rust
        // Draw rectangle
        painter.rect_filled(rect, 5.0, egui::Color32::from_rgb(200, 220, 255));
        painter.rect_stroke(rect, 5.0, egui::Stroke::new(2.0, egui::Color32::BLUE));
```

### 3.2 修复站点节点绘制

**查找：**
```rust
        // Draw circle
        painter.circle(
            pos,
            radius,
            egui::Color32::from_rgb(200, 255, 200),
            egui::Stroke::new(2.0, egui::Color32::GREEN),
        );
```

**替换为：**
```rust
        // Draw circle
        painter.circle_filled(pos, radius, egui::Color32::from_rgb(200, 255, 200));
        painter.circle_stroke(pos, radius, egui::Stroke::new(2.0, egui::Color32::GREEN));
```

## 验证修复

```bash
# 检查编译
cargo check --bin egui_remote_sync --features gui

# 如果成功，尝试运行
cargo run --bin egui_remote_sync --features gui
```

## 提交更改

```bash
git add src/gui/mod.rs src/gui/app.rs src/gui/canvas/topology_canvas.rs
git commit -m "fix: 修复 egui UI 编译问题

- 更新 mod.rs 移除 gpui 依赖
- 修复 eframe 存储 API 调用
- 更新 painter API 调用以匹配 egui 0.33"
```

## 如果遇到其他错误

### 错误: 找不到 gpui_component

这些是旧文件，不需要编译。确保 `src/gui/mod.rs` 只导出 egui 相关模块。

### 错误: 找不到 story

同上，这是旧的 gpui 依赖，已经移除。

### 错误: IntoElement 宏

这是 gpui 的宏，不需要了。确保没有导入旧的 GUI 文件。

## 下一步

修复完成后，参考 `IMPLEMENTATION_PROGRESS.md` 继续实现剩余功能。
