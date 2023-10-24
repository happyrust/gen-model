use aios_core::AttrMap;
use aios_core::pdms_types::*;
use sqlx::{MySql, Pool};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::gen_item_code_data_attr_val;
use crate::pcf::pcf_api::{create_refno_data, create_s_key_data, create_weld_spec_data};

pub async fn gen_redu_data(aios_mgr: &AiosDBManager, attr: &AttrMap, pool: &Pool<MySql>, materials: &mut Vec<(RefU64, String)>) -> Vec<u8> {
    let mut data = vec![];
    let refno = attr.get_refno();
    if refno.is_none() { return vec![]; }
    let refno = refno.unwrap();
    data.append(&mut create_s_key_data(attr, aios_mgr).await);
    let spre = attr.get_val("SPRE");
    data.append(&mut gen_item_code_data_attr_val(spre, aios_mgr, materials).await);
    data.append(&mut create_weld_spec_data(attr, aios_mgr).await);
    data.append(&mut create_refno_data(attr));
    data
}