use aios_core::AttrMap;
use aios_core::pdms_types::*;
use sqlx::{MySql, Pool};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::{gen_item_code_data_attr_val, gen_type_name_data};
use crate::pcf::pcf_api::{create_center_point_data, create_cref_name_data, create_refno_data, create_s_key_data};

pub async fn gen_nozz_data(aios_mgr: &AiosDBManager, attr: &AttrMap, pool: &Pool<MySql>) -> Vec<u8> {
    let mut data = vec![];
    let refno = attr.get_refno();
    if refno.is_none() { return vec![]; }
    let refno = refno.unwrap();
    data.append(&mut gen_type_name_data("NOZZLE"));
    data.append(&mut create_center_point_data(refno, aios_mgr).await);
    data.append(&mut create_cref_name_data(attr, pool).await);
    data.append(&mut create_refno_data(attr));
    data
}