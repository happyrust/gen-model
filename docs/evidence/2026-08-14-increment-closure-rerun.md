# Plant UI + E3D 增量闭环修复复验

日期：2026-08-14
工作区：`D:\work\plant-code\old\test-worklspace`（目录名按现有夹具保留）

## 夹具预检

命令：

```text
l3_suite --fixture-manifest scripts/e3d/increment_fixture/fixture-manifest.json --target-db-file D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001 --target-dbnum 7999 --aios-project AvevaMarineSample --aios-namespace 1516 --project-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample --fixture-check-only --output D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\preflight
```

- 退出码：`0`
- 解析结果：`dbnum=7999`、`db_type=DESI`、`WORLD=16191/0`、`scenario_count=9`
- 原始输入、输出与退出码：
  - `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\preflight-command.json`
  - `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\preflight-command.log`
  - `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\preflight\preflight.json`

## Plant UI 三端运行

命令与预检相同，去掉 `--fixture-check-only` 并增加 `--fixture-ui`，输出目录为
`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\equipment-ui`。

- 退出码：`1`
- 结果：启动门在任何变更宏执行前发现既有 E3D 会话，故本轮未写入目标文件、未推进水位，也未执行恢复宏。
- 保留的既有进程：`des.exe` PID `68872`、`58016`、`35528`，以及配套 `PDMSConsole.exe`。
- 原始输入、输出与退出码：
  - `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\equipment-ui-command.json`
  - `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\equipment-ui.stdout.log`
  - `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-closure-20260814\equipment-ui.stderr.log`

该结果只证明新预检和互斥保护生效，不登记为设备/管道 CRUD live 通过。完整三端场景仍需在 E3D 空窗重新运行。
