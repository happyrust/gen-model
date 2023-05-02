use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use crate::aql_api::children::query_travel_children_filter_negative_sibl_nodes;
use crate::data_interface::interface::PdmsDataInterface;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::test::test_helper::{get_test_ams_db_manager, get_test_ams_db_manager_async};

///  测试获取包含负实体的集合 （也包含了正实体）
#[tokio::test]
async fn test_query_travel_children_filter_negative_sibl_nodes() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_refno_str("31896/10042").unwrap();
    let result = query_travel_children_filter_negative_sibl_nodes(refno, &database).await?;
    dbg!(&result);
    Ok(())
}
//query_refnos_has_neg_geom

///  测试获取有负实体的parent
#[tokio::test]
async fn test_query_refnos_has_neg_geom() -> anyhow::Result<()> {
    let refno = RefU64::from_refno_str("31896/10042").unwrap();
    let interface = get_test_ams_db_manager_async().await;
    let result = interface.query_refnos_has_neg_map(refno).await?;
    dbg!(&result);
    Ok(())
}