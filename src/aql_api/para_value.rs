use aios_core::pdms_types::RefU64;
use arangors_lite::{AqlQuery, Connection, Database};
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::virtual_hole_value::query_virtual_hole_value;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::graph_db::structs::VirtualHoleGraphNode;
use crate::options::DbOption;

pub async fn query_para_from_desi_refno(refno: RefU64, database: &Database) -> anyhow::Result<Option<Vec<f64>>> {
    let catr_refno = query_foreign_refno_aql(refno, vec!["SPRE", "CATR"], database).await?;
    if catr_refno.is_none() { return Ok(None); }
    query_para_value(catr_refno.unwrap(), database).await
}

pub async fn query_para_value(refno: RefU64, database: &Database) -> anyhow::Result<Option<Vec<f64>>> {
    let aql = AqlQuery::new("
    return document(@collection,@refno).para")
        .bind_var("collection", "para_eles")
        .bind_var("refno", refno.to_url_refno());
    let mut result: Vec<Vec<f64>> = database.aql_query(aql).await?;
    return if result.len() == 0 { Ok(None) } else { Ok(Some(result.remove(0))) };
}

pub async fn query_des_para_value(refno: RefU64, database: &Database) -> anyhow::Result<Option<Vec<f64>>> {
    let aql = AqlQuery::new("
    return document(@collection,@refno).para")
        .bind_var("collection", "despara_eles")
        .bind_var("refno", refno.to_url_refno());
    let mut result: Vec<Vec<f64>> = database.aql_query(aql).await?;
    return if result.len() == 0 { Ok(None) } else { Ok(Some(result.remove(0))) };
}

#[tokio::test]
async fn test_query_para_from_desi_refno() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let result = query_para_from_desi_refno(RefU64::from_refno_str("23584/5931").unwrap(), &database).await?;
    dbg!(&result);
    Ok(())
}


#[tokio::test]
async fn test() -> anyhow::Result<()> {
    let mut refnos = Vec::new();
    refnos.push(RefU64::from_refno_str("24383/46246").unwrap());
    refnos.push(RefU64::from_refno_str("24383/380").unwrap());
    let result = query_virtual_hole_value(refnos).await?;
    dbg!(&result.unwrap());
    Ok(())
}


#[tokio::test]
async fn insert_virtual_hole_test() -> anyhow::Result<()> {
    let mut refnos = Vec::new();
    refnos.push(RefU64::from_refno_str("24383/46246").unwrap());
    refnos.push(RefU64::from_refno_str("24383/380").unwrap());
    let result = query_virtual_hole_value(refnos).await?;
    dbg!(&result.unwrap());
    Ok(())
}