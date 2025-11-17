# egui Remote Sync UI - 开发指南

## 📋 项目概述

这是一个基于 egui 的异地协同运维界面，用于管理远程同步环境、站点配置、监控任务状态等。

## 🎯 当前状态

- **基础框架**: ✅ 已完成
- **核心模块**: ✅ 已完成
- **UI 组件**: ✅ 已完成
- **基础页面**: ✅ 已完成（3个）
- **编译状态**: ⚠️ 需要修复
- **功能完成度**: 约 40%

## 🚀 快速开始

### 1. 切换到 Worktree

```bash
cd /Volumes/DPC/work/plant-code/aios-database-egui-ui
```

### 2. 验证环境

```bash
git branch  # 应该显示 * egui-ui-dev
pwd         # 应该显示 worktree 路径
```

### 3. 修复编译问题

**选项 A: 使用脚本（推荐）**
```bash
bash ../.kiro/specs/egui-remote-sync-ui/FIX_COMPILATION.sh
```

**选项 B: 手动修复**

参考 `MANUAL_FIXES.md` 逐步修复。

### 4. 验证编译

```bash
cargo check --bin egui_remote_sync --features gui
```

### 5. 运行程序

```bash
cargo run --bin egui_remote_sync --features gui
```

## 📚 文档索引

| 文档 | 用途 |
|------|------|
| `README.md` | 本文件，总体指南 |
| `SWITCH_TO_WORKTREE.md` | 如何切换到 worktree |
| `MANUAL_FIXES.md` | 手动修复编译问题 |
| `FIX_COMPILATION.sh` | 自动修复脚本 |
| `QUICK_START.md` | 快速开始指南 |
| `IMPLEMENTATION_PROGRESS.md` | 详细实现进度 |
| `WORKTREE_SETUP.md` | Worktree 设置说明 |
| `requirements.md` | 需求文档 |
| `design.md` | 设计文档 |
| `tasks.md` | 任务列表 |

## 🗂️ 项目结构

```
src/
├── gui/
│   ├── mod.rs                      # 模块入口
│   ├── app.rs                      # 主应用
│   ├── state.rs                    # 状态管理
│   ├── api_client.rs               # API 客户端
│   ├── theme.rs                    # 主题系统
│   ├── components/                 # UI 组件
│   │   ├── toast.rs                # 提示管理器
│   │   ├── confirm_dialog.rs       # 确认对话框
│   │   └── env_form.rs             # 环境表单
│   ├── pages/                      # 页面
│   │   ├── environment_list.rs     # 环境列表
│   │   ├── monitor_dashboard.rs    # 监控面板
│   │   └── web_server.rs           # 服务器管理
│   └── canvas/                     # 画布
│       └── topology_canvas.rs      # 拓扑画布
└── bin/
    └── egui_remote_sync.rs         # 程序入口
```

## 🔧 开发流程

### 日常开发

1. **切换到 worktree**
   ```bash
   cd /Volumes/DPC/work/plant-code/aios-database-egui-ui
   ```

2. **创建功能分支（可选）**
   ```bash
   git checkout -b feature/your-feature-name
   ```

3. **开发和测试**
   ```bash
   # 编辑代码
   vim src/gui/pages/new_page.rs
   
   # 检查编译
   cargo check --bin egui_remote_sync --features gui
   
   # 运行测试
   cargo run --bin egui_remote_sync --features gui
   ```

4. **提交更改**
   ```bash
   git add .
   git commit -m "feat: 添加新功能"
   git push origin egui-ui-dev
   ```

### 添加新页面

1. 在 `src/gui/pages/` 创建新文件
2. 在 `src/gui/pages/mod.rs` 中导出
3. 在 `src/gui/app.rs` 中添加页面枚举和路由
4. 实现页面的 `render()` 方法

### 添加新组件

1. 在 `src/gui/components/` 创建新文件
2. 在 `src/gui/components/mod.rs` 中导出
3. 在需要的页面中使用

## 📝 待实现功能

### 高优先级

- [ ] 站点配置管理页面
- [ ] 日志查询页面（筛选、分页、导出）
- [ ] 拓扑画布交互（拖拽、连接）
- [ ] 运维操作工具栏
- [ ] API 集成和异步处理

### 中优先级

- [ ] 解析任务管理页面
- [ ] 模型生成配置页面
- [ ] 一键部署页面
- [ ] 配置持久化
- [ ] 文件对话框集成

### 低优先级

- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化
- [ ] 用户文档

## 🐛 常见问题

### Q: 编译错误 "cannot find function `get_value`"

A: 参考 `MANUAL_FIXES.md` 修复 eframe 存储 API。

### Q: 编译错误 "unresolved import `gpui_component`"

A: 确保 `src/gui/mod.rs` 只导出 egui 模块，不导入旧的 gpui 代码。

### Q: 运行时找不到后端 API

A: 确保 Web Server 运行在 `http://localhost:3000`。

### Q: 如何返回主仓库？

A: `cd /Volumes/DPC/work/plant-code/gen-model-fork`

## 🔗 相关链接

- [egui 文档](https://docs.rs/egui/)
- [eframe 文档](https://docs.rs/eframe/)
- [egui 示例](https://github.com/emilk/egui/tree/master/examples)

## 📞 获取帮助

1. 查看相关文档（见上方文档索引）
2. 检查设计文档了解架构
3. 参考现有页面实现

---

**开始开发**: `cd /Volumes/DPC/work/plant-code/aios-database-egui-ui`

**修复编译**: 参考 `MANUAL_FIXES.md`

**实现功能**: 参考 `IMPLEMENTATION_PROGRESS.md`
