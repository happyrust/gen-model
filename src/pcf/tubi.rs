use aios_core::pdms_types::{AttrMap, RefU64};
use glam::Vec3;
use sqlx::{MySql, Pool};
use crate::pcf::bran::{gen_endpoint_data, gen_item_code_data_attr_val, gen_refno_data, gen_refno_data_pipe};

pub async fn gen_tubi_data(start_point: Vec3, end_point: Vec3, bore: f32,
                           bran_attr: &AttrMap, from_refno: Option<RefU64>, pool: &Pool<MySql>,materials:&mut Vec<(RefU64,String)>) -> Vec<u8> {
    let mut pipe_data = Vec::new();
    pipe_data.push("PIPE \r\n".to_string().into_bytes());
    pipe_data.push(gen_endpoint_data(start_point, bore));
    pipe_data.push(gen_endpoint_data(end_point, bore));
    let hstu_refno = bran_attr.get_val("HSTU");
    pipe_data.push(gen_item_code_data_attr_val(hstu_refno, pool,materials).await);
    if let Some(from_refno) = from_refno {
        pipe_data.push(gen_refno_data_pipe(from_refno));
    }
    pipe_data.into_iter().flatten().collect()
}

