use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::pdms_types::{EleGeoInstanceJson, RefU64};
use aios_core::rvm_types::{GeomsInfoAql, GeoParaInfo, RvmGeoInfo};
use arangors_lite::{AqlQuery, Database};
use bevy::prelude::Transform;
use glam::{Quat, Vec3};
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::options::DbOption;
use crate::rvm::data_api::{gen_prim_data, ShapeModule, ShapeTypeData};

pub async fn create_cata_element_data(refno: RefU64, desi_instance: RvmGeoInfo, database: &Database) -> anyhow::Result<Vec<u8>> {
    let mut data = Vec::new();
    let geo_infos = query_rvm_geo_infos_aql(refno, database).await?;
    if geo_infos.is_none() { return Ok(data); }
    let geo_infos = geo_infos.unwrap();
    for (idx, geo_info) in geo_infos.geo_params.into_iter().enumerate() {
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
        CateGeoParam::Cone(_) => {}
        CateGeoParam::LCylinder(_) => {}
        CateGeoParam::SCylinder(data) => {
            let radius = (data.diameter / 2.0 * 100.0).round() / 100.0;
            let height = data.height;
            let shape = ShapeTypeData::Cylinder([radius, height]);
            result.append(&mut gen_prim_data(rvm_geo_info, shape, ShapeModule::Cata));
        }
        CateGeoParam::Dish(_) => {}
        CateGeoParam::Extrusion(_) => {}
        CateGeoParam::Profile(_) => {}
        CateGeoParam::Line(_) => {}
        CateGeoParam::Pyramid(_) => {}
        CateGeoParam::RectTorus(_) => {}
        CateGeoParam::Revolution(_) => {}
        CateGeoParam::Sline(_) => {}
        CateGeoParam::SlopeBottomCylinder(_) => {}
        CateGeoParam::Snout(_) => {}
        CateGeoParam::Sphere(_) => {}
        CateGeoParam::Torus(_) => {}
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