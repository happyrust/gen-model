//! PE 写事务的包装：`BEGIN / COMMIT`、字节 / 写行上限、按上限切块、带重试的执行。
//!
//! 原住 `dbnum_stats.rs`——那时每笔事务还带 `dbnum_info_table` 的逐笔 ±1 观测块，包装与统计是一体的。
//! 2026-09-10（ADR-071 D6 / S7）观测块退役之后，包装这一半与统计无关，搬到这里；
//! 2026-11-07 Ref0 统计整体退役，`dbnum_stats.rs` 随之删除。
//! `render` / `transaction` / `batches` 的签名照旧收 `ids`——只做规范性校验（`ref0_ref1`），不再进任何语句。

use surrealdb::{Surreal, engine::any::Any};

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

/// 一笔事务的写行数 = 正文的 PE / 属性 / 边写。观测块退了之后没有每 Ref0 那一笔 UPSERT 要算。
fn batch_rows(_ids: &[String], writes: &str) -> anyhow::Result<u64> {
    write_rows(writes)
}

pub async fn retire_event_on(db: &Surreal<Any>) -> anyhow::Result<()> {
    crate::surreal_retry::execute_surreal_checked_on(
        db,
        "REMOVE EVENT IF EXISTS update_dbnum_event ON pe;",
        "retire PE statistics event",
    )
    .await
}

/// 旧库那个 `update_dbnum_event`（由事件维护统计）不许被装回来：它会把已退役的 Ref0
/// 统计表重新写出来。启动 `retire_dbnum_event` 删一次；清库 / 暂存收尾路上显式核对一次。
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

/// Bare statements: the caller owns the transaction boundary and must include
/// the entire returned body. `ids` are canonical `ref0_ref1` strings——只做校验：
/// 观测块已退（ADR-071 D6），正文原样返回，`dbnum` 与 `ids` 不再进任何语句。
pub fn render(_dbnum: u32, ids: &[String], writes: &str) -> anyhow::Result<String> {
    for id in ids {
        let (a, b) = id
            .split_once('_')
            .ok_or_else(|| anyhow::anyhow!("invalid PE id {id}"))?;
        let ref0: u32 = a.parse()?;
        let ref1: u32 = b.parse()?;
        anyhow::ensure!(*id == format!("{ref0}_{ref1}"), "noncanonical PE id {id}");
    }
    // 观测块在时正文后面总跟着一个 `;`——调用方（`read_through` 等）拼语句时省掉最后那个分号
    // 一直是被它兜着的，这里照旧兜住，别让 `COMMIT` 紧贴上一条语句。
    if writes.trim_end().ends_with(';') {
        Ok(writes.to_string())
    } else {
        Ok(format!("{writes};"))
    }
}

pub fn transaction(dbnum: u32, ids: &[String], writes: &str) -> anyhow::Result<String> {
    transaction_with_limits(dbnum, ids, writes, super::tx_limits::data_tx_limits())
}

/// 一笔 PE 写事务有多大：包上 `BEGIN / COMMIT` 之后的字节数，与正文的写行数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxSize {
    pub bytes: usize,
    pub rows: u64,
}

/// 一笔事务超过了切块上限。
///
/// 上限是**怎么切**的依据，不是拒收数据的理由：拿到它的调用方把这一块再切小——`element_batch` 把元素劈半，
/// 劈到只剩一个元素就把它的成员边分几笔写（gen-model #155）；[`insert_ignore_batches`] 劈半。真切不动才把它上报，
/// 文案带实际大小与上限。字节先超时不再数行（数行要把整段正文解析一遍），`rows` 为 `None`。
///
/// 此前的文案是 `PE statistics batch exceeds … limit`——「statistics」是逐笔观测块还在时留下的叫法
/// （ADR-071 D6 已退），这里数的一直是正文自己的行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExceedsTxLimits {
    pub bytes: usize,
    pub rows: Option<u64>,
    pub limits: super::tx_limits::DataTxLimits,
}

impl std::fmt::Display for ExceedsTxLimits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.rows {
            None => write!(
                f,
                "PE write transaction exceeds the byte limit: {} bytes > {}",
                self.bytes, self.limits.max_bytes
            ),
            Some(rows) => write!(
                f,
                "PE write transaction exceeds the write-row limit: {rows} write rows > {} ({} bytes)",
                self.limits.max_write_rows, self.bytes
            ),
        }
    }
}

impl std::error::Error for ExceedsTxLimits {}

/// [`transaction`] 的显式上限形态。超限时的错误是 [`ExceedsTxLimits`]（`error.is::<ExceedsTxLimits>()`）。
pub fn transaction_with_limits(
    dbnum: u32,
    ids: &[String],
    writes: &str,
    limits: super::tx_limits::DataTxLimits,
) -> anyhow::Result<String> {
    let sql = format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        render(dbnum, ids, writes)?
    );
    if sql.len() > limits.max_bytes {
        return Err(anyhow::Error::new(ExceedsTxLimits {
            bytes: sql.len(),
            rows: None,
            limits,
        }));
    }
    let rows = batch_rows(ids, writes)?;
    if rows > limits.max_write_rows {
        return Err(anyhow::Error::new(ExceedsTxLimits {
            bytes: sql.len(),
            rows: Some(rows),
            limits,
        }));
    }
    Ok(sql)
}

/// 同 [`transaction_with_limits`]，写行数由调用方给：正文是调用方自己渲的、行数按构造就知道时用
/// （`element_batch` 分笔写的成员边——一条边一行），省掉为了数行把整段正文解析一遍（调试版里解析一兆正文要一秒
/// 上下，一个上万成员的属主光边就一兆）。字节照样量，id 照样校验。
pub fn transaction_with_known_rows(
    dbnum: u32,
    ids: &[String],
    writes: &str,
    rows: u64,
    limits: super::tx_limits::DataTxLimits,
) -> anyhow::Result<String> {
    let sql = format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        render(dbnum, ids, writes)?
    );
    if sql.len() > limits.max_bytes {
        return Err(anyhow::Error::new(ExceedsTxLimits {
            bytes: sql.len(),
            rows: None,
            limits,
        }));
    }
    if rows > limits.max_write_rows {
        return Err(anyhow::Error::new(ExceedsTxLimits {
            bytes: sql.len(),
            rows: Some(rows),
            limits,
        }));
    }
    Ok(sql)
}

/// 这段正文包成一笔事务有多大——不看上限。切块的人据此算「这笔事务里还装得下多少」。
pub fn measure(dbnum: u32, ids: &[String], writes: &str) -> anyhow::Result<TxSize> {
    let sql = format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        render(dbnum, ids, writes)?
    );
    Ok(TxSize {
        bytes: sql.len(),
        rows: batch_rows(ids, writes)?,
    })
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
                "single PE replacement exceeds the data transaction limits: {} bytes (limits: {} bytes / {} write rows)",
                sql.len(),
                max_batch_bytes(),
                max_batch_write_rows()
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

/// 执行一笔 PE 写事务（乐观冲突指数退避重试）。此前每笔都先 `INFO FOR TABLE pe` 查一遍旧事件
/// 有没有被装回来（[`require_no_event_on`]）——观测块退了，这张表的统计不再靠事务维护，那一查
/// 也随之退成启动时一次（`retire_dbnum_event`）与清库路上的显式核对。
pub async fn execute_on(db: &Surreal<Any>, sql: &str) -> anyhow::Result<()> {
    crate::surreal_retry::execute_surreal_checked_on(db, sql, "PE batch").await
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
        db.use_ns("pe_batch").use_db(label).await.unwrap();
        db
    }

    /// 观测块已退（ADR-071 D6）：`render` 只校验 id、正文原样返回——没有 `$before_` / `$after_`、
    /// 没有版本检查、`dbnum_info_table` 一个字都不出现；`ids` 不规范照旧拒绝。
    #[test]
    fn render_validates_ids_and_returns_the_writes_verbatim() {
        let ids = vec!["24384_1".to_string(), "24384_2".to_string()];
        let writes = "UPSERT pe:24384_1 SET dbnum=8000; UPSERT pe:24384_2 SET dbnum=8000;";
        assert_eq!(render(8000, &ids, writes).unwrap(), writes);
        assert_eq!(render(8000, &[], writes).unwrap(), writes);
        // 末尾没分号的正文补一个（老观测块一直替调用方兜着这一下）。
        assert_eq!(
            render(8000, &ids, "UPSERT pe:24384_1 SET dbnum=8000").unwrap(),
            "UPSERT pe:24384_1 SET dbnum=8000;"
        );
        let sql = transaction(8000, &ids, writes).unwrap();
        assert_eq!(
            sql,
            format!("BEGIN TRANSACTION;\n{writes}\nCOMMIT TRANSACTION;")
        );
        for bad in ["24384", "024384_1", "24384_01", "1_2; DELETE pe", "a_b"] {
            assert!(render(8000, &[bad.to_string()], writes).is_err(), "{bad}");
        }
        let source = include_str!("pe_batch.rs");
        let render_body = source
            .split_once("pub fn render(")
            .expect("render")
            .1
            .split_once("pub fn transaction(")
            .expect("transaction follows render")
            .0;
        for retired in [
            "LET $before_",
            "LET $after_",
            "array::len($before_",
            "PE statistics missing or inconsistent",
            "dbnum_info_table",
        ] {
            assert!(
                !render_body.contains(retired),
                "逐笔观测块已退，不得回来: {retired}"
            );
        }
    }

    /// 旧事件被装回来时，清库 / 暂存收尾那条显式核对要拒绝；删掉（幂等）之后放行。
    /// `execute_on` 自己不再每笔查一遍 `INFO FOR TABLE pe`。
    #[tokio::test]
    async fn the_legacy_event_check_is_explicit_and_idempotent_to_retire() {
        let db = database("guards").await;
        db.query("DEFINE EVENT update_dbnum_event ON pe WHEN true THEN { RETURN NONE; };")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(require_no_event_on(&db).await.is_err());
        for _ in 0..2 {
            retire_event_on(&db).await.unwrap();
        }
        require_no_event_on(&db).await.unwrap();
        let ids = vec!["30_1".into()];
        let sql = transaction(7, &ids, "UPSERT pe:30_1 SET dbnum=7, sesno=1").unwrap();
        execute_on(&db, &sql).await.unwrap();
        let mut result = db.query("RETURN pe:30_1.dbnum;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(7));
        let source = include_str!("pe_batch.rs");
        let execute = source
            .split_once("pub async fn execute_on(")
            .expect("execute_on")
            .1
            .split_once("pub fn insert_ignore_batches(")
            .expect("insert_ignore_batches follows execute_on")
            .0;
        assert!(
            !execute.contains("require_no_event_on"),
            "每笔一次 INFO FOR TABLE pe 已退: {execute}"
        );
    }

    /// 切块与重放：块大小随 `tx_limits` 走；同一批事务并发 / 重放都收敛到同一份 pe 行。
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_batches_and_interrupted_replay_converge() {
        let db = database("concurrent").await;
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
        let mut result = db
            .query("SELECT count() AS n FROM pe WHERE dbnum = 7 GROUP ALL;")
            .await
            .unwrap();
        #[derive(serde::Deserialize)]
        struct Count {
            n: i64,
        }
        let rows: Vec<Count> = result.take(0).unwrap();
        assert_eq!(rows[0].n, (2 * per_batch + 5) as i64);
    }

    /// 超限的错误带类型（调用方靠它决定「再切」），文案带实际大小与上限、不再叫「statistics」；字节先超时不数行。
    /// [`measure`] 不看上限。不是大小的错（id 不规范）不带这个类型。
    #[test]
    fn exceeding_a_limit_is_a_typed_error_that_states_the_size_and_the_limit() {
        use super::super::tx_limits::DataTxLimits;
        let ids = vec!["40_1".to_string()];
        let writes = "INSERT IGNORE INTO pe [{ id: pe:40_1 }, { id: pe:40_2 }, { id: pe:40_3 }] RETURN NONE;";
        let roomy = DataTxLimits::default();
        let sql = transaction_with_limits(7, &ids, writes, roomy).unwrap();
        assert_eq!(sql, transaction(7, &ids, writes).unwrap());
        assert_eq!(
            measure(7, &ids, writes).unwrap(),
            TxSize {
                bytes: sql.len(),
                rows: 3
            }
        );

        let two_rows = DataTxLimits {
            max_write_rows: 2,
            ..roomy
        };
        let by_rows = transaction_with_limits(7, &ids, writes, two_rows).unwrap_err();
        let exceeded = by_rows
            .downcast_ref::<ExceedsTxLimits>()
            .expect("超限是带类型的错");
        assert_eq!((exceeded.bytes, exceeded.rows), (sql.len(), Some(3)));
        let text = by_rows.to_string();
        assert!(text.contains("3 write rows > 2"), "{text}");
        assert!(!text.contains("statistics"), "{text}");

        let tight = DataTxLimits {
            max_bytes: sql.len() - 1,
            ..two_rows
        };
        let by_bytes = transaction_with_limits(7, &ids, writes, tight).unwrap_err();
        let exceeded = by_bytes
            .downcast_ref::<ExceedsTxLimits>()
            .expect("超限是带类型的错");
        assert_eq!(exceeded.rows, None, "字节先超就不数行");
        let text = by_bytes.to_string();
        assert!(
            text.contains(&format!("{} bytes > {}", sql.len(), sql.len() - 1)),
            "{text}"
        );

        let invalid =
            transaction_with_limits(7, &["1_2; DELETE pe".into()], writes, roomy).unwrap_err();
        assert!(!invalid.is::<ExceedsTxLimits>(), "{invalid:#}");

        // 行数由调用方给的形态：同一段正文、同一个结果，只是不解析正文数行；id 照样校验、字节照样量。
        assert_eq!(
            transaction_with_known_rows(7, &ids, writes, 3, roomy).unwrap(),
            sql
        );
        let by_known_rows = transaction_with_known_rows(7, &ids, writes, 3, two_rows).unwrap_err();
        assert_eq!(
            by_known_rows.downcast_ref::<ExceedsTxLimits>(),
            by_rows.downcast_ref::<ExceedsTxLimits>()
        );
        assert!(
            transaction_with_known_rows(7, &ids, writes, 3, tight)
                .unwrap_err()
                .downcast_ref::<ExceedsTxLimits>()
                .is_some_and(|exceeded| exceeded.rows.is_none())
        );
        assert!(
            transaction_with_known_rows(7, &["1_2; DELETE pe".into()], writes, 3, roomy).is_err()
        );
    }

    #[test]
    fn batch_limits_validate_explicit_ids_and_split_by_bytes() {
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
}
