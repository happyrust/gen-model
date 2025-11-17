# 快速开始指南

本指南帮助您快速启动和使用远程同步运维平台。

---

## 前置要求

### 后端
- Rust 1.70+
- Cargo
- SQLite 3

### 前端
- Node.js 20+
- npm 或 yarn

---

## 安装步骤

### 1. 克隆项目
```bash
git clone <repository-url>
cd gen-model-fork
```

### 2. 启动后端服务

```bash
# 编译并运行 Web 服务器
cargo run --bin web_server --features web_server

# 或者使用 release 模式 (更快)
cargo build --release --features web_server
./target/release/web_server
```

后端服务将在 `http://localhost:8080` 启动。

### 3. 启动前端服务

```bash
cd frontend/v0-aios-database-management

# 安装依赖
npm install

# 启动开发服务器
npm run dev
```

前端服务将在 `http://localhost:3000` 启动。

---

## 首次使用

### 1. 配置环境

访问 `http://localhost:3000/remote-sync/topology`

1. 点击 "添加环境" 按钮
2. 填写环境信息：
   - 名称: 例如 "生产环境"
   - MQTT 主机: 例如 "mqtt://localhost:1883"
   - 文件服务器: 例如 "http://localhost:8080"
   - 数据库编号: 例如 "7999,8000"
3. 点击 "保存"

### 2. 添加站点

1. 点击 "添加站点" 按钮
2. 填写站点信息：
   - 名称: 例如 "远程站点 A"
   - 位置: 例如 "北京"
   - 数据库编号: 例如 "7999"
3. 从环境节点拖动连接线到站点节点
4. 点击 "保存"

### 3. 查看监控

访问 `http://localhost:3000/remote-sync/monitor`

- 查看实时同步状态
- 监控性能指标
- 查看活跃任务

### 4. 查询日志

访问 `http://localhost:3000/remote-sync/logs`

- 使用筛选器查找特定日志
- 点击日志查看详情
- 导出日志为 CSV 或 JSON

### 5. 性能分析

访问 `http://localhost:3000/remote-sync/metrics`

- 查看实时性能指标
- 分析历史趋势
- 导出性能报告

### 6. 告警管理

访问 `http://localhost:3000/remote-sync/alerts`

- 配置告警规则
- 查看告警历史
- 设置通知渠道

---

## 常用操作

### 启动同步服务

在任何页面使用运维工具栏：

1. 点击 "启动" 按钮
2. 确认操作
3. 等待服务启动

### 添加同步任务

1. 点击 "添加任务" 按钮
2. 填写文件路径
3. 选择同步方向 (推送/拉取)
4. 点击 "添加任务"

### 查看实时事件

监控页面会自动显示实时事件：
- 同步开始/完成/失败
- MQTT 连接状态
- 队列大小变化
- 告警通知

---

## API 测试

### 使用 curl 测试

```bash
# 获取拓扑配置
curl http://localhost:8080/api/remote-sync/topology

# 获取性能指标
curl http://localhost:8080/api/sync/metrics

# 获取历史指标
curl http://localhost:8080/api/sync/metrics/history?time_range=day

# 查询日志
curl http://localhost:8080/api/remote-sync/logs?limit=10

# 启动同步服务
curl -X POST http://localhost:8080/api/sync/start

# 停止同步服务
curl -X POST http://localhost:8080/api/sync/stop
```

### 使用 SSE 测试

```bash
# 监听实时事件
curl -N http://localhost:8080/api/sync/events
```

---

## 故障排查

### 后端无法启动

**问题**: `数据库连接初始化失败`

**解决方案**:
1. 检查 `DbOption.toml` 文件是否存在
2. 确保 SurrealDB 服务运行在配置的端口
3. 检查配置文件中的连接信息

### 前端无法连接后端

**问题**: `Failed to fetch`

**解决方案**:
1. 确认后端服务已启动
2. 检查 `NEXT_PUBLIC_API_BASE_URL` 环境变量
3. 检查 CORS 配置

### SSE 连接失败

**问题**: `EventSource failed`

**解决方案**:
1. 检查后端 `/api/sync/events` 端点是否可访问
2. 确认浏览器支持 SSE
3. 检查网络代理设置

### 日志查询缓慢

**问题**: 查询超过 2 秒

**解决方案**:
1. 减少查询范围 (使用筛选器)
2. 降低 limit 参数
3. 检查数据库索引

---

## 配置文件

### 后端配置 (DbOption.toml)

```toml
[database]
host = "localhost"
port = 8020
namespace = "test"
database = "test"

[sync]
mqtt_host = "localhost"
mqtt_port = 1883
file_server_host = "http://localhost:8080"
```

### 前端环境变量 (.env.local)

```env
NEXT_PUBLIC_API_BASE_URL=http://localhost:8080
```

---

## 开发模式

### 后端热重载

```bash
# 使用 cargo-watch
cargo install cargo-watch
cargo watch -x 'run --bin web_server --features web_server'
```

### 前端热重载

```bash
# Next.js 自动支持热重载
npm run dev
```

---

## 生产部署

### 后端

```bash
# 编译 release 版本
cargo build --release --features web_server

# 运行
./target/release/web_server

# 或使用 systemd
sudo systemctl start remote-sync-ops
```

### 前端

```bash
# 构建生产版本
npm run build

# 启动生产服务器
npm start

# 或使用 PM2
pm2 start npm --name "remote-sync-ui" -- start
```

---

## 性能调优

### 数据库优化

```sql
-- 创建索引
CREATE INDEX idx_logs_created_at ON remote_sync_logs(created_at);
CREATE INDEX idx_logs_status ON remote_sync_logs(status);
CREATE INDEX idx_logs_env_id ON remote_sync_logs(env_id);
```

### 前端优化

```javascript
// 调整 React Query 缓存时间
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5000, // 5 秒
      cacheTime: 300000, // 5 分钟
    },
  },
})
```

---

## 常见问题

### Q: 如何修改端口？

**A**: 
- 后端: 修改 `src/web_server/mod.rs` 中的 `start_web_server(8080)`
- 前端: 修改 `package.json` 中的 `dev` 脚本

### Q: 如何添加新的告警规则？

**A**: 
1. 访问 `/remote-sync/alerts`
2. 点击 "配置" 按钮
3. 修改阈值参数
4. 点击 "保存配置"

### Q: 如何导出大量日志？

**A**: 
使用 API 直接导出：
```bash
curl "http://localhost:8080/api/remote-sync/logs?limit=10000" > logs.json
```

### Q: 如何备份数据？

**A**: 
备份 SQLite 数据库文件：
```bash
cp deployment_sites.sqlite deployment_sites.sqlite.backup
```

---

## 下一步

- 阅读 [实现总结](./IMPLEMENTATION_SUMMARY.md) 了解架构细节
- 查看 [实现状态](./IMPLEMENTATION_STATUS.md) 了解开发进度
- 参考 [设计文档](./design.md) 了解系统设计

---

## 获取帮助

如遇到问题，请：
1. 查看本文档的故障排查部分
2. 检查后端日志输出
3. 检查浏览器控制台错误
4. 提交 Issue 到项目仓库

---

*最后更新: 2024-11-17*
