//! 直写版崩溃重放对拍（ADR-056 实施约束 3 / spec 035 T171，R1）。
//!
//! 接替 `staging/parity.rs` 里随 kv-mem 暂存退役的三条「写回前零落盘」用例：暂存路径
//! 靶的是「写回之前持久层不得变」，直写路径没有这个不变量——它靶的是 **ADR-001 的
//! 另一半**：窗口语句批分块提交（`persist_transaction_batches`），任一块之后进程 kill，
//! 水位没动、恢复记录还在，重启后 `apply_one` 按持久化的固定区间**整窗口重放**，终态必须
//! 与一次成功逐表相等。块内原子，所以「块边界」是崩溃能落在的全部位置；本文件把每一个
//! 边界都停一次再重放，与干净一次跑的数据面快照逐表比较。
//!
//! 窗口是合成的（不读 .dat 文件），但语句全部来自**真实渲染**
//! （`IncrementPipeline::render_persist_statements` → old-pdms-io `to_surql` /
//! `to_modify_surql`）、分块来自真实分块、收口走真实 `finalize_attempt_on`：
//! 对拍的是生产写法，不是测试自己拼的 SQL。语句形态覆盖 Add（含 refno 复用的边清理）、
//! 连续 Modified 折叠（属性新增 / 修改 / 置空 + children_changed）、软删。
//!
//! 载体与快照助手在 `table_parity`；P3 删暂存目录时本文件与它都不动。

#![cfg(test)]

use std::collections::BTreeMap;

use aios_core::{NamedAttrValue, RefU64, RefU64Vec, RefnoEnum};
use parse_pdms_db::parse::EleData;
use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use crate::data_interface::increment_pipeline::{
    IncrementPipeline, PERSIST_TX_CHUNK, persist_transaction_batches,
};
use crate::data_interface::model_update_pending::{ATTEMPT_TABLE, finalize_attempt_on};
use crate::data_interface::model_update_plan::ModelUpdatePlan;
use crate::data_interface::table_parity::{
    apply_all, changed_data_tables, fresh_mem_db, init_schema_on, snapshot_data_tables,
};

/// 保留段 dbnum / refno，避开其它夹具（GLOBAL_AABB_TREE 等进程级共享物不在本文件的
/// 调用树上，但 refno 空间仍按惯例隔开）。
const DBNUM: u32 = 7981;
/// 窗口前水位。
const BASE_SESNO: i32 = 41;
/// 窗口 `START..=END`，两个会话，让同一 refno 的 Modified 跨会话形成可折叠的连跑。
const START_SESNO: u32 = 42;
const END_SESNO: i32 = 43;

fn refu(n: u64) -> RefU64 {
    RefU64((4000000001u64 << 32) | n)
}

/// 窗口前的世界与窗口里要动的元素。
struct Fixture {
    /// 根，不动。
    site: RefU64,
    /// 窗口里被连续改两次（NAME / FUNC / 置空 PURP / children_changed）的容器。
    equi: RefU64,
    /// 窗口里被软删的 BOX。
    gone: RefU64,
    /// refno 复用：窗口前是 EQUI 下的 NOZZ、自己名下还挂着一个旧槽位；窗口里以 BOX 重建。
    reused: RefU64,
    /// `reused` 旧世代的子节点，只以一条陈旧 `pe_owner` 边的形式存在。
    stale_child: RefU64,
    /// 窗口里新增的 BOX。
    new_box: RefU64,
}

fn fixture() -> Fixture {
    Fixture {
        site: refu(781001),
        equi: refu(781002),
        gone: refu(781003),
        reused: refu(781004),
        stale_child: refu(781005),
        new_box: refu(781006),
    }
}

fn pe(refno: RefU64) -> String {
    RefnoEnum::from(refno).to_pe_key()
}

/// 窗口前持久态 + 水位 + `prepare_attempt` 落下的恢复记录（`apply_one` 在写第一块之前
/// 就把它写好，崩溃后靠它决定重放哪一段）。
fn base_statements(f: &Fixture) -> Vec<String> {
    let (site, equi, gone, reused, stale_child) = (
        pe(f.site),
        pe(f.equi),
        pe(f.gone),
        pe(f.reused),
        pe(f.stale_child),
    );
    let (site_id, equi_id, gone_id, reused_id) = (f.site, f.equi, f.gone, f.reused);
    vec![
        format!(
            "UPSERT {site} CONTENT {{ noun: 'SITE', deleted: false, refno: SITE:⟨{site_id}⟩ }};"
        ),
        format!("UPSERT SITE:⟨{site_id}⟩ CONTENT {{ TYPE: 'SITE', NAME: '/RP-SITE' }};"),
        format!(
            "UPSERT {equi} CONTENT {{ noun: 'EQUI', deleted: false, owner: {site}, refno: EQUI:⟨{equi_id}⟩ }};"
        ),
        format!(
            "UPSERT EQUI:⟨{equi_id}⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/RP-EQUI', PURP: 'old-purpose' }};"
        ),
        format!(
            "UPSERT {gone} CONTENT {{ noun: 'BOX', deleted: false, owner: {equi}, refno: BOX:⟨{gone_id}⟩ }};"
        ),
        format!("UPSERT BOX:⟨{gone_id}⟩ CONTENT {{ TYPE: 'BOX', NAME: '/RP-GONE' }};"),
        format!(
            "UPSERT {reused} CONTENT {{ noun: 'NOZZ', deleted: false, owner: {equi}, refno: NOZZ:⟨{reused_id}⟩ }};"
        ),
        format!("UPSERT NOZZ:⟨{reused_id}⟩ CONTENT {{ TYPE: 'NOZZ', NAME: '/RP-OLD-NOZZ' }};"),
        format!(
            "INSERT RELATION INTO pe_owner [ {{ id: [{equi}, 0], in: {gone}, out: {equi} }}, \
             {{ id: [{equi}, 1], in: {reused}, out: {equi} }} ];"
        ),
        // 旧世代残留：`reused` 自己名下的一个子槽位（issue #27 refno 复用的形状）。
        format!(
            "INSERT RELATION INTO pe_owner [ {{ id: [{reused}, 0], in: {stale_child}, out: {reused} }} ];"
        ),
        format!(
            "UPSERT dbnum_watermark:{DBNUM} SET dbnum = {DBNUM}, applied_sesno = {BASE_SESNO};"
        ),
        format!(
            "UPSERT {ATTEMPT_TABLE}:{DBNUM} SET dbnum = {DBNUM}, db_type = 'DESI', \
             file_path = 'mem://replay-parity', start_sesno = {START_SESNO}, end_sesno = {END_SESNO}, \
             plan_json = '{{}}', commit_token = NONE, status = 'prepared';"
        ),
    ]
}

fn add(refno: RefU64, owner: RefU64, noun: &str, name: &str, sesno: u32) -> EleOperationData {
    let mut ele = EleData::default();
    ele.refno = refno;
    ele.owner = owner;
    // 渲染器（NamedAttrMap::pe / gen_sur_json）的 id、refno、owner 全部取自属性映射的
    // REFNO / OWNER 属性而非 EleData 字段——缺 REFNO 会渲染出 id 冲突语句。
    let map = &mut ele.whole_attmap.attmap.map;
    map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
    map.insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
    map.insert("TYPE".into(), NamedAttrValue::StringType(noun.into()));
    map.insert("NAME".into(), NamedAttrValue::StringType(name.into()));
    map.insert("DBNUM".into(), NamedAttrValue::IntegerType(DBNUM as i32));
    EleOperationData::new(refno, sesno, EleOperationDetail::Add(ele))
}

fn text(value: &str) -> NamedAttrValue {
    NamedAttrValue::StringType(value.into())
}

fn modified_equi(
    f: &Fixture,
    sesno: u32,
    explicit_modified: &[(&str, &str, &str)],
    explicit_deleted: &[(&str, &str)],
    children: (Vec<RefU64>, Vec<RefU64>),
) -> EleOperationData {
    let mut current_data = EleData::default();
    current_data.refno = f.equi;
    current_data.owner = f.site;
    let element = ModifiedElement {
        current_data,
        added_attrs: Default::default(),
        deleted_attrs: Default::default(),
        modified_attrs: Default::default(),
        added_explicit_attrs: Default::default(),
        deleted_explicit_attrs: explicit_deleted
            .iter()
            .map(|(key, old)| (key.to_string(), text(old)))
            .collect(),
        modified_explicit_attrs: explicit_modified
            .iter()
            .map(|(key, old, new)| (key.to_string(), (text(old), text(new))))
            .collect(),
        added_uda_attrs: Default::default(),
        deleted_uda_attrs: Default::default(),
        modified_uda_attrs: Default::default(),
        noun: "EQUI".into(),
        children_changed: Some((RefU64Vec(children.0), RefU64Vec(children.1))),
    };
    EleOperationData::new(f.equi, sesno, EleOperationDetail::Modified(element))
}

/// 合成窗口：会话 42 新增 `new_box`、改 EQUI 的 NAME 并把 `new_box` 挂进 children；会话 43
/// 软删 `gone`、以 BOX 重建 `reused`、再改 EQUI 的 FUNC / 置空 PURP 并从 children 摘掉 `gone`。
/// 两次 EQUI 修改在扁平序里是同 refno 的连续 Modified，`fold_window` 会把它们折成一条。
fn window(f: &Fixture) -> BTreeMap<u32, Vec<EleOperationData>> {
    BTreeMap::from([
        (
            START_SESNO,
            vec![
                add(f.new_box, f.equi, "BOX", "/RP-NEW-BOX", START_SESNO),
                modified_equi(
                    f,
                    START_SESNO,
                    &[("NAME", "/RP-EQUI", "/RP-EQUI-2")],
                    &[],
                    (vec![f.gone, f.reused], vec![f.gone, f.reused, f.new_box]),
                ),
            ],
        ),
        (
            END_SESNO as u32,
            vec![
                EleOperationData::new(f.gone, END_SESNO as u32, EleOperationDetail::Deleted),
                add(f.reused, f.equi, "BOX", "/RP-REUSED-BOX", END_SESNO as u32),
                modified_equi(
                    f,
                    END_SESNO as u32,
                    &[("FUNC", "", "pump")],
                    &[("PURP", "old-purpose")],
                    (vec![f.gone, f.reused, f.new_box], vec![f.reused, f.new_box]),
                ),
            ],
        ),
    ])
}

async fn seeded_db(label: &str) -> Surreal<Any> {
    let db = fresh_mem_db("replay_parity", label)
        .await
        .expect("mem boots");
    init_schema_on(&db).await.expect("production schema");
    apply_all(&db, &base_statements(&fixture()))
        .await
        .expect("base fixture");
    db
}

async fn run_batches(db: &Surreal<Any>, batches: &[String]) {
    apply_all(db, batches).await.expect("window batches");
}

async fn finalize(db: &Surreal<Any>) {
    finalize_attempt_on(db, DBNUM, END_SESNO, None, &ModelUpdatePlan::default(), &[])
        .await
        .expect("finalize tail");
}

async fn watermark(db: &Surreal<Any>) -> Option<i32> {
    let mut response = db
        .query(format!("RETURN dbnum_watermark:{DBNUM}.applied_sesno;"))
        .await
        .expect("watermark transport");
    response.take(0).expect("watermark value")
}

async fn attempt_status(db: &Surreal<Any>) -> Option<String> {
    let mut response = db
        .query(format!("RETURN {ATTEMPT_TABLE}:{DBNUM}.status;"))
        .await
        .expect("attempt transport");
    response.take(0).expect("attempt status")
}

async fn ids(db: &Surreal<Any>, sql: String) -> Vec<String> {
    let mut response = db.query(sql).await.expect("query transport");
    let mut rows: Vec<String> = response.take(0).expect("take ids");
    rows.sort();
    rows
}

/// 每一个块边界停一次再整窗口重放，终态都得与一次成功逐表相等；三种分块（每条一块 /
/// 三条一块 / 生产值）都跑——生产值下整窗口只有一块，等于「崩溃只可能在持久化之前或
/// 之后」，小块才把中间态真正暴露出来。
#[tokio::test(flavor = "multi_thread")]
async fn direct_window_replay_converges_from_every_crash_point() {
    let f = fixture();
    let statements = IncrementPipeline::render_persist_statements(&window(&f), DBNUM as i32);
    assert!(
        statements.len() >= 6,
        "对拍对象不能是空集，窗口至少要有 Add 清理 + 四条操作: {statements:?}"
    );

    for chunk in [1usize, 3, PERSIST_TX_CHUNK] {
        let batches = persist_transaction_batches(&statements, chunk);
        assert!(!batches.is_empty());

        let clean = seeded_db(&format!("clean_{chunk}")).await;
        run_batches(&clean, &batches).await;
        finalize(&clean).await;
        let expected = snapshot_data_tables(&clean).await.expect("clean snapshot");
        assert_eq!(watermark(&clean).await, Some(END_SESNO));

        for crashed_after in 0..=batches.len() {
            let db = seeded_db(&format!("crash_{chunk}_{crashed_after}")).await;
            // 第一次尝试：写了前 crashed_after 块就被 kill。
            run_batches(&db, &batches[..crashed_after]).await;
            assert_eq!(
                watermark(&db).await,
                Some(BASE_SESNO),
                "chunk={chunk} crash_after={crashed_after}: 尾事务之前水位不得动"
            );
            assert_eq!(
                attempt_status(&db).await.as_deref(),
                Some("prepared"),
                "chunk={chunk} crash_after={crashed_after}: 恢复记录必须还在，否则重启后没人知道要重放"
            );
            // 重启：按固定区间整窗口重放 + 收口。
            run_batches(&db, &batches).await;
            finalize(&db).await;

            let replayed = snapshot_data_tables(&db).await.expect("replayed snapshot");
            let changed = changed_data_tables(&expected, &replayed);
            assert!(
                changed.is_empty(),
                "chunk={chunk} crash_after={crashed_after}: 重放终态与一次成功不一致，差异表 {changed:?}\n\
                 expected={expected:#?}\nreplayed={replayed:#?}"
            );
            assert_eq!(watermark(&db).await, Some(END_SESNO));
            assert_eq!(
                attempt_status(&db).await,
                None,
                "收口必须删掉恢复记录，否则下一轮会再重放一次"
            );
        }
    }
}

/// 整窗口写完但尾事务没跑到（崩在 persist 与 finalize 之间）：水位与恢复记录都不动——
/// 这是 N1/N2「水位只在尾事务推进」的可执行形态，也是上面每个 crash 点的前提。
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_before_the_tail_leaves_watermark_and_recovery_record_untouched() {
    let f = fixture();
    let statements = IncrementPipeline::render_persist_statements(&window(&f), DBNUM as i32);
    let batches = persist_transaction_batches(&statements, PERSIST_TX_CHUNK);

    let db = seeded_db("crash_before_tail").await;
    run_batches(&db, &batches).await;
    assert_eq!(watermark(&db).await, Some(BASE_SESNO), "数据落库不推进水位");
    assert_eq!(attempt_status(&db).await.as_deref(), Some("prepared"));

    finalize(&db).await;
    assert_eq!(watermark(&db).await, Some(END_SESNO), "只有尾事务推进水位");
    assert_eq!(attempt_status(&db).await, None);
}

/// 一次成功的终态就是它该有的形状——没有这条，「重放 ≡ 一次成功」可能只是两边都
/// 什么都没写。同时钉住今天渲染的一处已知残留（spec 035 清理清单 §6.1 第 2 条）。
#[tokio::test(flavor = "multi_thread")]
async fn the_direct_window_lands_the_expected_shapes() {
    let f = fixture();
    let statements = IncrementPipeline::render_persist_statements(&window(&f), DBNUM as i32);
    let db = seeded_db("shapes").await;
    run_batches(
        &db,
        &persist_transaction_batches(&statements, PERSIST_TX_CHUNK),
    )
    .await;
    finalize(&db).await;

    let (equi, gone, reused, new_box) = (pe(f.equi), pe(f.gone), pe(f.reused), pe(f.new_box));

    // 软删：只翻 pe.deleted。
    let mut response = db
        .query(format!("RETURN {gone}.deleted;"))
        .await
        .expect("deleted transport");
    let deleted: Option<bool> = response.take(0).expect("deleted flag");
    assert_eq!(deleted, Some(true), "Deleted 渲染成软删");

    // 折叠：两次 Modified 合成一条 MERGE——NAME 与 FUNC 都在、PURP 被置空。
    let mut response = db
        .query(format!(
            "RETURN EQUI:⟨{}⟩.NAME; RETURN EQUI:⟨{}⟩.FUNC; RETURN EQUI:⟨{}⟩.PURP; RETURN {equi}.name;",
            f.equi, f.equi, f.equi
        ))
        .await
        .expect("equi transport");
    let name: Option<String> = response.take(0).expect("NAME");
    let func: Option<String> = response.take(1).expect("FUNC");
    let purp: Option<String> = response.take(2).expect("PURP");
    let pe_name: Option<String> = response.take(3).expect("pe.name");
    assert_eq!(
        name.as_deref(),
        Some("/RP-EQUI-2"),
        "会话 42 的 NAME 修改在折叠后仍落库"
    );
    assert_eq!(
        func.as_deref(),
        Some("pump"),
        "会话 43 的 FUNC 修改在折叠后仍落库"
    );
    assert_eq!(purp, None, "deleted_explicit_attrs 渲染成 null，属性被置空");
    assert_eq!(
        pe_name.as_deref(),
        Some("/RP-EQUI-2"),
        "NAME 同步进 pe.name"
    );

    // children_changed 折叠取最旧 old / 最新 new：EQUI 名下只剩 reused + new_box。
    let children = ids(
        &db,
        format!("SELECT VALUE record::id(in) FROM pe_owner WHERE out = {equi};"),
    )
    .await;
    let mut expected_children = vec![f.reused.to_string(), f.new_box.to_string()];
    expected_children.sort();
    assert_eq!(
        children, expected_children,
        "EQUI 的 pe_owner 边 = 窗口终态 children"
    );

    // refno 复用：旧世代的出向边与子槽位都被 Add 前置清理掉，只剩新 owner 边。
    let owners = ids(
        &db,
        format!("SELECT VALUE record::id(out) FROM pe_owner WHERE in = {reused};"),
    )
    .await;
    assert_eq!(
        owners,
        vec![f.equi.to_string()],
        "复用 refno 只剩一条指向新 owner 的边"
    );
    let stale_slots = ids(
        &db,
        format!("SELECT VALUE record::id(in) FROM pe_owner WHERE out = {reused};"),
    )
    .await;
    assert!(
        stale_slots.is_empty(),
        "旧世代子槽位必须被清掉: {stale_slots:?}"
    );
    let mut response = db
        .query(format!("RETURN {reused}.noun; RETURN {new_box}.noun;"))
        .await
        .expect("noun transport");
    let reused_noun: Option<String> = response.take(0).expect("reused noun");
    let new_noun: Option<String> = response.take(1).expect("new noun");
    assert_eq!(reused_noun.as_deref(), Some("BOX"), "pe 行整行替换成新世代");
    assert_eq!(new_noun.as_deref(), Some("BOX"));

    // 已知残留（清理清单 §6.1 第 2 条）：软删 / 复用都只动 pe，旧 noun 的名词表行留着。
    // 这里钉的是**今天的行为**；P4 给 `TypeChanged` 配上 `DELETE <old_noun>:{id}` 时把它翻成
    // `None`，不许静默改。
    let mut response = db
        .query(format!("RETURN NOZZ:⟨{}⟩.NAME;", f.reused))
        .await
        .expect("nozz transport");
    let lingering: Option<String> = response.take(0).expect("NOZZ row");
    assert_eq!(
        lingering.as_deref(),
        Some("/RP-OLD-NOZZ"),
        "今天 refno 复用不清旧 noun 行（已知残留，P4 TypeChanged 收口时翻转本断言）"
    );
}

/// 生产直写与本对拍必须共用同一份分块——对拍在别的分块上绿不算数。
#[test]
fn production_persist_uses_the_shared_chunking() {
    let source = include_str!("increment_pipeline.rs");
    let body = source
        .split_once("async fn persist_latest_main_data(")
        .expect("persist_latest_main_data exists")
        .1
        .split_once("pub(crate) fn render_persist_statements(")
        .expect("render_persist_statements follows")
        .0;
    assert!(
        body.contains("persist_transaction_batches(&statements, PERSIST_TX_CHUNK)"),
        "生产分块必须经 persist_transaction_batches(PERSIST_TX_CHUNK): {body}"
    );
    assert!(
        !body.contains(".chunks("),
        "不得在生产路径另起一套分块: {body}"
    );
}
