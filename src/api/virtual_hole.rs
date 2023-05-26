use aios_core::data_center::{SendHoleData, SendHoleDataToArango};
use arangors_lite::{AqlQuery, Database};

// pub async fn query_virtual_hole_data(database: &Database, key_value: &str) -> anyhow::Result<Option<Vec<SendHoleData>>> {
//     let aql = AqlQuery::new("let v = document('virtual_hole',@_key)\
//         return unset(v , '_id','_rev') ")
//         .bind_var("_key", key_value);
//     let data_vec:Vec<SendHoleData> = database.aql_query(aql).await?;
//     return Ok(Some((data_vec)));
// }
pub async fn query_virtual_hole_audit_data_by_name(database: &Database, name: &str) -> anyhow::Result<Option<Vec<SendHoleDataToArango>>> {
    let aql = AqlQuery::new("FOR u IN @@collection
                                                FILTER u.HumanCode==@name
                                                return unset(u , '_id','_rev')")
        .bind_var("@collection", "virtual_hole")
        .bind_var("name", name);
    let data_vec: Vec<SendHoleDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}