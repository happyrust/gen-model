# 2026-08-14 管道增量闭环续测

## 范围

- E3D 8000 DESI：FTUB 位移、恢复及会话窗口分类。
- 严格初始化：Catalogue 未提交基线残留恢复。
- 数据闭环：db8000 会话 225..=228 落库、水位、模型、AABB。
- Plant UI：按 `tree_item + refno` 定位 FTUB，核对属性并显示模型。
- GENSEC 新增：验证变更驱动在保存事实不明确时停止重放。

测试使用独立 SurrealDB/项目镜像与隔离 Plant UI 设置；既有 E3D 会话未被终止。

## 1. FTUB 变更与恢复

变更驱动命令同时传入目标 DB 文件和项目：

```text
l3_suite.exe --check-driver <macro> --target-db-file <ams8000_0001> \
  --aios-project AvevaMarineSample --project-dir <project-dir> --output <evidence-dir>
```

结果（exit 0）：

| 步骤 | 会话号 | 位置 U | 分类 |
|---|---:|---:|---|
| 基线 | 226 | 2900 mm | — |
| apply | 227 | 3400 mm | `completed` |
| restore | 228 | 2900 mm | `completed` |

目标身份始终为 `dbnum=8000 / DESI / WORLD=16192/0`；apply 和 restore 均观察到
ALIVE、DONE 与 exit 0。合并净窗口把 FTUB 往返识别为 unchanged rewrite，最终文件
已恢复。原始记录：

- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-classified-20260814-213543\summary.json`
- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-classified-20260814-213543\semantic-window-diff.json`

## 2. Catalogue 未提交基线恢复

首次严格初始化在 CATA 5052 发现 `PE=306957`、`applied_sesno=0`，其中 11 行是前次
中断留下的非 WORL 残留；旧的 `INSERT IGNORE` 重放会永久保留这些行，使完整性检查
反复失败。修复后在全量解析前清除水位 0 的未提交行，日志为：

```text
dbnum=5052 清理未提交基线残留 PE=306957，从水位 0 重新解析
```

任务 `db-20260814-220421-000000` 随后成功：解析/最终 PE/`dbnum_info` 均为
306945，应用水位为 189，exit 0。证据：

- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-baseline-recovery-20260814-221100\task-5052-final.json`
- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-baseline-recovery-20260814-221100\surreal-5052-final.json`

## 3. db8000 数据、模型与 Plant UI

在只含 db8000 的最小 Design 镜像中执行 `preview → execute`。任务
`db-20260814-223101-000000` 成功应用 225..=228：`changed_elements=8`，
`applied_sesno=228`，FTUB `24384/23262` 的 OWNER 为 `24384/23257`，POS 恢复为
`[10887,12332,2900]`，HEIG 为 480。这里保存发生在 preview 之前，所以
`merged_sesnos=[]` 符合既有契约。

BRAN `24384/23257` 的按需模型生成返回：

```json
{"status":"Generated","model_available":true,"model_instance_count":23,"generated_instance_count":9}
```

FTUB 空间范围为：

```text
min=[10867.896484375,11978.7822265625,2899.5]
max=[11251.38671875,12351.7822265625,2930.0]
```

Plant UI 使用隔离设置文件启动，AccessKit 以 refno 精确定位到 FTUB 4；属性面板显示
FTUB、`24384/23262`、owner `/C-OR-1R345-C`、HEIG 480；“显示模型”完成 1 个目标，
三维视口出现对应实体。证据目录：

- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-design-only-20260814-223100`
- 其中 `data-task-final.json`、`surreal-final.json`、`ensure-bran.json`、
  `ftub-spatial-bounds.json`、`accesskit-ftub.txt`、`plant-ui-ftub-model.png`。

## 4. GENSEC 新增阻断

GENSEC apply 在进入宏后发生 E3D access violation，DONE 未出现且会话号保持 228。
驱动结构化裁决为 `indeterminate`，只执行一次，不重放 apply，文件身份和内容未变化：

```text
alive_seen=true, done_seen=false, before_sesno=228, after_sesno=228
outcome=indeterminate, attempts=1
```

证据：`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-gensec-add-20260814-224000\check-driver-evidence.json`。

## 5. 剩余问题

1. GENSEC 新增宏仍被 E3D 运行时 access violation 阻断；当前分类和不重放规则已避免重复写入。
2. 历史 `model_update_pending.updated_at=NULL` 行在升序认领时位于新工作之前；本轮新
   `post_regen_aabb` 需要按需生成绕过积压。Plant UI 预览还会展示 1608 条历史死信，
   后续应给新数据关联工作提供公平调度并收敛预览噪声。

## 6. 本轮质量门

```text
cargo fmt --all -- --check                                      exit 0
cargo check                                                     exit 0
cargo test --locked --lib partial_baseline_is_rebuilt_before_advancing_watermark ...
                                                                1 passed, exit 0
cargo test --locked --bin l3_suite stateful_macro_detection_accepts_save_comments_and_rejects_lookalikes ...
                                                                1 passed, exit 0
cargo test --locked --bin l3_suite ...                           25 passed, exit 0
cargo test --locked --lib data_interface::manual_update::tests ...
                                                                100 passed / 1 ignored, exit 0
cargo build --locked --bin aios-database --no-default-features --features ws,gen_model,manifold,project_hd,http_api
                                                                exit 0
```

## 7. 提交后 F6 OWNER 搬移闭环

提交 `ec0d6279` 后继续执行独立的 F6 管道场景。FTUB `24384/22403` 在两个 BRAN
之间搬移并恢复，两个变更宏均由结构化驱动判为 `completed`：

| 步骤 | 会话号 | OWNER |
|---|---:|---|
| 基线 | 228 | `24384/22402` |
| apply | 229 | `24384/22404` |
| restore | 230 | `24384/22402` |

数据任务 `db-20260814-225755-000000` 成功应用 229..=230，`changed_elements=6`，
最终 `applied_sesno=file_latest_sesno=230`；`pe` 与 `pe_owner` 均指回
`24384/22402`。两个受影响 BRAN 的按需模型生成均成功：

```text
24384/22402: model_available=true, instances=4, generated=1
24384/22404: model_available=true, instances=177, generated=35
```

FTUB 的 `inst_relate` 与 AABB 指针存在，关联 pending 已收口。Plant UI 使用精确命令
`=24384_22403` 定位真实树行；属性面板显示 FTUB、refno `24384_22403`、owner
`/C-IY-1R330-A`（即 `24384/22402`），与数据库一致。

证据目录：

- `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\pipeline-ftub-owner-20260814-225426`
- 关键文件：`summary.json`、`data-task-230-final.json`、`surreal-final.json`、
  `ensure-roots.json`、`ui-ftub-properties.txt`、`plant-ui-ftub-owner-restored.png`。

本轮结束后已停止本轮启动的 Plant UI 与 aios-database；目标 E3D 文件保持恢复后的
业务状态，会话号为 230。
