use aios_core::pdms_types::{AttrMap, RefU64};
use arangors_lite::{AqlQuery, Connection, Database};
use dashmap::{DashMap, DashSet};
use crate::aql_api::children::{query_children_aql, query_owner_with_type_aql};
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::PdmsPLINAttrAql;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::options::DbOption;

/// 传入desi的参考号，返回该参考号对应的plin attr_map
pub async fn query_plin_attrs(refnos: Vec<(RefU64, String)>, database: &Database) -> anyhow::Result<DashMap<RefU64, String>> {
    let mut result = DashMap::new();
    // 存 wall下的所有p_key以及对应的值
    let mut wall_map: DashMap<RefU64, DashMap<String, String>> = DashMap::new();
    let mut owner_map = DashMap::new();
    for (refno, _) in &refnos {
        let owner = query_owner_with_type_aql(database, *refno).await?;
        if owner.is_none() { continue; }
        let owner = owner.unwrap().0;
        owner_map.insert(*refno, owner);
        if wall_map.contains_key(&owner) { continue; }
        let pstr = query_foreign_refno_aql(owner, vec!["SPRE", "PSTR"], database).await?;
        if pstr.is_none() { continue; }
        let pstr_children = query_children_aql(database, pstr.unwrap()).await?;
        let mut children = vec![];
        pstr_children.into_iter().for_each(|ele| {
            let refno = RefU64::from_refno_string(ele.refno);
            if let Ok(refno) = refno {
                children.push(refno);
            }
        });
        let plin_attrs = query_plin_attrs_with_refnos(children, database).await?;
        for plin_attr in plin_attrs {
            let plin_refno = RefU64::from_url_refno(plin_attr._key);
            if plin_refno.is_none() { continue; }
            let attr = plin_attr.attr;
            let p_key = attr.get_val("PKEY");
            let plax = attr.get_val("PLAX");
            if p_key.is_none() || plax.is_none() { continue; }
            wall_map.entry(owner).or_insert_with(DashMap::new)
                .entry(p_key.unwrap().string_value()).or_insert(plax.unwrap().string_value());
        }
    }
    for (refno, pos_line) in refnos {
        let owner = owner_map.get(&refno);
        if owner.is_none() { continue; }
        let plin_map = wall_map.get(&owner.unwrap());
        if plin_map.is_none() { continue; }
        if let Some(value) = plin_map.unwrap().value().get(&pos_line) {
            result.entry(refno).or_insert(value.value().to_string());
        }
    }
    Ok(result)
}

/// 传入plin参考号集合，返回集合中的所有plin的attr_map
pub async fn query_plin_attrs_with_refnos(refnos: Vec<RefU64>, database: &Database) -> anyhow::Result<Vec<PdmsPLINAttrAql>> {
    let mut children = vec![];
    let collection = "plin_eles";
    refnos.into_iter().for_each(|refno| {
        children.push(RefU64::to_url_refno(&refno))
    });
    // let json = serde_json::to_string(&children).unwrap_or("[]".to_string());
    let aql = AqlQuery::new("
    let data = @element
    for v in data
    let e = document('plin_eles',v)
        return {
            '_key':e._key,
            'attr':e.attr
        } "
    ).bind_var("element", children);
    let result: Vec<PdmsPLINAttrAql> = database.aql_query(aql).await?;
    Ok(result)
}

#[tokio::test]
async fn test_query_plin_attrs() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let conn = Connection::establish_jwt(&db_option.arangodb_url, "root", "")
        .await?;
    let database = conn.db("pdms").await?;
    let request = vec![(RefU64::from_refno_str("23584/5934").unwrap(), "IBOW".to_string()),
                       (RefU64::from_refno_str("23584/5935").unwrap(), "IBOW".to_string()),
                       (RefU64::from_refno_str("23584/5936").unwrap(), "OBOW".to_string())];
    let result = query_plin_attrs(request, &database).await?;
    dbg!(&result);
    Ok(())
}