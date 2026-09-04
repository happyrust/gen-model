//! 基线写者的渲染半边：把 [`BaselineChunk`]（e3d-io 退化窗口流出的一块 `Add`）攒成
//! 有界队列里的写者消息。
//!
//! 出处：`docs/plans/2026-09-04-legacy-parser-retirement-endgame-plan.md` §2 / B8.3。
//!
//! * **行形状与增量路同一份**（[`ElementRows`]：`pe` / `ATT_<noun>` / `ATT_UDA` / `pe_owner`），
//!   宪法 II。
//! * **语句动词沿用 legacy 基线**：`INSERT IGNORE INTO … [..]` 批（写者 `SenderJsonsData` 三种
//!   消息原样不动）。不是只为吞吐——`manual_update` 的父层补缺把 extract 母库整库再同步进
//!   同一个 dbnum（`replace_dbs = false`），靠的就是「已有行赢」：叶子 extract 里正在改的
//!   版本不许被母库的旧版本覆盖。`UPSERT` 会把这条语义改掉。
//! * WORL 作为普通元素写全三样行（legacy 不写它的 `pe` 行），计数口径见
//!   [`BaselineStats::elements`]。

use aios_core::RefU64;

use crate::data_interface::e3d_collector::BaselineChunk;
use crate::data_interface::ele_ops::{ElementRows, UdaKeyMap};
use crate::versioned_db::database::SenderJsonsData;

/// 一块基线元素 → 写者消息。
///
/// `pe_chunk` / `att_chunk` 是一条 `INSERT IGNORE` 里最多放几行（`DbOption::pe_chunk / att_chunk`，
/// 与 legacy 基线写者同一对开关）；`pe_owner` 边沿用它 500 一批的粒度。
pub fn render_baseline_batches(
    chunk: &BaselineChunk,
    dbnum: i32,
    uda_keys: &UdaKeyMap,
    pe_chunk: usize,
    att_chunk: usize,
) -> Vec<SenderJsonsData> {
    const RELATE_BATCH: usize = 500;
    let pe_chunk = pe_chunk.max(1);
    let att_chunk = att_chunk.max(1);

    let mut pe_rows = Vec::with_capacity(chunk.elements.len());
    let mut noun_rows: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::new();
    let mut uda_rows = Vec::new();
    let mut edges = Vec::new();
    for (sesno, element) in &chunk.elements {
        let id = element_id(element);
        let rows = ElementRows::render(element, &id, dbnum, *sesno, uda_keys);
        pe_rows.push(rows.pe_row_with_id());
        if !rows.noun.is_empty() && !rows.noun_row.is_empty() {
            noun_rows
                .entry(rows.noun.clone())
                .or_default()
                .push(rows.noun_row.clone());
        }
        if let Some(uda) = rows.uda_row.clone() {
            uda_rows.push(uda);
        }
        edges.extend(rows.owner_edges);
    }

    let mut messages = Vec::new();
    for rows in pe_rows.chunks(pe_chunk) {
        messages.push(SenderJsonsData::PEJson(rows.to_vec()));
    }
    for (noun, rows) in noun_rows {
        for rows in rows.chunks(att_chunk) {
            messages.push(SenderJsonsData::AttJson((noun.clone(), rows.to_vec())));
        }
    }
    for rows in uda_rows.chunks(att_chunk) {
        messages.push(SenderJsonsData::AttJson(("ATT_UDA".to_string(), rows.to_vec())));
    }
    for rows in edges.chunks(RELATE_BATCH) {
        messages.push(SenderJsonsData::PERelateJson(rows.to_vec()));
    }
    messages
}

/// 行 id 与增量路同一口径：`PersistElementChange::to_surql` 用 `refno.to_string()`，
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

    fn messages_of(chunk: &BaselineChunk, pe_chunk: usize, att_chunk: usize) -> Vec<SenderJsonsData> {
        render_baseline_batches(chunk, DBNUM, &UdaKeyMap::empty(), pe_chunk, att_chunk)
    }

    fn pe_batches(messages: &[SenderJsonsData]) -> Vec<&Vec<String>> {
        messages
            .iter()
            .filter_map(|message| match message {
                SenderJsonsData::PEJson(rows) => Some(rows),
                _ => None,
            })
            .collect()
    }

    fn att_batches<'a>(messages: &'a [SenderJsonsData], table: &str) -> Vec<&'a Vec<String>> {
        messages
            .iter()
            .filter_map(|message| match message {
                SenderJsonsData::AttJson((name, rows)) if name == table => Some(rows),
                _ => None,
            })
            .collect()
    }

    /// 一块里的每个元素都出 `pe` 行（带 id）与自己 noun 表的行；同 noun 的行攒在同一条消息里。
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
        let messages = messages_of(&chunk, 100, 100);

        let pe = pe_batches(&messages);
        assert_eq!(pe.len(), 1, "3 行 pe 装在一条 INSERT 里");
        assert_eq!(pe[0].len(), 3);
        assert!(pe[0][0].contains(&format!("\"id\": pe:{site}")), "{}", pe[0][0]);
        assert!(pe[0][0].contains("\"sesno\": 3"), "pe.sesno 是 last-touch 会话: {}", pe[0][0]);
        assert!(pe[0][1].contains("\"sesno\": 5"), "{}", pe[0][1]);

        assert_eq!(att_batches(&messages, "SITE").len(), 1);
        let equi = att_batches(&messages, "EQUI");
        assert_eq!(equi.len(), 1);
        assert_eq!(equi[0].len(), 2, "两个 EQUI 攒在同一条消息里");
        assert!(equi[0][0].contains(&format!("\"id\": \"{}\"", refu(2))), "{}", equi[0][0]);
        assert!(att_batches(&messages, "ATT_UDA").is_empty(), "没有 UDA 值就没有 ATT_UDA 消息");
    }

    /// `pe_owner` 边：`in = child, out = owner`，id `[owner, 槽位]`——与增量路同一份 [`pe_owner_edge`]。
    #[test]
    fn owner_edges_point_child_to_owner_in_member_order() {
        let site = refu(1);
        let chunk = BaselineChunk {
            elements: vec![(3, element(site, refu(0), "SITE", &[refu(2), refu(3)]))],
            ignored: Vec::new(),
        };
        let messages = messages_of(&chunk, 100, 100);
        let edges = messages
            .iter()
            .find_map(|message| match message {
                SenderJsonsData::PERelateJson(rows) => Some(rows),
                _ => None,
            })
            .expect("有 children 就有边");
        assert_eq!(
            edges,
            &vec![
                format!("{{ id: pe_owner:[pe:{site}, 0], in: pe:{}, out: pe:{site} }}", refu(2)),
                format!("{{ id: pe_owner:[pe:{site}, 1], in: pe:{}, out: pe:{site} }}", refu(3)),
            ]
        );
    }

    /// 批大小按 `pe_chunk` / `att_chunk` 切；0 视作 1，不许除零也不许整块一条消息。
    #[test]
    fn batches_respect_the_configured_chunk_sizes() {
        let chunk = BaselineChunk {
            elements: (10..15u64)
                .map(|n| (1, element(refu(n), refu(1), "BOX", &[])))
                .collect(),
            ignored: Vec::new(),
        };
        let messages = messages_of(&chunk, 2, 3);
        let pe = pe_batches(&messages);
        assert_eq!(pe.iter().map(|rows| rows.len()).collect::<Vec<_>>(), vec![2, 2, 1]);
        let boxes = att_batches(&messages, "BOX");
        assert_eq!(boxes.iter().map(|rows| rows.len()).collect::<Vec<_>>(), vec![3, 2]);
        assert_eq!(pe_batches(&messages_of(&chunk, 0, 0)).len(), 5);
    }

    /// 有 UDA 值的元素多出一条 `ATT_UDA` 行，形状由 `ElementRows` 保证与增量路一致。
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
        let messages = render_baseline_batches(&chunk, DBNUM, &keys, 10, 10);
        let uda = att_batches(&messages, "ATT_UDA");
        assert_eq!(uda.len(), 1);
        assert!(uda[0][0].contains("'u': UDA:24381_9"), "{}", uda[0][0]);
        assert!(uda[0][0].contains(&format!("\"id\": \"{}\"", refu(20))), "{}", uda[0][0]);
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
