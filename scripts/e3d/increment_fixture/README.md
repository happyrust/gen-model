# E3D 增量更新自建夹具

该目录由 `l3_suite --fixture-manifest` 使用。运行器会：

1. 校验目标文件确实是指定编号的 DESI DB，并读取其 WORLD REF。
2. 通过 TTY 执行 `setup.mac`，在该 WORLD 下创建十个独立 SITE。
3. 启动本次运行专用的 RocksDB，执行 SYS 同步和目标 dbnum 基线解析。
4. 解析稳定名称到本次 E3D REFNO，依次执行 apply/restore 宏和四平面断言；每一窗之后再经
   `GET /api/v1/datacenter/deletes` 核交付删除记录：本案范围内的行必须恰好是 `datacenter_delete` /
   `datacenter_delete_after_restore` 声明的那几行（没声明 = 一行都不许有），新落的行 `sesno` 是这一窗、
   `published = true`（运行器只为声明了事件的对象预置同 refno 的 `datacenter_version`），并用 `from` / `to`
   夹住它的 `sesno_time` 查回来；同一 refno 的恢复改名由 `INSERT IGNORE` 忽略，仍只保留首次事件。
5. 生成 `summary.json`、`junit.xml`、`report.md`、`report.html` 及逐案例证据。

## 一条命令运行

```powershell
powershell -File scripts\Run-E3DFixtureSuite.ps1 `
  -TargetDbFile 'D:\AVEVA\Projects\E3D31-L3\AvevaMarineSample\ams000\ams7999_0001' `
  -TargetDbnum 7999 `
  -ProjectDir 'D:\AVEVA\Projects\E3D31-L3\AvevaMarineSample' `
  -AiosProject AvevaMarineSample `
  -AiosNamespace 1516 `
  -E3dProject AMS `
  -E3dMdb /ALL
```

目标 DB 必须已加入 MDB 且对登录用户可写。默认结束时删除夹具 SITE；失败证据和该次
RocksDB 保留在 `output/e3d-fixture/<run-id>`。加 `-KeepSites` 可保留 E3D 夹具，
加 `-Ui` 才启动 plant-ui 冒烟检查。界面连的是夹具自己的栈（默认 18028 / 18048，`L3_FIXTURE_API_PORT` /
`L3_FIXTURE_SURREAL_PORT` 可改），项目配置与设置文件都按这对端口现写（2026-09-29 之前写死金套件的 8028 / 8048）。

本机没有 `E3D31-L3` 副本时，直接对 `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample` 跑也行——
选一个小的 DESI 做靶（`ams7998_0001` / 7998，基线解析几秒钟），夹具 SITE 都建在它的 WORLD 下、
跑完即删。桌面上开着 E3D GUI 会话时设 `L3_ALLOW_EXISTING_E3D_SESSION=1`，驱动只等待并清理
自己起的那一个 TTY 会话（2026-09-20 实跑：9/9 PASS，约 6 分钟）。

运行器给隔离服务准备的东西（2026-09-20 起，之前三处都断在起服务 / 播种）：`<run>/resource/surreal`
（aios_core 连库时按 CWD 找函数脚本）、`project-mirror` 里目标 DB 旁的 `amssys` / `amscom` / `amsmis`
（服务从 SYS **文件**读 MDB 声明）；播种前先等启动的模型补齐任务落定，撞上 409 只重试不判死。
房间归属按**代码默认**跑（2026-09-29 起默认开）：渲染出的配置里去掉 `room_membership` 这一行，
各进程也不带 `AIOS_ROOM_MEMBERSHIP`——两个 `room: true` 场景断言的是 `room_relate` 边，默认一旦退回关它们就红。

## 只读预检

直接运行 `l3_suite` 并增加 `--fixture-check-only`，只生成 `preflight.json`，不会登录
E3D、启动服务或改写目标 DB。宏文件必须把会话退出交给 TTY wrapper，并且每个阶段
恰好包含一个 `SAVEWORK`。
