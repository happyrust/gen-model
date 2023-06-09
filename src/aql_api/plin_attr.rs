use std::ops::Neg;
use aios_core::options::DbOption;
use aios_core::pdms_types::{AttrMap, AttrVal, RefU64};
use bb8_arangodb::arangors::{AqlQuery, Database};
use dashmap::{DashMap, DashSet};
use glam::Vec3;
use smol_str::SmolStr;
use crate::aql_api::children::{query_children_aql, query_owner_with_type_aql};
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::PdmsPLINAttrAql;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option;

/// 传入desi的参考号，返回该参考号对应的plin attr_map 和 wall 引用的 NA 等对应的数值
pub async fn query_plin_attrs(refnos: Vec<(RefU64, String)>, database: &ArDatabase) -> anyhow::Result<DashMap<RefU64, String>> {
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
        let pstr = query_foreign_refno_aql(owner, &["SPRE", "PSTR"], &database).await?;
        if pstr.is_none() { continue; }
        let pstr_children = query_children_aql(&database,pstr.unwrap()).await?;
        let mut children = vec![];
        pstr_children.into_iter().for_each(|ele| {
            children.push(ele.refno);
        });
        let plin_attrs = query_plin_attrs_with_refnos(children, &database).await?;
        for plin_attr in plin_attrs {
            let plin_refno = RefU64::from_url_refno(&plin_attr._key);
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

pub async fn query_pline_value(refno: RefU64, jusl: &str, database: &ArDatabase) -> anyhow::Result<Option<[String; 3]>> {
    let pstr = query_foreign_refno_aql(refno,  &["SPRE", "PSTR"], &database).await?;
    // dbg!(pstr);
    if pstr.is_none() { return Ok(None); }
    let pstr_children = query_children_aql(&database,pstr.unwrap()).await?;
    let mut children = vec![];
    pstr_children.into_iter().for_each(|ele| {
        children.push(ele.refno);
    });
    // dbg!(&children);
    let plin_attrs = query_plin_attrs_with_refnos(children, &database).await?;
    for plin_attr in plin_attrs {
        let plin_refno = RefU64::from_url_refno(&plin_attr._key);
        // dbg!(&plin_refno);
        if plin_refno.is_none() { continue; }
        let attr = plin_attr.attr;
        // dbg!(attr.to_string_hashmap());
        let p_key = attr.get_as_string("PKEY");
        let px = attr.get_as_string("PX");
        let py = attr.get_as_string("PY");
        let pz = attr.get_as_string("PZ");
        if p_key.is_none() { continue; }
        if p_key.unwrap() == jusl {
            let px = px.unwrap_or("0".to_string());
            let py = py.unwrap_or("0".to_string());
            let pz = pz.unwrap_or("0".to_string());
            return Ok(Some([px, py, pz]));
        }
    }
    Ok(None)
}

/// 传入plin参考号集合，返回集合中的所有plin的attr_map
async fn query_plin_attrs_with_refnos(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsPLINAttrAql>> {
    let mut children = vec![];
    refnos.into_iter().for_each(|refno| {
        children.push(RefU64::to_url_refno(&refno))
    });
    let aql = AqlQuery::builder().query("
    let data = @element
    for v in data
    let e = document('plin_eles',v)
        return {
            '_key':e._key,
            'attr':e.attr
        } "
    ).bind_var("element", children)
        .build();
    let result: Vec<PdmsPLINAttrAql> = database.aql_query(aql).await?;
    Ok(result)
}

async fn query_plin_attrs_with_refno(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<PdmsPLINAttrAql>> {
    let aql = AqlQuery::builder().query("
    let e = document('plin_eles',@refno)
        return {
            '_key':e._key,
            'attr':e.attr
        } "
    ).bind_var("refno", refno.to_url_refno()).build();
    let result: Vec<PdmsPLINAttrAql> = database.aql_query(aql).await?;
    Ok(result)
}

pub fn match_jusline_attr(exp: String, para: Vec<f64>) -> f64 {
    match exp.as_str() {
        "DESP[1]" => para[0],
        "DESP[2]" => para[1],
        _ => 0.0,
    }
}

#[tokio::test]
async fn test_query_plin_attrs() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let request = vec![(RefU64::from_refno_str("23584/5934").unwrap(), "IBOW".to_string()),
                       (RefU64::from_refno_str("23584/5935").unwrap(), "IBOW".to_string()),
                       (RefU64::from_refno_str("23584/5936").unwrap(), "OBOW".to_string())];
    let result = query_plin_attrs(request, &database).await?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_query_wall_jusl_value() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let result = query_pline_value(RefU64::from_refno_str("23584/5931").unwrap(), "NA", &database).await?;
    dbg!(&result);
    Ok(())
}