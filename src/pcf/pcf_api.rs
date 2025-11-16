use aios_core::{AttrMap, AttrVal};
use aios_core::pdms_types::*;
use aios_core::prim_geo::tubing::{TubiEdge, TubiSize};
use itertools::Itertools;
use lazy_static::lazy_static;
use sqlx::{MySql, Pool};
use dashmap::{DashMap, DashSet};
use glam::Vec3;
use log::kv::ToValue;
use crate::api::attr::{query_explicit_attr, query_attr, query_implicit_attr};
use crate::api::element::query_name;
use aios_core::get_db_option;
use aios_core::db_pool::get_project_pool;
use crate::pcf::atta::gen_atta_data;
use crate::pcf::bend::gen_bend_data;
use crate::pcf::bran::{gen_center_point_data, gen_co_ords_data, gen_cords_point_data, gen_endpoint_data, gen_refno_data, gen_type_name_data};
use crate::pcf::cap::gen_cap_data;
use crate::pcf::coup::gen_coup_data;
use crate::pcf::elbo::gen_elbo_data;
use crate::pcf::flan::gen_flan_data;
use crate::pcf::gask::gen_gask_data;
use crate::pcf::inst::gen_inst_data;
use crate::pcf::olet::gen_olet_data;
use crate::pcf::redu::gen_redu_data;
use crate::pcf::tee::gen_tee_data;
use crate::pcf::valv::gen_valv_data;

lazy_static! {
    /// attr_map 中不需要转为 bytes的属性
    pub static ref PCF_NODES: DashSet<String> = {
        let mut set = DashSet::new();
        set.insert("ATTA".to_string());
        set.insert("ELBO".to_string());
        set.insert("FLAN".to_string());
        set.insert("GASK".to_string());
        set.insert("INST".to_string());
        set.insert("OLET".to_string());
        set.insert("REDU".to_string());
        set.insert("TEE".to_string());
        set.insert("VALV".to_string());
        set.insert("BEND".to_string());
        set.insert("CAP".to_string());
        set.insert("COUP".to_string());
        set
    };
}

/// 生成每个节点都存在的 pcf 数据 ，返回值为是否是cap 是cap就代表bran结束，后面的节点就不用执行了
pub async fn gen_node_basic_data(refno: RefU64, mut data: &mut Vec<u8>, mut materials: &mut Vec<(RefU64, String)>,
                                 bran_attr: &AttrMap, start_edge: &TubiEdge, thickness_map: &DashMap<String, DashMap<String, String>>,
                                 end_edge: &TubiEdge, aios_mgr: &AiosDBManager, pool: &Pool<MySql>) -> bool {
    let attr = query_attr(refno, &aios_mgr, None).await;
    if attr.is_err() { return false; }
    let attr = attr.unwrap();
    let type_name = attr.get_type_str();
    if !PCF_NODES.contains(type_name) { return false; }
    if !["ATTA", "TEE"].contains(&type_name) { // TEE 需要做特殊处理
        data.append(&mut gen_type_name_data(type_name));

        if let TubiSize::BoreSize(bore) = start_edge.tubi_size {
            let start_point = start_edge.end_pt;
            data.append(&mut gen_endpoint_data(start_point, bore));
        }

        if let TubiSize::BoreSize(bore) = end_edge.tubi_size {
            let end_point = end_edge.start_pt;
            data.append(&mut gen_endpoint_data(end_point, bore));
        }

    }
    match type_name {
        "ATTA" => { data.append(&mut gen_atta_data(aios_mgr, &attr, pool, materials).await); }
        "BEND" => { data.append(&mut gen_bend_data(aios_mgr, &attr, pool, materials).await); }
        "CAP" => {
            data.append(&mut gen_cap_data(aios_mgr, &attr, pool).await);
            // cap 是管套 代表一个bran结束
            return true;
        }
        "ELBO" => { data.append(&mut gen_elbo_data(aios_mgr, &attr, pool, materials).await); }
        "GASK" => { data.append(&mut gen_gask_data(aios_mgr, &attr, pool, materials).await); }
        "FLAN" => { data.append(&mut gen_flan_data(aios_mgr, &attr, pool, materials).await); }
        "VALV" => { data.append(&mut gen_valv_data(aios_mgr, &attr, pool, materials).await); }
        "TEE" => { data.append(&mut gen_tee_data(aios_mgr, &attr, bran_attr, pool, materials, start_edge, end_edge, thickness_map).await); }
        "REDU" => { data.append(&mut gen_redu_data(aios_mgr, &attr, pool, materials).await); }
        "INST" => { data.append(&mut gen_inst_data(aios_mgr, &attr, pool, materials).await); }
        "OLET" => { data.append(&mut gen_olet_data(aios_mgr, &attr, pool, materials).await); }
        "COUP" => { data.append(&mut gen_coup_data(aios_mgr, &attr, pool, materials).await); }
        _ => {}
    }
    false
}

/// 生成 center_point 数据
pub async fn create_center_point_data(refno: RefU64, aios_mgr: &AiosDBManager) -> Vec<u8> {
    let center_point = aios_mgr.get_world_transform(refno).await;
    if let Ok(Some(center_point)) = center_point {
        return gen_center_point_data(center_point.translation);
    }
    vec![]
}

/// 生成 cords_point 数据
pub async fn create_cords_point_data(refno: RefU64, aios_mgr: &AiosDBManager) -> Vec<u8> {
    let center_point = aios_mgr.get_world_transform(refno).await;
    if let Ok(Some(center_point)) = center_point {
        return gen_cords_point_data(center_point.translation);
    }
    vec![]
}

/// 生成 SKEY 数据
pub async fn create_s_key_data(attr: &AttrMap, aios_mgr: &AiosDBManager) -> Vec<u8> {
    let spre_refno = attr.get_refu64("SPRE");
    if spre_refno.is_none() { return vec![]; }
    let spre_refno = spre_refno.unwrap();
    let spre_cache = aios_mgr.get_refno_basic(spre_refno);
    if spre_cache.is_none() { return vec![]; }
    let spre_cache = spre_cache.unwrap();
    let spre_pool = aios_mgr.get_project_pool_by_refno(spre_refno).await;
    if spre_pool.is_none() { return vec![]; }
    let (project, spre_pool) = spre_pool.unwrap();
    let spre_attr = query_implicit_attr(spre_refno, spre_cache.value(), &spre_pool, Some(vec!["DETR"])).await;
    if spre_attr.is_err() { return vec![]; }
    let spre_attr = spre_attr.unwrap();
    let detr_refno = spre_attr.get_refu64("DETR");
    if let Some(detr_refno) = detr_refno {
        let detr_pool = aios_mgr.get_project_pool_by_refno(detr_refno).await;
        if detr_pool.is_none() { return vec![]; }
        let (project, detr_pool) = detr_pool.unwrap();
        if let Some(cache) = aios_mgr.get_refno_basic(detr_refno) {
            let detr_att = query_implicit_attr(detr_refno, cache.value(), &detr_pool, Some(vec!["SKEY"])).await;
            if let Ok(detr_att) = detr_att {
                let s_key = detr_att.get_str("SKEY");
                if let Some(s_key) = s_key {
                    return gen_s_key_data_str(s_key);
                }
            }
        }
    }
    vec![]
}

pub async fn get_s_key_value(attr: &AttrMap, aios_mgr: &AiosDBManager, pool: &Pool<MySql>) -> Option<String> {
    let spre_refno = attr.get_refu64("SPRE");
    if spre_refno.is_none() { return None; }
    let spre_refno = spre_refno.unwrap();
    let spre_cache = aios_mgr.get_refno_basic(spre_refno);
    if spre_cache.is_none() { return None; }
    let spre_cache = spre_cache.unwrap();
    let spre_attr = query_implicit_attr(spre_refno, spre_cache.value(), pool, Some(vec!["DETR"])).await;
    if spre_attr.is_err() { return None; }
    let spre_attr = spre_attr.unwrap();
    let detr_refno = spre_attr.get_refu64("DETR");
    if let Some(detr_refno) = detr_refno {
        if let Some(cache) = aios_mgr.get_refno_basic(detr_refno) {
            let detr_att = query_implicit_attr(detr_refno, cache.value(), pool, Some(vec!["SKEY"])).await;
            if let Ok(detr_att) = detr_att {
                let s_key = detr_att.get_str("SKEY");
                if let Some(s_key) = s_key {
                    return Some(s_key.to_string());
                }
            }
        }
    }
    None
}

/// 生成 ANGL 数据
pub fn create_angl_data(attr: &AttrMap) -> Vec<u8> {
    let angle = attr.get_val("ANGL");
    if let Some(AttrVal::DoubleType(angl)) = angle {
        return gen_angl_data_str(*angl);
    }
    vec![]
}

pub fn create_refno_data(attr: &AttrMap) -> Vec<u8> {
    if let Some(refno) = attr.get_refno() {
        return gen_refno_data(refno);
    }
    vec![]
}

pub fn create_temperature_data(temp: f64) -> Vec<u8> {
    gen_temperature_data_str(temp)
}

pub fn create_s_text_data(attr: &AttrMap) -> Vec<u8> {
    if let Some(s_text) = attr.get_str("STEX") {
        return gen_s_text_str(s_text);
    }
    vec![]
}

pub async fn create_pipeline_spec_data(attr: &AttrMap, pool: &Pool<MySql>) -> Vec<u8> {
    if let Some(pspe_refno) = attr.get_refu64("PSPE") {
        let pspe_name = query_name(pspe_refno, pool).await;
        if let Ok(name) = pspe_name {
            return gen_pipeline_spec_str(&name);
        }
    }
    vec![]
}


pub async fn create_tee_item_code_bran_data(attr: &AttrMap, pool: &Pool<MySql>) -> Vec<u8> {
    if let Some(pspe_refno) = attr.get_refu64("PSPE") {
        let pspe_name = query_name(pspe_refno, pool).await;
        if let Ok(name) = pspe_name {
            return gen_tee_item_code_bran_data_str(&name);
        }
    }
    vec![]
}

pub fn create_pipeline_href_data(attr: &AttrMap) -> Vec<u8> {
    if let Some(href_refno) = attr.get_refu64("HREF") {
        return gen_pipeline_href_str(href_refno);
    }
    vec![]
}

pub fn create_pipeline_tref_data(attr: &AttrMap) -> Vec<u8> {
    if let Some(tref_refno) = attr.get_refu64("TREF") {
        return gen_pipeline_tref_str(tref_refno);
    }
    vec![]
}

pub async fn create_cref_name_data(attr: &AttrMap, pool: &Pool<MySql>) -> Vec<u8> {
    let cref = attr.get_refu64("CREF");
    if cref.is_none() { return Vec::new(); }
    let cref = cref.unwrap();
    let cref_name = query_name(cref, pool).await;
    if cref_name.is_err() { return Vec::new(); }
    let cref_name = cref_name.unwrap();
    gen_cref_name_str(&cref_name)
}

pub fn create_end_position_null_data(position: Vec3) -> Vec<u8> {
    let mut data = Vec::new();
    data.append(&mut gen_end_connection_null_head_data());
    data.append(&mut gen_co_ords_data(position));
    data
}

pub async fn create_weld_spec_data(attr: &AttrMap, aios_mgr: &AiosDBManager) -> Vec<u8> {
    let mut data = Vec::new();
    let spre = attr.get_refu64("SPRE");
    if spre.is_none() { return data; }
    let spre = spre.unwrap();

    let spre_cache = aios_mgr.get_refno_basic(spre);
    if spre_cache.is_none() { return data; }
    let spre_cache = spre_cache.unwrap();
    let spre_pool = aios_mgr.get_project_pool_by_refno(spre).await;
    if spre_pool.is_none() { return data; }
    let (_, spre_pool) = spre_pool.unwrap();
    let spre_attr = query_implicit_attr(spre, spre_cache.value(), &spre_pool, Some(vec!["DETR"])).await;
    if spre_attr.is_err() { return data; }
    let spre_attr = spre_attr.unwrap();

    let detr = spre_attr.get_refu64("DETR");
    if detr.is_none() { return data; }
    let detr = detr.unwrap();
    let detr_pool = aios_mgr.get_project_pool_by_refno(detr).await;
    if detr_pool.is_none() { return data; }
    let (_, detr_pool) = detr_pool.unwrap();
    let detr_attr = query_explicit_attr(detr, &detr_pool).await;
    if detr_attr.is_err() { return data; }
    let detr_attr = detr_attr.unwrap();

    let mut weld_spec = "";
    let r_text = detr_attr.get_str("RTEX").unwrap_or("");
    if r_text.contains("BW") {
        weld_spec = "BW";
    } else if r_text.contains("RF") {
        let r_text_splits = r_text.split(" ").collect::<Vec<_>>();
        for r_text_split in r_text_splits {
            if r_text_split.contains("RF/") {
                weld_spec = r_text_split;
                break;
            }
        }
    }
    data.append(&mut gen_weld_spec_str(weld_spec));
    data
}

/// b_pipe 分为 bran_thickness 和 pipe_thickness 两种 取数据方式一样，最后输出的文字不同
pub fn create_thickness_data(name: &str, thickness_map: &DashMap<String, DashMap<String, String>>, b_pipe: bool) -> Vec<u8> {
    let mut data = Vec::new();
    let mut od = "".to_string();
    let mut thick = "".to_string();
    let name_splits = name.split("-").collect::<Vec<_>>();
    if name_splits.len() >= 5 && !thickness_map.is_empty() {
        let dn = name_splits[3];
        let key = name_splits[4];
        let key = &key[0..1];
        if let Some(dn) = thickness_map.get(dn) {
            let dn = dn.value();
            if let Some(value) = dn.get(key) {
                let value = value.split("x").collect::<Vec<_>>();
                if value.len() > 1 {
                    od = value[0].to_string();
                    thick = value[1].to_string();
                }
            }
        }
    }
    if b_pipe {
        data.append(&mut gen_pipe_od_str(&od));
        data.append(&mut gen_pipe_thick_str(&thick));
    } else {
        data.append(&mut gen_bran_od_str(&od));
        data.append(&mut gen_bran_thick_str(&thick));
    }
    data
}

/// 生成 SKEY pcf 的数据
pub fn gen_s_key_data_str(s_key: &str) -> Vec<u8> {
    format!("        SKEY  {}\r\n", s_key).into_bytes()
}

fn gen_angl_data_str(angl: f64) -> Vec<u8> {
    format!("        ANGLE        {}\r\n", angl).into_bytes()
}

fn gen_temperature_data_str(temp: f64) -> Vec<u8> {
    format!("        PIPELINE-TEMP       {}\r\n", temp).into_bytes()
}

fn gen_pipeline_spec_str(pspe_name: &str) -> Vec<u8> {
    format!("        PIPING-SPEC    {}\r\n", pspe_name).into_bytes()
}

fn gen_pipeline_href_str(href: RefU64) -> Vec<u8> {
    format!("        ID-HREF    {}\r\n", href.to_string()).into_bytes()
}

fn gen_pipeline_tref_str(tref: RefU64) -> Vec<u8> {
    format!("        ID-TREF    {}\r\n", tref.to_string()).into_bytes()
}

fn gen_tee_item_code_bran_data_str(pspe_name: &str) -> Vec<u8> {
    format!("        ITEM-CODE-BRANCH1    {}\r\n", pspe_name).into_bytes()
}

fn gen_s_text_str(s_text: &str) -> Vec<u8> {
    format!("        SUPPORT-TYPE    {}\r\n", s_text).into_bytes()
}

fn gen_cref_name_str(cref_name: &str) -> Vec<u8> {
    format!("        CONNECTION-REFERENCE    {}\r\n", cref_name).into_bytes()
}

fn gen_end_connection_null_head_data() -> Vec<u8> {
    format!("END-POSITION-NULL\r\n").into_bytes()
}

fn gen_weld_spec_str(weld_spec: &str) -> Vec<u8> {
    format!("        WELD-SPEC  {}\r\n", weld_spec).into_bytes()
}

fn gen_pipe_od_str(od: &str) -> Vec<u8> {
    format!("        PIPE-OD  {}\r\n", od).into_bytes()
}

fn gen_pipe_thick_str(thick: &str) -> Vec<u8> {
    format!("        PIPE-THICK  {}\r\n", thick).into_bytes()
}

fn gen_bran_od_str(od: &str) -> Vec<u8> {
    format!("        BRANCH1-OD  {}\r\n", od).into_bytes()
}

fn gen_bran_thick_str(thick: &str) -> Vec<u8> {
    format!("        BRANCH1-THICK  {}\r\n", thick).into_bytes()
}