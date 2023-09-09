use aios_core::data_center::{SendHoleData, SendHoleDataToArango};
use aios_core::create_attas_structs::VirtualHoleGraphNodeQuery;
use aios_core::create_attas_structs::VirtualEmbedGraphNode;
use aios_core::create_attas_structs::VirtualHoleGraphNode;
use crate::graph_db::pdms_arango::{ArDatabase, connect_arangodb};
use aios_core::create_attas_structs::VirtualEmbedGraphNodeQuery;
use bb8_arangodb::arangors_lite::AqlQuery;

pub async fn query_virtual_hole_data(database: &ArDatabase, key_value: &str) -> anyhow::Result<Option<Vec<SendHoleData>>> {
    let aql = AqlQuery::new("with virtual_hole let v = document('virtual_hole',@_key)\
        return unset(v , '_id','_rev') ")
        .bind_var("_key", key_value)
        ;
    let data_vec: Vec<SendHoleData> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

pub async fn query_virtual_hole_audit_data_by_name(database: &ArDatabase, name: &str) -> anyhow::Result<Option<Vec<SendHoleDataToArango>>> {
    let aql = AqlQuery::new("with virtual_hole
                                                FOR u IN @@collection
                                                FILTER u.formdata.HumanCode==@name
                                                return unset(u , '_id','_rev')")
        .bind_var("@collection", "virtual_hole")
        .bind_var("name", name);
    let data_vec: Vec<SendHoleDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

pub async fn query_all_virtual_hole_audit_data(database: &ArDatabase) -> anyhow::Result<Option<Vec<SendHoleDataToArango>>> {
    let aql = AqlQuery::new("with virtual_hole
                                                FOR u IN @@collection
                                                return unset(u , '_id','_rev')")
        .bind_var("@collection", "virtual_hole");
    let data_vec: Vec<SendHoleDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}


pub async fn query_hole_detail_data_by_code(database: &ArDatabase, key: &str) -> anyhow::Result<Option<Vec<VirtualHoleGraphNodeQuery>>> {
    let aql = AqlQuery::new("with hole_data let v = document('hole_data',@_key)\
        return unset(v , '_id','_rev') ")
        .bind_var("_key", key);
    let data_vec: Vec<VirtualHoleGraphNodeQuery> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

pub async fn query_embed_detail_data_by_code(database: &ArDatabase, key: &str) -> anyhow::Result<Option<Vec<VirtualEmbedGraphNodeQuery>>> {
    let aql = AqlQuery::new("with embed_data let v = document('embed_data',@_key)\
        return unset(v , '_id','_rev') ")
        .bind_var("_key", key);
    let data_vec: Vec<VirtualEmbedGraphNodeQuery> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}