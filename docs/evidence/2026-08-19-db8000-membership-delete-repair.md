# dbnum=8000 / sesno 236 成员删除纠正证据

日期：2026-08-19；项目：`AvevaMarineSample`；文件：
`D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`（mtime
`2026-08-19 13:27:41`）。

## 收集对拍

```powershell
cargo test --locked --lib live_ams8000_ses236_membership_delete_matches_expected_net `
  --no-default-features --features ws,gen_model,manifold,project_hd -- --ignored --nocapture
```

字面结果（exit 0）：

```text
running 1 test
test data_interface::increment_pipeline::cache_tests::live_ams8000_ses236_membership_delete_matches_expected_net ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1003 filtered out; finished in 0.50s
```

断言值：`Add=0 / Modified=1 / Deleted=1 / membership_deleted=1`，删除目标
`24384/26201`。

## 纠正前

```text
watermark = 236
pe:24384_26201 = { noun: STRU, owner: pe:24384_26199, deleted: false, sesno: 0 }
pe_owner WHERE in=pe:24384_26201 = []
pe:24384_26199 = { noun: ZONE, deleted: false, sesno: 236 }
```

health：`model_ready`、worker idle，`staging_windows=[]`。

## 维护纠正

先停止前台服务，再执行：

```powershell
.\db_window_repair.exe --dbnum 8000 --from 236 --to 236 --expect-watermark 236 `
  --file 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001'
```

首次全库无 OWNER 入边审计命中窗口外历史孤立行 `24384_26184`，工具按
fail-closed 终止（exit 1），尚未创建/提交 staging，水位与数据未变。审计随后
收窄到本窗口发生成员变化的 owner，避免将全库历史债务混入单窗口纠正。

最终字面结果（exit 0）：

```json
{
  "dbnum": 8000,
  "from_sesno": 236,
  "to_sesno": 236,
  "added": 0,
  "modified": 1,
  "deleted": 1,
  "membership_deleted": 1,
  "unreachable_rows": 0,
  "watermark_before": 236,
  "watermark_after": 236,
  "staging_windows": 0,
  "cleaned_refnos": ["24384/26201"],
  "verification": "watermark unchanged; deleted pe/noun/UDA/owner/model rows absent"
}
```

完整输出：
`D:\work\plant-code\old\test-worklspace\bin\db-window-repair-8000-236-hard-delete.log`。

## 纠正后查询

字面结果（全部 HTTP 200 / Surreal status `OK`）：

```text
RETURN dbnum_watermark:8000.applied_sesno;                 => 236
SELECT * FROM pe:24384_26201;                              => []
SELECT * FROM STRU:24384_26201;                            => []
SELECT * FROM ATT_UDA:24384_26201;                         => []
SELECT id,in,out FROM pe_owner WHERE in=pe:24384_26201;    => []
SELECT id,in,out FROM inst_relate WHERE in=pe:24384_26201; => []
SELECT id,noun,deleted,sesno FROM pe:24384_26199;
=> [{ id: pe:24384_26199, noun: ZONE, deleted: false, sesno: 236 }]
```

## 质量门

```text
cargo check (repair CLI feature set)                         exit 0
cata_closure unit tests                                      21 passed
watch_scope unit tests                                        9 passed
db8000_two_delete_fixture                                     6 passed
db_session_fixture_selfcheck                                 15 passed
db8000_session_pairs                                         20 passed
pdms_record_boundary                                          3 passed
net_window_rejects_duplicate_terminal_operations              1 passed
net_caliber_warning_reports_membership_deleted                 1 passed
live_ams8000_ses236_membership_delete_matches_expected_net     1 passed
release db_window_repair build                               exit 0
release aios-database build                                  exit 0
git diff --check（本次主仓文件及外部依赖成员删除实现）          exit 0
sigmap verify-plan specs/012-*/plan.md                        exit 0
sigmap verify-ai-output 本证据文件                            exit 0
```

`sigmap scaffold db_window_repair` 因仓库索引未检测到统一文件命名约定而按设计拒绝
（exit 1），未写文件。`sigmap review-pr` 对当前共享工作树执行后报告 388 个改动文件、
88 项跨任务发现并以 exit 1 收口；其中包含既存工作流/数据库配置与其它任务修改，不能
作为本次局部补丁的通过结论。本次改动另以限定文件 `git diff --check`、编译、单测、
四项 CI 集成测试、真实 236 对拍和 live 纠正结果完成验证。

## 部署与重启

- 修改产物：
  `D:\work\plant-code\old\test-worklspace\bin\aios-database.exe`
- SHA-256：`896C2C1F688F2CB238C0B0EBACE912ED5A27B14AEA085AF373E0BD71CE5E63C6`
- 部署证据/原件/补丁/回滚：
  `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\db8000-delete-20260819-141340`
- 回滚脚本已在停服状态实际执行，原件 SHA-256
  `6A89623E938872FC7ECBF9492FD3A85A7930D71BA4084AEEB82B4B3C71BAEFF7` 校验通过；
  随后重新部署修复版并复验上述修改版哈希。
- 最终重启 PID：`67640`；health：`status=ok`、`initialization=model_ready`、
  `worker_alive=true`、`staging_windows=[]`、空间树 `ready`且 `file_epoch=db_epoch=335`。
