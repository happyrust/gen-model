use std::io::Write;
use std::sync::Arc;
use aios_core::{AttrMap, AttrVal};
use aios_core::pdms_types::*;
use aios_core::prim_geo::tubing::TubiEdge;


use dashmap::DashMap;
use glam::Vec3;
use parse_pdms_db::parse_explict_tools::times_keep_f32_two_decimal_place;
use sqlx::{MySql, Pool};
use crate::api::attr::{query_attr, query_explicit_attr, query_implicit_attr};
use crate::api::element::{query_children, query_children_eles, query_name};
use crate::aql_api::tubi::query_bran_info;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::elbo::gen_elbo_data;
use crate::pcf::excel_api::get_pipe_thickness_table;
use crate::pcf::flan::gen_flan_data;
use crate::pcf::gask::gen_gask_data;
use crate::pcf::nozz::gen_nozz_data;
use crate::pcf::pcf_api::{create_end_position_null_data, create_pipeline_href_data, create_pipeline_spec_data, create_pipeline_tref_data, create_refno_data, create_temperature_data, create_thickness_data, gen_node_basic_data};
use crate::pcf::tee::gen_tee_data;
use crate::pcf::tubi::gen_tubi_data;
use crate::pcf::valv::gen_valv_data;


fn gen_pcf_file_head() -> String {
    "ISOGEN-FILES            ISOGEN.FLS
     UNITS-BORE              INCH
     UNITS-CO-ORDS           MM
     UNITS-BOLT-LENGTH       MM
     UNITS-BOLT-DIA          INCH
     UNITS-WEIGHT            KGS\r\n".to_string()
}

pub async fn get_bran_name_and_children(refno: RefU64, aios_mgr: &AiosDBManager,
                                        thickness_map: &DashMap<String, DashMap<String, String>>) -> anyhow::Result<Vec<u8>> {
    let mut data = vec![];
    let mut materials = vec![];
    data.append(&mut gen_pcf_file_head().into_bytes());
    let pool = aios_mgr.project_map.get(&aios_mgr.db_option.project_name).unwrap();
    let database = aios_mgr.get_arango_db().await?;
    // let bran_attr = query_attr(refno, &aios_mgr, None).await?;
    let bran_attr = aios_core::get_named_attmap(refno).await?;
    let bran_name = bran_attr.get_name_or_default();

    // 先把 pipe_thickness 算好，需要的直接放进去就好了
    let pipe_refno = aios_mgr.get_owner(refno);
    let pipe_name = query_name(pipe_refno, pool.value()).await?;
    let pipe_cache = aios_mgr.get_refno_basic(pipe_refno);
    if pipe_cache.is_none() { return Ok(data); }
    let pipe_cache = pipe_cache.unwrap();
    let pipe_temp = query_implicit_attr(pipe_refno,pipe_cache.value(),pool.value(),Some(vec!["TEMP"])).await.unwrap_or_default();
    let pipe_temp = pipe_temp.get_f64("TEMP").unwrap_or(-100000.0);
    let pipe_thickness_data = create_thickness_data(&pipe_name, thickness_map, true);
    // 查找 bran带tubi和元件 的数据
    let bran_infos = query_bran_info(refno, &database).await?;
    // 生成 bran href 的数据
    if let Some(start_position) = bran_infos.first() {
        let start_position = start_position.start_pt;
        data.append(&mut gen_bran_pipeline_reference_data(&bran_attr, start_position, pool.value(), &pipe_thickness_data,pipe_temp).await);
        let href = bran_attr.get_refu64("HREF");
        data.append(&mut gen_bran_connection_data(href, start_position, aios_mgr, pool.value()).await);
    }
    // 生成 bran 中间节点的数据
    for i in 0..bran_infos.len() {
        // 可能存在 tubi
        let tubi_start_index = if i == 0 { 1 } else { i };
        // 如果 tubi_edge 的 att_type 是 ATTA ， 统计tubi的时候就需要跳过下一个tubi
        if bran_infos[tubi_start_index - 1].att_type != "ATTA" {
            let mut tubi_end_index = i;
            let distance = bran_infos[i].start_pt.distance(bran_infos[tubi_end_index].end_pt);
            if distance >= TUBI_TOL {
                // 跳过 atta ，不记录 atta的长度
                if bran_infos[i].att_type == "ATTA" {
                    while tubi_end_index < bran_infos.len() - 1 {
                        if bran_infos[tubi_end_index].att_type == "ATTA" { tubi_end_index += 1; } else { break; }
                    }
                }
                let from_refno = RefU64::from_str(&bran_infos[i]._from);
                let mut tubi_data = gen_tubi_data(bran_infos[i].start_pt, bran_infos[tubi_end_index].end_pt,
                                                  bran_infos[tubi_end_index].tubi_size, &bran_attr, from_refno,
                                                  &mut materials, &pipe_thickness_data, aios_mgr).await;
                data.append(&mut tubi_data);
            }
        }
        // 生成 bran 元件数据
        if i == bran_infos.len() - 1 { continue; }
        let refno = convert_refno_from_edge_str(&bran_infos[i]._to);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        if gen_node_basic_data(refno, &mut data, &mut materials, &bran_attr,
                               &bran_infos[i], thickness_map, &bran_infos[i + 1],
                               aios_mgr, pool.value()).await {
            break;
        }
    }
    // 生成 bran tref 的数据
    if let Some(leave_position) = bran_infos.last() {
        let leave_position = leave_position.end_pt;
        let tref = bran_attr.get_refu64("TREF");
        data.append(&mut gen_bran_connection_data(tref, leave_position, aios_mgr, pool.value()).await);
    }
    // 生成 material 数据
    data.append(&mut gen_material_data(materials, aios_mgr, pool.value()).await);
    Ok(data)
}

pub async fn gen_bran_pipeline_reference_data(attr: &AttrMap, start_position: Vec3, pool: &Pool<MySql>,
                                              pipe_thickness_data: &Vec<u8>,pipe_temp:f64) -> Vec<u8> {
    let mut data = vec![];
    let name = attr.get_name_or_default();
    data.append(&mut gen_pipeline_reference_data_str_head(name.as_str()));
    data.append(&mut gen_start_co_ords_data(start_position));
    data.append(&mut create_temperature_data(pipe_temp));
    data.append(&mut create_pipeline_spec_data(attr, pool).await);
    data.append(&mut create_refno_data(attr));
    data.append(&mut create_pipeline_href_data(attr));
    data.append(&mut create_pipeline_tref_data(attr));
    data.append(&mut pipe_thickness_data.clone());
    data
}

pub async fn gen_bran_connection_data(refno: Option<RefU64>, leave_position: Vec3, aios_mgr: &AiosDBManager, pool: &Pool<MySql>) -> Vec<u8> {
    let mut data = vec![];
    if let Some(refno) = refno {
        if refno == RefU64(0) { return create_end_position_null_data(leave_position); }
        let refno_table_name = aios_mgr.get_refno_basic(refno);
        if refno_table_name.is_none() { return data; }
        let refno_table_name = refno_table_name.unwrap();
        // 如果连接的是 nozz ， 则是另一种取数据方式
        if refno_table_name.table.to_uppercase() == "NOZZ" {
            let nozz_attr = query_attr(refno, aios_mgr, Some(vec!["CREF"])).await;
            if let Ok(nozz_attr) = nozz_attr {
                data.append(&mut gen_nozz_data(aios_mgr, &nozz_attr, pool).await);
            }
        } else {
            data.append(&mut gen_end_connection_pipeline_head_data());
            data.append(&mut gen_co_ords_data(leave_position));
            let name = query_name(refno, pool).await.unwrap_or("".to_string());
            data.append(&mut gen_pipeline_reference_data_str(&name));
        }
    } else {
        data.append(&mut create_end_position_null_data(leave_position));
    }
    data
}

async fn gen_material_data(materials: Vec<(RefU64, String)>, aios_mgr: &AiosDBManager, pool: &Pool<MySql>) -> Vec<u8> {
    let mut data = Vec::new();
    data.push(gen_material_head_data());
    for (spre_refno, spre_name) in materials {
        data.push(gen_item_code_data_name(&spre_name));
        let spre_cache = aios_mgr.get_refno_basic(spre_refno);
        if spre_cache.is_err() { continue; }
        let spre_cache = spre_cache.unwrap();
        let spre_attr = query_implicit_attr(spre_refno, spre_cache.value(), pool, Some(vec!["DETR"])).await;
        if spre_attr.is_err() { continue; }
        let spre_attr = spre_attr.unwrap();
        let detr_refno = spre_attr.get_refu64("DETR");
        if detr_refno.is_err() { continue; }
        let detr_refno = detr_refno.unwrap();
        // dbg!(&detr_refno);
        let detr_cache = aios_mgr.get_refno_basic(detr_refno);
        if detr_cache.is_err() { continue; }
        let detr_cache = detr_cache.unwrap();
        let detr_attr = query_explicit_attr(detr_refno, pool).await;
        if detr_attr.is_err() { continue; }
        let detr_attr = detr_attr.unwrap();
        let r_text = detr_attr.get_str("RTEX");
        if let Some(r_text) = r_text {
            data.push(gen_material_item_code_description(r_text));
        }
    }
    data.into_iter().flatten().collect()
}

fn match_type_name(input: &str) -> &str {
    match input {
        "ATTA" => { "SUPPORT" }
        "GASK" => { "GASKET" }
        "FLAN" => { "FLANGE" }
        "ELBO" => { "ELBOW" }
        "VALV" => { "VALVE" }
        "REDU" => { "REDUCER-CONCENTRIC" }
        "INST" => { "INSTRUMENT" }
        _ => { input }
    }
}

/// 生成 pipeline_reference 数据
fn gen_bran_reference_data(bran_name: &str) -> Vec<u8> {
    format!("PIPELINE-REFERENCE      {}\r\n", bran_name).into_bytes()
}

/// 生成 pcf type 名
pub fn gen_type_name_data(type_name: &str) -> Vec<u8> {
    let type_name = match_type_name(type_name);
    format!("{}\r\n", type_name).into_bytes()
}

/// 生成 end_point 得 pcf 数据
pub fn gen_endpoint_data(point: Vec3, bore: f32) -> Vec<u8> {
    format!("        END-POINT    {}  {}  {} {}\r\n", point.x, point.y, point.z, bore).into_bytes()
}

/// 生成 center_point 得 pcf 数据
pub fn gen_center_point_data(center_point: Vec3) -> Vec<u8> {
    format!("        CENTRE-POINT    {}  {}  {}\r\n", center_point.x, center_point.y, center_point.z).into_bytes()
}

pub fn gen_cords_point_data(cords_point: Vec3) -> Vec<u8> {
    format!("        CO-ORDS    {}  {}  {}\r\n", cords_point.x, cords_point.y, cords_point.z).into_bytes()
}

pub async fn gen_item_code_data_attr_val(spre_refno: Option<&AttrVal>, aios_mgr: &AiosDBManager, materials: &mut Vec<(RefU64, String)>) -> Vec<u8> {
    if let Some(spre) = spre_refno {
        let spre_refno = spre.refno_value();
        if let Some(spre_refno) = spre_refno {
            let spre_pool = aios_mgr.get_project_pool_by_refno(spre_refno).await;
            if spre_pool.is_none() { return vec![]; }
            let (_, spre_pool) = spre_pool.unwrap();
            let spre_name = query_name(spre_refno, &spre_pool).await;
            return if let Ok(spre_name) = spre_name {
                if !materials.contains(&(spre_refno, spre_name.clone())) {
                    materials.push((spre_refno, spre_name.clone()));
                }
                format!("        ITEM-CODE    {}\r\n", spre_name).into_bytes()
            } else {
                vec![]
            };
        }
    }
    vec![]
}

pub async fn gen_item_code_data_refno(spre_refno: RefU64, pool: &Pool<MySql>) -> Vec<u8> {
    let spre_name = query_name(spre_refno, pool).await;
    return if let Ok(spre_name) = spre_name {
        format!("        ITEM-CODE    {}\r\n", spre_name).into_bytes()
    } else {
        vec![]
    };
}

pub fn gen_item_code_data_name(spre_name: &str) -> Vec<u8> {
    format!("ITEM-CODE    {}\r\n", spre_name).into_bytes()
}

/// 生成 ID-REF-NO 的数据
pub fn gen_refno_data(refno: RefU64) -> Vec<u8> {
    format!("        ID-REF-NO    {}\r\n", refno.to_string()).into_bytes()
}

pub fn gen_refno_data_pipe(refno: RefU64) -> Vec<u8> {
    format!("        ID-REF-NO    {}-1\r\n", refno.to_string()).into_bytes()
}

/// 从图数据库的 edge 的 _from / _to 数据转换成 RefU64
fn convert_refno_from_edge_str(refno_str: &str) -> Option<RefU64> {
    let refno_str = refno_str.split("/").collect::<Vec<_>>();
    if refno_str.len() <= 1 { return None; }
    let refno_str = refno_str[1];
    RefU64::from_str(refno_str)
}


fn gen_end_connection_pipeline_head_data() -> Vec<u8> {
    format!("END-CONNECTION-PIPELINE\r\n").into_bytes()
}

fn gen_start_co_ords_data(position: Vec3) -> Vec<u8> {
    format!("        START-CO-ORDS  {}  {}  {}\r\n", position.x, position.y, position.z).into_bytes()
}

pub fn gen_co_ords_data(position: Vec3) -> Vec<u8> {
    format!("        CO-ORDS  {}  {}  {}\r\n", position.x, position.y, position.z).into_bytes()
}

fn gen_pipeline_reference_data_str(name: &str) -> Vec<u8> {
    format!("        PIPELINE-REFERENCE  {}\r\n", name).into_bytes()
}

fn gen_pipeline_reference_data_str_head(name: &str) -> Vec<u8> {
    format!("PIPELINE-REFERENCE  {}\r\n", name).into_bytes()
}

fn gen_material_head_data() -> Vec<u8> {
    format!("MATERIALS\r\n").into_bytes()
}

fn gen_material_item_code_description(desc: &str) -> Vec<u8> { format!("        DESCRIPTION    {}\r\n", desc).into_bytes() }
