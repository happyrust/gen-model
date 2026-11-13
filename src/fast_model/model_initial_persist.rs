//! 初始化直写的模型侧渲染（ADR-077 D4 / D5）：一片切片 → 逐根 SQL → 按 8 MiB 装成若干笔事务文本。
//!
//! 这里是纯函数：不碰库、不碰 `ModelMemoryStore`、不登记 write-behind。产物是给落盘写者
//! （`data_interface::persist_writer`）的事务文本，加上提交成功后进空间树要用的盒子。
//!
//! 一根渲成的东西与增量路那一根**同一份行**（`model_db_adapter::render_records`，`WriteMode::Initial`：
//! 初始化前库里没有旧行，不做整片删、不读 `slice_sources_on`；「没有旧行」由初始化入口先清模型面保证，
//! 见 `dbnum_initialize::clear_model_face_before_initial`）+ 这一根的两张房间边（`room_stage` 的
//! 同一套算法，输入只有内存记录与任务开工冻住的计算器）。**没有回执行**（ADR-079：`gen_root` 退役）：
//! 「这个库整库生成完了」由初始化收尾那一笔盖 `model_sesno` 承诺，逐根成败只进进程内账本；零几何的根
//! 照样进笔（它的空间盒 / 房间边可能为空，但根名单要算进这一笔的账）。
//!
//! 装笔：一片一笔；渲出来超 [`PUBLICATION_STATEMENT_HARD_CAP`] 就按根拆成几笔、根不劈；单根超 cap 在这里
//! 就判死信、不进队列（ADR-077 D4 / D8）。

use aios_core::RefnoEnum;
use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use e3d_io::refno::RefNo;

use crate::data_interface::generation_root::refno_from_e3d;
use crate::fast_model::model_db_adapter::{
    PUBLICATION_STATEMENT_HARD_CAP, WriteMode, publication_transaction, render_records,
};
use crate::fast_model::model_memory_store::{ProjectionCommit, RootReceipt};
use crate::fast_model::room_model::MemoryRoomCalculator;

/// 一根渲好的份：几何行 + 房间边（`;\n` 收尾，不含事务包装）与回执（只作校验与账，不落库）。
pub(crate) struct RenderedRoot {
    pub root: RefNo,
    body: String,
    /// 这一根的回执：装笔时它的 `source_sesno` 随根名单进账（初始化收尾据此记账本）。
    pub receipt: RootReceipt,
    pub records: usize,
    pub room_edges: usize,
    /// 这一根随几何同笔写的 `pe_ptset` 行数（每个路由成员 / NOZZ 一行，没点的也算）。
    pub ptset_rows: usize,
    /// 这一根的空间树条目（世界盒，隐式管身不进树）。
    pub tree_boxes: Vec<RStarBoundingBox>,
    /// 这一根写了几何的来源元素（隐式管身不算）——树收敛的 `refreshed` 名单。
    pub refreshed: Vec<RefnoEnum>,
}

impl RenderedRoot {
    /// 装笔时的体量估计：几何行 + 房间边。
    fn estimated_bytes(&self) -> usize {
        self.body.len()
    }
}

/// 一笔要交给模型 lane 的事务：文本已经包好 `BEGIN … COMMIT`。
pub(crate) struct InitialTransaction {
    pub dbnum: u32,
    pub sql: String,
    /// 这一笔装的根，与各自回执的源会话号（提交成功后进程内账本记 `Generated` 用，ADR-079）。
    pub roots: Vec<RefNo>,
    pub root_sesnos: Vec<(RefNo, u32)>,
    pub records: usize,
    pub room_edges: usize,
    /// 这一笔装的 `pe_ptset` 行数（观测用）。
    pub ptset_rows: usize,
    pub tree_boxes: Vec<RStarBoundingBox>,
    pub refreshed: Vec<RefnoEnum>,
}

impl InitialTransaction {
    /// 给写者失败样例用的标签（ADR-077 D2：写者不认识根，根名单只在标签里）。
    pub(crate) fn label(&self) -> String {
        format!(
            "model slice dbnum={} roots=[{}]",
            self.dbnum,
            self.roots
                .iter()
                .map(|root| format!("{}/{}", root.word0, root.word1))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

/// 一片渲完的产物：若干笔事务 + 渲不出来的根（死信）。
#[derive(Default)]
pub(crate) struct RenderedSlice {
    pub transactions: Vec<InitialTransaction>,
    pub dead_letters: Vec<(RefNo, String)>,
}

/// 渲一根。`Err` = 这一根是死信（渲染期问题：房间算不出、单条记录 / 整根超 cap、`removals` 非空）。
pub(crate) fn render_root(
    root: RefNo,
    commit: &ProjectionCommit,
    rooms: Option<&MemoryRoomCalculator>,
    cap: usize,
) -> Result<RenderedRoot, String> {
    if !commit.removals.is_empty() {
        return Err(format!(
            "initialization commit of root {root} carries {} removal(s); a fresh dbnum has nothing to remove",
            commit.removals.len()
        ));
    }
    let (receipt_root, receipt) = commit
        .receipt
        .as_ref()
        .ok_or_else(|| format!("initialization commit of root {root} carries no receipt"))?;
    if *receipt_root != root {
        return Err(format!(
            "initialization commit of root {root} carries a receipt for {receipt_root}"
        ));
    }
    let records = &commit.upserts;
    let mut body = if records.is_empty() {
        String::new()
    } else {
        render_records(records, WriteMode::Initial)
            .map_err(|error| format!("render root {root}: {error:#}"))?
    };
    // 房间关卡（ADR-073 D1 / D2）：算不出来 = 该根死信，同片其余根照常。
    let room_edges = match rooms {
        Some(calculator) if !records.is_empty() => {
            let edges = calculator.calculate_for_root(records).map_err(|error| {
                format!("room membership of root {root} could not be computed: {error:#}")
            })?;
            body.push_str(&calculator.render_root_room_edges(records, &edges));
            edges.len()
        }
        _ => 0,
    };
    // `pe_ptset` 行与房间边同一个位置：这一根的几何之后、同一笔（ptset 分表计划 W2，与 `model_db_adapter::persist_group`
    // 同一条规矩）。初始化前库里没有旧行（重初始化的整库 wipe 把 `pe_ptset` 一起按 Ref0 区间删了），所以不清只写；
    // 没带点集的提交（盘上没这一格的旧产物）一行不写。
    let ptset_rows = match commit.point_sets.as_ref() {
        Some(point_sets) => {
            body.push_str(&crate::fast_model::ptset_rows::render_rows(
                receipt.source_sesno,
                point_sets,
            ));
            point_sets.len()
        }
        None => 0,
    };
    let rendered = RenderedRoot {
        root,
        body,
        receipt: receipt.clone(),
        records: records.len(),
        room_edges,
        ptset_rows,
        tree_boxes: records
            .iter()
            .filter_map(crate::fast_model::aabb_tree::projected_tree_entry)
            .collect(),
        refreshed: records
            .iter()
            .filter(|record| record.tube.is_none())
            .map(|record| refno_from_e3d(record.source_refno))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    let bytes = rendered.estimated_bytes();
    if bytes > cap {
        return Err(format!(
            "root {root} renders to {bytes} bytes, over the {cap} byte transaction cap"
        ));
    }
    Ok(rendered)
}

/// 渲一片：逐根渲、按 `cap` 贪心装笔。空的根（零几何、有回执）照样进笔——它要随这一笔的
/// 根名单进账（ADR-079：账本与初始化收尾的 `model_sesno` 是它「生成过」的唯一说法）。
pub(crate) fn render_slice(
    dbnum: u32,
    commits: &[(RefNo, ProjectionCommit)],
    rooms: Option<&MemoryRoomCalculator>,
    cap: usize,
) -> RenderedSlice {
    let mut slice = RenderedSlice::default();
    let mut rendered = Vec::with_capacity(commits.len());
    for (root, commit) in commits {
        match render_root(*root, commit, rooms, cap) {
            Ok(root) => rendered.push(root),
            Err(reason) => slice.dead_letters.push((*root, reason)),
        }
    }
    slice.transactions = pack_transactions(dbnum, rendered, cap);
    slice
}

/// 默认上限就是那 8 MiB 硬顶。
pub(crate) fn default_transaction_cap() -> usize {
    PUBLICATION_STATEMENT_HARD_CAP
}

/// 按字节贪心装笔：装不下就封当前这一笔另起一笔；每一根都已在 [`render_root`] 里保证单独装得下。
fn pack_transactions(dbnum: u32, roots: Vec<RenderedRoot>, cap: usize) -> Vec<InitialTransaction> {
    let mut transactions = Vec::new();
    let mut current: Vec<RenderedRoot> = Vec::new();
    let mut current_bytes = 0usize;
    for root in roots {
        let bytes = root.estimated_bytes();
        if !current.is_empty() && current_bytes + bytes > cap {
            transactions.push(seal(dbnum, std::mem::take(&mut current)));
            current_bytes = 0;
        }
        current_bytes += bytes;
        current.push(root);
    }
    if !current.is_empty() {
        transactions.push(seal(dbnum, current));
    }
    transactions
}

/// 一笔：各根的几何行 + 房间边 → `spatial_epoch` +1 → 包上事务。
///
/// 没有回执行（ADR-079）：一笔原子，行要么全落要么全不落；「这一笔成了」由写者回执说，成了才把
/// 根名单记进账本。`spatial_epoch` 每笔 +1（ADR-071 D9：凡改变树应有内容的已提交变更都在同一事务里 +1）。
fn seal(dbnum: u32, roots: Vec<RenderedRoot>) -> InitialTransaction {
    let mut bodies = String::new();
    let mut transaction = InitialTransaction {
        dbnum,
        sql: String::new(),
        roots: Vec::with_capacity(roots.len()),
        root_sesnos: Vec::with_capacity(roots.len()),
        records: 0,
        room_edges: 0,
        ptset_rows: 0,
        tree_boxes: Vec::new(),
        refreshed: Vec::new(),
    };
    for root in roots {
        bodies.push_str(&root.body);
        transaction.roots.push(root.root);
        transaction
            .root_sesnos
            .push((root.root, root.receipt.source_sesno));
        transaction.records += root.records;
        transaction.room_edges += root.room_edges;
        transaction.ptset_rows += root.ptset_rows;
        transaction.tree_boxes.extend(root.tree_boxes);
        transaction.refreshed.extend(root.refreshed);
    }
    let mut statements = bodies;
    statements.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
    statements.push('\n');
    transaction.sql = publication_transaction(&statements);
    transaction
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::model_memory_store::{
        CommitScope, ProjectedGeometry, ProjectedShape, packed_refno,
    };
    use e3d_model::GeometryId;
    use glam::DMat4;
    use surrealdb::{Surreal, engine::any::Any};

    const ROOT: RefNo = RefNo::new(24383, 73948);
    const OTHER: RefNo = RefNo::new(24383, 80000);

    fn record(root: RefNo, ordinal: u32) -> ProjectedGeometry {
        let refno = RefNo::new(root.word0, root.word1 + ordinal);
        ProjectedGeometry {
            record_id: format!("{}_{}", refno.word0, refno.word1),
            geometry_id: GeometryId::Element {
                refno: format!("{}/{}", refno.word0, refno.word1),
            },
            source_refno: refno,
            dbnum: 7997,
            source_sesno: 42,
            noun: "EQUI".into(),
            mesh_id: format!("e3d_baked_v2_{ordinal:08x}"),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0, 0.0, 0.0], [1.0, 2.0, 3.0]],
            ancestors: vec![
                packed_refno(refno),
                packed_refno(root),
                packed_refno(RefNo::new(24383, 1)),
            ],
            shape: ProjectedShape::Baked,
            tube: None,
        }
    }

    fn commit(root: RefNo, records: usize) -> ProjectionCommit {
        let upserts = (0..records as u32)
            .map(|n| record(root, n))
            .collect::<Vec<_>>();
        ProjectionCommit {
            tube_branch_resets: Vec::new(),
            removals: Vec::new(),
            point_sets: None,
            receipt: Some((
                root,
                RootReceipt {
                    source_sesno: 42,
                    model_target: None,
                    manifest_hash: format!("test-{records}"),
                    geometry_count: records,
                    published_via: "regen".into(),
                    room: None,
                },
            )),
            upserts,
            scope: CommitScope::WholeRoot,
        }
    }

    async fn database(label: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("model_initial_persist")
            .use_db(label)
            .await
            .unwrap();
        db
    }

    #[derive(serde::Deserialize)]
    struct Count {
        n: i64,
    }

    async fn count(db: &Surreal<Any>, sql: &str) -> i64 {
        let rows: Vec<Count> = db
            .query(sql)
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        rows.first().map(|row| row.n).unwrap_or(0)
    }

    /// ADR-077 D5 (a) / ADR-079：空库上发一根，行落了；库里**没有**逐根回执行（`gen_root` 退役），
    /// 这一笔的根名单与源会话号随事务交回，提交成功后由编排记进进程内账本。
    #[tokio::test]
    async fn one_root_on_an_empty_database_lands_rows_and_carries_its_root_for_the_ledger() {
        let db = database("one_root").await;
        let slice = render_slice(
            7997,
            &[(ROOT, commit(ROOT, 3))],
            None,
            default_transaction_cap(),
        );
        assert!(slice.dead_letters.is_empty(), "{:?}", slice.dead_letters);
        assert_eq!(slice.transactions.len(), 1);
        let tx = &slice.transactions[0];
        assert_eq!(tx.roots, vec![ROOT]);
        assert_eq!(tx.root_sesnos, vec![(ROOT, 42)]);
        assert_eq!(tx.records, 3);
        assert_eq!(tx.tree_boxes.len(), 3);
        assert_eq!(tx.refreshed.len(), 3);
        assert!(tx.label().contains("roots=[24383/73948]"), "{}", tx.label());
        assert!(tx.sql.starts_with("BEGIN TRANSACTION;"), "{}", tx.sql);
        assert!(!tx.sql.contains("DELETE"), "初始化不删：{}", tx.sql);
        assert!(
            !tx.sql.contains("gen_root"),
            "回执不落库（ADR-079）：{}",
            tx.sql
        );
        db.query(tx.sql.clone()).await.unwrap().check().unwrap();
        assert_eq!(
            count(&db, "SELECT count() AS n FROM inst_relate GROUP ALL;").await,
            3
        );
        #[derive(serde::Deserialize)]
        struct Epoch {
            value: u64,
        }
        let epoch: Vec<Epoch> = db
            .query("SELECT `value` FROM spatial_epoch:current;")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(epoch[0].value, 1, "每笔 +1");
    }

    /// ptset 分表（W2）：初始化直写这条路也把根的 `pe_ptset` 行装进同一笔——整库初始化走的是这里而不是 write-behind，
    /// 漏了这一条，刚初始化完的库一行 P 点都没有（2026-10-27 在 8000 的临时库上实测到的形状）。没带点集的提交不写。
    #[tokio::test]
    async fn point_sets_land_in_the_same_initialization_transaction_as_the_geometry() {
        use crate::fast_model::ptset_rows::{ProjectedPoint, ProjectedPointSet};
        let db = database("ptset").await;
        let mut with_points = commit(ROOT, 1);
        with_points.point_sets = Some(vec![ProjectedPointSet {
            root: ROOT,
            refno: RefNo::new(ROOT.word0, ROOT.word1 + 7),
            noun: "ELBO".into(),
            points: vec![ProjectedPoint {
                number: 1,
                noun: "PTAX".into(),
                pt: [0.0, 0.0, 0.0],
                dir: [0.0, 0.0, -1.0],
                bore: Some(100.0),
            }],
            unresolved: Vec::new(),
            reason: None,
        }]);
        let slice = render_slice(
            7997,
            &[(ROOT, with_points), (OTHER, commit(OTHER, 1))],
            None,
            default_transaction_cap(),
        );
        assert!(slice.dead_letters.is_empty(), "{:?}", slice.dead_letters);
        assert_eq!(slice.transactions.len(), 1);
        let tx = &slice.transactions[0];
        assert_eq!(tx.ptset_rows, 1);
        assert!(
            tx.sql.contains("UPSERT pe_ptset:⟨24383_73955⟩ CONTENT { pe: pe:⟨24383_73955⟩, root: pe:⟨24383_73948⟩"),
            "{}",
            tx.sql
        );
        assert!(!tx.sql.contains("DELETE"), "初始化不清旧行：{}", tx.sql);
        db.query(tx.sql.clone()).await.unwrap().check().unwrap();
        assert_eq!(
            count(&db, "SELECT count() AS n FROM inst_relate GROUP ALL;").await,
            2
        );
        assert_eq!(
            count(&db, "SELECT count() AS n FROM pe_ptset GROUP ALL;").await,
            1
        );
        assert_eq!(
            count(
                &db,
                "SELECT count() AS n FROM pe_ptset WHERE sesno = 42 GROUP ALL;"
            )
            .await,
            1,
            "行上的 sesno 是回执的 source_sesno"
        );
    }

    /// 零几何的根也要进笔、随根名单进账（`geometry_count = 0`）：它没有行可落，但「生成过」这件事
    /// 要在这一笔提交之后进账本，任务收尾数「一根不剩」时才算得上它。
    #[tokio::test]
    async fn a_root_without_geometry_still_rides_a_transaction_for_the_ledger() {
        let db = database("empty_root").await;
        let slice = render_slice(
            7997,
            &[(ROOT, commit(ROOT, 0))],
            None,
            default_transaction_cap(),
        );
        assert!(slice.dead_letters.is_empty());
        assert_eq!(slice.transactions.len(), 1);
        assert_eq!(slice.transactions[0].roots, vec![ROOT]);
        assert_eq!(slice.transactions[0].root_sesnos, vec![(ROOT, 42)]);
        db.query(slice.transactions[0].sql.clone())
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            count(&db, "SELECT count() AS n FROM inst_relate GROUP ALL;").await,
            0
        );
    }

    /// 装笔：两根装得下就一笔；上限压到刚够一根时拆成两笔，根不劈。
    #[test]
    fn roots_pack_into_one_transaction_or_split_by_root_under_the_cap() {
        let commits = vec![(ROOT, commit(ROOT, 2)), (OTHER, commit(OTHER, 2))];
        let one = render_slice(7997, &commits, None, default_transaction_cap());
        assert_eq!(one.transactions.len(), 1);
        assert_eq!(one.transactions[0].roots, vec![ROOT, OTHER]);
        assert_eq!(one.transactions[0].records, 4);

        let per_root = render_root(ROOT, &commits[0].1, None, default_transaction_cap())
            .unwrap()
            .estimated_bytes();
        let tight = render_slice(7997, &commits, None, per_root + per_root / 2);
        assert!(tight.dead_letters.is_empty(), "{:?}", tight.dead_letters);
        assert_eq!(tight.transactions.len(), 2, "一根一笔");
        assert_eq!(tight.transactions[0].roots, vec![ROOT]);
        assert_eq!(tight.transactions[1].roots, vec![OTHER]);
    }

    /// 单根超 cap = 死信，不进任何一笔（ADR-077 D8：渲染期问题归根，不归 lane）。
    #[test]
    fn an_oversized_root_is_a_dead_letter_and_never_enters_the_queue() {
        let commits = vec![(ROOT, commit(ROOT, 2)), (OTHER, commit(OTHER, 1))];
        let small = render_root(OTHER, &commits[1].1, None, default_transaction_cap())
            .unwrap()
            .estimated_bytes();
        let slice = render_slice(7997, &commits, None, small);
        assert_eq!(slice.dead_letters.len(), 1);
        assert_eq!(slice.dead_letters[0].0, ROOT);
        assert!(
            slice.dead_letters[0].1.contains("over the"),
            "{}",
            slice.dead_letters[0].1
        );
        assert_eq!(slice.transactions.len(), 1);
        assert_eq!(slice.transactions[0].roots, vec![OTHER]);
    }

    /// 初始化的提交不该带 removals，也不该没有回执——带了就是上游接错了路，判死信而不是静默落。
    #[test]
    fn a_commit_with_removals_or_without_a_receipt_is_refused() {
        let mut with_removals = commit(ROOT, 1);
        with_removals.removals.push("24383_1".into());
        let error = render_root(ROOT, &with_removals, None, default_transaction_cap())
            .err()
            .expect("removals are refused");
        assert!(error.contains("removal"), "{error}");

        let mut without_receipt = commit(ROOT, 1);
        without_receipt.receipt = None;
        let error = render_root(ROOT, &without_receipt, None, default_transaction_cap())
            .err()
            .expect("a missing receipt is refused");
        assert!(error.contains("no receipt"), "{error}");
    }

    /// `ModelTarget` 不再进 SQL（ADR-079：它只活在回执 / 账本里）：带目标的两根渲出来的一笔
    /// 只有几何行与 `spatial_epoch`，一个 `LET $mt` 都没有。
    #[test]
    fn a_model_target_never_reaches_the_transaction_text() {
        use crate::fast_model::e3d_model_service::{ModelFileTarget, ModelTarget};
        let target = ModelTarget {
            project: "AMS".into(),
            design: ModelFileTarget {
                dbnum: 7997,
                db_type: "DESI".into(),
                file: "ams7997".into(),
                session: 42,
                digest: "abc".into(),
            },
            catalogue: Vec::new(),
            template_attlib_digest: "t".into(),
            generator_fingerprint: "g".into(),
            tessellation_profile: "p".into(),
        };
        let mut a = commit(ROOT, 1);
        a.receipt.as_mut().unwrap().1.model_target = Some(target.clone());
        let mut b = commit(OTHER, 1);
        b.receipt.as_mut().unwrap().1.model_target = Some(target);
        let slice = render_slice(
            7997,
            &[(ROOT, a), (OTHER, b)],
            None,
            default_transaction_cap(),
        );
        let sql = &slice.transactions[0].sql;
        for forbidden in [
            "LET $mt",
            "desired_model_target",
            "gen_root",
            "root_dependency",
        ] {
            assert!(!sql.contains(forbidden), "{forbidden} in {sql}");
        }
        assert_eq!(slice.transactions[0].roots, vec![ROOT, OTHER]);
    }

    /// 本库 `pe` 为 0、模型面却留着一行带几何指针、没有 `model_sesno` 的 legacy `inst_relate`（T-D-6）。
    const LEGACY_POINTER_ROW: &str = "INSERT INTO inst_relate [{id: inst_relate:⟨24383_73948⟩, \
        in: pe:⟨24383_73948⟩, dbnum: 7997, generic: 'EQUI', aabb: aabb:legacybox, \
        world_trans: trans:legacytrans}];";

    #[derive(serde::Deserialize)]
    struct RelationRow {
        aabb: String,
        #[serde(default)]
        model_sesno: Option<i64>,
    }

    async fn relation_row(db: &Surreal<Any>) -> RelationRow {
        let rows: Vec<RelationRow> = db
            .query("SELECT record::id(aabb) AS aabb, model_sesno FROM inst_relate:⟨24383_73948⟩;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        rows.into_iter().next().expect("relation row")
    }

    /// T-D-6：`Initial` 直写是 `INSERT IGNORE`，不先清就让同 id 的 legacy 行赢（旧盒指针、没有
    /// `model_sesno`）；初始化入口先清模型面（Ref0 由本库 `pe` 行圈到；`pe` 也空时改由残留行自己的 id
    /// 圈、按位解码核对归属，见下两条）之后，同一笔落得进去。
    #[tokio::test]
    async fn the_initial_write_replaces_a_legacy_pointer_row_once_the_entry_clears_the_model_face()
    {
        use crate::data_interface::dbnum_initialize::clear_model_face_before_initial_on;
        let slice = render_slice(
            7997,
            &[(ROOT, commit(ROOT, 1))],
            None,
            default_transaction_cap(),
        );
        let sql = slice.transactions[0].sql.clone();

        let uncleared = database("legacy_uncleared").await;
        uncleared
            .query(LEGACY_POINTER_ROW)
            .await
            .unwrap()
            .check()
            .unwrap();
        uncleared.query(sql.clone()).await.unwrap().check().unwrap();
        let stale = relation_row(&uncleared).await;
        assert_eq!(stale.aabb, "legacybox", "不清就直写：旧指针赢");
        assert_eq!(stale.model_sesno, None);

        let db = database("legacy_cleared").await;
        db.query(LEGACY_POINTER_ROW).await.unwrap().check().unwrap();
        db.query("CREATE pe:⟨24383_73948⟩ SET dbnum = 7997, noun = 'EQUI';")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            count(&db, "SELECT count() AS n FROM pe GROUP ALL;").await,
            1
        );
        let cleared = clear_model_face_before_initial_on(&db, 7997).await.unwrap();
        assert_eq!(cleared, 1, "清之前模型面只有那一行 legacy");
        db.query(sql).await.unwrap().check().unwrap();
        let fresh = relation_row(&db).await;
        assert_eq!(fresh.aabb, "24383_73948", "关系行换成这次生成的盒指针");
        assert_eq!(fresh.model_sesno, Some(42));
    }

    /// 模型面空就什么都不做。本库 `pe` 一行都没有、残留行的 Ref0 又不是本库的（`LEGACY_POINTER_ROW`
    /// 写着 `dbnum: 7997`，id 却在 24383 下——按位解码那是 7999 的 Ref0）时，没有任何东西能证明这段
    /// 区间归本库：入口拒绝开工、点名，一行不删，而不是把直写压在旧行上、或按区间删到别的库。
    #[tokio::test]
    async fn the_entry_refuses_an_initial_write_over_a_model_face_it_cannot_clear() {
        use crate::data_interface::dbnum_initialize::clear_model_face_before_initial_on;
        let empty = database("model_face_empty").await;
        assert_eq!(
            clear_model_face_before_initial_on(&empty, 7997)
                .await
                .unwrap(),
            0
        );

        let db = database("legacy_unscoped").await;
        db.query(LEGACY_POINTER_ROW).await.unwrap().check().unwrap();
        let error = clear_model_face_before_initial_on(&db, 7997)
            .await
            .expect_err("圈不到 Ref0 就拒绝开工");
        let message = format!("{error:#}");
        assert!(message.contains("仍有 1 行"), "{message}");
        assert!(
            message.contains("Ref0 24383") && message.contains("dbnum 7999"),
            "要点名是哪个 Ref0、它按位解码是谁的: {message}"
        );
        assert_eq!(
            count(&db, "SELECT count() AS n FROM inst_relate GROUP ALL;").await,
            1
        );
    }

    /// 2026-11-10 / 11 的 7997：本库 `pe` 一行都没有，模型面却有一整套行——按需生成对还没初始化的库
    /// 照样落库，初始化中途库进程死掉也会留下。`pe` 圈不到 Ref0，入口此前只会拒绝，于是这个库每次
    /// 启动都初始化失败、停跑，只能人工清库。现在 Ref0 从残留行自己的 id 取（`tubi_relate` 的 id 是
    /// 复合键，取 `in`），按位解码确认是本库的才进区间：两个 extent 下的行都清掉，管身边按 `dbnum`
    /// 列删，别的库一行不动，模型水位那半句照常复位；清完同 id 的直写落得进去。
    #[tokio::test]
    async fn the_entry_clears_a_model_face_that_no_pe_row_vouches_for() {
        use crate::data_interface::dbnum_initialize::clear_model_face_before_initial_on;
        // 24381 / 16189 是 7997 的两个 extent（`refno_route`：低 13 位 0x1F3D）；24384 是 8000 的。
        const MEMBER: RefNo = RefNo::new(24381, 100);
        let db = database("orphan_model_face").await;
        db.query(
            "INSERT INTO inst_relate [\
                {id: inst_relate:⟨24381_100⟩, in: pe:⟨24381_100⟩, dbnum: 7997, generic: 'EQUI', \
                 aabb: aabb:⟨24381_100⟩, world_trans: trans:⟨24381_100⟩, model_sesno: 447},\
                {id: inst_relate:⟨16189_5⟩, in: pe:⟨16189_5⟩, dbnum: 7997, generic: 'EQUI', \
                 model_sesno: 447},\
                {id: inst_relate:⟨24384_1⟩, in: pe:⟨24384_1⟩, dbnum: 8000, generic: 'EQUI', \
                 aabb: aabb:⟨24384_1⟩, model_sesno: 1139}];\
             INSERT RELATION INTO tubi_relate [\
                {id: tubi_relate:[pe:⟨24381_200⟩, 0], in: pe:⟨24381_200⟩, out: inst_geo:tube, dbnum: 7997},\
                {id: tubi_relate:[pe:⟨24384_2⟩, 0], in: pe:⟨24384_2⟩, out: inst_geo:tube, dbnum: 8000}];\
             CREATE aabb:⟨24381_100⟩ SET d = [0, 0, 0, 1, 1, 1];\
             CREATE trans:⟨24381_100⟩ SET d = [1];\
             CREATE inst_info:⟨24381_100⟩ SET dbnum = 7997;\
             CREATE pe_ptset:⟨24381_100⟩ SET points = [];\
             CREATE aabb:⟨24384_1⟩ SET d = [0, 0, 0, 1, 1, 1];\
             CREATE dbnum_watermark:7997 SET applied_sesno = 0, model_sesno = 0, state_revision = 4;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        assert_eq!(
            count(&db, "SELECT count() AS n FROM pe GROUP ALL;").await,
            0,
            "前提：本库 pe 一行都没有"
        );

        let cleared = clear_model_face_before_initial_on(&db, 7997).await.unwrap();
        assert_eq!(
            cleared, 3,
            "按 dbnum 列看见的残留：两行 inst_relate、一条管身边"
        );

        for (label, sql) in [
            (
                "inst_relate",
                "SELECT count() AS n FROM inst_relate WHERE dbnum = 7997 GROUP ALL;",
            ),
            (
                "tubi_relate",
                "SELECT count() AS n FROM tubi_relate WHERE dbnum = 7997 GROUP ALL;",
            ),
            (
                "aabb",
                "SELECT count() AS n FROM aabb:⟨24381_100⟩ GROUP ALL;",
            ),
            (
                "trans",
                "SELECT count() AS n FROM trans:⟨24381_100⟩ GROUP ALL;",
            ),
            (
                "inst_info",
                "SELECT count() AS n FROM inst_info:⟨24381_100⟩ GROUP ALL;",
            ),
            (
                "pe_ptset",
                "SELECT count() AS n FROM pe_ptset:⟨24381_100⟩ GROUP ALL;",
            ),
        ] {
            assert_eq!(count(&db, sql).await, 0, "{label} 的本库残留要清掉");
        }
        for (label, sql) in [
            (
                "inst_relate",
                "SELECT count() AS n FROM inst_relate WHERE dbnum = 8000 GROUP ALL;",
            ),
            (
                "tubi_relate",
                "SELECT count() AS n FROM tubi_relate WHERE dbnum = 8000 GROUP ALL;",
            ),
            ("aabb", "SELECT count() AS n FROM aabb:⟨24384_1⟩ GROUP ALL;"),
        ] {
            assert_eq!(count(&db, sql).await, 1, "别的库的 {label} 不许动");
        }
        assert_eq!(
            count(&db, "SELECT state_revision AS n FROM dbnum_watermark:7997;").await,
            5,
            "模型水位那半句照常复位、state_revision 前进一步"
        );

        // 清完之后同 id 的直写落得进去：行上是这次生成的会话号，不是残留的 447。
        let slice = render_slice(
            7997,
            &[(MEMBER, commit(MEMBER, 1))],
            None,
            default_transaction_cap(),
        );
        assert!(slice.dead_letters.is_empty(), "{:?}", slice.dead_letters);
        db.query(slice.transactions[0].sql.clone())
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            count(&db, "SELECT model_sesno AS n FROM inst_relate:⟨24381_100⟩;").await,
            42
        );
    }

    /// 清的时候还有行落进来：按需生成不等初始化，它发布的根由写后台随时落库（2026-11-11 用户定：基线没落的库上
    /// 按需生成的结果也写库，由初始化接住）。一批恰好落在「清完」与「复查」之间时，入口再圈一遍、再清一遍
    /// （归属照样逐个核对），而不是报「清不干净」把这次初始化判败；连着每一遍都有新行落进来才拒绝开工。
    /// 测试钩在每遍清完、复查之前往库里补一行。
    #[tokio::test]
    async fn the_entry_clears_again_when_rows_land_between_the_wipe_and_the_recheck() {
        use crate::data_interface::dbnum_initialize::{
            ENTRY_CLEAR_PASSES, ROWS_LANDING_DURING_ENTRY_CLEAR, clear_model_face_before_initial_on,
        };
        let landing = |key: &str| {
            format!(
                "CREATE inst_relate:⟨{key}⟩ SET in = pe:⟨{key}⟩, dbnum = 7997, model_sesno = 447;"
            )
        };
        async fn seeded(label: &str) -> Surreal<Any> {
            let db = database(label).await;
            db.query(
                "CREATE inst_relate:⟨24381_100⟩ SET in = pe:⟨24381_100⟩, dbnum = 7997, model_sesno = 447;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
            db
        }
        const LEFT: &str = "SELECT count() AS n FROM inst_relate WHERE dbnum = 7997 GROUP ALL;";

        // 落进来一批：第二遍清掉，入口放行。落在另一个 extent（16189）上的照样圈得到。
        let db = seeded("landing_once").await;
        let cleared = ROWS_LANDING_DURING_ENTRY_CLEAR
            .scope(
                std::cell::RefCell::new(vec![landing("16189_5")]),
                clear_model_face_before_initial_on(&db, 7997),
            )
            .await
            .unwrap();
        assert_eq!(cleared, 2, "第一遍 1 行、第二遍 1 行");
        assert_eq!(count(&db, LEFT).await, 0);

        // 每一遍都有新行落进来：清满遍数之后拒绝开工，说清是怎么回事。
        let db = seeded("landing_always").await;
        let rows = (0..ENTRY_CLEAR_PASSES)
            .map(|n| landing(&format!("24381_{}", 200 + n)))
            .collect::<Vec<_>>();
        let error = ROWS_LANDING_DURING_ENTRY_CLEAR
            .scope(
                std::cell::RefCell::new(rows),
                clear_model_face_before_initial_on(&db, 7997),
            )
            .await
            .expect_err("一直有行落进来");
        let message = format!("{error:#}");
        assert!(
            message.contains(&format!("连清 {ENTRY_CLEAR_PASSES} 遍"))
                && message.contains("仍有 1 行"),
            "{message}"
        );
        assert_eq!(count(&db, LEFT).await, 1);

        // 落进来的行在别的库的 Ref0 上（`dbnum` 列与 id 对不上）：第二遍圈范围时照样拒绝、点名，那一行不删。
        let db = seeded("landing_foreign").await;
        let error = ROWS_LANDING_DURING_ENTRY_CLEAR
            .scope(
                std::cell::RefCell::new(vec![landing("24383_7")]),
                clear_model_face_before_initial_on(&db, 7997),
            )
            .await
            .expect_err("别的库的 Ref0");
        let message = format!("{error:#}");
        assert!(
            message.contains("Ref0 24383") && message.contains("dbnum 7999"),
            "{message}"
        );
        assert_eq!(count(&db, LEFT).await, 1);
    }
}
