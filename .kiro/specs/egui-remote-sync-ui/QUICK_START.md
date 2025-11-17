# egui Remote Sync UI - 快速开始指南

## 🚀 快速切换到开发环境

```bash
# 1. 切换到 worktree
cd ../aios-database-egui-ui

# 2. 验证环境
git status
git branch  # 应该显示 egui-ui-dev

# 3. 检查编译
cargo check --bin egui_remote_sync --features gui
```

## 📋 当前状态

### ✅ 已完成
- 基础项目结构
- 核心模块框架
- 主应用入口
- 状态管理系统
- API 客户端框架
- 基础 UI 组件（Toast、Dialog、Form）
- 3 个基础页面（环境列表、监控面板、Web Server）

### 🔧 待修复
- egui_extras feature 配置问题
- 编译错误

### 📝 待实现
- 站点配置管理页面
- 日志查询页面
- 拓扑画布完整功能
- 运维操作工具栏
- API 集成和异步处理

## 🛠️ 开发流程

### 1. 修复编译问题

当前问题：egui_extras 的 feature 配置
```bash
cargo check --bin egui_remote_sync --features gui
```

### 2. 实现下一个功能

按照 tasks.md 中的顺序：
1. 站点配置管理
2. 拓扑画布增强
3. 监控面板完善
4. 日志查询页面

### 3. 测试功能

```bash
# 运行程序
cargo run --bin egui_remote_sync --features gui

# 确保后端服务运行在 http://localhost:3000
```

### 4. 提交代码

```bash
git add .
git commit -m "feat: 实现XXX功能"
git push origin egui-ui-dev
```

## 📚 重要文件位置

- **主应用**: `src/gui/app.rs`
- **页面**: `src/gui/pages/`
- **组件**: `src/gui/components/`
- **API**: `src/gui/api_client.rs`
- **状态**: `src/gui/state.rs`
- **入口**: `src/bin/egui_remote_sync.rs`
- **配置**: `Cargo.toml`

## 🎯 开发建议

1. **一次一个功能**: 专注于单个页面或组件
2. **频繁测试**: 每个功能完成后立即测试
3. **参考设计文档**: 查看 `design.md` 了解详细实现
4. **使用现有组件**: 复用 components/ 中的组件

## 🐛 调试技巧

```bash
# 查看详细编译信息
RUST_BACKTRACE=1 cargo build --bin egui_remote_sync --features gui

# 运行时日志
RUST_LOG=debug cargo run --bin egui_remote_sync --features gui

# 检查特定模块
cargo check --bin egui_remote_sync --features gui --message-format=short
```

## 📞 需要帮助？

- 查看设计文档: `.kiro/specs/egui-remote-sync-ui/design.md`
- 查看需求文档: `.kiro/specs/egui-remote-sync-ui/requirements.md`
- 查看任务列表: `.kiro/specs/egui-remote-sync-ui/tasks.md`

## 🔄 返回主 worktree

```bash
cd /Volumes/DPC/work/plant-code/gen-model-fork
```

## 📦 依赖版本

- egui: 0.33.0
- eframe: 0.33.0
- egui_extras: 0.33.0
- reqwest: 0.12
- tokio: 1.47.1
- serde_json: 1.0
- chrono: 0.4

祝开发顺利！🎉
