# LiteFS 原生部署指南（含 UI 操作流程）

## 概述

本指南将帮助你在**不使用 Docker** 的环境下，部署 LiteFS 分布式 SQLite 数据库同步系统，实现多服务器之间的配置自动同步。

## 架构说明

```
北京服务器（主节点）         上海服务器（副本）         广州服务器（副本）
     │                            │                          │
  10.0.1.100                  10.0.2.100               10.0.3.100
     │                            │                          │
   LiteFS                       LiteFS                     LiteFS
     │                            │                          │
/litefs/deployment_sites.sqlite    (自动同步)
     ↑                            ↑                          ↑
   可读写                        只读                        只读
```

### 核心特性
- ✅ **零配置同步**：在任意节点创建环境，自动同步到所有节点
- ✅ **实时同步**：< 1 秒延迟
- ✅ **智能路由**：写操作自动重定向到主节点
- ✅ **UI 提示**：侧边栏显示节点状态

## 前提条件

### 硬件要求
- CPU: 2 核心以上
- 内存: 4GB 以上
- 磁盘: 20GB 可用空间

### 软件要求
- 操作系统: Linux (Ubuntu 20.04+, CentOS 8+) 或 macOS
- 已安装: `curl`, `tar`, `systemd`（Linux）
- 网络: 各服务器之间可以互相访问端口 20202, 20203, 8080

### 服务器信息
准备至少 2 台服务器（推荐 3 台）：

| 服务器 | IP 地址 | 节点类型 | 用途 |
|--------|---------|---------|------|
| 北京 | 10.0.1.100 | 主节点 (Primary) | 可读写 |
| 上海 | 10.0.2.100 | 副本 (Replica) | 只读 |
| 广州 | 10.0.3.100 | 副本 (Replica) | 只读 |

## 一、主节点部署（北京服务器）

### 1.1 安装 LiteFS

```bash
# 克隆项目代码
cd /opt
git clone <your-repo-url> gen-model
cd gen-model

# 执行安装脚本
sudo bash install-litefs.sh
```

安装过程中会提示：
```
请选择节点类型:
  1) 主节点 (Primary) - 可读写，用于第一台服务器
  2) 副本节点 (Replica) - 只读，用于其他服务器
请输入选择 (1/2): 1
```

**输入 `1` 选择主节点**

### 1.2 验证 LiteFS 安装

```bash
# 检查 LiteFS 是否运行
sudo systemctl status litefs

# 应该看到：
# ● litefs.service - LiteFS - Distributed SQLite
#    Active: active (running) since ...
```

```bash
# 检查挂载点
ls -la /litefs/

# 检查 LiteFS API
curl http://localhost:20203/status | python3 -m json.tool
```

输出示例：
```json
{
  "is_primary": true,
  "current": "10.0.1.100:20202",
  "primary": "10.0.1.100:20202",
  "candidate": false
}
```

### 1.3 配置应用数据库路径

编辑 `DbOption.toml`:

```toml
# 修改数据库路径为 LiteFS 挂载点
deployment_sites_sqlite_path = "/litefs/deployment_sites.sqlite"
```

### 1.4 编译并启动 Web UI

```bash
# 编译项目
cargo build --release

# 启动服务
./target/release/web_ui
```

或使用 systemd 管理：

```bash
# 创建 systemd 服务文件
sudo tee /etc/systemd/system/web-ui.service > /dev/null <<EOF
[Unit]
Description=Web UI Service
After=network.target litefs.service
Requires=litefs.service

[Service]
Type=simple
User=$USER
WorkingDirectory=/opt/gen-model
ExecStart=/opt/gen-model/target/release/web_ui
Restart=on-failure
RestartSec=5s
Environment="RUST_LOG=info"
Environment="NODE_NAME=beijing"

[Install]
WantedBy=multi-user.target
EOF

# 重载并启动
sudo systemctl daemon-reload
sudo systemctl enable web-ui
sudo systemctl start web-ui
```

### 1.5 验证主节点部署

访问 Web UI: `http://10.0.1.100:8080`

检查侧边栏底部：
- 应该显示 **"主节点"** 徽章
- 应该显示 **"LiteFS"** 绿色徽章

## 二、副本节点部署（上海、广州服务器）

在每台副本服务器上重复以下步骤：

### 2.1 安装 LiteFS

```bash
# 克隆项目代码
cd /opt
git clone <your-repo-url> gen-model
cd gen-model

# 执行安装脚本
sudo bash install-litefs.sh
```

安装过程中会提示：
```
请选择节点类型:
  1) 主节点 (Primary)
  2) 副本节点 (Replica)
请输入选择 (1/2): 2
```

**输入 `2` 选择副本节点**

然后会提示输入主节点 IP：
```
请输入主节点 IP 地址:
主节点 IP: 10.0.1.100
```

**输入主节点的 IP 地址（北京服务器的 IP）**

### 2.2 验证同步状态

```bash
# 检查 LiteFS 状态
sudo systemctl status litefs

# 检查同步状态
curl http://localhost:20203/status | python3 -m json.tool
```

输出应显示：
```json
{
  "is_primary": false,
  "current": "10.0.2.100:20202",
  "primary": "10.0.1.100:20202",
  "candidate": false
}
```

### 2.3 配置和启动 Web UI

```bash
# 修改 DbOption.toml
nano DbOption.toml
# 设置: deployment_sites_sqlite_path = "/litefs/deployment_sites.sqlite"

# 编译并启动
cargo build --release

# 创建 systemd 服务（上海节点）
sudo tee /etc/systemd/system/web-ui.service > /dev/null <<EOF
[Unit]
Description=Web UI Service
After=network.target litefs.service
Requires=litefs.service

[Service]
Type=simple
User=$USER
WorkingDirectory=/opt/gen-model
ExecStart=/opt/gen-model/target/release/web_ui
Restart=on-failure
RestartSec=5s
Environment="RUST_LOG=info"
Environment="NODE_NAME=shanghai"

[Install]
WantedBy=multi-user.target
EOF

# 启动服务
sudo systemctl daemon-reload
sudo systemctl enable web-ui
sudo systemctl start web-ui
```

### 2.4 验证副本节点

访问上海节点 Web UI: `http://10.0.2.100:8080`

检查侧边栏底部：
- 应该显示 **"副本"** 徽章
- 应该显示 **"LiteFS"** 绿色徽章
- 鼠标悬停可以看到主节点地址

## 三、验证多节点同步

### 3.1 创建测试环境（在主节点）

1. 访问北京节点（主节点）Web UI: `http://10.0.1.100:8080`

2. 点击侧边栏 **"异地协同"**

3. 点击 **"创建协同组"** 按钮

4. 填写表单：
   - **环境名称**: `测试环境`
   - **位置描述**: `北京数据中心`
   - **MQTT 服务器地址**: `mqtt.example.com`
   - **MQTT 端口**: `1883`

5. 点击 **"下一步"**，然后点击 **"创建协同环境"**

### 3.2 验证同步（在副本节点）

1. 等待 1-2 秒

2. 访问上海节点 Web UI: `http://10.0.2.100:8080`

3. 点击侧边栏 **"异地协同"**

4. **应该能看到刚才创建的"测试环境"** ✅

5. 访问广州节点 Web UI: `http://10.0.3.100:8080`，同样应该能看到

### 3.3 测试智能路由

在副本节点（上海）尝试创建环境：

1. 访问 `http://10.0.2.100:8080`

2. 点击 **"创建协同组"**

3. 填写并提交表单

4. **请求会自动转发到主节点**，创建成功后数据会同步回来 ✅

## 四、UI 操作详细流程

### 4.1 查看节点状态

**位置**: 侧边栏底部

<img src="docs/images/node-status-badge.png" width="300" />

**显示信息**:
- **主节点** 徽章：蓝色，显示 "主节点"
- **副本** 徽章：灰色，显示 "副本"
- **LiteFS** 徽章：
  - 绿色 ✅ = LiteFS 已连接
  - 黄色 ⚠️ = 本地模式（未启用 LiteFS）

**鼠标悬停提示框**:
```
节点信息               beijing
─────────────────────────────
节点类型:    主节点 (可读写)
数据库路径:  /litefs/deployment_sites.sqlite
LiteFS 状态: 已连接
```

### 4.2 创建异地协同环境

**步骤 1**: 进入异地协同页面
1. 点击侧边栏 **"异地协同"**
2. 页面显示现有的协同环境列表

**步骤 2**: 创建新环境
1. 点击右上角 **"创建协同组"** 按钮
2. 弹出对话框，显示 "步骤 1 / 2: 基本信息"

**步骤 3**: 填写基本信息
```
┌─────────────────────────────────┐
│  创建协同组                      │
│  步骤 1 / 2: 基本信息            │
├─────────────────────────────────┤
│  环境名称 *                      │
│  ┌───────────────────────────┐  │
│  │ 北京-上海协同环境          │  │
│  └───────────────────────────┘  │
│                                  │
│  位置描述                        │
│  ┌───────────────────────────┐  │
│  │ 北京数据中心               │  │
│  └───────────────────────────┘  │
│                                  │
│  ─── MQTT 服务器配置 ───         │
│                                  │
│  MQTT 服务器地址 *               │
│  ┌───────────────────────────┐  │
│  │ mqtt.example.com          │  │
│  └───────────────────────────┘  │
│                                  │
│  MQTT 端口                       │
│  ┌───────────────────────────┐  │
│  │ 1883                      │  │
│  └───────────────────────────┘  │
│                                  │
│  MQTT 用户名（可选）             │
│  ┌───────────────────────────┐  │
│  │                           │  │
│  └───────────────────────────┘  │
│                                  │
│  MQTT 密码（可选）               │
│  ┌───────────────────────────┐  │
│  │ ●●●●●●●●                  │  │
│  └───────────────────────────┘  │
│                                  │
│  文件服务器地址（可选）          │
│  ┌───────────────────────────┐  │
│  │ http://files.example.com  │  │
│  └───────────────────────────┘  │
│                                  │
│                  ┌──────┐ ┌────┐│
│                  │ 取消 │ │下一步││
│                  └──────┘ └────┘│
└─────────────────────────────────┘
```

4. 填写完成后，点击 **"下一步"**

**步骤 4**: 站点配置
```
┌─────────────────────────────────┐
│  创建协同组                      │
│  步骤 2 / 2: 站点配置            │
├─────────────────────────────────┤
│  环境创建后，可以在详情页面      │
│  添加和管理站点。                │
│                                  │
│        ┌──────┐ ┌──────────────┐│
│        │上一步│ │创建协同环境   ││
│        └──────┘ └──────────────┘│
└─────────────────────────────────┘
```

5. 点击 **"创建协同环境"**

**结果**:
- 如果在主节点：直接创建成功 ✅
- 如果在副本节点：请求自动转发到主节点，创建成功后数据同步回来 ✅

### 4.3 查看协同环境列表

创建成功后，页面显示环境卡片：

```
┌──────────────────────────────────────────────┐
│ 异地协同配置                          🔄 刷新│
│ 管理多站点协同组，实现配置同步和数据协调     │
├──────────────────────────────────────────────┤
│                                              │
│ ┌──────────┐ ┌──────────┐ ┌──────────┐     │
│ │协同组总数│ │ 活跃组  │ │ 同步中  │     │
│ │    3    │ │    2    │ │    1    │     │
│ └──────────┘ └──────────┘ └──────────┘     │
│                                              │
│ ┌─────────────────────────────────────────┐ │
│ │ 北京-上海协同环境         [活跃]        │ │
│ │ ─────────────────────────────────────── │ │
│ │ 类型: 数据同步                          │ │
│ │ 站点数量: 0                             │ │
│ │ 同步模式: 单向                          │ │
│ │ 位置: 北京数据中心                      │ │
│ └─────────────────────────────────────────┘ │
│                                              │
│ ┌─────────────────────────────────────────┐ │
│ │ 测试环境                  [活跃]        │ │
│ │ ...                                     │ │
│ └─────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘
```

### 4.4 查看环境详情

点击任意环境卡片，进入详情页：

```
┌──────────────────────────────────────────────┐
│ ← 北京-上海协同环境        [活跃]  🔄 🚀 ⚙️ 🗑️│
│                                              │
├──────────────────────────────────────────────┤
│ ┌──────────┐ ┌──────────┐ ┌──────────┐     │
│ │站点数量  │ │同步模式  │ │同步记录  │     │
│ │    0    │ │  单向   │ │    0    │     │
│ └──────────┘ └──────────┘ └──────────┘     │
│                                              │
│ ┌─ 协同组信息 ──────────────────────────────┐│
│ │ 协同组类型: 数据同步                      ││
│ │ 位置: 北京数据中心                        ││
│ │ 自动同步: 已启用                          ││
│ │ 冲突解决: 最新更新优先                    ││
│ └───────────────────────────────────────────┘│
│                                              │
│ ┌─ 同步记录 ────────────────────────────────┐│
│ │ 暂无同步记录                              ││
│ └───────────────────────────────────────────┘│
└──────────────────────────────────────────────┘
```

**操作按钮**:
- 🔄 **刷新**: 刷新页面数据
- 🚀 **激活环境**: 启动 MQTT 同步（仅主节点可用）
- ⚙️ **设置**: 修改环境配置
- 🗑️ **删除**: 删除环境

### 4.5 副本节点提示

在副本节点上尝试写操作时，会看到提示：

```
┌─────────────────────────────────┐
│  ⚠️ 当前节点为副本               │
│                                  │
│  只能执行读操作，写操作将自动    │
│  重定向到主节点                  │
│                                  │
│  主节点地址:                     │
│  http://10.0.1.100:8080         │
└─────────────────────────────────┘
```

## 五、监控和管理

### 5.1 查看 LiteFS 日志

```bash
# 查看 LiteFS 日志
sudo journalctl -u litefs -f

# 查看最近 100 条日志
sudo journalctl -u litefs -n 100
```

### 5.2 查看同步状态

```bash
# API 方式
curl http://localhost:20203/status | python3 -m json.tool

# Web UI 方式
# 访问 http://your-server:8080/api/sync-status
```

输出示例：
```json
{
  "status": "ok",
  "is_primary": false,
  "primary": "10.0.1.100:20202",
  "sync_lag_seconds": 0,
  "timestamp": "2025-09-28T10:30:00Z"
}
```

### 5.3 健康检查

```bash
# 检查节点健康状态
curl http://localhost:8080/api/health | python3 -m json.tool
```

输出：
```json
{
  "status": "ok",
  "database": "healthy",
  "is_primary": true,
  "litefs": {
    "is_primary": true,
    "current": "10.0.1.100:20202"
  },
  "timestamp": "2025-09-28T10:30:00Z"
}
```

### 5.4 重启服务

```bash
# 重启 LiteFS
sudo systemctl restart litefs

# 重启 Web UI
sudo systemctl restart web-ui

# 重启全部
sudo systemctl restart litefs web-ui
```

## 六、故障排查

### 6.1 LiteFS 无法启动

**症状**: `systemctl status litefs` 显示 failed

**检查**:
```bash
# 查看详细错误日志
sudo journalctl -u litefs -n 50

# 检查配置文件
sudo cat /etc/litefs.yml

# 检查挂载点权限
ls -la /litefs
ls -la /var/lib/litefs
```

**常见原因**:
1. FUSE 未安装: `sudo apt install fuse3`
2. 端口被占用: `sudo netstat -tuln | grep 20203`
3. 配置文件错误: 检查 YAML 格式

### 6.2 副本节点无法连接主节点

**症状**: 副本节点 status 显示 "Cannot connect to primary"

**检查**:
```bash
# 测试主节点连接
curl http://10.0.1.100:20203/status

# 检查防火墙
sudo firewall-cmd --list-ports
# 或
sudo ufw status
```

**解决方法**:
```bash
# 开放端口（CentOS/RHEL）
sudo firewall-cmd --permanent --add-port=20202/tcp
sudo firewall-cmd --permanent --add-port=20203/tcp
sudo firewall-cmd --reload

# 开放端口（Ubuntu）
sudo ufw allow 20202/tcp
sudo ufw allow 20203/tcp
```

### 6.3 数据库只读错误

**症状**: Web UI 提示 "database is readonly"

**原因**: 副本节点尝试写入

**解决**: 智能路由应该自动处理，如果还是出错：

1. 检查节点状态 API:
```bash
curl http://localhost:8080/api/node-status
```

2. 清除缓存，重新加载页面

3. 手动访问主节点进行写操作

### 6.4 同步延迟过高

**症状**: 副本节点数据更新延迟超过 5 秒

**检查**:
```bash
# 检查网络延迟
ping 10.0.1.100

# 检查同步状态
curl http://localhost:20203/status
```

**解决**:
1. 优化网络连接
2. 检查主节点负载: `top`, `htop`
3. 增加副本节点的重连频率（修改 litefs.yml）

## 七、维护和备份

### 7.1 数据备份

```bash
# 停止服务
sudo systemctl stop web-ui litefs

# 备份数据目录
sudo tar -czf /backup/litefs-$(date +%Y%m%d).tar.gz /var/lib/litefs/

# 启动服务
sudo systemctl start litefs web-ui
```

### 7.2 主节点切换

如果主节点需要维护，可以切换主节点：

1. 停止当前主节点:
```bash
# 在北京节点
sudo systemctl stop web-ui litefs
```

2. 提升副本节点为主节点:
```bash
# 在上海节点
sudo nano /etc/litefs.yml
# 修改: is-primary: true

sudo systemctl restart litefs
```

3. 更新其他副本节点的主节点地址

### 7.3 升级 LiteFS

```bash
# 下载新版本
cd /tmp
curl -L https://github.com/superfly/litefs/releases/download/v0.5.12/litefs-v0.5.12-linux-amd64.tar.gz | tar xz

# 停止服务
sudo systemctl stop litefs

# 替换二进制
sudo mv litefs /usr/local/bin/litefs
sudo chmod +x /usr/local/bin/litefs

# 启动服务
sudo systemctl start litefs
```

## 八、总结

### 已实现的功能

✅ LiteFS 分布式 SQLite 数据库同步
✅ 主节点-副本节点架构
✅ 实时数据同步（< 1 秒）
✅ 前端节点状态显示
✅ 智能路由和重定向
✅ 健康检查和监控 API
✅ systemd 服务管理

### 关键文件清单

```
gen-model/
├── litefs-primary.yml          # 主节点配置
├── litefs-replica.yml          # 副本节点配置
├── litefs.service              # systemd 服务文件
├── install-litefs.sh           # 自动安装脚本
├── src/web_ui/
│   ├── litefs_handlers.rs      # 节点状态 API
│   └── remote_sync_handlers.rs # 远程同步（已修改）
└── frontend/v0-aios-database-management/
    ├── components/
    │   ├── node-status-badge.tsx   # 节点状态组件
    │   └── sidebar.tsx             # 侧边栏（已修改）
    └── lib/api/
        └── collaboration-adapter.ts # 智能路由（已修改）
```

### 下一步

1. 监控生产环境性能
2. 调优同步参数
3. 添加更多副本节点
4. 实施自动备份策略

### 获取帮助

- LiteFS 文档: https://fly.io/docs/litefs/
- 项目问题: 查看项目 README
- 技术支持: 联系运维团队

---

**部署完成！** 🎉

现在你的多服务器平台已经实现了配置自动同步。在任意节点创建的环境配置，都会在 1 秒内同步到所有其他节点。