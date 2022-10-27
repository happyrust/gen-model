use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use arangors_lite::{AqlQuery, Database};
use crate::data_interface::tidb_manager::AiosDBManager;

/// 查询某参考号的引用参考号  例如：refno -> spre -> catr -> ptre  可直接查到 ptre
pub async fn query_foreign_refno_aql(refno: RefU64, foreign_types: Vec<&str>, arango_database: &Database) -> anyhow::Result<Option<RefU64>> {
    let id = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    if foreign_types.len() <= 1 { return Ok(None); }
    let aql = AqlQuery::new("\
    for v, e, p in 1..5 outbound @id foreign_edges
    filter p.edges[0].foreign_type == @foreign_type_first
    filter e.foreign_type == @final_type
    filter v != null
    return v._key")
        .bind_var("id", id)
        .bind_var("foreign_type_first", foreign_types[0])
        .bind_var("final_type", foreign_types[foreign_types.len() - 1]);
    let results: Vec<String> = arango_database.aql_query(aql).await?;
    for result in results {
        if let Some(refno) = RefU64::from_url_refno(&result) {
            return Ok(Some(refno));
        }
    }
    Ok(None)
}

/// 查询外键对应的 name
pub async fn query_foreign_name_aql(refno:RefU64,foreign_types:Vec<&str>,arango_database:&Database) -> anyhow::Result<Option<String>> {
    let id = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    if foreign_types.len() <= 1 { return Ok(None); }
    let aql = AqlQuery::new("\
    let foreign_key = (for v, e, p in 1..5 outbound @id foreign_edges
                           filter p.edges[0].foreign_type == @foreign_type_first
                           filter e.foreign_type == @final_type
                           filter v != null
                           return v._key )
    return document('pdms_eles',foreign_key[0]).name")
        .bind_var("id", id)
        .bind_var("foreign_type_first", foreign_types[0])
        .bind_var("final_type", foreign_types[foreign_types.len() - 1]);
    let results: Result<Vec<String>, _> = arango_database.aql_query(aql).await;
    if results.is_err() { return Ok(None); }
    let mut results = results.unwrap();
    if results.len() == 0 { return Ok(None); }
    Ok(Some(results.remove(0)))
}