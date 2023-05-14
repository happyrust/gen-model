use std::sync::Arc;
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use bevy::prelude::Vec3;
use crate::api::attr::query_implicit_attr;
use crate::api::element::query_refno_type;
use crate::aql_api::children::{query_children_aql, query_travel_children_aql};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool};
use crate::graph_db::pdms_arango::save_virtual_hole_value_to_arangodb;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CylinderTransform {
    pub refno: String,
    pub pos_up: glam::Vec3,
    pub pos_down: glam::Vec3,
    pub ori_up: glam::Vec3,
    pub ori_down: glam::Vec3,
}

pub async fn query_cylinder_transform(refno: RefU64, mgr: Arc<AiosDBManager>) -> anyhow::Result<Vec<CylinderTransform>> {
    let mut result = vec![];
    if let Some((_, project_db)) = mgr.get_project_pool_by_refno(refno).await {
        let att_type = query_refno_type(refno, &project_db).await?;
        if att_type != "EQUI" { return Ok(vec![]); }
        // 获得设备下的 cyli
        let children = query_travel_children_aql(mgr.get_arangodb().await?, refno).await?;
        for child in children {
            if child.noun != "CYLI" { continue; }
            let child_refno = child.refno;
            if mgr.get_refno_basic(child_refno).is_none() { continue; }
            let refno_basic = mgr.get_refno_basic(child_refno).unwrap();
            // 取得 cyli 的 heig ori属性
            let attr = query_implicit_attr(child_refno, refno_basic.value(), &project_db, Some(vec!["ORI", "HEIG"])).await?;
            if attr.get_val("HEIG").is_none() { continue; }
            if attr.get_val("ORI").is_none() { continue; }
            let height = attr.get_val("HEIG").unwrap().double_value().unwrap_or(0.0) as f32;
            let ori = attr.get_val("ORI").unwrap().vec3_value().unwrap_or([0.0, 0.0, 0.0]);
            // 获取 cyli的世界坐标
            if mgr.get_world_transform(child_refno).await?.is_none() { continue; }
            let world_transform = mgr.get_world_transform(child_refno).await?.unwrap();
            // 将中心坐标转化为cyli上下两个点的坐标
            let pos_up = world_transform.transform_point(Vec3::new(0.0, 0.0, height / 2.0_f32));
            let pos_down = world_transform.transform_point(Vec3::new(0.0, 0.0, -height / 2.0_f32));
            let ori_up = world_transform.transform_point(Vec3::new(0.0, 0.0, 1.0)).normalize();
            let ori_down = world_transform.transform_point(Vec3::new(0.0, 0.0, -1.0)).normalize();
            let cylinder_transform = CylinderTransform {
                refno: child_refno.to_refno_string(),
                pos_up,
                pos_down,
                ori_up,
                ori_down,
            };
            result.push(cylinder_transform);
        }
    }
    Ok(result)
}

#[tokio::test]
async fn virtual_hole_test() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    save_virtual_hole_value_to_arangodb(&db_option).await.unwrap();
    Ok(())
}