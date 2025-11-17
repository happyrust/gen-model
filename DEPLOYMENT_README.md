# AIOS Web Server & Database Deployment Guide

本文档说明如何构建和部署 AIOS Web Server 和 aios-database 应用。

## 🚀 快速开始

### 使用部署脚本（推荐）

```bash
# 构建所有组件
./deploy.sh

# 或只构建 web-server
./deploy.sh web-server

# 或只构建 aios-database
./deploy.sh aios-database

# 构建Docker镜像
./deploy.sh docker
```

### 使用 Docker Compose

```bash
# 启动所有服务
docker-compose up -d

# 查看日志
docker-compose logs -f

# 停止服务
docker-compose down
```

## 📦 构建产物

GitHub Actions 会自动构建以下产物：

### Web Server 应用
- **Linux二进制**: `web-server-linux-x86_64.tar.gz`
- **Windows二进制**: `web-server-windows-x86_64.zip`
- **Docker镜像**: `aios/web-server:latest`

### Aios Database 库
- **Linux库**: `libaios_database.so`
- **Windows库**: `aios_database.dll`
- **macOS库**: `libaios_database.dylib`

## 🔧 手动构建

### 前置要求
- Rust 1.83.0+
- CMake (Linux)
- OpenSSL 开发包

### Linux/macOS

```bash
# 安装依赖 (Ubuntu/Debian)
sudo apt-get update
sudo apt-get install -y pkg-config libssl-dev build-essential cmake g++ git

# 构建 Web Server
cargo build --release --bin web_server --no-default-features --features ws,sqlite-index,surreal-save,web_server

# 构建 Database 库
cargo build --release --lib --no-default-features --features ws,sqlite-index,surreal-save,web_server
```

### Windows

```bash
# 使用 MSVC
cargo build --release --bin web_server --no-default-features --features ws,sqlite-index,surreal-save,web_server

# 或者使用 GitHub Actions 自动构建
```

## 🏗️ 架构组件

### Web Server (web-server)
- **端口**: 8080 (默认)
- **功能**: REST API、文件服务、WebSocket支持
- **特性**: 
  - SQLite 数据库支持
  - SurrealDB 集成
  - WebSocket 实时通信
  - 静态文件服务

### Aios Database (aios-database)
- **类型**: Rust 库
- **功能**: 数据访问层、模型生成、空间索引
- **用途**: 可作为库集成到其他 Rust 项目

## 🐳 Docker 部署

### 单个容器
```bash
# 构建镜像
docker build -t aios/web-server .

# 运行容器
docker run -d \
  --name aios-web-server \
  -p 8080:8080 \
  -v $(pwd)/data:/app/data \
  aios/web-server
```

### Docker Compose (完整服务栈)
```bash
# 启动所有服务 (web-server + surrealdb + redis)
docker-compose up -d

# 检查服务状态
docker-compose ps
```

## 🔌 配置说明

### 环境变量
```bash
# 日志级别
RUST_LOG=info|debug|trace

# 数据库路径
DB_PATH=/app/data/ams-demo.db

# Web服务器端口
PORT=8080
```

### 配置文件
- `DbOption.toml`: 主要配置文件
- 数据库配置、API设置、功能开关等

## 🧪 测试部署

```bash
# 检查服务状态
curl http://localhost:8080/health

# 查看版本信息
./web_server --version

# 测试API端点
curl http://localhost:8080/api/v1/status
```

## 📊 监控

### 健康检查
Docker容器包含健康检查：
- **间隔**: 30秒
- **超时**: 10秒
- **重试**: 3次

### 日志
```bash
# Docker日志
docker-compose logs -f web-server

# 本地运行日志
./web_server 2>&1 | tee app.log
```

## 🔄 自动部署流程

1. **代码推送**: 推送到 `only-csg` 或 `main` 分支
2. **GitHub Actions**: 自动触发构建
3. **并行构建**: Web Server 和 Database 同时构建
4. **测试**: 自动运行基本功能测试
5. **打包**: 生成部署包
6. **发布**: (仅在标签推送时) 创建GitHub Release

## 🚨 故障排除

### 常见问题

1. **编译错误**: 检查Rust版本和依赖安装
2. **端口冲突**: 修改 `docker-compose.yml` 中的端口映射
3. **数据库权限**: 确保数据目录权限正确

### 调试命令
```bash
# 查看详细日志
RUST_LOG=debug ./web_server

# 检查依赖
cargo check --features web_server

# 清理重建
cargo clean && cargo build
```

## 📋 依赖更新

所有项目已更新使用 GitHub 仓库依赖：
- `aios_core`: 从 `https://github.com/happyrust/rs-core.git` (2.3分支)

确保使用相同的依赖版本以避免兼容性问题。

## 🔗 相关文档

- [API文档](./web-test/)
- [数据库架构](./docs/architecture/)
- [GitHub Actions配置](./.github/workflows/)

---

📝 如有问题，请检查 GitHub Actions 工作流日志或创建 Issue。
