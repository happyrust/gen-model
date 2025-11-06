# MQTT 服务器 (基于 rumqttd)

这是一个独立的 MQTT 服务器，用于支持 gen-model 项目的异步同步功能。

本项目通过命令行驱动编译好的 `rumqttd` 二进制文件，而不是直接使用 rumqttd 库。

## 安装和运行

### 1. 准备 rumqttd 二进制文件

`rumqttd` 二进制文件已经包含在本目录中（从 rumqtt 仓库编译的 v0.20.0 release 版本）。

如果需要重新编译，可以：
```bash
# 在父目录克隆 rumqtt 仓库
cd /path/to/gen-model
git clone https://github.com/bytebeamio/rumqtt.git
cd rumqtt
git checkout rumqttd-0.20.0
cargo build --release --bin rumqttd

# 拷贝二进制文件到 rumqttd-server 目录
cp target/release/rumqttd ../rumqttd-server/rumqttd
```

### 2. 编译 mqtt-server 包装程序

```bash
cd rumqttd-server
cargo build --release
```

### 3. 运行

使用默认配置：
```bash
./target/release/mqtt-server
```

使用自定义配置文件：
```bash
./target/release/mqtt-server --config my-config.toml
```

启用调试日志：
```bash
./target/release/mqtt-server --debug
```

指定 rumqttd 二进制文件路径（如果需要）：
```bash
./target/release/mqtt-server --rumqttd-bin /path/to/rumqttd
```

### 4. 作为系统服务运行

创建 systemd 服务文件 `/etc/systemd/system/mqtt-server.service`：

```ini
[Unit]
Description=MQTT Server for gen-model
After=network.target

[Service]
Type=simple
User=your-user
WorkingDirectory=/path/to/rumqttd-server
ExecStart=/path/to/rumqttd-server/target/release/mqtt-server
Restart=always

[Install]
WantedBy=multi-user.target
```

启动服务：
```bash
sudo systemctl daemon-reload
sudo systemctl enable mqtt-server
sudo systemctl start mqtt-server
```

## 配置说明

主要配置项在 `rumqttd.toml` 中：

- **端口配置**：
  - 1883: MQTT v4 标准端口
  - 1884: MQTT v5 端口
  - 8080: WebSocket 端口
  - 3030: 控制台端口

- **连接配置**：
  - `connection_timeout_ms`: 连接超时时间
  - `max_payload_size`: 最大消息大小
  - `max_inflight_count`: 最大飞行消息数

## 与 gen-model 集成

在 gen-model 的 `DbOption.toml` 中配置 MQTT 连接：

```toml
mqtt_host = "localhost"
mqtt_port = 1883
```

## 监控和管理

### 查看日志
```bash
journalctl -u mqtt-server -f
```

### 测试连接
使用 mosquitto 客户端测试：
```bash
# 订阅主题
mosquitto_sub -h localhost -p 1883 -t "test/#"

# 发布消息
mosquitto_pub -h localhost -p 1883 -t "test/topic" -m "Hello MQTT"
```

### 性能调优

1. **增加最大连接数**：
   编辑 `rumqttd.toml`，修改 `max_connections`

2. **调整消息缓冲**：
   修改 `max_segment_size` 和 `max_segment_count`

3. **优化内存使用**：
   调整 `max_payload_size` 和 `max_inflight_count`

## 安全配置

### 启用 TLS（可选）

在配置文件中添加：
```toml
[[servers]]
listen = "0.0.0.0:8883"
tls.cert = "/path/to/cert.pem"
tls.key = "/path/to/key.pem"
```

### 启用认证（可选）

添加认证配置：
```toml
[auth]
enable = true
users = [
    { username = "user1", password = "pass1" },
    { username = "user2", password = "pass2" }
]
```

## 故障排查

1. **端口被占用**：
   ```bash
   sudo lsof -i :1883
   ```

2. **查看服务状态**：
   ```bash
   sudo systemctl status mqtt-server
   ```

3. **检查防火墙**：
   ```bash
   sudo ufw allow 1883/tcp
   ```

## 开发说明

本项目使用 rumqttd v0.20.0（从源码编译的 release 版本），通过命令行驱动。

相关文档：
- [rumqttd GitHub](https://github.com/bytebeamio/rumqtt)
- [rumqttd 文档](https://docs.rs/rumqttd)

### 架构说明

- `mqtt-server`: 一个轻量级的包装程序，负责启动和管理 `rumqttd` 进程
- `rumqttd`: 实际的 MQTT broker 二进制文件（从 rumqtt 仓库编译）

这种设计的好处：
- 不需要在项目中直接依赖 rumqttd 库
- 可以使用官方编译的 release 版本，性能更优
- 更容易升级 rumqttd 版本，只需替换二进制文件

## License

与 gen-model 项目保持一致。