# gen-model-fork GitHub Actions 部署指南

## 🎯 概述

为 `aios-database` 项目配置了完整的 GitHub Actions CI/CD 流程，支持：

- ✅ **多平台构建**: Linux, Windows x64
- ✅ **自动测试**: 单元测试和集成测试
- ✅ **Windows 专属发布**: web_server.exe 自动打包
- ✅ **发布管理**: Tag 自动创建 Release

## 📦 已创建的工作流

### 1. CI 工作流 (`.github/workflows/ci.yml`)

**触发条件**:
- Push 到 master/main/only-csg 分支
- Pull Request 到 master/main
- 手动触发

**包含任务**:
```
Check & Lint → Build (Linux/Windows) → Build with Manifold → Test Suite → Docs
```

**特性**:
- 代码格式检查 (`cargo fmt`)
- Clippy 静态分析
- 多平台并行构建
- 智能特性组合（避免 manifold 在 Windows 上编译）

### 2. Windows 专属构建 (`.github/workflows/windows-build.yml`)

**触发条件**:
- Push 到 master/only-csg
- 推送版本标签 (`v*.*.*`)
- 手动触发

**输出产物**:
- `web_server.exe` - Windows x64 二进制
- `aios-database-windows-x64.zip` - 完整发布包
- 自动创建 GitHub Release（当推送 tag 时）

## 🔧 使用方法

### 本地测试（推送前验证）

```bash
cd /Volumes/DPC/work/plant-code/gen-model-fork

# 1. 使用脚本准备 CI 环境
./scripts/prepare-ci.sh

# 2. 验证编译
cargo check --features web_server

# 3. 测试构建
cargo build --no-default-features --features ws,sqlite-index,surreal-save,web_server

# 4. 恢复本地配置
./scripts/restore-local.sh
```

### 推送触发 CI

```bash
# 日常开发推送
git add .
git commit -m "feat: 添加新功能"
git push origin only-csg

# GitHub Actions 自动运行 CI 和 Windows 构建
```

### 发布新版本

```bash
# 1. 更新版本号
vim Cargo.toml  # version = "0.2.1"

# 2. 更新更新日志
echo "## [0.2.1] - 2025-01-15\n### Added\n- 新功能..." >> CHANGELOG.md

# 3. 提交并创建标签
git add Cargo.toml CHANGELOG.md
git commit -m "chore: bump version to 0.2.1"
git tag v0.2.1
git push origin v0.2.1

# 4. GitHub Actions 自动:
#    - 构建 Windows x64 二进制
#    - 创建 GitHub Release
#    - 上传 web_server.exe 和 ZIP 包
```

## 🚨 重要注意事项

### 1. 依赖迁移状态

**当前状态** (需要在推送前处理):

| 依赖 | 状态 | 操作 |
|------|------|------|
| aios_core | ✅ 已有 Git | 已使用 `branch = "2.3"` |
| parse_pdms_db | ⚠️ 本地路径 | 需修改为 Git URL |
| pdms_io | ⚠️ 本地路径 | 需修改为 Git URL |
| gen-xkt | ⚠️ 本地路径 | 需修改为 Git URL |

**修改方案**:

方式 A: 手动修改 `Cargo.toml`:
```toml
# 修改前
parse_pdms_db = { path = "../aios-parse-pdms" }

# 修改后
parse_pdms_db = { git = "https://gitee.com/happydpc/aios-parse-pdms.git" }
```

方式 B: 使用自动化脚本 (推荐):
```bash
./scripts/prepare-ci.sh  # 自动替换所有依赖
```

### 2. 特性标志策略

CI 构建使用的特性组合：

```bash
# 基础 web_server（不含 manifold）
--no-default-features --features ws,sqlite-index,surreal-save,web_server

# 完整版本（仅 Linux）
--features ws,gen_model,manifold,sqlite-index,surreal-save,web_server
```

**原因**: manifold-sys 在 Windows 上编译复杂，CI 中禁用以加速构建。

### 3. manifold 依赖问题

**问题**: `manifold-sys` 需要 C++ 编译器和特定库

**解决方案**:
- Linux CI: 安装 cmake, g++
- Windows CI: 暂时禁用 manifold feature
- 本地开发: 保持 default features

### 4. 构建时间预估

| 平台 | 首次构建 | 缓存命中 |
|------|---------|---------|
| Linux (无 manifold) | 15-20分钟 | 6-10分钟 |
| Linux (含 manifold) | 25-35分钟 | 12-18分钟 |
| Windows x64 | 20-30分钟 | 8-15分钟 |

## 📝 配置文件说明

### CI 工作流关键配置

```yaml
# 缓存策略
uses: Swatinem/rust-cache@v2
with:
  shared-key: "gen-model-${{ matrix.os }}"

# Linux 依赖安装
- name: Install Linux dependencies
  run: |
    sudo apt-get update
    sudo apt-get install -y pkg-config libssl-dev build-essential

# Windows MSBuild 设置
- name: Setup Windows build environment
  uses: microsoft/setup-msbuild@v2
```

### Windows 构建关键配置

```yaml
# 目标平台
targets: x86_64-pc-windows-msvc

# 构建命令
cargo build --release --bin web_server --target x86_64-pc-windows-msvc

# 打包
Compress-Archive -Path release-package/* -DestinationPath aios-database-windows-x64.zip
```

## 🐛 故障排查

### 问题 1: 依赖下载失败

**症状**: `error: failed to get 'aios_core'`

**解决**:
```bash
# 检查 Cargo.toml 是否正确修改
grep "aios_core\|parse_pdms_db\|pdms_io\|gen-xkt" Cargo.toml

# 确认所有依赖都有 Git URL
```

### 问题 2: Windows 构建超时

**症状**: CI 运行超过 60 分钟

**解决**:
- 检查是否启用了 manifold（应该禁用）
- 确认缓存是否正常工作
- 考虑减少并行度

### 问题 3: manifold 编译失败

**症状**: `error: failed to compile manifold-sys`

**解决**:
```yaml
# 在 ci.yml 中添加 continue-on-error
- name: Build with manifold feature
  run: cargo build --features manifold
  continue-on-error: true  # 允许失败
```

### 问题 4: 找不到 web_server.exe

**症状**: Release 中缺少二进制文件

**解决**:
```bash
# 检查构建是否成功
cargo build --release --bin web_server

# 确认路径正确
ls -la target/release/web_server.exe  # Windows
ls -la target/release/web_server      # Linux
```

## 📊 CI 状态徽章

在 README.md 中添加构建状态徽章：

```markdown
[![CI](https://github.com/你的用户名/gen-model-fork/workflows/CI/badge.svg)](https://github.com/你的用户名/gen-model-fork/actions)
[![Windows Build](https://github.com/你的用户名/gen-model-fork/workflows/Windows%20x64%20Build/badge.svg)](https://github.com/你的用户名/gen-model-fork/actions)
```

## 🎓 最佳实践

### 1. 分支策略

```
master/main  ← PR 合并后触发完整 CI
only-csg     ← 开发分支，频繁推送触发 CI
feature/*    ← 功能分支，PR 到 only-csg
```

### 2. 版本发布流程

```bash
# 1. 在 only-csg 分支开发测试
git checkout only-csg
# ... 开发 ...
git push origin only-csg  # 触发 CI

# 2. 合并到 master 并发布
git checkout master
git merge only-csg
git tag v0.2.1
git push origin master v0.2.1  # 触发发布流程
```

### 3. 本地开发建议

```bash
# 始终在本地配置中使用 path 依赖
aios_core = { path = "../rs-core" }

# 推送前使用脚本转换
./scripts/prepare-ci.sh
git commit -am "chore: prepare for CI"

# 推送后恢复
./scripts/restore-local.sh
```

## 🔗 相关资源

- [完整部署分析](./gen-model-fork部署分析.md)
- [GitHub Actions 文档](https://docs.github.com/en/actions)
- [Rust CI 最佳实践](https://doc.rust-lang.org/cargo/guide/continuous-integration.html)

## ✅ 验证清单

推送前确认：

- [ ] 所有本地依赖已改为 Git 依赖
- [ ] 本地 `cargo check --features web_server` 通过
- [ ] scripts/prepare-ci.sh 执行无错误
- [ ] .github/workflows/ 文件已添加
- [ ] README 更新了使用说明

推送后验证：

- [ ] GitHub Actions 自动运行
- [ ] CI 工作流通过
- [ ] Windows 构建生成 artifacts
- [ ] (可选) 发布 tag 触发 Release 创建

---

**配置完成！现在可以推送代码让 GitHub Actions 自动构建。** 🎉
