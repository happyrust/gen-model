use aios_core::data_center::SendHoleData;
use arangors_lite::{AqlQuery, Database};

pub async fn query_virtual_hole_data(database: &Database, key_value: &str) -> anyhow::Result<Option<Vec<SendHoleData>>> {
    let aql = AqlQuery::new("let v = document('virtual_hole',@_key)\
        return unset(v , '_id','_rev') ")
        .bind_var("_key", key_value);
    let data_vec:Vec<SendHoleData> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}
