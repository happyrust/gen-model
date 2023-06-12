use aios_core::data_center::{SendHoleData, SendHoleDataToArango};
use aios_core::create_attas_structs::VirtualHoleGraphNode;
use aios_core::create_attas_structs::VirtualEmbedGraphNode;
use arangors::AqlQuery;
use crate::graph_db::pdms_arango::{ArDatabase, connect_arangodb};


pub async fn query_virtual_hole_data(database: &ArDatabase, key_value: &str) -> anyhow::Result<Option<Vec<SendHoleData>>> {
    let aql = AqlQuery::builder().query("let v = document('virtual_hole',@_key)\
        return unset(v , '_id','_rev') ")
        .bind_var("_key", key_value)
        .build();
    let data_vec: Vec<SendHoleData> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

pub async fn query_virtual_hole_audit_data_by_name(database: &ArDatabase, name: &str) -> anyhow::Result<Option<Vec<SendHoleDataToArango>>> {
    let aql = AqlQuery::builder().query("FOR u IN @@collection
                                                FILTER u.formdata.HumanCode==@name
                                                return unset(u , '_id','_rev')")
        .bind_var("@collection", "virtual_hole")
        .bind_var("name", name).build();
    let data_vec: Vec<SendHoleDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

pub async fn query_hole_detail_data_by_code(database: &ArDatabase, code: &str) -> anyhow::Result<Option<Vec<VirtualHoleGraphNode>>> {
    let aql = AqlQuery::builder().query("FOR u IN @@collection
                                                FILTER u.ItemREF==@code
                                                return unset(u , '_id','_rev')")
        .bind_var("@collection", "hole_data")
        .bind_var("code", code).build();
    let data_vec: Vec<VirtualHoleGraphNode> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

pub async fn query_embed_detail_data_by_code(database: &ArDatabase, code: &str) -> anyhow::Result<Option<Vec<VirtualEmbedGraphNode>>> {
    let aql = AqlQuery::builder().query("FOR u IN @@collection
                                                FILTER u.REF==@code
                                                return unset(u , '_id','_rev')")
        .bind_var("@collection", "embed_data")
        .bind_var("code", code).build();
    let data_vec: Vec<VirtualEmbedGraphNode> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}