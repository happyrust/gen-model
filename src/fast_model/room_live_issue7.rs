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
//!
//! 同一套靶子上另有两条，来自
//! [issue #13](https://github.com/happyrust/gen-model/issues/13) 的 C2 与 C3：构件**移出**
//! 房间后归属要消失（上面那条只验了移动后能回来），以及按需生成留下的 `dbnum: 0` 存量
//! pending 行对它自己那个库是不可见的。
//!
//! 还有一条来自 [issue #5](https://github.com/happyrust/gen-model/issues/5)：**同一个 CAP**
//! 正是那张截图里的管件，它所属的 `/1WCC1135/B1` 就是截图里管段没画出来的那条分支。
//! 那条用例验的是「挪这个管件，隐含直管段跟不跟」——与房间无关，但靶子完全重合，
//! 备料、连库、复原那套东西一份就够。

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

    /// 报告人改的那个构件。issue #5 截图里那条分支的 CAP 也是它。
    const ELEMENT: &str = "24383_66460";
    /// `ELEMENT` 所属的分支 `/1WCC1135/B1`——issue #5 里管段没画出来的那条。
    const BRANCH: &str = "24383_66459";
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

    /// 这个生成根在 `model_update_pending` 里的那一行，渲染成
    /// `[id, dbnum, revision, attempts, status]`；没有则 `None`。
    async fn pending_row_of(root_refno: &str) -> Option<String> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE <string>[record::id(id), dbnum, revision, attempts, status] \
                 FROM model_update_pending \
                 WHERE action = 'regen_root' AND target_refno = '{root_refno}';"
            ))
            .await
            .expect("query pending row")
            .check()
            .expect("valid pending query");
        let rows: Vec<String> = response.take(0).expect("decode pending row");
        rows.into_iter().next()
    }

    /// 只清掉本轮为这两个靶子排出的房间队列行。
    ///
    /// 这里曾经是 `DELETE … WHERE action IN ['room_recalc_element', 'room_recalc_panel']`
    /// ——一把清空整张表的房间行。在真库上跑一次就会连带抹掉别人的积压（实测抹掉过 41 条
    /// `dbnum = 1112` 的 `room_recalc_element`），而那些行没有任何东西会把它们排回来：
    /// 房间任务的入队条件是「AABB 真的变了」，删掉就等于那批构件的归属静默停在旧值。
    /// 收尾只该收自己排的那两行，按确定的 record id 定点删。
    async fn clear_room_queue() {
        SUL_DB
            .query(format!(
                "DELETE model_update_pending:room_recalc_element_{ELEMENT};\
                 DELETE model_update_pending:room_recalc_panel_{PANEL};"
            ))
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

    /// issue #13 C2：构件**移出**房间之后，归属边必须消失。
    ///
    /// 上面那条只覆盖了「移动 +100 之后仍回到 R512」——移完 AABB 的 `mins.z` 是 5907.67，
    /// 人还在房间里，验的是「边能回来」。反方向没人测：把它挪到房间外，那条边必须被清掉。
    /// 这一半失手同样是静默的——元素分支先清后写，清那一步只覆盖本轮候选面板，漏了就留下
    /// 一条指向它早已离开的房间的陈旧边，而任务照样成功。
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8009 cargo test --lib --features http_api \
    ///     live_issue13_c2_moving_out_of_the_room_clears_membership -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: 写真实项目库（这个构件的 POS、它的归属边与队列行）"]
    async fn live_issue13_c2_moving_out_of_the_room_clears_membership() {
        connect_live().await;

        let element = RefnoEnum::from(ELEMENT);
        let panel = RefnoEnum::from(PANEL);
        let mut db_option = get_db_option().clone();
        db_option.room_key_word = Some(vec![ROOM_KEY_WORD.to_string()]);
        db_option.gen_spatial_tree = true;

        let original_z = pos_z().await;
        let baseline = edges_of_element().await;
        assert!(
            !baseline.is_empty(),
            "起点就没有归属边，这条用例无从谈起——先跑 \
             live_issue7_real_db_deleted_edges_come_back，或按 -RM05-R512 重建一次这间房"
        );
        println!("[issue13-c2] 起点 POS.z={original_z} 归属={baseline:#?}");

        load_aabb_tree().await.expect("load spatial tree");
        update_inst_relate_aabbs_by_refnos(&[panel, element], true)
            .await
            .expect("refresh both aabbs into the tree");

        let mgr = AiosDBManager::init_form_config()
            .await
            .expect("init db manager");
        clear_room_queue().await;

        // 真的挪出去：+100 是上面那条用例的量级，构件还在房间里。
        set_pos_z(original_z + 100_000.0).await;
        aios_core::clear_all_caches_batch(&[element]).await;
        mgr.update_world_transforms(&HashSet::from([element]))
            .await
            .expect("transform work item");
        println!("[issue13-c2] 移出后队列: {:?}", room_queue_rows().await);
        println!("[issue13-c2] 移出后 aabb: {:?}", element_aabb_json().await);
        let done_out = drain_rooms(&db_option).await.expect("drain room work");
        let after_out = edges_of_element().await;

        // 收尾：写回原位，把归属收敛回基线。
        set_pos_z(original_z).await;
        aios_core::clear_all_caches_batch(&[element]).await;
        mgr.update_world_transforms(&HashSet::from([element]))
            .await
            .expect("restore transform");
        let _ = drain_rooms(&db_option).await;
        let restored = edges_of_element().await;
        clear_room_queue().await;

        assert!(done_out >= 1, "那条元素任务必须被消费掉，实得 {done_out}");
        assert!(
            after_out.is_empty(),
            "\n构件已经挪出 R512，它的归属边必须被清掉\n实得: {after_out:#?}"
        );
        assert_eq!(
            restored, baseline,
            "\n写回原位之后归属要回到基线\n实得: {restored:#?}\n基线: {baseline:#?}"
        );
    }

    /// 隐含直管段在世界坐标下的起点与长度。
    ///
    /// 管段没有自己的 `pe`，行挂在 BRAN 名下、`out` 指向共享单位几何，所以
    /// `world_trans` 是「单位圆柱 → 世界管段」那个缩放矩阵：`translation` 是起点，
    /// `scale[2]` 是长度。两者相加就是管段的**远端**，也正是它该顶到的那个管件。
    #[derive(Debug, Clone, PartialEq, Deserialize)]
    struct Tube {
        id: String,
        tz: f64,
        sz: f64,
    }

    impl Tube {
        /// 管段远端的世界 z。
        fn far_z(&self) -> f64 {
            self.tz + self.sz
        }
    }

    async fn tubes_of_branch() -> Vec<Tube> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT record::id(id) AS id, \
                        world_trans.d.translation[2] AS tz, \
                        world_trans.d.scale[2] AS sz \
                 FROM inst_relate WHERE in = pe:{BRANCH} ORDER BY id;"
            ))
            .await
            .expect("query branch tubing")
            .check()
            .expect("valid tubing query");
        response.take(0).expect("decode branch tubing")
    }

    /// 一次纯位姿修改操作，形状与 `IncrementPipeline::collect_changes` 交出来的一致。
    ///
    /// 属性级分类只看**属性名**（`POS` → `TransformOnly`），不看值，所以这里用占位值就够；
    /// 真正决定去向的是 `current_data.owner` ——计划层要顺着它解析生成根。
    fn pose_change_op(
        refno: RefnoEnum,
        owner: RefnoEnum,
        noun: &str,
    ) -> pdms_io::io::EleOperationData {
        use aios_core::NamedAttrValue;
        use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement};

        let mut modified_attrs = std::collections::HashMap::new();
        modified_attrs.insert(
            "POS".to_string(),
            (
                NamedAttrValue::StringType("old".into()),
                NamedAttrValue::StringType("new".into()),
            ),
        );
        let mut modified = ModifiedElement {
            current_data: Default::default(),
            added_attrs: Default::default(),
            deleted_attrs: Default::default(),
            modified_attrs,
            added_explicit_attrs: Default::default(),
            deleted_explicit_attrs: Default::default(),
            modified_explicit_attrs: Default::default(),
            added_uda_attrs: Default::default(),
            deleted_uda_attrs: Default::default(),
            modified_uda_attrs: Default::default(),
            noun: noun.to_string(),
            children_changed: None,
        };
        modified.current_data.owner = owner.refno();
        EleOperationData::new(refno.refno(), 42, EleOperationDetail::Modified(modified))
    }

    /// 把这个分支重生成一遍——`RegenRoot` 落到执行层就是这一步。
    async fn regenerate_branch(mgr: &AiosDBManager) {
        aios_core::clear_all_caches_batch(&[
            RefnoEnum::from(ELEMENT),
            RefnoEnum::from(BRANCH),
        ])
        .await;
        crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(
            mgr,
            &[RefnoEnum::from(BRANCH).to_pdms_str()],
        )
        .await
        .expect("regenerate /1WCC1135/B1");
    }

    /// issue #5：挪这个管件，隐含直管段必须跟着走。
    ///
    /// 截图里的现象是「管件动了、管段停在旧位置」。成因是纯位姿变更走便宜路径：
    /// `POS` 判 `TransformOnly` → 给管件自己排 `Transform` → `update_world_transforms`
    /// 刷新子树世界变换，而那一步**显式排除了管段行**（管段几何是分支成员 arrive/leave
    /// 点的函数，位姿层算不出来）。修法在计划层：生成根是 BRAN/HANG 的位姿变更改判整根
    /// 重生成（`model_update_plan::reroute_derived_geometry_units`）。
    ///
    /// 这里两段都验，且都用报告人那条真实分支：
    ///
    /// 1. **计划层**——挪这个 CAP，工作项必须是该 BRAN 的 `RegenRoot`，不是 CAP 的
    ///    `Transform`；
    /// 2. **症状层**——真挪、真重生成，断言管段远端跟着管件走。判据取
    ///    `translation.z + scale.z == 管件的 POS.z`：基线上它逐位成立（5701.67 + 120
    ///    = 5821.67），修复前管段不动，这个等式会在移动后当场破掉。
    ///
    /// 收尾把 `POS` 写回并再重生成一次，断言回到基线。
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8009 cargo test --lib --features http_api \
    ///     live_issue5_moving_the_fitting_moves_its_implicit_tubing -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: 写真实项目库（这条分支的模型实例与 mesh 文件、这个管件的 POS）"]
    async fn live_issue5_moving_the_fitting_moves_its_implicit_tubing() {
        use crate::data_interface::model_update_plan::{ModelWorkAction, build_model_update_plan};

        /// `24383` 前缀属这个库（`dbnum_info_table:24383`）。
        const DBNUM: u32 = 7999;
        const SHIFT: f64 = 100.0;

        connect_live().await;
        let mgr = AiosDBManager::init_form_config()
            .await
            .expect("init db manager");

        let original_z = pos_z().await;
        let baseline = tubes_of_branch().await;
        println!("[issue5] 管件 POS.z = {original_z}");
        println!("[issue5] 基线管段: {baseline:#?}");
        assert_eq!(
            baseline.len(),
            1,
            "这条分支该恰好有一段隐含直管段；不是 1 段就先查生成，别在这条用例里猜"
        );
        assert!(
            (baseline[0].far_z() - original_z).abs() < 1e-3,
            "基线本身就不自洽：管段远端 {} 与管件 POS.z {original_z} 对不上，\
             先把这条分支重生成一遍再来复测",
            baseline[0].far_z()
        );

        // ---- 1. 计划层：挪这个管件必须整根重生成 ----
        let moved_z = original_z + SHIFT;
        set_pos_z(moved_z).await;
        aios_core::clear_all_caches_batch(&[RefnoEnum::from(ELEMENT)]).await;
        let plan = build_model_update_plan(
            DBNUM,
            42,
            "DESI",
            &std::collections::BTreeMap::from([(
                42,
                vec![pose_change_op(
                    RefnoEnum::from(ELEMENT),
                    RefnoEnum::from(BRANCH),
                    "CAP",
                )],
            )]),
        )
        .await
        .expect("build the moved fitting's plan");
        let planned = plan
            .work_items
            .iter()
            .map(|item| (item.action, item.target_refno.clone(), item.noun.clone()))
            .collect::<Vec<_>>();
        println!("[issue5] 计划: {planned:#?}");

        // ---- 2. 症状层：重生成之后管段必须跟到新位置 ----
        regenerate_branch(&mgr).await;
        let after_move = tubes_of_branch().await;
        println!("[issue5] 移动后管段: {after_move:#?}");

        // 收尾放在断言之前：任一条红了，真库也不能停在被挪走的状态。
        set_pos_z(original_z).await;
        regenerate_branch(&mgr).await;
        let restored = tubes_of_branch().await;
        SUL_DB
            .query(format!(
                "DELETE model_update_pending:room_recalc_element_{ELEMENT};\
                 DELETE model_update_pending:room_recalc_element_{BRANCH};"
            ))
            .await
            .expect("clear room queue rows this run may have enqueued")
            .check()
            .expect("valid queue cleanup");

        assert_eq!(
            planned,
            vec![(
                ModelWorkAction::RegenRoot,
                RefnoEnum::from(BRANCH).to_pdms_str(),
                "BRAN".to_string()
            )],
            "挪管件必须排整根重生成，不能是管件自己的 Transform（issue #5 的修法就在这一跳）"
        );
        assert_eq!(after_move.len(), 1, "移动后仍该只有一段管段");
        assert!(
            (after_move[0].far_z() - moved_z).abs() < 1e-3,
            "\n管段没跟着管件走（issue #5 的原始现象）\
             \n管件挪到 z={moved_z}，管段远端却停在 {}\n移动前: {baseline:#?}\n移动后: {after_move:#?}",
            after_move[0].far_z()
        );
        assert_eq!(
            restored, baseline,
            "\n写回原位再重生成之后，管段要回到基线\n实得: {restored:#?}\n基线: {baseline:#?}"
        );
    }

    /// issue #13 C3：按需生成留下的存量 pending 行，对它自己那个库是不可见的。
    ///
    /// 重试工作单按 `dbnum` **精确过滤**（`load_pending_model_units_for_retry`，为的是别让
    /// A 库的批次去跑 B 库的根），而 `ensure_regen_pending` 写的行 `dbnum` 是 0。于是窗口
    /// 即便把这个根重新生成成功，也拿不到它的 revision，尾事务不会收它，提交后空闲轮立刻
    /// 对着持久层把同一个根再生成一遍。修复是在 `UnitTask.revision` 缺位时补查一次
    /// `current_regen_revision`——那条路不按 dbnum 过滤。
    ///
    /// 三件事在真库真根上钉死：写的确实是 `dbnum: 0`；按库号取的工作单确实看不见它；而
    /// 按 `dbnum = 0` 取得到，所以「看不见」只能归因于那道过滤，不是行本身有问题。
    ///
    /// ```text
    /// AIOS_LIVE_WS=ws://localhost:8009 cargo test --lib --features http_api \
    ///     live_issue13_c3_on_demand_pending_is_invisible_to_its_own_database -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: 在真库上写一行 model_update_pending 再删掉"]
    async fn live_issue13_c3_on_demand_pending_is_invisible_to_its_own_database() {
        use crate::data_interface::manual_update::load_pending_model_units_for_retry;
        use crate::data_interface::model_update_pending::{
            current_regen_revision, ensure_regen_pending,
        };

        /// `pe:24383_66460`（那个 CAP）的生成根，`/1WCC1135/B1`。
        const ROOT: &str = "24383/66459";
        const ROOT_DBNUM: u32 = 7999;

        connect_live().await;

        assert!(
            pending_row_of(ROOT).await.is_none(),
            "真库里已经有 {ROOT} 的 pending 行了，这条用例会把它覆盖掉——先确认那行是谁的"
        );

        let revision = ensure_regen_pending(ROOT, "BRAN")
            .await
            .expect("按需生成落 durable pending");
        let row = pending_row_of(ROOT).await.expect("刚写的行必须查得到");
        println!("[issue13-c3] 按需生成写的行: {row}");
        assert!(
            row.starts_with("[regen_root_24383_66459, 0,"),
            "按需生成写的行 dbnum 必须是 0——这正是它被窗口漏收的根因: {row}"
        );

        let by_dbnum = load_pending_model_units_for_retry(ROOT_DBNUM)
            .await
            .expect("按库号取重试工作单");
        let by_zero = load_pending_model_units_for_retry(0)
            .await
            .expect("按 dbnum=0 取重试工作单");
        let looked_up = current_regen_revision(ROOT).await.expect("补查当前 revision");

        // 收尾放在断言之前：断言一旦红了，这行不能留在真库里。
        SUL_DB
            .query(format!(
                "DELETE model_update_pending \
                 WHERE action = 'regen_root' AND target_refno = '{ROOT}';"
            ))
            .await
            .expect("cleanup pending row")
            .check()
            .expect("valid cleanup");

        assert!(
            !by_dbnum.iter().any(|unit| unit.root_refno == ROOT),
            "按 dbnum={ROOT_DBNUM} 取的工作单不该看见 dbnum=0 的行；看得见就说明那道过滤变了，\
             本用例连同它守的那个补查一起要重新评估"
        );
        assert!(
            by_zero.iter().any(|unit| unit.root_refno == ROOT),
            "dbnum=0 那一侧必须取得到——否则上面那条『看不见』就不能归因于 dbnum 过滤"
        );
        assert_eq!(
            looked_up,
            Some(revision),
            "补查这条路必须看得见它，否则窗口把这个根生成成功之后仍然收不了口"
        );
        assert!(pending_row_of(ROOT).await.is_none(), "收尾没删干净");
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
