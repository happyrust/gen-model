# gen-model-fork GitHub Actions 部署分析

## 📋 项目概况

**项目名称**: aios-database  
**主要功能**: 工业三维模型生成和数据库管理  
**目标平台**: Windows x64, Linux, macOS  
**核心二进制**: web_server, db_option_ui

## 🔍 关键依赖分析

### 1. 本地路径依赖（需要迁移）

| 依赖 | 本地路径 | Git 仓库 | 状态 |
|------|---------|----------|------|
| **parse_pdms_db** | ../aios-parse-pdms | gitee.com/happydpc/aios-parse-pdms | ⚠️ 需迁移 |
| **aios_core** | ../rs-core | gitee.com/happydpc/rs-core | ✅ 已有 Git |
| **pdms_io** | ../pdms-io-fork | gitee.com/happydpc/pdms-io | ⚠️ 需迁移 |
| **gen-xkt** | ../gen-xkt | ? | ⚠️ 需迁移 |
| **story** | ../gpui-component/crates/story | ? | ⚠️ 可选依赖 |
| **gpui-component** | ../gpui-component/crates/ui | ? | ⚠️ 可选依赖 |
| **re_ui** | ../rerun/crates/viewer/re_ui | github.com/rerun-io/rerun | ⚠️ 可选依赖 |

### 2. 特性标志体系

**默认特性**:
```toml
default = ["ws", "gen_model", "manifold", "project_hd", "sqlite-index", "surreal-save"]
```

**web_server 特性所需**:
- axum (Web 框架)
- tower / tower-http (中间件)
- sysinfo (系统信息)
- re_ui (可选 UI 组件)

**Windows 编译关键点**:
- ✅ manifold-sys 需要 C++ 编译器
- ✅ rusqlite 使用 bundled feature，自带 SQLite
- ⚠️ 部分依赖可能需要 Windows 特定配置

## 🎯 部署策略

### 方案 A: 完全迁移（推荐）

**优点**:
- CI 环境可以直接构建
- 团队成员无需配置本地依赖
- 版本控制清晰

**步骤**:
1. 将所有本地依赖推送到 Gitee/GitHub
2. 修改 Cargo.toml 使用 Git 依赖
3. 配置 GitHub Actions

### 方案 B: Monorepo + Workspace

**优点**:
- 所有代码在同一仓库
- 依赖关系明确
- 适合紧密耦合的项目

**步骤**:
1. 创建 workspace 结构
2. 将所有子项目放入 workspace
3. GitHub Actions 在根目录构建

### 方案 C: 混合策略（实用）

**特点**:
- 必需依赖使用 Git
- 可选依赖（gui, grpc）在 CI 中禁用
- 专注核心功能：web_server 和库

**步骤**:
1. 迁移核心依赖
2. 标记可选依赖
3. CI 仅构建核心特性

## 🚀 推荐实施方案

### 阶段 1: 依赖准备（需要先执行）

```bash
# 1. 确认各依赖是否有 Git 仓库
cd /Volumes/DPC/work/plant-code

# 检查并推送 aios-parse-pdms
cd aios-parse-pdms
git remote -v  # 确认远程仓库
git push origin master

# 检查并推送 pdms-io-fork
cd ../pdms-io-fork
git remote -v
git push origin master

# 检查并推送 gen-xkt
cd ../gen-xkt
git remote -v
git push origin master
```

### 阶段 2: 修改 Cargo.toml

```toml
# 修改前
parse_pdms_db = { path = "../aios-parse-pdms" }
aios_core = { path = "../rs-core" }
pdms_io = { path = "../pdms-io-fork" }
gen-xkt = { path = "../gen-xkt" }

# 修改后
parse_pdms_db = { git = "https://gitee.com/happydpc/aios-parse-pdms.git" }
aios_core = { git = "https://gitee.com/happydpc/rs-core.git", branch = "2.3" }
pdms_io = { git = "https://gitee.com/happydpc/pdms-io.git" }
gen-xkt = { git = "https://gitee.com/happydpc/gen-xkt.git" }

# 可选依赖保持原样或注释掉
# story = { path = "../gpui-component/crates/story", optional = true }
# gpui-component = { path = "../gpui-component/crates/ui", optional = true }
# re_ui = { path = "../rerun/crates/viewer/re_ui", optional = true }
```

### 阶段 3: GitHub Actions 配置

#### 3.1 核心 CI（check + build）

```yaml
name: CI

on:
  push:
    branches: [ "master", "main", "only-csg" ]
  pull_request:
    branches: [ "master", "main" ]

jobs:
  check:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
    
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      
      - name: Check core features
        run: cargo check --features web_server
```

#### 3.2 Windows x64 构建

```yaml
name: Build Windows x64

jobs:
  build-windows:
    runs-on: windows-latest
    
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-pc-windows-msvc
      
      - name: Install dependencies
        run: |
          # manifold-sys 可能需要 vcpkg 或预编译库
          # 根据实际需求安装
      
      - name: Build web_server
        run: |
          cargo build --release --bin web_server --features web_server
      
      - name: Build library
        run: |
          cargo build --release --lib
      
      - name: Upload artifacts
        uses: actions/upload-artifact@v3
        with:
          name: windows-x64-binaries
          path: |
            target/release/web_server.exe
            target/release/aios_database.dll
```

## ⚠️ 潜在问题和解决方案

### 问题 1: manifold-sys 编译

**现象**: Windows 上 manifold-sys 编译失败

**解决方案**:
1. 使用 feature 标志控制：`--no-default-features --features web_server`
2. 或预编译 manifold-sys 并作为 artifact 缓存
3. 或在 Windows CI 上安装 vcpkg 依赖

### 问题 2: 依赖下载慢

**现象**: Gitee 依赖下载超时

**解决方案**:
1. 设置合理超时时间（60 分钟）
2. 使用 cargo-cache action
3. 考虑镜像到 GitHub

### 问题 3: 循环依赖

**现象**: aios_core 依赖 gen-model-fork，反之亦然

**解决方案**:
1. 重构依赖关系，消除循环
2. 使用 feature 标志隔离
3. 或采用 workspace 方式管理

### 问题 4: GUI 依赖

**现象**: re_ui, gpui 等依赖本地路径

**解决方案**:
1. CI 中禁用 gui feature
2. 或将这些依赖也迁移到 Git
3. 专注 web_server 无 GUI 构建

## 📊 预估构建时间

| 平台 | 首次构建 | 缓存命中 |
|------|---------|---------|
| Linux | 20-25分钟 | 8-12分钟 |
| Windows | 30-40分钟 | 12-18分钟 |
| macOS | 25-30分钟 | 10-15分钟 |

**优化建议**:
- 使用 sccache 或 cargo-cache
- 只构建必要的 features
- 并行构建多个 targets

## ✅ 行动清单

### 立即执行（必需）

- [ ] 检查所有本地依赖是否有 Git 仓库
- [ ] 推送缺失的依赖到 Gitee/GitHub
- [ ] 记录每个依赖的 Git URL 和分支

### 短期（1-2天）

- [ ] 修改 Cargo.toml 使用 Git 依赖
- [ ] 本地验证编译通过
- [ ] 创建 GitHub Actions 配置
- [ ] 测试 CI 构建

### 中期（1周）

- [ ] 优化构建时间
- [ ] 添加自动化测试
- [ ] 配置发布流程
- [ ] 文档更新

## 🔗 相关命令

### 检查依赖状态

```bash
cd /Volumes/DPC/work/plant-code

# 检查各依赖仓库状态
for dir in aios-parse-pdms pdms-io-fork gen-xkt rs-core; do
  echo "=== $dir ==="
  cd $dir 2>/dev/null && git remote -v && git status -s || echo "Not a git repo"
  cd ..
done
```

### 本地测试构建

```bash
cd gen-model-fork

# 仅构建核心功能
cargo build --no-default-features --features web_server

# 检查所有特性
cargo check --all-features

# 列出所有二进制
cargo build --bins --features web_server
```

### 准备发布

```bash
# 更新版本号
vim Cargo.toml  # version = "0.2.1"

# 创建标签
git tag v0.2.1
git push origin v0.2.1

# GitHub Actions 自动发布
```

## 🎯 最终目标

完成后，项目将实现：

✅ **自动化构建**: Push 即触发 CI  
✅ **多平台支持**: Linux, Windows x64, macOS  
✅ **发布管理**: Tag 自动生成 Release  
✅ **二进制分发**: 提供预编译的 web_server.exe  
✅ **团队协作**: 无需配置本地依赖

---

*分析完成时间: 2025-01-15*  
*下一步: 执行依赖迁移检查*
