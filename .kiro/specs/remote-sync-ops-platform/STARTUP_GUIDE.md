# 异地协同运维平台 - 启动指南

## 🚀 快速启动

### 前端服务

前端服务已成功启动！

```bash
cd frontend/v0-aios-database-management
npm run dev
```

**访问地址**: http://localhost:3000

**主要页面**:
- 环境列表: http://localhost:3000/remote-sync
- 部署向导: http://localhost:3000/remote-sync/deploy
- 监控仪表板: http://localhost:3000/remote-sync/monitor
- 数据流向: http://localhost:3000/remote-sync/flow
- 日志查询: http://localhost:3000/remote-sync/logs

### 后端服务

后端服务正在编译中...

```bash
cargo run --bin web_server --features web_server --release
```

**访问地址**: http://localhost:8080

**主要端点**:
- SSE 事件流: http://localhost:8080/api/sync/events
- 环境列表: http://localhost:8080/api/remote-sync/envs
- 同步状态: http://localhost:8080/api/sync/status

## 📝 当前状态

### ✅ 前端服务
- **状态**: 运行中
- **端口**: 3000
- **URL**: http://localhost:3000
- **启动时间**: ~5 秒

### ⏳ 后端服务
- **状态**: 编译中
- **端口**: 8080
- **URL**: http://localhost:8080
- **预计编译时间**: 2-5 分钟（首次编译）

## 🔧 故障排查

### 后端编译错误

如果遇到 `aios_core` 编译错误，可以尝试：

1. **跳过测试模块**:
```bash
# 编辑 rs-core/src/test/mod.rs
# 注释掉: pub mod test_bran_room_calc;
```

2. **使用已编译的版本**:
```bash
# 如果之前编译过
cargo run --bin web_server --features web_server
```

3. **清理重新编译**:
```bash
cargo clean
cargo build --bin web_server --features web_server --release
```

### 前端依赖问题

如果需要安装 React Query（可选）:

```bash
cd frontend/v0-aios-database-management

# 修复 npm 权限（如果需要）
sudo chown -R $(whoami) ~/.npm

# 安装依赖
npm install @tanstack/react-query @tanstack/react-query-devtools
```

## 🎯 使用指南

### 1. 访问前端

打开浏览器访问: http://localhost:3000/remote-sync

### 2. 创建环境

1. 点击"部署新环境"按钮
2. 按照向导填写信息：
   - 步骤 1: 基本信息（环境名称、MQTT 配置等）
   - 步骤 2: 添加站点
   - 步骤 3: 测试连接
   - 步骤 4: 激活环境

### 3. 监控同步

访问监控仪表板: http://localhost:3000/remote-sync/monitor

可以看到：
- 环境状态卡片
- 实时任务列表
- 性能指标
- 告警信息

### 4. 测试 SSE

```bash
# 测试 SSE 连接
curl -N http://localhost:8080/api/sync/events

# 发送测试事件
curl http://localhost:8080/api/sync/events/test
```

## 📊 功能清单

### ✅ 已实现
- [x] 环境列表展示
- [x] 部署向导（4 步骤）
- [x] 监控仪表板
- [x] 实时事件流（SSE）
- [x] 任务列表
- [x] 性能指标
- [x] 告警系统

### ⏳ 待实现
- [ ] 数据流向可视化
- [ ] 日志查询
- [ ] 性能监控图表
- [ ] 站点元数据浏览
- [ ] 配置管理

## 🔗 相关链接

- **前端**: http://localhost:3000/remote-sync
- **后端 API**: http://localhost:8080/api
- **SSE 测试**: http://localhost:8080/api/sync/events/test
- **健康检查**: http://localhost:8080/api/health

## 💡 提示

1. **首次启动**: 后端首次编译需要 2-5 分钟
2. **热重载**: 前端支持热重载，修改代码后自动刷新
3. **API 调试**: 可以使用浏览器开发者工具查看网络请求
4. **SSE 连接**: 监控页面会自动连接 SSE 事件流

## 🎊 开始使用

前端已经启动成功！现在可以：

1. 打开浏览器访问 http://localhost:3000/remote-sync
2. 查看环境列表
3. 尝试创建新环境（部署向导）
4. 查看监控仪表板

**注意**: 后端服务正在编译中，完成后所有 API 功能将可用。

---

**最后更新**: 2025-11-15
**前端状态**: ✅ 运行中
**后端状态**: ⏳ 编译中
