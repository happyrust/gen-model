//! 直写版崩溃重放对拍（ADR-056 实施约束 3 / spec 035 T171，R1）。
//!
//! 接替 `staging/parity.rs` 里随 kv-mem 暂存退役的三条「写回前零落盘」用例：暂存路径
//! 靶的是「写回之前持久层不得变」，直写路径没有这个不变量——它靶的是 **ADR-001 的
//! 另一半**：窗口语句批分块提交（`persist_transaction_batches`），任一块之后进程 kill，
//! 水位没动、恢复记录还在，重启后 `apply_one` 按持久化的固定区间**整窗口重放**，终态必须
//! 与一次成功逐表相等。块内原子，所以「块边界」是崩溃能落在的全部位置；本文件把每一个
//! 边界都停一次再重放，与干净一次跑的数据面快照逐表比较。
//!
//! 窗口是合成的（不读 .dat 文件），但语句全部来自**真实渲染**：生产路
//! （`IncrementPipeline::terminal_persist_batches` → `element_batch` 批量先删后插，
//! plan 2026-09-06 §3 D2）与兼容路（`render_persist_statements` → old-pdms-io `to_surql` /
//! `to_modify_surql`）各对拍一遍；分块来自真实分块、收口使用真实 `render_finalize_tail`：
//! 对拍的是生产写法，不是测试自己拼的 SQL。语句形态覆盖 Add（含 refno 复用的边清理）、
//! Modified（属性修改 / 置空 + children）、删除（生产路落墓碑、兼容路软删）。
//!
//! 载体与快照助手在 `table_parity`；P3 删暂存目录时本文件与它都不动。

#![cfg(test)]

use std::collections::{BTreeMap, HashMap};

use crate::data_interface::ele_data::EleData;
use crate::data_interface::ele_ops::{EleOperationData, EleOperationDetail, ModifiedElement};
use aios_core::{NamedAttrValue, RefU64, RefU64Vec, RefnoEnum};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use crate::data_interface::increment_pipeline::{
    IncrementPipeline, PERSIST_TX_CHUNK, persist_transaction_batches,
};
use crate::data_interface::increment_tail::render_finalize_tail;
use crate::data_interface::model_update_plan::ModelUpdatePlan;
use crate::data_interface::table_parity::{
    apply_all, changed_data_tables, fresh_mem_db, init_schema_on, snapshot_data_tables,
};
use crate::data_interface::write_state::{WRITE_STATE_TABLE, WriteIntent, render_mark_writing};

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

/// 窗口前持久态 + 水位 + 任务开工时落下的写入标记（`writing{BASE_SESNO, END_SESNO}`，
/// ADR-072 D1：崩溃后靠它决定退到哪一版再重跑）。
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
        render_mark_writing(DBNUM, BASE_SESNO, END_SESNO, WriteIntent::ApplyWindow),
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
    // 窗口前的行归本库、停在窗口前水位：生产路的事务带 pe 统计观测
    // （`pe_batch::render` 的 `$row.dbnum != dbnum → THROW`），没有 dbnum 的行会被当成别库的。
    db.query(format!("UPDATE pe SET dbnum={DBNUM}, sesno={BASE_SESNO};"))
        .await
        .expect("stamp dbnum transport")
        .check()
        .expect("stamp dbnum");
    db
}

async fn run_batches(db: &Surreal<Any>, batches: &[String]) {
    apply_all(db, batches).await.expect("window batches");
}

async fn finalize(db: &Surreal<Any>) {
    let render = render_finalize_tail(DBNUM, END_SESNO, None, &ModelUpdatePlan::default(), &[]);
    apply_all(db, &render.window_batches)
        .await
        .expect("finalize window batches");
    db.query(format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        render.tail
    ))
    .await
    .expect("finalize tail transport")
    .check()
    .expect("finalize tail");
}

async fn watermark(db: &Surreal<Any>) -> Option<i32> {
    let mut response = db
        .query(format!("RETURN dbnum_watermark:{DBNUM}.applied_sesno;"))
        .await
        .expect("watermark transport");
    response.take(0).expect("watermark value")
}

async fn write_state_status(db: &Surreal<Any>) -> Option<String> {
    let mut response = db
        .query(format!("RETURN {WRITE_STATE_TABLE}:{DBNUM}.status;"))
        .await
        .expect("write state transport");
    response.take(0).expect("write state status")
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
                write_state_status(&db).await.as_deref(),
                Some("writing"),
                "chunk={chunk} crash_after={crashed_after}: 写入标记必须还在，否则重启后没人知道要回退"
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
                write_state_status(&db).await.as_deref(),
                Some("writing"),
                "数据尾事务不碰写入标记（ADR-072 W2）：收它的是任务收尾那一笔"
            );
        }
    }
}

/// 整窗口写完但尾事务没跑到（崩在 persist 与 finalize 之间）：水位与写入标记都不动——
/// 这是 N1/N2「水位只在尾事务推进」的可执行形态，也是上面每个 crash 点的前提。
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_before_the_tail_leaves_watermark_and_write_marker_untouched() {
    let f = fixture();
    let statements = IncrementPipeline::render_persist_statements(&window(&f), DBNUM as i32);
    let batches = persist_transaction_batches(&statements, PERSIST_TX_CHUNK);

    let db = seeded_db("crash_before_tail").await;
    run_batches(&db, &batches).await;
    assert_eq!(watermark(&db).await, Some(BASE_SESNO), "数据落库不推进水位");
    assert_eq!(write_state_status(&db).await.as_deref(), Some("writing"));

    finalize(&db).await;
    assert_eq!(watermark(&db).await, Some(END_SESNO), "只有尾事务推进水位");
    assert_eq!(
        write_state_status(&db).await.as_deref(),
        Some("writing"),
        "尾事务只盖 applied_sesno，标记留给任务收尾那一笔"
    );
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

/// Compatibility renderer tests above remain separate from terminal production writes.
#[test]
fn production_persist_uses_the_shared_chunking() {
    let source = include_str!("increment_pipeline.rs");
    let body = source
        .split_once("async fn persist_latest_main_data(")
        .expect("persist_latest_main_data exists")
        .1
        .split_once("pub(crate) fn terminal_persist_batches(")
        .expect("terminal_persist_batches follows")
        .0;
    assert!(
        body.contains("Self::terminal_persist_batches(changes, retired, dbnum, &uda_keys)?"),
        "生产分块必须经 element_batch + 显式统计事务: {body}"
    );
    assert!(
        !body.contains(".chunks("),
        "不得在生产路径另起一套分块: {body}"
    );
    // 生产写法只有一种：逐元素 UPSERT / UPDATE 不许再回到 increment_pipeline 的落库路上。
    let persist_body = source
        .split_once("async fn persist_latest_main_data(")
        .unwrap()
        .1
        .split_once("/// 渲染差分操作的兼容语句")
        .expect("compat renderer doc follows the production entry points")
        .0;
    assert!(
        !persist_body.contains("UPSERT ") && !persist_body.contains("UPDATE pe:"),
        "生产落库路径不得渲染逐元素 UPSERT / UPDATE: {persist_body}"
    );
}

// ───────────────────────── 生产路（element_batch 批量先删后插）的崩溃重放对拍 ─────────────────────────

/// 完整终态的元素（收集器给生产路的就是这种：`pe` / noun 行 / children 都齐）。
fn full_element(
    refno: RefU64,
    owner: RefU64,
    noun: &str,
    name: &str,
    extra: &[(&str, &str)],
    children: &[RefU64],
) -> EleData {
    let mut ele = EleData::default();
    ele.refno = refno;
    ele.owner = owner;
    ele.children = RefU64Vec(children.to_vec());
    let map = &mut ele.whole_attmap.attmap.map;
    map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
    map.insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
    map.insert("TYPE".into(), NamedAttrValue::StringType(noun.into()));
    map.insert("NAME".into(), NamedAttrValue::StringType(name.into()));
    map.insert("DBNUM".into(), NamedAttrValue::IntegerType(DBNUM as i32));
    for (key, value) in extra {
        map.insert(key.to_string(), text(value));
    }
    ele
}

/// 生产路的窗口：同一批事实（新增 `new_box`、EQUI 改名 + 改 children、删 `gone`、`reused` 以 BOX
/// 重建），但 Modified 携带**完整**终态、Deleted 带基侧状态——e3d-io 收集器交给 `terminal_persist_changes`
/// 的就是这个形状。
fn production_changes(f: &Fixture) -> Vec<crate::data_interface::ele_ops::PersistElementChange> {
    let mut equi_modified = ModifiedElement {
        current_data: full_element(
            f.equi,
            f.site,
            "EQUI",
            "/RP-EQUI-2",
            &[("FUNC", "pump")],
            &[f.reused, f.new_box],
        ),
        added_attrs: Default::default(),
        deleted_attrs: Default::default(),
        modified_attrs: Default::default(),
        added_explicit_attrs: Default::default(),
        deleted_explicit_attrs: Default::default(),
        modified_explicit_attrs: Default::default(),
        added_uda_attrs: Default::default(),
        deleted_uda_attrs: Default::default(),
        modified_uda_attrs: Default::default(),
        noun: "EQUI".into(),
        children_changed: Some((
            RefU64Vec(vec![f.gone, f.reused]),
            RefU64Vec(vec![f.reused, f.new_box]),
        )),
    };
    equi_modified
        .modified_explicit_attrs
        .insert("NAME".into(), (text("/RP-EQUI"), text("/RP-EQUI-2")));
    let window = BTreeMap::from([
        (
            START_SESNO,
            vec![EleOperationData::new(
                f.new_box,
                START_SESNO,
                EleOperationDetail::Add(full_element(
                    f.new_box,
                    f.equi,
                    "BOX",
                    "/RP-NEW-BOX",
                    &[],
                    &[],
                )),
            )],
        ),
        (
            END_SESNO as u32,
            vec![
                EleOperationData::new(f.gone, END_SESNO as u32, EleOperationDetail::Deleted),
                EleOperationData::new(
                    f.reused,
                    END_SESNO as u32,
                    EleOperationDetail::Add(full_element(
                        f.reused,
                        f.equi,
                        "BOX",
                        "/RP-REUSED-BOX",
                        &[],
                        &[],
                    )),
                ),
                EleOperationData::new(
                    f.equi,
                    END_SESNO as u32,
                    EleOperationDetail::Modified(equi_modified),
                ),
            ],
        ),
    ]);
    let tombstones: HashMap<RefU64, EleData> = [(
        f.gone,
        full_element(f.gone, f.equi, "BOX", "/RP-GONE", &[], &[]),
    )]
    .into_iter()
    .collect();
    crate::data_interface::ele_ops::terminal_persist_changes(&window, &tombstones)
}

fn retired_rows(f: &Fixture) -> Vec<crate::data_interface::e3d_collector::RetiredTypeRow> {
    vec![crate::data_interface::e3d_collector::RetiredTypeRow {
        refno: f.reused,
        old_noun: "NOZZ".into(),
        new_noun: "BOX".into(),
    }]
}

/// 生产入口（`terminal_persist_batches`，生效上限）。
fn production_batches(f: &Fixture) -> Vec<String> {
    let (batches, report) = IncrementPipeline::terminal_persist_batches(
        &production_changes(f),
        &retired_rows(f),
        DBNUM as i32,
        &crate::data_interface::uda_keys::UdaKeyMap::empty(),
    )
    .unwrap();
    assert_eq!(report.tombstones, 1);
    assert_eq!(report.tombstones_without_base, 0);
    batches
}

/// 同一渲染器、元素上限压到 `elements`：让每 N 个元素自成一块，把块边界（= 崩溃点）全部暴露出来。
fn production_batches_with_elements(f: &Fixture, elements: usize) -> Vec<String> {
    let limits = crate::data_interface::tx_limits::DataTxLimits {
        elements,
        ..Default::default()
    };
    let (batches, _) = crate::data_interface::element_batch::render_transactions_with_limits(
        &production_changes(f),
        &retired_rows(f),
        DBNUM as i32,
        &crate::data_interface::uda_keys::UdaKeyMap::empty(),
        crate::data_interface::element_batch::WriteMode::Replace,
        limits,
    )
    .unwrap();
    batches
}

/// 生产路：每一个事务边界停一次再整窗口重放，终态都得与一次成功逐表相等。三种切法（每元素一块 /
/// 两元素一块 / 生效上限）都跑——生效上限下整窗口只有前置块 + 一块，小块才把中间态真正暴露出来。
#[tokio::test(flavor = "multi_thread")]
async fn production_window_replay_converges_from_every_crash_point() {
    let f = fixture();
    for elements in [1usize, 2, 1000] {
        let batches = production_batches_with_elements(&f, elements);
        assert!(
            batches.len() >= 2,
            "elements={elements}: 前置块 + 元素块，至少两笔: {}",
            batches.len()
        );
        if elements == 1 {
            assert_eq!(batches.len(), 1 + 4, "每个元素自成一块：前置块 + 4 个元素");
        }

        let clean = seeded_db(&format!("prod_clean_{elements}")).await;
        run_batches(&clean, &batches).await;
        finalize(&clean).await;
        let expected = snapshot_data_tables(&clean).await.expect("clean snapshot");
        assert_eq!(watermark(&clean).await, Some(END_SESNO));

        for crashed_after in 0..=batches.len() {
            let db = seeded_db(&format!("prod_crash_{elements}_{crashed_after}")).await;
            run_batches(&db, &batches[..crashed_after]).await;
            assert_eq!(
                watermark(&db).await,
                Some(BASE_SESNO),
                "elements={elements} crash_after={crashed_after}: 尾事务之前水位不得动"
            );
            assert_eq!(write_state_status(&db).await.as_deref(), Some("writing"));
            run_batches(&db, &batches).await;
            finalize(&db).await;
            let replayed = snapshot_data_tables(&db).await.expect("replayed snapshot");
            let changed = changed_data_tables(&expected, &replayed);
            assert!(
                changed.is_empty(),
                "elements={elements} crash_after={crashed_after}: 重放终态与一次成功不一致，差异表 {changed:?}\n\
             expected={expected:#?}\nreplayed={replayed:#?}"
            );
            assert_eq!(watermark(&db).await, Some(END_SESNO));
            assert_eq!(write_state_status(&db).await.as_deref(), Some("writing"));
        }
    }
}

/// ADR-072 D2 的可执行形态，也是 R1 的验收②：**回退 → 重跑**（而不是「原样重放」）之后，
/// 终态必须与一次成功逐表相等。
///
/// 与上面那条的区别正是这一轮换掉的东西：从前崩溃后按持久化的固定区间**接着写**（半应用
/// 的库直接被当成重放的起点），现在先 `rollback_to_watermark(dbnum, S, intent)` 把
/// `sesno > S` 的痕迹删掉、三水位退回 S，再从头跑 S→T。kill 点覆盖每一个事务边界，
/// 外加「数据全写完、尾事务前」与「尾事务已提交（applied 已到 T）」两处——D3 说崩在任何
/// 一面都退回 S，已经盖到 T 的水位也要退。
#[tokio::test(flavor = "multi_thread")]
async fn a_rolled_back_window_reruns_into_the_same_state_as_one_clean_pass() {
    use crate::data_interface::fast_delete::rollback_to_watermark_on;

    let f = fixture();
    for elements in [1usize, 2, 1000] {
        let batches = production_batches_with_elements(&f, elements);

        let clean = seeded_db(&format!("rollback_clean_{elements}")).await;
        run_batches(&clean, &batches).await;
        finalize(&clean).await;
        let expected = snapshot_data_tables(&clean).await.expect("clean snapshot");
        assert_eq!(watermark(&clean).await, Some(END_SESNO));

        // `..=batches.len()` 是「数据全写完、尾事务前」；再多一档是「尾事务也提交了」。
        for crashed_after in 0..=batches.len() + 1 {
            let tail_committed = crashed_after > batches.len();
            let written = crashed_after.min(batches.len());
            let db = seeded_db(&format!("rollback_crash_{elements}_{crashed_after}")).await;
            run_batches(&db, &batches[..written]).await;
            if tail_committed {
                finalize(&db).await;
                assert_eq!(
                    watermark(&db).await,
                    Some(END_SESNO),
                    "前提：这一档数据水位已经盖到 T"
                );
            }

            // 重启的水化段：见 writing → 回退到 S。
            let marker = write_state_status(&db).await;
            assert_eq!(marker.as_deref(), Some("writing"), "崩溃残留必须是 writing");
            let report = rollback_to_watermark_on(&db, DBNUM, BASE_SESNO, WriteIntent::ApplyWindow)
                .await
                .expect("rollback to watermark");
            assert!(!report.wiped);
            assert_eq!(
                watermark(&db).await,
                Some(BASE_SESNO),
                "elements={elements} crash_after={crashed_after}: 三水位必须退回 S"
            );
            assert_eq!(
                write_state_status(&db).await,
                None,
                "回退的最后一句删掉标记行，下一次开工重写 writing"
            );

            // 重跑 S→T：开工写标记 → 数据块 → 尾事务。
            crate::data_interface::table_parity::apply_all(
                &db,
                &[render_mark_writing(
                    DBNUM,
                    BASE_SESNO,
                    END_SESNO,
                    WriteIntent::ApplyWindow,
                )],
            )
            .await
            .expect("re-mark writing");
            run_batches(&db, &batches).await;
            finalize(&db).await;

            let replayed = snapshot_data_tables(&db).await.expect("replayed snapshot");
            let mut changed = changed_data_tables(&expected, &replayed);
            // `spatial_epoch` 是控制面计数器，回退**故意**要 bump 它（§4.3-7：不 bump 就会
            // 按指纹相等复用一棵还留着被删构件包围盒的树）。它在两边不等正是回退干过活
            // 的证据，不是数据面差异。
            assert!(
                changed.remove("spatial_epoch"),
                "回退必须在库侧留下空间痕迹（epoch bump）"
            );
            assert!(
                changed.is_empty(),
                "elements={elements} crash_after={crashed_after}: 回退重跑终态与一次成功不一致，\
                 差异表 {changed:?}\nexpected={expected:#?}\nreplayed={replayed:#?}"
            );
            assert_eq!(watermark(&db).await, Some(END_SESNO));
        }
    }
}

/// 属主没进本窗口的 Modified 元素，回退之后**它在属主名下那条成员边还在**。
///
/// 这是 t-468 量出来、这一版修掉的那处缺口：`pe` 是图顶点，删它会级联带走
/// `pe_owner:[属主, 槽位]`（`in` 指着被删元素的那条），而槽位号只存在于属主的成员表里
/// ——属主这一版没被改、不进 S→T 重放窗口，那条入边就再也补不回来。所以回退只删卫星行
/// （noun / `ATT_UDA`）与模型 / 房间面，**不删 `pe` 顶点**：数据面靠行幂等收敛，重放的
/// `INSERT … ON DUPLICATE KEY UPDATE` 整行覆盖。
///
/// 夹具刻意补上 `site → equi` 那条边：原来的 `base_statements` 里 EQUI 没有入边，
/// 缺口正好测不出来。
#[tokio::test(flavor = "multi_thread")]
async fn a_modified_element_keeps_the_membership_edge_its_unchanged_owner_owns() {
    use crate::data_interface::fast_delete::rollback_to_watermark_on;

    let f = fixture();
    let db = seeded_db("membership_edge").await;
    let (site, equi) = (pe(f.site), pe(f.equi));
    // SITE 停在窗口前水位、名下挂着 EQUI：EQUI 在窗口里只是 Modified，SITE 不进窗口。
    crate::data_interface::table_parity::apply_all(
        &db,
        &[format!(
            "INSERT RELATION INTO pe_owner [\
               {{ id: pe_owner:[{site}, 0], in: {equi}, out: {site} }}];"
        )],
    )
    .await
    .expect("owner edge fixture");

    let batches = production_batches_with_elements(&f, 1);
    let clean = seeded_db("membership_edge_clean").await;
    crate::data_interface::table_parity::apply_all(
        &clean,
        &[format!(
            "INSERT RELATION INTO pe_owner [\
               {{ id: pe_owner:[{site}, 0], in: {equi}, out: {site} }}];"
        )],
    )
    .await
    .expect("owner edge fixture");
    run_batches(&clean, &batches).await;
    finalize(&clean).await;
    let expected = snapshot_data_tables(&clean).await.expect("clean snapshot");

    // 崩在第一块之后 → 回退 → 重跑。
    run_batches(&db, &batches[..1]).await;
    rollback_to_watermark_on(&db, DBNUM, BASE_SESNO, WriteIntent::ApplyWindow)
        .await
        .expect("rollback");
    let owners = ids(
        &db,
        format!("SELECT VALUE record::id(out) FROM pe_owner WHERE in = {equi};"),
    )
    .await;
    assert_eq!(
        owners,
        vec![f.site.to_string()],
        "回退不得带走属主名下那条成员边（删 pe 顶点就会）"
    );

    crate::data_interface::table_parity::apply_all(
        &db,
        &[render_mark_writing(
            DBNUM,
            BASE_SESNO,
            END_SESNO,
            WriteIntent::ApplyWindow,
        )],
    )
    .await
    .expect("re-mark writing");
    run_batches(&db, &batches).await;
    finalize(&db).await;

    let replayed = snapshot_data_tables(&db).await.expect("replayed snapshot");
    let mut changed = changed_data_tables(&expected, &replayed);
    changed.remove("spatial_epoch");
    assert!(
        changed.is_empty(),
        "回退重跑之后逐表仍须与一次成功相等，差异表 {changed:?}"
    );
}

/// 回退把三枚水位一起退回 S（ADR-072 D3「崩溃不分面」），模型面 / 房间面的行按谓词一起走，
/// 而 S 及以前的行一个都不许动。
#[tokio::test(flavor = "multi_thread")]
async fn the_rollback_takes_every_face_back_to_s_and_leaves_the_older_rows_alone() {
    use crate::data_interface::fast_delete::rollback_to_watermark_on;

    let f = fixture();
    let db = seeded_db("rollback_faces").await;
    run_batches(&db, &production_batches(&f)).await;
    finalize(&db).await;
    // 模型面 / 房间面：S 那一版与 (S, T] 那一版各一行，回退只该带走后者。
    let (equi, gone) = (pe(f.equi), pe(f.gone));
    crate::data_interface::table_parity::apply_all(
        &db,
        &[
            format!(
                "INSERT INTO inst_relate [\
                   {{ id: inst_relate:old, in: {equi}, out: inst_info:old, dbnum: {DBNUM}, \
                      model_sesno: {BASE_SESNO} }},\
                   {{ id: inst_relate:new, in: {equi}, out: inst_info:new, dbnum: {DBNUM}, \
                      model_sesno: {END_SESNO} }}];"
            ),
            format!(
                "INSERT RELATION INTO room_relate [\
                   {{ id: room_relate:r1, in: {equi}, out: {gone}, room_num: 'R301' }}];"
            ),
            format!("UPDATE dbnum_watermark:{DBNUM} SET model_sesno = {END_SESNO};"),
        ],
    )
    .await
    .expect("model and room fixture");

    rollback_to_watermark_on(&db, DBNUM, BASE_SESNO, WriteIntent::ApplyWindow)
        .await
        .expect("rollback");

    let survivors = ids(
        &db,
        "SELECT VALUE record::id(id) FROM inst_relate;".to_string(),
    )
    .await;
    assert_eq!(
        survivors,
        vec!["old".to_string()],
        "只带走 sesno > S 的模型行"
    );
    let rooms = ids(
        &db,
        "SELECT VALUE record::id(id) FROM room_relate;".to_string(),
    )
    .await;
    assert!(
        rooms.is_empty(),
        "房间边整库删（本来就是整库重算）: {rooms:?}"
    );

    #[derive(Debug, serde::Deserialize)]
    struct Marks {
        applied_sesno: Option<i32>,
        model_sesno: Option<i32>,
    }
    let mut response = db
        .query(format!(
            "SELECT applied_sesno, model_sesno FROM ONLY dbnum_watermark:{DBNUM};"
        ))
        .await
        .expect("watermark transport");
    let marks: Option<Marks> = response.take(0).expect("watermark row");
    let marks = marks.expect("watermark row exists");
    assert_eq!(
        (marks.applied_sesno, marks.model_sesno),
        (Some(BASE_SESNO), Some(BASE_SESNO)),
        "两枚水位一起退回 S"
    );
}

/// 生产路一次成功的终态就是它该有的形状：墓碑保留 noun / owner、被删元素的 noun 行与 UDA 行
/// 消失、换 noun 的旧表行消失、children 边 = 终态、refno 复用只剩新 owner 边。
#[tokio::test(flavor = "multi_thread")]
async fn the_production_window_lands_the_expected_shapes() {
    let f = fixture();
    let db = seeded_db("prod_shapes").await;
    run_batches(&db, &production_batches(&f)).await;
    finalize(&db).await;
    let (equi, gone, reused) = (pe(f.equi), pe(f.gone), pe(f.reused));

    // 墓碑：pe 行在、deleted = true、noun / owner 保留、sesno 是删除会话；BOX 行没了。
    let mut response = db
        .query(format!(
            "RETURN {gone}.deleted; RETURN {gone}.noun; RETURN record::id({gone}.owner); RETURN {gone}.sesno; RETURN BOX:⟨{}⟩.NAME;",
            f.gone
        ))
        .await
        .expect("tombstone transport");
    assert_eq!(response.take::<Option<bool>>(0).unwrap(), Some(true));
    assert_eq!(
        response.take::<Option<String>>(1).unwrap().as_deref(),
        Some("BOX")
    );
    assert_eq!(
        response.take::<Option<String>>(2).unwrap().as_deref(),
        Some(f.equi.to_string().as_str())
    );
    assert_eq!(
        response.take::<Option<i64>>(3).unwrap(),
        Some(END_SESNO as i64)
    );
    assert_eq!(
        response.take::<Option<String>>(4).unwrap(),
        None,
        "被删元素的 noun 行随删除块消失"
    );

    // 被删元素在 owner 名下的边没了；EQUI 的 children 边 = 终态。
    let children = ids(
        &db,
        format!("SELECT VALUE record::id(in) FROM pe_owner WHERE out = {equi};"),
    )
    .await;
    let mut expected_children = vec![f.reused.to_string(), f.new_box.to_string()];
    expected_children.sort();
    assert_eq!(children, expected_children);

    // Modified 的属性终态与 pe.name 同步。
    let mut response = db
        .query(format!(
            "RETURN EQUI:⟨{}⟩.NAME; RETURN EQUI:⟨{}⟩.FUNC; RETURN {equi}.name;",
            f.equi, f.equi
        ))
        .await
        .expect("equi transport");
    assert_eq!(
        response.take::<Option<String>>(0).unwrap().as_deref(),
        Some("/RP-EQUI-2")
    );
    assert_eq!(
        response.take::<Option<String>>(1).unwrap().as_deref(),
        Some("pump")
    );
    assert_eq!(
        response.take::<Option<String>>(2).unwrap().as_deref(),
        Some("/RP-EQUI-2")
    );

    // refno 复用：旧出向边与旧子槽位清掉、只剩新 owner 边；旧 noun（NOZZ）行随删除块消失。
    let owners = ids(
        &db,
        format!("SELECT VALUE record::id(out) FROM pe_owner WHERE in = {reused};"),
    )
    .await;
    assert_eq!(owners, vec![f.equi.to_string()]);
    let stale_slots = ids(
        &db,
        format!("SELECT VALUE record::id(in) FROM pe_owner WHERE out = {reused};"),
    )
    .await;
    assert!(stale_slots.is_empty(), "{stale_slots:?}");
    let mut response = db
        .query(format!(
            "RETURN NOZZ:⟨{}⟩.NAME; RETURN {reused}.noun; RETURN BOX:⟨{}⟩.NAME;",
            f.reused, f.reused
        ))
        .await
        .expect("reused transport");
    assert_eq!(
        response.take::<Option<String>>(0).unwrap(),
        None,
        "换 noun 的旧表行随删除块消失（D2）"
    );
    assert_eq!(
        response.take::<Option<String>>(1).unwrap().as_deref(),
        Some("BOX")
    );
    assert_eq!(
        response.take::<Option<String>>(2).unwrap().as_deref(),
        Some("/RP-REUSED-BOX")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_production_batches_replay_statistics_before_watermark() {
    let f = fixture();
    let db = seeded_db("terminal_stats").await;
    db.query(format!("UPDATE pe SET dbnum={DBNUM}, sesno={BASE_SESNO};"))
        .await
        .unwrap()
        .check()
        .unwrap();
    let batches = production_batches(&f);
    for _ in 0..2 {
        for sql in &batches {
            crate::data_interface::pe_batch::execute_on(&db, sql)
                .await
                .unwrap();
        }
        assert_eq!(watermark(&db).await, Some(BASE_SESNO));
        assert_eq!(write_state_status(&db).await.as_deref(), Some("writing"));
    }
    // pe 统计不再随写事务逐笔记账（ADR-071 D6）：终态批次重放两遍，`dbnum_info_table` 一行都不动
    // ——`count` 由该库任务在数据批次应用之后从目标索引序列化（`ref0_snapshot` → `task_end`），
    // 这条只跑数据面语句的对拍看不到它。
    let mut response = db
        .query("SELECT count() AS n FROM pe GROUP ALL; SELECT count() AS n FROM dbnum_info_table GROUP ALL;")
        .await
        .unwrap();
    #[derive(Debug, serde::Deserialize)]
    struct Count {
        n: i64,
    }
    let pe: Vec<Count> = response.take(0).unwrap();
    let stats: Vec<Count> = response.take(1).unwrap();
    assert!(pe[0].n > 0);
    assert_eq!(
        stats.first().map_or(0, |count| count.n),
        0,
        "写事务不再碰 dbnum_info_table: {stats:?}"
    );
    finalize(&db).await;
    assert_eq!(watermark(&db).await, Some(END_SESNO));
}

// ───────────────────── 整任务的崩溃点重放（ADR-072 D2 / D3，计划 §6-2，R1 验收）─────────────────────
//
// 上面几条只在**数据面**的块边界 kill。计划 §3 的时间线还有三段：数据尾事务之后的模型块、
// 房间提交、`task_end` 收尾——崩在任何一段，D3 说都退回 S 再整窗口重跑。下面把一个 dbnum 任务
// 的整条写库时间线摆成一串 [`Step`]，在**每一个**步骤边界停一次：重启见 `writing` → 回退 →
// 重跑 → 逐表与一次成功相等。三种意图各一组：ApplyWindow（S = 窗口前水位）、ModelOnly
// （S = 模型水位）、Initialize（S = 0，回退 = 整库 wipe）。
//
// 数据块与两笔尾事务是真实渲染 / 真实入口；模型块与房间提交是**合成行**——生成器要 e3d 几何，
// mem:// 上摆不出来，但回退删的正是这些行的 `dbnum` / `model_sesno` / `source_end_sesno`
// / `in.dbnum` 列，行形状照生产写（`the_rollback_takes_every_face_back_to_s_…` 同一套）。

/// 一个 dbnum 任务写库时间线上的一步。`Data` / `Model` / `Rooms` 各是一笔独立提交。
enum Step {
    Data(String),
    /// 数据尾事务：`render_finalize_tail`（窗口）或 `render_watermark_advance`（基线）。
    DataTail {
        baseline: bool,
    },
    Model(String),
    Rooms(String),
    TaskEnd,
}

impl Step {
    fn label(&self) -> &'static str {
        match self {
            Step::Data(_) => "data",
            Step::DataTail { baseline: false } => "data_tail",
            Step::DataTail { baseline: true } => "baseline_tail",
            Step::Model(_) => "model",
            Step::Rooms(_) => "rooms",
            Step::TaskEnd => "task_end",
        }
    }
}

async fn run_step(db: &Surreal<Any>, step: &Step) {
    match step {
        Step::Data(sql) | Step::Model(sql) | Step::Rooms(sql) => {
            apply_all(db, &[sql.clone()]).await.expect("step statement");
        }
        Step::DataTail { baseline: false } => finalize(db).await,
        Step::DataTail { baseline: true } => {
            apply_all(
                db,
                &[format!(
                    "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
                    crate::data_interface::increment_tail::render_watermark_advance(
                        DBNUM, END_SESNO, None
                    )
                )],
            )
            .await
            .expect("baseline tail");
        }
        Step::TaskEnd => {
            let receipt = crate::data_interface::task_end::commit_task_end_on(
                db,
                &crate::data_interface::task_end::TaskEnd {
                    dbnum: DBNUM,
                    target_sesno: END_SESNO,
                    data: None,
                    target_time: None,
                    model: Some(crate::data_interface::task_end::TaskModelOutcome {
                        all_roots_ok: true,
                        dead_roots: 0,
                        catalogue_sessions: None,
                    }),
                    info: None,
                    closes_task: true,
                },
            )
            .await
            .expect("task end");
            assert_eq!(receipt.model_sesno, Some(END_SESNO), "收尾必须盖上模型水位");
        }
    }
}

async fn run_steps(db: &Surreal<Any>, steps: &[Step]) {
    for step in steps {
        run_step(db, step).await;
    }
}

/// 一根的模型面产物：几何行 `inst_relate` / `inst_info`（`dbnum` + `model_sesno`），record id 与
/// 生产同形（`<ref0>_<ref1>`）。逐根凭证行没有了（`gen_root` 退役，ADR-079）：模型面落库的只有几何行。
fn model_block(root: RefU64, sesno: i32) -> String {
    let key = root.to_string();
    format!(
        "BEGIN TRANSACTION;\n\
         INSERT INTO inst_info [{{ id: inst_info:⟨{key}⟩, dbnum: {DBNUM}, \
           model_sesno: {sesno} }}];\n\
         INSERT INTO inst_relate [{{ id: inst_relate:⟨{key}⟩, in: pe:⟨{key}⟩, out: inst_info:⟨{key}⟩, \
           dbnum: {DBNUM}, model_sesno: {sesno} }}];\n\
         COMMIT TRANSACTION;"
    )
}

/// 房间提交：房间边 + 面板边，`in` 是本库的 `pe` 行（回退按 `in.dbnum` 整库删）。
fn room_block(room: RefU64, panel: RefU64, member: RefU64) -> String {
    let (room_key, panel_key) = (room.to_string(), panel.to_string());
    format!(
        "BEGIN TRANSACTION;\n\
         INSERT RELATION INTO room_relate [{{ id: room_relate:⟨{room_key}⟩, in: {}, out: {}, \
           room_num: 'R301' }}];\n\
         INSERT RELATION INTO room_panel_relate [{{ id: room_panel_relate:⟨{panel_key}⟩, in: {}, out: {}, \
           room_num: 'R301' }}];\n\
         COMMIT TRANSACTION;",
        pe(room),
        pe(member),
        pe(panel),
        pe(room),
    )
}

/// ApplyWindow 一个任务的整条时间线：每元素一块的数据块 → 尾事务 → 三根各一块 → 房间 → 收尾。
fn apply_window_steps(f: &Fixture) -> Vec<Step> {
    let mut steps: Vec<Step> = production_batches_with_elements(f, 1)
        .into_iter()
        .map(Step::Data)
        .collect();
    steps.push(Step::DataTail { baseline: false });
    steps.extend(model_steps(f));
    steps.push(Step::Rooms(room_block(f.equi, f.reused, f.new_box)));
    steps.push(Step::TaskEnd);
    steps
}

fn model_steps(f: &Fixture) -> Vec<Step> {
    [f.equi, f.reused, f.new_box]
        .into_iter()
        .map(|root| Step::Model(model_block(root, END_SESNO)))
        .collect()
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
struct WatermarkPair {
    applied_sesno: Option<i32>,
    model_sesno: Option<i32>,
}

async fn watermark_pair(db: &Surreal<Any>) -> Option<WatermarkPair> {
    let mut response = db
        .query(format!(
            "SELECT applied_sesno, model_sesno FROM ONLY dbnum_watermark:{DBNUM};"
        ))
        .await
        .expect("watermark transport");
    response.take(0).expect("watermark row")
}

/// 一次成功与「崩溃 → 回退 → 重跑」之后的终态对拍：数据面逐表相等（`spatial_epoch` 是回退
/// 故意 bump 的控制面计数器，摘掉），两水位都在 T，写入标记盖成 `finished` 且窗口不变。
async fn assert_converged(
    db: &Surreal<Any>,
    expected: &BTreeMap<String, String>,
    start_sesno: i32,
    intent: WriteIntent,
    context: &str,
) {
    let replayed = snapshot_data_tables(db).await.expect("replayed snapshot");
    let mut changed = changed_data_tables(expected, &replayed);
    assert!(
        changed.remove("spatial_epoch"),
        "{context}: 回退必须在库侧留下空间痕迹（epoch bump）"
    );
    assert!(
        changed.is_empty(),
        "{context}: 回退重跑终态与一次成功不一致，差异表 {changed:?}\n\
         expected={expected:#?}\nreplayed={replayed:#?}"
    );
    assert_eq!(
        watermark_pair(db).await,
        Some(WatermarkPair {
            applied_sesno: Some(END_SESNO),
            model_sesno: Some(END_SESNO),
        }),
        "{context}: 两水位都该在 T"
    );
    let marker = crate::data_interface::write_state::load_on(db, DBNUM)
        .await
        .expect("load marker")
        .expect("marker row stays after finished");
    assert_eq!(
        marker.status, "finished",
        "{context}: 收尾那一笔盖成 finished"
    );
    assert_eq!(
        (marker.start_sesno, marker.end_sesno, marker.intent),
        (start_sesno, END_SESNO, intent),
        "{context}: 标记记的窗口与意图不随重跑漂移"
    );
    assert!(marker.finished_at.is_some(), "{context}: 收尾时刻必须留下");
}

/// ApplyWindow：数据第 k 块 / 尾事务后 / 模型第 k 块 / 房间提交前 / `task_end` 前，每一处 kill 一次
/// → 重启见 `writing` → `rollback_to_watermark(S, ApplyWindow)` → 重跑 → 逐表与一次成功相等。
/// 「房间提交前」= 最后一个模型块之后；「`task_end` 前」= 房间块之后。
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_window_task_killed_at_any_step_rolls_back_to_s_and_reruns_to_the_clean_state() {
    use crate::data_interface::fast_delete::rollback_to_watermark_on;

    let f = fixture();
    let steps = apply_window_steps(&f);
    assert_eq!(
        steps
            .iter()
            .filter(|step| matches!(step, Step::Model(_)))
            .count(),
        3,
        "三根各一块，模型第 k 块才有 k 可停"
    );

    let clean = seeded_db("task_clean").await;
    run_steps(&clean, &steps).await;
    let expected = snapshot_data_tables(&clean).await.expect("clean snapshot");
    assert_eq!(
        crate::data_interface::write_state::load_on(&clean, DBNUM)
            .await
            .unwrap()
            .map(|marker| marker.status),
        Some("finished".to_owned())
    );

    // 0 = 第一笔写库之前；steps.len() - 1 = task_end 之前（task_end 之后就是 finished，不是崩溃残留）。
    for killed_after in 0..steps.len() {
        let context = format!(
            "kill after step {killed_after} ({})",
            steps[..killed_after].last().map_or("start", Step::label)
        );
        let db = seeded_db(&format!("task_crash_{killed_after}")).await;
        run_steps(&db, &steps[..killed_after]).await;
        assert_eq!(
            write_state_status(&db).await.as_deref(),
            Some("writing"),
            "{context}: 崩溃残留必须是 writing"
        );

        // 重启的水化段。
        let report = rollback_to_watermark_on(&db, DBNUM, BASE_SESNO, WriteIntent::ApplyWindow)
            .await
            .expect("rollback to watermark");
        assert!(!report.wiped, "{context}");
        // 夹具的窗口前世界只有数据水位（模型水位是 NONE）：`math::min([NONE ?: 0, S])` = 0，
        // 而已经盖到 T 的那几档（尾事务后 / task_end 前）都得退回 S——崩溃不分面（D3）。
        assert_eq!(
            watermark_pair(&db).await,
            Some(WatermarkPair {
                applied_sesno: Some(BASE_SESNO),
                model_sesno: Some(0),
            }),
            "{context}: 两水位退到 S（崩溃不分面）"
        );
        assert_eq!(
            write_state_status(&db).await,
            None,
            "{context}: 回退删标记行"
        );
        let leftovers = ids(
            &db,
            format!(
                "SELECT VALUE record::id(id) FROM inst_relate WHERE dbnum = {DBNUM} \
                 AND model_sesno > {BASE_SESNO};"
            ),
        )
        .await;
        assert!(
            leftovers.is_empty(),
            "{context}: 模型面 sesno > S 的行必须删干净 {leftovers:?}"
        );

        // 重跑 S→T：开工标记 → 整条时间线。
        apply_all(
            &db,
            &[render_mark_writing(
                DBNUM,
                BASE_SESNO,
                END_SESNO,
                WriteIntent::ApplyWindow,
            )],
        )
        .await
        .expect("re-mark writing");
        run_steps(&db, &steps).await;
        assert_converged(
            &db,
            &expected,
            BASE_SESNO,
            WriteIntent::ApplyWindow,
            &context,
        )
        .await;
    }
}

/// ModelOnly：数据已经在 T、模型停在 S 的库上跑一个只写模型的任务，在模型第 k 块 kill。
///
/// 回退按标记记的 S = 模型水位走，但**不动数据面**（t-664 ⑥，`fast_delete::render_rollback`）：
/// 这类任务没写过数据，退数据面只会把上一窗已收口的 (S, applied] 数据行删掉、而重排的 ModelOnly
/// 又不补数据。所以回退只删 `model_sesno > S` 的模型行、`source_end_sesno > S` 的凭证、
/// 整库房间边，`model_sesno` 退到 S、`applied_sesno` 原地不动；重跑就是同意图的
/// ModelOnly S→T（模型 + 开关开启时的房间 + 收尾），终态与一次成功逐表相等。
#[tokio::test(flavor = "multi_thread")]
async fn a_model_only_task_killed_in_a_model_block_rolls_back_to_the_model_watermark_and_reruns() {
    use crate::data_interface::fast_delete::rollback_to_watermark_on;

    let f = fixture();
    // 数据面已到 T、模型 / 房间面停在 S 的库：数据块 + 尾事务，再把模型 / 房间水位摆在 S。
    async fn data_at_t(label: &str) -> Surreal<Any> {
        let f = fixture();
        let db = seeded_db(label).await;
        let mut steps: Vec<Step> = production_batches_with_elements(&f, 1)
            .into_iter()
            .map(Step::Data)
            .collect();
        steps.push(Step::DataTail { baseline: false });
        run_steps(&db, &steps).await;
        apply_all(
            &db,
            &[
                format!("UPDATE dbnum_watermark:{DBNUM} SET model_sesno = {BASE_SESNO};"),
                render_mark_writing(DBNUM, BASE_SESNO, END_SESNO, WriteIntent::ModelOnly),
            ],
        )
        .await
        .expect("model-only fixture");
        db
    }
    let mut model_only: Vec<Step> = model_steps(&f);
    model_only.push(Step::Rooms(room_block(f.equi, f.reused, f.new_box)));
    model_only.push(Step::TaskEnd);

    let clean = data_at_t("model_only_clean").await;
    run_steps(&clean, &model_only).await;
    let expected = snapshot_data_tables(&clean).await.expect("clean snapshot");

    // 数据面在崩溃前后都该原样：拿一份「数据在 T、还没跑模型」的快照当对照。
    let data_only = snapshot_data_tables(&data_at_t("model_only_data_only").await)
        .await
        .expect("data-only snapshot");

    for killed_after in 1..model_only.len() {
        let context = format!("model-only kill after {} step(s)", killed_after);
        let db = data_at_t(&format!("model_only_crash_{killed_after}")).await;
        run_steps(&db, &model_only[..killed_after]).await;
        assert_eq!(
            write_state_status(&db).await.as_deref(),
            Some("writing"),
            "{context}"
        );

        let report = rollback_to_watermark_on(&db, DBNUM, BASE_SESNO, WriteIntent::ModelOnly)
            .await
            .expect("rollback");
        assert!(!report.wiped, "{context}");
        assert_eq!(
            watermark_pair(&db).await,
            Some(WatermarkPair {
                applied_sesno: Some(END_SESNO),
                model_sesno: Some(BASE_SESNO),
            }),
            "{context}: 模型水位退回 S，数据水位原地不动"
        );
        // 数据面一行不少、模型面 / 房间面回到「还没跑模型」那一刻。被删空的表还留着表定义，
        // 对照库里则从没建过——「不存在」与「空」在这里是一回事。
        let after_rollback = snapshot_data_tables(&db)
            .await
            .expect("post-rollback snapshot");
        for (table, rows) in &data_only {
            assert_eq!(
                after_rollback.get(table),
                Some(rows),
                "{context}: 回退不得动 {table}（数据面）"
            );
        }
        for (table, rows) in &after_rollback {
            if table == "spatial_epoch" || data_only.contains_key(table) {
                continue;
            }
            // 空表的序列化形态是 `surrealdb::Value::Array([])`。
            assert!(
                rows == "[]" || rows == "{\"Array\":[]}",
                "{context}: 回退之后 {table} 必须清空: {rows}"
            );
        }

        // 重排同意图：ModelOnly S→T。
        apply_all(
            &db,
            &[render_mark_writing(
                DBNUM,
                BASE_SESNO,
                END_SESNO,
                WriteIntent::ModelOnly,
            )],
        )
        .await
        .expect("re-mark writing");
        run_steps(&db, &model_only).await;
        assert_converged(&db, &expected, BASE_SESNO, WriteIntent::ModelOnly, &context).await;
    }
}

/// Initialize：空库上的首次导入（S = 0）在第 k 步 kill → 回退 = 整库 wipe → 从头重排。
/// 数据块用基线的 `Fresh` 写法（只插不删），尾事务是基线那一句水位推进；模型 / 房间 / 收尾同上。
#[tokio::test(flavor = "multi_thread")]
async fn an_initialization_killed_at_any_step_wipes_and_reruns_from_scratch() {
    use crate::data_interface::fast_delete::rollback_to_watermark_on;

    let f = fixture();
    let steps = initialization_steps(&f);
    async fn empty_db(label: &str) -> Surreal<Any> {
        let db = fresh_mem_db("replay_parity", label)
            .await
            .expect("mem boots");
        init_schema_on(&db).await.expect("production schema");
        apply_all(
            &db,
            &[render_mark_writing(
                DBNUM,
                0,
                END_SESNO,
                WriteIntent::Initialize,
            )],
        )
        .await
        .expect("initialize marker");
        db
    }

    let clean = empty_db("init_clean").await;
    run_steps(&clean, &steps).await;
    let expected = snapshot_data_tables(&clean).await.expect("clean snapshot");
    assert!(
        expected.contains_key("pe") && expected.contains_key("inst_relate"),
        "一次成功的初始化必须真的写了数据面与模型面: {:?}",
        expected.keys().collect::<Vec<_>>()
    );
    assert!(
        !expected.contains_key("gen_root"),
        "逐根凭证表随 ADR-079 退役，落库不该再建它: {:?}",
        expected.keys().collect::<Vec<_>>()
    );

    for killed_after in 1..steps.len() {
        let context = format!(
            "init kill after step {killed_after} ({})",
            steps[killed_after - 1].label()
        );
        let db = empty_db(&format!("init_crash_{killed_after}")).await;
        run_steps(&db, &steps[..killed_after]).await;
        assert_eq!(
            write_state_status(&db).await.as_deref(),
            Some("writing"),
            "{context}"
        );

        let report = rollback_to_watermark_on(&db, DBNUM, 0, WriteIntent::Initialize)
            .await
            .expect("rollback = wipe");
        assert!(report.wiped, "{context}: 初始化的回退是整库 wipe");
        assert_eq!(report.target_watermark, 0);
        for table in [
            "pe",
            "EQUI",
            "BOX",
            "inst_relate",
            "inst_info",
            "room_relate",
            "room_panel_relate",
        ] {
            let rows = ids(&db, format!("SELECT VALUE <string>id FROM {table};")).await;
            assert!(
                rows.is_empty(),
                "{context}: wipe 之后 {table} 还剩 {rows:?}"
            );
        }
        assert_eq!(
            write_state_status(&db).await,
            None,
            "{context}: 标记行最后删"
        );

        apply_all(
            &db,
            &[render_mark_writing(
                DBNUM,
                0,
                END_SESNO,
                WriteIntent::Initialize,
            )],
        )
        .await
        .expect("re-mark writing");
        run_steps(&db, &steps).await;
        assert_converged(&db, &expected, 0, WriteIntent::Initialize, &context).await;
    }
}

/// 首次导入的世界：夹具里窗口前的五个元素全部以 Add 进库（`Fresh` 只插不删），每元素一块。
fn initialization_steps(f: &Fixture) -> Vec<Step> {
    let window = BTreeMap::from([(
        END_SESNO as u32,
        vec![
            EleOperationData::new(
                f.site,
                END_SESNO as u32,
                EleOperationDetail::Add(full_element(
                    f.site,
                    RefU64(0),
                    "SITE",
                    "/RP-SITE",
                    &[],
                    &[f.equi],
                )),
            ),
            EleOperationData::new(
                f.equi,
                END_SESNO as u32,
                EleOperationDetail::Add(full_element(
                    f.equi,
                    f.site,
                    "EQUI",
                    "/RP-EQUI",
                    &[("PURP", "old-purpose")],
                    &[f.gone, f.reused, f.new_box],
                )),
            ),
            EleOperationData::new(
                f.gone,
                END_SESNO as u32,
                EleOperationDetail::Add(full_element(f.gone, f.equi, "BOX", "/RP-GONE", &[], &[])),
            ),
            EleOperationData::new(
                f.reused,
                END_SESNO as u32,
                EleOperationDetail::Add(full_element(
                    f.reused,
                    f.equi,
                    "NOZZ",
                    "/RP-OLD-NOZZ",
                    &[],
                    &[],
                )),
            ),
            EleOperationData::new(
                f.new_box,
                END_SESNO as u32,
                EleOperationDetail::Add(full_element(
                    f.new_box,
                    f.equi,
                    "BOX",
                    "/RP-NEW-BOX",
                    &[],
                    &[],
                )),
            ),
        ],
    )]);
    let changes =
        crate::data_interface::ele_ops::terminal_persist_changes(&window, &HashMap::new());
    let limits = crate::data_interface::tx_limits::DataTxLimits {
        elements: 1,
        ..Default::default()
    };
    let (batches, _) = crate::data_interface::element_batch::render_transactions_with_limits(
        &changes,
        &[],
        DBNUM as i32,
        &crate::data_interface::uda_keys::UdaKeyMap::empty(),
        crate::data_interface::element_batch::WriteMode::Fresh,
        limits,
    )
    .unwrap();
    let mut steps: Vec<Step> = batches.into_iter().map(Step::Data).collect();
    steps.push(Step::DataTail { baseline: true });
    steps.extend(model_steps(f));
    steps.push(Step::Rooms(room_block(f.equi, f.reused, f.new_box)));
    steps.push(Step::TaskEnd);
    steps
}

// ───────────────────────────── 源码钉（ADR-072 W2，计划 R1 验收）─────────────────────────────

/// 数据尾事务**不写** `finished`、也不再删旧恢复记录 `increment_update_attempt`；`finished`
/// 全库只有任务收尾那一笔写得出、而且恰好一句（`closes_task = false` 的中途那一笔一个字都没有）。
#[test]
fn the_data_tail_never_writes_finished_and_the_task_end_writes_it_exactly_once() {
    let render = crate::data_interface::increment_tail::render_finalize_tail(
        DBNUM,
        END_SESNO,
        None,
        &ModelUpdatePlan::default(),
        &["UPDATE pe:x SET datacenter_version = 1;".to_owned()],
    );
    let tail = format!("{}\n{}", render.window_batches.join("\n"), render.tail);
    assert!(
        tail.contains("applied_sesno"),
        "尾事务至少推进数据水位: {tail}"
    );
    // `DELETE increment_update_attempt:{dbnum}` 是旧恢复记录的记录形式（`prepare_attempt` 写、
    // 尾事务删）——ADR-072 D1 之后尾事务不再删它。同一张表里 per-root attempts / `window_block`
    // 那两类行的清理（`WHERE dbnum = … AND kind IN […]`）是 R4 随 `staging/attempts.rs` 一起退的
    // 事，不在这条钉的口径里。
    for forbidden in [
        "finished",
        "DELETE increment_update_attempt:",
        WRITE_STATE_TABLE,
    ] {
        assert!(
            !tail.contains(forbidden),
            "数据尾事务不许出现 {forbidden:?}（ADR-072 D1 / W2）: {tail}"
        );
    }
    // 源码本身也钉一遍：渲染器改了形状、测试没跟上时，这一句先红。
    let production = include_str!("increment_tail.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap()
        .to_owned();
    for forbidden in [
        "'finished'",
        "STATUS_FINISHED",
        "render_mark_finished",
        "DELETE increment_update_attempt:",
    ] {
        assert!(
            !production.contains(forbidden),
            "increment_tail.rs 生产段不许出现 {forbidden:?}"
        );
    }

    let mut end = crate::data_interface::task_end::TaskEnd {
        dbnum: DBNUM,
        target_sesno: END_SESNO,
        data: None,
        target_time: None,
        model: Some(crate::data_interface::task_end::TaskModelOutcome {
            all_roots_ok: true,
            dead_roots: 0,
            catalogue_sessions: None,
        }),
        info: None,
        closes_task: true,
    };
    let closing = crate::data_interface::task_end::render_task_end_tail(&end);
    assert_eq!(
        closing.matches("status = 'finished'").count(),
        1,
        "收尾那一笔恰含一句 finished: {closing}"
    );
    assert!(
        closing.contains(&format!(
            "UPDATE {WRITE_STATE_TABLE}:{DBNUM} SET status = 'finished'"
        )),
        "{closing}"
    );
    end.closes_task = false;
    let midway = crate::data_interface::task_end::render_task_end_tail(&end);
    assert_eq!(
        midway.matches("finished").count(),
        0,
        "中途那一笔（数据批次之后先落 Ref0 统计）一个 finished 都不许有: {midway}"
    );
}
