use std::collections::HashMap;
use std::{env, fs};

use std::io::Write;
use aios_core::create_attas_structs::VirtualHoleGraphNode;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject, HoleType, ItemValue};
use aios_core::negative_mesh_type::NegativeEdges;
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use aios_core::tool::hash_tool::hash_two_str;
use bb8_arangodb::arangors_lite::{AqlQuery, Database};

use chrono::DateTime;
use chrono::{Datelike, NaiveDateTime, Timelike};
use glam::Vec3;
use regex::Regex;
use sqlx::{Error, Executor, MySql, Pool, Row};
use sqlx::mysql::{MySqlQueryResult, MySqlRow};
use aios_core::create_attas_structs::VirtualHoleGraphNodeQuery;
use config::{Config, File};
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::consts::{AQL_HOLE_DATA_COLLECTION, AQL_HOLE_EDGE_COLLECTION, HOLES_TABLE};
use crate::data_center_api::data_api::get_refno_latest_version;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{ArDatabase, remove_arangodb_with_refno_key, save_arangodb_doc, update_arangodb_doc};
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

/// 正则匹配字符串中的数字
pub fn get_num_from_str(input: &str) -> Option<i32> {
    let regex = Regex::new(r"[0-9]+([.]{1}[0-9]+){0,1}").unwrap();
    if let Some(captures) = regex.captures(input) {
        if let Ok(r) = captures[0].parse::<i32>() {
            return Some(r);
        }
    }
    None
}

async fn query_hole_data_tidb(id: u32, pool: &Pool<MySql>) -> Option<DataCenterInstance> {
    if let Ok(hole_type) = query_hole_type(id, pool).await {
        let result = match hole_type {
            HoleType::STUCJ => {
                DataCenterInstance {
                    object_model_code: "STUCJ".to_string(),
                    project_code: "1516".to_string(),
                    instance_code: format!("STUCJ{}", id),
                    version: get_refno_latest_version(),
                    attributes: gen_stucj_data(id, pool).await,
                }
            }
            HoleType::STUCG => {
                DataCenterInstance {
                    object_model_code: "STUCG".to_string(),
                    project_code: "1516".to_string(),
                    instance_code: format!("STUCG{}", id),
                    version: get_refno_latest_version(),
                    attributes: gen_stucg_data(id, pool).await,
                }
            }
            HoleType::STUCH => {
                DataCenterInstance {
                    object_model_code: "STUCH".to_string(),
                    project_code: "1516".to_string(),
                    instance_code: format!("STUCH{}", id),
                    version: get_refno_latest_version(),
                    attributes: gen_stuch_data(id, pool).await,
                }
            }
            _ => { return None; }
        };
        return Some(result);
    }
    None
}

pub async fn gen_hole_datacenter_instance_aql(keys: Vec<String>, project_code: &str, database: &ArDatabase) -> Option<Vec<DataCenterInstance>> {
    let mut instances_result = Vec::new();
    let Ok(instances) = query_hole_data_by_keys_aql(keys, &database).await else { return Some(instances_result); };
    for (idx, instance) in instances.into_iter().enumerate() {
        if let Ok(hole_type) = query_hole_type_aql(&instance).await {
            let result = match hole_type {
                HoleType::STUCJ => {
                    DataCenterInstance {
                        object_model_code: "STUCJ".to_string(),
                        project_code: project_code.to_string(),
                        instance_code: format!("STUCJ{}", idx),
                        version: get_refno_latest_version(),
                        attributes: gen_stucj_data_aql(instance).await,
                    }
                }
                HoleType::STUCG => {
                    DataCenterInstance {
                        object_model_code: "STUCG".to_string(),
                        project_code: project_code.to_string(),
                        instance_code: format!("STUCG{}", idx),
                        version: get_refno_latest_version(),
                        attributes: gen_stucg_data_aql(instance).await,
                    }
                }
                HoleType::STUCH => {
                    DataCenterInstance {
                        object_model_code: "STUCH".to_string(),
                        project_code: project_code.to_string(),
                        instance_code: format!("STUCH{}", idx),
                        version: get_refno_latest_version(),
                        attributes: gen_stuch_data_aql(instance).await,
                    }
                }
                _ => { continue; }
            };
            instances_result.push(result);
        }
    }
    Some(instances_result)
}

/// 查找改参考号属于哪种孔洞
async fn query_hole_type(id: u32, pool: &Pool<MySql>) -> anyhow::Result<HoleType> {
    let sql = gen_query_hole_type_sql(id);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let h_type = result.get::<String, _>("hType");
            let material = result.get::<Option<String>, _>("SubsMaterial").unwrap_or("".to_string());
            return match h_type.as_str() {
                "K" => { Ok(HoleType::STUCJ) }
                "T" => { if material.as_str() == "Q235" { Ok(HoleType::STUCG) } else { Ok(HoleType::STUCH) } }
                "G" => { Ok(HoleType::STUCK) }
                "S" => { Ok(HoleType::STUCL) }
                "X" | "Y" => { Ok(HoleType::STUCM) }
                _ => { Ok(HoleType::Unknown) }
            };
        }
        Err(err) => { dbg!(&err); }
    }
    Ok(HoleType::Unknown)
}

async fn query_hole_type_aql(hole_data: &VirtualHoleGraphNode) -> anyhow::Result<HoleType> {
    let h_type = &hole_data.h_type;
    let material = &hole_data.subs_material;
    match h_type.as_str() {
        "K" => { Ok(HoleType::STUCJ) }
        "T" => { if material.as_str() == "Q235" { Ok(HoleType::STUCG) } else { Ok(HoleType::STUCH) } }
        "G" => { Ok(HoleType::STUCK) }
        "S" => { Ok(HoleType::STUCL) }
        "X" | "Y" => { Ok(HoleType::STUCM) }
        _ => { Ok(HoleType::Unknown) }
    }
}

async fn gen_stucj_data(id: u32, pool: &Pool<MySql>) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    if let Ok(stucj_data_map) = query_stucj_data(id, pool).await {
        for i in 0..33 {
            let name = format!("STUCJ{}", i);
            let value = stucj_data_map.get(&name);
            if value.is_none() { continue; }
            let value = value.unwrap();
            result.push(DataCenterAttr {
                attribute_model_code: name,
                value: value.clone().into(),
            });
        }
    }
    result
}

async fn gen_stucj_data_aql(hole_data: VirtualHoleGraphNode) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    if let Ok(stucj_data_map) = query_stucj_data_aql(hole_data).await {
        for i in 0..33 {
            let name = format!("STUCJ{}", i);
            let value = stucj_data_map.get(&name);
            if value.is_none() { continue; }
            let value = value.unwrap();
            result.push(DataCenterAttr {
                attribute_model_code: name,
                value: value.clone().into(),
            });
        }
    }
    result
}

async fn gen_stucg_data(id: u32, pool: &Pool<MySql>) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    if let Ok(stucj_data_map) = query_stucg_data(id, pool).await {
        for i in 0..33 {
            let name = format!("STUCG{}", i);
            let value = stucj_data_map.get(&name);
            if value.is_none() { continue; }
            let value = value.unwrap();
            result.push(DataCenterAttr {
                attribute_model_code: name,
                value: value.clone().into(),
            });
        }
    }
    result
}

async fn gen_stucg_data_aql(hole_data: VirtualHoleGraphNode) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    if let Ok(stucj_data_map) = query_stucg_data_aql(hole_data).await {
        for i in 0..33 {
            let name = format!("STUCG{}", i);
            let value = stucj_data_map.get(&name);
            if value.is_none() { continue; }
            let value = value.unwrap();
            result.push(DataCenterAttr {
                attribute_model_code: name,
                value: value.clone().into(),
            });
        }
    }
    result
}


async fn gen_stuch_data(id: u32, pool: &Pool<MySql>) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    if let Ok(stucj_data_map) = query_stuch_data(id, pool).await {
        for i in 0..33 {
            let name = format!("STUCH{}", i);
            let value = stucj_data_map.get(&name);
            if value.is_none() { continue; }
            let value = value.unwrap();
            result.push(DataCenterAttr {
                attribute_model_code: name,
                value: value.clone().into(),
            });
        }
    }
    result
}

async fn gen_stuch_data_aql(hole_data: VirtualHoleGraphNode) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    if let Ok(stucj_data_map) = query_stuch_data_aql(hole_data).await {
        for i in 0..33 {
            let name = format!("STUCH{}", i);
            let value = stucj_data_map.get(&name);
            if value.is_none() { continue; }
            let value = value.unwrap();
            result.push(DataCenterAttr {
                attribute_model_code: name,
                value: value.clone().into(),
            });
        }
    }
    result
}

/// 查找stucj的数据
async fn query_stucj_data(id: u32, pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let sql = gen_query_hole_data_sql(id);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let item_ref = result.try_get::<String, _>("ItemREF").unwrap_or("".to_string());
            let value = get_item_ref_value(item_ref, HoleType::STUCJ);
            map.entry("STUCJ1".to_string()).or_insert(AttrValue::AttrItemArray(value));
            let h_type = result.get::<String, _>("hType");
            map.entry("STUCJ2".to_string()).or_insert(AttrValue::AttrString(h_type));
            let code = result.get::<String, _>("Code");
            map.entry("STUCJ3".to_string()).or_insert(AttrValue::AttrString(code));
            let rely_item = result.get::<String, _>("RelyItem");
            map.entry("STUCJ4".to_string()).or_insert(AttrValue::AttrString(rely_item));
            let rely_item_ref = result.get::<String, _>("RelyItemREF");
            map.entry("STUCJ5".to_string()).or_insert(AttrValue::AttrString(rely_item_ref));
            let mut pipe_line_map = HashMap::new();
            pipe_line_map.entry("工艺管道".to_string()).or_insert_with(Vec::new).push("Test".to_string());
            map.entry("STUCJ6".to_string()).or_insert(AttrValue::AttrMap(pipe_line_map));

            let position = result.get::<String, _>("Position");
            let position = get_pos_from_str(position);
            let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
            map.entry("STUCJ7".to_string()).or_insert(AttrValue::AttrFloatArray(position));
            let ori = result.get::<String, _>("Ori");
            map.entry("STUCJ8".to_string()).or_insert(AttrValue::AttrString(ori));

            let shape = result.get::<String, _>("Shape");
            let size_height = result.get::<f32, _>("SizeHeight");
            let size_width = result.get::<f32, _>("SizeWidth");
            let mut shape_map = HashMap::new();
            match shape.as_str() {
                "CIR" => {
                    shape_map.entry("圆形孔洞".to_string()).or_insert(vec![size_width]);
                }
                "RECT" => {
                    shape_map.entry("方形孔洞".to_string()).or_insert(vec![size_width, size_height]);
                }
                _ => {}
            };
            let a = Vec3::from_array([0.0, 0.0, 0.0]);
            let b = Vec3::from_array([0.0, 0.0, 0.0]);
            // map.entry("STUCJ9".to_string()).or_insert(AttrValue::AttrVec3Array(vec![a, b]));
            map.entry("STUCJ10".to_string()).or_insert(AttrValue::AttrMapFloatArray(shape_map));

            let bank_height = result.try_get::<f32, _>("BankHeight").unwrap_or(0.0);
            let bank_width = result.try_get::<f32, _>("BankWidth").unwrap_or(0.0);
            if bank_height != 0.0 && bank_width != 0.0 {
                map.entry("STUCJ11".to_string()).or_insert(AttrValue::AttrString("Y".to_string()));
            } else {
                map.entry("STUCJ11".to_string()).or_insert(AttrValue::AttrString("N".to_string()));
            }

            map.entry("STUCJ12".to_string()).or_insert(AttrValue::AttrFloat(bank_height));

            map.entry("STUCJ13".to_string()).or_insert(AttrValue::AttrFloat(bank_width));

            let plug_type = result.get::<Option<String>, _>("PlugType");
            // let plug_type = match_plug_type_str(&plug_type[..1]);
            map.entry("STUCJ17".to_string()).or_insert(AttrValue::AttrBool(plug_type.is_some()));
            map.entry("STUCJ18".to_string()).or_insert(AttrValue::AttrString(plug_type.unwrap_or("".to_string())));

            // map.entry("STUCJ19".to_string()).or_insert(AttrValue::AttrString("PIA100".to_string()));
            map.entry("STUCJ20".to_string()).or_insert(AttrValue::AttrString("600".to_string()));
            let b_second = result.get::<bool, _>("Second");
            map.entry("STUCJ21".to_string()).or_insert(AttrValue::AttrBool(b_second));

            map.entry("STUCJ22".to_string()).or_insert(AttrValue::AttrFloatArray(vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0]));
            let hole_work = result.get::<String, _>("HoleWork");
            map.entry("STUCJ23".to_string()).or_insert(AttrValue::AttrString(hole_work));

            let work_by = result.get::<String, _>("WorkBy");
            map.entry("STUCJ24".to_string()).or_insert(AttrValue::AttrString(work_by));

            let time = result.get::<String, _>("Time").replace("/", "-");
            let time = convert_time_to_vec(&time);
            map.entry("STUCJ25".to_string()).or_insert(AttrValue::AttrStrArray(time));

            let open_item = result.try_get::<String, _>("OpenItem").unwrap_or("".to_string());
            map.entry("STUCJ26".to_string()).or_insert(AttrValue::AttrString(open_item));

            let note = result.get::<Option<String>, _>("Note").unwrap_or("".to_string());
            map.entry("STUCJ27".to_string()).or_insert(AttrValue::AttrString(note));

            let note = result.get::<Option<String>, _>("FittRefNo").unwrap_or("".to_string());
            map.entry("STUCJ28".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("HoleBPID").unwrap_or("".to_string());
            map.entry("STUCJ29".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("HoleBPVER").unwrap_or("".to_string());
            map.entry("STUCJ30".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("RelyItemBPID").unwrap_or("".to_string());
            map.entry("STUCJ31".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("RelyItemBPVER").unwrap_or("".to_string());
            map.entry("STUCJ32".to_string()).or_insert(AttrValue::AttrString(note));
        }
        Err(err) => { dbg!(&err); }
    }
    Ok(map)
}

async fn query_stucj_data_aql(hole_data: VirtualHoleGraphNode) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let item_ref = hole_data.item_ref;
    let value = get_item_ref_value(item_ref, HoleType::STUCJ);
    map.entry("STUCJ1".to_string()).or_insert(AttrValue::AttrItemArray(value));
    let h_type = hole_data.h_type;
    map.entry("STUCJ2".to_string()).or_insert(AttrValue::AttrString(h_type));
    let code = hole_data._key;
    map.entry("STUCJ3".to_string()).or_insert(AttrValue::AttrString(code));
    let rely_item = hole_data.rely_item;
    map.entry("STUCJ4".to_string()).or_insert(AttrValue::AttrString(rely_item));
    let rely_item_ref = hole_data.rely_item_ref;
    map.entry("STUCJ5".to_string()).or_insert(AttrValue::AttrString(rely_item_ref));
    let mut pipe_line_map = HashMap::new();
    pipe_line_map.entry("工艺管道".to_string()).or_insert_with(Vec::new).push("Test".to_string());
    map.entry("STUCJ6".to_string()).or_insert(AttrValue::AttrMap(pipe_line_map));

    let position = hole_data.position;
    let position = get_pos_from_str(position);
    let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
    map.entry("STUCJ7".to_string()).or_insert(AttrValue::AttrFloatArray(position));
    let ori = hole_data.ori;
    map.entry("STUCJ8".to_string()).or_insert(AttrValue::AttrString(ori));

    let shape = hole_data.shape;
    let size_height = hole_data.size_height;
    let size_width = hole_data.size_width;
    let mut shape_map = HashMap::new();
    match shape.as_str() {
        "CIR" => {
            shape_map.entry("圆形孔洞".to_string()).or_insert(vec![size_width]);
        }
        "RECT" => {
            shape_map.entry("方形孔洞".to_string()).or_insert(vec![size_width, size_height]);
        }
        _ => {}
    };
    map.entry("STUCJ10".to_string()).or_insert(AttrValue::AttrMapFloatArray(shape_map));

    let bank_height = hole_data.bank_height;
    let bank_width = hole_data.bank_width;
    if bank_height != 0.0 && bank_width != 0.0 {
        map.entry("STUCJ11".to_string()).or_insert(AttrValue::AttrString("Y".to_string()));
    } else {
        map.entry("STUCJ11".to_string()).or_insert(AttrValue::AttrString("N".to_string()));
    }

    map.entry("STUCJ12".to_string()).or_insert(AttrValue::AttrFloat(bank_height));

    map.entry("STUCJ13".to_string()).or_insert(AttrValue::AttrFloat(bank_width));

    let plug_type = hole_data.plug_type;
    // let plug_type = match_plug_type_str(&plug_type[..1]);
    map.entry("STUCJ17".to_string()).or_insert(AttrValue::AttrBool(plug_type.is_empty()));
    map.entry("STUCJ18".to_string()).or_insert(AttrValue::AttrString(plug_type));

    map.entry("STUCJ20".to_string()).or_insert(AttrValue::AttrString("600".to_string()));
    let b_second = hole_data.second;
    map.entry("STUCJ21".to_string()).or_insert(AttrValue::AttrBool(b_second));

    map.entry("STUCJ22".to_string()).or_insert(AttrValue::AttrFloatArray(vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0]));
    let hole_work = hole_data.hole_work;
    map.entry("STUCJ23".to_string()).or_insert(AttrValue::AttrString(hole_work));

    let work_by = hole_data.work_by;
    map.entry("STUCJ24".to_string()).or_insert(AttrValue::AttrString(work_by));

    let time = hole_data.time.replace("/", "-");
    let time = convert_time_to_vec(&time);
    map.entry("STUCJ25".to_string()).or_insert(AttrValue::AttrStrArray(time));

    let open_item = hole_data.open_item;
    map.entry("STUCJ26".to_string()).or_insert(AttrValue::AttrString(open_item));

    let note = hole_data.note;
    map.entry("STUCJ27".to_string()).or_insert(AttrValue::AttrString(note));

    let fitt_refno = hole_data.fitt_refno;
    map.entry("STUCJ28".to_string()).or_insert(AttrValue::AttrString(fitt_refno));
    let hole_b_pid = hole_data.hole_bpid;
    map.entry("STUCJ29".to_string()).or_insert(AttrValue::AttrString(hole_b_pid));
    let hole_b_pver = hole_data.hole_bpver;
    map.entry("STUCJ30".to_string()).or_insert(AttrValue::AttrString(hole_b_pver));
    let rely_item_b_pid = hole_data.rely_item_bpid;
    map.entry("STUCJ31".to_string()).or_insert(AttrValue::AttrString(rely_item_b_pid));
    let rely_item_b_pver = hole_data.rely_item_bpver;
    map.entry("STUCJ32".to_string()).or_insert(AttrValue::AttrString(rely_item_b_pver));
    Ok(map)
}

async fn query_stucg_data(id: u32, pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let sql = gen_query_hole_data_sql(id);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let item_ref = result.try_get::<String, _>("ItemREF").unwrap_or("".to_string());
            let value = get_item_ref_value(item_ref, HoleType::STUCG);
            map.entry("STUCG1".to_string()).or_insert(AttrValue::AttrItemArray(value));
            let h_type = result.get::<String, _>("hType");
            let h_type = if &h_type == "T" { "直管".to_string() } else { "弯管".to_string() };
            map.entry("STUCG2".to_string()).or_insert(AttrValue::AttrString(h_type));
            let code = result.get::<String, _>("Code");
            map.entry("STUCG3".to_string()).or_insert(AttrValue::AttrString(code));
            let rely_item = result.get::<String, _>("RelyItem");
            map.entry("STUCG4".to_string()).or_insert(AttrValue::AttrString(rely_item));
            let rely_item_ref = result.get::<String, _>("RelyItemREF");
            map.entry("STUCG5".to_string()).or_insert(AttrValue::AttrString(rely_item_ref));

            let mut pipe_line_map = HashMap::new();
            pipe_line_map.entry("工艺管道".to_string()).or_insert_with(Vec::new).push("Test".to_string());
            map.entry("STUCG6".to_string()).or_insert(AttrValue::AttrMap(pipe_line_map));

            let position = result.get::<String, _>("Position");
            let position = get_pos_from_str(position);
            let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
            map.entry("STUCG7".to_string()).or_insert(AttrValue::AttrFloatArray(position));
            let ori = result.get::<String, _>("Ori");
            map.entry("STUCG8".to_string()).or_insert(AttrValue::AttrString(ori));

            let subs_type = result.get::<Option<String>, _>("SubsType").unwrap_or("".to_string());
            map.entry("STUCG10".to_string()).or_insert(AttrValue::AttrString(subs_type));
            let position = result.get::<Option<f32>, _>("SubsThickness").unwrap_or(0.0);
            let size_width = result.get::<f32, _>("SizeWidth");
            map.entry("STUCG11".to_string()).or_insert(AttrValue::AttrFloatArray(vec![size_width, position]));

            let extent_length_1 = result.get::<Option<f32>, _>("ExtentLength1").unwrap_or(0.0);
            let extent_length_2 = result.get::<Option<f32>, _>("ExtentLength2").unwrap_or(0.0);
            let size_throw_wall = result.get::<Option<f32>, _>("SizeThrowWall").unwrap_or(0.0);
            map.entry("STUCG12".to_string()).or_insert(AttrValue::AttrFloatArray(vec![extent_length_1, size_throw_wall, extent_length_2]));

            let subs_material = result.get::<Option<String>, _>("SubsMaterial").unwrap_or("".to_string());
            map.entry("STUCG13".to_string()).or_insert(AttrValue::AttrString(subs_material));
            map.entry("STUCG14".to_string()).or_insert(AttrValue::AttrVec3Array(vec![Vec3::ZERO, Vec3::ZERO]));

            map.entry("STUCG15".to_string()).or_insert(AttrValue::AttrFloat(0.0));
            let plug_type = result.get::<Option<String>, _>("PlugType");
            map.entry("STUCG16".to_string()).or_insert(AttrValue::AttrBool(plug_type.is_some()));
            // let plug_type = match_plug_type_str(&plug_type[..1]);
            map.entry("STUCG17".to_string()).or_insert(AttrValue::AttrString(plug_type.unwrap_or("".to_string())));
            map.entry("STUCG19".to_string()).or_insert(AttrValue::AttrString("".to_string()));

            let hole_work = result.get::<String, _>("HoleWork");
            map.entry("STUCG21".to_string()).or_insert(AttrValue::AttrString(hole_work));
            let work_by = result.get::<String, _>("WorkBy");
            map.entry("STUCG22".to_string()).or_insert(AttrValue::AttrString(work_by));
            let time = result.get::<String, _>("Time").replace("/", "-");
            let time = convert_time_to_vec(&time);
            map.entry("STUCG23".to_string()).or_insert(AttrValue::AttrStrArray(time));
            let open_item = result.try_get::<String, _>("OpenItem").unwrap_or("".to_string());
            map.entry("STUCG24".to_string()).or_insert(AttrValue::AttrString(open_item));
            let note = result.get::<Option<String>, _>("Note").unwrap_or("".to_string());
            map.entry("STUCG25".to_string()).or_insert(AttrValue::AttrString(note));

            let note = result.get::<Option<String>, _>("FittRefNo").unwrap_or("".to_string());
            map.entry("STUCG26".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("HoleBPID").unwrap_or("".to_string());
            map.entry("STUCG27".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("HoleBPVER").unwrap_or("".to_string());
            map.entry("STUCG28".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("RelyItemBPID").unwrap_or("".to_string());
            map.entry("STUCG29".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("RelyItemBPVER").unwrap_or("".to_string());
            map.entry("STUCG30".to_string()).or_insert(AttrValue::AttrString(note));
        }
        _ => {}
    }
    Ok(map)
}

async fn query_stucg_data_aql(hole_data: VirtualHoleGraphNode) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let item_ref = hole_data.item_ref;
    let value = get_item_ref_value(item_ref, HoleType::STUCG);
    map.entry("STUCG1".to_string()).or_insert(AttrValue::AttrItemArray(value));
    let h_type = hole_data.h_type;
    let h_type = if &h_type == "T" { "直管".to_string() } else { "弯管".to_string() };
    map.entry("STUCG2".to_string()).or_insert(AttrValue::AttrString(h_type));
    let code = hole_data._key;
    map.entry("STUCG3".to_string()).or_insert(AttrValue::AttrString(code));
    let rely_item = hole_data.rely_item;
    map.entry("STUCG4".to_string()).or_insert(AttrValue::AttrString(rely_item));
    let rely_item_ref = hole_data.rely_item_ref;
    map.entry("STUCG5".to_string()).or_insert(AttrValue::AttrString(rely_item_ref));

    let mut pipe_line_map = HashMap::new();
    pipe_line_map.entry("工艺管道".to_string()).or_insert_with(Vec::new).push("Test".to_string());
    map.entry("STUCG6".to_string()).or_insert(AttrValue::AttrMap(pipe_line_map));

    let position = hole_data.position;
    let position = get_pos_from_str(position);
    let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
    map.entry("STUCG7".to_string()).or_insert(AttrValue::AttrFloatArray(position));
    let ori = hole_data.ori;
    map.entry("STUCG8".to_string()).or_insert(AttrValue::AttrString(ori));

    let subs_type = hole_data.subs_type;
    map.entry("STUCG10".to_string()).or_insert(AttrValue::AttrString(subs_type));
    let subs_thickness = hole_data.subs_thickness;
    let size_width = hole_data.size_width;
    map.entry("STUCG11".to_string()).or_insert(AttrValue::AttrFloatArray(vec![size_width, subs_thickness]));

    let extent_length_1 = hole_data.extent_length1;
    let extent_length_2 = hole_data.extent_length2;
    let size_throw_wall = hole_data.size_throw_wall;
    map.entry("STUCG12".to_string()).or_insert(AttrValue::AttrFloatArray(vec![extent_length_1, size_throw_wall, extent_length_2]));

    let subs_material = hole_data.subs_material;
    map.entry("STUCG13".to_string()).or_insert(AttrValue::AttrString(subs_material));
    map.entry("STUCG14".to_string()).or_insert(AttrValue::AttrVec3Array(vec![Vec3::ZERO, Vec3::ZERO]));

    map.entry("STUCG15".to_string()).or_insert(AttrValue::AttrFloat(0.0));
    let plug_type = hole_data.plug_type;
    map.entry("STUCG16".to_string()).or_insert(AttrValue::AttrBool(plug_type.is_empty()));
    // let plug_type = match_plug_type_str(&plug_type[..1]);
    map.entry("STUCG17".to_string()).or_insert(AttrValue::AttrString(plug_type));
    map.entry("STUCG19".to_string()).or_insert(AttrValue::AttrString("".to_string()));

    let hole_work = hole_data.hole_work;
    map.entry("STUCG21".to_string()).or_insert(AttrValue::AttrString(hole_work));
    let work_by = hole_data.work_by;
    map.entry("STUCG22".to_string()).or_insert(AttrValue::AttrString(work_by));
    let time = hole_data.time.replace("/", "-");
    let time = convert_time_to_vec(&time);
    map.entry("STUCG23".to_string()).or_insert(AttrValue::AttrStrArray(time));
    let open_item = hole_data.open_item;
    map.entry("STUCG24".to_string()).or_insert(AttrValue::AttrString(open_item));
    let note = hole_data.note;
    map.entry("STUCG25".to_string()).or_insert(AttrValue::AttrString(note));

    let fitt_refno = hole_data.fitt_refno;
    map.entry("STUCG26".to_string()).or_insert(AttrValue::AttrString(fitt_refno));
    let hole_b_pid = hole_data.hole_bpid;
    map.entry("STUCG27".to_string()).or_insert(AttrValue::AttrString(hole_b_pid));
    let hole_b_pver = hole_data.hole_bpver;
    map.entry("STUCG28".to_string()).or_insert(AttrValue::AttrString(hole_b_pver));
    let rely_item_b_pid = hole_data.rely_item_bpid;
    map.entry("STUCG29".to_string()).or_insert(AttrValue::AttrString(rely_item_b_pid));
    let rely_item_b_pver = hole_data.rely_item_bpver;
    map.entry("STUCG30".to_string()).or_insert(AttrValue::AttrString(rely_item_b_pver));
    Ok(map)
}

async fn query_stuch_data(id: u32, pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let sql = gen_query_hole_data_sql(id);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let item_ref = result.try_get::<String, _>("ItemREF").unwrap_or("".to_string());
            let value = get_item_ref_value(item_ref, HoleType::STUCG);
            map.entry("STUCH1".to_string()).or_insert(AttrValue::AttrItemArray(value));
            let subs_type = result.try_get::<String, _>("SubsType").unwrap_or("".to_string());
            map.entry("STUCH2".to_string()).or_insert(AttrValue::AttrString(subs_type));
            let code = result.get::<String, _>("Code");
            map.entry("STUCH3".to_string()).or_insert(AttrValue::AttrString(code));
            let rely_item = result.get::<String, _>("RelyItem");
            map.entry("STUCH4".to_string()).or_insert(AttrValue::AttrString(rely_item));
            let rely_item_ref = result.get::<String, _>("RelyItemREF");
            map.entry("STUCH5".to_string()).or_insert(AttrValue::AttrString(rely_item_ref));

            let mut pipe_line_map = HashMap::new();
            pipe_line_map.entry("工艺管道".to_string()).or_insert_with(Vec::new).push("Test".to_string());
            map.entry("STUCH6".to_string()).or_insert(AttrValue::AttrMap(pipe_line_map));
            let position = result.get::<String, _>("Position");
            let position = get_pos_from_str(position);
            let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
            map.entry("STUCH7".to_string()).or_insert(AttrValue::AttrFloatArray(position));
            let ori = result.get::<String, _>("Ori");
            map.entry("STUCH8".to_string()).or_insert(AttrValue::AttrString(ori));

            let extent_length_1 = result.get::<Option<f32>, _>("ExtentLength1").unwrap_or(0.0);
            let extent_length_2 = result.get::<Option<f32>, _>("ExtentLength2").unwrap_or(0.0);
            let size_throw_wall = result.get::<Option<f32>, _>("SizeThrowWall").unwrap_or(0.0);
            map.entry("STUCH10".to_string()).or_insert(AttrValue::AttrFloatArray(vec![extent_length_1, size_throw_wall, extent_length_2]));
            // let position = result.get::<Option<f32>, _>("SubsThickness").unwrap_or(0.0);
            let plug_type = result.get::<Option<String>, _>("PlugType");
            map.entry("STUCH11".to_string()).or_insert(AttrValue::AttrBool(plug_type.is_some()));
            // let plug_type = match_plug_type_str(&plug_type[..1]);
            map.entry("STUCH12".to_string()).or_insert(AttrValue::AttrString(plug_type.unwrap_or("".to_string())));
            map.entry("STUCH14".to_string()).or_insert(AttrValue::AttrString("600".to_string()));

            let hole_work = result.get::<String, _>("HoleWork");
            map.entry("STUCH15".to_string()).or_insert(AttrValue::AttrString(hole_work));
            let work_by = result.get::<String, _>("WorkBy");
            map.entry("STUCH16".to_string()).or_insert(AttrValue::AttrString(work_by));
            let time = result.get::<String, _>("Time").replace("/", "-");
            let time = convert_time_to_vec(&time);
            map.entry("STUCH17".to_string()).or_insert(AttrValue::AttrStrArray(time));
            let open_item = result.try_get::<String, _>("OpenItem").unwrap_or("".to_string());
            map.entry("STUCH18".to_string()).or_insert(AttrValue::AttrString(open_item));
            let note = result.get::<Option<String>, _>("Note").unwrap_or("".to_string());
            map.entry("STUCH19".to_string()).or_insert(AttrValue::AttrString(note));

            let note = result.get::<Option<String>, _>("FittRefNo").unwrap_or("".to_string());
            map.entry("STUCH20".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("HoleBPID").unwrap_or("".to_string());
            map.entry("STUCH21".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("HoleBPVER").unwrap_or("".to_string());
            map.entry("STUCH22".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("RelyItemBPID").unwrap_or("".to_string());
            map.entry("STUCH23".to_string()).or_insert(AttrValue::AttrString(note));
            let note = result.get::<Option<String>, _>("RelyItemBPVER").unwrap_or("".to_string());
            map.entry("STUCH24".to_string()).or_insert(AttrValue::AttrString(note));
        }
        _ => {}
    }
    Ok(map)
}

async fn query_stuch_data_aql(hole_data: VirtualHoleGraphNode) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let item_ref = hole_data.item_ref;
    let value = get_item_ref_value(item_ref, HoleType::STUCG);
    map.entry("STUCH1".to_string()).or_insert(AttrValue::AttrItemArray(value));
    let subs_type = hole_data.subs_type;
    map.entry("STUCH2".to_string()).or_insert(AttrValue::AttrString(subs_type));
    let code = hole_data._key;
    map.entry("STUCH3".to_string()).or_insert(AttrValue::AttrString(code));
    let rely_item = hole_data.rely_item;
    map.entry("STUCH4".to_string()).or_insert(AttrValue::AttrString(rely_item));
    let rely_item_ref = hole_data.rely_item_ref;
    map.entry("STUCH5".to_string()).or_insert(AttrValue::AttrString(rely_item_ref));

    let mut pipe_line_map = HashMap::new();
    pipe_line_map.entry("工艺管道".to_string()).or_insert_with(Vec::new).push("Test".to_string());
    map.entry("STUCH6".to_string()).or_insert(AttrValue::AttrMap(pipe_line_map));
    let position = hole_data.position;
    let position = get_pos_from_str(position);
    let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
    map.entry("STUCH7".to_string()).or_insert(AttrValue::AttrFloatArray(position));
    let ori = hole_data.ori;
    map.entry("STUCH8".to_string()).or_insert(AttrValue::AttrString(ori));

    let extent_length_1 = hole_data.extent_length1;
    let extent_length_2 = hole_data.extent_length2;
    let size_throw_wall = hole_data.size_throw_wall;
    map.entry("STUCH10".to_string()).or_insert(AttrValue::AttrFloatArray(vec![extent_length_1, size_throw_wall, extent_length_2]));
    let plug_type = hole_data.plug_type;
    map.entry("STUCH11".to_string()).or_insert(AttrValue::AttrBool(plug_type.is_empty()));
    map.entry("STUCH12".to_string()).or_insert(AttrValue::AttrString(plug_type));
    map.entry("STUCH14".to_string()).or_insert(AttrValue::AttrString("600".to_string()));

    let hole_work = hole_data.hole_work;
    map.entry("STUCH15".to_string()).or_insert(AttrValue::AttrString(hole_work));
    let work_by = hole_data.work_by;
    map.entry("STUCH16".to_string()).or_insert(AttrValue::AttrString(work_by));
    let time = hole_data.time.replace("/", "-");
    let time = convert_time_to_vec(&time);
    map.entry("STUCH17".to_string()).or_insert(AttrValue::AttrStrArray(time));
    let open_item = hole_data.open_item;
    map.entry("STUCH18".to_string()).or_insert(AttrValue::AttrString(open_item));
    let note = hole_data.note;
    map.entry("STUCH19".to_string()).or_insert(AttrValue::AttrString(note));

    let fitt_refno = hole_data.fitt_refno;
    map.entry("STUCH20".to_string()).or_insert(AttrValue::AttrString(fitt_refno));
    let hole_b_pid = hole_data.hole_bpid;
    map.entry("STUCH21".to_string()).or_insert(AttrValue::AttrString(hole_b_pid));
    let hole_b_pver = hole_data.hole_bpver;
    map.entry("STUCH22".to_string()).or_insert(AttrValue::AttrString(hole_b_pver));
    let rely_item_b_pid = hole_data.rely_item_bpid;
    map.entry("STUCH23".to_string()).or_insert(AttrValue::AttrString(rely_item_b_pid));
    let rely_item_b_pver = hole_data.rely_item_bpver;
    map.entry("STUCH24".to_string()).or_insert(AttrValue::AttrString(rely_item_b_pver));
    Ok(map)
}

fn gen_query_hole_data_sql(id: u32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT * FROM {HOLES_TABLE} WHERE IntelId = {}", id));
    sql
}

fn gen_query_hole_type_sql(id: u32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT hType,SubsMaterial FROM {HOLES_TABLE} WHERE IntelId = {}", id));
    sql
}

fn get_item_ref_value(item_ref: String, h_type: HoleType) -> Vec<ItemValue> {
    let mut result = Vec::new();
    if item_ref.len() > 10 {
        match h_type {
            _ => {
                result.push(ItemValue::String(item_ref[..1].to_string()));
                result.push(ItemValue::String(item_ref[1..3].to_string()));
                result.push(ItemValue::String(item_ref[3..4].to_string()));
                result.push(ItemValue::String(item_ref[4..6].to_string()));
                let num = get_num_from_str(&item_ref[1..]).unwrap_or(0);
                result.push(ItemValue::Int(num));
                let len = item_ref.len();
                result.push(ItemValue::String(item_ref[len - 1..len].to_string()));
            }
        }
    }
    result
}


pub(crate) fn get_pos_from_str(input: String) -> Vec<f32> {
    let mut result = Vec::new();
    let input_split = input.split(",").collect::<Vec<&str>>();
    for input_str in input_split {
        let data = input_str.parse::<f32>();
        if data.is_err() { continue; }
        result.push(data.unwrap());
    }
    result
}

pub fn convert_time_to_vec(time: &str) -> Vec<String> {
    let mut r = Vec::new();
    if let Ok(dt) = NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S") {
        r.push(dt.year().to_string());
        r.push(dt.month().to_string());
        r.push(dt.day().to_string());
        r.push(dt.hour().to_string());
        r.push(dt.minute().to_string());
        r.push(dt.second().to_string());
    }
    r
}

pub async fn save_hole_data_to_arangodb(data: Vec<VirtualHoleGraphNode>, database: &ArDatabase) -> anyhow::Result<String> {
    let json = serde_json::to_value(&data);
    if json.is_err() { return Ok("输入的数据格式不符合规则".to_string()); }
    let json = json.unwrap();
    let r = save_arangodb_doc(json, AQL_HOLE_DATA_COLLECTION, database, false).await;
    let edge_r = create_hole_data_edge(&data, &database).await?;
    if let Err(r) = r {
        Ok(r.to_string())
    } else {
        Ok("保存成功".to_string())
    }
}

/// 替换孔洞数据
pub async fn replace_hole_data_to_arangodb(datas: Vec<VirtualHoleGraphNode>, database: &ArDatabase) -> anyhow::Result<String> {
    // 删除边
    let keys = datas.iter().map(|x| x._key.clone()).collect::<Vec<_>>();
    let edge_aql = AqlQuery::new("\
    With hole_data,hole_edge
    for key in @keys
        for c,e in 1 inbound CONCAT('hole_data/',key) hole_edge
            REMOVE e._key IN hole_edge
    ").bind_var("keys", keys);
    let result = database.aql_query::<Vec<()>>(edge_aql).await?;
    // 重新插入新的边
    match replace_hole_data_edge(&datas, &database).await {
        Ok(_) => {}
        Err(e) => {
            return Ok(e.to_string());
        }
    }
    let data_len = datas.len();
    // 替换数据
    for data in datas {
        let json = serde_json::to_value(&data)?;
        match update_arangodb_doc(&data._key, json, AQL_HOLE_DATA_COLLECTION, &database).await {
            Ok(_) => {}
            Err(e) => {
                return Ok(e.to_string());
            }
        }
    }
    // let json = serde_json::to_value(&datas);
    // if json.is_err() { return Ok("输入的数据格式不符合规则".to_string()); }
    // let json = json.unwrap();
    // match save_arangodb_doc(json, AQL_HOLE_DATA_COLLECTION, &database, true).await {
    //     Ok(_) => {}
    //     Err(e) => {
    //         return Ok(e.to_string())
    //     }
    // }
    Ok(format!("替换 {} 条数据 成功", data_len))
}

async fn create_hole_data_edge(data: &Vec<VirtualHoleGraphNode>, database: &ArDatabase) -> anyhow::Result<()> {
    let mut edges = Vec::new();
    for d in data {
        let refno = RefU64::from_refno_str(&d.rely_item_ref);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let from = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno());
        let to = format!("{}/{}", AQL_HOLE_DATA_COLLECTION, d._key);
        let hash = hash_two_str(&from, &to);
        edges.push(NegativeEdges {
            _key: hash.to_string(),
            _from: from,
            _to: to,
        });
    }
    if !edges.is_empty() {
        let json = serde_json::to_value(&edges)?;
        save_arangodb_doc(json, AQL_HOLE_EDGE_COLLECTION, database, false).await?;
    }
    Ok(())
}

async fn replace_hole_data_edge(data: &Vec<VirtualHoleGraphNode>, database: &ArDatabase) -> anyhow::Result<()> {
    let mut edges = Vec::new();
    for d in data {
        let refno = RefU64::from_refno_str(&d.rely_item_ref);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let from = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno());
        let to = format!("{}/{}", AQL_HOLE_DATA_COLLECTION, d._key);
        let hash = hash_two_str(&from, &to);
        edges.push(NegativeEdges {
            _key: hash.to_string(),
            _from: from,
            _to: to,
        });
    }
    if !edges.is_empty() {
        let json = serde_json::to_value(&edges)?;
        save_arangodb_doc(json, AQL_HOLE_EDGE_COLLECTION, &database, true).await?;
    }
    Ok(())
}


/// 通过孔洞依附的墙或板来查询这个墙上所有的孔洞数据
pub async fn query_hole_data_aql(rely_refno: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<VirtualHoleGraphNode>> {
    let keys = rely_refno.into_iter().map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::new("
    with @@pdms_eles,@@hole_edge,@@hole_data
    for key in @keys
    for c in 1 outbound key @@hole_edge
        filter c != null
        return unset(c , '_id','_rev')")
        .bind_var("keys", keys)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@hole_data", AQL_HOLE_DATA_COLLECTION)
        .bind_var("@hole_edge", AQL_HOLE_EDGE_COLLECTION);
    let result = database.aql_query::<VirtualHoleGraphNode>(aql).await?;
    Ok(result)
}

/// 获得当前可提资的所有孔洞
pub async fn query_available_hole_data_aql(rely_refno: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<VirtualHoleGraphNodeQuery>> {
    let keys = rely_refno.into_iter().map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::new("
    with @@pdms_eles,@@hole_edge,@@hole_data
    for key in @keys
    for c in 1 outbound key @@hole_edge
        filter c != null && c.HoleWork=='CONFIRM'
        return unset(c , '_id','_rev')")
        .bind_var("keys", keys)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@hole_data", AQL_HOLE_DATA_COLLECTION)
        .bind_var("@hole_edge", AQL_HOLE_EDGE_COLLECTION);
    let result = database.aql_query::<VirtualHoleGraphNodeQuery>(aql).await?;
    Ok(result)
}




/// 查询虚拟孔洞数据中已经转为实体的孔洞数据
pub async fn query_entity_hole_data(rely_refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<VirtualHoleGraphNode>> {
    let keys = rely_refnos.into_iter().map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::new("
    with @@pdms_eles,@@hole_edge,@@hole_data
    for key in @keys
    for c in 1 outbound key @@hole_edge
        filter c != null
        filter c.HoleWork == 'REAL'
        filter c.ItemREF like '%EE%' || c.ItemREF like '%KK%' || c.ItemREF like '%LL%'
        return unset(c , '_id','_rev')")
        .bind_var("keys", keys)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@hole_data", AQL_HOLE_DATA_COLLECTION)
        .bind_var("@hole_edge", AQL_HOLE_EDGE_COLLECTION);
    let result = database.aql_query::<VirtualHoleGraphNode>(aql).await?;
    Ok(result)
}


pub async fn query_hole_data_by_keys_aql(keys: Vec<String>, database: &ArDatabase) -> anyhow::Result<Vec<VirtualHoleGraphNode>> {
    let aql = AqlQuery::new("
    for key in @keys
        let c = document(@@hole_collection,key)
        filter c != null
        return unset(c , '_id','_rev')")
        .bind_var("keys", keys)
        .bind_var("@hole_collection", AQL_HOLE_DATA_COLLECTION);
    let result = database.aql_query::<VirtualHoleGraphNode>(aql).await?;
    Ok(result)
}

/// 查询所有的孔洞信息
pub async fn query_hole_data_total_aql(database: &ArDatabase) -> anyhow::Result<Vec<VirtualHoleGraphNodeQuery>> {
    let aql = AqlQuery::new("
    for c in @@collection
        return unset(c , '_id','_rev')").bind_var("@collection", AQL_HOLE_DATA_COLLECTION);
    let result = database.aql_query::<VirtualHoleGraphNodeQuery>(aql).await?;
    Ok(result)
}

#[tokio::test]
async fn test_query_hole_data_total_aql() {
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build().unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await.unwrap();
    if let Ok(result) = query_hole_data_total_aql(&database).await {
        dbg!(&result);
    }
}

/// 删除孔洞的信息，并删除边
pub async fn delete_hole_data_aql(keys: Vec<String>, database: &ArDatabase) -> anyhow::Result<bool> {
    let edge_aql = AqlQuery::new("\
    for key in @keys
        for c,e in 1 inbound CONCAT('hole_data/',key) hole_edge
            REMOVE e._key IN hole_edge
    ").bind_var("keys", keys.clone());
    let result = database.aql_query::<Vec<()>>(edge_aql).await;
    let data_aql = AqlQuery::new("\
    for key in @keys
       REMOVE key IN hole_data
    ").bind_var("keys", keys);
    let result = database.aql_query::<Vec<()>>(data_aql).await;
    Ok(!result.is_err())
}

fn match_plug_type_str(input: &str) -> String {
    match input.to_uppercase().as_str() {
        "A" => { "气密封堵".to_string() }
        "F" => { "防火封堵".to_string() }
        "W" => { "水淹封堵".to_string() }
        "B" => { "生物屏蔽封堵".to_string() }
        "N" => { "压力释放".to_string() }
        "B+" => { "重混凝土封堵".to_string() }
        "M" => { "MCT封堵".to_string() }
        "V" => { "无效孔洞".to_string() }
        "N1" => { "非边界孔洞不封堵".to_string() }
        "N2" => { "门洞、吊装洞、排水孔洞、通视孔、地漏等不封堵".to_string() }
        "G1" => { "国标防水封堵1,按照《防水套管》02S404图集要求进行封堵".to_string() }
        "G2" => { "国标防水封堵2,按照《消防水泵接合器装》99(03)S203图集要求使用C20细混凝材料进行封堵".to_string() }
        "G3" => { "国标防水封堵3,待雨水斗安装完毕后,按照《其他厂房排水》技术规格书(项目文件编码)要求进行封堵".to_string() }
        &_ => { "".to_string() }
    }
}


#[test]
fn test_convert_time_to_vec() {
    // let mut r = Vec::new();
    let time_str = "2023-3-9 10:42:35";
    let dt = NaiveDateTime::parse_from_str(time_str, "%Y-%m-%d %H:%M:%S").unwrap();
    dbg!(dt.year());
    // dbg!(&r);
}

#[test]
fn test_get_item_ref_value() {
    let item_refno = "1RSETT0003T".to_string();
    let r = get_item_ref_value(item_refno, HoleType::STUCG);
    dbg!(&r);
}

#[tokio::test]
async fn test_gen_stucj_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    // let refno = RefU64::from_refno_str("24383/101196").unwrap();
    let mut instances = Vec::new();
    for i in 0..40 {
        if let Some(r) = query_hole_data_tidb(i, &pool).await {
            instances.push(r);
        }
    }
    let mut file = fs::File::create("孔洞.json")?;
    let data = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801-208".to_string(),
        instances,
    };
    let data = serde_json::to_string(&data).unwrap();
    file.write_all(&data.into_bytes())?;
    Ok(())
}

#[tokio::test]
async fn test_gen_stucj_data_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let keys = vec!["8DB55F00DF18E30-B32E-19".to_string()];
    let instances = gen_hole_datacenter_instance_aql(keys, &db_option.project_code, &database).await.unwrap_or_default();
    let mut file = fs::File::create("孔洞_aql.json")?;
    let data = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801-208".to_string(),
        instances,
    };
    let data = serde_json::to_string(&data).unwrap();
    file.write_all(&data.into_bytes())?;
    Ok(())
}