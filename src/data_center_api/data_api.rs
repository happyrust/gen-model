use aios_core::data_center::DataCenterAttr;
use aios_core::pdms_types::{AttrMap, RefU64};
use arangors_lite::Database;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use crate::api::attr::query_explicit_attr;
use crate::api::children::travel_children_with_type;
use crate::api::element::{query_name, query_refno_type, query_types_refnos};
use crate::aql_api::children::query_travel_children_with_type_aql;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::pdms_room::{get_room_name_split, query_room_info_from_refno, query_room_name_from_refno_aql};
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
    let database = mgr.get_arangodb().await?;
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
pub async fn get_valv_data_from_pipe_major(refno: RefU64, mgr: &AiosDBManager) -> anyhow::Result<Vec<PipeRoomCodeData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(result); }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arangodb().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() { return Ok(result); }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" { return Ok(result); }
    let refnos = query_travel_children_with_type_aql(database, refno, "VALV").await?;
    for refno in refnos {
        let name = query_room_info_from_refno(refno.refno, "FRMW", database).await?.unwrap_or("".to_string());
        // 1516 的房间命名格式
        let mut room_code = "R101".to_string();
        let room_name = get_room_name_split(&name);
        if let Some(room_name) = room_name {
            room_code = room_name.room_name;
        }
        result.push(PipeRoomCodeData {
            refno: refno.refno.to_refno_string(),
            room_code,
        })
    }
    Ok(result)
}


pub async fn get_equi_data_from_electric_major(refno: RefU64, mgr: &AiosDBManager) -> anyhow::Result<Vec<InstPositionData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(result); }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arangodb().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() { return Ok(result); }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" { return Ok(result); }
    let refnos = query_travel_children_with_type_aql(database, refno, "EQUI").await?;
    for refno in refnos {
        let pos = mgr.get_world_transform(refno.refno).await?;
        if pos.is_none() { continue; }
        let pos = pos.unwrap().translation;
        let name = query_room_info_from_refno(refno.refno, "FRMW", database).await?.unwrap_or("".to_string());
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

/// 获取保温层厚度.例如 I90-HL 返回 I90 其他命名规则暂时不返回
pub async fn get_ispec_from_attr(attr: &AttrMap, aios_mgr: &AiosDBManager) -> anyhow::Result<String> {
    let Some(ispec_refno) = attr.get_refu64("ISPE") else { return Ok("".to_string()); };
    let Ok(ispec_name) = aios_mgr.get_name(ispec_refno).await else { return Ok("".to_string()); };
    if ispec_name.contains("HL") {
        let ispec_name_split = ispec_name.split("-").collect::<Vec<_>>();
        return Ok(ispec_name_split[0].to_string());
    }
    Ok("".to_string())
}

/// 获取非标编码 通过 rtext 是否包含 E4001, 4004, 4006，若是则返回 E4001 4004 4006
pub async fn get_rtext_from_attr(attr: &AttrMap, aios_mgr: &AiosDBManager) -> anyhow::Result<String> {
    let Some(refno) = attr.get_refno() else { return Ok("".to_string()); };
    let database = aios_mgr.get_arangodb().await?;
    let Some(detr) = query_foreign_refno_aql(refno, &vec!["SPRE", "DETR"], database).await?
        else { return Ok("".to_string()); };
    let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(detr).await else { return Ok("".to_string()); };
    let detr_map = query_explicit_attr(detr, &pool).await?;
    let rtext = detr_map.get_str("RTEX");
    if let Some(rtext) = rtext {
        let codes = vec!["E4001", "E4004", "E4006"];
        for code in codes {
            if rtext.contains(code) {
                return Ok(rtext.to_string());
            }
        }
    }
    Ok("".to_string())
}

/// 获取 壁厚 / 壁厚值 / 压力等级 在大宗材料中的数据，都是 Pressure 字段 ， SCH开头为壁厚 ， 纯数字为壁厚值，CL / PN 开头为压力等级
///
/// thickness , thickness_value , pressure_level 为 壁厚,壁厚值,压力等级 在元数据表中对应的属性编码,
/// spre_code: 元件编码
pub fn get_thickness_pressure_level(thickness:&str,thickness_value:&str,pressure_level:&str,spre_code:&str) -> Vec<DataCenterAttr> {
    vec![]
}

/// 获取该元件的房间号，和离该元件最近的其他房间的房间号
pub(crate) async fn get_quarantine_room_name(refno:RefU64,database:&Database) -> anyhow::Result<(String,String)> {
    let room_name = query_room_name_from_refno_aql(refno, database).await?.unwrap_or("".to_string());
    Ok((room_name,"".to_string()))
}

/// 获取元件的desc （ catr.desc）
pub(crate) async fn get_refno_desc(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<String> {
    let database = aios_mgr.get_arangodb().await?;
    let Some(catr) = query_foreign_refno_aql(refno, &vec!["SPRE", "CATR"], database).await?
        else { return Ok("".to_string()); };
    let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(catr).await else { return Ok("".to_string()); };
    let attr = aios_mgr.get_attr(refno).await?;
    Ok(attr.get_str("DESC").unwrap_or("").to_string())
}

/// 获取元件的 desp
pub(crate) async fn get_refno_desp(refno:RefU64,aios_mgr:&AiosDBManager) -> anyhow::Result<Vec<f64>> {
    let attr = aios_mgr.get_attr(refno).await?;
    Ok(attr.get_f64_vec("DESP").unwrap_or(vec![]))
}

/// 获取元件的 para
pub(crate) async fn get_refno_paras(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<f64>> {
    let database = aios_mgr.get_arangodb().await?;
    let Some(catr) = query_foreign_refno_aql(refno, &vec!["SPRE", "CATR"], database).await?
        else { return Ok(vec![]); };
    let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(catr).await else { return Ok(vec![]); };
    let attr = aios_mgr.get_attr(refno).await?;
    Ok(attr.get_f64_vec("PARA").unwrap_or(vec![]))
}

#[tokio::test]
async fn test_get_inst_data_from_inst_major() -> anyhow::Result<()> {
    let mgr = AiosDBManager::init_form_config().await?;
    let refno = RefU64::from_refno_str("24381/103249").unwrap();
    let data = get_inst_data_from_inst_major(refno, &mgr).await?;
    dbg!(data);
    Ok(())
}