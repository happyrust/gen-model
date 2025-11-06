# AIOS Database Management Platform - Development Setup

## 🎯 问题分析

您遇到的 404 错误是因为 Next.js 前端试图调用 `/api/deployment-sites` 端点，但这些端点是在 Rust 后端服务中实现的，而不是在 Next.js API 路由中。

## 🔧 解决方案

### 1. 环境配置

已创建 `.env.local` 文件，配置了后端 API 地址：
```
NEXT_PUBLIC_API_BASE_URL=http://localhost:8080
```

### 2. 启动服务

#### 方法一：使用启动脚本（推荐）
```bash
# 在 frontend/v0-aios-database-management 目录下运行
./start-dev.sh
```

这个脚本会：
- 启动 Rust 后端服务（端口 8080）
- 启动 Next.js 前端服务（端口 3000）
- 自动配置环境变量

#### 方法二：手动启动

**步骤 1：启动 Rust 后端**
```bash
# 在项目根目录下
cd /Volumes/DPC/work/plant-code/gen-model
cargo run --bin web_server --features "web_server,ws,gen_model,manifold,project_hd"
```

**步骤 2：启动 Next.js 前端**
```bash
# 在另一个终端中
cd /Volumes/DPC/work/plant-code/gen-model/frontend/v0-aios-database-management
pnpm run dev
```

### 3. 访问应用

- **前端界面**: http://localhost:3000
- **后端 API**: http://localhost:8080

## 🏗️ 架构说明

### 后端服务（Rust + Axum）
- **端口**: 8080
- **功能**: 提供所有 API 端点，包括 `/api/deployment-sites`
- **数据库**: SurrealDB + SQLite
- **启动命令**: `cargo run --bin web_server --features "web_server,ws,gen_model,manifold,project_hd"`

### 前端服务（Next.js）
- **端口**: 3000
- **功能**: 提供用户界面
- **API 代理**: 通过 `NEXT_PUBLIC_API_BASE_URL` 代理请求到后端

## 🔍 故障排除

### 常见问题

**Q: 仍然出现 404 错误**
A: 确保后端服务正在运行，检查 `http://localhost:8080/api/deployment-sites` 是否可访问

**Q: 前端无法连接到后端**
A: 检查 `.env.local` 文件中的 `NEXT_PUBLIC_API_BASE_URL` 配置

**Q: 后端启动失败**
A: 检查 Rust 依赖是否正确安装，确保所有 features 可用

### 验证步骤

1. **检查后端服务**:
   ```bash
   curl http://localhost:8080/api/node-status
   ```

2. **检查前端配置**:
   ```bash
   # 在浏览器开发者工具中查看网络请求
   # 应该看到请求被代理到 http://localhost:8080
   ```

## 📝 开发说明

### API 端点

后端提供以下主要 API 端点：
- `GET /api/deployment-sites` - 获取部署站点列表
- `POST /api/deployment-sites` - 创建部署站点
- `PATCH /api/deployment-sites/{id}` - 更新部署站点
- `DELETE /api/deployment-sites/{id}` - 删除部署站点
- `GET /api/node-status` - 获取节点状态

### 前端配置

前端通过 `lib/api.ts` 中的 `buildApiUrl` 函数自动处理 API 请求代理：

```typescript
const API_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL ?? ""
export function buildApiUrl(path: string): string {
  if (!API_BASE_URL) {
    return path  // 直接使用相对路径
  }
  return `${API_BASE_URL}${path}`  // 代理到后端
}
```

## 🚀 快速开始

1. 确保在正确的目录中
2. 运行 `./start-dev.sh`
3. 访问 http://localhost:3000
4. 开始开发！

---

**注意**: 如果遇到任何问题，请检查两个服务是否都在运行，并验证环境变量配置。
