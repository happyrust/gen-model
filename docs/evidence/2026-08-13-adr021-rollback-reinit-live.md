# ADR-021 回退默认整库重建：live 验证留痕

日期：2026-08-13  
环境：pytest 沙箱 SurrealDB @8019（`bin/surreal.exe`，fork 2.1.4，`rocksdb:python/testbed/.surreal/pytest-ams`），配置 `DB_OPTION_FILE=python/testbed/DbOption-pytest`，debug 构建（`--features http_api`）。  
关联：ADR-021、specs/002-watermark-data-backing、`docs/2026-08-12_live-test-ledger.md` 对应两行。

## 跑了什么

### 1. `live_rollback_wipe_clears_the_dbnum_for_reinit`（tests 模块）

**通过，4.74s**。魔术 dbnum 999_999_021 + 保留段 ref0 自建夹具（幸存行 sesno=40、
幽灵行 sesno=60 挂 inst_relate 派生、pending 高低各一、attempt 一条、info 统计一行、
水位 applied=60 + 登记身份），对「文件回退到 50」执行 `wipe_dbnum_for_reinit`：

- pe 全删（幸存行也不留，`report.pe_rows == 2`），派生行 / noun 行 / attempt / pending / info 统计全部出清；
- `dbnum_watermark` 行清值不删行：`applied_sesno == 0`，`file_name` / `db_type` 登记身份原地不动；
- spatial epoch 在同一元数据阶段递增（前后两读严格递增）。

### 2. `live_rollback_and_ghost_watermark_reinit_end_to_end`（live_tests 模块）

**通过，22.31s（两幕）**。靶库 7998（testbed 最小设计库），
`AIOS_MANUAL_UPDATE_PROJECT=AvevaMarineSample`，`RUST_MIN_STACK=16777216`
（debug 构建执行链会栈溢出，与 testbed 既有惯例一致）。

- **幕一（回退）**：把水位抬到 `file_latest + 7` → `enqueue_manual_update` 回执
  不 blocked、warnings 点名「已按整库重建入队」，**入队阶段库里数据原样未动**；
  worker 消费后任务 warnings 带「回退重建：已整库清空」、batch.message 为
  「首次按需初始化完成」，水位对齐回文件水位、库里有数据。
- **幕二（幽灵水位）**：删光该库 pe 行、水位压到 `file_latest - 2`（有增量窗口可走
  的形状）→ 批次路由到**基线**而不是增量窗口，warnings 带「水位与数据不一致」，
  水位对齐、数据重建。

## 首跑抓出的真缺陷（已修）

幕二首跑 **failed**：批次按入队形状（`start_sesno > 1`）先开了 ADR-017 kv-mem
暂存窗口，执行体检出幽灵水位改道基线（基线不走窗口协议），窗口等不来
finalize plan，批次以「暂存窗口缺少 finalize plan，模型前置未执行」失败收场——
入队时的观察窗口与冻结点的权威路由各判各的。

修复：`batch_worker` 开窗前增加冻结点预判 `batch_reroutes_to_initial_load`
（权威 applied 为 0 / 回退 `file_latest < applied` / 幽灵水位 `applied > 0` 且
pe 零行，三种形状一律不开窗、走直写执行体），数据支撑探针与执行体共用同一个
`dbnum_has_any_pe_row`。预判读失败按入队形状开窗，由执行体复核给响亮终态。

## 同批配套

- 纯函数/源码钉（CI 口径 `--no-default-features --features ws,gen_model,manifold,project_hd`）：
  全量 lib 683 passed / 0 failed（含新增真值表、`ScanGate` 逐类映射与四条源码钉）。
- Python 闭环（同日补，`python/tests/test_rollback_reinit.py`，@8071 一次性内存库，
  走 `incr.execute_manual` 子集 + 模块级 SYS 引导）：回退整库重建（幸存位/幽灵位
  标记行全部物理消失）、幽灵水位路由到基线（行数回到完整基线）、类型变更照旧
  阻断三条，`pytest tests\test_rollback_reinit.py -q` 3 passed / 27.6s；全套
  `pytest -q` 80 passed / 36.5s（含离线档 62）。
- 现场背景（8009 上 8 库回退、7350/7353/7741 幽灵水位的处置记录）见
  `.scratch/realign-20260813-114321`。
