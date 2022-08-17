use std::collections::HashMap;
use aios_core::pdms_types::{EleTreeNode, PdmsElement, RefU64};
use arangors_lite::{AqlQuery, Connection, Database};
use serde::{Serialize, Deserialize};
use crate::consts::URL;


#[derive(Debug, Default, Serialize, Deserialize)]
struct PdmsRefnoNameAql {
    pub refno: String,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PdmsRefnoTypeAql {
    pub refno: String,
    pub noun: String,
}

/// todo 需要放到 RefU64的 成员方法中
pub fn convert_refno_vec_from_vec_string(string_vec: Vec<String>) -> Vec<RefU64> {
    let mut result = vec![];
    for v in string_vec {
        if let Some(refno) = RefU64::from_url_refno(v) {
            result.push(refno);
        }
    }
    result
}

pub async fn query_children_with_name_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    FOR z in 1 INBOUND @id pdms_edges
        return {
        'refno':z._key,
        'name':z.name,
        }
    ").bind_var("id", refno_aql);
    let result: Vec<PdmsRefnoNameAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(v.refno) {
            r.push((refno, v.name));
        }
    }
    Ok(r)
}

/// 查找该参考号的owner和 owner的type
pub async fn query_owner_with_type_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Option<(RefU64, String)>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    FOR o in 1 OUTBOUND @id pdms_edges
        return {
            'refno':o._key,
            'noun':o.noun,
        }").bind_var("id", refno_aql);
    let result: Vec<PdmsRefnoTypeAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(v.refno) {
            r.push((refno, v.noun));
        }
    }
    return if r.len() > 0 {
        Ok(Some(r.remove(0)))
    } else {
        Ok(None)
    };
}

pub async fn query_ancestor_till_type_aql(arango_database: &Database, refno: RefU64, att_type: &str) -> anyhow::Result<Option<Vec<RefU64>>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    for o in 1..10 outbound @id pdms_edges
        PRUNE o.noun == @noun
        return o._key")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type);
    let mut result: Vec<String> = arango_database.aql_query(aql).await?;
    if result.len() == 0 { return Ok(None); };
    let r = convert_refno_vec_from_vec_string(result);
    Ok(Some(r))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PdmsElementAql {
    pub refno: String,
    pub owner: String,
    pub name: String,
    pub noun: String,
    pub version: u32,
    pub children_count: usize,
}

pub async fn query_travel_children_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    FOR z in 1..10 INBOUND @id pdms_edges
    return {
        'refno':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':0,
    }
    ").bind_var("id", refno_aql);
    let result: Vec<PdmsElementAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(v.refno) {
            if RefU64::from_url_refno(v.owner.clone()).is_none() { continue; }
            r.push(PdmsElement {
                refno: refno.to_refno_string(),
                owner: RefU64::from_url_refno(v.owner).unwrap(),
                name: v.name,
                noun: v.noun,
                version: 0,
                children_count: 0,
            })
        }
    }
    Ok(r)
}

pub async fn query_travel_children_with_type_aql(arango_database: &Database, refno: RefU64, att_type: &str) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    FOR z in 1..10 INBOUND @id pdms_edges
    Filter z.noun == @noun
    return {
        'refno':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':0,
    }")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type);
    let result: Vec<PdmsElementAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(v.refno) {
            if RefU64::from_url_refno(v.owner.clone()).is_none() { continue; }
            r.push(EleTreeNode {
                refno,
                owner: RefU64::from_url_refno(v.owner).unwrap(),
                name: v.name,
                noun: v.noun,
                children_count: 0,
            })
        }
    }
    Ok(r)
}

pub async fn query_refno_from_site_zone_name(arango_database: &Database, site_name: String, zone_name: String, att_type: String) -> anyhow::Result<Vec<RefU64>> {
    return if zone_name != "" {
        let aql = AqlQuery::new(r"
        FOR site IN pdms_eles
            FILTER site.noun == 'SITE' AND Contains(site.name , @site_name)
            FOR c IN 1 INBOUND site pdms_edges
                Filter Contains(c.name, @zone_name)
                FOR z in 3..4 INBOUND c pdms_edges
                    Filter z.noun == @noun
                    RETURN z._key")
            .bind_var("site_name", site_name)
            .bind_var("zone_name", zone_name)
            .bind_var("noun", att_type);
        let result: Vec<String> = arango_database.aql_query(aql).await?;
        Ok(convert_refno_vec_from_vec_string(result))
    } else {
        let aql = AqlQuery::new(r"
        FOR site IN pdms_eles
            FILTER site.noun == 'SITE' AND Contains(site.name , @site_name)
                FOR c IN 4..5 INBOUND site pdms_edges
                    FILTER c.noun == @noun
                    RETURN c._key")
            .bind_var("site_name", site_name)
            .bind_var("noun", att_type);
        let result: Vec<String> = arango_database.aql_query(aql).await?;
        Ok(convert_refno_vec_from_vec_string(result))
    };
}

#[tokio::test]
async fn test_query_children_aql() -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(URL, "root", "")
        .await
        .unwrap();

    let database = conn.db("pdms").await.unwrap();
    let site_name = "STABILIZER";
    let result = query_children_with_name_aql(&database, RefU64::from_refno_str("23584/5562").unwrap()).await?;
    dbg!(&result);
    dbg!(&result.len());
    Ok(())
}

#[tokio::test]
async fn test_query_travel_children_aql() -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(URL, "root", "")
        .await
        .unwrap();

    let database = conn.db("pdms").await.unwrap();
    let result = query_travel_children_aql(&database, RefU64::from_refno_str("23584/5562").unwrap()).await?;
    dbg!(&result);
    dbg!(&result.len());
    let r = query_travel_children_with_type_aql(&database, RefU64::from_refno_str("23584/5562").unwrap(), "FLAN").await?;
    dbg!(&r);
    dbg!(&r.len());
    Ok(())
}

#[tokio::test]
async fn test_query_refno_from_site_zone_name() -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(URL, "root", "")
        .await
        .unwrap();

    let database = conn.db("pdms").await.unwrap();
    let site_name = "STABILIZER";
    let result = query_refno_from_site_zone_name(&database, site_name.to_string(), "PIPE".to_string(), "ELBO".to_string()).await?;
    dbg!(&result);
    dbg!(&result.len());
    Ok(())
}

#[tokio::test]
async fn test_query_ancestor_till_type() -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(URL, "root", "")
        .await
        .unwrap();

    let database = conn.db("pdms").await.unwrap();
    let result = query_ancestor_till_type_aql(&database, RefU64::from_refno_str("23584/5506").unwrap(), "ZONE").await?;
    dbg!(&result);
    Ok(())
}