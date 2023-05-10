use std::sync::Arc;
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use arangors_lite::collection::CollectionType::{Document, Edge};
use arangors_lite::{AqlQuery, Database};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Pool};
use crate::api::attr::query_full_attr;
use crate::aql_api::children::query_children_aql;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{create_arangodb_conn, get_arangodb_conn_from_db_option, save_arangodb_with_database};
use crate::consts::AQL_PDMS_ELES_COLLECTION;

/// 土建出图轴网需要的数据
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AxisData {
    pub _key: String,
    pub gtype: String,
    pub description: String,
    pub poss: Vec3,
    pub pose: Vec3,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AxisEdge {
    pub _key: String,
    pub _from: String,
    pub _to: String,
}

/// 将提前存好的轴网数据从图数据库取出来
pub async fn query_axis_from_sbfr_aql(sbfr_refno: RefU64, database: &Database) -> anyhow::Result<Vec<AxisData>> {
    let key = format!("{AQL_PDMS_ELES_COLLECTION}/{}", sbfr_refno.to_url_refno());
    let aql = AqlQuery::new("\
    for v in 1 outbound @id axis_edge
        return {
        '_key':v._key,
        'description':v.description,
        'gtype':v.gtype,
        'pose':v.pose,
        'poss':v.poss,
    } ").bind_var("id", key);
    let result: Vec<AxisData> = database.aql_query(aql).await?;
    Ok(result)
}

/// 通过 sbfr 参考号 获取下面轴网需要的数据
pub async fn query_axis_from_sbfr(sbfr_refno: RefU64, database: &Database, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<AxisData>> {
    let mut result = vec![];
    let sctns = query_children_aql(database, sbfr_refno).await?;
    for sctn in sctns {
        let refno = RefU64::from_refno_str(&sctn.refno);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let attr = query_full_attr(refno, &aios_mgr, Some(vec!["GTYP", "POSS", "POSE"])).await?;
        let gtype = attr.get_str("GTYP").unwrap_or("");
        if gtype != "XGRD" && gtype != "YGRD" { continue; }
        let desc = attr.get_str("DESC").unwrap_or("");
        let poss = attr.get_poss();
        let pose = attr.get_pose();
        if poss.is_none() || pose.is_none() { continue; }
        let poss = poss.unwrap();
        let pose = pose.unwrap();
        result.push(AxisData {
            _key: refno.to_url_refno(),
            gtype: gtype.to_string(),
            description: desc.to_string(),
            poss,
            pose,
        })
    }
    Ok(result)
}

async fn save_axis_data(sbfr_refno: RefU64, axis_data: Vec<AxisData>, database: &Database) -> anyhow::Result<()> {
    let axis_eles_collection = "axis_eles";
    let axis_edge_collection = "axis_edge";
    let eles_json = serde_json::to_value(&axis_data)?;
    let mut edges = vec![];
    let sbfr_refno_url = sbfr_refno.to_url_refno();
    for axis in axis_data {
        let axis_refno = RefU64::from_url_refno(&axis._key).unwrap();
        let key = sbfr_refno.hash_with_another_refno(axis_refno);
        edges.push(AxisEdge {
            _key: key.to_string(),
            _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", &sbfr_refno_url),
            _to: format!("{}/{}", axis_eles_collection, axis_refno.to_url_refno()),
        })
    }
    let edge_json = serde_json::to_value(&edges)?;
    save_arangodb_with_database(eles_json, axis_eles_collection, database).await?;
    save_arangodb_with_database(edge_json, axis_edge_collection, database).await?;
    Ok(())
}

#[tokio::test]
async fn test_query_axis_from_sbfr() -> anyhow::Result<()> {
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let database = get_arangodb_conn_from_db_option(&mgr.db_option).await?;
    let axis_eles_collection = "axis_eles";
    let axis_edge_collection = "axis_edge";
    let _ = create_arangodb_conn(&database, axis_eles_collection, Document).await;
    let _ = create_arangodb_conn(&database, axis_edge_collection, Edge).await;

    // 暂时只测这一个轴网
    let refno = RefU64::from_refno_str("23584/56802").unwrap();
    let result = query_axis_from_sbfr(refno, &database, &mgr).await?;
    dbg!(&result.len());
    save_axis_data(refno, result, &database).await?;
    Ok(())
}

#[tokio::test]
async fn test_query_axis_from_sbfr_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_refno_str("23584/56802").unwrap();
    let result = query_axis_from_sbfr_aql(refno,&database).await?;
    dbg!(&result);
    Ok(())
}