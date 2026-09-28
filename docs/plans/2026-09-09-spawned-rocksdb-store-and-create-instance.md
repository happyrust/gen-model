# 2026-09-09 自带持久库档（spawned-rocksdb）：向导里「创建数据库实例」

一句话：给 `store_mode` 加第四档 **`spawned-rocksdb`**——服务每次启动自己拉起一台
**rocksdb 后端**的包内 SurrealDB（绑 `v_ip:v_port`，数据落 `workspace\.surreal\<project_code>`，
活得过重启），退出照常收尸；配置向导第②步加第三档「新建本机持久库」。「创建实例」由此
= 选档 + 填参数，首次正常启动自动建库，不引入任何一次性管理动作。

承接：向导计划 `2026-09-09-setup-wizard-web-frontend.md` Q5 留给 v2 的档、ADR-069 后果节点名的
「一键用包内 surreal 起持久库」。机制基座：`2026-09-04-spawned-memory-store-out-of-box.md`
（署名 / 四格裁决 / 收尸 / 版本闸，P1–P4 已服役）。

## 1. 已拍决策（2026-09-09 用户一轮 6 问全按推荐）

| # | 决策 |
| --- | --- |
| Q1 | **新档位**，不是一次性「创建」动作：`spawned-rocksdb` = 服务每次启动自己拉起、退出收尸、数据落盘活得过重启。一次性动作的实例没有属主（机器重启后不在，external 连不上又回到手动）；档位形态重启即自愈，四格裁决对 rocksdb 同样成立（孤儿收尸不丢数据——数据在盘上） |
| Q2 | **复用现有键**：绑定地址就是 `v_ip` / `v_port`（本档含义「绑哪」，向导默认 127.0.0.1:8010）；数据目录**不设键**，固定 `workspace\.surreal\<project_code>`（项目名/代号来自向导第①步） |
| Q3 | `v_ip` **允许 0.0.0.0**（局域网可连），向导明示警告：凭据默认 root/root，暴露前改 `v_user` / `v_password`。`resolve_bind` 对全 0 的探活口径现成（探回环） |
| Q4 | 向导第②步加第三档单选「**新建本机持久库（新部署推荐）**」：ip/端口带默认、数据目录只读展示；本档「测试连接」换成「端口预检」（库还没起，测连通没有意义）；保存重启后首次正常启动自动拉起并建库 |
| Q5 | **出厂模板不改**（仍 external + 空地址 → 设置模式）；spawned-rocksdb 与 spawned-mem 同律：写口合并后校验拦空地址（422、盘上不动），前端选本档且地址空时自动补 127.0.0.1:8010。出厂直接改本档（解压即有落盘库）另拍，不在本轮 |
| Q6 | 全链交付：本计划 + **ADR-070** + CONTEXT 词条；`Start-Surreal.ps1` 保留（external 部署仍用）但修「出厂 `v_port="0"` 被解析成端口 0」的坑 + 指引改口；随 **0.1.21-full** 出包，冒烟加 **C 腿**（本档起到正常模式、库文件真落盘） |

## 2. 档位语义（一张表定死，每格都要有护栏跟）

| 档位 | 谁拉起 | 介质 | durable | data_face 派生 | 实例锁 | /health.medium |
| --- | --- | --- | --- | --- | --- | --- |
| external | 没人（连现成的） | 对端说了算 | true | ingest | 拿 | external |
| **spawned-rocksdb（新）** | **本进程** | **rocksdb（落盘）** | **true** | **ingest** | **拿** | **spawned-rocksdb** |
| spawned-mem | 本进程 | memory | false | read-through | 拿 | spawned-mem |
| embedded-mem | 进程内嵌 | mem:// | false | read-through | 跳 | embedded-mem |

- `is_durable()`：`External | SpawnedRocks` 回真。`PersistMedium`：本档归 `RocksDb`
  （那枚枚举要紧的是「活得过重启」，2026-09-04 计划 P4 的原话）。
- **署名 / 四格裁决 / 心跳 / 孤儿回收一字不改**：`spawn_and_stamp` 对 rocksdb 与 memory
  完全同构；孤儿收尸对本档**不丢数据**（杀的是进程，rocksdb 目录在盘上，重起即接上）。
- `StoreStartup::AttachOnly`（探针/python/MCP 只附着不拉起）对本档同律，无需改。

## 3. 实现连锁表（加一档 = 每处加一臂）

| 位置 | 改什么 |
| --- | --- |
| `options.rs` | `StoreMode::SpawnedRocks`（解析 `"spawned-rocksdb"`）；`as_str` / `is_durable` / `has_endpoint` / `data_face_for_store` / `store_mode_notice` 各加一臂；档位真值表测试跟上 |
| `store_process.rs` | 数据源参数化：`"memory".into()`（:506）改为按档给 `memory` 或 `rocksdb:<workspace>\.surreal\<project_code>`（目录不存在自动建父目录；路径进参数前不需要转义——是位置参数）。`ensure_spawned_store` 加 datastore 入参；日志与错误里点名介质与目录 |
| `lib.rs` | 连库分派 `match store_mode()` 加 SpawnedRocks 臂（与 SpawnedMem 共走 `ensure_own_store` + `connect_configured_store`，只差 datastore）；横幅说「存储介质：rocksdb（store_mode=spawned-rocksdb，本进程自带，数据落 <目录>）」 |
| `setup.rs` | ①合并后校验扩到本档（空地址 422，文案带默认 127.0.0.1:8010）；②`setup_mode_reason_for` 扩一条：**spawn 两档且 `v_port=0`（哨兵）= 配置无效进设置模式**（手改坏形状给逃生门，不再死在 bind 端口 0；external 判据不动）；③真值表测试跟上 |
| `web/setup.html` | 第②步第三档单选（默认值提示、0.0.0.0 警告、数据目录只读展示 `workspace\.surreal\<代号>`）；本档下「测试连接」按钮换「端口预检」（复用 `/api/v1/setup/probe/port`，送 `v_ip:v_port`；occupied 时提示「若是上次自家库，启动时会自动收编/收尸」）；`gatherPatch` 本档空地址自动补 127.0.0.1:8010；**顺带**：radio-line 外层包 `<label>` 提升可点面积（走查发现 13px 圈难点中） |
| `scripts/Start-Surreal.ps1`（打包生成段） | `v_port` 读出 `0` 或读不出一律回落 8010；`.SYNOPSIS` 指引改口「向导第②步第三档可替代本脚本」 |
| `scripts/New-LocalRelease.ps1` | 冒烟 **C 腿**：本机配置副本改 `store_mode="spawned-rocksdb"`、`v_port=18010`（避开真 8010）、临时 workspace → 起服务 → 期待 `/health.sul_db` `{medium:"spawned-rocksdb", durable:true}` + `.surreal\<代号>` 目录真的出现在临时 ws → 杀进程清目录。A/B 两腿照旧 |
| 文档 | ADR-070（档位语义表 + 「创建实例=选档」+ 0.0.0.0 须改凭据的硬边界 + 数据目录按代号分家）；CONTEXT 词条「自带持久库 (Spawned RocksDB Store)」；`docs/release-notes/0.1.21.md` + changelog 条目 |

## 4. 关键边界（ADR-070 要写死的）

1. **数据目录按项目代号分家**：`workspace\.surreal\<project_code>`。换代号 = 新目录新库，
   旧目录留在盘上人工清——**换代号不是丢数据**，这句写进向导第③档展示文案与 ADR。
2. **0.0.0.0 绑定的安全边界**：库的认证是 surreal 自己的 `v_user`/`v_password`（默认 root/root）；
   向导选 0.0.0.0 时明示「先改凭据再暴露」。setup 面回环闸（ADR-069 D4）不变。
3. **spawn 档 `v_port=0` = 配置无效**（新增进设置模式判据）：这与「external 空地址」同理——
   形状自身注定起不来的配置进向导，不进启动失败。**库地址填了但连不上仍不算**（照旧）。
4. 版本闸 / 署名 / 收尸口径与 spawned-mem 逐字同源，不为 rocksdb 放宽任何一格
   （3.x 打开既有 rocksdb 目录会要求不可逆迁移——版本闸挡的就是这个）。

## 5. 测试与验收

- 单测：档位真值表（durable/has_endpoint/data_face/锁/medium 五格 × 新档）；datastore 参数
  形状钉（rocksdb 路径按代号、memory 照旧）；设置模式判据新真值表（spawn 两档 × v_port=0）；
  写口合并校验扩档；向导页内嵌钉（第三档标记）。
- 冒烟三腿全过（A 出厂设置模式 / B 本机全启动 / C 本档落盘）。
- 实机走查：临时 workspace 走向导选第三档 → 保存重启 → 正常模式 → `.surreal\<代号>` 出现
  → 杀进程重启 → 数据还在（durable 的直接证据）→ 孤儿场景（强杀后重启收编）。
- 收口：`New-LocalRelease.ps1` 出 `ams-local-0.1.21-full.zip` 三腿冒烟通过。

## 6. 顺序与协调

1）options/store_process/lib 档位臂 + 单测 → 2）setup.rs 判据与写口扩档 → 3）setup.html 第三档
→ 4）打包脚本（Start-Surreal 修坑 + C 腿）→ 5）文档 → 6）0.1.21-full 出包验收。

协调：`handlers.rs` / `ops.html` / `query_service.rs` / `room_task.rs` 等此刻有别的会话在途改动，
**一概不碰**；`store_process.rs` / `options.rs` / `lib.rs` 动前先看 `git status` 错峰；提交只点名
本批文件。

## 7. 风险

- **既有 rocksdb 目录被 3.x 动过**：版本闸拦 3.x 服务端，但目录若已被外部 3.x 迁移过，2.1.x
  打不开——错误带目录路径与「用当时的版本导出再导入」的出路，不静默。
- **端口被真 8010 生产库占着**：判据四格表本来就拒「别人的库」；向导端口预检提前把话说清。
- **C 腿在打包机上的隔离**：v_port 换 18010 + 临时 workspace，绝不碰 8010 真库（leg B 先例同款）。
- **`.surreal` 目录膨胀**：rocksdb 不自动瘦身；ADR 写明「按代号分目录、旧目录人工清」，
  release-notes 提示磁盘占用随库规模走。
