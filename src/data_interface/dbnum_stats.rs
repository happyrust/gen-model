//! Explicit PE statistics. Callers supply record IDs, never parsed SQL.
//! A batch observes actual rows before/after its writes in one transaction.
//! Migration is once per database number and commits with the first batch.

use std::collections::{BTreeMap, BTreeSet};
use surrealdb::{Surreal, engine::any::Any};

pub const VERSION: i32 = 1;

/// 切块上限统一从 `tx_limits` 取（plan 2026-09-06 §3 D3）：此前借的是 `staging::executor`
/// 的暂存期常量（64 KiB / 250 行 / 100 元素），那是 `mem://` 的口径。
fn batch_elements() -> usize {
    super::tx_limits::data_tx_limits().elements.max(1)
}
fn max_batch_bytes() -> usize {
    super::tx_limits::data_tx_limits().max_bytes
}
fn max_batch_write_rows() -> u64 {
    super::tx_limits::data_tx_limits().max_write_rows
}

fn write_rows(sql: &str) -> anyhow::Result<u64> {
    use surrealdb::sql::statements::IfelseStatement;
    use surrealdb::sql::{Data, Entry, Statement, Value};

    fn branches(branch: &IfelseStatement) -> u64 {
        branch
            .exprs
            .iter()
            .map(|(_, body)| body)
            .chain(branch.close.iter())
            .map(|body| {
                let Value::Block(block) = body else {
                    return 0;
                };
                block
                    .iter()
                    .map(|entry| match entry {
                        Entry::Create(write) => write.what.len() as u64,
                        Entry::Upsert(write) => write.what.len() as u64,
                        Entry::Update(write) => write.what.len().max(1) as u64,
                        Entry::Delete(write) => write.what.len().max(1) as u64,
                        Entry::Insert(write) => match &write.data {
                            Data::ValuesExpression(rows) => rows.len() as u64,
                            Data::SingleExpression(Value::Array(rows)) => rows.len() as u64,
                            _ => 1,
                        },
                        Entry::Ifelse(nested) => branches(nested),
                        _ => 0,
                    })
                    .sum::<u64>()
            })
            .sum()
    }
    let mut count = super::staging::replay_safe::estimate_write_rows(sql)?;
    // Inspect blocks directly: this fork's AST Display is not round-trip safe
    // for quoted strings (e.g. the WORL name '/*').
    for statement in surrealdb::sql::parse(sql)?.iter() {
        if let Statement::Ifelse(branch) = statement {
            count += branches(branch);
        }
    }
    Ok(count)
}

fn batch_rows(ids: &[String], writes: &str) -> anyhow::Result<u64> {
    let groups = ids
        .iter()
        .filter_map(|id| id.split_once('_').map(|(ref0, _)| ref0))
        .collect::<BTreeSet<_>>();
    // The one-time migration is a dbnum-wide maintenance transaction. Normal
    // batches charge every PE/attribute/edge write plus one write per Ref0.
    Ok(write_rows(writes)? + groups.len() as u64 + 1)
}

pub async fn retire_event_on(db: &Surreal<Any>) -> anyhow::Result<()> {
    crate::surreal_retry::execute_surreal_checked_on(
        db,
        "REMOVE EVENT IF EXISTS update_dbnum_event ON pe;",
        "retire PE statistics event",
    )
    .await
}

pub async fn require_no_event_on(db: &Surreal<Any>) -> anyhow::Result<()> {
    let mut response = db.query("INFO FOR TABLE pe;").await?.check()?;
    let info: Option<serde_json::Value> = response.take(0)?;
    anyhow::ensure!(
        info.as_ref()
            .and_then(|v| v.get("events"))
            .and_then(|v| v.get("update_dbnum_event"))
            .is_none(),
        "update_dbnum_event was reinstalled; stop the legacy writer and retire the event before writing PE"
    );
    Ok(())
}

/// This migration runs inside the same transaction as its version marker.
/// Zero-count historical groups are retained for Ref0 cleanup discovery.
pub fn rebuild_body(dbnum: u32) -> String {
    format!(
        r#"
        LET $stats_groups = (SELECT string::split(record::id(id), '_')[0] AS ref0,
            count() AS count, math::max(sesno ?? 0) AS sesno,
            math::max(<int>string::split(record::id(id), '_')[1]) AS max_ref1
            FROM pe WHERE dbnum = {dbnum} GROUP BY ref0);
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

/// Bare statements: the caller owns the transaction boundary and must include
/// the entire returned body. `ids` are canonical `ref0_ref1` strings.
pub fn render(dbnum: u32, ids: &[String], writes: &str) -> anyhow::Result<String> {
    let mut groups: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for id in ids {
        let (a, b) = id
            .split_once('_')
            .ok_or_else(|| anyhow::anyhow!("invalid PE id {id}"))?;
        let ref0: u32 = a.parse()?;
        let ref1: u32 = b.parse()?;
        anyhow::ensure!(*id == format!("{ref0}_{ref1}"), "noncanonical PE id {id}");
        groups.entry(ref0).or_default().insert(id.clone());
    }
    if groups.is_empty() {
        return Ok(writes.to_string());
    }
    let mut sql = format!(
        "IF dbnum_stats_state:{dbnum}.version != {VERSION} {{ {} }};\n",
        rebuild_body(dbnum)
    );
    for (ref0, ids) in &groups {
        let targets = ids
            .iter()
            .map(|id| format!("pe:{id}"))
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(
            "LET $before_{ref0} = (SELECT id, dbnum, sesno FROM [{targets}]);\n"
        ));
    }
    sql.push_str(writes);
    sql.push_str(";\n");
    for (ref0, ids) in &groups {
        let targets = ids
            .iter()
            .map(|id| format!("pe:{id}"))
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(r#"
            LET $after_{ref0} = (SELECT id, dbnum, sesno FROM [{targets}]);
            LET $observed = array::concat($before_{ref0}, $after_{ref0});
            FOR $row IN $observed {{
                IF $row.dbnum != {dbnum} {{ THROW 'PE statistics row ownership conflict'; }};
            }};
            LET $old = (SELECT * FROM ONLY dbnum_info_table:{ref0});
            IF $old.dbnum != NONE AND $old.dbnum != {dbnum} {{ THROW 'PE statistics Ref0 ownership conflict'; }};
            IF ($old.count ?? 0) < array::len($before_{ref0}) {{ THROW 'PE statistics missing or inconsistent; rebuild required'; }};
            LET $next = ($old.count ?? 0) + array::len($after_{ref0}) - array::len($before_{ref0});
            IF $next < 0 {{ THROW 'PE statistics negative count'; }};
            IF array::len($observed) > 0 {{
                LET $sessions = (SELECT VALUE sesno ?? 0 FROM $observed);
                LET $refs = (SELECT VALUE <int>string::split(record::id(id), '_')[1] FROM $observed);
                LET $identity = (SELECT file_name, db_type FROM ONLY dbnum_watermark:{dbnum});
                UPSERT dbnum_info_table:{ref0} MERGE {{ dbnum: {dbnum}, count: $next,
                    sesno: math::max(array::concat([$old.sesno ?? 0], $sessions)),
                    max_ref1: math::max(array::concat([$old.max_ref1 ?? 0], $refs)),
                    file_name: $identity.file_name ?? $old.file_name ?? '',
                    db_type: $identity.db_type ?? $old.db_type ?? '', updated_at: time::now() }};
            }};
        "#));
    }
    Ok(sql)
}

pub fn transaction(dbnum: u32, ids: &[String], writes: &str) -> anyhow::Result<String> {
    let sql = format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        render(dbnum, ids, writes)?
    );
    anyhow::ensure!(
        sql.len() <= max_batch_bytes(),
        "PE statistics batch exceeds byte limit"
    );
    anyhow::ensure!(
        batch_rows(ids, writes)? <= max_batch_write_rows(),
        "PE statistics batch exceeds write-row limit"
    );
    Ok(sql)
}

/// Keep each element's replacement intact. Auxiliary statements carry no ID.
pub fn batches(dbnum: u32, entries: &[(Option<String>, String)]) -> anyhow::Result<Vec<String>> {
    fn append(
        dbnum: u32,
        entries: &[(Option<String>, String)],
        out: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let ids = entries
            .iter()
            .filter_map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let writes = entries
            .iter()
            .map(|(_, sql)| sql.as_str())
            .collect::<Vec<_>>()
            .join(";\n");
        let sql = format!(
            "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
            render(dbnum, &ids, &writes)?
        );
        let fits =
            sql.len() <= max_batch_bytes() && batch_rows(&ids, &writes)? <= max_batch_write_rows();
        if !fits && entries.len() > 1 {
            let middle = entries.len() / 2;
            append(dbnum, &entries[..middle], out)?;
            append(dbnum, &entries[middle..], out)?;
        } else {
            anyhow::ensure!(
                fits,
                "single PE replacement exceeds transaction limits (including statistics)"
            );
            out.push(sql);
        }
        Ok(())
    }
    let mut out = Vec::new();
    for chunk in entries.chunks(batch_elements()) {
        append(dbnum, chunk, &mut out)?;
    }
    Ok(out)
}

pub async fn execute_on(db: &Surreal<Any>, sql: &str) -> anyhow::Result<()> {
    require_no_event_on(db).await?;
    crate::surreal_retry::execute_surreal_checked_on(db, sql, "PE batch and statistics").await
}

/// Unconditional rebuild from `pe` in one transaction — the explicit-repair / test
/// counterpart of [`initialize_on`], which only rebuilds when the version marker is behind.
pub async fn rebuild_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<()> {
    let sql = format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        rebuild_body(dbnum)
    );
    execute_on(db, &sql).await
}

/// Also establishes the version for a successfully initialized empty database.
pub async fn initialize_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<()> {
    let sql = format!(
        "BEGIN TRANSACTION; IF dbnum_stats_state:{dbnum}.version != {VERSION} {{ {} }}; COMMIT TRANSACTION;",
        rebuild_body(dbnum)
    );
    execute_on(db, &sql).await
}

pub fn insert_ignore_batches(dbnum: u32, rows: &[(String, String)]) -> anyhow::Result<Vec<String>> {
    fn append(dbnum: u32, rows: &[(String, String)], out: &mut Vec<String>) -> anyhow::Result<()> {
        let ids = rows.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        let writes = format!(
            "INSERT IGNORE INTO pe [{}];",
            rows.iter()
                .map(|(_, row)| row.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        match transaction(dbnum, &ids, &writes) {
            Ok(sql) => out.push(sql),
            Err(_) if rows.len() > 1 => {
                let middle = rows.len() / 2;
                append(dbnum, &rows[..middle], out)?;
                append(dbnum, &rows[middle..], out)?;
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }
    let mut out = Vec::new();
    for chunk in rows.chunks(batch_elements()) {
        append(dbnum, chunk, &mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database(label: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("stats").use_db(label).await.unwrap();
        db
    }

    #[tokio::test]
    async fn migration_is_atomic_once_and_retains_zero_groups() {
        let db = database("migration").await;
        db.query("CREATE pe:20_1 SET dbnum=7, sesno=3, deleted=true; CREATE dbnum_info_table:20 SET dbnum=7, count=90, max_ref1=9, file_name='old'; CREATE dbnum_info_table:21 SET dbnum=7, count=12; CREATE dbnum_watermark:7 SET applied_sesno=3;").await.unwrap().check().unwrap();
        let ids = vec!["20_1".into()];
        let fail = transaction(7, &ids, "UPDATE pe:20_1 SET sesno=4; THROW 'crash'").unwrap();
        assert!(execute_on(&db, &fail).await.is_err());
        let mut result = db.query("RETURN dbnum_stats_state:7.version; RETURN dbnum_info_table:20.count; RETURN dbnum_watermark:7.applied_sesno;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), None);
        assert_eq!(result.take::<Option<i64>>(1).unwrap(), Some(90));
        assert_eq!(result.take::<Option<i64>>(2).unwrap(), Some(3));
        execute_on(
            &db,
            &transaction(7, &ids, "UPDATE pe:20_1 SET sesno=4").unwrap(),
        )
        .await
        .unwrap();
        let mut result = db.query("RETURN dbnum_info_table:20.count; RETURN dbnum_info_table:21.count; RETURN dbnum_info_table:20.max_ref1;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(1));
        assert_eq!(result.take::<Option<i64>>(1).unwrap(), Some(0));
        assert_eq!(result.take::<Option<i64>>(2).unwrap(), Some(9));
        // A valid version must not issue another full scan: untouched malformed
        // legacy IDs would make the migration aggregation fail if run again.
        db.query("CREATE pe:unrelated SET dbnum=7;")
            .await
            .unwrap()
            .check()
            .unwrap();
        execute_on(
            &db,
            &transaction(7, &ids, "UPDATE pe:20_1 SET sesno=5").unwrap(),
        )
        .await
        .unwrap();
        let mut result = db.query("RETURN dbnum_info_table:20.count;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(1));
    }

    #[tokio::test]
    async fn event_reinstallation_and_ownership_conflicts_fail_closed() {
        let db = database("guards").await;
        db.query("DEFINE EVENT update_dbnum_event ON pe WHEN true THEN { RETURN NONE; };")
            .await
            .unwrap()
            .check()
            .unwrap();
        let ids = vec!["30_1".into()];
        let sql = transaction(7, &ids, "UPSERT pe:30_1 SET dbnum=7, sesno=1").unwrap();
        assert!(execute_on(&db, &sql).await.is_err());
        for _ in 0..2 {
            retire_event_on(&db).await.unwrap();
        }
        execute_on(&db, &sql).await.unwrap();
        let foreign = transaction(8, &ids, "UPSERT pe:30_1 SET dbnum=8").unwrap();
        assert!(execute_on(&db, &foreign).await.is_err());
        db.query("UPDATE dbnum_info_table:30 SET count=0;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(
            execute_on(&db, &transaction(7, &ids, "DELETE pe:30_1").unwrap())
                .await
                .is_err()
        );
        let mut result = db.query("RETURN pe:30_1.dbnum;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(7));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_batches_and_interrupted_replay_converge() {
        let db = database("concurrent").await;
        // 两块整 + 一个零头：块大小随 `tx_limits` 走，别把 100 写死。
        let per_batch = batch_elements();
        let entries = (1..=2 * per_batch + 5)
            .map(|n| {
                (
                    Some(format!("40_{n}")),
                    format!("UPSERT pe:40_{n} SET dbnum=7, sesno=2"),
                )
            })
            .collect::<Vec<_>>();
        let txs = batches(7, &entries).unwrap();
        assert!(txs.len() >= 3);
        execute_on(&db, &txs[0]).await.unwrap();
        let (a, b) = tokio::join!(execute_on(&db, &txs[1]), execute_on(&db, &txs[2]));
        a.unwrap();
        b.unwrap();
        for sql in &txs {
            execute_on(&db, sql).await.unwrap();
        }
        let mut result = db.query("RETURN dbnum_info_table:40.count;").await.unwrap();
        assert_eq!(
            result.take::<Option<i64>>(0).unwrap(),
            Some((2 * per_batch + 5) as i64)
        );
    }

    #[test]
    fn batch_limits_include_statistics_and_validate_explicit_ids() {
        assert_eq!(write_rows("IF true { UPSERT pe:1_1 SET name = '/*'; IF true { INSERT INTO pe [{id: pe:1_2}, {id: pe:1_3}]; }; };").unwrap(), 3);
        assert!(transaction(7, &["1_2; DELETE pe".into()], "RETURN NONE").is_err());
        let payload = "x".repeat(max_batch_bytes() / 2);
        let entries = (1..=3)
            .map(|n| {
                (
                    Some(format!("40_{n}")),
                    format!("UPSERT pe:40_{n} SET dbnum=7, payload='{payload}'"),
                )
            })
            .collect::<Vec<_>>();
        let txs = batches(7, &entries).unwrap();
        assert_eq!(txs.len(), 3);
        assert!(txs.iter().all(|sql| sql.len() <= max_batch_bytes()));
    }

    #[tokio::test]
    async fn actual_rows_make_replay_soft_delete_and_rollback_safe() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("stats").use_db("rows").await.unwrap();
        let ids = vec!["24384_1".into(), "24384_2".into()];
        let insert = transaction(8000, &ids, "INSERT IGNORE INTO pe [{id: pe:24384_1, dbnum:8000, sesno:5}, {id: pe:24384_2, dbnum:8000, sesno:6}]").unwrap();
        for _ in 0..2 {
            execute_on(&db, &insert).await.unwrap();
        }
        for deleted in [true, false, true, true] {
            let sql = transaction(
                8000,
                &ids,
                &format!("UPDATE pe:24384_1 SET deleted={deleted}, sesno=7"),
            )
            .unwrap();
            execute_on(&db, &sql).await.unwrap();
        }
        let mut result = db
            .query("RETURN dbnum_info_table:24384.count;")
            .await
            .unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(2));
        let failed =
            transaction(8000, &ids, "DELETE pe:24384_2; THROW 'injected failure'").unwrap();
        assert!(execute_on(&db, &failed).await.is_err());
        let delete = transaction(8000, &ids, "DELETE pe:24384_2").unwrap();
        for _ in 0..2 {
            execute_on(&db, &delete).await.unwrap();
        }
        let mut result = db.query("RETURN dbnum_info_table:24384.count; RETURN dbnum_info_table:24384.max_ref1; RETURN dbnum_info_table:24384.sesno;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(1));
        assert_eq!(result.take::<Option<i64>>(1).unwrap(), Some(2));
        assert_eq!(result.take::<Option<i64>>(2).unwrap(), Some(7));
    }
}
