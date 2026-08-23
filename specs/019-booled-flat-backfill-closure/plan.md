# Plan 019：布尔成品平表的存量收敛

**Date**: 2026-08-20  
**Spec**: `specs/019-booled-flat-backfill-closure/spec.md`

## Summary

三处改动收口 08-20 布尔显示修复留下的存量缺口：

1. `sweep_inst_relate_flat` 增加**修复段**——在既有 NONE 清扫循环之后，按批圈
   「`booled_id` 有值而 `insts_flat` 与之不符」的行改写为 `[{ geo_hash: booled_id }]`，
   幂等自收敛，随清扫既有的两个挂点（启动 + 脏位空闲轮）生效；
2. plant-ui 平表读 `query_insts_flat` 的 SQL 投影优先 `booled_id` 并回 `has_neg`，
   读侧不再原样信任可能过期的 `insts_flat`；
3. Manifold 布尔成功补写 `booled = true`，与 OCC 行形态对齐。

外加一次 AMS 实库收敛执行与证据留痕。08-20 那次只手修了单行
（`docs/evidence/2026-08-20-rm13-dome-live/verification-record.md`），本 plan 把
同一条已在 2.x 上验证过的条件表达式变成代码里的系统性修复。

## Technical Context

- **Language/Version**: Rust edition 2024，nightly-2026-08-02
- **Primary Dependencies**: fork SurrealDB 2.1.4、aios-core、glam、parry3d
- **Storage**: SurrealDB 无模式关系表；修复段走持久层非 journal 路径（与清扫同族）
- **Testing**: 源码形状断言 + `fork_surreal_compat` 双引擎 `mem://`/RocksDB 测试 + AMS 8009 live 复测
- **Target Platform**: Windows / PowerShell，生产兼容 Linux release
- **Constraints**: 不得 `cargo clean`；不得覆盖当前工作树内既有未提交改动（016/017/018 在飞）

## Constitution Check

- **I 水位承诺**：不碰水位。修复段是派生缓存物化，非 journal 路径，不进暂存窗口；
  失败上抛，空闲轮脏位机制天然重试（`sweep_inst_relate_flat_if_dirty` 失败放回脏位）。
- **II 单一规则**：「有 `booled_id` 只显示单位变换成品」这一规则的 SQL 措辞收敛为
  同一形状，出现在清扫 IF 分支、修复段、平表读投影三处；用源码形状断言钉住三处
  一致（仓库既有先例：`empty_difference_is_bad_bool_not_a_silent_swallow`）。
- **III 静默失效**：本特性正是消灭一处静默失效——平表缓存端着错误正体给读者，
  没人报错。`'none'`/空串脏值不静默改写，计数可见（FR-006）。
- **IV 队列收口**：不新增队列 action，不新增触发机制（FR-002）。
- **V 标识真值**：`booled_id` 即成品真值；空串/`'none'` 当缺失处理，不猜、不修成
  「看着像真的」的值。
- **VI 可执行守护**：`fork_surreal_compat` 补「脏行→收敛→二轮零行→正体行不动」
  双引擎用例；形状断言钉修复谓词与读投影；live 复测按 SC-001~004 留痕。

结论：通过，无需 Complexity Tracking 例外。

## Referenced Decisions

- `docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`：P4 写时物化与
  平表读两段式（三分法兜底）的来历；本 plan 修订其「行只会缺不会错」的假设。
- `docs/evidence/2026-08-20-rm13-dome-live/verification-record.md`：事故记录、单行手修
  所用的同型条件表达式（已在 SurrealDB 2.x 实测 exit 0）、回滚脚本。
- `docs/adr/ADR-017-staged-increment-window-commit.md`：非 journal 路径边界——修复段
  与清扫同族，不进窗口。
- gen-model `89f8b06b` / plant-ui `dbb348e25`：本 plan 收口的两个前置提交。

## Project Structure

```text
specs/019-booled-flat-backfill-closure/
├── specs/019-booled-flat-backfill-closure/spec.md
├── specs/019-booled-flat-backfill-closure/plan.md
└── specs/019-booled-flat-backfill-closure/tasks.md

src/fast_model/pdms_inst.rs            # 修复段 + 文档注释修订 + 形状断言
src/fast_model/manifold_bool.rs        # booled=true + 更新形状回归
src/test/fork_surreal_compat.rs        # 双引擎收敛用例
../plant-ui/vendor/rs-core/src/rs_surreal/inst.rs   # 平表读投影 + has_neg + 单测
docs/evidence/2026-08-XX-booled-flat-backfill/       # live 收敛证据
changelog.md、../plant-ui/CHANGELOG.md
```

## Implementation

1. **修复段**（`pdms_inst.rs`）：`sweep_inst_relate_flat` 在 NONE 循环收敛后追加第二个
   批量循环（沿用 `BATCH = 500` 与 `RETURN array::len($rows)` 收敛判断）：

   ```text
   LET $rows = SELECT VALUE id FROM inst_relate
     WHERE booled_id != NONE AND booled_id != '' AND string::lowercase(booled_id ?? '') != 'none'
       AND (insts_flat = NONE OR array::first(insts_flat ?? []).geo_hash != booled_id)
     LIMIT 500;
   UPDATE $rows SET insts_flat = [{ geo_hash: booled_id }] RETURN NONE;
   RETURN array::len($rows);
   ```

   **三引擎分叉（2026-08-20 实测定稿）**：生产 8009 服务器对函数的 NONE 实参
   直接报错（`string::lowercase(NONE)` / `array::first(NONE)`，AND/OR 不短路，
   另一臂照样求值），而 mem 与 fork 2.1.4 二进制都容忍——函数实参一律
   `?? ''` / `?? []` 兜底，这是能同时过三个引擎的唯一写法。空串/`'none'` 行
   单独 `SELECT count()` 一次，非零就在完成日志里带出来。顺手修订函数注释里
   「行只会缺不会错」的断言（FR-009）。
2. **双引擎守护**（`fork_surreal_compat.rs`）：在既有清扫用例旁植入
   「booled_id 有值 + insts_flat 为带缩放正体」的行，跑修复段断言收敛、二轮零行、
   正体行不动、空串 `booled_id` 行不被改写。
3. **平表读对齐**（plant-ui `inst.rs`）：`query_insts_flat` 的 select 改为
   `IF booled_id != NONE THEN [{ geo_hash: booled_id }] ELSE insts_flat END AS insts_flat,
   booled_id != NONE AS has_neg`；`FlatInstRow` 增 `#[serde(default)] has_neg: bool`，
   `GeomInstQuery.has_neg` 透传不再写死 false。`display_insts` 的字面 `'none'` 过滤
   加注释说明防御对象（取证见任务 T008）。
4. **双引擎行形态**（`manifold_bool.rs`）：成功路径 update 语句补 `booled=true`；
   同步更新 `empty_difference_is_bad_bool_not_a_silent_swallow` 的形状断言。
5. **live 收敛**（8009）：先 `SELECT count()` 取 mismatch 基线，启动带修复段的服务
   （或直跑同型语句）收敛，复查计数为 0、二轮零行；抽查 `24381_36945` 保持正确、
   任一正体行逐字节不变；证据落 `docs/evidence/`，两仓 changelog 各记一条。
6. **顺序**：先落失败回归（步骤 2 的用例在无修复段时必须红），再实现；
   `rustfmt`、定向 `cargo test`、`cargo check` 全过后才进 live。
