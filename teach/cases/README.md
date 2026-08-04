# 增量更新案例集（Case Atlas）

> 把 `gen-model` 增量更新链路上**已经真实发生过**的 20 个案例，按 teach 工作区的口径逐个立卡：
> 现象 → 证据 → 根因 → 修法 → 验证 → 规律。汇总版（含示意图与端到端讲解）见
> [`../reference/increment-update.html`](../reference/increment-update.html)。

## 怎么读

每张案例卡固定七段，段名不变，方便横向比对：

| 段 | 写什么 | 纪律 |
|---|---|---|
| 一句话 | 这个案例到底是什么事 | 不超过两行 |
| 现象 | 用户/数据库侧看到的坏结果 | 只写观测到的，不写推测 |
| 证据 | `file:line`、报告、反编译地址、实测数字 | **无证据不写结论** |
| 根因 | 为什么会这样 | 落到具体机制，不写「没考虑到」 |
| 修法 | 改了什么 / 决策是什么 | 含「为什么不选另一条路」 |
| 验证 | 哪条测试、哪个实库样本证明修好了 | 未验证的明确写「未验证」 |
| 规律 | 这个案例留下的可迁移经验 | 下次遇到同类问题能用上的那句话 |

证据里的可信度分三层，沿用 [`../../docs/2026-07-24_test-core-dll-incremental-alignment-report.md`](../../docs/2026-07-24_test-core-dll-incremental-alignment-report.md) §2 的口径：
**A 内核行为**（`core.dll` 反编译）→ **B 离线实现**（Rust 单测）→ **C 端到端**（实库 + 三维截图）。
三层不能互相顶替：单测绿不等于实库通过，实库数据对不等于三维截图对。

## 总索引

| # | 案例 | 族 | 严重度 | 状态 | 证据层 |
|---:|---|---|---|---|---|
| 01 | [OWNER 变更是「搬迁」，不是属性修改](case-01-owner-change-is-a-move.md) | A 变化语义 | High | 已修 | A+B |
| 02 | [TRANSFORM_ONLY 名单过宽：7 条属性走了便宜路径](case-02-transform-only-was-too-wide.md) | A 变化语义 | High | 已收窄，缺实库对拍 | A+B |
| 03 | [五张分类名单合并为唯一 attr→effect 映射](case-03-attribute-effect-single-source.md) | A 变化语义 | Medium | 已修 | B |
| 04 | [生成根归一：主路径 / 兜底 / 补偿必须同一套口径](case-04-generation-root-must-be-one-rule.md) | A 变化语义 | High | 已修 | B+C |
| 05 | [改一个共享 SPCO，72 个消费者全部重生成](case-05-shared-spco-reverse-cascade.md) | B 反向级联 | High | 已修 | B+C |
| 06 | [建边资格与效果分类解耦：NGMR / ORRF / VXREF](case-06-ref-edge-eligibility-decoupled.md) | B 反向级联 | High | 已修 | B+C |
| 07 | [CascadeExpand 种子与死信复活：SET 子句顺序即功能](case-07-cascade-expand-and-dead-letter.md) | B 反向级联 | Low（极脆） | 已钉断言 | B |
| 08 | [删除元素后旧几何残留：软删墓碑被生成期过滤掉了](case-08-deleted-element-orphan-geometry.md) | C 删除清理 | Critical | 已修 | B+C |
| 09 | [级联清理的事务边界：中途失败后永久孤儿，还上报成功](case-09-cascade-delete-transaction.md) | C 删除清理 | Medium | 已修 | B |
| 10 | [TUBI 的 aabb / world_trans 从未落 inst_relate](case-10-tubi-inst-relate-missing.md) | C 删除清理 | High | 已修 | B+C |
| 11 | [水位三段式：prepare → PE 落库 → finalize](case-11-watermark-three-phase.md) | D 水位与重放 | High | 已修 | B+C |
| 12 | [drain 的失败隔离：一次删除抖动拖垮整轮队列](case-12-drain-failure-isolation.md) | D 水位与重放 | High | 已修 | B |
| 13 | [mesh 生成失败 panic 炸掉看门狗](case-13-mesh-panic-kills-the-watchdog.md) | D 水位与重放 | High | 已修 | B+C |
| 14 | [同窗口重放必须收敛：pe_owner 幂等 + SurrealQL 转义](case-14-replay-must-converge.md) | D 水位与重放 | Medium | 已修 | B+C |
| 15 | [自动 watcher 的文件身份守卫：重复 dbnum / 回退 / 路径迁移](case-15-file-identity-guard.md) | D 水位与重放 | Medium | 已修 | B+C |
| 16 | [WALL 的精确 CATA 闭包漏了 GMSS，几何数为 0](case-16-wall-cata-closure-missed-gmss.md) | E 解析与按需 | High | 已修 | B+C |
| 17 | [结构专业三连：FLOOR 隐含子元素、GENSEC 圆角与端面](case-17-structural-on-demand-trio.md) | E 解析与按需 | High | 已修 | B+C |
| 18 | [跨块显式属性 CURD/DBLS 丢失，模型树整棵为空](case-18-cross-block-explicit-attrs.md) | E 解析与按需 | Critical | 已修 | B+C |
| 19 | [窗口折叠：last-writer-wins，以及它只在哪种窗口有效](case-19-window-folding.md) | F 性能 | — | 已实施 | B+C |
| 20 | [批量化三连，与 debug/release 的 93 倍测量陷阱](case-20-batching-and-the-measurement-trap.md) | F 性能 | — | 已实施 | B+C |

## 按「你正在排查什么」检索

| 症状 | 先看 |
|---|---|
| 改了东西，三维没变 | 02（便宜路径少算）、05 / 06（反向级联没建边）、16（目录闭包缺子树） |
| 删了东西，三维还在 | 08（清理集不含软删）、09（清理中途失败） |
| 搬走了，旧位置还留着 | 01（旧 owner 侧没进变更集） |
| 某个 dbnum 反复失败、水位不动 | 14（重放不收敛）、12（整轮被拖垮）、07（死信复活失效） |
| 看门狗悄无声息地停了 | 13（panic 向上 unwind） |
| 模型树是空的 | 18（跨块属性丢失 → pe 关系断链） |
| 明明没改，却整批重算 | 03（`Unknown` 兜底过度）、06（建边过宽的反向） |
| 冷启动很慢 | 19（窗口折叠）、20（批量化 + 测量口径） |

## 与仓库文档的对应

案例卡是**索引与提炼**，不取代原始记录。深挖时的一手材料：

- 决策：[`../../docs/adr/`](../../docs/adr/)（ADR-001 水位 / 003 反向索引 / 006 跨块收集 / 008 目录反向传播 / 009 搬迁语义）
- 规格与任务：[`../../docs/specs/incr-gen-fixes/`](../../docs/specs/incr-gen-fixes/)（F1–F9 的需求、方案、验收）
- 审核报告：[`../../docs/2026-07-26_increment-update-chain-audit-report.md`](../../docs/2026-07-26_increment-update-chain-audit-report.md)（A1–A3）、
  [`round2`](../../docs/2026-07-26_increment-update-chain-audit-round2.md)（B1–B6）
- 测试矩阵：[`../../docs/2026-07-25_test-plan-core-dll-model-update-complete-matrix-v2.md`](../../docs/2026-07-25_test-plan-core-dll-model-update-complete-matrix-v2.md)（31 字段 / 6 变化桶 / 25 等价类 / 批次 A–D）
- 术语：[`../../CONTEXT.md`](../../CONTEXT.md)（生成根、最小交付单元、净变化、搬迁、应用水位……）
- 内核侧背景：[`../learning-records/0002-core-dll-model-update-logic.md`](../learning-records/0002-core-dll-model-update-logic.md)、
  [`../lessons/0002-ref-rev-reverse-reference-index.html`](../lessons/0002-ref-rev-reverse-reference-index.html)
