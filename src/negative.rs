use aios_core::pdms_types::{PdmsElement, RefU64};
use arangors_lite::{AqlQuery, Database};
use dashmap::DashMap;
use crate::aql_api::PdmsElementAql;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::options::DbOption;

pub async fn query_negative_refnos_aql(refno: RefU64, database: &Database) -> anyhow::Result<DashMap<RefU64, Vec<PdmsElement>>> {
    let mut map = DashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::new("
    for c in 1..1000 inbound @id pdms_edges
    filter length(
        for z in 1 inbound c._id pdms_edges
            return 1
        ) == 0
    filter c.noun in ['NBOX','NCYL','NPYR']
    return {
        'refno':c._key,
        'owner':c.owner,
        'name':c.name,
        'noun':c.noun,
        'version':0,
        'children_count':0,
    }").bind_var("id", key);
    let result: Vec<PdmsElementAql> = database.aql_query(aql).await?;
    for v in result {
        if let Some(pdms_element) = v.change_to_pdms_element() {
            map.entry(pdms_element.owner).or_insert_with(Vec::new).push(pdms_element);
        }
    }
    Ok(map)
}

#[tokio::test]
async fn test_query_negative_refnos_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let result = query_negative_refnos_aql(RefU64::from_refno_str("23584/6799").unwrap(), &database).await?;
    dbg!(&result);
    Ok(())
}