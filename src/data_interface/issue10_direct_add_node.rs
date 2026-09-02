//! Issue #10（https://github.com/happyrust/gen-model/issues/10）在**直写路径**上的对应用例：
//! 「多次增量后，程序只能检测到增量变化，但模型树中看不到新增的节点」。
//!
//! 现场形态：E3D 里复制一条 BRAN（Copy-of-1WCC0211-...）并 SAVEWORK，增量扫描能看到变化，
//! 但查看器模型树里 PIPE 下只有旧的那条 BRAN。模型树的数据源是持久层的 `pe` 行 +
//! `pe_owner` 入边（rs-core `get_children_refnos` / `get_children_pes`），所以「新增节点可见」
//! 等价于两件事同时落库：
//!
//! 1. 新元素自己的 `pe` 行（Add 渲染的 `UPSERT pe CONTENT`）；
//! 2. 父元素 `children_changed` 重建的 `pe_owner` 边（`DELETE owner<-pe_owner` + 带显式成员序
//!    id 的 `INSERT RELATION`）。
//!
//! ADR-056 P1 之后没有暂存窗口与 journal：这两笔写由 `render_persist_statements` 渲染后**直接**
//! 打在持久层（`persist_latest_main_data` 同一份渲染），水位由 `finalize_attempt` 的尾事务单独
//! 推进。本模块只验数据面：连续两次增量后树可见、成员序正确、兄弟子树不受伤、水位不被
//! 持久化阶段碰到。前身 `staging/issue10_add_node.rs` 借真实暂存窗口 + 写回模拟同一场景，
//! 其中「窗口阻断 / 毒语句卡死写回」两条症状随暂存层一起退役（spec 035 T304）。

#![cfg(test)]

use std::collections::BTreeMap;

use aios_core::NamedAttrValue;
use aios_core::pdms_types::*;
use parse_pdms_db::parse::EleData;
use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use crate::data_interface::increment_pipeline::IncrementPipeline;
use crate::data_interface::table_parity::{apply_all, fresh_mem_db, init_schema_on};

const DBNUM: u32 = 7997;

/// 保留段 refno（与仓内其它夹具一致，避开真实项目数据）。
fn refu(n: u64) -> RefU64 {
    RefU64((4000000001u64 << 32) | n)
}

fn pe_key(refno: RefU64) -> String {
    RefnoEnum::from(refno).to_pe_key()
}

const SITE: u64 = 1;
const ZONE: u64 = 2;
const PIPE: u64 = 3;
const BRAN1: u64 = 10;
const BRAN2: u64 = 20;
const TUBI2: u64 = 21;
const BRAN3: u64 = 30;

/// 一个 Add 操作：属性映射带 REFNO / TYPE / NAME / OWNER / DBNUM（渲染器的 id、owner 全部
/// 取自属性映射，缺 REFNO 会渲染出 id 冲突语句）。
fn add_op(
    refno: RefU64,
    owner: RefU64,
    noun: &str,
    name: &str,
    children: Vec<RefU64>,
    sesno: u32,
) -> EleOperationData {
    let mut ele = EleData::default();
    ele.refno = refno;
    ele.owner = owner;
    ele.name = name.to_string();
    ele.children = RefU64Vec(children);
    let map = &mut ele.whole_attmap.attmap.map;
    map.insert("REFNO".to_string(), NamedAttrValue::RefU64Type(refno));
    map.insert("OWNER".to_string(), NamedAttrValue::RefU64Type(owner));
    map.insert(
        "TYPE".to_string(),
        NamedAttrValue::StringType(noun.to_string()),
    );
    map.insert(
        "NAME".to_string(),
        NamedAttrValue::StringType(name.to_string()),
    );
    map.insert(
        "DBNUM".to_string(),
        NamedAttrValue::IntegerType(DBNUM as i32),
    );
    EleOperationData::new(refno, sesno, EleOperationDetail::Add(ele))
}

/// 父元素成员表变化（E3D 复制 BRAN 时，会话里同时出现 PIPE 的 Modified）。
fn children_changed_op(
    owner: RefU64,
    noun: &str,
    old: Vec<RefU64>,
    new: Vec<RefU64>,
    sesno: u32,
) -> EleOperationData {
    EleOperationData::new(
        owner,
        sesno,
        EleOperationDetail::Modified(ModifiedElement {
            current_data: EleData::default(),
            added_attrs: Default::default(),
            deleted_attrs: Default::default(),
            modified_attrs: Default::default(),
            added_explicit_attrs: Default::default(),
            deleted_explicit_attrs: Default::default(),
            modified_explicit_attrs: Default::default(),
            added_uda_attrs: Default::default(),
            deleted_uda_attrs: Default::default(),
            modified_uda_attrs: Default::default(),
            noun: noun.to_string(),
            children_changed: Some((RefU64Vec(old), RefU64Vec(new))),
        }),
    )
}

/// 扮演持久层的 mem 实例：生产同款 schema + 基线层级
/// SITE → ZONE → PIPE(/1WCC0211) → BRAN1，水位 applied_sesno = 1。
async fn persistent_with_baseline() -> Surreal<Any> {
    let target = fresh_mem_db("issue10", "direct").await.expect("mem boots");
    init_schema_on(&target).await.expect("target schema");

    let baseline = format!(
        "UPSERT {site} CONTENT {{ noun: 'SITE', name: '/1WCC-PIPEBJ', dbnum: {DBNUM}, sesno: 1, deleted: false }};\n\
         UPSERT {zone} CONTENT {{ noun: 'ZONE', name: '/1WCC-PIPE-RX', dbnum: {DBNUM}, sesno: 1, deleted: false, owner: {site} }};\n\
         UPSERT {pipe} CONTENT {{ noun: 'PIPE', name: '/1WCC0211', dbnum: {DBNUM}, sesno: 1, deleted: false, owner: {zone} }};\n\
         UPSERT {bran1} CONTENT {{ noun: 'BRAN', name: '/1WCC0211-114.3-NADB-R52-R710', dbnum: {DBNUM}, sesno: 1, deleted: false, owner: {pipe} }};\n\
         INSERT RELATION INTO pe_owner [\
           {{ id: pe_owner:[{site}, 0], in: {zone}, out: {site} }},\
           {{ id: pe_owner:[{zone}, 0], in: {pipe}, out: {zone} }},\
           {{ id: pe_owner:[{pipe}, 0], in: {bran1}, out: {pipe} }}\
         ];\n\
         UPSERT dbnum_watermark:{DBNUM} SET dbnum = {DBNUM}, applied_sesno = 1;",
        site = pe_key(refu(SITE)),
        zone = pe_key(refu(ZONE)),
        pipe = pe_key(refu(PIPE)),
        bran1 = pe_key(refu(BRAN1)),
    );
    target
        .query(baseline)
        .await
        .expect("baseline transport")
        .check()
        .expect("baseline applied");
    target
}

/// 模型树取子节点的查询——与 rs-core `get_children_refnos_uncached` 同形
/// （沿 pe_owner 入边、滤幽灵行与软删行），返回排序后的 refno 文本。
async fn viewer_children(db: &Surreal<Any>, owner: RefU64) -> Vec<String> {
    let sql = format!(
        "SELECT VALUE record::id(in) FROM {}<-pe_owner \
         WHERE in.id != NONE AND record::exists(in.id) AND !in.deleted;",
        pe_key(owner)
    );
    let mut response = db.query(sql).await.expect("tree query transport");
    let mut children: Vec<String> = response.take(0).expect("tree query rows");
    children.sort();
    children
}

async fn applied_sesno(db: &Surreal<Any>) -> Option<i32> {
    let mut response = db
        .query(format!("RETURN dbnum_watermark:{DBNUM}.applied_sesno;"))
        .await
        .expect("watermark transport");
    response.take(0).expect("watermark value")
}

/// 直写路径的数据面：生产同一份渲染（主数据 + 反向索引）直接打在持久层。
/// 水位不在这里推进——那是 `finalize_attempt` 尾事务的事。
async fn run_direct_increment(db: &Surreal<Any>, range: &BTreeMap<u32, Vec<EleOperationData>>) {
    let statements = IncrementPipeline::render_persist_statements(range, DBNUM as i32)
        .into_iter()
        .chain(crate::data_interface::manual_update::build_reverse_index_statements(range))
        .collect::<Vec<_>>();
    assert!(!statements.is_empty(), "检测到的变化必须渲染出落库语句");
    apply_all(db, &statements).await.expect("direct persist");
}

/// E3D「复制 BRAN」的会话形态：新 BRAN（及其子元素）的 Add + 父 PIPE 的成员表变化。
fn copy_branch_session(
    sesno: u32,
    new_bran: u64,
    new_bran_children: Vec<u64>,
    pipe_children_before: Vec<u64>,
) -> BTreeMap<u32, Vec<EleOperationData>> {
    let mut ops = vec![add_op(
        refu(new_bran),
        refu(PIPE),
        "BRAN",
        &format!("/Copy-of-1WCC0211-{new_bran}"),
        new_bran_children.iter().map(|&n| refu(n)).collect(),
        sesno,
    )];
    for &child in &new_bran_children {
        ops.push(add_op(
            refu(child),
            refu(new_bran),
            "TUBI",
            &format!("/Copy-of-1WCC0211-{new_bran}-T{child}"),
            Vec::new(),
            sesno,
        ));
    }
    let mut pipe_children_after: Vec<RefU64> =
        pipe_children_before.iter().map(|&n| refu(n)).collect();
    pipe_children_after.push(refu(new_bran));
    ops.push(children_changed_op(
        refu(PIPE),
        "PIPE",
        pipe_children_before.iter().map(|&n| refu(n)).collect(),
        pipe_children_after,
        sesno,
    ));
    BTreeMap::from([(sesno, ops)])
}

/// 连续多次增量新增节点，每次直写之后模型树都能看见新 BRAN。
///
/// 这正是 issue #10 报「失效」的路径：第 1 次增量加 BRAN2（带子 TUBI），第 2 次增量再加
/// BRAN3。任何一环把 Add 或父成员表重建丢掉，这里的树查询立刻空悬。
#[tokio::test(flavor = "multi_thread")]
async fn added_branches_land_in_the_model_tree_across_consecutive_direct_increments() {
    let target = persistent_with_baseline().await;
    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10"]
    );

    // 增量 #1：复制出 BRAN2（子元素 TUBI2）。
    run_direct_increment(
        &target,
        &copy_branch_session(2, BRAN2, vec![TUBI2], vec![BRAN1]),
    )
    .await;
    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10", "4000000001_20"],
        "第一次增量后新 BRAN 必须出现在模型树"
    );
    assert_eq!(
        viewer_children(&target, refu(BRAN2)).await,
        vec!["4000000001_21"],
        "新 BRAN 的子树边也必须落库"
    );

    // 增量 #2：再复制出 BRAN3——「多次增量」的第二拍。
    run_direct_increment(
        &target,
        &copy_branch_session(3, BRAN3, Vec::new(), vec![BRAN1, BRAN2]),
    )
    .await;
    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10", "4000000001_20", "4000000001_30"],
        "第二次增量的新 BRAN 也必须出现——issue #10 的现场正是这一步之后看不见"
    );
    assert_eq!(
        viewer_children(&target, refu(BRAN2)).await,
        vec!["4000000001_21"],
        "父成员表重建（DELETE+INSERT RELATION）不得误伤兄弟分支的子树"
    );

    // 成员序 = pe_owner 复合 id 的下标位：树的显示顺序靠它。
    let mut response = target
        .query(format!(
            "RETURN [record::id(pe_owner:[{pipe}, 0].in), record::id(pe_owner:[{pipe}, 1].in), record::id(pe_owner:[{pipe}, 2].in)];",
            pipe = pe_key(refu(PIPE))
        ))
        .await
        .expect("member order transport");
    let order: Vec<Option<String>> = response.take(0).expect("member order");
    assert_eq!(
        order,
        vec![
            Some("4000000001_10".into()),
            Some("4000000001_20".into()),
            Some("4000000001_30".into())
        ],
        "成员序必须与父成员表一致"
    );

    // 数据面不碰水位：两次增量之后 applied_sesno 仍是基线的 1（推进只在尾事务）。
    assert_eq!(
        applied_sesno(&target).await,
        Some(1),
        "持久化阶段不得推进水位，水位属于 finalize_attempt 尾事务"
    );
}
