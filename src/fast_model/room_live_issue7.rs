//! [issue #7](https://github.com/happyrust/gen-model/issues/7) 在**真实项目库**上的两步复现。
//!
//! 与 `room_fixture` 里那条同名回归的分工：那条跑在一次性内存实例的合成夹具上，把主嫌
//! （元素分支对空间树的反向依赖）单独隔离出来，回答的是「这个成因还在不在」；这条跑在
//! 真库、真构件、真房间上，回答的是另一个一直欠着的问题——ADR-010 §9 那条「增量收敛
//! 结果 == 全量重建结果」在**真实数据**上到底成不成立。
//!
//! 靶子取自 issue 原文：
//!
//! | | refno | noun | dbnum | 出处 |
//! |---|---|---|---|---|
//! | 构件 | `24383_66460` | `CAP` | 7999 | 报告人改的那个（`CAP 1 of /1WCC1135/B1`） |
//! | 面板 | `24381_35844` | `PANE` | 7997 | 被删的边 `room_relate:⟨24381_35844_24383_66460⟩` 的 in 端 |
//! | 房间 | `24381_35842` | `FRMW` | 7997 | `/1RX-RM05-R512` |
//!
//! 库里那个 CAP 的 `POS.z` 此刻正是 **5821.67**——issue 里那次修改已经落在这套库上了。
//!
//! 这条用例**会写真库**（模型实例、`.mesh` 文件、这一间房的归属边、队列行），并在收尾
//! 把构件的 `POS` 原样写回。跑之前先确认 8009 上是你能写的那套数据。

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use aios_core::room::room::{GLOBAL_AABB_TREE, load_aabb_tree};
    use aios_core::{RefnoEnum, SUL_DB, get_db_option};
    use serde::Deserialize;
    use surrealdb::opt::{Config, auth::Root};

    use crate::data_interface::model_update_pending::drain_rooms;
    use crate::data_interface::tidb_manager::AiosDBManager;
    use crate::fast_model::occ_generate::update_inst_relate_aabbs_by_refnos;
    use crate::fast_model::room_model::build_room_relations;

    /// 报告人改的那个构件。
    const ELEMENT: &str = "24383_66460";
    /// 被删的两条边里，在这套库上存在的那块面板（另一块 `24381_1391` 本库没有）。
    const PANEL: &str = "24381_35844";
    /// 只让这一间房参与重建，别把库里 124 间真实房间一起卷进来。
    const ROOM_KEY_WORD: &str = "-RM05-R512";

    /// 一条房间归属边，可直接相等比较。
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
    struct Edge {
        panel: String,
        part: String,
        room_num: String,
    }

    async fn connect_live() {
        let endpoint =
            std::env::var("AIOS_LIVE_WS").unwrap_or_else(|_| "ws://localhost:8009".into());
        let ns = std::env::var("AIOS_LIVE_NS").unwrap_or_else(|_| "1516".into());
        let db = std::env::var("AIOS_LIVE_DB").unwrap_or_else(|_| "AvevaMarineSample".into());
        SUL_DB
            .connect((endpoint, Config::default().ast_payload()))
            .with_capacity(1000)
            .await
            .expect("connect live");
        SUL_DB.use_ns(&ns).use_db(&db).await.expect("use ns/db");
        SUL_DB
            .signin(Root {
                username: "root",
                password: "root",
            })
            .await
            .expect("signin");
    }

    /// 这个构件当前挂在哪些面板下，已排序。
    async fn edges_of_element() -> Vec<Edge> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT record::id(in) AS panel, record::id(out) AS part, room_num \
                 FROM room_relate WHERE out = pe:{ELEMENT} ORDER BY panel;"
            ))
            .await
            .expect("query room_relate")
            .check()
            .expect("valid room_relate query");
        let mut edges: Vec<Edge> = response.take(0).expect("decode room_relate");
        edges.sort();
        edges
    }

    async fn pos_z() -> f64 {
        #[derive(Deserialize)]
        struct Row {
            #[serde(rename = "POS")]
            pos: Vec<f64>,
        }
        let mut response = SUL_DB
            .query(format!("SELECT POS FROM CAP:{ELEMENT};"))
            .await
            .expect("query POS")
            .check()
            .expect("valid POS query");
        let rows: Vec<Row> = response.take(0).expect("decode POS");
        rows.first().expect("CAP row must exist").pos[2]
    }

    async fn set_pos_z(z: f64) {
        SUL_DB
            .query(format!("UPDATE CAP:{ELEMENT} SET POS[2] = {z};"))
            .await
            .expect("update POS")
            .check()
            .expect("valid POS update");
    }

    async fn room_queue_rows() -> Vec<String> {
        let mut response = SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM model_update_pending \
                 WHERE action IN ['room_recalc_element', 'room_recalc_panel'];",
            )
            .await
            .expect("query room queue")
            .check()
            .expect("valid room queue query");
        response.take(0).expect("decode room queue")
    }

    async fn element_aabb_json() -> Vec<String> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE <string>aabb.d FROM inst_relate WHERE in = pe:{ELEMENT};"
            ))
            .await
            .expect("query aabb")
            .check()
            .expect("valid aabb query");
        response.take(0).expect("decode aabb")
    }

    /// 清掉本轮排出的房间队列行，别把真库的队列留脏。
    async fn clear_room_queue() {
        SUL_DB
            .query(
                "DELETE model_update_pending \
                 WHERE action IN ['room_recalc_element', 'room_recalc_panel'];",
            )
            .await
            .expect("clear room queue")
            .check()
            .expect("valid queue cleanup");
    }

    /// issue #7 的两步，跑在真库真构件上。
    ///
    /// 三段：
    ///
    /// 1. **备料**——按需生成面板与构件两侧的几何。这一步本身就是 ADR-010 §9 一直被卡住
    ///    的前提（「结构库从未生成、`inst_relate WHERE in.noun = 'PANE'` 为 0」）。
    /// 2. **全量基线**——只重建 `/1RX-RM05-R512` 这一间，拿到这个构件应有的归属边。
    /// 3. **两步复现**——先 `DELETE` 掉它的归属边（报告人的第一步），再改它的 `POS.z`
    ///    并走生产上纯位姿变更那条链（`update_world_transforms` → 刷新包围盒 →
    ///    `enqueue_room_recalc` → `drain_rooms` → 元素分支），断言边原样回来。
    ///
    /// 收尾把 `POS` 写回、清掉队列行。
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8009 cargo test --lib --features http_api \
    ///     live_issue7_real_db_deleted_edges_come_back -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: 写真实项目库（模型实例、mesh 文件、这一间房的归属边、队列行）"]
    async fn live_issue7_real_db_deleted_edges_come_back() {
        connect_live().await;

        let element = RefnoEnum::from(ELEMENT);
        let panel = RefnoEnum::from(PANEL);
        let mut db_option = get_db_option().clone();
        db_option.room_key_word = Some(vec![ROOM_KEY_WORD.to_string()]);
        db_option.gen_spatial_tree = true;

        let original_z = pos_z().await;
        println!("[issue7] 构件 {ELEMENT} 当前 POS.z = {original_z}");

        // ---- 1. 备料：两侧几何 ----
        let mgr = AiosDBManager::init_form_config()
            .await
            .expect("init db manager");
        for target in [PANEL, ELEMENT] {
            let result = mgr
                .ensure_model_generated(RefnoEnum::from(target), false)
                .await
                .unwrap_or_else(|error| panic!("按需生成 {target} 失败: {error:#}"));
            println!(
                "[issue7] 备料 {target}: root={} status={:?} renderable={} written={}",
                result.generation_root,
                result.status,
                result.model_instance_count,
                result.generated_instance_count
            );
        }

        // ---- 2. 全量基线 ----
        load_aabb_tree().await.expect("load spatial tree");
        update_inst_relate_aabbs_by_refnos(&[panel, element], true)
            .await
            .expect("refresh both aabbs into the tree");
        assert!(
            !GLOBAL_AABB_TREE.read().await.is_empty(),
            "空间树是空的，全量重建会拒跑"
        );
        build_room_relations(&db_option)
            .await
            .expect("full rebuild of /1RX-RM05-R512");
        let baseline = edges_of_element().await;
        println!("[issue7] 全量基线: {baseline:#?}");
        assert!(
            !baseline.is_empty(),
            "全量重建都算不出这个构件的房间归属，两步复现无从谈起——\
             先查面板与构件的几何、包围盒，以及它是不是真在这间房里"
        );

        // ---- 3. 报告人的两步 ----
        // 先把主嫌隔离出来，与合成夹具那条同一个手法：业务库里的面板几何保持完整，只把
        // PANE 从空间树上摘掉。这正是报告人现场的形态——accel_tree.bin 是结构库生成之前
        // 落的，树里一条在册 PANE 都没有。修复前元素分支从树里按 noun 找候选，这时必然
        // 捞空，然后把这个构件的归属边按「不属于任何房间」清掉。
        let panes_in_tree: HashSet<aios_core::RefU64> = GLOBAL_AABB_TREE
            .read()
            .await
            .tree
            .iter()
            .filter(|bbox| bbox.noun == "PANE")
            .map(|bbox| bbox.refno)
            .collect();
        let removed = GLOBAL_AABB_TREE
            .write()
            .await
            .remove_by_refnos(&panes_in_tree);
        println!("[issue7] 从空间树上摘掉 {removed} 条 PANE 条目（隔离 issue #7 的主嫌）");
        assert!(
            GLOBAL_AABB_TREE
                .read()
                .await
                .tree
                .iter()
                .all(|bbox| bbox.noun != "PANE"),
            "隔离变量失败：空间树里仍有 PANE"
        );

        // 第一步：手动删掉它的房间边。
        SUL_DB
            .query(format!("DELETE room_relate WHERE out = pe:{ELEMENT};"))
            .await
            .expect("delete the element's room edges")
            .check()
            .expect("valid delete");
        assert!(
            edges_of_element().await.is_empty(),
            "手动删除之后不该还有归属边"
        );

        // 第二步：改 Z。走的是生产上纯位姿变更那条链，不直调元素分支。
        clear_room_queue().await;
        set_pos_z(original_z + 100.0).await;
        // 生产上这一步由 `IncrementPipeline::invalidate_caches` 做：`get_world_transform`
        // 带进程级 `#[cached]`，属性落库之后不失效的话，后面每一个消费者读到的都还是旧
        // 矩阵——包围盒不变、房间任务不入队，这一整条链会静默地什么都不做。
        aios_core::clear_all_caches_batch(&[element]).await;
        mgr.update_world_transforms(&HashSet::from([element]))
            .await
            .expect("transform work item");
        println!("[issue7] 移动后队列: {:?}", room_queue_rows().await);
        println!("[issue7] 移动后 aabb: {:?}", element_aabb_json().await);
        let done = drain_rooms(&db_option).await.expect("drain room work");
        let after_move = edges_of_element().await;

        // 收尾：位置写回原值，并把归属收敛回基线状态。
        set_pos_z(original_z).await;
        aios_core::clear_all_caches_batch(&[element]).await;
        mgr.update_world_transforms(&HashSet::from([element]))
            .await
            .expect("restore transform");
        let _ = drain_rooms(&db_option).await;
        let restored = edges_of_element().await;
        clear_room_queue().await;

        assert!(done >= 1, "那条元素任务必须被消费掉，实得 {done}");
        assert_eq!(
            after_move, baseline,
            "\n删掉的边必须被增量建回来（issue #7）\n增量: {after_move:#?}\n基线: {baseline:#?}"
        );
        assert_eq!(
            restored, baseline,
            "\n位置写回之后归属也要回到基线\n实得: {restored:#?}\n基线: {baseline:#?}"
        );
    }

    /// 只读探针：把两步复现的几个前提各查一遍，失败时用它分清是哪一段没到位。
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8009 cargo test --lib --features http_api \
    ///     live_issue7_probe -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: 只读探针"]
    async fn live_issue7_probe() {
        connect_live().await;
        for (label, sql) in [
            ("构件", format!("SELECT VALUE <string>[id, noun, dbnum, deleted] FROM pe:{ELEMENT};")),
            ("面板", format!("SELECT VALUE <string>[id, noun, dbnum, deleted] FROM pe:{PANEL};")),
            (
                "面板在册",
                format!("SELECT VALUE <string>[id, room_num] FROM room_panel_relate WHERE out = pe:{PANEL};"),
            ),
            (
                "构件几何",
                format!("SELECT VALUE <string>[id, aabb.d, world_trans.d != none] FROM inst_relate WHERE in = pe:{ELEMENT};"),
            ),
            (
                "面板几何",
                format!("SELECT VALUE <string>[id, aabb.d, world_trans.d != none] FROM inst_relate WHERE in = pe:{PANEL};"),
            ),
            (
                "归属边",
                format!("SELECT VALUE <string>[id, room_num] FROM room_relate WHERE out = pe:{ELEMENT};"),
            ),
        ] {
            let rows: Vec<String> = SUL_DB
                .query(&sql)
                .await
                .expect("probe query")
                .check()
                .expect("valid probe query")
                .take(0)
                .expect("decode probe");
            println!("[issue7] {label}: {rows:?}");
        }
        println!("[issue7] POS.z = {}", pos_z().await);
    }
}
