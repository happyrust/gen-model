//! [issue #5](https://github.com/happyrust/gen-model/issues/5) 在**真实项目库**上的复现，加一支只读探针。
//!
//! 靶子沿用 issue #7 / #13 那组（文件名由此而来）：
//!
//! | | refno | noun | dbnum | 出处 |
//! |---|---|---|---|---|
//! | 构件 | `24383_66460` | `CAP` | 7999 | 报告人改的那个（`CAP 1 of /1WCC1135/B1`） |
//! | 分支 | `24383_66459` | `BRAN` | 7999 | `/1WCC1135/B1`，issue #5 截图里管段没画出来的那条 |
//! | 面板 | `24381_35844` | `PANE` | 7997 | 被删的边 `room_relate:⟨24381_35844_24383_66460⟩` 的 in 端 |
//!
//! issue #7 / issue #13（C2、C3）那三条真库复现曾在这里：它们走的是「刷新包围盒 → 排房间待办行
//! → drain 消费 → 元素分支」那条第二条异步账。ADR-072 把那条账整体退役（R4 / R5）：房间归属
//! 如今由该库任务的房间段整库算完写完（`room_task::finish_task_rooms`），没有队列可等、也没有
//! 待办行可查，那三条用例连同它们守的「队列收口」性质一起退役。它们真正要守的两件事另有落点：
//! 「增量收敛 == 全量重建」在 `room_fixture` 的合成夹具上直调元素分支对拍；「实时行装出来的面板集
//! == 从前发布表装的」在 `room_model` / `room_resident` 的 `mem://` 门里。
//!
//! 这条用例**会写真库**（这条分支的模型实例、`.mesh` 文件、这个管件的 `POS`），并在收尾把
//! `POS` 原样写回。跑之前先确认 8009 上是你能写的那套数据。

#[cfg(test)]
mod tests {
    use aios_core::{RefnoEnum, SUL_DB};
    use serde::Deserialize;
    use surrealdb::opt::{Config, auth::Root};

    use crate::data_interface::tidb_manager::AiosDBManager;

    /// 报告人改的那个构件。issue #5 截图里那条分支的 CAP 也是它。
    const ELEMENT: &str = "24383_66460";
    /// `ELEMENT` 所属的分支 `/1WCC1135/B1`——issue #5 里管段没画出来的那条。
    const BRANCH: &str = "24383_66459";
    /// issue #7 里被删的那条边的 in 端面板；只供只读探针查它的几何与在册状态。
    const PANEL: &str = "24381_35844";

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
                 FROM pe:{BRANCH}->inst_relate ORDER BY id;"
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
    ) -> crate::data_interface::ele_ops::EleOperationData {
        use crate::data_interface::ele_ops::{
            EleOperationData, EleOperationDetail, ModifiedElement,
        };
        use aios_core::NamedAttrValue;

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
        aios_core::clear_all_caches_batch(&[RefnoEnum::from(ELEMENT), RefnoEnum::from(BRANCH)])
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

        assert_eq!(
            planned,
            vec![
                (
                    ModelWorkAction::RegenRoot,
                    RefnoEnum::from(BRANCH).to_pdms_str(),
                    "BRAN".to_string()
                ),
                (
                    ModelWorkAction::PostRegenAabb,
                    RefnoEnum::from(ELEMENT).to_pdms_str(),
                    String::new()
                )
            ],
            "挪管件必须排整根重生成并在重生成后刷新靶件 AABB，不能是管件自己的 Transform（issue #5 的修法就在这一跳）"
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
            (
                "构件",
                format!("SELECT VALUE <string>[id, noun, dbnum, deleted] FROM pe:{ELEMENT};"),
            ),
            (
                "面板",
                format!("SELECT VALUE <string>[id, noun, dbnum, deleted] FROM pe:{PANEL};"),
            ),
            (
                "面板在册",
                format!("SELECT VALUE <string>[id, room_num] FROM pe:{PANEL}<-room_panel_relate;"),
            ),
            (
                "构件几何",
                format!(
                    "SELECT VALUE <string>[id, aabb.d, world_trans.d != none] FROM pe:{ELEMENT}->inst_relate;"
                ),
            ),
            (
                "面板几何",
                format!(
                    "SELECT VALUE <string>[id, aabb.d, world_trans.d != none] FROM pe:{PANEL}->inst_relate;"
                ),
            ),
            (
                "归属边",
                format!("SELECT VALUE <string>[id, room_num] FROM pe:{ELEMENT}<-room_relate;"),
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
