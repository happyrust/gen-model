# Implementation Plan：模型实例保存有界合批

## Constitution Check

- **水位承诺**：保存失败继续沿既有错误链阻断提交；不新增水位写路径。
- **单一规则**：定向与全量 receiver 共用 `run_shape_save_receiver`，没有第二套合批判定。
- **响亮失败**：NaN、ID 冲突、normal/tubi 重叠和渲染失败均在删除前返回错误。
- **队列收口**：保留有界 flume、单 consumer 与现有生成任务错误传播。
- **标识真值**：元数据继续通过 `resolve_inst_meta` 读取，不用 Ref0 近似 dbnum。
- **可执行守护**：纯计划、coalescer 边界、staged 终态和性能门分别由测试覆盖。

无宪法例外，Complexity Tracking 为空。

## Design

1. `src/fast_model/shape_save.rs` 定义 `SaveMode`、`FlushReason`、`FrozenShapeBatch`、
   `SavePlan`、`SaveOutcome`、`SaveConflict` 与 receiver/coalescer。
2. `src/fast_model/pdms_inst.rs` 将旧保存器拆成 `build_save_plan` 与
   `execute_save_plan`；计划持有按阶段和 record ID 排序的 SQL packet。
3. `src/fast_model/gen_model.rs` 的定向与整库 receiver 改用统一入口；成功 outcome 才更新
   produced，保存失败则 `finish_shape_writer` 原样上抛。
4. `src/data_interface/staging/write_context.rs` 的现有上下文判定用于选择串行 staged 执行；
   直写使用最大四并发的有界调度。
5. 单元测试覆盖确定性、冲突和合批边界；mem/staged 测试覆盖终态与 journal 幂等；固定夹具
   钉住 70% 请求下降。

## Verification

- CI 特性口径的目标库单测与 staging 集成测试。
- `cargo fmt`、`cargo check`，不执行 `cargo clean`。
- `D:\work\plant-code\old\test-worklspace` 同一 16 根旧/新 A/B，记录终态 diff、journal、请求、
  空间树 checksum、重启恢复和五轮中位数。
- 更新 `docs/2026-08-12_live-test-ledger.md` 与 `docs/evidence/`。
- `sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`。
