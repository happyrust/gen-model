//! 持久层逐表对拍与「一个 SurrealDB 实例 + 生产 schema」的中性载体。
//!
//! 从 `staging/parity.rs`（kv-mem 暂存黄金等价 harness）抽出来的那一半**与暂存无关**：
//! 任何「两个引擎 / 两条路径 / 两次执行的终态是否逐表相等」的对拍都用它——
//! 直写版崩溃重放对拍（`direct_window_replay_parity`，spec 035 T171）、fork↔mem 一致性
//! 套件、以及 P3 之后把 `mem://` 当载体的单测。暂存目录随 ADR-056 P3 整体删除时，
//! 这里一行不动（T171 的「P3 删 parity.rs 时不丢」）。
//!
//! 两条口径纪律：
//! - **控制面白名单**（[`CONTROL_PLANE_TABLES`]）只用于「中途 diff」——恢复记录、水位、
//!   队列控制、durable pending、副作用队列是 I1 明文豁免的落库，不属于「窗口数据」；
//!   终态对拍要不要排除它们由调用方决定（水位就该在终态里被**查证**，不是被掩掉）。
//! - 快照是 `INFO FOR DB` 枚举表名 + 逐表 `SELECT * ORDER BY id` 的 serde 序列化文本：
//!   两份文本相等 ⇔ 终态逐表相等（F3 口径），差异表名由 [`changed_data_tables`] 给出。

use std::collections::{BTreeMap, BTreeSet};

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};

/// 中途 diff 时排除的控制面表（I1 豁免面）。
pub const CONTROL_PLANE_TABLES: [&str; 5] = [
    "dbnum_watermark",
    "increment_update_attempt",
    "queue_control",
    "model_update_pending",
    "incr_side_effect_pending",
];

/// 起一个全新的 `mem://` 实例并切到 `ns`/`db`。**不装 schema**——要生产 schema
/// 再调 [`init_schema_on`]，两步分开是为了让「无 schema 的裸实例」也能当对照组。
pub async fn fresh_mem_db(ns: &str, db: &str) -> anyhow::Result<Surreal<Any>> {
    let handle = connect("mem://")
        .await
        .map_err(|error| anyhow::anyhow!("mem:// 实例启动失败: {error}"))?;
    handle
        .use_ns(ns)
        .use_db(db)
        .await
        .map_err(|error| anyhow::anyhow!("切换 {ns}/{db} 失败: {error}"))?;
    Ok(handle)
}

/// 在给定实例上装**生产启动序列同一套** DEFINE（`run_cli` 的 schema 段）。
/// 单一事实来源：fork↔mem 一致性套件排练的就是这个函数，暂存库建库
/// （`staging::lifecycle::init_staging_schema`）也只是委托到这里。
///
/// 继承的既有行为（见 `docs/2026-08-05_fork-surreal-compat-findings.md`）：
/// `define_common_functions` 静默吞逐语句错误（全新库上 REMOVE 不存在的函数）；
/// F1（`idx_inst_relate_zone_refno` 的 `TYPE BTREE` 语法非法 + 吞错）已修，且该索引已随
/// P3 退役——生产与这里共用 `INST_RELATE_INDEX_SQL`（含摘除它的迁移语句），错误显式上抛。
///
/// 刻意不装 `update_dbnum_event`（F4）：该事件体假定 `pe` 的 record id 是数组（历史行
/// 形制），对字符串 id 的最新行（`pe:24381_100677`，fork 解析为字符串）任何 UPSERT/UPDATE
/// 都会因 `array::at` 类型错误而**整条语句失败**。它服务的 `dbnum_info_table` 是遗留
/// 水位迁移的记账面（`dbnum_state` 本就容忍其缺失/陈旧），不属于窗口数据语义。
pub async fn init_schema_on(db: &Surreal<Any>) -> anyhow::Result<()> {
    // 磁盘脚本（CWD 的 resource/surreal，站点扩展）＋内置快照收尾——与 run_cli
    // 同一顺序，同名函数以内置版为准。内置序列自带 D11 的 hd/hh 矫正；这里原先
    // 按 CARGO_MANIFEST_DIR 读 hd 文件，那是编译机路径，部署机上一开窗口就会失败。
    if std::path::Path::new("resource/surreal").is_dir() {
        aios_core::function::define_common_functions_on(db).await?;
    } else {
        aios_core::function::ensure_inst_meta_functions_on(db).await?;
    }
    crate::data_interface::embedded_surql::define_embedded_functions_on(db).await?;
    aios_core::create_geom_index_on(db).await?;
    aios_core::define_room_index_on(db).await?;
    aios_core::define_owner_index_on(db).await?;
    aios_core::define_fullname_index_on(db).await?;
    aios_core::define_pe_index_on(db).await?;
    aios_core::define_ses_index_on(db).await?;
    // gen-model 侧唯一的启动期 DEFINE（init_inst_relate_indices）——与生产同一组
    // 语句（F1 已修 + anc/dbnum 索引，见常量文档）。
    db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
        .await?
        .check()?;
    Ok(())
}

/// 按顺序逐条执行并 `check()`；失败时把那条 SQL 原样带在错误里。
pub async fn apply_all(db: &Surreal<Any>, statements: &[String]) -> anyhow::Result<()> {
    for sql in statements {
        db.query(sql)
            .await
            .map_err(|error| anyhow::anyhow!("执行失败（传输）: {sql}\n{error}"))?
            .check()
            .map_err(|error| anyhow::anyhow!("执行失败（语句）: {sql}\n{error}"))?;
    }
    Ok(())
}

/// `INFO FOR DB` 里的全部表名，升序。
pub async fn table_names(db: &Surreal<Any>) -> anyhow::Result<Vec<String>> {
    let mut response = db
        .query("INFO FOR DB")
        .await
        .map_err(|error| anyhow::anyhow!("INFO FOR DB 失败: {error}"))?;
    let info: surrealdb::Value = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("读取 INFO FOR DB 结果失败: {error}"))?;
    let info_json = serde_json::to_value(&info)?;
    let mut tables: Vec<String> = info_json
        .pointer("/Object/tables/Object")
        .and_then(|value| value.as_object())
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    tables.sort();
    Ok(tables)
}

async fn rows_text(db: &Surreal<Any>, table: &str) -> anyhow::Result<String> {
    let mut response = db
        .query(format!("SELECT * FROM `{table}` ORDER BY id"))
        .await
        .map_err(|error| anyhow::anyhow!("读取表 {table} 失败: {error}"))?;
    let rows: surrealdb::Value = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("取表 {table} 行失败: {error}"))?;
    Ok(serde_json::to_string(&rows)?)
}

/// 数据面逐表快照：排除 [`CONTROL_PLANE_TABLES`] 后，表名 → 该表全部行的序列化文本。
pub async fn snapshot_data_tables(db: &Surreal<Any>) -> anyhow::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for table in table_names(db).await? {
        if CONTROL_PLANE_TABLES.contains(&table.as_str()) {
            continue;
        }
        let rows = rows_text(db, &table).await?;
        out.insert(table, rows);
    }
    Ok(out)
}

/// 全部表（含控制面）的逐表快照，拼成一段可直接 `assert_eq!` 的文本。
///
/// 空表是「表定义残留」（DEFINE 集），两条路径都可能有；不跳过它——「一边有行
/// 一边没有」正是要抓的差异，而且两边都空时文本相同，不会误报。
pub async fn snapshot_tables(db: &Surreal<Any>) -> anyhow::Result<String> {
    let mut out = String::new();
    for table in table_names(db).await? {
        let rows = rows_text(db, &table).await?;
        out.push_str(&format!("== {table} ==\n{rows}\n"));
    }
    Ok(out)
}

/// 两份数据面快照里内容不同（或只在一边存在）的表名集合。
pub fn changed_data_tables(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> BTreeSet<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_tables_report_both_sides() {
        let before = BTreeMap::from([
            ("pe".to_string(), "[a]".to_string()),
            ("only_before".to_string(), "[x]".to_string()),
            ("same".to_string(), "[s]".to_string()),
        ]);
        let after = BTreeMap::from([
            ("pe".to_string(), "[b]".to_string()),
            ("only_after".to_string(), "[y]".to_string()),
            ("same".to_string(), "[s]".to_string()),
        ]);
        assert_eq!(
            changed_data_tables(&before, &after),
            BTreeSet::from([
                "only_after".to_string(),
                "only_before".to_string(),
                "pe".to_string(),
            ])
        );
        assert!(changed_data_tables(&before, &before).is_empty());
    }

    /// 白名单只挡「中途 diff 的噪音」；终态快照里水位必须仍然可见、可查证。
    #[tokio::test(flavor = "multi_thread")]
    async fn control_plane_is_excluded_from_data_snapshots_but_present_in_full_ones() {
        let db = fresh_mem_db("table_parity", "control_plane")
            .await
            .expect("mem boots");
        apply_all(
            &db,
            &[
                "UPSERT pe:p1 CONTENT { noun: 'BOX', deleted: false };".into(),
                "UPSERT dbnum_watermark:7997 SET dbnum = 7997, applied_sesno = 41;".into(),
            ],
        )
        .await
        .expect("fixture");

        let data = snapshot_data_tables(&db).await.expect("data snapshot");
        assert!(data.contains_key("pe"));
        assert!(
            !data.contains_key("dbnum_watermark"),
            "控制面表不进数据面快照: {data:?}"
        );

        let full = snapshot_tables(&db).await.expect("full snapshot");
        assert!(full.contains("== dbnum_watermark ==") && full.contains("applied_sesno"));
        assert!(full.contains("== pe =="));
    }
}
