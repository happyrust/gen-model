use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use bb8_arangodb::arangors::{AqlQuery, Database};
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

pub async fn query_para_from_desi_refno(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Vec<f64>>> {
    let catr_refno = query_foreign_refno_aql(&database, refno, &["SPRE", "CATR"]).await?;
    if catr_refno.is_none() { return Ok(None); }
    query_para_value(catr_refno.unwrap(), &database).await
}

pub async fn query_para_value(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Vec<f64>>> {
    let aql = AqlQuery::builder().query("
    return document(@collection,@refno).para")
        .bind_var("collection", "para_eles")
        .bind_var("refno", refno.to_url_refno()).build();
    let mut result: Vec<Vec<f64>> = database.aql_query(aql).await?;
    return if result.len() == 0 { Ok(None) } else { Ok(Some(result.remove(0))) };
}

pub async fn query_des_para_value(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Vec<f64>>> {
    let aql = AqlQuery::builder().query("
    return document(@collection,@refno).para")
        .bind_var("collection", "despara_eles")
        .bind_var("refno", refno.to_url_refno()).build();
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
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let result = query_para_from_desi_refno(RefU64::from_refno_str("23584/5931").unwrap(), &database).await?;
    dbg!(&result);
    Ok(())
}

