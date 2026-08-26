# 024 分块解析基线任务

勾选口径：`[x]` = 在工作树里指得出对应代码或证据，`[ ]` = 指不出。

## 阶段 0 —— 前置实测（不写实现代码）

- [ ] T01 给全量基线的五段（解析 / 生成 / 写回 / 空间树 / 房间）加**峰值 RSS 采样**，
      与 023 的分段壁钟共用同一批埋点；段内定时采样取最大值，不要只在段首尾各采一次。
- [ ] T02 同一个库跑两档：`sync_chunk_size` 默认（100000）与小值（如 5000），除该项外
      不动任何变量。两档都落 `docs/evidence/2026-08-22-bounded-baseline-parsing/`。
- [ ] T03 判读并留证：解析段峰值随小 chunk 应声而降 → 大头在属性层，**关掉阶段 1–3，
      只改默认值 + 补配置项**；峰值不动 → 大头在 `DbBasicData`，继续。
      结论必须写进 `plan.md` 的 R1 下面。

## 阶段 1 —— 遍历式分批与 mem 批容器

- [ ] T04 `src/options.rs` 新增 `baseline_shard_noun_types`，默认 `["ZONE"]`，照
      `generation_root.rs` 的 `delivery_unit_types` 形状（整体替换 + `append_` 追加 +
      非法值校验），补解析回归。
- [ ] T05 批上限配置：复用 `ResourceGauge` 的字节 + 行数三档，mem 与 journal 两份分开计量；
      补「达到上限即封批」的纯函数测试。
- [ ] T06 遍历式分批器：从 WORL 沿 children BFS，走到配置类型节点时整棵子树连着走完并
      优先封批，实际封口由上限触发。`visited` 保证不重不漏，补该性质的回归。
- [ ] T07 每批一个 `mem://` 实例 + 读路由（`connect` → `use_ns/use_db` →
      `init_staging_schema` → `StagingReadContext` → `with_staging_reads`），
      **不登记 `REGISTRY`、不建 commit token、不走 finalize plan**。
- [ ] T08 整批写回：`StagedExecutor` journal + `replay_journal_chunked`，确认后 DROP 该
      mem 实例。
- [ ] T09 源码形状门（plan.md R5）：基线解析路径内不得直接调用持久层写入入口——照 023
      `src/fast_model/concurrency.rs` 的 `mod shape` 做，新增直写点必须进豁免表并写明理由。
- [ ] T10 批内按 `(pgno, offset)` 排序后再读记录（plan.md R3），并把
      `PagedDbSession` 的 `physical_pages_read` / `cache_hits` / `prefetched_pages`
      打进每批日志。
- [ ] T11 [P] 逐表等价回归：`pe`、noun 属性、`ATT_UDA`、`pe_owner`、`dbnum_info` 全表
      digest 与改动前一致。
- [ ] T12 [P] 分块类型分别配 `["ZONE"]` / `["SITE"]` / `["EQUI"]` / `[]` 各跑一次，
      `pe_count` 完全一致（FR-2：配置只影响批的形状，不影响结果）。

## 阶段 2 —— 索引对账取代整文件解析

- [ ] T13 走 B-tree 索引树取全表 refno 清单（只读索引页，不读数据记录），与遍历结果对账。
- [ ] T14 差集按元素自带 owner 补回，再跑 `member_prune::prune_resurrected_members`；
      `authoritative_members` 的权威口径不变，取数改走
      `OnDemandDbSession::parse_element(refno).children`。
- [ ] T15 基线路径摘掉 `read_full_basic_data`，改走 `OnDemandDbSession`。
      **做完这一步 NFR-1 才成立**，之前都不算。
- [ ] T16 断链回归：注入一条「记录不带成员块」的元素，索引对账必须发现并补回。
- [ ] T17 issue #10 的现有回归用例保持不变且继续通过（`member_prune` 那一组）。
- [ ] T18 `AIOS_PDMS_ON_DEMAND_READ_MODE=compare` 跑一遍全量基线，legacy 与 paged 对拍无
      差异。

## 阶段 3 —— 幽灵收口与失败语义

- [ ] T19 收口前把 `dropped_elements` 从库里删掉；删除未完成即不推进 `applied_sesno`、
      不转 published。**独立钉一道回归**，不复用完整性闸当保险丝。
- [ ] T20 [P] issue #10 形状注入：已删子树在收口后库里查不到。
- [ ] T21 [P] 中途 kill 进程：`applied_sesno` 未推进；重跑清库后结果与一次跑完等价。
- [ ] T22 完整性闸 `pe_count - root_count == parsed_count` 保持硬闸，补
      「分片重复 → 闸红」「收口删除失败 → 闸红」两条。
- [ ] T23 每批可观测输出：批序号、封批原因、元素数、mem 字节、journal 字节、写回块数、
      峰值 RSS；收口输出 `dropped_elements` 与索引对账差额。
- [ ] T24 峰值 RSS 随批上限单调变化；上限调到极小时仍能跑完。
- [ ] T25 [P] 更新 `changelog.md` 与 `docs/2026-08-12_live-test-ledger.md`。
- [ ] T26 `cargo fmt`、`cargo check --tests`、相关 feature 单测与 isolated live 测试。
- [ ] T27 `sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`，结果留证。

---

`[P]` 仅表示文件所有权互不重叠时可并行。

硬顺序：**T03 必须先于阶段 1**（结论可能是整个阶段 1–3 都不做）；**阶段 1 必须先于阶段 2**
（先证明分批骨架逐表等价，再动 issue #10 那块死角）；**T15 之前 NFR-1 不成立**，不要在
阶段 1 结束时宣称内存有界。
