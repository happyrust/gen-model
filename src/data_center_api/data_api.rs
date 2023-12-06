use aios_core::data_center::AttrValue::{AttrFloat, AttrString, AttrVec3};
use aios_core::data_center::DataCenterAttr;
use aios_core::pdms_types::*;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use dashmap::DashMap;
use std::collections::HashMap;
use aios_core::{AttrMap, AttrVal, NamedAttrValue};
use aios_core::pdms_user::RefnoMajor;

use crate::api::attr::query_explicit_attr;
use crate::api::children::travel_children_with_type;
use crate::api::element::*;
use crate::aql_api::children::{query_refnos_belong_major, query_travel_children_with_type_aql};
use crate::aql_api::foreign_refnos::{query_foreign_name_aql, query_foreign_refno_aql};
use crate::aql_api::pdms_room::*;
use crate::consts::PUHUA_DQ_MATERIAL_TABLE;
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use glam::Vec3;
use regex::Regex;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use sqlx::{Executor, Row};
use crate::aql_api::attr_map::query_refnos_point_map_aql;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InstPositionData {
    pub refno: String,
    pub room_code: String,
    pub elevation: Vec3,
}

/// 获取仪控专业的仪表位置数据，传入
pub async fn get_inst_data_from_inst_major(
    refno: RefU64,
    mgr: &AiosDBManager,
) -> anyhow::Result<Vec<InstPositionData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() {
        return Ok(result);
    }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arango_db().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() {
        return Ok(result);
    }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" {
        return Ok(result);
    }
    let name = query_name(refno, &pool).await?;
    if !name.contains("YK") {
        return Ok(result);
    }
    let refnos = query_travel_children_with_type_aql(&database, refno, "INST").await?;
    for ele in refnos {
        let pos = mgr.get_world_transform(ele.refno).await?;
        if pos.is_none() {
            continue;
        }
        let pos = pos.unwrap().translation;
        let name = mgr
            .query_room_names_of_ele(ele.refno)
            .await?
            .into_iter()
            .next()
            .unwrap_or_default();

        // 1516 的房间命名格式
        let mut room_code = "R101".to_string();
        let room_name = get_room_name_split(&name);
        if let Some(room_name) = room_name {
            room_code = room_name.room_name;
        }
        result.push(InstPositionData {
            refno: ele.refno.to_string(),
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

/// 获取工艺管件 itema等重复的数据
pub(crate) async fn get_bran_itema_attr(
    refno: PdmsElement,
    bran_name: &str,
    room_code: String,
    database: &ArDatabase,
    aios_mgr: &AiosDBManager,
    mut result: &mut Vec<DataCenterAttr>,
) {
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEM1".to_string(),
        value: AttrString(refno.refno.to_string()).into(),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMA1".to_string(),
        value: AttrString(refno.name).into(),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMA2".to_string(),
        value: AttrString(refno.noun).into(),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMA3".to_string(),
        value: AttrString(bran_name.to_string()).into(),
    };
    result.push(item_4);
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMA4".to_string(),
        value: AttrString("".to_string()).into(),
    };
    result.push(item_5);
    let world_position = aios_mgr
        .get_world_transform(refno.refno).await
        .unwrap_or(None)
        .unwrap_or_default();
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMA5".to_string(),
        value: AttrVec3(world_position.translation).into(),
    };
    result.push(item_5);
    let item_8 = DataCenterAttr {
        attribute_model_code: "ITEMA8".to_string(),
        value: AttrString(quat_to_pdms_ori_str(&world_position.rotation)).into(),
    };
    result.push(item_8);
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA20".to_string(),
        value: AttrString(room_code).into(),
    });
    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();
    let ispec = get_ispec_from_attr(&attr, &aios_mgr)
        .await
        .unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA21".to_string(),
        value: AttrString(ispec).into(),
    });
    let tspe = query_foreign_name_aql(refno.refno, vec!["TSPE", "TSPE"], database)
        .await
        .unwrap_or(None)
        .unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA22".to_string(),
        value: AttrString(tspe).into(),
    });
    let r_text = get_rtext_from_attr(&attr, aios_mgr)
        .await
        .unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA24".to_string(),
        value: AttrString(r_text).into(),
    });
}

/// 获取大宗材料表单壁厚值等数据
///
/// wall_thickness_number_code : 壁厚/管表号系列 属性编码
///
/// wall_thickness_value 壁厚值 属性编码
///
/// pressure_level 压力等级 属性编码
pub(crate) async fn get_material_pressure_code(
    wall_thickness_number_code: &str,
    wall_thickness_value: &str,
    pressure_level: &str,
    mut result: &mut Vec<DataCenterAttr>,
    material_map: &DashMap<String, String>,
) {
    if let Some(pressure) = material_map.get("Pressure") {
        if pressure.to_lowercase().starts_with("sch") {
            result.push(DataCenterAttr {
                attribute_model_code: wall_thickness_number_code.to_string(),
                value: AttrString(pressure.value().clone()).into(),
            });
        } else if pressure.starts_with("cl") || pressure.starts_with("pn") {
            result.push(DataCenterAttr {
                attribute_model_code: wall_thickness_value.to_string(),
                value: AttrString(pressure.value().clone()).into(),
            });
        } else {
            result.push(DataCenterAttr {
                attribute_model_code: pressure_level.to_string(),
                value: AttrString(pressure.value().clone()).into(),
            })
        }
    }
}

/// 获取阀门所处的房间号，工艺专业
pub async fn get_valv_data_from_pipe_major(
    refno: RefU64,
    mgr: &AiosDBManager,
) -> anyhow::Result<Vec<PipeRoomCodeData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() {
        return Ok(result);
    }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arango_db().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() {
        return Ok(result);
    }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" {
        return Ok(result);
    }
    let refnos = query_travel_children_with_type_aql(&database, refno, "VALV").await?;
    for ele in refnos {
        let name = mgr
            .query_room_names_of_ele(ele.refno)
            .await?
            .into_iter()
            .next()
            .unwrap_or_default();
        // 1516 的房间命名格式
        let mut room_code = "R101".to_string();
        let room_name = get_room_name_split(&name);
        if let Some(room_name) = room_name {
            room_code = room_name.room_name;
        }
        result.push(PipeRoomCodeData {
            refno: ele.refno.to_string(),
            room_code,
        })
    }
    Ok(result)
}

pub async fn get_equi_data_from_electric_major(
    refno: RefU64,
    mgr: &AiosDBManager,
) -> anyhow::Result<Vec<InstPositionData>> {
    let mut result = vec![];
    let pool = mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() {
        return Ok(result);
    }
    let (_, pool) = pool.unwrap();
    let database = mgr.get_arango_db().await?;
    let refno_cache = mgr.get_refno_basic(refno);
    if refno_cache.is_none() {
        return Ok(result);
    }
    let refno_cache = refno_cache.unwrap();
    let table_name = &refno_cache.table;
    if table_name.to_lowercase() != "zone" {
        return Ok(result);
    }
    let refnos = query_travel_children_with_type_aql(&database, refno, "EQUI").await?;
    for ele in refnos {
        let pos = mgr.get_world_transform(ele.refno).await?;
        if pos.is_none() {
            continue;
        }
        let pos = pos.unwrap().translation;
        let name = mgr
            .query_room_names_of_ele(ele.refno)
            .await?
            .into_iter()
            .next()
            .unwrap_or_default();

        // 1516 的房间命名格式
        let mut room_code = "R101".to_string();
        let room_name = get_room_name_split(&name);
        if let Some(room_name) = room_name {
            room_code = room_name.room_name;
        }
        result.push(InstPositionData {
            refno: ele.refno.to_string(),
            room_code,
            elevation: pos,
        });
    }
    Ok(result)
}

/// 获取保温层厚度.例如 I90-HL 返回 I90 其他命名规则暂时不返回
pub async fn get_ispec_from_attr(
    attr: &AttrMap,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<String> {
    let Some(ispec_refno) = attr.get_refu64("ISPE") else {
        return Ok("".to_string());
    };
    let Ok(ispec_name) = aios_mgr.get_name(ispec_refno).await else {
        return Ok("".to_string());
    };
    if ispec_name.contains("HL") {
        let ispec_name_split = ispec_name.split("-").collect::<Vec<_>>();
        return Ok(ispec_name_split[0].to_string());
    }
    Ok("".to_string())
}

/// 获取非标编码 通过 rtext 是否包含 E4001, 4004, 4006，若是则返回 E4001 4004 4006
pub async fn get_rtext_from_attr(
    attr: &AttrMap,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<String> {
    let Some(refno) = attr.get_refno() else {
        return Ok("".to_string());
    };
    let database = aios_mgr.get_arango_db().await?;
    let Some(detr) = query_foreign_refno_aql(&database, refno, &vec!["SPRE", "DETR"]).await? else {
        return Ok("".to_string());
    };
    let detr_map = aios_mgr.get_attr(detr).await?;
    let rtext = detr_map.get_str("RTEX");
    if let Some(rtext) = rtext {
        let codes = vec!["E4001", "E4004", "E4006"];
        for code in codes {
            if rtext.contains(code) {
                return Ok(code.to_string());
            }
        }
    }
    Ok("".to_string())
}

/// 获取 壁厚 / 壁厚值 / 压力等级 在大宗材料中的数据，都是 Pressure 字段 ， SCH开头为壁厚 ， 纯数字为壁厚值，CL / PN 开头为压力等级
///
/// thickness , thickness_value , pressure_level 为 壁厚,壁厚值,压力等级 在元数据表中对应的属性编码,
/// spre_code: 元件编码
pub fn get_thickness_pressure_level(
    thickness: &str,
    thickness_value: &str,
    pressure_level: &str,
    pressure_code: &str,
) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let mut thickness_code = "".to_string();
    let mut thickness_value_code = "".to_string();
    let mut pressure_level_code = "".to_string();

    if pressure_code.clone().to_lowercase().starts_with("sch") {
        thickness_code = pressure_code.to_string();
    } else if pressure_code.ends_with("mm") {
        thickness_value_code = pressure_code.to_string();
    } else if pressure_code.starts_with("CL") || pressure_code.starts_with("PN") {
        pressure_level_code = pressure_code.to_string();
    }

    result.push(DataCenterAttr {
        attribute_model_code: thickness.to_string(),
        value: thickness_code.to_string(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: thickness_value.to_string(),
        value: thickness_value_code.to_string(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: pressure_level.to_string(),
        value: pressure_level_code.to_string(),
    });
    result
}

/// 获取该元件的房间号，和离该元件最近的其他房间的房间号
pub(crate) async fn get_quarantine_room_name(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<(String, String)> {
    let room_name = query_room_name_from_refno_aql(refno, &database)
        .await?
        .unwrap_or("".to_string());
    Ok((room_name, "".to_string()))
}

/// 获取元件的desc （ catr.desc）
pub(crate) async fn get_refno_desc(
    refno: RefU64,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<String> {
    let attr = aios_mgr.get_cat_attmap(refno).unwrap_or_default();
    Ok(attr.get_str("DESC").unwrap_or("").to_string())
}

/// 获取元件在desi中的desc
pub(crate) async fn get_refno_desi_desc(
    refno: RefU64,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<String> {
    let attr = aios_mgr.get_attr(refno).await?;
    Ok(attr.get_str("DESC").unwrap_or("").to_string())
}

/// 获取元件的 desp
pub(crate) async fn get_refno_desp(
    refno: RefU64,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<Vec<f64>> {
    let attr = aios_mgr.get_attr(refno).await?;
    Ok(attr.get_f32_vec("DESP").unwrap_or(vec![]))
}

/// 获取元件的 para
pub(crate) fn get_refno_paras(
    refno: RefU64,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<Vec<f64>> {
    // let database = aios_mgr.get_arango_db().await?;
    // let Some(catr) = query_foreign_refno_aql(&database, refno, &vec!["SPRE", "CATR"]).await? else {
    //     return Ok(vec![]);
    // };
    // let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(catr).await else { return Ok(vec![]); };
    let Some(attr) = aios_mgr.get_cat_attmap(refno) else { return Ok(vec![]); };
    Ok(attr.get_f32_vec("PARA").unwrap_or(vec![]))
}

/// 获取电气专业的标准号
///
/// BRAN的 DESC为某个元件的NAME，然后取元件的CATREF的DESC
pub(crate) async fn get_refno_stander_num(
    refno: RefU64,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<Option<String>> {
    let desc = get_refno_desc(refno, aios_mgr).await?;
    let desc = if desc.starts_with("/") {
        desc
    } else {
        format!("/{}", desc)
    };
    let Some(pool) = aios_mgr.get_project_pool(&aios_mgr.db_option.project_name) else {
        return Ok(None);
    };
    let refno = query_id_from_name(&desc, None, &pool).await?;
    if refno.is_empty() {
        return Ok(None);
    }
    let desc = get_refno_desc(refno[0], aios_mgr).await?;
    Ok(Some(desc))
}

/// 获取电气专业大宗材料信息
///
/// stander_num :  BRAN的 DESC为某个元件的NAME，然后取元件的CATREF的DESC
///
/// fileds: 需要大宗材料的哪些字段
pub(crate) async fn get_dq_material_code(
    spre_name: &str,
    stander_num: &str,
    fileds: &Vec<String>,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<HashMap<String, String>> {
    let spre_name_split = spre_name.split("/").collect::<Vec<_>>();
    let Some(spre_name_split_last) = spre_name_split.last() else {
        return Ok(HashMap::default());
    };
    let sql = gen_dq_material_code_sql(spre_name_split_last, stander_num, fileds);
    let pool = aios_mgr.get_puhua_pool().await?;
    let mut conn = pool;
    let query_result = conn.fetch_one(sql.as_str()).await?;
    let mut map = HashMap::new();
    for filed in fileds {
        let Ok(r) = query_result.try_get::<String, _>(filed.as_str()) else {
            continue;
        };
        map.entry(filed.to_string()).or_insert(r);
    }
    Ok(map)
}

/// 获取该节点的世界坐标下的poss和pose, bran 返回 hpos , tpos 的世界坐标
pub async fn get_refno_world_poss_pose(
    refno: RefU64,
    att_type: &str,
    database: &ArDatabase,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<Option<(Vec3, Vec3)>> {
    match att_type {
        "GENSEC" => {
            let points = query_travel_children_with_type_aql(&database, refno, "POINSP").await?;
            if points.len() < 2 {
                return Ok(None);
            }
            // 默认只有两个点
            let poss_point = points[0].refno;
            let pose_point = points[1].refno;
            let Ok(Some(poss)) = aios_mgr.get_world_transform(poss_point).await else {
                return Ok(None);
            };
            let Ok(Some(pose)) = aios_mgr.get_world_transform(pose_point).await else {
                return Ok(None);
            };
            Ok(Some((poss.translation, pose.translation)))
        }
        "BRAN" => {
            let bran_attr = aios_mgr.get_attr(refno).await?;
            let Ok(Some(branch_transform)) = aios_mgr.get_world_transform(refno).await else {
                return Ok(None);
            };
            let hpos_wrt = branch_transform.transform_point(bran_attr.get_vec3("HPOS").unwrap());
            let tpos_wrt = branch_transform.transform_point(bran_attr.get_vec3("TPOS").unwrap());
            Ok(Some((hpos_wrt, tpos_wrt)))
        }
        _ => {
            let Some(world_transform) = aios_mgr.get_world_transform(refno).await? else {
                return Ok(None);
            };
            let attr = aios_mgr.get_attr(refno).await?;
            let Some(poss) = attr.get_vec3("POSS") else {
                return Ok(None);
            };
            let Some(pose) = attr.get_vec3("POSE") else {
                return Ok(None);
            };
            let world_poss = world_transform.transform_point(poss);
            let world_pose = world_transform.transform_point(pose);
            Ok(Some((world_poss, world_pose)))
        }
    }
}

/// 获取多个参考号的 arrive leave 点的信息或世界坐标
///
/// b_request_w_pos 是否请求世界坐标
pub async fn get_refnos_arrive_leave_info(refnos: Vec<RefU64>, b_request_w_pos: bool, aios_mgr: &AiosDBManager) -> anyhow::Result<HashMap<RefU64, HashMap<String, NamedAttrValue>>> {
    let database = aios_mgr.get_arango_db().await?;
    let points = query_refnos_point_map_aql(refnos, &database).await?;
    let mut map = HashMap::new();
    for point in points {
        // 找到arrive 和 leave 对应的点集信息
        let Ok(attr) = aios_mgr.get_attr(point.refno).await else { continue; };
        let Some(AttrVal::IntegerType(arrive)) = attr.get_val("ARRI") else { continue; };
        let Some(AttrVal::IntegerType(leave)) = attr.get_val("LEAV") else { continue; };
        let Some(arrive_point) = point.ptset_map.get(arrive) else { continue; };
        let Some(leave_point) = point.ptset_map.get(leave) else { continue; };
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("ARRIVE_POINT".to_string()).or_insert(NamedAttrValue::Vec3Type(arrive_point.pt));
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("LEAVE_POINT".to_string()).or_insert(NamedAttrValue::Vec3Type(leave_point.pt));
        // 查询世界坐标
        if b_request_w_pos {
            let w_pos = aios_mgr.get_world_transform(point.refno).await?.unwrap_or_default();
            // 根据世界坐标变换
            let arrive_transform = w_pos.transform_point(arrive_point.pt);
            let leave_transform = w_pos.transform_point(leave_point.pt);
            // 放入结果
            map.entry(point.refno).or_insert_with(HashMap::new)
                .entry("ARRIVE_W_POS".to_string()).or_insert(NamedAttrValue::F32VecType(
                vec![arrive_transform.x, arrive_transform.y, arrive_transform.z]
            ));

            map.entry(point.refno).or_insert_with(HashMap::new)
                .entry("LEAVE_W_POS".to_string()).or_insert(NamedAttrValue::F32VecType(
                vec![leave_transform.x, leave_transform.y, leave_transform.z]
            ));
        }
        // 如果在 HashMap 中找不到指定的 `point.refno` 键，则插入一个新的 HashMap，并返回对它的可变引用。
        // 如果已存在，则返回对现有 HashMap 的可变引用。
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("ARRIVE_PHEIGTH".to_string()).or_insert(NamedAttrValue::F32Type(arrive_point.pheight));
        // 在上述获取的 HashMap 中，如果找不到 "ARRIVE_PHEIGTH" 键，则插入一个新的键值对，
        // 键是 "ARRIVE_PHEIGTH"，值是 `arrive_point.pheight` 的 F32Type 包装。
        // 如果已存在，则不执行插入操作。
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("LEAVE_PHEIGTH".to_string()).or_insert(NamedAttrValue::F32Type(leave_point.pheight));
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("ARRIVE_PWIDTH".to_string()).or_insert(NamedAttrValue::F32Type(arrive_point.pwidth));
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("LEAVE_PWIDTH".to_string()).or_insert(NamedAttrValue::F32Type(leave_point.pwidth));
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("ARRIVE_PBORE".to_string()).or_insert(NamedAttrValue::F32Type(arrive_point.pbore));
        map.entry(point.refno).or_insert_with(HashMap::new)
            .entry("LEAVE_PBORE".to_string()).or_insert(NamedAttrValue::F32Type(leave_point.pbore));
    }
    Ok(map)
}

/// 返回pspec属性对应的中文名
pub(crate) async fn get_pspec_code(refno: RefU64, database: &ArDatabase) -> anyhow::Result<String> {
    let pspe_name = query_foreign_name_aql(refno, vec!["PSPE", "PSPE"], database).await?;
    let mut kind = "".to_string();
    if let Some(pspe_name) = pspe_name {
        match pspe_name {
            s if s.contains("Ladder") => kind = "梯架".to_string(),
            s if s.contains("Ventilated") => kind = "带孔托盘".to_string(),
            s if s.contains("Trough") => kind = "实底托盘".to_string(),
            s if s.contains("Riser") => kind = "竖梯".to_string(),
            s if s.contains("Divider") => kind = "分隔板".to_string(),
            _ => {}
        }
    }
    Ok(kind)
}

/// 获取该节点的方位并转为pdms oria形式
pub(crate) async fn get_ori_angle_str(
    refno: RefU64,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<String> {
    let Ok(ori) = aios_mgr.get_attr(refno).await else {
        return Ok("".to_string());
    };
    let Some(ori) = ori.get_vec3("ORI") else {
        return Ok("0degree 0degree 0degree".to_string());
    };
    Ok(format!("{}degree {}degree {}degree", ori.x, ori.y, ori.z))
}

fn gen_dq_material_code_sql(
    spre_name_split: &str,
    stander_num: &str,
    fileds: &Vec<String>,
) -> String {
    let mut sql = String::from("SELECT ");
    for filed in fileds {
        sql.push_str(&format!("{} ,", filed));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(&format!("FROM `{}` ", PUHUA_DQ_MATERIAL_TABLE));
    sql.push_str(&format!(
        "WHERE ComponentName = '{}' AND StandardNum = '{}'",
        spre_name_split, stander_num
    ));
    sql
}

/// 通过spre name 返回材料编码 命名规则为 第二个 / 到 :
///
/// 例如 "/VMB1/CPP00102:P,50" -> "CPP00102"
pub(crate) fn get_spre_material_code(spre_name: &str) -> Option<String> {
    let spre_name_split = spre_name.split("/").collect::<Vec<_>>();
    if spre_name_split.len() < 3 {
        return None;
    }
    let spre_name_last = spre_name_split[2];
    let split = spre_name_last.split(":").collect::<Vec<_>>();
    if split.len() < 2 {
        return None;
    }
    Some(split[0].to_string())
}

/// 获取该节点的当年校审版本
pub fn get_refno_latest_version() -> String {
    "A版".to_string()
}

/// 分割字符串的字符部分和数字部分 例如 "sch400" -> sch  400
fn split_char_and_number(input: &str) -> Option<(String, String)> {
    let re = Regex::new(r"(?P<char_part>[A-Za-z]+)(?P<number_part>\d+)$").unwrap();
    if let Some(captures) = re.captures(input) {
        let Some(char_part) = captures.name("char_part") else {
            return None;
        };
        let Some(number_part) = captures.name("number_part") else {
            return None;
        };
        Some((
            char_part.as_str().to_string(),
            number_part.as_str().to_string(),
        ))
    } else {
        None
    }
}

/// 获取参考号集合所属的专业代码
pub async fn get_refnos_major_map(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<HashMap<RefU64, RefnoMajor>> {
    let refnos_major = query_refnos_belong_major(refnos, database).await?;
    let mut major_map = HashMap::new();
    for major in refnos_major {
        let Ok(refno) = RefU64::from_str(&major.refno) else { continue; };
        major_map.entry(refno).or_insert(major);
    }
    Ok(major_map)
}

/// 去掉 name 开头的 /
pub(crate) fn take_off_name_first_char(name: &str) -> String {
    if name.starts_with("/") { name[1..].to_string() } else { name.to_string() }
}


#[tokio::test]
async fn test_get_inst_data_from_inst_major() -> anyhow::Result<()> {
    let mgr = AiosDBManager::init_form_config().await?;
    let refno = RefU64::from_str("24381/103249").unwrap();
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
    let material_map = get_dq_material_code(&spre_name, &stander_num, &fileds, &aios_mgr).await?;
    dbg!(&material_map);

    Ok(())
}
