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

    use aios_core::options::DbOption;
    use aios_core::room::room::{GLOBAL_AABB_TREE, load_aabb_tree};
    use aios_core::{RefnoEnum, SUL_DB, get_db_option};
    use serde::Deserialize;
    use surrealdb::opt::{Config, auth::Root};

    use crate::data_interface::model_update_pending::drain_rooms;
    use crate::data_interface::tidb_manager::AiosDBManager;
    use crate::fast_model::aabb_tree::rebuild_tree_from_pointers;
    use crate::fast_model::occ_generate::update_inst_relate_aabbs_by_refnos;
    use crate::fast_model::room_model::build_room_relations;

    /// 报告人改的那个构件。issue #5 截图里那条分支的 CAP 也是它。
    const ELEMENT: &str = "24383_66460";
    /// `ELEMENT` 所属的分支 `/1WCC1135/B1`——issue #5 里管段没画出来的那条。
    const BRANCH: &str = "24383_66459";
    /// 被删的两条边里，落在本用例重建范围内的那块面板。
    const PANEL: &str = "24381_35844";
    /// `PANEL` 所属的房间节点（`/1RX-RM05-R512`），[`ROOM_KEY_WORD`] 圈的就是它。
    const ROOM: &str = "24381_35842";
    /// 只让这一间房参与重建，别把库里两百多间真实房间一起卷进来。
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
                 FROM pe:{ELEMENT}<-room_relate ORDER BY panel;"
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
                "SELECT VALUE <string>aabb.d FROM pe:{ELEMENT}->inst_relate;"
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

    /// [`ROOM`] 名下的 PANE（子 + 孙两层，与房间归属计算的层级覆盖同口径）。
    ///
    /// 这是本用例**真正重建的范围**：`ROOM_KEY_WORD` 把 `build_room_relations` 卡到这一间，
    /// 元素分支的 `PanelIndex` 也只装得下这些面板。断言必须收在同一个范围里，否则就是拿
    /// 「只重建了一间房」的结果去比「这个构件在全库的所有归属边」——两个东西。
    ///
    /// 这不是假想：2026-08-25 在 `python/testbed/.surreal/pytest-ams` 上实测，靶件 CAP
    /// 同时挂着 `24381_35844 -> R512` 与 `24381_1391 -> R142`（后者在 `/1RX-RM01-R142`
    /// 名下），范围内只有前一条。用例此前能过，是因为 2026-08-06 那套库里 `pe:24381_1391`
    /// 压根不存在——验证报告把它记成了脚注，其实是个依赖。
    async fn scoped_panels() -> HashSet<String> {
        let panels: Vec<String> = SUL_DB
            .query(format!(
                "SELECT VALUE record::id(id) FROM pe WHERE noun = 'PANE' AND deleted != true \
                 AND (owner = pe:⟨{ROOM}⟩ OR owner.owner = pe:⟨{ROOM}⟩);"
            ))
            .await
            .expect("query the scoped panels")
            .check()
            .expect("valid scoped panel query")
            .take(0)
            .expect("decode the scoped panels");
        let scope: HashSet<String> = panels.into_iter().collect();
        assert!(
            scope.contains(PANEL),
            "重建范围里没有靶子面板 {PANEL}，后面比什么都没意义——先确认 {ROOM} 还是 \
             /1RX-RM05-R512 且 {PANEL} 还挂在它名下"
        );
        scope
    }

    /// 只保留落在本次重建范围内的边。
    fn within_scope(edges: &[Edge], scope: &HashSet<String>) -> Vec<Edge> {
        edges
            .iter()
            .filter(|edge| scope.contains(&edge.panel))
            .cloned()
            .collect()
    }

    /// 一条 `room_relate` 边的完整载荷，够按原值写回。
    #[derive(Debug, Clone, Deserialize)]
    struct RoomEdgePayload {
        panel: String,
        part: String,
        room_num: String,
        inside_count: i64,
        center_dist: f64,
    }

    /// 落在本次重建范围**之外**的归属边。
    ///
    /// 必须单独捞出来备份：元素分支发的是 `DELETE {element}<-room_relate`，只避开
    /// `protected_panels`（在册但缺几何的面板）。范围外那间房的面板在这个 keyword 下压根
    /// 不在册、不受保护，于是它的边会被一并抹掉，而用例的收尾只写回 `POS`——跑一次少一条，
    /// 共享沙箱里没有任何东西会把它补回来。
    async fn out_of_scope_edges(scope: &HashSet<String>) -> Vec<RoomEdgePayload> {
        let edges: Vec<RoomEdgePayload> = SUL_DB
            .query(format!(
                "SELECT record::id(in) AS panel, record::id(out) AS part, room_num, \
                        inside_count, center_dist \
                 FROM pe:{ELEMENT}<-room_relate;"
            ))
            .await
            .expect("query the element's full room edges")
            .check()
            .expect("valid full room edge query")
            .take(0)
            .expect("decode the element's full room edges");
        edges
            .into_iter()
            .filter(|edge| !scope.contains(&edge.panel))
            .collect()
    }

    /// 把范围外的边按原值写回。
    ///
    /// 这是收尾的**最后**一步数据操作：排在任何一次 drain 之前写回，等于把它再喂给元素
    /// 分支抹一遍。
    async fn restore_out_of_scope_edges(edges: &[RoomEdgePayload]) {
        if edges.is_empty() {
            return;
        }
        // 显式 id 用 `type::thing`：refno 形态的 id（`24381_1391`）写成裸字面量会被
        // SurrealQL 当成带下划线分隔符的数字。
        let rows = edges
            .iter()
            .map(|edge| {
                format!(
                    "{{ id: type::thing('room_relate', '{}_{}'), \
                       in: type::thing('pe', '{}'), out: type::thing('pe', '{}'), \
                       room_num: '{}', inside_count: {}, center_dist: {} }}",
                    edge.panel,
                    edge.part,
                    edge.panel,
                    edge.part,
                    edge.room_num,
                    edge.inside_count,
                    edge.center_dist
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        SUL_DB
            .query(format!("INSERT RELATION INTO room_relate [{rows}];"))
            .await
            .expect("restore the out-of-scope room edges")
            .check()
            .expect("valid out-of-scope restore");
        println!("[房间基线] 已写回 {} 条范围外归属边", edges.len());
    }

    /// 备料 + 只重建 `/1RX-RM05-R512` 这一间，返回这个构件在**范围内**应有的归属边。
    ///
    /// 两条用例共用。此前只有 issue7 那条做这件事，issue13-c2 直接把 `edges_of_element()`
    /// 的当前值当基线并要求它非空——于是它隐式依赖「issue7 先跑过」。2026-08-19 的批次里
    /// #01（issue7）红了，#02（c2）随即以「起点无归属边」前置阻断，报的根本不是它自己要
    /// 守的那条性质。备料与全量重建两侧都幂等，自足铸一次基线换来的是两条用例可以任意
    /// 顺序、单独运行。
    ///
    /// `rebuild_tree_from_pointers` 不能省：`build_room_relations` 前面那道覆盖率门要求
    /// 树的条目数达到库里可用包围盒指针数的 90%，只刷这两个 refno 的包围盒够不到。
    async fn build_room_baseline(
        mgr: &AiosDBManager,
        db_option: &DbOption,
        scope: &HashSet<String>,
    ) -> Vec<Edge> {
        for target in [PANEL, ELEMENT] {
            let result = mgr
                .ensure_model_generated(RefnoEnum::from(target), false)
                .await
                .unwrap_or_else(|error| panic!("按需生成 {target} 失败: {error:#}"));
            println!(
                "[房间基线] 备料 {target}: root={} status={:?} renderable={} written={}",
                result.generation_root,
                result.status,
                result.model_instance_count,
                result.generated_instance_count
            );
        }

        load_aabb_tree().await.expect("load spatial tree");
        rebuild_tree_from_pointers()
            .await
            .expect("rebuild complete spatial tree from persistent pointers");
        update_inst_relate_aabbs_by_refnos(
            &[RefnoEnum::from(PANEL), RefnoEnum::from(ELEMENT)],
            true,
        )
        .await
        .expect("refresh both aabbs into the tree");
        assert!(
            !GLOBAL_AABB_TREE.read().await.is_empty(),
            "空间树是空的，全量重建会拒跑"
        );

        build_room_relations(db_option)
            .await
            .expect("full rebuild of /1RX-RM05-R512");
        let baseline = within_scope(&edges_of_element().await, scope);
        assert!(
            !baseline.is_empty(),
            "全量重建都算不出这个构件在 /1RX-RM05-R512 里的归属，两步复现无从谈起——\
             先查面板与构件的几何、包围盒，以及它是不是真在这间房里"
        );
        baseline
    }

    /// 这个构件的元素任务在队列里的现状，渲染成 `[id, revision, attempts, status]`；
    /// 已经被收走则 `None`。
    async fn room_element_row() -> Option<String> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE <string>[record::id(id), revision, attempts, status] \
                 FROM model_update_pending:room_recalc_element_{ELEMENT};"
            ))
            .await
            .expect("query the element room task")
            .check()
            .expect("valid element room task query")
            .take::<Vec<String>>(0)
            .expect("decode the element room task");
        response.pop()
    }

    const ROOM_CONVERGENCE_POLL: std::time::Duration = std::time::Duration::from_secs(2);
    const ROOM_CONVERGENCE_ROUNDS: usize = 30;

    /// 一次「等这条元素任务收口」的结果。判定只看 [`RoomConvergence::remaining`]，
    /// 其余字段都是诊断用的。
    struct RoomConvergence {
        /// 进场时那行还在不在。**只作诊断**：共享实库上生产 worker 可能在我们看第一眼
        /// 之前就把它收走了，拿它当判据等于把刚修掉的竞态换个地方再犯一次。
        enqueued: bool,
        /// 本进程自己的 `drain_rooms` 吃掉了几行。同样只作诊断。
        consumed_here: usize,
        rounds: usize,
        /// 等满还在的话，它的现状；收口了则 `None`。
        remaining: Option<String>,
        /// 本进程 drain 期间报出来的失败（含 drain 调用本身失败）。
        failures: Vec<String>,
    }

    impl RoomConvergence {
        fn converged(&self) -> bool {
            self.remaining.is_none()
        }

        fn describe(&self) -> String {
            let mut text = format!(
                "进场时{}见任务行；本进程消费 {} 行 / 等待 {} 轮；{}",
                if self.enqueued { "" } else { "未" },
                self.consumed_here,
                self.rounds,
                match &self.remaining {
                    Some(row) => format!("仍在队列: {row}"),
                    None => "已收口".to_string(),
                }
            );
            if !self.failures.is_empty() {
                text.push_str(&format!("；drain 报错: {}", self.failures.join(" | ")));
            }
            text
        }
    }

    /// 等到这个构件的元素任务从队列里消失为止，**不关心是谁把它收走的**。
    ///
    /// 这里刻意不再断言 `drain_rooms(..).done >= 1`。`done` 是**本次调用**的吞吐计数，
    /// 而共享实库上还跑着生产 worker——它的空闲轮 `room_round` 会先把这行收走，本用例
    /// 随后拿到 0。2026-08-19 @8019 那次失败就是这么来的：引擎把边算对了，用例却红在一个
    /// 与不变量无关的计数上，而且那条计数断言排在边集断言之前，把唯一有意义的结论一并
    /// 遮住了（见 `docs/2026-08-12_live-test-ledger.md`）。
    ///
    /// 要守的性质是「这行最终被收口」——它对『谁收的』不敏感，因此独占实例与共享实例上
    /// 都成立。本进程照旧每轮自己 drain 一次：没有 worker 时它就是唯一的消费者。
    async fn wait_for_room_convergence(db_option: &DbOption) -> RoomConvergence {
        let mut convergence = RoomConvergence {
            enqueued: room_element_row().await.is_some(),
            consumed_here: 0,
            rounds: 0,
            remaining: None,
            failures: Vec::new(),
        };
        for round in 1..=ROOM_CONVERGENCE_ROUNDS {
            convergence.rounds = round;
            match drain_rooms(db_option).await {
                Ok(report) => {
                    convergence.consumed_here += report.done;
                    convergence.failures.extend(report.failures);
                }
                Err(error) => convergence
                    .failures
                    .push(format!("drain_rooms 调用失败: {error:#}")),
            }
            match room_element_row().await {
                None => {
                    convergence.remaining = None;
                    return convergence;
                }
                Some(row) => convergence.remaining = Some(row),
            }
            tokio::time::sleep(ROOM_CONVERGENCE_POLL).await;
        }
        convergence
    }

    /// issue #7 的两步，跑在真库真构件上。
    ///
    /// 三段：
    ///
    /// 1. **备料**——按需生成面板与构件两侧的几何。这一步本身就是 ADR-010 §9 一直被卡住
    ///    的前提（「结构库从未生成、`inst_relate WHERE in.noun = 'PANE'` 为 0」）。
    /// 2. **全量基线**——只重建 `/1RX-RM05-R512` 这一间，拿到这个构件应有的归属边。
    ///    两段都在 [`build_room_baseline`] 里，与 issue13-c2 共用。
    /// 3. **两步复现**——先 `DELETE` 掉它的归属边（报告人的第一步），再改它的 `POS.z`
    ///    并走生产上纯位姿变更那条链（`update_world_transforms` → 刷新包围盒 →
    ///    `enqueue_room_recalc` → 房间队列 → 元素分支），等队列行收口后断言边原样回来。
    ///
    /// 收口用 [`wait_for_room_convergence`] 等，**不**断言本进程 drain 的吞吐计数：
    /// 共享实库上生产 worker 会先把行收走，那个计数与要守的性质无关。
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
        let mut db_option = get_db_option().clone();
        db_option.room_key_word = Some(vec![ROOM_KEY_WORD.to_string()]);

        let original_z = pos_z().await;
        println!("[issue7] 构件 {ELEMENT} 当前 POS.z = {original_z}");

        // ---- 1. 备料 + 2. 全量基线 ----
        let scope = scoped_panels().await;
        let outside = out_of_scope_edges(&scope).await;
        println!(
            "[issue7] 重建范围 {} 块面板；范围外归属边 {} 条（收尾写回）: {outside:?}",
            scope.len(),
            outside.len()
        );
        let mgr = AiosDBManager::init_form_config()
            .await
            .expect("init db manager");
        let baseline = build_room_baseline(&mgr, &db_option, &scope).await;
        println!("[issue7] 全量基线: {baseline:#?}");

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
            .query(format!("DELETE pe:{ELEMENT}<-room_relate;"))
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
        // 共享实库可能已有超过一页的房间积压；把本测试自己的确定性任务提到本轮首页，
        // 不删除也不改动别人的任务。
        SUL_DB
            .query(format!(
                "UPDATE model_update_pending:room_recalc_element_{ELEMENT} \
                 SET updated_at = d'1970-01-01T00:00:00Z';"
            ))
            .await
            .expect("prioritize target room task")
            .check()
            .expect("valid target room priority update");
        println!("[issue7] 移动后队列: {:?}", room_queue_rows().await);
        println!("[issue7] 移动后 aabb: {:?}", element_aabb_json().await);
        let moved = wait_for_room_convergence(&db_option).await;
        println!("[issue7] 移动后收敛: {}", moved.describe());
        let after_move = within_scope(&edges_of_element().await, &scope);

        // 收尾：位置写回原值，并把归属收敛回基线状态。范围外的边最后写回——排在任何一次
        // drain 之前写，等于把它再喂给元素分支抹一遍。
        set_pos_z(original_z).await;
        aios_core::clear_all_caches_batch(&[element]).await;
        mgr.update_world_transforms(&HashSet::from([element]))
            .await
            .expect("restore transform");
        let restore = wait_for_room_convergence(&db_option).await;
        println!("[issue7] 写回后收敛: {}", restore.describe());
        let restored = within_scope(&edges_of_element().await, &scope);
        restore_out_of_scope_edges(&outside).await;
        clear_room_queue().await;

        // 不变量排在最前：删掉的边有没有回来。收敛诊断塞进失败信息，任务卡住时一眼看得见
        // 是「没算」还是「算错」——反过来把收敛断言排在前面，一个与正确性无关的计数就能
        // 把唯一有意义的那条结论遮住，2026-08-19 那次红的就是这个形态。
        assert_eq!(
            after_move,
            baseline,
            "\n删掉的边必须被增量建回来（issue #7）\n增量: {after_move:#?}\n基线: {baseline:#?}\n收敛: {}",
            moved.describe()
        );
        assert_eq!(
            restored,
            baseline,
            "\n位置写回之后归属也要回到基线\n实得: {restored:#?}\n基线: {baseline:#?}\n收敛: {}",
            restore.describe()
        );
        assert!(
            moved.converged(),
            "那条元素任务必须被收口（谁收的不重要）: {}",
            moved.describe()
        );
        assert!(
            restore.converged(),
            "写回之后排出的元素任务同样要收口: {}",
            restore.describe()
        );
    }

    /// issue #13 C2：构件**移出**房间之后，归属边必须消失。
    ///
    /// 上面那条只覆盖了「移动 +100 之后仍回到 R512」——移完 AABB 的 `mins.z` 是 5907.67，
    /// 人还在房间里，验的是「边能回来」。反方向没人测：把它挪到房间外，那条边必须被清掉。
    /// 这一半失手同样是静默的——元素分支先清后写，清那一步只覆盖本轮候选面板，漏了就留下
    /// 一条指向它早已离开的房间的陈旧边，而任务照样成功。
    ///
    /// 基线走 [`build_room_baseline`] 自己铸，与 issue7 那条**没有执行顺序依赖**，可以
    /// 单独运行。
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
        let mut db_option = get_db_option().clone();
        db_option.room_key_word = Some(vec![ROOM_KEY_WORD.to_string()]);

        let original_z = pos_z().await;

        // 自足铸基线，不再要求「先跑 issue7 那条」：那个隐式顺序依赖让本用例在 2026-08-19
        // 的批次里因为兄弟用例失败而连带阻断，报的不是它自己要守的性质。见
        // [`build_room_baseline`]。
        let scope = scoped_panels().await;
        let outside = out_of_scope_edges(&scope).await;
        println!(
            "[issue13-c2] 重建范围 {} 块面板；范围外归属边 {} 条（收尾写回）: {outside:?}",
            scope.len(),
            outside.len()
        );
        let mgr = AiosDBManager::init_form_config()
            .await
            .expect("init db manager");
        let baseline = build_room_baseline(&mgr, &db_option, &scope).await;
        println!("[issue13-c2] 起点 POS.z={original_z} 归属={baseline:#?}");

        clear_room_queue().await;

        // 真的挪出去：+100 是上面那条用例的量级，构件还在房间里。
        set_pos_z(original_z + 100_000.0).await;
        aios_core::clear_all_caches_batch(&[element]).await;
        mgr.update_world_transforms(&HashSet::from([element]))
            .await
            .expect("transform work item");
        println!("[issue13-c2] 移出后队列: {:?}", room_queue_rows().await);
        println!("[issue13-c2] 移出后 aabb: {:?}", element_aabb_json().await);
        let moved_out = wait_for_room_convergence(&db_option).await;
        println!("[issue13-c2] 移出后收敛: {}", moved_out.describe());
        let after_out = within_scope(&edges_of_element().await, &scope);

        // 收尾：写回原位，把归属收敛回基线；范围外的边最后写回（理由同 issue7）。
        set_pos_z(original_z).await;
        aios_core::clear_all_caches_batch(&[element]).await;
        mgr.update_world_transforms(&HashSet::from([element]))
            .await
            .expect("restore transform");
        let restore = wait_for_room_convergence(&db_option).await;
        println!("[issue13-c2] 写回后收敛: {}", restore.describe());
        let restored = within_scope(&edges_of_element().await, &scope);
        restore_out_of_scope_edges(&outside).await;
        clear_room_queue().await;

        // 「边空了」这条断言本身分不清「删干净了」和「压根没算」，所以收敛诊断必须跟着
        // 一起报出来（房间自动化测试计划 RL2 的旁证要求）。
        assert!(
            after_out.is_empty(),
            "\n构件已经挪出 R512，它的归属边必须被清掉\n实得: {after_out:#?}\n收敛: {}",
            moved_out.describe()
        );
        assert_eq!(
            restored,
            baseline,
            "\n写回原位之后归属要回到基线\n实得: {restored:#?}\n基线: {baseline:#?}\n收敛: {}",
            restore.describe()
        );
        assert!(
            moved_out.converged(),
            "那条元素任务必须被收口（谁收的不重要）: {}",
            moved_out.describe()
        );
        assert!(
            restore.converged(),
            "写回之后排出的元素任务同样要收口: {}",
            restore.describe()
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
        let looked_up = current_regen_revision(ROOT)
            .await
            .expect("补查当前 revision");

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
