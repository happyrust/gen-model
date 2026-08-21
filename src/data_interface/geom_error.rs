//! 布尔降级账本。
//!
//! 起因：2026-08-19 现场，BEND `24384/23259` 的正实体网格进不了 manifold
//! （`manifold3d status: NotManifold`），这一句错抛上去把 BRAN `/C-OR-1R345-C` 的
//! `regen_root` 连撞 5 次撞成死信——同一批里另外 9 个根做完了也没用，
//! `model_ready` 就此永远停在 false。窗口外的确定性坏几何采用可观察降级；活动
//! 暂存窗口是正确性边界，采用 `Required` 并阻断水位。
//!
//! 跳过之后这件事就没有落脚点了：`model_update_pending` 那一行不再产生（不失败
//! 也就没有 `last_error` 与死信计数），控制台那句 `println!` 会滚走。这张表补的
//! 就是这个洞——**哪些元素的布尔被降级了、栽在哪块几何上、多少次、最近一次什么
//! 时候**，直接查表就能回答：
//!
//! ```surql
//! SELECT * FROM geom_error ORDER BY last_seen_at DESC;
//! SELECT target, geom, occurrences, last_error FROM geom_error WHERE kind = 'bool_pos';
//! ```
//!
//! 它**不是工作队列**——没有人重试这些行。几何修好（或换目录）之后，同一个元素
//! 下次布尔成功时自动销账。
//!
//! ## 为什么直接写 `SUL_DB`
//!
//! 这是诊断账本，不是数据面：窗口回滚了它也得留着——几何确实是坏的，这件事跟那
//! 个暂存窗口成不成功没关系。累加计数（`occurrences + 1`）本身也不满足 journal
//! 要求的幂等，进不了暂存语句日志。
//!
//! ## 销账为什么要先播种
//!
//! 只对**表里确实有行**的目标发 DELETE（[`KNOWN`]，首次记账/销账时从表里播种）。
//! 否则每一轮布尔成功的几万个元素都要陪跑一条 DELETE，而其中绝大多数本来就不在
//! 表里。播种同时解决跨进程的问题：上一个进程记下的行，本进程没见过它失败，不
//! 播种就永远等不到销账。

use std::collections::HashSet;
use std::sync::Mutex;

use aios_core::SUL_DB;

use crate::data_interface::dbnum_state::escape_surql_str;

pub const TABLE: &str = "geom_error";

/// 正实体网格载不进 manifold 的诊断类型。
pub(crate) const BOOL_POS: &str = "bool_pos";
/// 负实体网格载不进 manifold 的诊断类型。
pub(crate) const BOOL_NEG: &str = "bool_neg";
/// 基本体数据无法产生可用 BREP（缺失、非法或变换含 NaN）。
pub(crate) const PRIMITIVE: &str = "primitive";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryFailurePolicy {
    Required,
    BestEffortFallback,
}

const BOOL_KINDS: [&str; 2] = [BOOL_POS, BOOL_NEG];

/// 本进程已知在表里有行的目标；决定一次成功要不要发 DELETE。
static KNOWN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn known() -> std::sync::MutexGuard<'static, Option<HashSet<String>>> {
    KNOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn known_key(kind: &str, target: &str) -> String {
    format!("{kind}\0{target}")
}

fn record_id(kind: &str, target: &str) -> String {
    format!("{TABLE}:['{kind}', '{}']", escape_surql_str(target))
}

/// 累计计数只涨不覆盖、首见时刻只写一次——同一块坏几何会被成百上千个实例踩到，
/// 这两条决定了这本账能不能回答「有多大规模、从什么时候开始」。
fn render_upsert(kind: &str, target: &str, geom: &str, error: &str) -> String {
    format!(
        "UPSERT {id} SET kind = '{kind}', target = '{target_text}', geom = '{geom_text}', \
         occurrences = (occurrences?:0) + 1, \
         first_seen_at = first_seen_at?:time::now(), \
         last_seen_at = time::now(), last_error = '{error_text}';",
        id = record_id(kind, target),
        target_text = escape_surql_str(target),
        geom_text = escape_surql_str(geom),
        error_text = escape_surql_str(error),
    )
}

fn render_primitive_upsert(target: &str, noun: &str, error: &str) -> String {
    format!(
        "UPSERT {id} SET kind = '{PRIMITIVE}', target = '{target_text}', noun = '{noun_text}', \
         occurrences = (occurrences?:0) + 1, \
         first_seen_at = first_seen_at?:time::now(), \
         last_seen_at = time::now(), last_error = '{error_text}';",
        id = record_id(PRIMITIVE, target),
        target_text = escape_surql_str(target),
        noun_text = escape_surql_str(noun),
        error_text = escape_surql_str(error),
    )
}

/// 一个元素布尔成功就把它这两类降级记录一起清掉：正/负实体是同一件事的两侧，
/// 留半条会让清单说「它还坏着」。
fn render_clear(target: &str) -> String {
    BOOL_KINDS
        .iter()
        .map(|kind| format!("DELETE {};", record_id(kind, target)))
        .collect()
}

fn render_clear_kind(kind: &str, target: &str) -> String {
    format!("DELETE {};", record_id(kind, target))
}

/// 把表里现有的行播种进 [`KNOWN`]。
async fn seed() -> anyhow::Result<()> {
    if known().is_some() {
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
    *known() = Some(
        rows.into_iter()
            .map(|row| known_key(&row.kind, &row.target))
            .collect(),
    );
    Ok(())
}

/// 记一次布尔降级。
///
/// 记账失败只发一句 warn：跳过这件事已经定了，不能因为账本写不进去又把它变回硬
/// 错误——那正是这次改动要拆掉的东西。
pub(crate) async fn note_skip(kind: &'static str, target: &str, geom: &str, error: &str) {
    if let Err(error) = seed().await {
        log::warn!("[geom_error] 已有降级行播种失败（本轮不销账）: {error:#}");
    }
    if let Err(write_error) = SUL_DB
        .query(render_upsert(kind, target, geom, error))
        .await
        .and_then(|response| response.check())
    {
        log::warn!("[geom_error] 布尔降级记录落库失败 target={target}: {write_error}");
        return;
    }
    if let Some(set) = known().as_mut() {
        set.insert(known_key(kind, target));
    }
}

/// 严格记录一条基本体数据错误。调用方仍保留原来的生成失败语义；若诊断写入失败，
/// 错误会继续上浮，避免“模型坏了，但数据库里没有记录”的静默缺口。
pub(crate) async fn record_primitive_failure(
    target: &str,
    noun: &str,
    error: &str,
) -> anyhow::Result<()> {
    if let Err(seed_error) = seed().await {
        log::warn!("[geom_error] 已有错误行播种失败（继续尝试写当前基本体）: {seed_error:#}");
    }
    SUL_DB
        .query(render_primitive_upsert(target, noun, error))
        .await?
        .check()?;
    if let Some(set) = known().as_mut() {
        set.insert(known_key(PRIMITIVE, target));
    }
    Ok(())
}

/// 基本体重新生成成功后销掉它自己的错误行，不触碰同一目标可能存在的布尔错误。
pub(crate) async fn clear_primitive_failure(target: &str) -> anyhow::Result<()> {
    seed().await?;
    let key = known_key(PRIMITIVE, target);
    if !known().as_ref().is_some_and(|set| set.contains(&key)) {
        return Ok(());
    }
    SUL_DB
        .query(render_clear_kind(PRIMITIVE, target))
        .await?
        .check()?;
    if let Some(set) = known().as_mut() {
        set.remove(&key);
    }
    Ok(())
}

/// 这个元素这一轮布尔做成了：销账。没在册的目标一个 DELETE 都不发。
pub(crate) async fn note_success(target: &str) {
    if let Err(error) = seed().await {
        log::warn!("[geom_error] 已有降级行播种失败（本轮不销账）: {error:#}");
        return;
    }
    let listed = known().as_ref().is_some_and(|set| {
        BOOL_KINDS
            .iter()
            .any(|kind| set.contains(&known_key(kind, target)))
    });
    if !listed {
        return;
    }
    if let Err(error) = SUL_DB
        .query(render_clear(target))
        .await
        .and_then(|response| response.check())
    {
        log::warn!("[geom_error] 布尔降级销账失败 target={target}: {error}");
        return;
    }
    if let Some(set) = known().as_mut() {
        for kind in BOOL_KINDS {
            set.remove(&known_key(kind, target));
        }
    }
}

/// `/health` 用：现在有多少件的布尔被降级了、最近一条长什么样。
///
/// 表为空就是 `null`——与 `parse_errors` / `idle_round_panic` 同一个约定：没有问题
/// 时不占版面。
pub async fn snapshot() -> Option<serde_json::Value> {
    #[derive(serde::Deserialize)]
    struct Row {
        kind: String,
        target: String,
        #[serde(default)]
        geom: Option<String>,
        #[serde(default)]
        noun: Option<String>,
        #[serde(default)]
        occurrences: u64,
        #[serde(default)]
        last_error: Option<String>,
        #[serde(default)]
        last_seen_at: Option<String>,
    }
    let query = format!(
        "SELECT kind, target, geom, noun, occurrences, last_error, \
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
    let mut by_kind = serde_json::Map::new();
    for row in &rows {
        let count = by_kind
            .entry(row.kind.clone())
            .or_insert_with(|| serde_json::json!(0));
        *count = serde_json::json!(count.as_u64().unwrap_or_default() + 1);
    }
    Some(serde_json::json!({
        "total": rows.len(),
        "by_kind": by_kind,
        "occurrences": rows.iter().map(|row| row.occurrences).sum::<u64>(),
        "last_kind": latest.kind,
        "last_target": latest.target,
        "last_geom": latest.geom,
        "last_noun": latest.noun,
        "last_error": latest.last_error,
        "last_seen_at": latest.last_seen_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同一块坏几何会被成百上千个实例踩到：计数必须累加、首见时刻不许被覆盖，
    /// 否则这本账只能回答「最近一次」，回答不了「有多大、从什么时候开始」。
    #[test]
    fn the_upsert_accumulates_instead_of_overwriting() {
        let sql = render_upsert(
            BOOL_POS,
            "24384/23259",
            "9217283283071768476",
            "manifold3d status: NotManifold",
        );
        assert!(sql.contains("occurrences = (occurrences?:0) + 1"), "{sql}");
        assert!(
            sql.contains("first_seen_at = first_seen_at?:time::now()"),
            "{sql}"
        );
        assert!(sql.contains("last_seen_at = time::now()"), "{sql}");
        assert!(
            sql.starts_with("UPSERT geom_error:['bool_pos', '24384/23259']"),
            "{sql}"
        );
    }

    /// 一次成功要把正负两侧一起清掉：留半条会让清单继续说「它还坏着」。
    #[test]
    fn a_success_clears_both_sides_of_the_same_element() {
        let sql = render_clear("24384/23259");
        assert!(
            sql.contains("DELETE geom_error:['bool_pos', '24384/23259'];"),
            "{sql}"
        );
        assert!(
            sql.contains("DELETE geom_error:['bool_neg', '24384/23259'];"),
            "{sql}"
        );
    }

    #[test]
    fn primitive_success_only_clears_the_primitive_error() {
        let sql = render_clear_kind(PRIMITIVE, "24381/38635");
        assert_eq!(sql, "DELETE geom_error:['primitive', '24381/38635'];");
        assert!(!sql.contains("bool_pos"), "{sql}");
        assert!(!sql.contains("bool_neg"), "{sql}");
    }

    #[test]
    fn primitive_failure_persists_noun_dimensions_and_reference() {
        let sql = render_primitive_upsert(
            "24381/38635",
            "NCYL",
            "targeted primitive 24381_38635 (NCYL) produced an invalid BREP shape; DIAM=0, HEIG=0",
        );
        assert!(
            sql.contains("geom_error:['primitive', '24381/38635']"),
            "{sql}"
        );
        assert!(sql.contains("noun = 'NCYL'"), "{sql}");
        assert!(sql.contains("DIAM=0, HEIG=0"), "{sql}");
    }

    /// 目标与错误文本进的是记录 id 与字段，单引号必须逃逸——否则一句带撇号的错误
    /// 就能让整条语句解析失败，而这条语句正是「降级」的唯一记录。
    #[test]
    fn quotes_cannot_break_the_statement() {
        let sql = render_upsert(BOOL_NEG, "o'brien/1", "o'geom", "o'boom");
        assert!(!sql.contains("o'brien"), "{sql}");
        assert!(sql.contains("o\\'brien"), "{sql}");
        assert!(sql.contains("o\\'geom"), "{sql}");
        assert!(sql.contains("o\\'boom"), "{sql}");
    }

    /// 语句得真能在 SurrealDB 上跑：数组记录 id、计数累加、首见时刻不被覆盖、销账。
    ///
    /// 纯字符串断言查不出 `geom_error:['bool_pos', '…']` 是不是合法 SurrealQL，而这
    /// 批语句一旦解析失败，「布尔被降级过」这件事就彻底没了记录。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_ledger_statements_round_trip_on_surreal() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("geom_error")
            .await
            .expect("select fixture db");

        db.query(render_upsert(
            BOOL_POS,
            "24384/23259",
            "9217283283071768476",
            "manifold3d status: NotManifold",
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
            BOOL_POS,
            "24384/23259",
            "9217283283071768476",
            "manifold3d status: NotManifold",
        ))
        .await
        .expect("second upsert")
        .check()
        .expect("second upsert statement");

        let mut response = db
            .query(format!(
                "SELECT kind, target, geom, occurrences, \
                 type::string(first_seen_at) AS first_seen_at FROM {TABLE};"
            ))
            .await
            .expect("read")
            .check()
            .expect("read statement");
        let rows: Vec<serde_json::Value> = response.take(0).expect("decode");
        assert_eq!(rows.len(), 1, "同一目标同一类只占一行: {rows:?}");
        assert_eq!(rows[0]["occurrences"], 2, "计数必须累加而不是覆盖");
        assert_eq!(rows[0]["target"], "24384/23259");
        assert_eq!(rows[0]["geom"], "9217283283071768476");
        assert_eq!(rows[0]["first_seen_at"], first_seen, "首见时刻只写一次");

        db.query(render_clear("24384/23259"))
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

        db.query(render_primitive_upsert(
            "24381/38635",
            "NCYL",
            "targeted primitive 24381_38635 (NCYL) produced an invalid BREP shape; DIAM=0, HEIG=0",
        ))
        .await
        .expect("primitive upsert")
        .check()
        .expect("primitive upsert statement");
        let mut response = db
            .query(format!(
                "SELECT kind, target, noun, last_error FROM {TABLE};"
            ))
            .await
            .expect("read primitive")
            .check()
            .expect("read primitive statement");
        let rows: Vec<serde_json::Value> = response.take(0).expect("decode primitive");
        assert_eq!(rows.len(), 1, "基本体错误必须真实落库: {rows:?}");
        assert_eq!(rows[0]["kind"], PRIMITIVE);
        assert_eq!(rows[0]["target"], "24381/38635");
        assert_eq!(rows[0]["noun"], "NCYL");
        assert!(
            rows[0]["last_error"]
                .as_str()
                .is_some_and(|error| error.contains("DIAM=0") && error.contains("HEIG=0")),
            "尺寸诊断必须跟记录一起落库: {rows:?}"
        );

        db.query(render_clear_kind(PRIMITIVE, "24381/38635"))
            .await
            .expect("clear primitive")
            .check()
            .expect("clear primitive statement");
        let mut response = db
            .query(format!("SELECT target FROM {TABLE};"))
            .await
            .expect("recount primitive")
            .check()
            .expect("recount primitive statement");
        let remaining: Vec<serde_json::Value> =
            response.take(0).expect("decode primitive remaining");
        assert!(remaining.is_empty(), "基本体恢复后必须销账: {remaining:?}");
    }
}
