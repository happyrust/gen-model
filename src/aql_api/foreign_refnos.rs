use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use arangors_lite::{AqlQuery, Database};
use crate::data_interface::tidb_manager::AiosDBManager;

//可选的去过滤查询, start_types 和 endtypes，都是外键的类型
pub async fn query_foreign_refno_fuzzy(adb: &Database, refno: RefU64, start_types: &[&[&str]], end_types: &[&str], t_types: &[&str]) -> anyhow::Result<Option<RefU64>> {
    let id = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let mut aql = "\
        for ver, edge, path in 1..15 outbound @id foreign_edges
               FILTER LENGTH(@t_types) == 0 and length(for c in 1 INBOUND ver._id foreign_edges
                    return 0 )
                    __START_FILTER__
               filter LENGTH(@end_types) == 0 or (edge.foreign_type in @end_types)
               filter LENGTH(@t_types) == 0 or (ver.noun in @t_types)
        filter ver != null
        return ver._key";
    let mut start_aql = String::new();
    for i in 0..start_types.len() {
        let in_str = start_types[i].iter().map(|&x| format!(" \"{x}\" ")).collect::<Vec<_>>().join(",");
        // dbg!(&in_str);
       start_aql.push_str(&format!("filter LENGTH(path.edges) > {i} and path.edges[{i}].foreign_type in [{in_str}] "));
    }
    let final_aql = aql.replace("__START_FILTER__", &start_aql);
    // dbg!(&final_aql);
    let mut aql = AqlQuery::new(&final_aql)
        .bind_var("id", id)
        .bind_var("end_types", end_types)
        .bind_var("t_types", t_types)
        ;
    // for i in 0..start_types.len() {
    //     aql = aql.bind_var(&format!("start_types{i}"), start_types[i].clone());
    // }
    let results: Vec<String> = adb.aql_query(aql).await?;
    for result in results {
        if let Some(refno) = RefU64::from_url_refno(&result) {
            return Ok(Some(refno));
        }
    }
    Ok(None)
}

/// 查询某参考号的引用参考号  例如：refno -> spre -> catr -> ptre  可直接查到 ptre
// 加入可选的出发，以及可选的结束
pub async fn query_foreign_refno_aql(refno: RefU64, foreign_types: &[&str], arango_database: &Database) -> anyhow::Result<Option<RefU64>> {
    let id = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    if foreign_types.len() < 2 { return Ok(None); }
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