use aios_core::AttrMap;
use aios_core::pdms_types::*;
use aios_core::prim_geo::tubing::TubiSize;
use dashmap::DashMap;
use glam::Vec3;
use sqlx::{MySql, Pool};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::pcf::bran::{gen_endpoint_data, gen_item_code_data_attr_val, gen_refno_data, gen_refno_data_pipe};

pub async fn gen_tubi_data(start_point: Vec3,
                           end_point: Vec3,
                           tubi_size: TubiSize,
                           bran_attr: &AttrMap,
                           from_refno: Option<RefU64>,
                           materials:&mut Vec<(RefU64,String)>,
                           pipe_thickness_data:&Vec<u8>,
                           aios_mgr:&AiosDBManager) -> Vec<u8> {

    let mut pipe_data = Vec::new();

    if let TubiSize::BoreSize(bore) = tubi_size {
        pipe_data.append(&mut "PIPE \r\n".to_string().into_bytes());
        pipe_data.append(&mut gen_endpoint_data(start_point, bore));
        pipe_data.append(&mut gen_endpoint_data(end_point, bore));
        let hstu_refno = bran_attr.get_val("HSTU");
        pipe_data.append(&mut gen_item_code_data_attr_val(hstu_refno, aios_mgr,materials).await);
        if let Some(from_refno) = from_refno {
            pipe_data.append(&mut gen_refno_data_pipe(from_refno));
        }
        pipe_data.append(&mut pipe_thickness_data.clone());
    }

    pipe_data
}

