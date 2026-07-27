use std::collections::HashSet;

use aios_core::{RefnoEnum, SUL_DB};
use anyhow::anyhow;
use surrealdb::sql::Thing;

/// 查询子树时的分批大小（与 increment_manager 的 QUERY_BATCH_SIZE 一致，避免 SQL 过长）。
const SUBTREE_QUERY_BATCH: usize = 20;

pub(crate) fn pe_thing_to_refno(value: Thing) -> anyhow::Result<RefnoEnum> {
    let raw = value.to_string();
    let refno = RefnoEnum::from(value);
    anyhow::ensure!(refno.is_valid(), "invalid PE record id: {raw}");
    Ok(refno)
}

pub(crate) async fn collect_pe_subtree_refnos(
    refnos: &[RefnoEnum],
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let mut all: HashSet<RefnoEnum> = refnos.iter().copied().collect();
    let mut frontier = refnos.to_vec();

    while !frontier.is_empty() {
        let mut next = Vec::new();
        for chunk in frontier.chunks(SUBTREE_QUERY_BATCH) {
            let pe_keys = chunk
                .iter()
                .map(|r| r.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "array::distinct(array::flatten(SELECT VALUE <-pe_owner.in FROM [{pe_keys}]));"
            );
            let mut response = SUL_DB.query(&sql).await?.check()?;
            for value in response.take::<Vec<Thing>>(0)? {
                let refno = pe_thing_to_refno(value)?;
                if all.insert(refno) {
                    next.push(refno);
                }
            }
        }
        frontier = next;
    }

    Ok(all)
}

/// 渲染单个 refno 的级联删除，三条语句共一个事务。
///
/// 事务不是为了跨 refno 的原子性（那反而会让一个坏 refno 拖垮整批），而是因为
/// 「删边」与「按引用计数回收 inst_info」之间**不能存在可观察的中间态**：清理条件
/// 读的正是刚被删掉的那条 `inst_relate`。若边已删而 `if` 块没跑（语句报错、连接
/// 中断、服务端重启），重试时 `$old_inst` 只会读到 `NONE`，整段清理被静默跳过，
/// 而函数照样返回 `Ok`——inst_info / geo_relate / inst_geo 就此永久孤儿，且无告警。
/// 包进事务后这种半执行会整体回滚，重试从干净状态开始，可自愈。
///
/// `inst_info` 本身用显式 `delete $old_inst` 回收，而不是靠 `geo_relate` 三元组的
/// `in` 端顺带删除：几何生成半途失败会留下**没有任何 `geo_relate` 边**的 `inst_info`，
/// 顺带删除对它是空集、永远删不掉（2026-07-26 审计 B2）。
fn render_cascade_delete(inst_relate_key: &str) -> String {
    format!(
        r#"BEGIN TRANSACTION;
let $old_inst = (select value out from {inst_relate_key})[0];
delete from {inst_relate_key};
if $old_inst != none and array::len($old_inst<-inst_relate) = 0 {{
    delete array::flatten(select value [out, id] from $old_inst->geo_relate);
    delete $old_inst;
}};
COMMIT TRANSACTION;"#
    )
}

/// 级联删除 inst_relate 及其关联的 geo_relate 和 inst_geo 数据
///
/// 当 replace_mesh 开启时，需要完全删除之前生成的数据，包括：
/// - inst_geo: 几何体节点
/// - geo_relate: 几何关系边
/// - inst_info: 实例信息节点
/// - inst_relate: 实例关系边
///
/// # 参数
/// * `refnos` - 需要删除的 refno 列表
/// * `chunk_size` - 分批处理的大小
///
/// # 删除顺序
/// 1. inst_relate（仅删除目标元素的关系）
/// 2. 若 inst_info 已无其他 inst_relate 引用，再删除其 inst_geo / geo_relate / inst_info
///
/// inst_info 可能由相同 catalogue hash 的多个元素共享，不能在仍有引用时删除。
/// 每个 refno 的这两步在一个事务里（见 [`render_cascade_delete`]），失败整体回滚。
pub async fn delete_inst_relate_cascade(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        chunk_size > 0,
        "delete chunk_size must be greater than zero"
    );

    for chunk in refnos.chunks(chunk_size) {
        let mut delete_sql_vec = vec![];

        for refno in chunk {
            delete_sql_vec.push(render_cascade_delete(&refno.to_inst_relate_key()));
        }
        if !delete_sql_vec.is_empty() {
            let sql = delete_sql_vec.join("\n");
            SUL_DB
                .query(&sql)
                .await
                .map_err(|e| anyhow!("delete model relations failed: {e}"))?
                .check()
                .map_err(|e| anyhow!("delete model relations statement failed: {e}"))?;
        }
    }

    Ok(())
}

/// 收集给定 refno 及其**子树（含已软删节点）**的全部节点，级联删除它们的
/// `inst_relate / geo_relate / inst_info` 几何数据（F1：删除元素几何孤儿清理）。
///
/// 背景：被删元素只做软删（`pe.deleted = true`），几何重生成时被 `!deleted` 过滤，
/// 因而**不会**进入 `save_instance_data(replace_exist)` 的删除集（那只删本次生成的键），
/// 其旧 `inst_relate` 等会成为孤儿。这里按 `pe_owner` 子树（**不**过滤 deleted）收集
/// self + 全部后代，再交给幂等的 [`delete_inst_relate_cascade`]（对无 inst_relate 的
/// refno 为 no-op），从而无论删除是「逐元素记录」还是「只记顶层」都能清干净。
///
/// 子树收集失败必须上抛，让 pending/补偿任务保留并重试；否则只删根自身会把子件旧
/// 模型永久遗留，同时错误地把任务标记为完成。
pub async fn delete_inst_relate_subtree(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    // 自身 + 全部后代（沿 pe_owner 向下），刻意不加 `!in.deleted`：
    // 我们要清理的正是已软删节点。
    let all = collect_pe_subtree_refnos(refnos)
        .await
        .map_err(|e| anyhow!("collect deleted PE subtree failed: {e}"))?;

    let all_vec: Vec<RefnoEnum> = all.into_iter().collect();
    delete_inst_relate_cascade(&all_vec, chunk_size).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_delete_keeps_the_edge_delete_and_refcount_gc_in_one_transaction() {
        let sql = render_cascade_delete("inst_relate:7997_1");

        assert!(sql.starts_with("BEGIN TRANSACTION;"), "{sql}");
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");
        // The GC condition reads the edge this block just deleted, so a commit
        // boundary between the two would strand inst_info on retry.
        let delete_at = sql
            .find("delete from inst_relate:7997_1")
            .expect("edge delete");
        let gc_at = sql
            .find("array::len($old_inst<-inst_relate)")
            .expect("gc guard");
        assert!(delete_at < gc_at, "{sql}");
        assert!(
            sql.find("let $old_inst").expect("binding") < delete_at,
            "{sql}"
        );
    }

    /// B2（2026-07-26 审计 round2）：`inst_info` 必须被显式删除。若只靠
    /// `geo_relate` 三元组的 `in` 端顺带删除，一个没有任何 `geo_relate` 边的
    /// `inst_info`（几何生成半途失败的残留）将永远不被回收。
    #[test]
    fn cascade_delete_reclaims_inst_info_even_without_geo_relate_edges() {
        let sql = render_cascade_delete("inst_relate:7997_1");

        let explicit_gc_at = sql
            .find("delete $old_inst;")
            .expect("inst_info must be deleted explicitly, not via geo_relate rows");
        // 显式回收发生在引用计数守卫之内、几何三元组清理之后。
        let guard_at = sql
            .find("array::len($old_inst<-inst_relate)")
            .expect("gc guard");
        let geo_rows_at = sql
            .find("from $old_inst->geo_relate")
            .expect("geo triple cleanup");
        assert!(guard_at < explicit_gc_at, "{sql}");
        assert!(geo_rows_at < explicit_gc_at, "{sql}");
        assert!(
            !sql.contains("[out, id, in]"),
            "inst_info must not ride the geo_relate triple delete: {sql}"
        );
    }

    #[tokio::test]
    async fn delete_rejects_zero_chunk_size() {
        let error = delete_inst_relate_cascade(&[RefnoEnum::from("1/1")], 0)
            .await
            .expect_err("zero chunk size must not panic");
        assert!(error.to_string().contains("chunk_size"));
    }

    #[tokio::test]
    #[ignore = "manual live: requires the configured AvevaMarineSample Surreal database"]
    async fn live_deleted_branch_subtree_includes_known_damp_child() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let branch = RefnoEnum::from("24381/100817");
        let damp = RefnoEnum::from("24381/100819");
        let subtree = collect_pe_subtree_refnos(&[branch])
            .await
            .expect("collect BRAN subtree");
        assert!(subtree.contains(&branch));
        assert!(subtree.contains(&damp));
    }

    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_shared_inst_info_is_deleted_only_after_last_reference() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let first = RefnoEnum::from("4000000000/1");
        let second = RefnoEnum::from("4000000000/2");
        let cleanup = "delete inst_relate:4000000000_1, inst_relate:4000000000_2, \
            geo_relate:zz_increment_cleanup_shared, inst_info:zz_increment_cleanup_shared, \
            inst_geo:zz_increment_cleanup_shared, pe:4000000000_1, pe:4000000000_2;";
        let setup = format!(
            "{cleanup}
            create pe:4000000000_1;
            create pe:4000000000_2;
            create inst_info:zz_increment_cleanup_shared;
            create inst_geo:zz_increment_cleanup_shared;
            relate pe:4000000000_1->inst_relate:4000000000_1->inst_info:zz_increment_cleanup_shared;
            relate pe:4000000000_2->inst_relate:4000000000_2->inst_info:zz_increment_cleanup_shared;
            relate inst_info:zz_increment_cleanup_shared->geo_relate:zz_increment_cleanup_shared
                ->inst_geo:zz_increment_cleanup_shared;"
        );
        SUL_DB
            .query(setup)
            .await
            .expect("create shared graph")
            .check()
            .expect("valid setup");

        delete_inst_relate_cascade(&[first], 1)
            .await
            .expect("delete first reference");
        let mut response = SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000000_2').id != none,
                    inst_info:zz_increment_cleanup_shared.id != none,
                    geo_relate:zz_increment_cleanup_shared.id != none,
                    inst_geo:zz_increment_cleanup_shared.id != none
                ];",
            )
            .await
            .expect("query shared graph after first delete")
            .check()
            .expect("valid first-delete query");
        let after_first = response
            .take::<Vec<bool>>(0)
            .expect("decode first-delete state");

        delete_inst_relate_cascade(&[second], 1)
            .await
            .expect("delete last reference");
        let mut response = SUL_DB
            .query(
                "return [
                    inst_info:zz_increment_cleanup_shared.id != none,
                    geo_relate:zz_increment_cleanup_shared.id != none,
                    inst_geo:zz_increment_cleanup_shared.id != none
                ];",
            )
            .await
            .expect("query shared graph after last delete")
            .check()
            .expect("valid last-delete query");
        let after_last = response
            .take::<Vec<bool>>(0)
            .expect("decode last-delete state");

        SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup shared graph")
            .check()
            .expect("valid cleanup");
        assert_eq!(after_first, vec![true, true, true, true]);
        assert_eq!(after_last, vec![false, false, false]);
    }

    /// 临时探针：从 `AIOS_PROBE_SQL` 读 `;;` 分隔的查询并打印截断结果，用完即删。
    #[tokio::test]
    #[ignore = "temp probe: run ad-hoc SQL from AIOS_PROBE_SQL"]
    async fn probe_live_sql() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let batch = std::env::var("AIOS_PROBE_SQL").expect("set AIOS_PROBE_SQL");
        for sql in batch.split(";;").map(str::trim).filter(|s| !s.is_empty()) {
            let mut response = SUL_DB
                .query(sql)
                .await
                .expect("probe query")
                .check()
                .expect("probe statement");
            let v: surrealdb::Value = response.take(0).expect("take probe");
            let s = format!("{v:?}");
            println!("== {sql}\n{}\n", &s[..s.len().min(4000)]);
        }
    }

    /// B2 Live：几何生成半途失败会留下没有任何 `geo_relate` 边的 `inst_info`。
    /// 级联删除必须把它一并回收，而不是因「几何三元组为空集」而永久遗留。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_inst_info_without_geo_relate_is_reclaimed() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let orphan = RefnoEnum::from("4000000000/3");
        let cleanup =
            "delete inst_relate:4000000000_3, inst_info:zz_no_geo_orphan, pe:4000000000_3;";
        let setup = format!(
            "{cleanup}
            create pe:4000000000_3;
            create inst_info:zz_no_geo_orphan;
            relate pe:4000000000_3->inst_relate:4000000000_3->inst_info:zz_no_geo_orphan;"
        );
        SUL_DB
            .query(setup)
            .await
            .expect("create orphan graph")
            .check()
            .expect("valid setup");

        delete_inst_relate_cascade(&[orphan], 1)
            .await
            .expect("delete lone reference");

        let mut response = SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000000_3').id != none,
                    inst_info:zz_no_geo_orphan.id != none
                ];",
            )
            .await
            .expect("query orphan state")
            .check()
            .expect("valid orphan query");
        let state = response.take::<Vec<bool>>(0).expect("decode orphan state");

        SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup orphan graph")
            .check()
            .expect("valid cleanup");
        assert_eq!(
            state,
            vec![false, false],
            "an inst_info with no geo_relate edges must still be reclaimed"
        );
    }

    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_soft_deleted_subtree_removes_all_model_nodes() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let root = RefnoEnum::from("4000000000/10");
        let cleanup = "delete pe:4000000000_10, pe:4000000000_11, pe:4000000000_12, \
            inst_relate:4000000000_10, inst_relate:4000000000_11, inst_relate:4000000000_12, \
            inst_info:zz_delete_10, inst_info:zz_delete_11, inst_info:zz_delete_12, \
            geo_relate:zz_delete_10, geo_relate:zz_delete_11, geo_relate:zz_delete_12, \
            inst_geo:zz_delete_10, inst_geo:zz_delete_11, inst_geo:zz_delete_12;";
        let setup = format!(
            "{cleanup}
            create pe:4000000000_10 set deleted = true;
            create pe:4000000000_11 set deleted = true;
            create pe:4000000000_12 set deleted = true;
            relate pe:4000000000_11->pe_owner->pe:4000000000_10;
            relate pe:4000000000_12->pe_owner->pe:4000000000_11;
            create inst_info:zz_delete_10;
            create inst_info:zz_delete_11;
            create inst_info:zz_delete_12;
            create inst_geo:zz_delete_10;
            create inst_geo:zz_delete_11;
            create inst_geo:zz_delete_12;
            relate pe:4000000000_10->inst_relate:4000000000_10->inst_info:zz_delete_10;
            relate pe:4000000000_11->inst_relate:4000000000_11->inst_info:zz_delete_11;
            relate pe:4000000000_12->inst_relate:4000000000_12->inst_info:zz_delete_12;
            relate inst_info:zz_delete_10->geo_relate:zz_delete_10->inst_geo:zz_delete_10;
            relate inst_info:zz_delete_11->geo_relate:zz_delete_11->inst_geo:zz_delete_11;
            relate inst_info:zz_delete_12->geo_relate:zz_delete_12->inst_geo:zz_delete_12;"
        );
        SUL_DB
            .query(setup)
            .await
            .expect("create deleted subtree")
            .check()
            .expect("valid setup");

        delete_inst_relate_subtree(&[root], 10)
            .await
            .expect("delete model subtree");
        let mut response = SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000000_10').id != none,
                    type::thing('inst_relate', '4000000000_11').id != none,
                    type::thing('inst_relate', '4000000000_12').id != none,
                    inst_info:zz_delete_10.id != none,
                    inst_info:zz_delete_11.id != none,
                    inst_info:zz_delete_12.id != none,
                    geo_relate:zz_delete_10.id != none,
                    geo_relate:zz_delete_11.id != none,
                    geo_relate:zz_delete_12.id != none,
                    inst_geo:zz_delete_10.id != none,
                    inst_geo:zz_delete_11.id != none,
                    inst_geo:zz_delete_12.id != none
                ];",
            )
            .await
            .expect("query deleted model subtree")
            .check()
            .expect("valid deleted-subtree query");
        let state = response
            .take::<Vec<bool>>(0)
            .expect("decode deleted-subtree state");

        SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup deleted subtree")
            .check()
            .expect("valid cleanup");
        assert_eq!(state, vec![false; 12]);
    }
}
