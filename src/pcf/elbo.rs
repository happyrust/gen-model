use aios_core::{AttrMap, AttrVal};
use aios_core::pdms_types::*;
use sqlx::{MySql, Pool};
use crate::api::attr::{query_explicit_attr, query_implicit_attr};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::{gen_center_point_data, gen_item_code_data_attr_val, gen_refno_data};
use crate::pcf::pcf_api::{create_angl_data, create_center_point_data, create_s_key_data, create_weld_spec_data};

/// 生成 elbo 特有的 pcf 数据
pub async fn gen_elbo_data(aios_mgr: &AiosDBManager, attr: &AttrMap, pool: &Pool<MySql>, materials: &mut Vec<(RefU64, String)>) -> Vec<u8> {
    let mut data = vec![];
    let refno = attr.get_refno();
    if refno.is_none() { return vec![]; }
    let refno = refno.unwrap();
    data.append(&mut create_center_point_data(refno, aios_mgr).await);
    data.append(&mut create_s_key_data(attr, aios_mgr).await);
    data.append(&mut create_angl_data(attr));
    let spre = attr.get_val("SPRE");
    data.append(&mut gen_item_code_data_attr_val(spre, aios_mgr, materials).await);
    data.append(&mut create_weld_spec_data(attr, aios_mgr).await);
    data.append(&mut gen_refno_data(refno));
    data.append(&mut get_catr_para_data_from_spre(spre, &aios_mgr, pool).await);
    data
}

pub async fn get_catr_para_data_from_spre(spre: Option<&AttrVal>, aios_mgr: &AiosDBManager, pool: &Pool<MySql>) -> Vec<u8> {
    if let Some(AttrVal::RefU64Type(spre_refno)) = spre {
        let spre_refno = *spre_refno;
        let cache_basic = aios_mgr.get_refno_basic(spre_refno);
        if let Some(cache_basic) = cache_basic {
            let spre_attr = query_implicit_attr(spre_refno, cache_basic.value(), pool, Some(vec!["CATR"])).await;
            if let Ok(spre_attr) = spre_attr {
                let catr = spre_attr.get_refu64("CATR");
                if let Some(catr) = catr {
                    let catr_attr = query_explicit_attr(catr, pool).await;
                    if let Ok(catr_attr) = catr_attr {
                        let para = catr_attr.get_val("PARA");
                        if let Some(AttrVal::DoubleArrayType(paras)) = para {
                            if let Some(para) = paras.get(1) {
                                return gen_radius_data(*para);
                            }
                        }
                    }
                }
            }
        }
    }
    vec![]
}


/// 生成 radius 数据 ( 直接从desi的attr里面取的radius )
pub fn create_radius_data(attr: &AttrMap) -> Vec<u8> {
    if let Some(radius) = attr.get_f64("RADI") {
        return gen_radius_data(radius);
    }
    vec![]
}

fn gen_radius_data(radius: f64) -> Vec<u8> { format!("        BEND-RADIUS  {}\r\n", radius).into_bytes() }