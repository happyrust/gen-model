# 远程文件服务器异地同步测试说明

本测试文档用于验证：

- 使用 `web_server` 内置的目录服务（`/assets/archives`）作为 `.cba` 文件 HTTP 下发端；
- 通过 MQTT + 增量同步链路，实现 BJ / SJZ 两个“区域”之间的 DB 文件同步；
- 同时覆盖「已有 DB 文件 sesno 增量」与「新增 DB 文件」两种触发方式。

## 1. 测试数据准备

源数据目录示例：

- `/Volumes/DPC/work/e3d_models/AvevaMarineSample/ams000/ams251270_0001`
- `/Volumes/DPC/work/e3d_models/AvevaMarineSample/ams000/ams251181_0001`

为避免影响原始数据，建议准备两个独立测试目录：

```text
/Volumes/DPC/work/e3d_models/test_bj/
  AvevaMarineSample/
    ams000/
      ams251270_0001

/Volumes/DPC/work/e3d_models/test_sjz/
  AvevaMarineSample/
    ams000/
      ams251181_0001
```

拷贝命令示例：

```bash
mkdir -p /Volumes/DPC/work/e3d_models/test_bj/AvevaMarineSample/ams000
mkdir -p /Volumes/DPC/work/e3d_models/test_sjz/AvevaMarineSample/ams000

cp /Volumes/DPC/work/e3d_models/AvevaMarineSample/ams000/ams251270_0001 \
   /Volumes/DPC/work/e3d_models/test_bj/AvevaMarineSample/ams000/

cp /Volumes/DPC/work/e3d_models/AvevaMarineSample/ams000/ams251181_0001 \
   /Volumes/DPC/work/e3d_models/test_sjz/AvevaMarineSample/ams000/
```

## 2. DbOption 配置示例

### 2.1 BJ 侧（DbOption-bj.toml）

基于现有 `DbOption.toml` 复制一份：

```bash
cp DbOption.toml DbOption-bj.toml
```

然后至少调整以下字段（其它保持原样即可）：

```toml
# 项目路径指向 BJ 测试目录
project_path = "/Volumes/DPC/work/e3d_models/test_bj"
included_projects = ["AvevaMarineSample"]

# 区域标识
location = "bj"
location_dbs = [251270]          # 示例：251270 属于 bj

# MQTT broker
mqtt_host = "127.0.0.1"
mqtt_port = 1883

# 本地文件服务器地址，对应 web_server 中的 ServeDir("assets/archives")
file_server_host = "http://localhost:8081/assets/archives"
```

并在 BJ 代码目录下准备静态目录：

```bash
mkdir -p assets/archives
mkdir -p assets/temp
```

### 2.2 SJZ 侧（DbOption-sjz.toml）

在 SJZ 使用的代码工作区中同样复制配置：

```bash
cp DbOption.toml DbOption-sjz.toml
```

示例修改：

```toml
project_path = "/Volumes/DPC/work/e3d_models/test_sjz"
included_projects = ["AvevaMarineSample"]

location = "sjz"
location_dbs = [251181]          # 示例：251181 属于 sjz

mqtt_host = "127.0.0.1"
mqtt_port = 1883

file_server_host = "http://localhost:8082/assets/archives"
```

并准备静态目录：

```bash
mkdir -p assets/archives
mkdir -p assets/temp
```

> 注意：`file_server_host` 必须是“目录 URL”，不要包含文件名。
> 实际 HTTP 地址为 `file_server_host + "/" + file_name + ".cba"`。

## 3. 启动 BJ / SJZ web_server

### 3.1 BJ 实例

在 BJ 代码目录：

```bash
PORT=8081 cargo run --bin web_server --features "web_server mqtt" -- --config DbOption-bj
```

### 3.2 SJZ 实例

在 SJZ 代码目录：

```bash
PORT=8082 cargo run --bin web_server --features "web_server mqtt" -- --config DbOption-sjz
```

启动后，可通过浏览器检查目录服务：

- `http://localhost:8081/assets/archives/`
- `http://localhost:8082/assets/archives/`

在首次初始化完成后，会看到对应的 `.cba` 文件：

- BJ：`assets/archives/ams251270_0001.cba`
- SJZ：`assets/archives/ams251181_0001.cba`

## 4. 通过 Web UI /remote-sync 启用 runtime

在各自站点打开：

- BJ：`http://localhost:8081/remote-sync`
- SJZ：`http://localhost:8082/remote-sync`

示例：在 BJ 侧新建环境 `bj-test`：

- Location：`bj`
- MQTT 主机：`127.0.0.1`
- MQTT 端口：`1883`
- 文件服务地址：`http://localhost:8081/assets/archives`
- 本地 DBNums：`251270`

保存后，点击：

- **写入配置**（Apply）：同步到 `DbOption-bj.toml`；
- **应用即生效**（Activate）：启动 runtime（watcher + MQTT 订阅）。

SJZ 侧同理，新建 `sjz-test` 环境并激活。

顶部“运行时状态”区域应显示：

- 当前激活环境 `env_id` 非空；
- MQTT 状态为“已连接”；
- 可通过刷新按钮更新状态。

## 5. 测试场景 A：已有 DB 文件的 sesno 增量

前提：

- BJ / SJZ runtime 均已启动；
- MQTT broker 正常。

步骤概述：

1. 在 BJ 测试目录中，对 `ams251270_0001` 执行一次增量导出（sesno 变大）。
2. BJ 进程日志应出现：
   - `发现需要增量更新的文件: "ams251270_0001" ...`
   - `发生了增量更新，推送：ams251270_0001`
3. BJ 会生成新的 `assets/archives/ams251270_0001.cba`，并通过 MQTT 发布 `SyncE3dFileMsg`。
4. SJZ 进程日志中应看到：
   - `Start delta clone db files num: 1 from http://localhost:8081/assets/archives/ams251270_0001.cba`
   - `Clone ams251270_0001 cost: ...`
5. SJZ 本地对应 DB 文件被 clone 更新，可通过文件时间戳或内容验证。

## 6. 测试场景 B：新增 DB 文件触发异地同步

在 `increment_manager.rs::async_watch` 中，对“新增 DB 文件”增加了处理逻辑：

- 新路径在 `watcher.headers` 中不存在时：
  - 初始化 headers；
  - 按 `location_dbs` 过滤 dbnum；
  - 生成 `assets/archives/{file_name}.cba`；
  - 若 `e3d_sync` 中不存在相同 `(location != 本地, file_name, file_hash)` 记录，则加入通知列表；
  - 后续复用原有逻辑写入 `e3d_sync` 并发布 `SyncE3dFileMsg`。

### 6.1 触发新增文件同步（BJ → SJZ）

1. 确保 BJ 已使用最新代码重新编译并运行。
2. 在 BJ 监控目录中拷贝一个“新 DB 文件”，例如：

   ```bash
   cp /Volumes/DPC/work/e3d_models/AvevaMarineSample/ams000/ams251270_0001 \
      /Volumes/DPC/work/e3d_models/test_bj/AvevaMarineSample/ams000/ams251270_0002
   ```

3. BJ 日志预期：
   - `在 watcher.headers 中找不到路径: ...ams251270_0002`
   - `发现新增 db 文件，推送：ams251270_0002`

4. SJZ 日志预期：
   - `Start delta clone db files num: 1 from http://localhost:8081/assets/archives/ams251270_0002.cba`
   - `Clone ams251270_0002 cost: ...`

5. SJZ 对应 DB 文件被 clone 更新，`assets/archives` 下可看到 `ams251270_0002.cba`。

### 6.2 注意事项

- `location_dbs` 用于限制“本地区负责的 dbnums”，新增文件只有在 dbnum 落在此列表内才会触发通知；
- 重复拷贝同一个 DB 文件且内容不变时，由于 `file_hash` 未变化，`e3d_sync` 去重会避免重复推送；
- 若需要测试 SJZ → BJ 方向，只需在 SJZ 目录按同样方式新增或更新 DB 文件，并确保 BJ 的 `location_dbs` 中包含目标 dbnum。

## 7. 验证要点总结

- Web 目录服务：
  - `/assets/archives` 是否可通过浏览器访问，并能下载 `.cba`；
- MQTT：
  - `/remote-sync` 顶部运行时状态中的 MQTT 是否为“已连接”；
  - 日志中是否出现 `SyncE3dFileMsg` 相关输出；
- Clone：
  - 对端日志中是否出现 `Start delta clone...` 与 `Clone ... cost: ...`；
  - 对端测试目录和 `assets/archives` 中对应 DB 文件/压缩包是否更新。

通过以上步骤，可在本机模拟 BJ / SJZ 两个区域间的 DB 文件增量同步流程，验证目录服务、MQTT 通知、增量解析与远程 clone 的整体链路是否工作正常。
