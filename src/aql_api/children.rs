use std::collections::HashMap;
use aios_core::pdms_types::{PdmsElement, RefU64};
use arangors_lite::{AqlQuery, Connection, Database};
use crate::graph_db::arango::URL;

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
    let mut result: Vec<HashMap<String, String>> = arango_database.aql_query(aql).await?;
    for mut v in result {
        if let Some(refno_url) = v.remove("refno") {
            if let Some(refno) = RefU64::from_url_refno(refno_url) {
                if let Some(name) = v.remove("name") {
                    r.push((refno, name));
                }
            }
        }
    }
    Ok(r)
}

pub async fn query_travel_children_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    FOR z in 1..10 INBOUND @id pdms_edges
    return {
        'refno':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
    }
    ").bind_var("id", refno_aql);
    let result: Vec<HashMap<String, String>> = arango_database.aql_query(aql).await?;
    let r = convert_string_vec_to_pdms_ele(result);
    Ok(r)
}

pub fn convert_string_vec_to_pdms_ele(mut input: Vec<HashMap<String, String>>) -> Vec<PdmsElement> {
    let mut result = vec![];
    for mut v in input {
        let mut ele = PdmsElement::default();
        if let Some(refno_url) = v.remove("refno") {
            if let Some(refno) = RefU64::from_url_refno(refno_url) {
                ele.refno = refno.to_refno_string();
            }
        }
        if let Some(owner_url) = v.remove("owner") {
            if let Some(owner) = RefU64::from_url_refno(owner_url) {
                ele.owner = owner;
            }
        }
        if let Some(noun) = v.remove("noun") {
            ele.noun = noun;
        }
        if let Some(name) = v.remove("name") {
            ele.name = name;
        }
        result.push(ele);
    }
    result
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
