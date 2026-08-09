# E3D 增量更新自建夹具

该目录由 `l3_suite --fixture-manifest` 使用。运行器会：

1. 校验目标文件确实是指定编号的 DESI DB，并读取其 WORLD REF。
2. 通过 TTY 执行 `setup.mac`，在该 WORLD 下创建九个独立 SITE。
3. 启动本次运行专用的 RocksDB，执行 SYS 同步和目标 dbnum 基线解析。
4. 解析稳定名称到本次 E3D REFNO，依次执行 apply/restore 宏和四平面断言。
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
加 `-Ui` 才启动 plant-ui 冒烟检查。

## 只读预检

直接运行 `l3_suite` 并增加 `--fixture-check-only`，只生成 `preflight.json`，不会登录
E3D、启动服务或改写目标 DB。宏文件必须把会话退出交给 TTY wrapper，并且每个阶段
恰好包含一个 `SAVEWORK`。
