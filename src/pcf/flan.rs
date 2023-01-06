use aios_core::pdms_types::{AttrMap, RefU64};
use sqlx::{MySql, Pool};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::gen_item_code_data_attr_val;
use crate::pcf::pcf_api::{create_refno_data, create_s_key_data, create_weld_spec_data};

pub async fn gen_flan_data(aios_mgr:&AiosDBManager, attr: &AttrMap, pool: &Pool<MySql>,materials:&mut Vec<(RefU64,String)>) -> Vec<u8> {
    let mut data = vec![];
    data.append(&mut create_s_key_data(attr,aios_mgr,pool).await);
    let spre = attr.get_val("SPRE");
    data.append(&mut gen_item_code_data_attr_val(spre, &pool,materials).await);
    data.append(&mut create_weld_spec_data(attr, aios_mgr, pool).await);
    data.append(&mut create_refno_data(attr));
    data
}