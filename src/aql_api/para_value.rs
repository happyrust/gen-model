use std::str::FromStr;

use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;

use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::consts::{AQL_DESPARA_ELES_COLLECTION, AQL_PARA_ELES_COLLECTION};
use crate::arangodb::ArDatabase;

pub async fn query_para_from_desi_refno(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Vec<f64>>> {
    let catr_refno = query_foreign_refno_aql(&database, refno, &["SPRE", "CATR"]).await?;
    if catr_refno.is_none() { return Ok(None); }
    query_para_value(catr_refno.unwrap(), &database).await
}

pub async fn query_para_value(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Vec<f64>>> {
    let aql = AqlQuery::new("
    With @@collection
    return document(@@collection,@refno).para")
        .bind_var("@collection", AQL_PARA_ELES_COLLECTION)
        .bind_var("refno", refno.to_string());
    let mut result: Vec<Vec<f64>> = database.aql_query(aql).await?;
    return if result.len() == 0 { Ok(None) } else { Ok(Some(result.remove(0))) };
}

pub async fn query_des_para_value(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Vec<f64>>> {
    let aql = AqlQuery::new("
    With @@collection
    return document(@@collection,@refno).para")
        .bind_var("@collection", AQL_DESPARA_ELES_COLLECTION)
        .bind_var("refno", refno.to_string());
    let mut result: Vec<Vec<f64>> = database.aql_query(aql).await?;
    return if result.len() == 0 { Ok(None) } else { Ok(Some(result.remove(0))) };
}



