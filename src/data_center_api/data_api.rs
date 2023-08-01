use std::collections::HashMap;
use aios_core::data_center::DataCenterAttr;
use aios_core::pdms_types::{AttrMap, RefU64};

use glam::Vec3;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use sqlx::{Executor, Row};
use crate::api::attr::query_explicit_attr;
use crate::api::children::travel_children_with_type;
use crate::api::element::*;
use crate::aql_api::children::query_travel_children_with_type_aql;
use crate::aql_api::foreign_refnos::{query_foreign_name_aql, query_foreign_refno_aql};
use crate::aql_api::pdms_room::*;
use crate::consts::PUHUA_DQ_MATERIAL_TABLE;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;

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
pub async fn get_valv_data_from_pipe_major(refno: RefU64, mgr: &AiosDBManager) -> anyhow::Result<Vec<PipeRoomCodeData>> {
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
    let database = aios_mgr.get_arango_db().await?;
    let Some(detr) = query_foreign_refno_aql(&database, refno, &vec!["SPRE", "DETR"]).await?
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
pub fn get_thickness_pressure_level(thickness: &str, thickness_value: &str, pressure_level: &str, spre_code: &str) -> Vec<DataCenterAttr> {
    vec![]
}

/// 获取该元件的房间号，和离该元件最近的其他房间的房间号
pub(crate) async fn get_quarantine_room_name(refno: RefU64, database: &ArDatabase) -> anyhow::Result<(String, String)> {
    let room_name = query_room_name_from_refno_aql(refno, &database).await?.unwrap_or("".to_string());
    Ok((room_name, "".to_string()))
}

/// 获取元件的desc （ catr.desc）
pub(crate) async fn get_refno_desc(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<String> {
    let database = aios_mgr.get_arango_db().await?;
    let Some(catr) = query_foreign_refno_aql(&database, refno, &vec!["SPRE", "CATR"]).await?
        else { return Ok("".to_string()); };
    // let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(catr).await else { return Ok("".to_string()); };
    let attr = aios_mgr.get_attr(catr).await?;
    Ok(attr.get_str("DESC").unwrap_or("").to_string())
}

/// 获取元件在desi中的desc
pub(crate) async fn get_refno_desi_desc(refno:RefU64,aios_mgr:&AiosDBManager) -> anyhow::Result<String> {
    let attr = aios_mgr.get_attr(refno).await?;
    Ok(attr.get_str("DESC").unwrap_or("").to_string())
}

/// 获取元件的 desp
pub(crate) async fn get_refno_desp(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<f64>> {
    let attr = aios_mgr.get_attr(refno).await?;
    Ok(attr.get_f64_vec("DESP").unwrap_or(vec![]))
}

/// 获取元件的 para
pub(crate) async fn get_refno_paras(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<f64>> {
    let database = aios_mgr.get_arango_db().await?;
    let Some(catr) = query_foreign_refno_aql(&database, refno, &vec!["SPRE", "CATR"]).await?
        else { return Ok(vec![]); };
    // let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(catr).await else { return Ok(vec![]); };
    let attr = aios_mgr.get_attr(catr).await?;
    Ok(attr.get_f64_vec("PARA").unwrap_or(vec![]))
}

/// 获取电气专业的标准号
///
/// BRAN的 DESC为某个元件的NAME，然后取元件的CATREF的DESC
pub(crate) async fn get_refno_stander_num(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<String>> {
    let desc = get_refno_desc(refno, aios_mgr).await?;
    let desc = if desc.starts_with("/") { desc } else { format!("/{}", desc) };
    let Some(pool) = aios_mgr.get_project_pool(&aios_mgr.db_option.project_name) else { return Ok(None); };
    let refno = query_id_from_name(&desc, None, &pool).await?;
    if refno.is_empty() { return Ok(None); }
    let desc = get_refno_desc(refno[0], aios_mgr).await?;
    Ok(Some(desc))
}

/// 获取电气专业大宗材料信息
///
/// stander_num :  BRAN的 DESC为某个元件的NAME，然后取元件的CATREF的DESC
///
/// fileds: 需要大宗材料的哪些字段
pub(crate) async fn get_dq_material_code(spre_name: &str, stander_num: &str, fileds: &Vec<String>, aios_mgr: &AiosDBManager)
                                         -> anyhow::Result<HashMap<String, String>> {
    let spre_name_split = spre_name.split("/").collect::<Vec<_>>();
    let Some(spre_name_split_last) = spre_name_split.last() else { return Ok(HashMap::default()); };
    let sql = gen_dq_material_code_sql(spre_name_split_last, stander_num, fileds);
    let pool = aios_mgr.get_puhua_pool().await?;
    let mut conn = pool.acquire().await?;
    let query_result = conn.fetch_one(sql.as_str()).await?;
    let mut map = HashMap::new();
    for filed in fileds {
        let Ok(r) = query_result.try_get::<String, _>(filed.as_str()) else { continue; };
        map.entry(filed.to_string()).or_insert(r);
    }
    Ok(map)
}

/// 获取该节点的世界坐标下的poss和pose
pub(crate) async fn get_refno_world_poss_pose(refno: RefU64, att_type: &str, database: &ArDatabase, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<(Vec3, Vec3)>> {
    match att_type {
        "GENSEC" => {
            let points = query_travel_children_with_type_aql(&database, refno, "POINSP").await?;
            if points.len() < 2 { return Ok(None); }
            // 默认只有两个点
            let poss_point = points[0].refno;
            let pose_point = points[1].refno;
            let Ok(Some(poss)) = aios_mgr.get_world_transform(poss_point).await else { return Ok(None); };
            let Ok(Some(pose)) = aios_mgr.get_world_transform(pose_point).await else { return Ok(None); };
            Ok(Some((poss.translation, pose.translation)))
        }
        _ => {
            let Some(world_transform) = aios_mgr.get_world_transform(refno).await? else { return Ok(None); };
            let attr = aios_mgr.get_attr(refno).await?;
            let Some(poss) = attr.get_vec3("POSS") else { return Ok(None); };
            let Some(pose) = attr.get_vec3("POSE") else { return Ok(None); };
            let world_poss = world_transform.transform_point(poss);
            let world_pose = world_transform.transform_point(pose);
            Ok(Some((world_poss, world_pose)))
        }
    }
}

/// 返回pspec属性对应的中文名
pub(crate) async fn get_pspec_code(refno: RefU64, database: &ArDatabase) -> anyhow::Result<String> {
    let pspe_name = query_foreign_name_aql(refno, vec!["PSPE", "PSPE"], database).await?;
    let mut kind = "".to_string();
    if let Some(pspe_name) = pspe_name {
        match pspe_name {
            s if s.contains("Ladder") => { kind = "梯架".to_string() }
            s if s.contains("Ventilated") => { kind = "带孔托盘".to_string() }
            s if s.contains("Trough") => { kind = "实底托盘".to_string() }
            s if s.contains("Riser") => { kind = "竖梯".to_string() }
            s if s.contains("Divider") => { kind = "分隔板".to_string() }
            _ => {}
        }
    }
    Ok(kind)
}

/// 获取该节点的方位并转为pdms oria形式
pub(crate) async fn get_ori_angle_str(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<String> {
    let Ok(ori) = aios_mgr.get_implicit_attr(refno, Some(vec!["ORI"])).await else { return Ok("".to_string()); };
    let Some(ori) = ori.get_vec3("ORI") else { return Ok("0degree 0degree 0degree".to_string()); };
    Ok(format!("{}degree {}degree {}degree", ori.x, ori.y, ori.z))
}

fn gen_dq_material_code_sql(spre_name_split: &str, stander_num: &str, fileds: &Vec<String>) -> String {
    let mut sql = String::from("SELECT ");
    for filed in fileds {
        sql.push_str(&format!("{} ,", filed));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(&format!("FROM `{}` ", PUHUA_DQ_MATERIAL_TABLE));
    sql.push_str(&format!("WHERE ComponentName = '{}' AND StandardNum = '{}'", spre_name_split, stander_num));
    sql
}

/// 通过spre name 返回材料编码 命名规则为 第二个 / 到 :
///
/// 例如 "/VMB1/CPP00102:P,50" -> "CPP00102"
pub(crate) fn get_spre_material_code(spre_name:&str) -> Option<String> {
    let spre_name_split = spre_name.split("/").collect::<Vec<_>>();
    if spre_name_split.len() < 3 { return None; }
    let spre_name_last = spre_name_split[2];
    let split = spre_name_last.split(":").collect::<Vec<_>>();
    if split.len() < 2 { return None; }
    Some(split[0].to_string())
}

/// 获取该节点的当年校审版本
pub fn get_refno_latest_version() -> String {
    "A版".to_string()
}

#[tokio::test]
async fn test_get_inst_data_from_inst_major() -> anyhow::Result<()> {
    let mgr = AiosDBManager::init_form_config().await?;
    let refno = RefU64::from_refno_str("24381/103249").unwrap();
    let data = get_inst_data_from_inst_major(refno, &mgr).await?;
    dbg!(data);
    Ok(())
}

#[tokio::test]
async fn test_get_dq_material_code() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let spre_name = "/ACP1000-Trough/ACP1000-TFVL:50".to_string();
    let stander_num = "233";
    let fileds = vec!["ItemCode".to_string(), "Unit".to_string()];
    let material_map = get_dq_material_code(&spre_name,
                                            &stander_num, &fileds, &aios_mgr).await?;
    dbg!(&material_map);
    Ok(())
}