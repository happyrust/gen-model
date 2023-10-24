use aios_core::AttrMap;
use aios_core::pdms_types::*;
use sqlx::{MySql, Pool};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::gen_item_code_data_attr_val;
use crate::pcf::elbo::{create_radius_data, get_catr_para_data_from_spre};
use crate::pcf::pcf_api::{create_angl_data, create_center_point_data, create_refno_data, create_s_key_data, create_weld_spec_data};
use crate::pcf::tee::create_tee_branch_point_data;

pub async fn gen_bend_data(aios_mgr: &AiosDBManager, attr: &AttrMap, pool: &Pool<MySql>, materials: &mut Vec<(RefU64, String)>) -> Vec<u8> {
    let mut data = vec![];
    let refno = attr.get_refno();
    if refno.is_none() { return vec![]; }
    let refno = refno.unwrap();
    let spre = attr.get_val("SPRE");
    data.append(&mut create_center_point_data(refno, aios_mgr).await);
    data.append(&mut create_s_key_data(attr, aios_mgr).await);
    // data.append(&mut get_catr_para_data_from_spre(spre, &aios_mgr, pool).await); // BEND-RADIUS
    data.append(&mut create_radius_data(attr));
    data.append(&mut create_angl_data(attr));
    data.append(&mut gen_item_code_data_attr_val(spre, aios_mgr, materials).await);
    data.append(&mut create_weld_spec_data(attr, aios_mgr).await);
    data.append(&mut create_refno_data(attr));
    data
}