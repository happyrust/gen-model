# 管道增量 + 房间增量 端到端联测（2026-10-23，房间归属恒开之后的第一轮）

## 场景

真实 E3D SAVEWORK → gen-model（127.0.0.1:8023，AvevaMarineSample / /ALL / ns 1516，`watch_dbnums = [7998, 8000]`，持久层是
`D:\work\plant-code\old\test-issue-regen-model\ams-db` 那台 surreal 1516）数据批次 → 模型重算 → 房间边重算，管道、房间各往返一轮。
服务是本日「`room_membership` 开关退役」那次构建（`D:\Rust\target\debug\aios-database.exe` 11:14）：配置里还写着的
`room_membership = false` 只在横幅被点名（`*** ⚠ DbOption.toml 里的 room_membership = false 已退役（…）；本进程算得了房间，按开跑`），
常驻房间模型照建（215 间房 / 230 块面板，代 `9b655a48…`）。这台夹具库从没对齐过房间，启动走了一次 29 个 dbnum 的完整房间初始化，
耗时 25m31s（其中 dbnum 7333 占 10m48s），之后才开始本测。

## 执行方式

E3D TTY：`scripts/e3d/run_ams_c_entrymacro.bat` + 包装宏（`evidence/driver_*.mac`：L3-ALIVE / `$M` / L3-DONE / QUIT）。
**工作区里那份未提交的 bat 把默认运行时改成了 `D:\work\plant-code\old\e3d31-runtime`，它起不来（des.exe 7 s 内退出，0xC0020001，bat 自己的注释也说这不是验过的运行时）**；
本测用环境变量 `L3_E3D_INSTALL_DIR=E:\reverse\e3d\shadow_e3d31_gen_model_test`（HEAD 里的默认值，2026-10-16 那轮用的就是它）压回去，没动那个文件。
每腿恰好一次 SAVEWORK；`POST /api/v1/update/execute` 没用，全靠 watcher 自动入队。`ams8000_0001` 动手前已备份到
`.scratch/fixture-8023/backup/ams8000_0001.baseline-20261023`（SHA256 `55AB7205…C0D2C`，不入库）。

## 管道：BRAN `/C-OR-1R345-C`（`24384/23257`，dbnum 8000）的 FTUBE 4（`24384/23262`）抬 500 mm

- 基线：`Position E 10887mm N 12332mm U 2900mm`；`/model/records` 9 件，`24384_23262` `world_trans.t = (10887.02, 12332.26, 2900)`，aabb z 2900..2930；
  `e3d.room.lookup` 对 `24384_23262` 为 `ready_empty`（见下文「基线行来自老生成器」）。
- apply（`pipe_apply.mac`，12:12:48 SAVEWORK，sesno 652→653）：E3D 回读 `U 2900mm → U 3400mm`。watcher 12:12:5x 发现文件 651→653，
  任务 `db-20261023-121254-000001` `succeeded`：`modified_elements=3`，unit `24384/23257 BRAN generated kind=regen`，`model_verdict=in_sync`，
  stages `data → generation → publication → spatial → rooms`，总耗时 4.4 s（`model_generate` 334 ms）。生成器点名「未产管=3（misaligned=2 reversed=1）」——
  单抬一根显式管段、邻居不动，隐式管身接不上是应有的几何后果。
- 模型：`24384_23262` `world_trans.t = (10887, 12332, 3400)`，aabb z 3400..3430（`evidence/pipe_apply_records_24384_23257.json`）；`/dbnums` 8000 `653/653/653 in_sync`，`room_alignment.current=true`。
- 房间（管道也进了房间增量）：抬到 U 3400 后 `24384_23262` 的归属从空变为 **R113（面板 24381_1204）+ R345（面板 24381_35186）**，`published_version` 651→653，常驻代不变。
- plant-ui（`PLANT_UI_SETTINGS_FILE=…\.scratch\ui-test\settings.ron`，指 8023）：搜 `/C-OR-1R345-C` → 右键「显示模型」，日志「查询到 9 个元素，9 个网格实例 … 模型显示完成」，三维视图里那根管段明显高出其余支管、两端断开（截图在 `.scratch/winmcp/incr-shot-8-1.png`，不入库）。
- restore（`pipe_restore.mac`，12:53:20 SAVEWORK）：E3D 回读 `U 3400mm → U 2900mm`。**watcher 在 E3D 还在写的时候先看到了 654**：任务 `db-20261023-125324-000002`（654→654）在提交前被
  「冻结源身份复核」拦下（`dabacon 冻结目标会话 654 的 session/index anchor 已改变`，`failed`，水位不推），5 s 后重扫排出 `db-20261023-125329-000004`（654→655）`succeeded`，
  同一根 regen。这是设计内的护栏，不是缺陷；证据 `evidence/pipe_restore_task_8000.json` / `_followup.json`。
- restore 后：`24384_23262` 回到 `(10887, 12332, 2900)`，9 件与 apply 后逐件对比只有它一件差 500 mm（`evidence/pipe_restore_records_24384_23257.json`）；`/dbnums` 8000 `655/655/655 in_sync`。

**基线行来自老生成器**：夹具库里这根的存量行（sesno 651，用户 2026-10-22 的构建写的）里 5 件 FTUBE `insts` 为空、`24384_23263` 有 4 件；本构建重算后每件恰好 1 个 inst，
所以「重算后 vs 存量行」有 2 mm～143 mm 的几何差，而「FTUBE 在 U 2900 没有房间」也只是因为存量行没有几何可算——重算后回到 U 2900 它落在
**R213（面板 24381_855，inside 8）+ R345（inside 6）**。判「往返幂等」要拿 apply 后的行比 restore 后的行（只差那 500 mm），不能拿存量行比。

## 房间：EQUI `/1-LNR-Q005-PJ`（`24384/24776`，dbnum 8000）U 1400 → 53400 → 1400

与 `verification/incr-model-room-20261016/` 同一套变更体：叶子 BOX `24384/24777` 基线在 **R224**（面板 `24381_948`，inside 4，center_dist 2785.836；跨库边：成员 8000 / 面板 7997）。

- apply（`room_apply.mac`，13:06:1x SAVEWORK，656→657）：任务 `db-20261023-130624-000006` `succeeded`，unit `24384/24776 EQUI generated regen`，`in_sync`，整趟 2.0 s；
  `24384_24777` `world_trans.t.z` 1400→53400，aabb z 53125..53675；`e3d.room.lookup` **R224 → R000（面板 24381_36945）**，`published_version` 655→657，`stale=false`；
  `room_relate` 里 `out=pe:24384_24777` 只剩 `in=pe:24381_36945 R000 inside_count=8`（旧 R224 边已删）；常驻代仍是 `9b655a48…`（定向重算，不换代）。
- restore（`room_restore.mac`，13:12:5x SAVEWORK，658→659）：任务 `db-20261023-131254-000008` `succeeded`；`world_trans.t.z` 回 1400，aabb 1125..1675；
  归属回 **R224（面板 24381_948，inside 4，center_dist 2785.836）**，与 2026-10-16 的 restore 边逐字相同；`/dbnums` 8000 `659/659/659 in_sync`，`room_alignment.current=true`。

## 任务链

```
db-20261023-121254-000001  succeeded  8000  652→653   管道 apply
db-20261023-125324-000002  failed     8000  654→654   管道 restore，文件还在写，提交前身份复核拦下（水位不推）
db-20261023-125329-000004  succeeded  8000  654→655   管道 restore（重扫补排）
db-20261023-130624-000006  succeeded  8000  656→657   房间 apply
db-20261023-131254-000008  succeeded  8000  658→659   房间 restore
```

每批 stages `data → generation → publication → spatial → rooms`，房间重算在批内（`durable_pending`）完成，没有单独房间轮；每批还伴随一条 `amssys`（8191）的 SYST 批次。

## 结论

- 管道增量：FTUBE 位移传到了 `inst_relate` 的 `world_trans` / `aabb`，根 regen、水位按会话推进（651→653→655），plant-ui 显示出来；往返幂等。✓
- 房间增量：管段和设备两类成员的边都在增量窗内删+插（FTUBE 空→R113+R345→R213+R345；BOX R224→R000→R224），跨库边正常，常驻模型不换代。✓
- 房间归属恒开：配置里的 `room_membership = false` 不再关得掉它，只被点名。✓

## 顺带观察（未处理）

- plant-ui 在 `read_face: service` 下选中元素时日志报「房间归属查询失败」：它的 `plant_ui_data::room::element_rooms` 直连 SurrealDB（`SUL_DB.query`），不走服务的 `e3d.room.lookup`；同一个 refno 用 `/api/v1/query` 问服务是 200。与本次改动无关。
- 未提交的 `scripts/e3d/run_ams_c_entrymacro.bat` 默认运行时起不来（见上）。
- 本机另一台 aios-database（PID 17712，端口 18522）全程在跑；若它也监听这个工程目录，这 8 个会话它也收到了。
