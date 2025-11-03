use std::str::FromStr;

use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use crate::aql_api::children::query_travel_children_filter_negative_sibl_nodes;
use crate::aql_api::pdms_mesh::query_pdms_mesh_aql;
use crate::data_interface::interface::PdmsDataInterface;
use crate::arangodb::ArDatabase;
use crate::graph_db::pdms_inst_arango::query_insts_shape_data;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use crate::test::test_helper::{get_test_ams_db_manager, get_test_ams_db_manager_async};

///  测试获取包含负实体的集合 （也包含了正实体）
#[tokio::test]
async fn test_query_travel_children_filter_negative_sibl_nodes() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_str("31896/10042").unwrap();
    let result = query_travel_children_filter_negative_sibl_nodes(refno, &database).await?;
    dbg!(&result);
    Ok(())
}
//query_refnos_has_neg_geom

///  测试获取有负实体的parent
#[tokio::test]
async fn test_query_refnos_has_neg_geom() -> anyhow::Result<()> {

    let interface = get_test_ams_db_manager_async().await;
    let database = interface.get_arango_db().await?;
    println!("here");
    let shape_insts = query_insts_shape_data(&database,
                                             &[RefU64::from_two_nums(17496, 161711)],
                                                Some(&[GeoBasicType::Pos, GeoBasicType::Compound])).await?;
    dbg!(&shape_insts.inst_geos_map);
    let geo_hashs = shape_insts.get_geo_hashs().iter().map(|x| *x).collect::<Vec<_>>();
    if let Ok(meshes_data) = query_pdms_mesh_aql(&database, geo_hashs.iter()).await {
        dbg!(meshes_data.meshes.len());
        // let r = PdmsInstanceMeshData{
        //     shape_insts,
        //     meshes_data,
        // };
    }
    // let result = interface.query_refnos_has_neg_pos_map(refno).await?;
    // dbg!(&result);
    // query_refnos_has_neg_map
    // let refno = RefU64::from_str("17496/169987").unwrap();
    // // let refno = RefU64::from_str("24381/101405").unwrap();
    // let result = interface.query_refnos_has_pos_neg_map(&[refno]).await?;
    // dbg!(&result);
    Ok(())
}