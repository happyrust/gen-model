use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::pdms_types::{EleGeoInstanceJson, RefU64};
use aios_core::prim_geo::helper::RotateInfo;
use aios_core::rvm_types::{GeomsInfoAql, GeoParaInfo, RvmGeoInfo};
use arangors_lite::{AqlQuery, Database};
use bevy::prelude::Transform;
use bitvec::macros::internal::funty::Floating;
use glam::{Quat, Vec3};
use nom::number::streaming::f32;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::options::DbOption;
use crate::rvm::data_api::{gen_prim_data, keep_2_decimals_from_f32, ShapeModule, ShapeTypeData};

pub async fn create_cata_element_data(refno: RefU64, desi_instance: RvmGeoInfo, database: &Database) -> anyhow::Result<Vec<u8>> {
    let mut data = Vec::new();
    let geo_infos = query_rvm_geo_infos_aql(refno, database).await?;
    if geo_infos.is_none() { return Ok(data); }
    let geo_infos = geo_infos.unwrap();
    for (_idx, geo_info) in geo_infos.geo_params.into_iter().enumerate() {
        data.append(&mut gen_cata_element_prim_data(geo_info, desi_instance.clone()));
    }
    Ok(data)
}

fn gen_cata_element_prim_data(geo_info: GeoParaInfo, desi_instance: RvmGeoInfo) -> Vec<u8> {
    let mut result = Vec::new();

    let cata_transform = Transform {
        translation: geo_info.transform.1,
        rotation: geo_info.transform.0,
        scale: geo_info.transform.2,
    };
    let desi_transform = Transform {
        translation: desi_instance.world_transform.1,
        rotation: desi_instance.world_transform.0,
        scale: desi_instance.world_transform.2,
    };
    let world_transform = desi_transform * cata_transform;
    let mut rvm_geo_info = RvmGeoInfo {
        _key: "".to_string(),
        aabb: Some(geo_info.aabb),
        data: vec![],
        world_transform: (world_transform.rotation, world_transform.translation, world_transform.scale),
    };
    match geo_info.geometry {
        CateGeoParam::Boxi(_) => {}
        CateGeoParam::Box(data) => {
            if data.size.len() > 2 {
                let x = data.size[0];
                let y = data.size[1];
                let z = data.size[2];
                let shape = ShapeTypeData::Box([x, y, z]);
                result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
            }
        }
        CateGeoParam::Cone(data) => {
            let bottom_radius = keep_2_decimals_from_f32(data.diameter / 2.0);
            let top_radius = 0.0;
            let height = keep_2_decimals_from_f32(data.dist_to_btm);
            let offset = 0.0;
            let shape = ShapeTypeData::Snout([bottom_radius, top_radius, height, offset, 0., 0., 0., 0., 0.]);
            result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
        }
        CateGeoParam::LCylinder(data) => {
            let radius = keep_2_decimals_from_f32(data.diameter / 2.0);
            let height = keep_2_decimals_from_f32(data.dist_to_top - data.dist_to_btm).abs();
            let shape = ShapeTypeData::Cylinder([radius, height]);
            result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
        }
        CateGeoParam::SCylinder(data) => {
            let radius = (data.diameter / 2.0 * 100.0).round() / 100.0;
            let height = data.height.abs();
            let shape = ShapeTypeData::Cylinder([radius, height]);
            result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
        }
        CateGeoParam::Dish(data) => {
            let radius = keep_2_decimals_from_f32(data.radius);
            let height = keep_2_decimals_from_f32(data.height);
            let shape = ShapeTypeData::EllipticalDish([radius, height]);
            result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
        }
        CateGeoParam::Extrusion(_) => {}
        CateGeoParam::Profile(_) => {}
        CateGeoParam::Line(_) => {}
        CateGeoParam::Pyramid(data) => {
            let x_bottom = keep_2_decimals_from_f32(data.x_bottom);
            let y_bottom = keep_2_decimals_from_f32(data.y_bottom);
            let x_top = keep_2_decimals_from_f32(data.x_top);
            let y_top = keep_2_decimals_from_f32(data.y_top);
            let x_offset = keep_2_decimals_from_f32(data.x_offset);
            let y_offset = keep_2_decimals_from_f32(data.y_offset);
            let height = keep_2_decimals_from_f32(data.dist_to_top);
            let shape = ShapeTypeData::Pyramid([x_bottom, y_bottom, x_top, y_top, x_offset, y_offset, height]);
            result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
        }
        CateGeoParam::RectTorus(data) => {
            let height = keep_2_decimals_from_f32(data.diameter);
            let width = keep_2_decimals_from_f32(data.height);
            let pa = data.pa;
            let pb = data.pb;
            if let Some(pa) = pa {
                if let Some(pb) = pb {
                    if let Some(r_torus_info) = RotateInfo::cal_rotate_info(pa.dir, pa.pt, pb.dir, pb.pt) {
                        let radius = keep_2_decimals_from_f32(r_torus_info.radius);
                        let angle = keep_2_decimals_from_f32(r_torus_info.angle / 180.0 * f32::PI);
                        let shape = ShapeTypeData::RectangularTorus([radius, width, height, angle]);
                        result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
                    }
                }
            }
        }
        CateGeoParam::Revolution(_) => {}
        CateGeoParam::Sline(_) => {}
        CateGeoParam::SlopeBottomCylinder(_) => {}
        CateGeoParam::Snout(data) => {
            let bottom_radius = keep_2_decimals_from_f32(data.btm_diameter / 2.0);
            let top_radius = keep_2_decimals_from_f32(data.top_diameter / 2.0);
            let height = keep_2_decimals_from_f32(data.dist_to_btm - data.dist_to_top).abs();
            let offset = keep_2_decimals_from_f32(data.offset);
            let shape = ShapeTypeData::Snout([bottom_radius, top_radius, height, offset, 0., 0., 0., 0., 0.]);
            result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
        }
        CateGeoParam::Sphere(_) => {}
        CateGeoParam::Torus(data) => {
            if let Some(pa) = data.pa {
                if let Some(pb) = data.pb {
                    let torus = RotateInfo::cal_rotate_info(pa.dir, pa.pt, pb.dir, pb.pt);
                    if let Some(torus) = torus {
                        let arc_radius = torus.radius; //外圆半径
                        let angle = keep_2_decimals_from_f32(torus.angle / 180.0 * f32::PI);
                        let radius = keep_2_decimals_from_f32(data.diameter / 2.0); // 内圆半径
                        let shape = ShapeTypeData::CircularTorus([arc_radius, radius, angle]);
                        result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
                    }
                }
            }
        }
        CateGeoParam::TubeImplied(_) => {}
        CateGeoParam::SVER(_) => {}
        CateGeoParam::Unknown => {}
    }
    result
}

async fn query_rvm_geo_infos_aql(refno: RefU64, database: &Database) -> anyhow::Result<Option<GeomsInfoAql>> {
    let key = refno.to_url_refno();
    let aql = AqlQuery::new("\
        return document('geo_infos',@key)
    ").bind_var("key", key);
    let result = database.aql_query::<GeomsInfoAql>(aql).await;
    if result.is_err() { return Ok(None); }
    let mut result = result.unwrap();
    if result.is_empty() { return Ok(None); }
    Ok(Some(result.remove(0)))
}

#[tokio::test]
async fn test_query_rvm_geo_infos_aql() {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build().unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await.unwrap();
    let refno = RefU64::from_refno_str("23584/209").unwrap();
    let result = query_rvm_geo_infos_aql(refno, &database).await.unwrap().unwrap();
    dbg!(&result);
}