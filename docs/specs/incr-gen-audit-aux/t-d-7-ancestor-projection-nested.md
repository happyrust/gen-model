# T-D-7 盘点：祖先投影不含嵌套记录时，整片删会清掉嵌套根的行

日期：2026-10-28
结论：现状**符合 R-D-20**（嵌套根行能活下来当且仅当本轮投影把它们重新插入）。T-D-7 这根钉现在写是**绿**的——它钉住的是一条既有的危险依赖，不是在修缺陷。
复核方式：`rg` + 定向 `Read`，未跑 `cargo`。行号按 2026-10-28 工作树（`refactor` 分支，含未提交改动）。

## 一、祖先投影的实现位置

- `ModelMemoryStore` 的祖先索引 `by_ancestor: BTreeMap<i64, BTreeSet<String>>`（`src/fast_model/model_memory_store.rs:332`）：打包祖先 → 挂在它下面的 record_id，就是旧 `anc CONTAINS <packed>` 的内存形态（`:331` 的文档）。
- `record_ids_under(root)`（`model_memory_store.rs:593-597`）：取 `by_ancestor[packed_refno(root)]`。
- `ModelMemoryStore::records_of_root(scope, root)`（`model_memory_store.rs:1555-1573`）：根名下的全部记录，即祖先链含该根的所有记录。嵌套生成根名下的记录祖先链里也有祖先根，所以也在里面。这就是「祖先投影」。

## 二、整片删怎么用它（单根落库链）

```
persist_root_on(target, store, root)                        src/fast_model/model_db_adapter.rs:411-417
  └─ persist_root_impl                                       :2143
       ├─ records = store.records_of_root(Current, root)     :2150   ← 祖先投影
       ├─ 回执校验：geometry_count == records.len()；记录 source_sesno ≤ 回执、祖先链含根   :2152-2172
       ├─ 有回执 → sliced_roots = [packed(root)]；无回执 → 空  :2186-2190
       ├─ prelude += render_slice_delete(&sliced_roots)      :2248
       │     anc CONTAINSANY [root] AND model_sesno != NONE（模型面）+ 同根带几何指针的老行   :888-895
       └─ WriteMode::Initial 插 records（INSERT IGNORE）      :2235-2242
```

- 整片删按**祖先**圈行（`:889`），嵌套根名下的行也在圈内；插回去的只有祖先投影里有的记录。
- 所以祖先投影里**没有**嵌套根的记录时，嵌套根名下的模型面行会被删、且不会插回，嵌套行计数归零。被删的包括 `inst_relate`，以及同键的 `inst_info` / `aabb` / `trans` 与 `geo_relate`（`:896-903`）。
- 无回执时不整片删（`:2179-2190`），走 `WriteMode::Incremental`，只按本批 id 删；缺席不当删除。这一侧已经有钉（见第三节）。
- 组落库 `persist_group`（`:1882`）在 `:2001` 同样是 `render_slice_delete(&sliced_roots)`，口径相同；本钉走单根路径就够了。

## 三、现有相关测试（都在 `src/fast_model/model_db_adapter.rs` 的 `mod tests`）

| 测试 | 行 | 钉的是什么 |
|---|---|---|
| `reconciling_an_ancestor_root_does_not_delete_a_nested_roots_rows` | `:9023-9074` | 正方向。祖先投影**含**嵌套记录时：落祖先根不删嵌套根的行；落嵌套根不删祖先根别处的行；真孤儿照删。文档（`:9020-9022`）写明：今天不出事靠的是「祖先链是全库的」和「生成是整棵子树的」这两条事实 |
| `an_unreceipted_partial_projection_does_not_delete_nested_root_rows` | `:4957-4991` | 无回执的残缺投影：缺席不当删除，`removed == 0`，行数保持 2 |
| `reconciliation_requires_a_matching_receipt_and_accepts_a_complete_empty_projection` | `:4994-5038` | 回执件数对不上就拒绝；完整的空投影整片删到 0（`:5036-5037`：`removed == 2`、计数 0，含嵌套那行） |
| `a_record_may_lag_its_root_receipt_but_never_lead_it` | `:5047-` | 回执口径是 `<=` |
| `batched_persist_shares_chunks_across_roots_and_stamps_every_receipt` | `:5371-` | 组落库：嵌套根与祖先根各自带完整记录 |
| `bound_rows_land_exactly_what_the_rendered_text_lands` | `:3823-` | 只用这个夹具核渲染 / 绑定同形，不涉及整片删语义 |
| `live_testbed_complete_projection_reclaims_only_the_absent_primitive` | `:9080-`（`#[ignore]`，手工 RocksDB testbed） | 完整投影只回收缺席的图元 |

缺的正是「**有回执**、但祖先投影**不含**嵌套记录」这一格，也就是 R-D-20 里「仅当」那个方向。

## 四、T-D-7 钉：放哪、断言什么、用什么夹具

- **位置**：`src/fast_model/model_db_adapter.rs` 的 `mod tests`，紧跟 `reconciling_an_ancestor_root_does_not_delete_a_nested_roots_rows`（`:9074` 之后），与它成对。
- **名字建议**：`an_ancestor_projection_without_the_nested_records_slices_the_nested_root_away`。
- **夹具**（都是现成的）：
  - `surreal_mem()`（`:9249`）；
  - `seed_pe(&db)`（`:9213`，已建 `pe:⟨24383_600⟩`）；
  - `ModelMemoryStore::default()`、`ProjectionCommit`、`ProjectionScope::Current`；
  - `test_receipt(root, n)`（`:3684`）；
  - `baked_record()`（`:3665`，祖先链 `[BRANCH, 24383_1]`）；
  - `nested_root_record()`（`:3772`，祖先链 `[24383_600, NESTED_ROOT = 24383_500, 24383_1]`）；
  - `PersistTarget::new(&db, PersistMedium::Mem)`、`persist_root_on`、`count`（`:9260`）。
- **断言**：第一次落祖先根（投影含嵌套记录）后，嵌套行为 1。第二次故意不放 `nested_root_record`，回执件数跟着改成 1，再落祖先根：
  - 嵌套行计数归零；
  - `inst_relate` 只剩 1 行；
  - 同键的 `inst_info` / `aabb` / `trans:⟨24383_600⟩` 也没了。

代码形态（未编译；`rows_under` 照 `count`（`:9260`）的写法写一个带 `WHERE anc CONTAINS` 的版本）：

```rust
/// R-D-20 的「仅当」方向：整片删按祖先圈行，祖先投影没带上的嵌套根行不会插回。
/// 和上一条成对：哪天整片删改成跳过嵌套根，或生成改成祖先根跳过嵌套根，两条会一红一绿。
#[tokio::test]
async fn an_ancestor_projection_without_the_nested_records_slices_the_nested_root_away() {
    let db = surreal_mem().await;
    seed_pe(&db).await;
    let ancestor_root = RefNo::new(24383, 1);
    let target = PersistTarget::new(&db, PersistMedium::Mem);

    let store = ModelMemoryStore::default();
    store.commit(
        ProjectionScope::Current,
        vec![ProjectionCommit {
            upserts: vec![baked_record(), nested_root_record()],
            receipt: Some(test_receipt(ancestor_root, 2)),
            ..ProjectionCommit::default()
        }],
    );
    persist_root_on(target, &store, ancestor_root).await.expect("persist ancestor");
    assert_eq!(count(&db, "inst_relate").await, 2);
    assert_eq!(rows_under(&db, NESTED_ROOT).await, 1);

    // 故意不放 nested_root_record：这一轮祖先根的投影里没有嵌套根的记录，回执件数跟着是 1。
    store.commit(
        ProjectionScope::Current,
        vec![ProjectionCommit {
            removals: vec![nested_root_record().record_id],
            receipt: Some(test_receipt(ancestor_root, 1)),
            ..ProjectionCommit::default()
        }],
    );
    persist_root_on(target, &store, ancestor_root).await.expect("persist ancestor");
    assert_eq!(rows_under(&db, NESTED_ROOT).await, 0, "嵌套行被整片删圈走、没有插回");
    assert_eq!(count(&db, "inst_relate").await, 1);
    // 同键槽位一起走（render_slice_delete :896-903）：inst_info / aabb / trans:⟨24383_600⟩ 均已不在。
}
```

也可以不用 `removals`，换一个只含 `baked_record()` 与 `test_receipt(ancestor_root, 1)` 的新 store 去落第二次，库上效果一样。

## 五、判定：现在写是绿还是红

**绿**。依据：
- `persist_root_impl` 只要有回执就 `sliced_roots = [packed(root)]`（`:2186-2190`）。
- `render_slice_delete` 按 `anc CONTAINSANY [root] AND model_sesno != NONE` 圈行（`:888-889`）。嵌套根那行的 `anc` 含 `24383_1`（夹具 `:3779-3783`），所以被圈中删掉。
- `WriteMode::Initial` 只插 `records_of_root` 的记录（`:2150`、`:2235-2242`）。
- 同一机制已经在 `reconciliation_requires_a_matching_receipt_and_accepts_a_complete_empty_projection` 里让「完整的空投影」把两行（含嵌套那行）删到 0（`:5036-5037`）。

这说明现状没有「整片删多删」的缺陷，行为与 R-D-20 一致。这根钉的价值是把「嵌套根行能不能活，取决于祖先投影有没有带上它们」写成可见的契约。哪天有人把整片删谓词改成跳过嵌套根，或把生成改成「祖先根跳过嵌套根」（core R10 的做法，见 `:4951-4953` 的文档与 `docs/specs/core3d-partial-update-conformance.md` §8），这条与 `:9023` 那条会一红一绿，指出口径变了。

## 六、假设与未确认

- 未跑 `cargo`；「绿」是按代码读出来的结论，不是运行结果。
- 用 `removals` 提交后，`records_of_root` 不再含该记录、回执替换成 `test_receipt(A, 1)`：这是按 `:9053-9065` 的既有用法推断的（同样的提交形态在那里让 `rows.removed == 1`）。
- `PersistedRows.removed` 在本场景下的取值未核：它的计数口径跟 `removals` 有关，而不是跟整片删删了多少行。钉里建议不断言它，只断言库上的行数。
- 嵌套行计数可以用 `anc CONTAINS <packed_refno(NESTED_ROOT)>` 查，也可以直接查 `inst_relate:⟨24383_600⟩`。`packed_refno` 在测试模块里可用（`:3781` 已经在用）。
- 任务列的候选文件（`dbnum_state.rs` / `task_end.rs` / `direct_store.rs` / `fast_delete.rs`）里都没有祖先投影或整片删的实现：
  - `rg "render_slice_delete|anc CONTAINSANY" src` 只在 `fast_model/model_db_adapter.rs` 命中整片删；
  - `increment_pipeline.rs:1410` 是搬家时改 `anc` 的 UPDATE；
  - `fast_delete.rs` 是整库 wipe。
- 祖先投影在 `fast_model/model_memory_store.rs`。
