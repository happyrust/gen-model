//! 解析错误账本。
//!
//! 起因：解析这一侧的失败在此之前**没有任何落地的地方**。模型生成失败有
//! [`model_update_pending`](crate::data_interface::model_update_pending) 那一行的
//! `last_error` + `attempts` + 死信，窗口失败有 `increment_update_attempt`，唯独
//! 解析失败是一句 `log::warn!` 就没了：`element_parse_skipped` 之后这个元素按
//! cache-miss 处理照常往下走，`locator_scan_failed` 之后这个库当轮不可定位——两条
//! 都是「降级继续跑」，不报错、不阻断、也**不留痕**。事后想知道哪些元素解析不出来，
//! 只能去翻当时的控制台。
//!
//! 所以这里只做一件事：把解析失败按目标归行，记下次数、首末时刻与最近一句错误，
//! 并在同一个目标下次解析成功时销账。它**不是工作队列**——没有人重试这些行，重试
//! 由上层各自的路径决定；它是一份「现在有哪些东西解析不出来」的可查清单。
//!
//! ## 为什么先攒后写
//!
//! 记录点在热路径上：`element_parse_skipped` 位于逐元素的解析循环里，一个坏文件
//! 能在一轮里刷出成千上万条。每条一次 UPSERT 会把「解析慢」变成「解析卡死」。
//! 因此同步侧只往进程内缓冲区里攒（同一目标合并计数），由调用链上最近的 async
//! 收口点 [`flush`] 一次性落库。
//!
//! ## 销账
//!
//! 只对**本进程见过它失败**的目标发 DELETE（[`KNOWN_FAILED`]，首次 flush 时从表里
//! 播种）。否则每一轮成功解析的几万个元素都要陪跑一条 DELETE，而其中绝大多数
//! 本来就不在表里。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use aios_core::SUL_DB;

use crate::data_interface::dbnum_state::escape_surql_str;

pub const TABLE: &str = "parse_error";

/// 一个目标在缓冲区里的待写状态。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    /// 失败了 `count` 次，最近一句是 `error`。
    Failed { count: u64, error: String },
    /// 解析成功，销账。
    Cleared,
}

/// `(kind, target)`：与记录 id `parse_error:[kind, target]` 一一对应。
type Key = (&'static str, String);

static PENDING: Mutex<BTreeMap<Key, Pending>> = Mutex::new(BTreeMap::new());
/// 本进程已知在表里有行的目标；决定一次成功要不要发 DELETE。
static KNOWN_FAILED: Mutex<Option<HashSet<Key>>> = Mutex::new(None);

const ELEMENT: &str = "element";
const FILE: &str = "file";

fn pending() -> std::sync::MutexGuard<'static, BTreeMap<Key, Pending>> {
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn known_failed() -> std::sync::MutexGuard<'static, Option<HashSet<Key>>> {
    KNOWN_FAILED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 攒一次失败。同一目标在一轮里失败多次只占一行，计数累加，错误取最后一句。
fn note_failure(key: Key, error: &str) {
    let mut buffer = pending();
    match buffer.get_mut(&key) {
        Some(Pending::Failed { count, error: last }) => {
            *count += 1;
            last.clear();
            last.push_str(error);
        }
        // 同一轮里先成功后失败：以失败为准，销账作废。
        _ => {
            buffer.insert(
                key,
                Pending::Failed {
                    count: 1,
                    error: error.to_string(),
                },
            );
        }
    }
}

/// 攒一次销账。只对本进程见过失败的目标记——见 [`KNOWN_FAILED`]。
fn note_success(key: Key) {
    if !known_failed()
        .as_ref()
        .is_some_and(|set| set.contains(&key))
    {
        return;
    }
    let mut buffer = pending();
    // 同一轮里已经记了失败就不销账：坏的那次更值得留下。
    if !matches!(buffer.get(&key), Some(Pending::Failed { .. })) {
        buffer.insert(key, Pending::Cleared);
    }
}

/// 一个元素解析不出来（`element_parse_skipped`）。
pub(crate) fn note_element_failure(target: &str, error: &str) {
    note_failure((ELEMENT, target.to_string()), error);
}

/// 这个元素这一轮解析成功了。
pub(crate) fn note_element_success(target: &str) {
    note_success((ELEMENT, target.to_string()));
}

/// 一个库文件扫不动（`locator_scan_failed`）。
pub(crate) fn note_file_failure(path: &Path, error: &str) {
    note_failure((FILE, path.display().to_string()), error);
}

/// 这个库文件这一轮扫通了。
pub(crate) fn note_file_success(path: &Path) {
    note_success((FILE, path.display().to_string()));
}

fn record_id(kind: &str, target: &str) -> String {
    format!("{TABLE}:['{kind}', '{}']", escape_surql_str(target))
}

/// 累计计数只涨不覆盖，首见时刻只写一次——两条都要经得起同一目标反复失败。
fn render_upsert(kind: &str, target: &str, count: u64, error: &str) -> String {
    format!(
        "UPSERT {id} SET kind = '{kind}', target = '{target_text}', \
         occurrences = (occurrences?:0) + {count}, \
         first_seen_at = first_seen_at?:time::now(), \
         last_seen_at = time::now(), last_error = '{error_text}';",
        id = record_id(kind, target),
        target_text = escape_surql_str(target),
        error_text = escape_surql_str(error),
    )
}

fn render_delete(kind: &str, target: &str) -> String {
    format!("DELETE {};", record_id(kind, target))
}

/// 把表里现有的行播种进 [`KNOWN_FAILED`]。
///
/// 不播种的话，上一个进程记下的行永远等不到销账——它这一轮解析成功，而本进程
/// 「没见过它失败」，于是 DELETE 发不出去，一条已经修好的记录会一直挂在清单上。
async fn seed_known_failed() -> anyhow::Result<()> {
    if known_failed().is_some() {
        return Ok(());
    }
    let mut response = SUL_DB
        .query(format!("SELECT kind, target FROM {TABLE};"))
        .await?
        .check()?;
    #[derive(serde::Deserialize)]
    struct Row {
        kind: String,
        target: String,
    }
    let rows: Vec<Row> = response.take(0)?;
    let seeded = rows
        .into_iter()
        .filter_map(|row| match row.kind.as_str() {
            ELEMENT => Some((ELEMENT, row.target)),
            FILE => Some((FILE, row.target)),
            _ => None,
        })
        .collect::<HashSet<_>>();
    *known_failed() = Some(seeded);
    Ok(())
}

/// 把攒下的解析结果落库，返回写了几行。
///
/// 播种失败不能连累落库：账本读不出来最多让销账晚一轮，而丢掉这一批失败记录
/// 等于这一轮解析问题彻底没留痕——那正是这张表要解决的事。
pub async fn flush() -> anyhow::Result<usize> {
    if pending().is_empty() {
        return Ok(0);
    }
    if let Err(error) = seed_known_failed().await {
        log::warn!("[parse_error] 已有解析错误行播种失败（本轮只写不销账）: {error:#}");
    }
    let batch = std::mem::take(&mut *pending());
    if batch.is_empty() {
        return Ok(0);
    }

    let mut statements = Vec::with_capacity(batch.len());
    {
        let mut known = known_failed();
        for ((kind, target), entry) in &batch {
            match entry {
                Pending::Failed { count, error } => {
                    statements.push(render_upsert(kind, target, *count, error));
                    if let Some(set) = known.as_mut() {
                        set.insert((kind, target.clone()));
                    }
                }
                Pending::Cleared => {
                    statements.push(render_delete(kind, target));
                    if let Some(set) = known.as_mut() {
                        set.remove(&(*kind, target.clone()));
                    }
                }
            }
        }
    }

    let written = statements.len();
    let sql = statements.join("\n");
    // 失败就把这批放回缓冲区：解析侧没有重试通道，这里丢了就是永久丢了。
    if let Err(error) = SUL_DB
        .query(sql)
        .await
        .and_then(|response| response.check())
    {
        let mut buffer = pending();
        for (key, entry) in batch {
            buffer.entry(key).or_insert(entry);
        }
        anyhow::bail!("[parse_error] 解析错误账本落库失败（已退回缓冲区）: {error}");
    }
    Ok(written)
}

/// `/health` 用：现在有多少东西解析不出来、最近一条长什么样。
///
/// 表为空就是 `null`——和 `idle_round_panic` 同一个约定：没有问题时不占版面。
pub async fn snapshot() -> Option<serde_json::Value> {
    #[derive(serde::Deserialize)]
    struct Row {
        kind: String,
        target: String,
        #[serde(default)]
        occurrences: u64,
        #[serde(default)]
        last_error: Option<String>,
        #[serde(default)]
        last_seen_at: Option<String>,
    }
    let query = format!(
        "SELECT kind, target, occurrences, last_error, \
         type::string(last_seen_at) AS last_seen_at \
         FROM {TABLE} ORDER BY last_seen_at DESC;"
    );
    let rows: Vec<Row> = SUL_DB
        .query(query)
        .await
        .and_then(|response| response.check())
        .and_then(|mut response| response.take(0))
        .unwrap_or_default();
    let latest = rows.first()?;
    Some(serde_json::json!({
        "total": rows.len(),
        "elements": rows.iter().filter(|row| row.kind == ELEMENT).count(),
        "files": rows.iter().filter(|row| row.kind == FILE).count(),
        "occurrences": rows.iter().map(|row| row.occurrences).sum::<u64>(),
        "last_target": latest.target,
        "last_error": latest.last_error,
        "last_seen_at": latest.last_seen_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例自己的缓冲区状态：静态是全局的，用完必须还原，否则并行跑互相踩。
    fn reset() {
        pending().clear();
        *known_failed() = Some(HashSet::new());
    }

    /// 同一目标一轮里失败多次只占一行，计数累加、错误取最后一句。
    ///
    /// 这是「先攒后写」的全部意义：`element_parse_skipped` 在逐元素循环里，一个
    /// 坏文件一轮能刷上万条，逐条 UPSERT 会把解析慢变成解析卡死。
    #[test]
    fn repeated_failures_of_one_target_collapse_into_one_row() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();

        note_element_failure("4000000001/11", "APPDAR not exist in attr_in");
        note_element_failure("4000000001/11", "页式元素解析失败");
        note_element_failure("4000000001/12", "APPDAR not exist in attr_in");

        let buffer = pending();
        assert_eq!(buffer.len(), 2);
        assert_eq!(
            buffer.get(&(ELEMENT, "4000000001/11".to_string())),
            Some(&Pending::Failed {
                count: 2,
                error: "页式元素解析失败".to_string(),
            })
        );
    }

    /// 没见过它失败就不为一次成功发 DELETE。
    ///
    /// 否则每轮成功解析的几万个元素都要陪跑一条 DELETE，而其中绝大多数根本不在表里。
    #[test]
    fn success_of_a_never_failed_target_writes_nothing() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();

        note_element_success("4000000001/11");

        assert!(pending().is_empty());
    }

    /// 见过它失败，这次成功了：销账。同一轮里又失败的话以失败为准。
    #[test]
    fn a_repaired_target_is_cleared_but_a_fresh_failure_wins() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        known_failed()
            .as_mut()
            .unwrap()
            .insert((ELEMENT, "4000000001/11".to_string()));

        note_element_success("4000000001/11");
        assert_eq!(
            pending().get(&(ELEMENT, "4000000001/11".to_string())),
            Some(&Pending::Cleared)
        );

        note_element_failure("4000000001/11", "又炸了");
        assert!(matches!(
            pending().get(&(ELEMENT, "4000000001/11".to_string())),
            Some(Pending::Failed { count: 1, .. })
        ));

        // 失败之后再来一次成功不覆盖它：坏的那次更值得留下。
        note_element_success("4000000001/11");
        assert!(matches!(
            pending().get(&(ELEMENT, "4000000001/11".to_string())),
            Some(Pending::Failed { .. })
        ));
    }

    /// 累计计数只涨不覆盖、首见时刻只写一次——同一目标反复失败时这两条是账的全部价值。
    #[test]
    fn the_upsert_accumulates_instead_of_overwriting() {
        let sql = render_upsert(ELEMENT, "4000000001/11", 3, "APPDAR not exist");
        assert!(sql.contains("occurrences = (occurrences?:0) + 3"), "{sql}");
        assert!(
            sql.contains("first_seen_at = first_seen_at?:time::now()"),
            "{sql}"
        );
        assert!(sql.contains("last_seen_at = time::now()"), "{sql}");
        assert!(
            sql.starts_with("UPSERT parse_error:['element', '4000000001/11']"),
            "{sql}"
        );
    }

    /// 目标文本进的是记录 id 与字段，路径里的单引号必须逃逸，否则一条 Windows 路径
    /// 就能让整批语句解析失败——而这批语句正是「解析失败」的唯一记录。
    #[test]
    fn quotes_in_a_target_cannot_break_the_statement() {
        let sql = render_upsert(
            FILE,
            "D:\\proj\\o'brien\\ams1",
            "boom".len() as u64,
            "o'boom",
        );
        assert!(!sql.contains("o'brien\\"), "{sql}");
        assert!(sql.contains("o\\'brien"), "{sql}");
        assert!(sql.contains("o\\'boom"), "{sql}");
    }

    /// 语句得真能在 SurrealDB 上跑：数组记录 id、计数累加、首见时刻不被覆盖、销账。
    ///
    /// 纯字符串断言查不出 `parse_error:['element', '…']` 这种 id 写法是不是合法
    /// SurrealQL——而这批语句一旦解析失败，「解析失败」这件事本身就没了记录。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_ledger_statements_round_trip_on_surreal() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("parse_error")
            .await
            .expect("select fixture db");

        db.query(render_upsert(
            ELEMENT,
            "4000000001/11",
            2,
            "APPDAR not exist",
        ))
        .await
        .expect("first upsert")
        .check()
        .expect("first upsert statement");
        let mut response = db
            .query(format!(
                "SELECT type::string(first_seen_at) AS first_seen_at FROM {TABLE};"
            ))
            .await
            .expect("read first_seen_at")
            .check()
            .expect("read first_seen_at statement");
        let first: Vec<serde_json::Value> = response.take(0).expect("decode first_seen_at");
        let first_seen = first[0]["first_seen_at"].clone();

        db.query(render_upsert(
            ELEMENT,
            "4000000001/11",
            3,
            "页式元素解析失败",
        ))
        .await
        .expect("second upsert")
        .check()
        .expect("second upsert statement");

        let mut response = db
            .query(format!(
                "SELECT kind, target, occurrences, last_error, \
                 type::string(first_seen_at) AS first_seen_at FROM {TABLE};"
            ))
            .await
            .expect("read")
            .check()
            .expect("read statement");
        let rows: Vec<serde_json::Value> = response.take(0).expect("decode");
        assert_eq!(rows.len(), 1, "同一目标只占一行: {rows:?}");
        assert_eq!(rows[0]["occurrences"], 5, "计数必须累加而不是覆盖");
        assert_eq!(rows[0]["last_error"], "页式元素解析失败");
        assert_eq!(rows[0]["target"], "4000000001/11");
        assert_eq!(rows[0]["first_seen_at"], first_seen, "首见时刻只写一次");

        db.query(render_delete(ELEMENT, "4000000001/11"))
            .await
            .expect("clear")
            .check()
            .expect("clear statement");
        let mut response = db
            .query(format!("SELECT target FROM {TABLE};"))
            .await
            .expect("recount")
            .check()
            .expect("recount statement");
        let remaining: Vec<serde_json::Value> = response.take(0).expect("decode remaining");
        assert!(remaining.is_empty(), "销账之后不该还有行: {remaining:?}");
    }

    static TEST_LOCK: Mutex<()> = Mutex::new(());
}
