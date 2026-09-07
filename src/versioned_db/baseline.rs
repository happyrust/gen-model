//! 基线写者的渲染半边：把 [`BaselineChunk`]（e3d-io 退化窗口流出的一块 `Add`）渲成
//! 有界队列里的写者消息——**每条消息就是一笔完整事务**。
//!
//! 出处：`docs/plans/2026-09-04-legacy-parser-retirement-endgame-plan.md` §2 / B8.3；
//! 2026-09-06 并轨到 `element_batch`（plan 2026-09-06 §3 D2 / P2）。
//!
//! * **行形状与语句形状都与增量路同一份**（`element_batch`：`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`，
//!   每表一条 `INSERT IGNORE INTO … [..]`），宪法 II 从「行形状一份」扩到「语句形状一份」。
//! * **`Fresh` 模式**：只插、不删。不是只为吞吐——`manual_update` 的父层补缺把 extract 母库整库再同步进
//!   同一个 dbnum（`replace_dbs = false`），靠的就是「已有行赢」：叶子 extract 里正在改的
//!   版本不许被母库的旧版本覆盖。`Replace` 会把这条语义改掉。
//! * 事务切块按 `tx_limits`（默认 2 MiB / 5000 行 / 1000 元素）；`DbOption::pe_chunk / att_chunk` 不再参与切块。
//! * **不带逐笔 pe 统计观测**（`StatisticsMode::Deferred`，2026-09-07）：整库基线写进刚清空的 dbnum，
//!   `$before` 恒空、`$after` 恒等于本批，逐笔观测的点读量与正文同量级、UPSERT 的又是同一个热点行；
//!   写者排空后 `database::settle_dbnum_info_after_total_sync` 用 `dbnum_stats::rebuild_body` 一次算清。
//!   父层补缺（`replace_dbs = false`）走的也是这条路——同一个收尾重建对它同样成立（GROUP BY 数的是实际行）。
//! * WORL 作为普通元素写全三样行（legacy 不写它的 `pe` 行），计数口径见
//!   [`BaselineStats::elements`]。

use aios_core::RefU64;

use crate::data_interface::e3d_collector::BaselineChunk;
use crate::data_interface::ele_ops::{ElementRows, UdaKeyMap};
use crate::data_interface::element_batch::{
    BatchItem, ElementBatchReport, StatisticsMode, WriteMode,
    render_item_transactions_with_limits_and_statistics,
};
use crate::data_interface::tx_limits::{DataTxLimits, data_tx_limits};

/// Resolve a baseline input before crossing a spawned parser boundary.
pub(crate) fn source_for_file(
    path: &std::path::Path,
    target: Option<u32>,
    active: Option<&crate::data_interface::frozen_source::FrozenSource>,
) -> anyhow::Result<crate::data_interface::frozen_source::FrozenSource> {
    use crate::data_interface::frozen_source::FrozenSource;
    if let Some(active) = active {
        if active.matches_path(path) {
            anyhow::ensure!(
                target.is_none_or(|session| session == active.target_sesno()),
                "baseline target differs from active frozen file round"
            );
            return active.reopen_verified();
        }
    }
    match target {
        Some(session) => FrozenSource::capture_at(path, session),
        None => FrozenSource::capture(path),
    }
}

#[cfg(test)]
#[path = "baseline_source_tests.rs"]
mod baseline_source_tests;

/// 一块基线元素 → 若干笔事务 SQL（`Fresh`：只 `INSERT IGNORE`，已有行赢）。
pub fn render_baseline_batches(
    chunk: &BaselineChunk,
    dbnum: i32,
    uda_keys: &UdaKeyMap,
) -> anyhow::Result<(Vec<String>, ElementBatchReport)> {
    render_baseline_batches_with_limits(chunk, dbnum, uda_keys, data_tx_limits())
}

/// [`render_baseline_batches`] 的显式上限形态（测试把元素上限压小看切块）。
pub fn render_baseline_batches_with_limits(
    chunk: &BaselineChunk,
    dbnum: i32,
    uda_keys: &UdaKeyMap,
    limits: DataTxLimits,
) -> anyhow::Result<(Vec<String>, ElementBatchReport)> {
    let items = chunk
        .elements
        .iter()
        .map(|(sesno, element)| BatchItem::Rows {
            rows: ElementRows::render(element, &element_id(element), dbnum, *sesno, uda_keys),
            is_add: false,
            old_noun: None,
            dependency: false,
        })
        .collect::<Vec<_>>();
    render_item_transactions_with_limits_and_statistics(
        &items,
        dbnum,
        WriteMode::Fresh,
        limits,
        StatisticsMode::Deferred,
    )
}

/// 行 id 与增量路同一口径：`PersistElementChange::id` 用 `refno.to_string()`，
/// 而渲染器内部的 id / refno / owner 又取自属性映射的 `REFNO`——两者在收集器产物上恒等。
fn element_id(element: &crate::data_interface::ele_data::EleData) -> String {
    let refno: RefU64 = element.refno;
    refno.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::ele_data::EleData;
    use aios_core::NamedAttrValue;
    use aios_core::RefU64Vec;
    use aios_core::types::whole_attmap::ExplicitAttr;

    const DBNUM: i32 = 8000;

    /// 基线事务不带逐笔统计观测（`Deferred`），统计由收尾的 `rebuild_on` 一次算清——三轮（基线 + 两次
    /// 父层补缺重放）之后 `dbnum_info_table` 的 count 仍等于实际 pe 行数，且**写块自身**不碰统计表。
    #[tokio::test]
    async fn baseline_and_parent_gap_replay_keep_actual_pe_counts() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("stats").use_db("baseline").await.unwrap();
        for (sesno, numbers) in [(5, vec![1, 2]), (2, vec![2, 3]), (2, vec![2, 3])] {
            let chunk = BaselineChunk {
                elements: numbers
                    .into_iter()
                    .map(|n| (sesno, element(refu(n), refu(0), "BOX", &[])))
                    .collect(),
                ignored: Vec::new(),
            };
            for sql in batches_of(&chunk) {
                assert!(
                    !sql.contains("dbnum_info_table") && !sql.contains("$before_"),
                    "基线写块不带观测:\n{sql}"
                );
                crate::data_interface::dbnum_stats::execute_on(&db, &sql)
                    .await
                    .unwrap();
            }
            // 与 `settle_dbnum_info_after_total_sync` 同一步：写者排空后按 Ref0 重建统计。
            crate::data_interface::dbnum_stats::rebuild_on(&db, DBNUM as u32)
                .await
                .unwrap();
        }
        let mut result = db
            .query("RETURN dbnum_info_table:8000.count; RETURN pe:8000_2.sesno;")
            .await
            .unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(3));
        assert_eq!(
            result.take::<Option<i64>>(1).unwrap(),
            Some(5),
            "父层补缺重放：已有行赢（Fresh 只 INSERT IGNORE）"
        );
    }

    fn refu(n: u64) -> RefU64 {
        RefU64((DBNUM as u64) << 32 | n)
    }

    fn element(refno: RefU64, owner: RefU64, noun: &str, children: &[RefU64]) -> EleData {
        let mut ele = EleData::default();
        ele.refno = refno;
        ele.owner = owner;
        let map = &mut ele.whole_attmap.attmap.map;
        map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        map.insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
        map.insert("TYPE".into(), NamedAttrValue::StringType(noun.into()));
        map.insert("DBNUM".into(), NamedAttrValue::IntegerType(DBNUM));
        ele.children = RefU64Vec(children.to_vec());
        ele
    }

    fn batches_of(chunk: &BaselineChunk) -> Vec<String> {
        render_baseline_batches(chunk, DBNUM, &UdaKeyMap::empty())
            .unwrap()
            .0
    }

    /// 一条 `INSERT … INTO <table> [` 语句的方括号内容（一块里每表恰一条）。
    fn insert_rows<'a>(sql: &'a str, table: &str) -> Option<&'a str> {
        let needle = format!("INTO {table} [");
        let start = sql.find(&needle)? + needle.len();
        let end = sql[start..].find("] RETURN NONE;")? + start;
        Some(&sql[start..end])
    }

    /// 一块里的每个元素都出 `pe` 行（带 id）与自己 noun 表的行；同 noun 的行攒在同一条语句里；
    /// 基线是 `Fresh`：没有任何 DELETE，也没有原地替换。
    #[test]
    fn every_element_lands_a_pe_row_and_its_noun_row() {
        let site = refu(1);
        let chunk = BaselineChunk {
            elements: vec![
                (3, element(site, refu(0), "SITE", &[refu(2), refu(3)])),
                (5, element(refu(2), site, "EQUI", &[])),
                (5, element(refu(3), site, "EQUI", &[])),
            ],
            ignored: Vec::new(),
        };
        let batches = batches_of(&chunk);
        assert_eq!(batches.len(), 1, "3 个元素装在一笔事务里");
        let sql = &batches[0];
        assert!(sql.starts_with("BEGIN TRANSACTION;"));
        assert!(!sql.contains("DELETE"), "基线只插:\n{sql}");
        assert!(
            !sql.contains("ON DUPLICATE KEY"),
            "基线不原地替换（已有行赢）:\n{sql}"
        );

        let pe = insert_rows(sql, "pe").expect("pe 行");
        assert_eq!(sql.matches("INSERT IGNORE INTO pe [").count(), 1);
        assert!(pe.contains(&format!("\"id\": pe:{site}")), "{pe}");
        assert!(
            pe.contains("\"sesno\": 3"),
            "pe.sesno 是 last-touch 会话: {pe}"
        );
        assert!(pe.contains("\"sesno\": 5"), "{pe}");

        assert_eq!(sql.matches("INSERT IGNORE INTO SITE [").count(), 1);
        let equi = insert_rows(sql, "EQUI").expect("EQUI 行");
        assert_eq!(
            sql.matches("INSERT IGNORE INTO EQUI [").count(),
            1,
            "两个 EQUI 攒在同一条语句里"
        );
        assert!(equi.contains(&format!("\"id\": \"{}\"", refu(2))), "{equi}");
        assert!(equi.contains(&format!("\"id\": \"{}\"", refu(3))), "{equi}");
        assert!(!sql.contains("ATT_UDA"), "没有 UDA 值就没有 ATT_UDA 语句");
    }

    /// `pe_owner` 边：`in = child, out = owner`，id `[owner, 槽位]`——与增量路同一份 `pe_owner_edge`，
    /// 且带 `RELATION IGNORE`（父层补缺重放叶子已写过的边时必须幂等）。
    #[test]
    fn owner_edges_point_child_to_owner_in_member_order() {
        let site = refu(1);
        let chunk = BaselineChunk {
            elements: vec![(3, element(site, refu(0), "SITE", &[refu(2), refu(3)]))],
            ignored: Vec::new(),
        };
        let batches = batches_of(&chunk);
        let sql = &batches[0];
        assert!(
            sql.contains("INSERT RELATION IGNORE INTO pe_owner ["),
            "{sql}"
        );
        let edges = insert_rows(sql, "pe_owner").expect("有 children 就有边");
        assert_eq!(
            edges,
            format!(
                "{{ id: pe_owner:[pe:{site}, 0], in: pe:{}, out: pe:{site} }},\
                 {{ id: pe_owner:[pe:{site}, 1], in: pe:{}, out: pe:{site} }}",
                refu(2),
                refu(3)
            )
        );
    }

    /// 切块按 `tx_limits.elements`：5 个元素、上限 2 → 三笔（2 / 2 / 1）；上限 0 视作 1。
    #[test]
    fn batches_split_by_the_element_limit() {
        let chunk = BaselineChunk {
            elements: (10..15u64)
                .map(|n| (1, element(refu(n), refu(1), "BOX", &[])))
                .collect(),
            ignored: Vec::new(),
        };
        let limits = |elements: usize| DataTxLimits {
            elements,
            ..Default::default()
        };
        let (batches, report) =
            render_baseline_batches_with_limits(&chunk, DBNUM, &UdaKeyMap::empty(), limits(2))
                .unwrap();
        let per_batch = batches
            .iter()
            .map(|sql| insert_rows(sql, "BOX").unwrap().matches("\"id\": ").count())
            .collect::<Vec<_>>();
        assert_eq!(per_batch, vec![2, 2, 1]);
        assert_eq!(report.transactions, 3);
        assert_eq!(report.elements, 5);
        let (single, _) =
            render_baseline_batches_with_limits(&chunk, DBNUM, &UdaKeyMap::empty(), limits(0))
                .unwrap();
        assert_eq!(single.len(), 5, "0 视作 1，不许除零也不许整块一笔");
    }

    /// 有 UDA 值的元素多出一条 `ATT_UDA` 语句，形状由 `ElementRows` 保证与增量路一致。
    #[test]
    fn uda_values_produce_an_att_uda_batch() {
        use crate::data_interface::uda_keys::UdaDefinitionRow;
        let hash = 0x171F_AD39 + 77;
        let keys = UdaKeyMap::from_rows([UdaDefinitionRow {
            id: RefU64((24381u64 << 32) | 9),
            ukey: Some(hash as i64),
            udna: Some(":ROOM".into()),
        }]);
        let mut ele = element(refu(20), refu(1), "EQUI", &[]);
        ele.whole_attmap.uda_atts.push(ExplicitAttr {
            name: ":ROOM".into(),
            value: NamedAttrValue::StringType("A1".into()),
            is_uda: true,
            hash_val: hash,
        });
        let chunk = BaselineChunk {
            elements: vec![(2, ele)],
            ignored: Vec::new(),
        };
        let (batches, _) = render_baseline_batches(&chunk, DBNUM, &keys).unwrap();
        let uda = insert_rows(&batches[0], "ATT_UDA").expect("有 UDA 值就有 ATT_UDA 语句");
        assert!(uda.contains("'u': UDA:24381_9"), "{uda}");
        assert!(uda.contains(&format!("\"id\": \"{}\"", refu(20))), "{uda}");
    }

    /// issue #10 端到端收口（自 `member_prune` 搬来，判据一个字不改）：把真实的 dbnum 7999
    /// 解析进一个空库，`/1WCC-PIPEBJ` 必须只落一棵。
    ///
    /// legacy 基线的补链轮会把已删子树（`24383_2`，sesno 21/26/29 删）按记录自带的 owner
    /// 复活，于是与 sesno 30 重建的 `24383_66456` 并存，靠 `member_prune` 事后裁；e3d-io 的
    /// 目标索引在 T 上根本没有已删元素，`member_refnos()` 就是元素自己的成员块，幽灵在源头
    /// 就不存在——这一条钉的是「不裁也没有幽灵」。
    ///
    /// 默认忽略。跑法（空库即可，不碰 `.surreal/` 里的真实数据）：
    /// `./scripts/Start-Surreal8009.ps1 -Memory`，再
    /// `cargo test --lib a_reparse_lands_exactly_one_site_per_name -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: requires an empty Surreal on :8009 and local AMS files"]
    async fn a_reparse_lands_exactly_one_site_per_name() {
        use crate::versioned_db::database::sync_total_async_threaded;
        use dashmap::DashSet;
        use std::sync::Arc;

        let db_option = aios_core::init_test_surreal()
            .await
            .expect("connect the empty live Surreal");

        let mut options = db_option.clone();
        options.total_sync = true;
        options.replace_dbs = true;
        options.included_db_files = Some(vec!["ams7999_0001".into()]);
        options.manual_db_nums = Some(vec![7999]);
        options.gen_model = false;
        options.gen_mesh = false;

        let parsed = sync_total_async_threaded(
            &options,
            "AvevaMarineSample",
            Arc::new(DashSet::new()),
            &["DESI"],
            100,
        )
        .await
        .expect("parse dbnum 7999");
        assert!(
            parsed.get(&7999).copied().unwrap_or_default() > 0,
            "dbnum 7999 必须解析出元素，否则下面的断言是空的"
        );

        let mut response = aios_core::SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM pe \
                 WHERE dbnum = 7999 AND noun = 'SITE' AND name = '/1WCC-PIPEBJ';",
            )
            .await
            .expect("query sites")
            .check()
            .expect("valid site query");
        let sites: Vec<String> = response.take(0).expect("site rows");
        assert_eq!(
            sites,
            vec!["24383_66456".to_string()],
            "/1WCC-PIPEBJ 只该剩下 sesno 30 重建的那棵"
        );

        let mut response = aios_core::SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM pe \
                 WHERE dbnum = 7999 AND noun = 'PIPE' AND name = '/1WCC1135';",
            )
            .await
            .expect("query pipes")
            .check()
            .expect("valid pipe query");
        let pipes: Vec<String> = response.take(0).expect("pipe rows");
        assert_eq!(
            pipes,
            vec!["24383_66458".to_string()],
            "截图里那条管子也只该剩真身"
        );

        // 幽灵整棵不可达，连 pe 行都不该写出来。
        let mut response = aios_core::SUL_DB
            .query("RETURN [record::exists(pe:24383_2), record::exists(pe:24383_4)];")
            .await
            .expect("query ghosts")
            .check()
            .expect("valid ghost query");
        let ghosts: Vec<bool> = response.take(0).expect("ghost flags");
        assert_eq!(ghosts, vec![false, false], "已删子树不得留下任何 pe 行");
    }
}
