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

use surrealdb::engine::any::{connect, Any};
use surrealdb::Surreal;

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
    init_staging_schema(&staged_target).await.expect("target schema");
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
    init_staging_schema(&direct_target).await.expect("target schema");
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
