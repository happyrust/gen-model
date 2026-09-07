//! 读透（`data_interface::read_through`）的行级对拍——计划
//! `docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md` G2 唯一的验收，不变量 R2：
//!
//! **写透 / 按需读透对一个元素渲出的 `pe` / `ATT_<noun>` / `ATT_UDA` / `pe_owner` 行，必须与
//! `stream_baseline` 对同一元素渲出的逐字段相同——包括 `pe.sesno`（元素 last-touch 会话）。**
//!
//! 两条路共用 `read_element` 与 `ElementRows::render`，但读透多了一段自己的东西：按 refno
//! 单点 `seek` 索引、从会话链反查 last-touch 会话、UPSERT 落库。这里钉的就是那一段没有把
//! 行写歪，以及三条边界（不属于本库的 refno 不写、索引里没有的 refno 点名、重写幂等）。
//!
//! 夹具与 `baseline_stream_8000` 同一份：issue-019 的真实 db8000 归档（sesno 24/25/26），
//! 模板与 attlib 取自 `AIOS_E3D_TEMPLATE_DIR`（未设时用本机默认目录）。
//!
//! 跑法：`cargo test --locked --test read_through_rows_parity -- --nocapture`

#[path = "../src/bin/db8000_two_delete_fixture/archive.rs"]
#[allow(dead_code)]
mod archive;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use aios_core::RefnoEnum;
use aios_core::pdms_types::RefU64;
use aios_database::data_interface::direct_store::DirectSchema;
use aios_database::data_interface::e3d_collector::{open_pinned_set, stream_baseline};
use aios_database::data_interface::ele_data::EleData;
use aios_database::data_interface::ele_ops::{ElementRows, UdaKeyMap};
use aios_database::data_interface::frozen_source::FrozenSource;
use aios_database::data_interface::read_through::{
    ReadThroughCommitCoordinator, ReadThroughSource, ancestors_of, render_rows,
    subtree_with_ancestors, write_through_on,
};
use archive::verify_and_extract;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;

/// 最终快照（sesno 26）在台账里的角色名。
const FINAL_ROLE: &str = "parent_deleted";

/// 逐字段对拍抽多少个元素（等距，跨越整个键序）。整库逐个 `seek` 也就毫秒级，
/// 但断言失败时要看得出是哪一个，样本比全量好读。
const SAMPLES: usize = 64;

fn fixture_root() -> PathBuf {
    std::env::var_os("AIOS_DB8000_TWO_DELETE_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/issues/issue-019-cross-session-parent-child-delete")
        })
}

fn pdms_str(refno: RefU64) -> String {
    RefnoEnum::from(refno).to_pdms_str()
}

fn refno_of(refno: RefU64) -> RefNo {
    RefNo::new(refno.get_0(), refno.get_1())
}

/// 一个元素渲出来的全部行，摊平成可比、可读的形状。
#[derive(Debug, PartialEq, Eq)]
struct RowsView {
    id: String,
    pe: String,
    noun: String,
    noun_row: String,
    uda_row: Option<String>,
    owner_edges: Vec<String>,
}

fn view(rows: &ElementRows) -> RowsView {
    RowsView {
        id: rows.id.clone(),
        pe: rows.pe_row_with_id(),
        noun: rows.noun.clone(),
        noun_row: rows.noun_row.clone(),
        uda_row: rows.uda_row.clone(),
        owner_edges: rows.owner_edges.clone(),
    }
}

struct Fixture {
    _extracted: archive::ExtractedFixture,
    frozen: FrozenSource,
    set: Arc<DbSet>,
    source: ReadThroughSource,
    /// 基线流出来的 `(last-touch 会话, 终稿)`，键序。
    baseline: Vec<(u32, EleData)>,
}

fn fixture() -> Fixture {
    let extracted = verify_and_extract(&fixture_root()).expect("verify issue-019 fixture");
    let final_path = extracted
        .path_for_role(FINAL_ROLE)
        .expect("issue-019 final snapshot");
    let frozen = FrozenSource::capture(&final_path).expect("freeze the db8000 final snapshot");

    let mut baseline = Vec::new();
    stream_baseline(&frozen, frozen.db_type(), 256, |chunk| {
        baseline.extend(chunk.elements);
        Ok(())
    })
    .expect("stream the db8000 baseline");
    assert!(!baseline.is_empty(), "夹具一个元素都没流出来");

    // 读透那一侧的开法与基线同一个（`open_pinned_set`），钉在同一个目标会话上。
    let schema = DirectSchema::open_from_env().expect("template dir");
    let set = open_pinned_set(
        &schema,
        frozen.path(),
        frozen.db_type(),
        frozen.target_sesno(),
    )
    .expect("pin the final snapshot");
    let source = ReadThroughSource {
        dbnum: frozen.dbnum() as u32,
        file: frozen.path().to_path_buf(),
        sesno: frozen.target_sesno(),
    };
    Fixture {
        _extracted: extracted,
        frozen,
        set,
        source,
        baseline,
    }
}

fn sample<'a>(baseline: &'a [(u32, EleData)]) -> Vec<&'a (u32, EleData)> {
    let step = baseline.len().div_ceil(SAMPLES).max(1);
    baseline.iter().step_by(step).collect()
}

/// R2 正题：读透渲出的行与基线逐字段相同，`pe.sesno` 在内。
#[test]
fn read_through_rows_match_the_baseline_field_by_field() {
    let fixture = fixture();
    let keys = UdaKeyMap::empty();
    let sampled = sample(&fixture.baseline);
    let refnos: Vec<RefNo> = sampled.iter().map(|(_, ele)| refno_of(ele.refno)).collect();

    let rendered = render_rows(&fixture.set, &fixture.source, &refnos, &keys)
        .expect("render read-through rows");
    assert_eq!(rendered.foreign, 0);
    assert!(
        rendered.missing.is_empty(),
        "样本全在目标索引里：{:?}",
        rendered.missing
    );
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    assert_eq!(rendered.elements.len(), refnos.len());

    let dbnum = fixture.frozen.dbnum();
    let mut compared = 0usize;
    for ((sesno, ele), got) in sampled.iter().zip(&rendered.elements) {
        assert_eq!(got.refno, ele.refno, "渲染顺序必须与请求顺序一致");
        assert_eq!(
            got.sesno,
            *sesno,
            "{} 的 last-touch 会话：读透 {} vs 基线 {}",
            pdms_str(ele.refno),
            got.sesno,
            sesno
        );
        let expected = ElementRows::render(ele, &ele.refno.to_string(), dbnum, *sesno, &keys);
        assert_eq!(
            view(&got.rows),
            view(&expected),
            "{} 在两条路上渲出的行不一致",
            pdms_str(ele.refno)
        );
        compared += 1;
    }
    assert!(compared >= 8, "只对拍了 {compared} 个元素，样本太小");
}

/// 祖先链从任何一个元素上溯都到 WORL 为止，链上每一个都在基线里；WORL 自己没有祖先。
#[test]
fn ancestors_climb_to_the_world_and_stop() {
    let fixture = fixture();
    let world_noun = aios_core::tool::db_tool::db1_hash("WORL");
    let world = fixture
        .baseline
        .iter()
        .find(|(_, ele)| ele.noun == world_noun)
        .map(|(_, ele)| ele.refno)
        .expect("夹具里有一个 WORL");
    let known: BTreeSet<RefU64> = fixture.baseline.iter().map(|(_, ele)| ele.refno).collect();

    assert!(
        ancestors_of(&fixture.set, refno_of(world))
            .expect("ancestors of world")
            .is_empty(),
        "WORL 之上没有祖先"
    );

    let mut deepest = 0usize;
    for (_, ele) in sample(&fixture.baseline) {
        if ele.noun == world_noun {
            continue;
        }
        let chain = ancestors_of(&fixture.set, refno_of(ele.refno)).expect("ancestors");
        assert!(!chain.is_empty(), "{} 至少有一个属主", pdms_str(ele.refno));
        let last = chain.last().copied().expect("non-empty");
        assert_eq!(
            RefU64::from_two_nums(last.word0, last.word1),
            world,
            "{} 的祖先链必须以 WORL 收尾",
            pdms_str(ele.refno)
        );
        for ancestor in &chain {
            assert!(
                known.contains(&RefU64::from_two_nums(ancestor.word0, ancestor.word1)),
                "祖先 {ancestor:?} 不在基线里"
            );
        }
        deepest = deepest.max(chain.len());
    }
    assert!(
        deepest >= 2,
        "样本里最深的链只有 {deepest} 层，夹具不该这么平"
    );

    // 名单 ∪ 祖先：去重、保序，根在前、WORL 在最后。
    let (_, leaf) = fixture
        .baseline
        .iter()
        .rev()
        .find(|(_, ele)| ele.noun != world_noun)
        .expect("a non-world element");
    let root = refno_of(leaf.refno);
    let list = subtree_with_ancestors(&fixture.set, root, &[root, root]).expect("list");
    assert_eq!(list[0], root);
    assert_eq!(list.iter().filter(|r| **r == root).count(), 1, "去重");
    assert_eq!(
        RefU64::from_two_nums(list.last().unwrap().word0, list.last().unwrap().word1),
        world
    );
}

/// 三条边界：不属于本库的 refno 不写（ADR-058 S3）、索引里没有的点名、真库 UPSERT 幂等。
#[tokio::test]
async fn write_through_is_bounded_to_the_database_and_idempotent() {
    let fixture = fixture();
    let keys = UdaKeyMap::empty();
    let world_noun = aios_core::tool::db_tool::db1_hash("WORL");
    let world = fixture
        .baseline
        .iter()
        .find(|(_, ele)| ele.noun == world_noun)
        .map(|(_, ele)| ele.refno)
        .expect("夹具里有一个 WORL");

    let db = surrealdb::engine::any::connect("mem://")
        .await
        .expect("mem 引擎应能启动（surrealdb 需带 kv-mem feature）");
    db.use_ns("read_through")
        .use_db("parity")
        .await
        .expect("select ns/db");

    let mut refnos: Vec<RefNo> = sample(&fixture.baseline)
        .iter()
        .map(|(_, ele)| refno_of(ele.refno))
        .collect();
    // WORL 一定要在样本里：下面要拿它验 `pe.sesno` 与 `pe_owner` 边。
    if !refnos.contains(&refno_of(world)) {
        refnos.push(refno_of(world));
    }
    let requested = refnos.len();
    // 一个别的库的 refno（dbno 1）与一个本库索引里没有的 refno。
    let foreign = RefNo::new(1, 7);
    let missing = RefNo::new(refnos[0].word0, u32::MAX / 2);
    assert_ne!(foreign.dbno(), fixture.source.dbnum);
    refnos.push(foreign);
    refnos.push(missing);

    let receipt = write_through_on(&db, &fixture.set, &fixture.source, &refnos, &keys)
        .await
        .expect("write through");
    assert_eq!(receipt.dbnum, fixture.source.dbnum);
    assert_eq!(receipt.sesno, fixture.source.sesno);
    assert_eq!(receipt.requested, requested + 2);
    assert_eq!(receipt.foreign, 1, "别的库的 refno 一律不写");
    assert_eq!(
        receipt.missing,
        vec![pdms_str(RefU64::from_two_nums(
            missing.word0,
            missing.word1
        ))],
        "索引里没有的 refno 要点名"
    );
    assert_eq!(receipt.written, requested);
    assert_eq!(receipt.statements, requested);

    let count = |db: &surrealdb::Surreal<surrealdb::engine::any::Any>, from: &str| {
        let db = db.clone();
        let sql = format!("SELECT count() AS n FROM {from} GROUP ALL");
        async move {
            let mut response = db.query(sql).await.expect("count query");
            let rows: Vec<serde_json::Value> = response.take(0).expect("count rows");
            rows.first()
                .and_then(|row| row.get("n"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        }
    };
    assert_eq!(count(&db, "pe").await, requested as i64, "一个元素一行 pe");

    // `pe.sesno` 落库后仍是 last-touch 会话，不是目标会话；`pe_owner` 边随 WORL 一起在。
    let (world_sesno, _) = fixture
        .baseline
        .iter()
        .find(|(_, ele)| ele.refno == world)
        .expect("world in baseline");
    let mut response = db
        .query(format!("SELECT VALUE sesno FROM pe:{world}"))
        .await
        .expect("read back the world");
    let sesno: Vec<i64> = response.take(0).expect("sesno");
    assert_eq!(
        sesno,
        vec![*world_sesno as i64],
        "WORL 的 pe.sesno 必须是 last-touch 会话"
    );
    let world_edges = count(&db, &format!("pe_owner WHERE out = pe:{world}")).await;
    assert!(world_edges > 0, "WORL 名下的 pe_owner 边必须写进去");
    let edges_before = count(&db, "pe_owner").await;

    // 再写一遍：UPSERT 幂等，行数不变，边也不翻倍（每个元素先 `DELETE <-pe_owner` 再插）。
    let again = write_through_on(&db, &fixture.set, &fixture.source, &refnos, &keys)
        .await
        .expect("write through again");
    assert_eq!(again.written, requested);
    assert_eq!(
        receipt.cache_epoch, 1,
        "first material cache change advances epoch"
    );
    assert_eq!(
        again.cache_epoch, receipt.cache_epoch,
        "same session and same content must not advance cache epoch"
    );
    assert_eq!(count(&db, "pe").await, requested as i64, "重写不许多出一行");
    let mut stats = db
        .query("SELECT math::sum(count) AS n FROM dbnum_info_table GROUP ALL;")
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = stats.take(0).unwrap();
    assert_eq!(
        rows[0]["n"].as_i64(),
        Some(requested as i64),
        "读透重放必须原子维护 PE 统计"
    );
    assert_eq!(count(&db, "pe_owner").await, edges_before, "边也不许翻倍");
    assert_eq!(
        count(&db, &format!("pe_owner WHERE out = pe:{world}")).await,
        world_edges
    );

    // 库里每张 `ATT_<noun>` 表的行数之和 = 元素数（一个元素恰一行名词表行）。
    let nouns: BTreeMap<String, usize> = receipt_nouns(&fixture, &refnos[..requested]);
    for (noun, expected) in nouns {
        let got = count(&db, &noun).await;
        assert_eq!(got, expected as i64, "{noun} 表行数");
    }

    let retyped_target = RefU64::from_two_nums(refnos[0].word0, refnos[0].word1);
    db.query(format!(
        "UPDATE pe:{retyped_target} SET noun = 'OLDNOUN'; CREATE OLDNOUN:{retyped_target} CONTENT {{ marker: true }};"
    ))
    .await
    .expect("install old noun row")
    .check()
    .expect("old noun statements");
    write_through_on(&db, &fixture.set, &fixture.source, &refnos, &keys)
        .await
        .expect("noun replacement");
    assert_eq!(
        count(&db, &format!("OLDNOUN WHERE id = OLDNOUN:{retyped_target}")).await,
        0,
        "changing noun removes the prior noun-table row"
    );

    // A parse from an older file session may finish after a newer writer.  The
    // element guard covers pe + noun + UDA + owner edges as one transaction.
    let stale_target = RefU64::from_two_nums(refnos[0].word0, refnos[0].word1);
    db.query(format!("UPDATE pe:{stale_target} SET sesno = 2147483647;"))
        .await
        .expect("install newer sentinel")
        .check()
        .expect("sentinel statement");
    let stale = write_through_on(&db, &fixture.set, &fixture.source, &refnos, &keys)
        .await
        .expect("late old write is accepted as a stale skip");
    assert_eq!(stale.skipped_stale, 1);
    let mut response = db
        .query(format!("SELECT VALUE sesno FROM pe:{stale_target}"))
        .await
        .expect("read newer sentinel");
    let sesno: Vec<i64> = response.take(0).expect("sentinel sesno");
    assert_eq!(sesno, vec![2_147_483_647]);
    assert_eq!(stale.cache_epoch, again.cache_epoch);

    let deleted_target = RefU64::from_two_nums(refnos[1].word0, refnos[1].word1);
    let (deleted, delete_epoch) = ReadThroughCommitCoordinator::global()
        .soft_delete_missing(
            &db,
            fixture.source.dbnum,
            2_000_000_000,
            &[deleted_target.to_string()],
        )
        .await
        .expect("write deletion tombstone");
    assert_eq!(deleted, 1);
    assert!(delete_epoch > stale.cache_epoch);
    write_through_on(&db, &fixture.set, &fixture.source, &refnos, &keys)
        .await
        .expect("late snapshot after deletion");
    let mut response = db
        .query(format!("SELECT VALUE deleted FROM pe:{deleted_target}"))
        .await
        .expect("read tombstone");
    let deleted: Vec<bool> = response.take(0).expect("deleted flag");
    assert_eq!(
        deleted,
        vec![true],
        "old snapshot must not resurrect a tombstone"
    );
}

fn receipt_nouns(fixture: &Fixture, refnos: &[RefNo]) -> BTreeMap<String, usize> {
    let wanted: BTreeSet<RefU64> = refnos
        .iter()
        .map(|refno| RefU64::from_two_nums(refno.word0, refno.word1))
        .collect();
    let mut nouns = BTreeMap::new();
    for (_, ele) in &fixture.baseline {
        if wanted.contains(&ele.refno) {
            let noun = ele.whole_attmap.att_map().get_type();
            *nouns.entry(noun).or_insert(0) += 1;
        }
    }
    nouns
}
