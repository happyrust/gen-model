//! Name-first addressing for the live E2E targets.
//!
//! Acceptance is written in names because refnos churn: elements are created
//! and deleted all the time, so a hard-coded `=24381/100819` only ever pins one
//! snapshot of the project. The refno a name resolves to is still what every
//! downstream watermark / edge / AABB / topology assertion tracks on — the
//! change is in how a target is *addressed*, not how it is followed.
//!
//! Callers must have connected `SUL_DB` to the live store first.

#![allow(dead_code)]

use aios_core::SUL_DB;

/// Refno of the single element carrying `name`, scoped to `dbnum` when given.
///
/// 名字在 PDMS 里只保证**库内**唯一。不限定 dbnum 时，跨库同名（`/Copy-of-*`
/// 这一类尤其容易撞）会让下面的唯一性检查判成「歧义」当场 panic，看上去像靶子
/// 漂移，其实只是查询没划范围。每个探针本来就钉死在一个库上，把它说出来即可。
pub async fn by_name(name: &str, dbnum: Option<u32>) -> String {
    let hits = query_refnos(
        &format!(
            "SELECT VALUE record::id(id) FROM pe \
             WHERE name = $name AND deleted = false{}",
            dbnum_clause(dbnum)
        ),
        vec![("name", name.to_string())],
    )
    .await;
    exactly_one(hits, &format!("element named {name}{}", scope(dbnum)))
}

/// Refno of the single `noun` child of the element named `parent`.
///
/// Geometry primitives (CONE / PANE / CAP …) carry no name of their own in
/// PDMS, so their named owner plus the noun is the closest stable handle they
/// have. This deliberately fails rather than guessing when the pair stops being
/// unique — a second CAP appearing under the same BRAN is exactly the drift
/// that would otherwise make the test silently assert against the wrong body.
pub async fn child_of(parent: &str, noun: &str, dbnum: Option<u32>) -> String {
    let owner = by_name(parent, dbnum).await;
    let hits = query_refnos(
        &format!(
            "SELECT VALUE record::id(id) FROM pe \
             WHERE owner = type::thing('pe', $owner) AND noun = $noun AND deleted = false{}",
            dbnum_clause(dbnum)
        ),
        vec![("owner", owner), ("noun", noun.to_string())],
    )
    .await;
    exactly_one(hits, &format!("{noun} under {parent}{}", scope(dbnum)))
}

/// `dbnum` 是调用方写死的常量，直接内联进 SQL；这里够不到外部输入，没有注入面。
fn dbnum_clause(dbnum: Option<u32>) -> String {
    dbnum
        .map(|dbnum| format!(" AND dbnum = {dbnum}"))
        .unwrap_or_default()
}

fn scope(dbnum: Option<u32>) -> String {
    dbnum
        .map(|dbnum| format!("（dbnum={dbnum}）"))
        .unwrap_or_default()
}

async fn query_refnos(sql: &str, binds: Vec<(&'static str, String)>) -> Vec<String> {
    let mut query = SUL_DB.query(sql);
    for bind in binds {
        query = query.bind(bind);
    }
    let mut response = query
        .await
        .expect("resolve target by name")
        .check()
        .expect("valid target lookup");
    response.take(0).expect("decode target refnos")
}

fn exactly_one(hits: Vec<String>, what: &str) -> String {
    match hits.len() {
        1 => hits.into_iter().next().expect("checked length"),
        0 => panic!("no {what} in the live store — the target moved or was renamed"),
        _ => panic!(
            "{what} is ambiguous, matched {} elements: {hits:?}",
            hits.len()
        ),
    }
}
