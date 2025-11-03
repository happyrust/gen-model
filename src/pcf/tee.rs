use std::task::Poll;
use aios_core::AttrMap;
use aios_core::pdms_types::*;
use aios_core::prim_geo::tubing::{TubiEdge, TubiSize};
use dashmap::DashMap;
use glam::Vec3;
use sqlx::{MySql, Pool};
use crate::api::attr::query_implicit_attr;
use crate::api::element::query_name;
use crate::aql_api::tubi::query_bran_info;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::{gen_endpoint_data, gen_item_code_data_attr_val, gen_type_name_data};
use crate::pcf::pcf_api::{create_center_point_data, create_thickness_data, create_pipeline_spec_data, create_refno_data, create_s_key_data, create_tee_item_code_bran_data, create_weld_spec_data, gen_s_key_data_str, get_s_key_value};

pub async fn gen_tee_data(aios_mgr: &AiosDBManager, attr: &AttrMap, bran_attr: &AttrMap,
                          pool: &Pool<MySql>, materials: &mut Vec<(RefU64, String)>,
                          start_edge: &TubiEdge, end_edge: &TubiEdge, thickness_map: &DashMap<String, DashMap<String, String>>) -> Vec<u8> {
    let mut data = vec![];
    let refno = attr.get_refno();
    if refno.is_none() { return vec![]; }
    let refno = refno.unwrap();
    let type_name = attr.get_type_str();
    let s_key = get_s_key_value(attr, aios_mgr, pool).await;
    let s_key = s_key.unwrap_or("".to_string());
    // TEE 的 SKEY 值为 TEST时 需要做特殊处理
    if s_key == "TESO" {
        data.append(&mut gen_type_name_data("TEE-SET-ON"));
        data.append(&mut create_center_point_data(refno, aios_mgr).await);
        data.append(&mut create_tee_set_on_branch_1_point_data(aios_mgr, refno).await);
        data.append(&mut gen_s_key_data_str("TESO"));
        data.append(&mut create_tee_item_code_bran_data(bran_attr, pool).await);
        data.append(&mut create_refno_data(attr));
    } else {
        data.append(&mut gen_type_name_data(type_name));
        if let TubiSize::BoreSize(bore) = start_edge.tubi_size {
            let start_point = start_edge.end_pt;
            data.append(&mut gen_endpoint_data(start_point, bore));
        }

        if let TubiSize::BoreSize(bore) = end_edge.tubi_size {
            let end_point = end_edge.start_pt;
            data.append(&mut gen_endpoint_data(end_point, bore));
        }
        data.append(&mut create_center_point_data(refno, aios_mgr).await);
        data.append(&mut create_tee_branch_point_data(aios_mgr, attr, pool).await);
        data.append(&mut gen_s_key_data_str(s_key.as_str()));
        let spre = attr.get_val("SPRE");
        data.append(&mut gen_item_code_data_attr_val(spre, aios_mgr, materials).await);
        data.append(&mut create_weld_spec_data(attr, aios_mgr).await);
        data.append(&mut create_refno_data(attr));
        data.append(&mut create_cref_thickness_data(attr, pool, thickness_map, false).await);
    }
    data
}

pub async fn create_tee_branch_point_data(aios_mgr: &AiosDBManager, attr: &AttrMap, pool: &Pool<MySql>) -> Vec<u8> {
    let refno = attr.get_refno().unwrap(); // 在调用本方法之前已经判断过 attr 中是否存在 refno
    if let Some(cref_refno) = attr.get_refu64("CREF") {
        let database = aios_mgr.get_arango_db().await;
        if database.is_err() { return vec![]; }
        let database = database.unwrap();
        let bran_infos = query_bran_info(cref_refno, &database).await;
        if bran_infos.is_err() { return vec![]; }
        let bran_infos = bran_infos.unwrap();
        let cref_cache = aios_mgr.get_refno_basic(cref_refno);
        if cref_cache.is_none() { return vec![]; }
        let cref_cache = cref_cache.unwrap();
        let cref_attr = query_implicit_attr(cref_refno, cref_cache.value(), pool, Some(vec!["HREF", "TREF"])).await;
        if cref_attr.is_err() { return vec![]; }
        let cref_attr = cref_attr.unwrap();
        // 判断 bran 是 头 / 尾 连接的 tee
        if let Some(href_refno) = cref_attr.get_refu64("HREF") {
            if refno == href_refno {
                if let Some(first_tubi) = bran_infos.first() {
                    return gen_branch_1_point_data_str(first_tubi.start_pt);
                }
            }
        }
        if let Some(tref_refno) = cref_attr.get_refu64("TREF") {
            if refno == tref_refno {
                if let Some(last_tubi) = bran_infos.last() {
                    return gen_branch_1_point_data_str(last_tubi.end_pt);
                }
            }
        }
    }
    vec![]
}

async fn create_tee_set_on_branch_1_point_data(aios_mgr: &AiosDBManager, refno: RefU64) -> Vec<u8> {
    let world_transform = aios_mgr.get_world_transform(refno).await;
    if let Ok(Some(world_transform)) = world_transform {
        return gen_branch_1_point_data_str(world_transform.translation);
    }
    vec![]
}

/// cref_thickness 分为 pipe 和 bran 两种 b_pipe就代表是pipe这一种
async fn create_cref_thickness_data(attr: &AttrMap, pool: &Pool<MySql>, thickness_map: &DashMap<String, DashMap<String, String>>, b_pipe: bool) -> Vec<u8> {
    let mut data = Vec::new();
    let cref = attr.get_refu64("CREF");
    if cref.is_none() { return data; }
    let cref = cref.unwrap();
    let cref_name = query_name(cref, pool).await;
    if cref_name.is_err() { return data; }
    let cref_name = cref_name.unwrap();
    create_thickness_data(&cref_name, thickness_map, b_pipe)
}

fn gen_branch_1_point_data_str(point: Vec3) -> Vec<u8> {
    format!("        BRANCH1-POINT  {}  {}  {}\r\n", point.x, point.y, point.z).into_bytes()
}