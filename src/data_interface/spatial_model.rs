// cal_zdis_pki

use aios_core::pdms_types::*;
use aios_core::prim_geo::spine::SweepPath3D;
use aios_core::shape::pdms_shape::LEN_TOL;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use glam::{Mat3, Quat, Vec3};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_inst_arango::query_insts_shape_data;

impl AiosDBManager {

    ///获得当前参考号下的模型前后方向关键点
    pub async fn get_along_end_key_points(&self, refno: RefU64) -> Option<(Vec3, Vec3)> {
        let database = self.get_arango_db().await.ok()?;
        //排除了负实体
        let inst_data = query_insts_shape_data(&database, &[refno], Some(&[GeoBasicType::Pos])).await.ok()?;
        if inst_data.inst_info_map.is_empty() { return None; }
        let mut whole_key_points = vec![];
        for (&refno, info) in &inst_data.inst_info_map {
            let Some(inst_geos) = inst_data.get_inst_geos(info) else {
                continue;
            };
            let key_points = inst_geos.iter()
                .map(|x| x.geo_param.key_points().into_iter().map(|v| x.transform.transform_point(*v)))
                .flatten()
                .map(|x| info.world_transform.transform_point(x))
                .collect::<Vec<_>>();
            whole_key_points.extend_from_slice(&key_points);
        }
        if whole_key_points.len() < 2 { return None; }
        whole_key_points.sort_by(|a, b| a.length().partial_cmp(&b.length()).unwrap());
        let final_key_points = (whole_key_points.first().cloned().unwrap(), whole_key_points.last().cloned().unwrap());
        // dbg!(&final_key_points);
        Some(final_key_points)
    }




}