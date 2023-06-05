use aios_core::pdms_types::RefU64;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use crate::api::children::travel_children_with_type;
use crate::api::element::{query_name, query_refno_type, query_types_refnos};
use crate::aql_api::children::query_travel_children_with_type_aql;
use crate::aql_api::pdms_room::{get_room_name_split, query_room_info_from_refno};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InstPositionData {
    pub refno: String,
    pub room_code: String,
    pub elevation: Vec3,
}

/// 获取仪控专业的仪表位置数据，传入
pub async fn get_inst_data_from_inst_major(refno: RefU64, mgr: &AiosDBManager) -> anyhow::Result<Vec<InstPositionData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(result); }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arango_db().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() { return Ok(result); }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" { return Ok(result); }
    let name = query_name(refno, &pool).await?;
    if !name.contains("YK") { return Ok(result); }
    let refnos = query_travel_children_with_type_aql(&database, refno, "INST").await?;
    for refno in refnos {
        let pos = mgr.get_world_transform(refno.refno).await?;
        if pos.is_none() { continue; }
        let pos = pos.unwrap().translation;
        let name = query_room_info_from_refno(refno.refno, "FRMW", &database).await?.unwrap_or("".to_string());
        // 1516 的房间命名格式
        let mut room_code = "R101".to_string();
        let room_name = get_room_name_split(&name);
        if let Some(room_name) = room_name {
            room_code = room_name.room_name;
        }
        result.push(InstPositionData {
            refno: refno.refno.to_refno_string(),
            room_code,
            elevation: pos,
        })
    }
    Ok(result)
}


#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PipeRoomCodeData {
    pub refno: String,
    pub room_code: String,
}

/// 获取阀门所处的房间号，工艺专业
pub async fn get_valv_data_from_pipe_major(refno:RefU64,mgr:&AiosDBManager) -> anyhow::Result<Vec<PipeRoomCodeData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(result); }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arango_db().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() { return Ok(result); }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" { return Ok(result); }
    let refnos = query_travel_children_with_type_aql(&database, refno, "VALV").await?;
    for refno in refnos {
        let name = query_room_info_from_refno(refno.refno, "FRMW", &database).await?.unwrap_or("".to_string());
        // 1516 的房间命名格式
        let mut room_code = "R101".to_string();
        let room_name = get_room_name_split(&name);
        if let Some(room_name) = room_name {
            room_code = room_name.room_name;
        }
        result.push(PipeRoomCodeData{
            refno: refno.refno.to_refno_string(),
            room_code,
        })
    }
    Ok(result)
}



pub async fn get_equi_data_from_electric_major(refno:RefU64,mgr:&AiosDBManager) -> anyhow::Result<Vec<InstPositionData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(result); }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arango_db().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() { return Ok(result); }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" { return Ok(result); }
    let refnos = query_travel_children_with_type_aql(&database, refno, "EQUI").await?;
    for refno in refnos {
        let pos = mgr.get_world_transform(refno.refno).await?;
        if pos.is_none() { continue; }
        let pos = pos.unwrap().translation;
        let name = query_room_info_from_refno(refno.refno, "FRMW", &database).await?.unwrap_or("".to_string());
        // 1516 的房间命名格式
        let mut room_code = "R101".to_string();
        let room_name = get_room_name_split(&name);
        if let Some(room_name) = room_name {
            room_code = room_name.room_name;
        }
        result.push(InstPositionData {
            refno: refno.refno.to_refno_string(),
            room_code,
            elevation: pos,
        });
    }
    Ok(result)
}

#[tokio::test]
async fn test_get_inst_data_from_inst_major() -> anyhow::Result<()> {
    let mgr = AiosDBManager::init_form_config().await?;
    let refno = RefU64::from_refno_str("24381/103249").unwrap();
    let data = get_inst_data_from_inst_major(refno,&mgr).await?;
    dbg!(data);
    Ok(())
}