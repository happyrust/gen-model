//! mini-window parity harness（开发方案 T0.6，黄金等价测试）。
//!
//! 不依赖生成管线的小型窗口：insert / update / delete / relation / fn:: 调用 /
//! commit-time-only 各一条，同一脚本走两条路径——
//!
//! - **暂存路径**：预载（StagingOnly）→ 窗口语句经 StagedExecutor（Both /
//!   CommitOnly）→ `commit_to` 分块写回 + 尾事务；
//! - **直写路径**：同一批语句按原始顺序直接打在持久层上（今天的行为）。
//!
//! 唯一硬标准（I4）：两条路径的持久层终态**逐表相等**；附带 I1 探针：写回之前
//! 持久层不得有任何变化。后续每个接入阶段（P1 解析、P2 生成、P3 房间）都先在
//! 本 harness 上加对应形态的语句再动真实管线。

#![cfg(test)]

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};

use super::executor::{ExecMode, StagedExecutor};
use super::lifecycle::init_staging_schema;

/// 一个 mini 窗口脚本。
pub(crate) struct MiniWindowScript {
    /// 两条路径共同的持久层基态（窗口开始前就存在的数据）。
    pub base: Vec<String>,
    /// 暂存路径的预载：把窗口要读的既有行拷进暂存（StagingOnly，不进日志）。
    /// 直写路径不需要它——数据本来就在持久层。
    pub preload: Vec<String>,
    /// 窗口语句（按执行顺序）。
    pub steps: Vec<(String, ExecMode)>,
    /// 尾事务（水位收口等）。
    pub tail: Option<String>,
}

// 快照 / diff / 载体助手的本体已抽到与暂存无关的 `table_parity`（spec 035 T171），
// 这里只剩 panic 风格的薄包装，P3 删本文件时随之消失。

async fn fresh_db(ns: &str, db: &str) -> Surreal<Any> {
    crate::data_interface::table_parity::fresh_mem_db(ns, db)
        .await
        .expect("mem boots")
}

async fn apply_all(db: &Surreal<Any>, statements: &[String]) {
    crate::data_interface::table_parity::apply_all(db, statements)
        .await
        .unwrap_or_else(|error| panic!("{error:#}"));
}

pub(crate) async fn snapshot_data_tables(
    db: &Surreal<Any>,
) -> std::collections::BTreeMap<String, String> {
    crate::data_interface::table_parity::snapshot_data_tables(db)
        .await
        .expect("data snapshot")
}

pub(crate) use crate::data_interface::table_parity::changed_data_tables;

pub(crate) async fn snapshot_tables(db: &Surreal<Any>) -> String {
    crate::data_interface::table_parity::snapshot_tables(db)
        .await
        .expect("snapshot")
}

/// 跑双路径并返回（暂存路径终态, 直写路径终态, 写回前的持久层快照, 基态快照）。
pub(crate) async fn run_both_paths(script: &MiniWindowScript) -> (String, String, String, String) {
    // 暂存路径。
    let staged_target = fresh_db("parity", "staged_target").await;
    init_staging_schema(&staged_target)
        .await
        .expect("target schema");
    apply_all(&staged_target, &script.base).await;
    let base_snapshot = snapshot_tables(&staged_target).await;

    let staging = fresh_db("staging", "staging_7997_parity").await;
    init_staging_schema(&staging).await.expect("staging schema");
    let mut executor = StagedExecutor::new(staging, "staging_7997_parity");
    for sql in &script.preload {
        executor
            .execute(sql.clone(), ExecMode::StagingOnly)
            .await
            .expect("preload");
    }
    for (sql, mode) in &script.steps {
        executor.execute(sql.clone(), *mode).await.expect("step");
    }
    // I1 探针：写回之前，持久层与基态一字不差（零落盘）。
    let before_commit = snapshot_tables(&staged_target).await;
    executor
        .commit_to(&staged_target, &[], script.tail.as_deref())
        .await
        .expect("commit");
    let staged_final = snapshot_tables(&staged_target).await;

    // 直写路径：同一批语句按原始顺序直接执行（今天的行为）。
    let direct_target = fresh_db("parity", "direct_target").await;
    init_staging_schema(&direct_target)
        .await
        .expect("target schema");
    apply_all(&direct_target, &script.base).await;
    for (sql, _mode) in &script.steps {
        direct_target
            .query(sql)
            .await
            .expect("direct transport")
            .check()
            .unwrap_or_else(|e| panic!("direct failed: {sql}\n{e}"));
    }
    if let Some(tail) = &script.tail {
        direct_target
            .query(tail)
            .await
            .expect("direct tail transport")
            .check()
            .expect("direct tail");
    }
    let direct_final = snapshot_tables(&direct_target).await;

    (staged_final, direct_final, before_commit, base_snapshot)
}

/// P1 语句级对拍：**真实渲染管线**产出的窗口（`render_persist_statements` 的
/// Added / Deleted + `render_finalize_tail` 的收口）走「executor 暂存 + 写回」
/// 与「直写」两条路径，逐表终态相等。这是 T1.1/T1.3 接入的黄金验收——
/// 渲染是同一份，差异只可能来自执行介质。
#[tokio::test(flavor = "multi_thread")]
async fn staged_parse_window_with_real_rendering_matches_direct_write() {
    use crate::data_interface::increment_pipeline::IncrementPipeline;
    use crate::data_interface::model_update_pending::render_finalize_tail;
    use crate::data_interface::model_update_plan::ModelUpdatePlan;
    use aios_core::{NamedAttrValue, RefU64};
    use parse_pdms_db::parse::EleData;
    use pdms_io::io::{EleOperationData, EleOperationDetail};
    use std::collections::BTreeMap;

    let added_refno = RefU64((7997u64 << 32) | 11);
    let deleted_refno = RefU64((7997u64 << 32) | 12);

    let mut ele = EleData::default();
    ele.refno = added_refno;
    // 渲染器（NamedAttrMap::pe / gen_sur_json）的 id、refno、owner 全部取自
    // 属性映射的 REFNO / OWNER 属性而非 EleData 字段——缺 REFNO 会渲染出
    // 「目标 BOX:7997_11、CONTENT id "0_0"」的 id 冲突语句，两条路径都会失败。
    ele.whole_attmap
        .attmap
        .map
        .insert("REFNO".to_string(), NamedAttrValue::RefU64Type(added_refno));
    ele.whole_attmap.attmap.map.insert(
        "TYPE".to_string(),
        NamedAttrValue::StringType("BOX".to_string()),
    );
    ele.whole_attmap
        .attmap
        .map
        .insert("DBNUM".to_string(), NamedAttrValue::IntegerType(7997));

    let mut range_eles: BTreeMap<u32, Vec<EleOperationData>> = BTreeMap::new();
    range_eles.insert(
        43,
        vec![
            EleOperationData::new(added_refno, 43, EleOperationDetail::Add(ele)),
            EleOperationData::new(deleted_refno, 43, EleOperationDetail::Deleted),
        ],
    );

    let statements = IncrementPipeline::render_persist_statements(&range_eles, 7997);
    assert!(
        !statements.is_empty(),
        "真实渲染必须产出语句，否则对拍对象是空集"
    );
    // 两条路径不是同一时刻执行；信息性时间戳不属于终态等价判据。
    let tail = render_finalize_tail(7997, 43, None, &ModelUpdatePlan::default(), &[])
        .tail
        .replace("time::now()", "NONE");

    // 暂存路径。
    let staging = fresh_db("staging", "staging_7997_real").await;
    let mut executor = StagedExecutor::new(staging, "staging_7997_real");
    for sql in &statements {
        executor
            .execute(sql.clone(), ExecMode::Both)
            .await
            .unwrap_or_else(|e| {
                panic!("真实渲染语句必须过 validator 并在暂存执行成功:\n{sql}\n{e}")
            });
    }
    let staged_target = fresh_db("parity", "real_staged_target").await;
    executor
        .commit_to(&staged_target, &[], Some(&tail))
        .await
        .expect("staged commit");

    // 直写路径（今天的行为：分块直写 + finalize 事务）。
    let direct_target = fresh_db("parity", "real_direct_target").await;
    for sql in &statements {
        direct_target
            .query(sql)
            .await
            .expect("direct transport")
            .check()
            .unwrap_or_else(|e| panic!("direct failed: {sql}\n{e}"));
    }
    direct_target
        .query(format!("BEGIN TRANSACTION;\n{tail}\nCOMMIT TRANSACTION;"))
        .await
        .expect("direct tail transport")
        .check()
        .expect("direct tail");

    let staged_final = snapshot_tables(&staged_target).await;
    let direct_final = snapshot_tables(&direct_target).await;
    assert_eq!(
        staged_final, direct_final,
        "真实渲染窗口的双路径终态必须逐表相等"
    );
    assert!(
        staged_final.contains("applied_sesno") && staged_final.contains("dbnum_watermark"),
        "尾事务收口必须在场: {staged_final}"
    );
}

/// T0.6 黄金等价：六类语句形态的 mini 窗口，暂存+写回 ≡ 直写，且写回前零落盘。
#[tokio::test(flavor = "multi_thread")]
async fn mini_window_staged_write_back_equals_direct_write() {
    let script = MiniWindowScript {
        base: vec![
            // 既有模型产物与设计行（窗口开始前的持久层世界）。
            "UPSERT pe:e1 CONTENT { noun: 'BOX', name: 'old-name' };".into(),
            "UPSERT pe:gone CONTENT { noun: 'BOX', name: 'to-delete' };".into(),
            "UPSERT panel:p1 CONTENT { noun: 'PANE' };".into(),
            "UPSERT pe:z1 CONTENT { noun: 'ZONE' };".into(),
            "INSERT RELATION INTO inst_relate [{ id: inst_relate:[pe:e1, 0], in: pe:e1, out: pe:e1, dbnum: 7997 }];".into(),
        ],
        preload: vec![
            // ② 既有产物 / 设计行按工作项拷入暂存（与 base 同源）。
            "UPSERT pe:e1 CONTENT { noun: 'BOX', name: 'old-name' };".into(),
            "UPSERT pe:gone CONTENT { noun: 'BOX', name: 'to-delete' };".into(),
            "UPSERT panel:p1 CONTENT { noun: 'PANE' };".into(),
            "UPSERT pe:z1 CONTENT { noun: 'ZONE' };".into(),
        ],
        steps: vec![
            // insert
            (
                "INSERT INTO inst_info [{ id: inst_info:new1, geo_hash: 'h1', dbnum: 7997 }];".into(),
                ExecMode::Both,
            ),
            // update
            ("UPDATE pe:e1 SET name = 'renamed';".into(), ExecMode::Both),
            // delete
            ("DELETE pe:gone;".into(), ExecMode::Both),
            // relation
            (
                "INSERT RELATION INTO room_relate [{ id: room_relate:rr1, in: panel:p1, out: pe:e1, room_num: 'R101', inside_count: 8, center_dist: 1.0 }];".into(),
                ExecMode::Both,
            ),
            // fn:: 调用（读自己写的：上一步的 room_relate 边）
            (
                "UPSERT report:r1 SET room = fn::room_num_of(pe:e1);".into(),
                ExecMode::Both,
            ),
            // commit-time-only：全局修补（anc 回填的缩影）
            (
                "UPDATE inst_relate SET anc = [1] WHERE anc = NONE;".into(),
                ExecMode::CommitOnly,
            ),
        ],
        tail: Some(
            "UPSERT dbnum_watermark:7997 SET dbnum = 7997, applied_sesno = 42;".into(),
        ),
    };

    let (staged, direct, before_commit, base) = run_both_paths(&script).await;

    assert_eq!(
        before_commit, base,
        "I1 零落盘：写回之前持久层必须与基态一字不差"
    );
    assert_eq!(staged, direct, "I4 终态等价：暂存+写回 必须逐表等于 直写");
    assert!(
        staged.contains("renamed") && staged.contains("R101") && staged.contains("applied_sesno"),
        "对拍对象不能是空集: {staged}"
    );
}

/// T5.1 精简版（2026-08-07 修复计划 W3）：**真实窗口设施**跑三种语句形态——
/// 解析写（真实渲染 → journal）、Transform（`refresh_world_transform_products`）、
/// regen 产物写（`execute_model_write`）——写回之前对「扮演持久层」的实例做
/// 数据面快照 diff，必须为空；写回之后三种形态各自的见证表随 journal 一起收敛，
/// 控制面水位不进数据面快照、但确实在尾事务里推进。
///
/// 与暂存 Transform 回归（`staged_transform_write_routing_tests`）互补：那条钉
/// 单个函数的写路由；这条是机械防线——窗口调用树里**任何**函数漏接暂存路由，
/// 要么打在刻意不连接的 `SUL_DB` 上当场报错，要么落在持久层实例上被中途 diff
/// 抓住。live 版 T5.1（对真实 fork 服务器的执行中途快照）仍留在 P5。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "ADR-056 P1：暂存写路由已退役（spec 035 T122/T123）；直写版「中途 kill → 重放 → 逐表一致」对拍见 T171，落地后本文件随 P3 删除"]
async fn a_real_window_touches_the_persistent_layer_only_at_write_back() {
    use super::lifecycle::create_window_on;
    use super::resources::ResourceThresholds;
    use super::{StagedFinalize, register_staged_finalize};
    use crate::data_interface::increment_manager::refresh_world_transform_products;
    use crate::data_interface::increment_pipeline::IncrementPipeline;
    use aios_core::options::DbOption;
    use aios_core::{NamedAttrValue, RefU64, RefnoEnum};
    use parse_pdms_db::parse::EleData;
    use pdms_io::io::{EleOperationData, EleOperationDetail};
    use std::collections::BTreeMap;

    const DBNUM: u32 = 7985;
    // 保留段，序号避开其它夹具——GLOBAL_AABB_TREE 是进程级共享。
    let refu = |n: u64| RefU64((4000000001u64 << 32) | n);
    let root = RefnoEnum::from(refu(778001));
    let equi = RefnoEnum::from(refu(778002));
    let added = refu(778003);
    let root_pe = root.to_pe_key();
    let equi_pe = equi.to_pe_key();
    let equi_inst = equi.to_inst_relate_key();
    let root_id = root_pe.trim_start_matches("pe:").to_string();
    let equi_id = equi_pe.trim_start_matches("pe:").to_string();

    // 基线：设计行（owner 链 + 已带新 POS 的名词表行）+ 既有产物（旧 trans /
    // aabb / inst_relate / geo）。target 与 staging 同源种入（T0.6 的 base /
    // preload 纪律）。
    let fixture = format!(
        "UPSERT {root_pe} CONTENT {{ noun: 'SITE', deleted: false, refno: SITE:⟨{root_id}⟩ }};\
         UPSERT SITE:⟨{root_id}⟩ CONTENT {{ TYPE: 'SITE', NAME: '/ZZPR-ROOT' }};\
         UPSERT {equi_pe} CONTENT {{ noun: 'EQUI', deleted: false, owner: {root_pe}, refno: EQUI:⟨{equi_id}⟩ }};\
         UPSERT EQUI:⟨{equi_id}⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/ZZPR-EQUI', POS: [2000.0, 0.0, 0.0] }};\
         UPSERT trans:zzpr_old CONTENT {{ d: {{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }} }};\
         UPSERT aabb:zzpr_geo CONTENT {{ d: {{ mins: [0.0, 0.0, 0.0], maxs: [100.0, 100.0, 100.0] }} }};\
         UPSERT inst_info:zzpr_geo CONTENT {{ dbnum: {DBNUM} }};\
         UPSERT inst_geo:zzpr_geo CONTENT {{ meshed: true, visible: true, aabb: aabb:zzpr_geo }};\
         INSERT RELATION INTO geo_relate [{{ id: geo_relate:zzpr_geo, in: inst_info:zzpr_geo, out: inst_geo:zzpr_geo, trans: trans:zzpr_old, geo_type: 'Pos', visible: true }}];\
         INSERT RELATION INTO inst_relate [{{ id: {equi_inst}, in: {equi_pe}, out: inst_info:zzpr_geo, world_trans: trans:zzpr_old, aabb: aabb:zzpr_geo, solid: true, generic: 'EQUI' }}];"
    );

    let target = fresh_db("probe", "probe_persistent").await;
    init_staging_schema(&target).await.expect("target schema");
    apply_all(
        &target,
        &[
            fixture.clone(),
            format!("UPSERT dbnum_watermark:{DBNUM} SET dbnum = {DBNUM}, applied_sesno = 1;"),
        ],
    )
    .await;
    let baseline = snapshot_data_tables(&target).await;

    let instance = connect("mem://").await.expect("staging mem boots");
    let mut window = create_window_on(&instance, DBNUM, 2, 2, ResourceThresholds::default())
        .await
        .expect("create window");
    window
        .staging_db()
        .query(&fixture)
        .await
        .expect("preload transport")
        .check()
        .expect("preload applied");

    // 形态一：解析写（真实渲染管线，进 journal）。
    let mut ele = EleData::default();
    ele.refno = added;
    ele.owner = equi.refno();
    let map = &mut ele.whole_attmap.attmap.map;
    map.insert("REFNO".to_string(), NamedAttrValue::RefU64Type(added));
    map.insert(
        "OWNER".to_string(),
        NamedAttrValue::RefU64Type(equi.refno()),
    );
    map.insert(
        "TYPE".to_string(),
        NamedAttrValue::StringType("BOX".to_string()),
    );
    map.insert(
        "NAME".to_string(),
        NamedAttrValue::StringType("/ZZPR-NEW-BOX".to_string()),
    );
    map.insert(
        "DBNUM".to_string(),
        NamedAttrValue::IntegerType(DBNUM as i32),
    );
    let range = BTreeMap::from([(
        2u32,
        vec![EleOperationData::new(
            added,
            2,
            EleOperationDetail::Add(ele),
        )],
    )]);
    let staged = window
        .stage_parsed_window(&range, DBNUM)
        .await
        .expect("stage parsed window");
    assert!(staged > 0, "解析必须检测到变化并进 journal");

    // 形态二 + 三：Transform 产物刷新与 regen 形态的产物写，全在窗口读写上下文内。
    let db_option = DbOption::default();
    window
        .scope(async {
            refresh_world_transform_products(&db_option, &[equi]).await?;
            crate::surreal_retry::execute_model_write(
                &format!(
                    "INSERT IGNORE INTO inst_info {{ id: inst_info:zzpr_new, dbnum: {DBNUM} }};"
                ),
                "probe regen product",
            )
            .await?;
            register_staged_finalize(StagedFinalize {
                dbnum: DBNUM,
                start_sesno: 2,
                end_sesno: 2,
                end_sesno_time: None,
                plan: Default::default(),
                window_statements: Vec::new(),
                cache_refnos: Vec::new(),
            })
            .await
        })
        .await
        .expect("窗口计算全程不得触碰持久层（SUL_DB 未连接，直写即错）");

    // T5.1 探针本体：写回之前，持久层数据面与基线一字不差。
    let mid = snapshot_data_tables(&target).await;
    assert_eq!(
        changed_data_tables(&baseline, &mid),
        std::collections::BTreeSet::new(),
        "I1 零落盘：窗口计算中途持久层数据面必须与基线一字不差"
    );

    window
        .commit_registered_to(&target)
        .await
        .expect("staged write-back");
    window.drop_database().await.expect("cleanup");

    // 写回之后：三种形态各自的见证表随 journal 收敛。
    let after = snapshot_data_tables(&target).await;
    let changed = changed_data_tables(&baseline, &after);
    for witness in ["pe", "trans", "aabb", "inst_relate", "inst_info"] {
        assert!(
            changed.contains(witness),
            "{witness} 必须随写回收敛（解析 / Transform / regen 产物各留见证）: {changed:?}"
        );
    }
    assert!(
        !after.contains_key("dbnum_watermark"),
        "控制面表不属于数据面快照"
    );
    // 水位收口发生在写回尾事务——白名单排除的是「中途 diff 的噪音」，不是
    // 「不推进」，直接查证。
    let mut response = target
        .query(format!("RETURN dbnum_watermark:{DBNUM}.applied_sesno;"))
        .await
        .expect("watermark transport");
    let applied: Option<i32> = response.take(0).expect("watermark value");
    assert_eq!(applied, Some(2), "写回尾事务必须推进水位");
}

/// 2026-08-07 修复计划 §3.4 的另一半：**窗口废弃后持久层 `world_trans` 不变**。
///
/// P0-1 后果 2 的悬空形态只在两个时刻看得见——「写回成功前」与「窗口阻断 / 废弃
/// 后永久」。上面那条钉的是前者（中途 diff）然后就提交了；提交成功之后新 trans
/// 记录随 journal 落盘，指针照样解得开，那个时刻取不到证据。本条钉后者：窗口把
/// Transform 产物算完**不写回**直接废弃，持久层必须还是窗口前那一份自洽状态——
/// 指针仍指旧 trans 记录、`.d` 解得开、元素照旧落在 `world_trans.d != NONE` 的
/// 读者视野里。
///
/// 修复前指针 UPDATE 直打持久层，这个元素的 `world_trans` 会指向一条只存在于
/// 暂存库的新 hash；暂存库随窗口一起蒸发之后，它从 viewer / 几何查询 / 包围盒
/// 刷新 / 房间判定**全部**读者里消失，且无人能修（D9 形态）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "ADR-056 P1：暂存写路由已退役（spec 035 T122/T123）；直写版「中途 kill → 重放 → 逐表一致」对拍见 T171，落地后本文件随 P3 删除"]
async fn an_abandoned_window_leaves_the_persistent_world_trans_resolvable() {
    use super::lifecycle::create_window_on;
    use super::resources::ResourceThresholds;
    use crate::data_interface::increment_manager::refresh_world_transform_products;
    use aios_core::options::DbOption;
    use aios_core::{RefU64, RefnoEnum};

    const DBNUM: u32 = 7984;
    // 保留段，序号避开其它夹具——GLOBAL_AABB_TREE 是进程级共享。
    let refu = |n: u64| RefU64((4000000001u64 << 32) | n);
    let root = RefnoEnum::from(refu(779001));
    let equi = RefnoEnum::from(refu(779002));
    let root_pe = root.to_pe_key();
    let equi_pe = equi.to_pe_key();
    let equi_inst = equi.to_inst_relate_key();
    let root_id = root_pe.trim_start_matches("pe:").to_string();
    let equi_id = equi_pe.trim_start_matches("pe:").to_string();
    let equi_inst_id = equi_inst.trim_start_matches("inst_relate:").to_string();

    // 窗口前的持久态：名词表行已带新 POS（解析已应用），产物仍是旧的一整套
    // （trans:zzab_old ← inst_relate.world_trans，且 trans:zzab_old 自身在场）。
    let fixture = format!(
        "UPSERT {root_pe} CONTENT {{ noun: 'SITE', deleted: false, refno: SITE:⟨{root_id}⟩ }};\
         UPSERT SITE:⟨{root_id}⟩ CONTENT {{ TYPE: 'SITE', NAME: '/ZZAB-ROOT' }};\
         UPSERT {equi_pe} CONTENT {{ noun: 'EQUI', deleted: false, owner: {root_pe}, refno: EQUI:⟨{equi_id}⟩ }};\
         UPSERT EQUI:⟨{equi_id}⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/ZZAB-EQUI', POS: [3000.0, 0.0, 0.0] }};\
         UPSERT trans:zzab_old CONTENT {{ d: {{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }} }};\
         UPSERT aabb:zzab_geo CONTENT {{ d: {{ mins: [0.0, 0.0, 0.0], maxs: [100.0, 100.0, 100.0] }} }};\
         UPSERT inst_info:zzab_geo CONTENT {{ dbnum: {DBNUM} }};\
         UPSERT inst_geo:zzab_geo CONTENT {{ meshed: true, visible: true, aabb: aabb:zzab_geo }};\
         INSERT RELATION INTO geo_relate [{{ id: geo_relate:zzab_geo, in: inst_info:zzab_geo, out: inst_geo:zzab_geo, trans: trans:zzab_old, geo_type: 'Pos', visible: true }}];\
         INSERT RELATION INTO inst_relate [{{ id: {equi_inst}, in: {equi_pe}, out: inst_info:zzab_geo, world_trans: trans:zzab_old, aabb: aabb:zzab_geo, solid: true, generic: 'EQUI' }}];"
    );

    let target = fresh_db("abandon", "abandon_persistent").await;
    init_staging_schema(&target).await.expect("target schema");
    apply_all(&target, &[fixture.clone()]).await;
    let baseline = snapshot_data_tables(&target).await;

    let instance = connect("mem://").await.expect("staging mem boots");
    let window = create_window_on(&instance, DBNUM, 2, 2, ResourceThresholds::default())
        .await
        .expect("create window");
    window
        .staging_db()
        .query(&fixture)
        .await
        .expect("preload transport")
        .check()
        .expect("preload applied");

    let db_option = DbOption::default();
    window
        .scope(refresh_world_transform_products(&db_option, &[equi]))
        .await
        .expect("窗口计算全程不得触碰持久层（SUL_DB 未连接，直写即错）");

    // 暂存里指针确实改指了新 hash——没有这一条，下面的「持久层没变」就可能只是
    // 因为 Transform 压根没算出东西来。
    let mut response = window
        .staging_db()
        .query(format!("RETURN record::id({equi_inst}.world_trans);"))
        .await
        .expect("staged pointer transport")
        .check()
        .expect("valid staged pointer query");
    let staged_trans: Option<String> = response.take(0).expect("take staged trans id");
    assert_ne!(
        staged_trans.as_deref(),
        Some("zzab_old"),
        "前提：窗口内必须真的算出了新 trans 记录并改指，否则本用例什么都没验"
    );

    // 窗口废弃：不写回，直接扔掉暂存库（阻断 / 资源超限 / 进程重启的终态形态）。
    window.drop_database().await.expect("drop staging db");

    assert_eq!(
        changed_data_tables(&baseline, &snapshot_data_tables(&target).await),
        std::collections::BTreeSet::new(),
        "窗口废弃后持久层数据面必须与窗口前一字不差"
    );

    let mut response = target
        .query(format!(
            "RETURN record::id({equi_inst}.world_trans);\
             RETURN {equi_inst}.world_trans.d != NONE;\
             SELECT VALUE record::id(id) FROM inst_relate WHERE world_trans.d != NONE;"
        ))
        .await
        .expect("persistent pointer transport")
        .check()
        .expect("valid persistent pointer query");
    let trans_id: Option<String> = response.take(0).expect("take trans id");
    let resolvable: Option<bool> = response.take(1).expect("take resolvable");
    let visible: Vec<String> = response.take(2).expect("take visible rows");

    assert_eq!(
        trans_id.as_deref(),
        Some("zzab_old"),
        "窗口废弃后持久层指针必须还指着窗口前那条 trans 记录"
    );
    assert_eq!(
        resolvable,
        Some(true),
        "持久层指针必须解得开——指向暂存专属记录就是永久悬空（D9）"
    );
    assert!(
        visible.contains(&equi_inst_id),
        "元素必须仍出现在 `world_trans.d != NONE` 的读者视野里（viewer / 几何查询 / \
         包围盒刷新 / 房间判定共用这条判据）: {visible:?}"
    );
}

/// W1（2026-08-07 方案 W5.2）：带 POS 祖先的 Transform 走**解析式祖先预载 +
/// 真实窗口 + 写回**，三条断言——
///
/// 1. **I1 零落盘**：预载（StagingOnly）与窗口计算中途，持久层数据面与基线
///    一字不差；journal 里不得出现任何祖先预载行（pe / 名词表 / 链边）；
/// 2. **绝对位置**：写回后持久层 `world_trans.d.translation` 恰等于完整祖先链
///    合成的真值 [1000, 500, 7]——不是「变了」，是「对了」；
/// 3. **写回 diff 恰等于 journal 终态**：变化只落在 Transform 自己的产物表
///    （trans / aabb / inst_relate），祖先设计数据表（pe / WORL / SITE / ZONE /
///    EQUI / pe_owner）零变化——StagingOnly 的旧态没混进写回。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "ADR-056 P1：暂存写路由已退役（spec 035 T122/T123）；直写版「中途 kill → 重放 → 逐表一致」对拍见 T171，落地后本文件随 P3 删除"]
async fn staged_transform_with_a_pos_ancestor_writes_back_the_absolute_position() {
    use super::ancestor_preload::fixtures::world_chain;
    use super::ancestor_preload::{
        apply_ancestor_preload, resolve_ancestor_closure, validate_ancestor_preload,
    };
    use super::lifecycle::create_window_on;
    use super::resources::ResourceThresholds;
    use super::{StagedFinalize, register_staged_finalize};
    use crate::data_interface::increment_manager::refresh_world_transform_products;
    use aios_core::RefnoEnum;
    use aios_core::options::DbOption;

    const DBNUM: u32 = 7978;
    // 保留段 785xxx，避开其它夹具——GLOBAL_AABB_TREE 是进程级共享。
    let (chain, worl, equi_ref) = world_chain(785000);
    let equi = RefnoEnum::from(equi_ref);
    let equi_pe = equi.to_pe_key();
    let equi_inst = equi.to_inst_relate_key();

    // 窗口前的持久层世界：完整祖先链（pe + 名词表行，POS 齐备）+ 旧产物。
    // 设计数据与文件态同源（纯位姿变更的解析已应用形态），祖先在持久层本就
    // 在场——暂存侧的它们只能来自解析式预载，这正是本用例要证的通路。
    let base_fixture = format!(
        "UPSERT pe:4000000001_785001 CONTENT {{ noun: 'WORL', deleted: false, refno: WORL:⟨4000000001_785001⟩ }};\
         UPSERT WORL:⟨4000000001_785001⟩ CONTENT {{ TYPE: 'WORL', NAME: '/*' }};\
         UPSERT pe:4000000001_785002 CONTENT {{ noun: 'SITE', deleted: false, owner: pe:4000000001_785001, refno: SITE:⟨4000000001_785002⟩ }};\
         UPSERT SITE:⟨4000000001_785002⟩ CONTENT {{ TYPE: 'SITE', NAME: '/ZZAP-SITE', POS: [0.0, 0.0, 7.0] }};\
         UPSERT pe:4000000001_785003 CONTENT {{ noun: 'ZONE', deleted: false, owner: pe:4000000001_785002, refno: ZONE:⟨4000000001_785003⟩ }};\
         UPSERT ZONE:⟨4000000001_785003⟩ CONTENT {{ TYPE: 'ZONE', NAME: '/ZZAP-ZONE', POS: [0.0, 500.0, 0.0] }};\
         UPSERT {equi_pe} CONTENT {{ noun: 'EQUI', deleted: false, owner: pe:4000000001_785003, refno: EQUI:⟨4000000001_785004⟩ }};\
         UPSERT EQUI:⟨4000000001_785004⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/ZZAP-EQUI', POS: [1000.0, 0.0, 0.0] }};\
         UPSERT trans:zzpa_old CONTENT {{ d: {{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }} }};\
         UPSERT aabb:zzpa_geo CONTENT {{ d: {{ mins: [0.0, 0.0, 0.0], maxs: [100.0, 100.0, 100.0] }} }};\
         UPSERT inst_info:zzpa_geo CONTENT {{ dbnum: {DBNUM} }};\
         UPSERT inst_geo:zzpa_geo CONTENT {{ meshed: true, visible: true, aabb: aabb:zzpa_geo }};\
         INSERT RELATION INTO geo_relate [{{ id: geo_relate:zzpa_geo, in: inst_info:zzpa_geo, out: inst_geo:zzpa_geo, trans: trans:zzpa_old, geo_type: 'Pos', visible: true }}];\
         INSERT RELATION INTO inst_relate [{{ id: {equi_inst}, in: {equi_pe}, out: inst_info:zzpa_geo, world_trans: trans:zzpa_old, aabb: aabb:zzpa_geo, solid: true, generic: 'EQUI' }}];"
    );

    let target = fresh_db("ancestor_parity", "ancestor_parity_persistent").await;
    init_staging_schema(&target).await.expect("target schema");
    apply_all(
        &target,
        &[
            base_fixture.clone(),
            format!("UPSERT dbnum_watermark:{DBNUM} SET dbnum = {DBNUM}, applied_sesno = 1;"),
        ],
    )
    .await;
    let baseline = snapshot_data_tables(&target).await;

    let instance = connect("mem://").await.expect("staging mem boots");
    let window = create_window_on(&instance, DBNUM, 2, 2, ResourceThresholds::default())
        .await
        .expect("create window");
    // 暂存只种「窗口解析会写的」：目标自己的 pe + 名词表行 + 既有产物——
    // 祖先一行都不种，逼它们走解析式预载。
    window
        .staging_db()
        .query(format!(
            "UPSERT {equi_pe} CONTENT {{ noun: 'EQUI', deleted: false, owner: pe:4000000001_785003, refno: EQUI:⟨4000000001_785004⟩ }};\
             UPSERT EQUI:⟨4000000001_785004⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/ZZAP-EQUI', POS: [1000.0, 0.0, 0.0] }};\
             UPSERT trans:zzpa_old CONTENT {{ d: {{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }} }};\
             UPSERT aabb:zzpa_geo CONTENT {{ d: {{ mins: [0.0, 0.0, 0.0], maxs: [100.0, 100.0, 100.0] }} }};\
             UPSERT inst_info:zzpa_geo CONTENT {{ dbnum: {DBNUM} }};\
             UPSERT inst_geo:zzpa_geo CONTENT {{ meshed: true, visible: true, aabb: aabb:zzpa_geo }};\
             INSERT RELATION INTO geo_relate [{{ id: geo_relate:zzpa_geo, in: inst_info:zzpa_geo, out: inst_geo:zzpa_geo, trans: trans:zzpa_old, geo_type: 'Pos', visible: true }}];\
             INSERT RELATION INTO inst_relate [{{ id: {equi_inst}, in: {equi_pe}, out: inst_info:zzpa_geo, world_trans: trans:zzpa_old, aabb: aabb:zzpa_geo, solid: true, generic: 'EQUI' }}];"
        ))
        .await
        .expect("plant staged fixture")
        .check()
        .expect("staged fixture applied");

    let closure = {
        let mut lookup = {
            let chain = chain.clone();
            move |refno| std::future::ready(Ok(chain.get(&refno).cloned()))
        };
        resolve_ancestor_closure(&[equi_ref], worl, 2, &mut lookup)
            .await
            .expect("resolve ancestor closure")
    };

    let db_option = DbOption::default();
    window
        .scope(async {
            apply_ancestor_preload(&closure, DBNUM).await?;
            validate_ancestor_preload(&closure).await?;
            refresh_world_transform_products(&db_option, &[equi]).await?;
            register_staged_finalize(StagedFinalize {
                dbnum: DBNUM,
                start_sesno: 2,
                end_sesno: 2,
                end_sesno_time: None,
                plan: Default::default(),
                window_statements: Vec::new(),
                cache_refnos: Vec::new(),
            })
            .await
        })
        .await
        .expect("窗口计算全程不得触碰持久层（SUL_DB 未连接，直写即错）");

    // 断言 1a：journal 里没有任何祖先预载行（StagingOnly 纪律）。
    for entry in window.journal().await {
        assert!(
            !entry.sql.contains("INSERT IGNORE INTO pe [")
                && !entry.sql.contains("INSERT IGNORE INTO WORL")
                && !entry.sql.contains("INSERT IGNORE INTO SITE")
                && !entry.sql.contains("INSERT IGNORE INTO ZONE")
                && !entry.sql.contains("INSERT IGNORE INTO EQUI")
                && !entry.sql.contains("pe_owner:["),
            "祖先预载行绝不进 journal: {}",
            entry.sql
        );
    }
    // 断言 1b：写回之前持久层数据面零变化。
    assert_eq!(
        changed_data_tables(&baseline, &snapshot_data_tables(&target).await),
        std::collections::BTreeSet::new(),
        "I1 零落盘：窗口计算中途持久层数据面必须与基线一字不差"
    );

    window
        .commit_registered_to(&target)
        .await
        .expect("staged write-back");
    window.drop_database().await.expect("cleanup");

    // 断言 2：写回后的绝对位置 = 完整祖先链合成的真值。
    let mut response = target
        .query(format!("RETURN {equi_inst}.world_trans.d.translation;"))
        .await
        .expect("persistent world trans transport")
        .check()
        .expect("valid persistent world trans query");
    let translation: Vec<f64> = response.take(0).expect("take translation");
    assert_eq!(
        translation,
        vec![1000.0, 500.0, 7.0],
        "写回后的世界变换必须合成 ZONE/SITE 的位移（修复前会静默丢成 [1000,0,0]）"
    );

    // 断言 3：写回 diff 恰等于 journal 终态——只有 Transform 自己的产物表
    // （外加尾事务登记的空间意图 spatial_epoch）变了，祖先设计数据表零变化。
    let changed = changed_data_tables(&baseline, &snapshot_data_tables(&target).await);
    assert_eq!(
        changed,
        std::collections::BTreeSet::from([
            "aabb".to_string(),
            "inst_relate".to_string(),
            "spatial_epoch".to_string(),
            "trans".to_string(),
        ]),
        "写回只许改 Transform 的产物表与尾事务的空间意图；祖先旧态（pe / 名词表 / 链边）\
         不得混进写回"
    );
    let mut response = target
        .query(format!("RETURN dbnum_watermark:{DBNUM}.applied_sesno;"))
        .await
        .expect("watermark transport");
    let applied: Option<i32> = response.take(0).expect("watermark value");
    assert_eq!(applied, Some(2), "写回尾事务必须推进水位");
}
