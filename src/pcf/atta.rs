use aios_core::AttrMap;
use aios_core::pdms_types::*;
use sqlx::{MySql, Pool};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::{gen_item_code_data_attr_val, gen_type_name_data};
use crate::pcf::elbo::get_catr_para_data_from_spre;
use crate::pcf::pcf_api::{create_angl_data, create_center_point_data, create_cords_point_data, create_refno_data, create_s_key_data, create_s_text_data};

pub async fn gen_atta_data(aios_mgr: &AiosDBManager, attr: &AttrMap, pool: &Pool<MySql>, materials: &mut Vec<(RefU64, String)>) -> Vec<u8> {
    let mut data = vec![];
    let refno = attr.get_refno();
    if refno.is_none() { return vec![]; }
    let refno = refno.unwrap();
    let spre = attr.get_val("SPRE");
    data.append(&mut gen_type_name_data(attr.get_type()));
    data.append(&mut create_cords_point_data(refno, aios_mgr).await);
    data.append(&mut create_s_key_data(attr, aios_mgr).await);
    data.append(&mut gen_item_code_data_attr_val(spre, &aios_mgr, materials).await);
    data.append(&mut create_refno_data(attr));
    data.append(&mut create_s_text_data(attr));
    data
}