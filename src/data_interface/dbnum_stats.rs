//! `dbnum_info_table` 的**显式重建**（`bin/rebuild_dbnum_stats` 的修复口）。
//!
//! 这张表的常态写者只有一个：dbnum 任务在数据批次应用之后从目标索引序列化的那一笔
//! （`ref0_snapshot` → `task_end`，ADR-071 D6，`count` = 目标会话上的活元素数）。这里的重建是
//! 人工修复口，从 `pe` 按 Ref0 重算，口径与快照同为活元素（`!deleted`）——ADR-039 #3
//! 「二选一不并存」：两条路必须同一口径。逐笔 ±1 观测块与它的版本标记检查 2026-09-10 退役；
//! PE 写事务的包装（`BEGIN / COMMIT`、上限、切块、执行）搬去了 [`super::pe_batch`]。
//! 基线收尾不再重建（基线也走任务收尾那一笔快照），`pe_count == info_count` 那三处对账同日退役。

use surrealdb::{Surreal, engine::any::Any};

pub const VERSION: i32 = 1;

/// 从 `pe` 显式重算这个库的 Ref0 统计。
/// 口径与任务收尾的快照一致：`count` 只数**活**元素（`!deleted`），墓碑不算（ADR-071 D6；
/// ADR-039 #3「二选一不并存」——两条路必须同一口径）。历史 Ref0 零计数留行，供清理发现。
pub fn rebuild_body(dbnum: u32) -> String {
    format!(
        r#"
        LET $stats_groups = (SELECT string::split(record::id(id), '_')[0] AS ref0,
            count() AS count, math::max(sesno ?? 0) AS sesno,
            math::max(<int>string::split(record::id(id), '_')[1]) AS max_ref1
            FROM pe WHERE dbnum = {dbnum} AND !deleted GROUP BY ref0);
        UPDATE dbnum_info_table SET count = 0 WHERE dbnum = {dbnum};
        FOR $g IN $stats_groups {{
            LET $key = type::thing('dbnum_info_table', <int>$g.ref0);
            LET $old = (SELECT * FROM ONLY $key);
            IF $old.dbnum != NONE AND $old.dbnum != {dbnum} {{ THROW 'PE statistics Ref0 ownership conflict'; }};
            UPSERT $key MERGE {{ dbnum: {dbnum}, count: $g.count,
                sesno: math::max([$old.sesno ?? 0, $g.sesno ?? 0]),
                max_ref1: math::max([$old.max_ref1 ?? 0, $g.max_ref1 ?? 0]), updated_at: time::now() }};
        }};
        UPSERT dbnum_stats_state:{dbnum} SET version = {VERSION};
    "#
    )
}

/// 无条件重建（一笔事务）。
pub async fn rebuild_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<()> {
    let sql = format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        rebuild_body(dbnum)
    );
    super::pe_batch::execute_on(db, &sql).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 显式重建与任务收尾的快照同口径：只数活元素，墓碑不算；历史 Ref0 零计数留行；幂等。
    #[tokio::test]
    async fn rebuild_counts_live_rows_only_and_retains_zero_groups() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("stats").use_db("rebuild").await.unwrap();
        db.query(
            "CREATE pe:20_1 SET dbnum=7, sesno=3; CREATE pe:20_2 SET dbnum=7, sesno=4, deleted=true; \
             CREATE pe:20_9 SET dbnum=7, sesno=5, deleted=false; CREATE pe:30_1 SET dbnum=8, sesno=1; \
             CREATE dbnum_info_table:21 SET dbnum=7, count=12;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        rebuild_on(&db, 7).await.unwrap();
        let mut result = db
            .query(
                "RETURN dbnum_info_table:20.count; RETURN dbnum_info_table:20.max_ref1; \
                 RETURN dbnum_info_table:20.sesno; RETURN dbnum_info_table:21.count; \
                 RETURN dbnum_info_table:30.count; RETURN dbnum_stats_state:7.version;",
            )
            .await
            .unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(2), "墓碑不算");
        assert_eq!(result.take::<Option<i64>>(1).unwrap(), Some(9));
        assert_eq!(result.take::<Option<i64>>(2).unwrap(), Some(5));
        assert_eq!(
            result.take::<Option<i64>>(3).unwrap(),
            Some(0),
            "历史 Ref0 留行清零"
        );
        assert_eq!(result.take::<Option<i64>>(4).unwrap(), None, "别库不碰");
        assert_eq!(result.take::<Option<i64>>(5).unwrap(), Some(VERSION as i64));
        rebuild_on(&db, 7).await.unwrap();
        let mut result = db.query("RETURN dbnum_info_table:20.count;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(2));
    }

    /// 这个文件只剩重建：包装在 `pe_batch`，观测块与版本检查不得回来。
    #[test]
    fn only_the_explicit_rebuild_lives_here() {
        let source = include_str!("dbnum_stats.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        for retired in [
            "pub fn render(",
            "pub fn transaction(",
            "pub fn batches(",
            "pub async fn execute_on(",
            "pub async fn initialize_on(",
            "LET $before_",
            ".version != ",
        ] {
            assert!(!production.contains(retired), "dbnum_stats 不该再有: {retired}");
        }
        assert!(production.contains("AND !deleted GROUP BY ref0"), "重建只数活元素");
    }
}
