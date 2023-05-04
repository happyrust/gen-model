use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use arangors_lite::{AqlQuery, Connection, Database};
use dashmap::DashMap;
use crate::aql_api::change_vec_refnos_into_vec_string;
use crate::aql_api::children::query_children_aql;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::para_value::query_des_para_value;
use crate::graph_db::DataDocument;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;

/// 查询 catr refno引用的 dtse下 data 的 ppro和 dpro数据
pub async fn query_dtse_ppro_from_catr_refno(refno: RefU64, database: &Database) -> anyhow::Result<Option<DashMap<String, DataDocument>>> {
    let dtre_refno = query_foreign_refno_aql(refno,  &["DTRE", "DTRE"], database).await?;
    if dtre_refno.is_none() { return Ok(None); }
    let data_refnos = query_children_aql(database, dtre_refno.unwrap()).await?;
    let mut children = vec![];
    for data_refno in data_refnos.into_iter() {
        children.push(data_refno.refno);
    }
    let result = query_data_attr_from_refnos(children, database).await?;
    Ok(Some(result))
}

/// 返回data下对应的ppro数据 -> k: dkey
async fn query_data_attr_from_refnos(refnos: Vec<RefU64>, database: &Database) -> anyhow::Result<DashMap<String, DataDocument>> {
    let children = change_vec_refnos_into_vec_string(refnos);
    let aql = AqlQuery::new("
    let data = @element
    for v in data
    let e = document('data_eles',v)
        return {
            '_key':e._key,
            'dkey':e.dkey,
            'ppro':e.ppro,
            'dpro':e.dpro,
        } "
    ).bind_var("element", children);
    let result: Vec<DataDocument> = database.aql_query(aql).await?;
    let mut data_map = DashMap::new();
    for r in result {
        data_map.entry(r.dkey.clone()).or_insert(r);
    }
    Ok(data_map)
}

#[tokio::test]
async fn test_query_dtse_ppro_from_catr_refno() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_refno_str("15193/14606").unwrap();
    let result = query_dtse_ppro_from_catr_refno(refno, &database).await?;
    dbg!(&result);
    Ok(())
}