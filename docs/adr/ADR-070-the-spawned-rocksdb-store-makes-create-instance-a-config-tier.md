# ADR-070：「创建数据库实例」是一档配置（spawned-rocksdb），不是一次性动作

状态：**已作废（2026-09-13，被 ADR-074 取代）**——`store_mode` 收成 `mem` / `rocksdb` 两档，**本进程从不拉起数据库**，
`spawned-rocksdb` 一档与它依赖的 `store_process.rs`（署名 / 心跳 / 四格裁决 / 收尸）整个删除。下文 D1–D6 全部失效，
只作历史记录保留；「新机器要一台落盘库」的现行路径是：先跑包内 `Start-Surreal.ps1`，再在向导第②步选 `rocksdb` 连它。
旧配置里写着 `spawned-rocksdb` 的，启动时按 `rocksdb` 生效并大声告警（ADR-074 D4）。

（原状态：**已接受**——2026-09-09 用户一轮 6 问全按推荐拍板。**号到提交时复核**——ADR-060 曾撞号两次。）
日期：2026-09-09
落地：`src/options.rs`（`StoreMode::SpawnedRocks`）、`src/store_process.rs`（`SpawnDatastore` 参数化 +
`default_rocksdb_dir`）、`src/lib.rs`（分派臂 + 横幅）、`src/web_service/setup.rs`（判据与写口扩档）、
`web/setup.html`（第②步第三档）、`scripts/New-LocalRelease.ps1`（冒烟 C 腿 + Start-Surreal 修坑）。
关联：
- 机制基座：`docs/plans/2026-09-04-spawned-memory-store-out-of-box.md`（署名 / 四格裁决 / 收尸 /
  版本闸，P1–P4 已服役）——本档**全字复用**那套机制，唯一真改动是把写死的数据源参数
  `"memory"` 参数化成 `rocksdb:<目录>`。
- **修订 ADR-069 D3 的一句**：设置模式判据从「external 且地址空」扩成「**需要端点的档位**
  （除 embedded-mem 外）且地址空」——spawn 两档空地址是必坏形状（bind 端口 0 / `ws://:0`），
  给手改坏形状一个逃生门。其余判据（project_path、库连不上不算）不动。
- 计划与 6 问决策表：`docs/plans/2026-09-09-spawned-rocksdb-store-and-create-instance.md`。
- CONTEXT.md 新增词条「自带持久库」。

## 背景

用户要「创建数据库实例」：指定 ip（默认本机）、端口、项目名、代号等。0.1.20 的向导第②步只有
「连已有库（external）」与「试用内存库（spawned-mem）」，新机器想要一台**落盘**库仍要手跑
`Start-Surreal.ps1`——而那台手工库没有属主，机器重启后不在，服务只会报 external 连不上。

## 决定

### D1　「创建实例」= 选一档配置，生命周期归服务

新 `store_mode = "spawned-rocksdb"`：服务每次启动用包内 surreal（fork 2.1.x，版本闸不放宽）
在 `v_ip:v_port` 拉起一台 **rocksdb 后端**的库，退出照常收尸（杀的是进程，数据在盘上），
下次启动重新拉起即接上——**重启即自愈**，没有「实例还在不在」这个问题。署名 / 心跳 /
四格裁决（空闲拉起 / 自家附着 / 孤儿收尸 / 别人的库拒启）与 spawned-mem 逐字同一套；
孤儿收尸对本档**不丢数据**。探针 / python / MCP 照旧 `AttachOnly`，不为本档开口子。

### D2　参数复用现有键，数据目录按项目代号分家

绑定地址就是 `v_ip` / `v_port`（本档语义「绑哪」，向导默认 127.0.0.1:8010）；数据目录**不设键**，
固定 `workspace\.surreal\<project_code>`（代号空 / "0" 回落项目名，再回落 `ams`；目录名滤掉
路径不安全字符）。**换代号 = 新目录新库，旧目录留在盘上人工清——这不是丢数据**，向导文案
与本 ADR 都要说。

### D3　`v_ip = 0.0.0.0` 允许，但凭据是硬前提

「一台工作站起库、多台连它」是真实部署形态。向导选 0.0.0.0 时明示：库的凭据默认
root/root，**先改 `v_user` / `v_password` 再暴露**。setup 面回环闸（ADR-069 D4）不因此放宽。

### D4　档位语义一张表（每格有护栏）

`is_durable()`：External | SpawnedRocks；`data_face` 派生：durable → ingest；实例锁：照拿
（只有 embedded-mem 跳）；`/health.medium`："spawned-rocksdb"（`as_str` 直通，两个分支自动跟）；
`PersistMedium`：归 `RocksDb`（那枚枚举要紧的是「活得过重启」）。

### D5　出厂模板不改；写口与判据同律扩档

出厂仍是 external + 空地址（ADR-069 D5 不动）。spawn 两档空地址：写口合并后校验拦
（422、盘上不动，错误里带各自默认 8009/8010），前端选档时自动补默认；已经落盘的坏形状
由 D 修订的设置模式判据兜住。「出厂直接改 spawned-rocksdb」是产品形态决定，单独拍。

### D6　冒烟门第三腿（修订 ADR-068 D6 / ADR-069 D8）

C 腿：本机配置副本改 `spawned-rocksdb`@18010（绝不碰真 8010）起包内 exe，期待正常模式
`/health` 报 `medium:"spawned-rocksdb"` + `durable:true` 且 `workspace\.surreal\` 目录真落盘；
强杀后按端口收掉自带 surreal、清临时目录。任一腿失败照旧标「未冒烟」，`-StrictSmoke` 不出包。

## 后果

- 新机器首跑向导：①选项目 → ②选「新建本机持久库」（地址留空即默认）→ ③④保存重启 →
  ⑤初始化——全程零命令行，数据活得过重启。
- `Start-Surreal.ps1` 降为「external 部署（库独立于服务起停）才需要」的手动路径；顺带修掉
  它把出厂 `v_port="0"` 哨兵当端口用的坑（读到 0 回落 8010）。
- spawned-mem 手改成空地址的坏形状从「启动死在 bind 端口 0」变成「进设置模式」——语义更软，
  是有意的（形状自身注定起不来的配置进向导，不进启动失败）。
- rocksdb 目录不自动瘦身：磁盘占用随库规模走，release-notes 提示；旧代号目录人工清。

## 实施约束

1. 数据源是 `surreal start` 的**位置参数**（`the_datastore_is_a_positional_argument_not_a_flag`
   扩到 rocksdb 形态）；目录命名由 `the_rocksdb_dir_is_keyed_by_project_code_with_fallbacks` 钉。
2. 档位真值表护栏：`only_the_disk_backed_stores_are_durable_and_only_the_embedded_one_has_no_port`
   + `data_face_follows_storage_backup_scope`（rocks → ingest）。
3. 设置模式判据真值表扩档：`setup_mode_triggers_on_missing_project_path_or_blank_endpoint_address`
   （spawn 两档 × 空地址 = 设置模式；embedded-mem 不需要地址）。
4. 写口合并后校验扩档：`write_config_refuses_spawned_mem_with_a_blank_address`（含 rocks 分支，
   错误里带各自默认端口）。
5. 版本闸 / 署名 / 收尸与 spawned-mem 同源同律，**不为 rocksdb 放宽任何一格**——3.x 打开既有
   rocksdb 目录会要求不可逆迁移，版本闸挡的正是它。
