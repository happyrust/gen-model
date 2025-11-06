# LiteFS 在 macOS 上的部署方案

## 问题说明

**LiteFS 不支持 macOS**，原因如下：

1. **依赖 Linux FUSE**: LiteFS 使用 FUSE（Filesystem in Userspace）技术，依赖 Linux 特定的 fuse3 库
2. **文件系统调用差异**: LiteFS 拦截 Linux 文件系统调用，与 macOS 文件系统不兼容
3. **无官方支持**: Fly.io 官方文档只提供 Linux 和 Docker 部署方案

**当前环境**: macOS (Darwin) ARM64

---

## 解决方案对比

| 方案 | 复杂度 | 成本 | 推荐度 | 说明 |
|------|--------|------|--------|------|
| **方案 1: Multipass** | ⭐ 低 | 免费 | ⭐⭐⭐⭐⭐ | 最简单，Canonical 官方，适合快速测试 |
| **方案 2: Lima** | ⭐⭐ 中 | 免费 | ⭐⭐⭐⭐ | 轻量级，开源，适合开发 |
| **方案 3: OrbStack** | ⭐ 低 | 付费 | ⭐⭐⭐ | 最现代，性能好，但需购买 |
| **方案 4: 远程服务器** | ⭐⭐⭐ 高 | 看情况 | ⭐⭐⭐⭐ | 最真实，适合生产测试 |
| **方案 5: Mock API** | ⭐⭐ 中 | 免费 | ⭐⭐ | 仅测试容错，无法测试真实同步 |

---

## 🥇 推荐方案：Multipass（最简单）

### 优点
- ✅ **官方支持**: Canonical 官方维护
- ✅ **完全免费**: 无任何费用
- ✅ **简单易用**: 一条命令创建 Ubuntu 虚拟机
- ✅ **性能良好**: 使用系统原生虚拟化
- ✅ **文件共享**: 支持 host 和 VM 之间文件共享

### 快速开始

#### 1. 安装 Multipass

```bash
brew install multipass
```

#### 2. 创建主节点虚拟机

```bash
# 创建 Ubuntu VM（4GB 内存，20GB 磁盘）
multipass launch --name litefs-primary --memory 4G --disk 20G

# 进入主节点
multipass shell litefs-primary
```

#### 3. 创建副本节点虚拟机

```bash
# 创建副本节点 1
multipass launch --name litefs-replica1 --memory 2G --disk 10G

# 创建副本节点 2（可选）
multipass launch --name litefs-replica2 --memory 2G --disk 10G
```

#### 4. 在主节点安装 LiteFS

```bash
# 在主节点 shell 中执行
multipass shell litefs-primary

# 下载并运行安装脚本（需要先传输文件）
```

#### 5. 文件传输

```bash
# 从 host 传输到 VM
multipass transfer install-litefs.sh litefs-primary:/tmp/
multipass transfer litefs-primary.yml litefs-primary:/tmp/
multipass transfer target/release/web_server litefs-primary:/tmp/

# 在 VM 中执行安装
multipass shell litefs-primary
sudo bash /tmp/install-litefs.sh
```

#### 6. 获取 VM IP 地址

```bash
# 查看所有 VM 信息
multipass list

# 示例输出：
# Name              State     IPv4           Image
# litefs-primary    Running   192.168.64.2   Ubuntu 22.04 LTS
# litefs-replica1   Running   192.168.64.3   Ubuntu 22.04 LTS
```

#### 7. 配置网络互通

Multipass 虚拟机之间默认互通，可以直接使用 IP 地址通信。

#### 8. 常用命令

```bash
# 启动 VM
multipass start litefs-primary

# 停止 VM
multipass stop litefs-primary

# 删除 VM
multipass delete litefs-primary
multipass purge

# 查看 VM 信息
multipass info litefs-primary

# 执行命令（不进入 shell）
multipass exec litefs-primary -- ls -la /litefs
```

---

## 方案 2: Lima（轻量级）

### 优点
- ✅ **开源免费**: MIT 许可证
- ✅ **轻量级**: 占用资源少
- ✅ **自动文件共享**: 自动挂载 home 目录
- ✅ **支持多种发行版**: Ubuntu, Debian, Alpine 等

### 安装步骤

```bash
# 安装 Lima
brew install lima

# 创建 Ubuntu 实例
limactl start --name=litefs-primary

# 进入实例
lima litefs-primary

# 或直接执行命令
lima litefs-primary ls -la
```

### 配置文件

Lima 使用 YAML 配置文件，可以定制内存、磁盘、网络等。

---

## 方案 3: OrbStack（现代化）

### 优点
- ✅ **性能最好**: 比 Docker Desktop 快
- ✅ **UI 友好**: 图形界面管理
- ✅ **集成度高**: 与 macOS 深度集成
- ❌ **收费软件**: 个人版 $8/月

### 安装

```bash
# 下载安装包
open https://orbstack.dev/download

# 或使用 brew
brew install orbstack
```

---

## 方案 4: 远程 Linux 服务器

### 适用场景
- 有云服务器或 VPS
- 需要测试真实网络环境
- 准备生产部署

### 步骤

1. **准备服务器**: 至少 2 台 Ubuntu 20.04+ 服务器
2. **上传文件**: 使用 `scp` 上传安装脚本和二进制文件
3. **执行安装**: SSH 连接后运行 `install-litefs.sh`
4. **配置防火墙**: 开放端口 20202, 20203, 8080

### 示例命令

```bash
# 上传文件到主节点
scp install-litefs.sh user@server1:/tmp/
scp target/release/web_server user@server1:/opt/aios/

# SSH 连接并安装
ssh user@server1
sudo bash /tmp/install-litefs.sh
```

---

## 方案 5: Mock LiteFS API（仅测试容错）

### 说明

创建一个 Mock 服务，模拟 LiteFS HTTP API 响应，用于测试应用的容错和降级逻辑。

**限制**: 无法测试真实的数据同步功能。

### 实现

```python
# mock-litefs.py
from flask import Flask, jsonify
app = Flask(__name__)

@app.route('/status')
def status():
    return jsonify({
        "current": "primary",
        "isPrimary": True,
        "candidates": ["primary"],
        "hostname": "mock-primary"
    })

if __name__ == '__main__':
    app.run(port=20203)
```

```bash
# 运行 Mock 服务
pip install flask
python mock-litefs.py
```

---

## 📋 推荐执行计划（Multipass 方案）

### 阶段 1: 环境准备（5 分钟）

```bash
# 1. 安装 Multipass
brew install multipass

# 2. 创建主节点
multipass launch --name litefs-primary --memory 4G --disk 20G

# 3. 创建副本节点
multipass launch --name litefs-replica1 --memory 2G --disk 10G

# 4. 查看 IP 地址
multipass list
```

### 阶段 2: 文件传输（2 分钟）

```bash
# 传输安装脚本
multipass transfer install-litefs.sh litefs-primary:/tmp/
multipass transfer install-litefs.sh litefs-replica1:/tmp/

# 传输配置文件
multipass transfer litefs-primary.yml litefs-primary:/tmp/
multipass transfer litefs-replica.yml litefs-replica1:/tmp/

# 传输应用程序
multipass transfer target/release/web_server litefs-primary:/tmp/
multipass transfer target/release/web_server litefs-replica1:/tmp/
```

### 阶段 3: 安装 LiteFS（主节点，3 分钟）

```bash
# 进入主节点
multipass shell litefs-primary

# 在 VM 中执行
sudo bash /tmp/install-litefs.sh
# 选择: Primary node
# 输入: 本机 IP（从 multipass list 获取）

# 启动 LiteFS
sudo systemctl start litefs
sudo systemctl status litefs

# 验证挂载
ls -la /litefs/
```

### 阶段 4: 安装 LiteFS（副本节点，3 分钟）

```bash
# 进入副本节点
multipass shell litefs-replica1

# 在 VM 中执行
sudo bash /tmp/install-litefs.sh
# 选择: Replica node
# 输入: 主节点 IP（从 multipass list 获取）

# 启动 LiteFS
sudo systemctl start litefs
sudo systemctl status litefs

# 验证挂载
ls -la /litefs/
```

### 阶段 5: 部署应用（5 分钟）

```bash
# 在主节点
multipass shell litefs-primary
sudo mv /tmp/web_server /opt/aios/
sudo chmod +x /opt/aios/web_server
export DATABASE_PATH=/litefs/deployment_sites.sqlite
cd /opt/aios && ./web_server

# 在副本节点（新终端）
multipass shell litefs-replica1
sudo mv /tmp/web_server /opt/aios/
sudo chmod +x /opt/aios/web_server
export DATABASE_PATH=/litefs/deployment_sites.sqlite
export PRIMARY_URL=http://<主节点IP>:8080
cd /opt/aios && ./web_server
```

### 阶段 6: 测试同步（3 分钟）

```bash
# 从 macOS host 访问主节点
curl http://<主节点IP>:8080/api/node-status | jq

# 创建协同组（写入主节点）
curl -X POST http://<主节点IP>:8080/api/remote-sync/envs \
  -H "Content-Type: application/json" \
  -d '{"name": "测试环境", "mqtt_host": "mqtt.test.com", "mqtt_port": 1883}'

# 从副本节点读取（验证同步）
curl http://<副本节点IP>:8080/api/remote-sync/envs | jq
```

---

## 🎯 下一步操作

根据您的需求选择：

### 选项 A: 使用 Multipass（推荐，最快）
- 时间: 约 20 分钟
- 成本: 免费
- 难度: 简单

### 选项 B: 使用远程服务器
- 时间: 约 30 分钟
- 成本: 看服务器配置
- 难度: 中等

### 选项 C: 使用 Lima
- 时间: 约 25 分钟
- 成本: 免费
- 难度: 中等

### 选项 D: 暂不测试 LiteFS
- 继续完善其他功能
- 等待生产环境部署时再测试

---

## 常见问题

### Q1: Multipass VM 无法联网？
```bash
# 检查网络
multipass exec litefs-primary -- ping 8.8.8.8

# 重启网络
multipass restart litefs-primary
```

### Q2: 如何访问 VM 内的服务？
```bash
# 方法 1: 使用 VM IP 直接访问
curl http://192.168.64.2:8080

# 方法 2: 端口转发（需要配置）
# 编辑 /etc/ssh/sshd_config 允许端口转发
```

### Q3: 如何共享大文件？
```bash
# 挂载 host 目录到 VM
multipass mount ~/work litefs-primary:/mnt/host
```

### Q4: 如何清理环境？
```bash
# 删除所有 VM
multipass delete --all
multipass purge
```

---

**文档生成时间**: 2025-09-28
**适用环境**: macOS (Darwin ARM64)