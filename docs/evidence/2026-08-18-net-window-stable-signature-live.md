# 2026-08-18 issue-019 净窗口全执行终态签名 live

状态：**正常档通过；强制空跑红证通过；源文件已恢复**。

## 固定真值

真值只取已跟踪的 `tests/fixtures/issues/issue-019-cross-session-parent-child-delete`：

- archive：4,497,104 字节，SHA256
  `6f7abbf548b37d8c016d2b8a2b52f3eddb1610fce1a00eca85fe71c9aa23f871`；
- baseline@24：10,776,576 字节，SHA256
  `aa199e88d6f962027bb8bbcb39a11ace78847866acace9d132eb460167aae2d0`；
- final@26：10,823,680 字节，SHA256
  `84b0040fdbc242d406540eab3d511d41a44aac899f55106821a93f5e419e6454`；
- 固定净三态：ZONE `24384_24775` modified；EQUI `24384_24778` 与
  BOX `24384_24779` deleted；added 为空。

loader 同时校验 ZIP 成员、每个快照的字节数/SHA 和 manifest presence，不调用
`parse.net_changes` 生成 oracle。

## 隔离与恢复

- SurrealDB：夹具自起 `127.0.0.1:8071`，2.1.4 fork，memory 后端。
- 项目副本：
  `test-increment/runs/codex-net-window-minimal-20260818-205623/projects`，只含 SYST+8000。
- 被替换前 db8000 SHA256：
  `2eae30556380eb79daf903cb15428e22df075e871e69acbcbed09a7edd337137`。
- fixture 用同卷临时文件 + `os.replace` 换入快照；所有路径（含断言失败）都在
  `finally` 恢复原文件、复核上述 SHA，并把 memory 库恢复到原 latest。

## 正常档

```powershell
$env:AIOS_NET_AB = '1'
Remove-Item Env:AIOS_T11B_FORCE_EMPTYRUN -ErrorAction SilentlyContinue
python/.venv/Scripts/python.exe -m pytest `
  python/tests/test_net_window_ab.py::test_net_window_full_execution_lands_a_stable_signature `
  python/tests/test_net_window_ab.py::test_net_window_agrees_on_a_stock_deletion `
  -q -s --tb=short -p isolate_plugin -p no:cacheprovider
```

结果：exit `0`，`2 passed in 32.94s`。

- baseline 后两个固定删除目标均为活行；
- preview 首条 warning 自报 ADR-031 净口径，`sessions=[25,26]`，a/m/d=`0/1/2`；
- 扫描→入队→worker→kv-mem 暂存 `staging_8000_1`→写回→水位完整执行；
- 队列窗口 `25..=26` 已消费，`changed_elements=3`，`merged_sesnos=[25,26]`；
- `applied_sesno` / `dbnum_info.sesno` / `file_latest_sesno` 均为 `26`；
- 墓碑集**恰好**为固定 EQUI、BOX；ZONE 保持活行；
- PE key 集与 baseline 完全相等，活行集恰少上述 2 项，没有额外 PE 行。

## 强制空跑红证与立即复验

```powershell
$env:AIOS_T11B_FORCE_EMPTYRUN = '1'
python/.venv/Scripts/python.exe -m pytest `
  python/tests/test_net_window_ab.py::test_net_window_agrees_on_a_stock_deletion `
  -q -s --tb=short -p isolate_plugin -p no:cacheprovider
```

结果：exit `1`，`1 error in 28.04s`。错误停在执行前活行断言：

```text
固定删除目标在起点不是活行：期望 ['24384_24778', '24384_24779']，实际 []；force_empty_run=True
```

清除 `AIOS_T11B_FORCE_EMPTYRUN` 后立即用同一命令复跑：exit `0`，
`1 passed in 32.68s`；末尾再次打印并核对原文件 SHA 为
`2eae30556380eb79daf903cb15428e22df075e871e69acbcbed09a7edd337137`。

## 回归发现

首次固定窗口运行发现 preview 只从实际操作分组产生 `sessions`，因此漏掉冻结清单中
没有净操作的会话 25。`fill_change_summary` 已改为先从 `CollectedWindow.session_sesnos`
建立零计数条目、再叠加操作计数；纯单测
`preview_sessions_come_from_the_frozen_session_page_list` 通过。修正只补齐诊断回执，
不改变 `collect_window`、绑定或 HTTP DTO 形状。
