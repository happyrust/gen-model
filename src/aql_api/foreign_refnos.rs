use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use clap::builder::Str;
use crate::consts::{AQL_PDMS_ELES_COLLECTION, PDMS_ELEMENTS_TABLE};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::consts::AQL_FOREIGN_EDGES_COLLECTION;

///可选的去过滤查询, start_types 和 endtypes，都是外键的类型
pub async fn query_foreign_refnos_fuzzy(adb: &ArDatabase, refnos: &[RefU64], start_types: &[&[&str]], end_types: &[&str], t_types: &[&str], depth: u32) -> anyhow::Result<Vec<RefU64>> {
    let ids = refnos.into_iter().map(|x| x.format_url_name(AQL_PDMS_ELES_COLLECTION)).collect::<Vec<_>>();
    let mut aql = r#"
        with foreign_edges, pdms_eles
        for id in @ids
            let t = (for ver, edge, path in 1..15 outbound id foreign_edges
                   OPTIONS { order: "bfs"  }
                        __START_FILTER__
                   filter LENGTH(@end_types) == 0 or ((edge.foreign_type in @end_types) and (LENGTH(for c,e in 1 OUTBOUND ver._id foreign_edges
                        filter e.foreign_type in @end_types
                        return 1 ) == 0))
                   filter LENGTH(@t_types) == 0 or (ver.noun in @t_types)
                   filter ver != null
                   filter LENGTH(path.edges) <= @depth
                   return ver._key)
            return LENGTH(t) == 0 ? "0/0" : t[0]
                   "#;
    let mut start_aql = String::new();
    for i in 0..start_types.len() {
        let in_str = start_types[i].iter().map(|&x| format!(" \"{x}\" ")).collect::<Vec<_>>().join(",");
        start_aql.push_str(&format!("filter LENGTH(path.edges) > {i} and path.edges[{i}].foreign_type in [{in_str}] "));
    }
    let final_aql = aql.replace("__START_FILTER__", &start_aql);
    // dbg!(&final_aql);
    let mut aql = AqlQuery::new(&final_aql)
        .bind_var("ids", ids)
        .bind_var("depth", depth)
        .bind_var("end_types", end_types)
        .bind_var("t_types", t_types);
    let results: Vec<String> = adb.aql_query(aql).await?;
    let refnos = results.iter().map(|x| RefU64::from_url_refno_default(x)).collect::<Vec<_>>();
    Ok(refnos)
}

/// 查询某参考号的引用参考号  例如：refno -> spre -> catr -> ptre  可直接查到 ptre
// 加入可选的出发，以及可选的结束
pub async fn query_foreign_refno_aql(arango_database: &ArDatabase, refno: RefU64, foreign_types: &[&str]) -> anyhow::Result<Option<RefU64>> {
    let id = refno.format_url_name(AQL_PDMS_ELES_COLLECTION);
    if foreign_types.len() < 2 { return Ok(None); }
    let aql = AqlQuery::new("\
    With @@pdms_eles, @@foreign_edges
    for v, e, p in 1..5 outbound @id @@foreign_edges
    filter p.edges[0].foreign_type == @foreign_type_first
    filter e.foreign_type == @final_type
    filter v != null
    return v._key")
        .bind_var("id", id)
        .bind_var("foreign_type_first", foreign_types[0])
        .bind_var("final_type", foreign_types[foreign_types.len() - 1])
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@foreign_edges", AQL_FOREIGN_EDGES_COLLECTION);
    let results: Vec<String> = arango_database.aql_query(aql).await?;
    for result in results {
        if let Some(refno) = RefU64::from_url_refno(&result) {
            return Ok(Some(refno));
        }
    }
    Ok(None)
}

/// 查询外键对应的 name
pub async fn query_foreign_name_aql(refno: RefU64, foreign_types: Vec<&str>, arango_database: &ArDatabase) -> anyhow::Result<Option<String>> {
    let id = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno());
    if foreign_types.len() <= 1 { return Ok(None); }
    let aql = AqlQuery::new("\
    WITH @@pdms_eles,@@foreign_edges
    let foreign_key = (for v, e, p in 1..5 outbound @id @@foreign_edges
                           filter p.edges[0].foreign_type == @foreign_type_first
                           filter e.foreign_type == @final_type
                           filter v != null
                           return v._key )
    return document(@@pdms_eles,foreign_key[0]).name")
        .bind_var("id", id)
        .bind_var("foreign_type_first", foreign_types[0])
        .bind_var("final_type", foreign_types[foreign_types.len() - 1])
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@foreign_edges", AQL_FOREIGN_EDGES_COLLECTION);
    let results = arango_database.aql_query::<String>(aql).await;
    if results.is_err() {
        return Ok(None);
    }
    let mut results = results.unwrap();
    if results.len() == 0 { return Ok(None); }
    Ok(Some(results.remove(0)))
}