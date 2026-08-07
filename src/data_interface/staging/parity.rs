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

async fn fresh_db(ns: &str, db: &str) -> Surreal<Any> {
    let handle = connect("mem://").await.expect("mem boots");
    handle.use_ns(ns).use_db(db).await.expect("use db");
    handle
}

async fn apply_all(db: &Surreal<Any>, statements: &[String]) {
    for sql in statements {
        db.query(sql)
            .await
            .expect("apply transport")
            .check()
            .unwrap_or_else(|e| panic!("apply failed: {sql}\n{e}"));
    }
}

/// 数据面逐表快照（T5.1 精简版探针的口径）：排除控制面白名单后，表名 → 该表
/// 全部行的序列化文本。窗口计算**中途**对持久层做本快照的 diff，必须为空——
/// 控制面（恢复记录、水位观察、队列控制、durable pending、side-effect pending）
/// 是 I1 明文豁免的落库，不属于「窗口数据」。
pub(crate) const CONTROL_PLANE_TABLES: [&str; 5] = [
    "dbnum_watermark",
    "increment_update_attempt",
    "queue_control",
    "model_update_pending",
    "incr_side_effect_pending",
];

pub(crate) async fn snapshot_data_tables(
    db: &Surreal<Any>,
) -> std::collections::BTreeMap<String, String> {
    let mut response = db.query("INFO FOR DB").await.expect("info");
    let info: surrealdb::Value = response.take(0).expect("take info");
    let info_json = serde_json::to_value(&info).expect("serialize info");
    let mut tables: Vec<String> = info_json
        .pointer("/Object/tables/Object")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    tables.sort();

    let mut out = std::collections::BTreeMap::new();
    for table in tables {
        if CONTROL_PLANE_TABLES.contains(&table.as_str()) {
            continue;
        }
        let mut response = db
            .query(format!("SELECT * FROM `{table}` ORDER BY id"))
            .await
            .expect("select table");
        let rows: surrealdb::Value = response.take(0).expect("take rows");
        out.insert(table, serde_json::to_string(&rows).expect("serialize rows"));
    }
    out
}

/// 两份数据面快照里内容不同（或只在一边存在）的表名集合。
pub(crate) fn changed_data_tables(
    before: &std::collections::BTreeMap<String, String>,
    after: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeSet<String> {
    before
        .iter()
        .filter(|(table, rows)| after.get(*table) != Some(*rows))
        .map(|(table, _)| table.clone())
        .chain(
            after
                .keys()
                .filter(|table| !before.contains_key(*table))
                .cloned(),
        )
        .collect()
}

/// 逐表快照：INFO FOR DB 枚举表名，逐表 `SELECT * ORDER BY id` 后序列化拼接。
/// 两个引擎、两条路径产出的文本相等 ⇔ 终态相等（serde 结构化序列化，F3 口径）。
pub(crate) async fn snapshot_tables(db: &Surreal<Any>) -> String {
    let mut response = db.query("INFO FOR DB").await.expect("info");
    let info: surrealdb::Value = response.take(0).expect("take info");
    let info_json = serde_json::to_value(&info).expect("serialize info");
    let mut tables: Vec<String> = info_json
        .pointer("/Object/tables/Object")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    tables.sort();

    let mut out = String::new();
    for table in tables {
        let mut response = db
            .query(format!("SELECT * FROM `{table}` ORDER BY id"))
            .await
            .expect("select table");
        let rows: surrealdb::Value = response.take(0).expect("take rows");
        let rendered = serde_json::to_string(&rows).expect("serialize rows");
        // 空表是「表定义残留」，两条路径都可能有（DEFINE 集），跳过内容为空的表
        // 会掩盖「一边有行一边没有」吗？不会——那种情况 rendered 不同。
        out.push_str(&format!("== {table} ==\n{rendered}\n"));
    }
    out
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
        .commit_to(&staged_target, script.tail.as_deref())
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
    let tail = render_finalize_tail(7997, 43, &ModelUpdatePlan::default(), &[])
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
        .commit_to(&staged_target, Some(&tail))
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
            // commit-time-only：全局修补（zone_refno 回填的缩影）
            (
                "UPDATE inst_relate SET zone_refno = pe:z1 WHERE zone_refno = NONE;".into(),
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
        vec![EleOperationData::new(added, 2, EleOperationDetail::Add(ele))],
    )]);
    let staged = IncrementPipeline::stage_parsed_window(&mut window, &range, DBNUM)
        .await
        .expect("stage parsed window");
    assert!(staged > 0, "解析必须检测到变化并进 journal");

    // 形态二 + 三：Transform 产物刷新与 regen 形态的产物写，全在窗口读写上下文内。
    let db_option = DbOption {
        gen_spatial_tree: true,
        ..Default::default()
    };
    window
        .scope(async {
            refresh_world_transform_products(&db_option, &[equi]).await?;
            crate::surreal_retry::execute_model_write(
                &format!("INSERT IGNORE INTO inst_info {{ id: inst_info:zzpr_new, dbnum: {DBNUM} }};"),
                "probe regen product",
            )
            .await?;
            register_staged_finalize(StagedFinalize {
                dbnum: DBNUM,
                end_sesno: 2,
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
