# d9517052 后两条全量红收口验证

日期：2026-08-24  
工作树：D:\work\plant-code\old\gen-model  
依赖：git sources，aios-core 9f1bf0f，local deps OFF。

## 修改对象

- D:\work\plant-code\old\gen-model\src\fast_model\manifold_tessellate.rs
  - SSCL 改为消费 vendor SCylinder::folded_shear_angles()；角度出界诊断先于 bool check_valid()。
- D:\work\plant-code\old\gen-model\src\fast_model\cata_model.rs
  - 顺序分块与 optimized fan-out 块宽均由 geometry_gate().chunk_size() 推导。
- D:\work\plant-code\old\gen-model\src\fast_model\concurrency.rs
  - T09 守护新增 let batch_size = if ... 固定 fan-out 块宽检测。
- D:\work\plant-code\old\gen-model\specs\009-retire-occ\tasks.md
- D:\work\plant-code\old\gen-model\changelog.md

原始与修改后逻辑内容 SHA-256：hashes.json。

## 基线回滚与失败复现

回滚命令：

`powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\evidence\2026-08-24-dependency-reds-closure\Rollback-Or-Reapply.ps1
`

输出与状态：

`	ext
OK mode=rollback files=5
exit 0
`

SSCL 基线命令：

`powershell
cargo test --locked --lib --no-default-features --features ws,gen_model,manifold,project_hd an_unfoldable_shear_angle_is_rejected_not_bent -- --nocapture
`

字面结果与状态：

`	ext
错误要说清是折叠后出界: PrimSCylinder is degenerate
test result: FAILED. 0 passed; 1 failed; 1189 filtered out
exit 101
`

fan-out 基线命令：

`powershell
cargo test --locked --lib --no-default-features --features ws,gen_model,manifold,project_hd no_hardcoded_fanout_width_survives_in_fast_model -- --nocapture
`

字面结果与状态：

`	ext
cata_model.rs:879: 写死的分块数：let mut batch_chunks_cnt = 4;。宽度用 geometry_gate().chunk_size()
test result: FAILED. 0 passed; 1 failed; 1189 filtered out
exit 101
`

## 重放与修改后验证

重放命令：

`powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\evidence\2026-08-24-dependency-reds-closure\Rollback-Or-Reapply.ps1 -Reapply
`

输出与状态：

`	ext
OK mode=reapply files=5
exit 0
`

两条定点命令的字面结果：

`	ext
cargo test: 1 passed, 1189 filtered out (1 suite, 0.00s)
exit 0
cargo test: 1 passed, 1189 filtered out (1 suite, 0.01s)
exit 0
`

全量命令：

`powershell
cargo test --locked --lib --no-default-features --features ws,gen_model,manifold,project_hd
`

字面结果与状态：

`	ext
cargo test: 1104 passed, 86 ignored (1 suite, 6.48s)
exit 0
`

质量门：

`	ext
cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd
cargo check: 0 errors, 4 pre-existing warnings (154 crates)
exit 0

git diff --check
exit 0

sigmap verify-plan specs/023-parallel-root-generation-pipeline/plan.md
plan checks out

sigmap verify-ai-output <summary>
no hallucinations detected
`

sigmap review-pr 按整个历史脏工作树统计 570 个 changed files，报 113 项全局噪声；本次聚焦补丁由 ocused.patch 独立界定，并已完成 reverse-check、实际回滚、实际重放和哈希校验。

## 提交暂存快照复验

为排除工作树其它未暂存改动，基于 `HEAD e696105c` 建立独立 worktree，仅应用暂存补丁：

```text
cargo check: 0 errors, 4 pre-existing warnings
cargo test: 1102 passed, 85 ignored (1 suite, 3.43s)
exit 0
```

该快照另包含 `cata_closure` 源码顺序守卫随 `scan_identity_ref0s` 改名更新；不带它时独立快照会在旧字符串的 `unwrap()` 处失败，定点复跑 exit 101。
