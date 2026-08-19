# E3D TTY × 解析器语义净窗口验收

日期：2026-08-14  
目标：证明 TTY `SAVEWORK` 后可直接从 dabacon 文件取得指定会话范围的语义增删改，
不查询 SurrealDB；apply 后必须执行 restore，并验证业务属性恢复。

## 发现与修复

首次使用 `aios_db.parse.net_changes(215, 216)` 时，目标 FTUB 在 apply + restore 后仍被
报告为 `modified`。根因是该入口输出**会话索引记录位置触达三态**：两端记录换页即
Modified，尚未运行属性合成层，不能代表属性语义净变化。

新增 `aios_db.parse.net_window(path, start, end, detail=True)`，复用生产
`net_window::collect_net_window`，在文件内读取 base / 终稿并做一次属性 diff：

- 过滤位置换页但内容相同的原样重写；
- 返回真正的 Add / Deleted / Modified 属性载荷；
- 继续如实保留 E3D 自动保存元数据（本例 BRAN.CACHID）；
- 全程不连接 SurrealDB。

## 自动化用例

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File scripts\e3d\Test-TtyNetWindow.ps1
```

实现：`scripts/e3d/Test-TtyNetWindow.ps1`。脚本执行：

1. 复制原文件作为 baseline 证据并计算 SHA-256；
2. 经 `l3_suite --check-driver` 执行 apply（恰好一次 `SAVEWORK`）；
3. 直接调用 `parse.net_window`，断言目标 `24384_23262` 为 Modified、POS.U
   `2900 → 3400`；
4. `finally` 中执行 restore（恰好一次 `SAVEWORK`）；
5. 断言 POS.U `3400 → 2900`、目标全属性等于 baseline、合并窗口不再含目标；
6. 合并窗口只允许 BRAN.CACHID 保存元数据残留。

## 本轮结果

最终复验目录：`output/e3d-tty-net-window/20260814-085034/`（运行产物不入 git）。

- 会话：baseline 218 → apply 219 → restore 220；
- apply：FTUB.POS.U `2900 → 3400`；
- restore：FTUB.POS.U `3400 → 2900`；
- 合并窗口：目标 FTUB 消失（业务变化净零）；
- 合并窗口剩余 1 条 BRAN Modified，仅 `modified_explicit.CACHID`；
- rollback：已执行并验证；
- exit status：0。

四个可复核角色：修改对象为
`D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001`；基线副本为
`baseline-db-file.copy`（SHA-256
`0091ff5029e32dc274026ac599efe4dd3a8fa8ebfb0e570f4408762f101f5791`）；语义
patch/diff 为 `semantic-window-diff.json`；验证记录为 `summary.json`；rollback 命令
及 exit status 记录在 `restore-driver.json`，并由 `summary.json.rollback.verified=true`
确认。最终文件会话 220 的目标 POS.U 已重新打开验证为 2900。

同时通过：Python TTY 编排 10、`e3d_query` 5、`l3_suite` 20、`e3d_mcp` 2、
真实 `live_identity_query` 1、Python 解析离线档 12。

## 2026-08-19 续测

再次执行 `scripts/e3d/Test-TtyNetWindow.ps1`，exit status 0。证据目录：
`output/e3d-tty-net-window/20260819-082310/`。

- 会话：baseline 230 → apply 231 → restore 232；
- apply / restore 均为 2 条 Modified（目标 FTUB + BRAN.CACHID）；
- FTUB.POS.U：`2900 → 3400 → 2900`，最终元素属性与 baseline 相等；
- 合并窗口 231..=232 中目标 FTUB 消失，`unchanged_rewrites=1`；
- 合并窗口仅剩 BRAN.CACHID `14 → 16`，无业务属性残留；
- `summary.json.rollback.verified=true`，restore driver exit status 0；
- 本轮结束后重新枚举进程，只存在 2026-08-15 已启动的既存
  `des.exe` / `PDMSConsole.exe` 会话，本轮未遗留新会话。

四个复核角色均已重新打开：修改对象
`D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001`；基线副本
`baseline-db-file.copy`（SHA-256
`1bd3c8306486572c3d6be5d7b623da2a05b2df9336f803bdbbedd79eb3471b56`）；
语义 patch/diff `semantic-window-diff.json`；验证记录 `summary.json`；rollback
命令与字面输出见 `restore-driver.json` / `restore-driver/check-driver.log`。
最终文件因正常追加 restore 会话而具有新的文件 SHA-256
`fb8d66ac8f9186b8e6de43c484825ea143cc28de05225987deeac346c3b69311`；回滚判据是
目标业务属性恢复而非文件逐字节回退。
