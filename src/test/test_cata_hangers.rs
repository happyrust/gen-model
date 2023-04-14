use crate::graph_db::pdms_arango::save_arangodb_with_db_option;
use config::{Config, ConfigError, Environment, File};
use std::env;
use arangors_lite::collection::CollectionType::{Document, Edge};
use std::sync::Arc;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::*;
use crate::plot_data::hangers;
use crate::test::test_helper;
use crate::test::test_helper::get_test_ams_db_manager;

#[test]
fn test_area() {
    use geo::polygon;
    use geo::Area;
    use geo::Coordinate;
    use geo::{coord, LineString, Polygon};

    let polygon = geo::geometry::Polygon::new(
        LineString::from(vec![(0., 0.), (1., 1.), (1., 0.), (0., 0.)]),
        vec![],
    );

    assert_eq!(polygon.unsigned_area(), 30.);
}

#[tokio::test]
async fn test_save_hangers_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;


    let database = test_helper::get_test_ams_arrango_db().await;
    create_arangodb_conn(&database, "hanger_data", Document).await?;
    create_arangodb_conn(&database, "hanger_edges", Edge).await?;

    let mgr = Arc::new(test_helper::get_test_ams_db_manager());
    let data = hangers::save_hangers_data(mgr.clone()).await?;
    if let Some(data) = data {
        let json = serde_json::to_value(&vec![data]).unwrap();
        save_arangodb_with_db_option(json, &mgr.db_option, "hanger_data").await?;
    }
    Ok(())
}
