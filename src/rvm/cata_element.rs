use aios_core::geom_types::RvmGeoInfo;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::prim_geo::helper::RotateInfo;

use bevy_transform::prelude::Transform;
use bitvec::macros::internal::funty::Floating;
use glam::{Quat, Vec3};
use nom::number::streaming::f32;
use crate::arangodb::ArDatabase;
use crate::options::DbOption;

pub async fn create_cata_element_data(refno: RefU64, desi_instance: RvmGeoInfo, database: &ArDatabase) -> anyhow::Result<Vec<u8>> {
    let mut data = Vec::new();
    let geo_infos = query_rvm_geo_infos_aql(refno, &database).await?;
    if geo_infos.is_none() { return Ok(data); }
    let geo_infos = geo_infos.unwrap();
    for (_idx, geo_info) in geo_infos.geo_params.into_iter().enumerate() {
        data.append(&mut gen_cata_element_rvm_data(geo_info, desi_instance.clone()));
    }
    Ok(data)
}

/// 生成rvm 的基本体数据
// fn gen_cata_element_rvm_data(geo_info: GeoParaInfo, desi_instance: RvmGeoInfo) -> Vec<u8> {
//     let mut result = Vec::new();
//
//     let cata_transform = Transform {
//         translation: geo_info.transform.1,
//         rotation: geo_info.transform.0,
//         scale: Vec3::ONE,
//     };
//     let desi_transform = Transform {
//         translation: desi_instance.world_transform.1,
//         rotation: desi_instance.world_transform.0,
//         scale: Vec3::ONE,
//     };
//     let world_transform = desi_transform * cata_transform;
//     let mut rvm_geo_info = RvmGeoInfo {
//         _key: "".to_string(),
//         aabb: Some(geo_info.aabb),
//         data: vec![],
//         world_transform: (world_transform.rotation, world_transform.translation, Vec3::ONE),
//     };
//     match geo_info.geometry {
//         PdmsGeoParam::Boxi(_) => {}
//         PdmsGeoParam::Box(data) => {
//             if data.size.len() > 2 {
//                 let x = data.size[0];
//                 let y = data.size[1];
//                 let z = data.size[2];
//                 let shape = RvmShapeTypeData::Box([x, y, z]);
//                 result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//             }
//         }
//         PdmsGeoParam::Cone(data) => {
//             let bottom_radius = keep_2_decimals_from_f32(data.diameter / 2.0);
//             let top_radius = 0.0;
//             let height = keep_2_decimals_from_f32(data.dist_to_btm);
//             let offset = 0.0;
//             let shape = RvmShapeTypeData::Snout([bottom_radius, top_radius, height, offset, 0., 0., 0., 0., 0.]);
//             result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//         }
//         PdmsGeoParam::LCylinder(data) => {
//             let radius = keep_2_decimals_from_f32(data.diameter / 2.0);
//             let height = keep_2_decimals_from_f32(data.dist_to_top - data.dist_to_btm).abs();
//             let shape = RvmShapeTypeData::Cylinder([radius, height]);
//             result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//         }
//         PdmsGeoParam::SCylinder(data) => {
//             let radius = (data.diameter / 2.0 * 100.0).round() / 100.0;
//             let height = data.height.abs();
//             // rvm_geo_info.world_transform.1.z -= height/2.0;
//             let shape = RvmShapeTypeData::Cylinder([radius, height]);
//             result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//         }
//         PdmsGeoParam::Dish(data) => {
//             let radius = keep_2_decimals_from_f32(data.radius);
//             let height = keep_2_decimals_from_f32(data.height);
//             let shape = RvmShapeTypeData::EllipticalDish([radius, height]);
//             result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//         }
//         PdmsGeoParam::Extrusion(_) => {}
//         PdmsGeoParam::Profile(_) => {}
//         PdmsGeoParam::Line(_) => {}
//         PdmsGeoParam::Pyramid(data) => {
//             let x_bottom = keep_2_decimals_from_f32(data.x_bottom);
//             let y_bottom = keep_2_decimals_from_f32(data.y_bottom);
//             let x_top = keep_2_decimals_from_f32(data.x_top);
//             let y_top = keep_2_decimals_from_f32(data.y_top);
//             let x_offset = keep_2_decimals_from_f32(data.x_offset);
//             let y_offset = keep_2_decimals_from_f32(data.y_offset);
//             let height = keep_2_decimals_from_f32(data.dist_to_top);
//             let shape = RvmShapeTypeData::Pyramid([x_bottom, y_bottom, x_top, y_top, x_offset, y_offset, height]);
//             result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//         }
//         PdmsGeoParam::RectTorus(data) => {
//             let height = keep_2_decimals_from_f32(data.diameter);
//             let width = keep_2_decimals_from_f32(data.height);
//             let pa = data.pa;
//             let pb = data.pb;
//             if let Some(pa) = pa {
//                 if let Some(pb) = pb {
//                     if let Some(r_torus_info) = RotateInfo::cal_rotate_info(pa.dir, pa.pt, pb.dir, pb.pt) {
//                         let radius = keep_2_decimals_from_f32(r_torus_info.radius);
//                         let angle = keep_2_decimals_from_f32(r_torus_info.angle / 180.0 * f32::PI);
//                         let shape = RvmShapeTypeData::RectangularTorus([radius, width, height, angle]);
//                         result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//                     }
//                 }
//             }
//         }
//         PdmsGeoParam::Revolution(_) => {}
//         PdmsGeoParam::Sline(_) => {}
//         PdmsGeoParam::SlopeBottomCylinder(_) => {}
//         PdmsGeoParam::Snout(data) => {
//             let bottom_radius = keep_2_decimals_from_f32(data.btm_diameter / 2.0);
//             let top_radius = keep_2_decimals_from_f32(data.top_diameter / 2.0);
//             let height = keep_2_decimals_from_f32(data.dist_to_btm - data.dist_to_top).abs();
//             let offset = keep_2_decimals_from_f32(data.offset);
//             let shape = RvmShapeTypeData::Snout([bottom_radius, top_radius, height, offset, 0., 0., 0., 0., 0.]);
//             result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//         }
//         PdmsGeoParam::Sphere(_) => {}
//         PdmsGeoParam::Torus(data) => {
//             if let Some(pa) = data.pa {
//                 if let Some(pb) = data.pb {
//                     let torus = RotateInfo::cal_rotate_info(pa.dir, pa.pt, pb.dir, pb.pt);
//                     if let Some(torus) = torus {
//                         let arc_radius = torus.radius; //外圆半径
//                         let angle = keep_2_decimals_from_f32(torus.angle / 180.0 * f32::PI);
//                         let radius = keep_2_decimals_from_f32(data.diameter / 2.0); // 内圆半径
//                         let shape = RvmShapeTypeData::CircularTorus([arc_radius, radius, angle]);
//                         result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
//                     }
//                 }
//             }
//         }
//         PdmsGeoParam::TubeImplied(_) => {}
//         PdmsGeoParam::SVER(_) => {}
//         PdmsGeoParam::Unknown => {}
//         _ => {}
//     }
//     result
// }
//
// async fn query_rvm_geo_infos_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<GeomsInfoAql>> {
//     let key = refno.to_string();
//     let aql = AqlQuery::new(
//         "\
//         return document('geo_infos',@key)
//         "
//     ).bind_var("key", key);
//     let result = database.aql_query::<GeomsInfoAql>(aql).await;
//     if result.is_err() { return Ok(None); }
//     let mut result = result.unwrap();
//     if result.is_empty() { return Ok(None); }
//     Ok(Some(result.remove(0)))
// }

// #[tokio::test]
// async fn test_query_rvm_geo_infos_aql() {
//     use config::{Config, ConfigError, Environment, File};
//     let s = Config::builder()
//         .add_source(File::with_name("DbOption"))
//         .build().unwrap();
//     let db_option: DbOption = s.try_deserialize().unwrap();
//     let database = get_arangodb_conn_from_db_option_for_test(&db_option).await.unwrap();
//     let refno = RefU64::from_str("23584/209").unwrap();
//     let result = query_rvm_geo_infos_aql(refno, &database).await.unwrap().unwrap();
//     dbg!(&result);
// }