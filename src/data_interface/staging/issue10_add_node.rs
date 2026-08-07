//! Issue #10 模拟（https://github.com/happyrust/gen-model/issues/10）：
//! 「多次增量后，程序只能检测到增量变化，但模型树中看不到新增的节点」。
//!
//! 现场形态：E3D 里复制一条 BRAN（Copy-of-1WCC0211-...）并 SAVEWORK，增量扫描
//! 能看到变化，但查看器模型树里 PIPE 下只有旧的那条 BRAN。模型树的数据源是
//! 持久层的 pe 行 + pe_owner 入边（rs-core get_children_refnos /
//! get_children_pes），所以「新增节点可见」等价于两件事同时落库：
//!
//! 1. 新元素自己的 pe 行（Add 渲染的 UPSERT pe CONTENT）；
//! 2. 父元素 children_changed 重建的 pe_owner 边（DELETE owner<-pe_owner
//!    + 带显式成员序 id 的 INSERT RELATION）。
//!
//! ADR-017 之后这两笔写都先进 kv-mem 暂存窗口、随 journal 写回。本模块用真实
//! 渲染与真实窗口设施（stage_parsed_window → register_staged_finalize →
//! commit_registered_to）在 mem 引擎上模拟连续多次增量，钉住三件事：
//!
//! - 【期望行为】连续多个暂存窗口各自新增节点，每次写回后模型树查询都能看到；
//! - 【issue #10 的症状】窗口因生成失败被阻断（A 语义：整批才落盘）时，解析
//!   明明检测到了新增（journal 里有语句），持久层的树却纹丝不动——直到吸收
//!   重算解除阻断；
//! - 【issue #10 的另一形态】journal 里的语句被持久层确定性拒绝（例如坏版
//!   update_dbnum_event 对字符串 id 的 pe 行报 array::at 类型错）时，写回
//!   卡死、水位不动、树不更新；排除毒语句后同一份 journal 重放收敛。

#![cfg(test)]

use std::collections::BTreeMap;

use aios_core::NamedAttrValue;
use aios_core::pdms_types::*;
use parse_pdms_db::parse::EleData;
use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement};
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};

use super::lifecycle::{ActiveStagedWindow, create_window_on, init_staging_schema};
use super::resources::ResourceThresholds;
use super::{StagedFinalize, register_staged_finalize};
use crate::data_interface::increment_pipeline::IncrementPipeline;

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

/// 一个 Add 操作：属性映射带 REFNO / TYPE / NAME / OWNER / DBNUM（渲染器的 id、
/// owner 全部取自属性映射，缺 REFNO 会渲染出 id 冲突语句——见 parity 测试的教训）。
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

/// 扮演 fork 持久层的 mem 实例：生产同款 schema + 基线层级
/// SITE → ZONE → PIPE(/1WCC0211) → BRAN1，水位 applied_sesno = 1。
async fn persistent_with_baseline() -> Surreal<Any> {
    let target = connect("mem://").await.expect("mem boots");
    target
        .use_ns("issue10")
        .use_db("persistent")
        .await
        .expect("use persistent db");
    init_staging_schema(&target).await.expect("target schema");

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

/// 模型树取子节点的查询——与 rs-core get_children_refnos_uncached 同形
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

/// 把一批解析操作装进一个真实暂存窗口（真实渲染 + 真实 journal），登记 finalize
/// ——与 batch_worker::execute_frozen_batch 成功路径同一套设施。
async fn staged_window_with(
    range: &BTreeMap<u32, Vec<EleOperationData>>,
    start_sesno: i32,
    end_sesno: i32,
) -> ActiveStagedWindow {
    let instance = connect("mem://").await.expect("staging mem boots");
    let mut window = create_window_on(
        &instance,
        DBNUM,
        start_sesno,
        end_sesno,
        ResourceThresholds::default(),
    )
    .await
    .expect("create staged window");
    let staged = IncrementPipeline::stage_parsed_window(&mut window, range, DBNUM)
        .await
        .expect("stage parsed window");
    assert!(
        staged > 0,
        "解析必须检测到变化并进 journal（issue #10 的前半句）"
    );
    window
        .scope(register_staged_finalize(StagedFinalize {
            dbnum: DBNUM,
            end_sesno,
            plan: Default::default(),
            window_statements: Vec::new(),
            cache_refnos: Vec::new(),
        }))
        .await
        .expect("register finalize");
    window
}

/// 一次成功的增量：装窗 → 写回 → 拆窗。
async fn run_staged_increment(
    target: &Surreal<Any>,
    range: &BTreeMap<u32, Vec<EleOperationData>>,
    start_sesno: i32,
    end_sesno: i32,
) {
    let window = staged_window_with(range, start_sesno, end_sesno).await;
    window
        .commit_registered_to(target)
        .await
        .expect("staged write-back");
    window.drop_database().await.expect("drop staging db");
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

/// 【期望行为】连续多次增量新增节点，每次窗口写回后模型树都能看见新 BRAN。
///
/// 这正是 issue #10 报「失效」的路径：第 1 次增量加 BRAN2（带子 TUBI），第 2 次
/// 增量再加 BRAN3，第 3 次增量只改名。任何一环把 Add 或父成员表重建丢掉，
/// 这里的树查询立刻空悬。
#[tokio::test(flavor = "multi_thread")]
async fn added_branches_land_in_the_model_tree_across_consecutive_staged_increments() {
    let target = persistent_with_baseline().await;
    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10"]
    );

    // 增量 #1：复制出 BRAN2（子元素 TUBI2）。
    let first = copy_branch_session(2, BRAN2, vec![TUBI2], vec![BRAN1]);
    run_staged_increment(&target, &first, 2, 2).await;

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
    assert_eq!(applied_sesno(&target).await, Some(2), "写回尾事务推进水位");

    // 增量 #2：再复制出 BRAN3——「多次增量」的第二拍。
    let second = copy_branch_session(3, BRAN3, Vec::new(), vec![BRAN1, BRAN2]);
    run_staged_increment(&target, &second, 3, 3).await;

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
    assert_eq!(applied_sesno(&target).await, Some(3));

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
        "成员序边必须按最新 children 列表重建"
    );

    // 增量 #3：改名（Modified 带 NAME）——多次增量后旧节点的更新也要继续生效。
    let mut rename = ModifiedElement {
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
        noun: "BRAN".to_string(),
        children_changed: None,
    };
    rename.modified_explicit_attrs.insert(
        "NAME".to_string(),
        (
            NamedAttrValue::StringType("/Copy-of-1WCC0211-20".into()),
            NamedAttrValue::StringType("/Copy-renamed".into()),
        ),
    );
    let third = BTreeMap::from([(
        4u32,
        vec![EleOperationData::new(
            refu(BRAN2),
            4,
            EleOperationDetail::Modified(rename),
        )],
    )]);
    run_staged_increment(&target, &third, 4, 4).await;

    let mut response = target
        .query(format!("RETURN {}.name;", pe_key(refu(BRAN2))))
        .await
        .expect("name transport");
    let name: Option<String> = response.take(0).expect("name value");
    assert_eq!(name.as_deref(), Some("/Copy-renamed"));
    assert_eq!(
        viewer_children(&target, refu(PIPE)).await.len(),
        3,
        "改名不得影响树的成员"
    );
}

/// 【issue #10 的症状 · 窗口阻断形态】生成根重试穷尽 → 窗口废弃（A 语义：整批
/// 才落盘）→ 解析明明检测到了新增（journal 非空），持久层的模型树却纹丝不动、
/// 水位原地——「程序只能检测到增量变化，但是在模型树中看不到新增的节点」。
/// 修复源数据重存 → 吸收重置 attempts → 重算窗口写回 → 树收敛。
#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_window_reproduces_issue_10_and_absorb_reset_recovers() {
    use super::attempts;
    use crate::data_interface::model_update_pending::MAX_ATTEMPTS;

    let target = persistent_with_baseline().await;
    let bad_root = RefnoEnum::from(refu(BRAN2)).to_pdms_str();

    // 增量到达并被检测（装窗成功、journal 非空）……
    let session = copy_branch_session(2, BRAN2, vec![TUBI2], vec![BRAN1]);
    let window = staged_window_with(&session, 2, 2).await;
    assert!(!window.journal().await.is_empty(), "变化已被检测并暂存");

    // ……但新 BRAN 的模型生成反复失败到达阻断门槛（batch_worker 的 staged 分支行为）。
    let mut attempts_count = 0;
    for i in 0..MAX_ATTEMPTS {
        attempts_count = attempts::record_root_failure_on(
            &target,
            DBNUM,
            &bad_root,
            &format!("staged generation failed #{i}"),
        )
        .await
        .expect("record root failure");
    }
    assert!(attempts::reaches_block_threshold(attempts_count));
    attempts::record_window_block_on(
        &target,
        DBNUM,
        "模型生成重试已耗尽",
        std::slice::from_ref(&bad_root),
    )
    .await
    .expect("record window block");
    // 窗口废弃：持久层零落盘。
    window.drop_database().await.expect("abandon window");

    // issue #10 的截图状态：树里只有旧 BRAN，新增节点不可见，水位没动。
    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10"],
        "阻断窗口不得留下任何可见的新节点"
    );
    let mut response = target
        .query(format!("RETURN record::exists({});", pe_key(refu(BRAN2))))
        .await
        .expect("exists transport");
    let exists: Option<bool> = response.take(0).expect("exists value");
    assert_eq!(exists, Some(false), "新 BRAN 的 pe 行不得半写落盘");
    assert_eq!(applied_sesno(&target).await, Some(1), "水位必须原地");
    assert!(
        attempts::load_window_block_on(&target, DBNUM)
            .await
            .expect("load block")
            .is_some(),
        "阻断记录必须在控制面可见（面板/告警的依据）"
    );

    // 修复重存 → 新会话吸收 → 重置受影响根 → 同一区间重算并写回。
    attempts::reset_roots_on_absorb_on(&target, DBNUM, std::slice::from_ref(&bad_root))
        .await
        .expect("absorb reset");
    assert!(
        attempts::load_window_block_on(&target, DBNUM)
            .await
            .expect("load block")
            .is_none(),
        "全部坏根被新数据触及后阻断解除"
    );
    let retry = copy_branch_session(2, BRAN2, vec![TUBI2], vec![BRAN1]);
    run_staged_increment(&target, &retry, 2, 3).await;

    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10", "4000000001_20"],
        "阻断解除后重算窗口必须让新节点出现在模型树"
    );
    assert_eq!(applied_sesno(&target).await, Some(3));
}

/// 【issue #10 的另一形态 · 写回中毒】持久层对 journal 语句确定性报错（这里用
/// F4 记录在案的坏版 update_dbnum_event：字符串 id 的 pe 行 UPSERT/UPDATE 因
/// array::at 类型错整语句失败）时：写回失败、水位不动、树看不见新节点——而
/// 暂存库里一切正常，batch_worker 的 retry_until_recovered 会带着这个窗口
/// 无限重试（写回滞留告警），单 worker 从此不再消费队列，外在表现就是
/// 「程序失效，只剩检测」。毒源排除后，同一份 journal 整体重放收敛。
#[tokio::test(flavor = "multi_thread")]
async fn a_poisoned_write_back_stalls_the_window_but_replay_converges_after_repair() {
    let target = persistent_with_baseline().await;
    // 坏版事件（rs-core 旧实现，fork 与 mem 行为一致，见 fork_surreal_compat F4）。
    target
        .query(
            r#"DEFINE EVENT OVERWRITE update_dbnum_event ON pe WHEN $event = "CREATE" OR $event = "UPDATE" OR $event = "DELETE" THEN { LET $id = record::id($value.id); LET $ref_0 = array::at($id, 0); UPSERT type::thing('dbnum_info_table', $ref_0) SET count = count?:0 + 1; };"#,
        )
        .await
        .expect("bad event transport")
        .check()
        .expect("bad event defined");

    let session = copy_branch_session(2, BRAN2, Vec::new(), vec![BRAN1]);
    let window = staged_window_with(&session, 2, 2).await;

    let error = window
        .commit_registered_to(&target)
        .await
        .err()
        .expect("坏事件在场时写回必须失败");
    assert!(
        format!("{error:#}").contains("写回块")
            || format!("{error:#}").contains("statement failed"),
        "错误应指向写回重放: {error:#}"
    );

    // I1：失败的写回块整体回滚，持久层零痕迹——树保持旧状、水位原地。
    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10"],
        "写回中毒期间模型树不得出现半写节点"
    );
    assert_eq!(applied_sesno(&target).await, Some(1));

    // 排毒（运维动作：换回 string::split 好版或移除事件）后，同一窗口原样重试。
    target
        .query("REMOVE EVENT update_dbnum_event ON pe;")
        .await
        .expect("remove event transport")
        .check()
        .expect("event removed");
    window
        .commit_registered_to(&target)
        .await
        .expect("journal 保留在窗口里，排毒后整体重放必须收敛");
    window.drop_database().await.expect("cleanup");

    assert_eq!(
        viewer_children(&target, refu(PIPE)).await,
        vec!["4000000001_10", "4000000001_20"],
        "排毒后新节点必须出现在模型树"
    );
    assert_eq!(applied_sesno(&target).await, Some(2));
}
