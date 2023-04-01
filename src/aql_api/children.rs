use std::collections::HashMap;
use aios_core::pdms_types::{EleTreeNode, PdmsElement, RefU64};
use arangors_lite::{AqlQuery, Connection, Database};
use serde::{Serialize, Deserialize};
use crate::aql_api::{convert_refno_vec_from_vec_string, PdmsElementAql, PdmsPLINAttrAql, PdmsRefnoNameAql, PdmsRefnoTypeAql};


pub async fn query_children_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    for z in 1 inbound @id pdms_edges
        return {
        'refno':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':length(for c in 1 inbound z._id pdms_edges
                            return 1 ),
    }").bind_var("id", refno_aql);
    let result: Vec<PdmsElementAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(pdms_element) = v.change_to_pdms_element() {
            r.push(pdms_element);
        }
    }
    Ok(r)
}

pub async fn query_children_order_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    let datas = (
    for v,e in 1 inbound @id pdms_edges
        filter v!= null
        return v._id )

    let backs = (
    for v in 1..1000 inbound datas[0] sibl_edges
        return v )

    let front = (
    for v in 0..1000 outbound datas[0] sibl_edges
        return v
    )
    let children = append(REVERSE(front),backs)

    for child in children
        filter child._key != null
        return {
            'refno':child._key,
            'owner':child.owner,
            'name':child.name,
            'noun':child.noun,
            'version':0,
            'children_count':length(for c in 1 inbound child._id pdms_edges
                                return 1 ),
        }").bind_var("id", refno_aql);
    let result: Vec<PdmsElementAql> = arango_database.aql_query(aql).await.unwrap_or(Vec::new());
    for v in result {
        if let Some(pdms_element) = v.change_to_pdms_element() {
            r.push(pdms_element);
        }
    }
    Ok(r)
}

pub async fn query_children_refnos_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    for z in 1 inbound @id pdms_edges
        return  z._key ").bind_var("id", refno_aql);
    let result: Vec<String> = arango_database.aql_query(aql).await?;
    Ok(convert_refno_vec_from_vec_string(result))
}

/// 找到该节点同级的上一个节点
pub async fn query_brother_node_front(refno: RefU64, database: &Database) -> anyhow::Result<Option<(RefU64, String)>> {
    let key = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
        for v in 1 outbound @key sibl_edges
            return { '_key' : v._key, 'noun': v.noun }
    ").bind_var("key", key);
    let mut result: Vec<PdmsRefnoTypeAql> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
    let result = result.remove(0);
    let refno = RefU64::from_url_refno(&result.refno);
    if refno.is_none() { return Ok(None); }
    return Ok(Some((refno.unwrap(), result.noun)));
}

/// 获取refno的children，返回Vec<(RefU64, String)>
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
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
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
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
            r.push((refno, v.noun));
        }
    }
    return if r.len() > 0 {
        Ok(Some(r.remove(0)))
    } else {
        Ok(None)
    };
}

/// 向上遍历父节点直到某个type
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

pub async fn query_ancestor_with_name_till_type_aql(arango_database: &Database, refno: RefU64, att_type: &str) -> anyhow::Result<Vec<PdmsRefnoNameAql>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    for o in 0..10 outbound @id pdms_edges
        PRUNE o.noun == @noun
        return { refno:o._key, name:o.name }")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type);
    let mut result: Vec<PdmsRefnoNameAql> = arango_database.aql_query(aql).await?;
    if result.len() == 0 { return Ok(vec![]); };
    Ok(result)
}

/// 向上遍历父节点，到某个类型停止，返回该类型的 name
pub async fn query_ancestor_name_of_type_aql(arango_database: &Database, refno: RefU64, att_type: &str) -> anyhow::Result<Option<String>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    for o in 1..10 outbound @id pdms_edges
        Filter o.noun == @noun
        return o.name")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type);
    let mut result: Vec<String> = arango_database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
    Ok(Some(result.remove(0)))
}

/// 遍历refno获取所有子节点的PdmsElement
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
        if let Some(pdms_element) = v.change_to_pdms_element() {
            r.push(pdms_element);
        }
    }
    Ok(r)
}

/// 遍历该refno的所有子节点，不包含叶子节点
pub async fn query_travel_children_with_out_leaf_aql(arango_database: &Database, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    for c in 1..10 inbound @id pdms_edges
    filter length(
        for z in 1 inbound c._id pdms_edges
            return 1
        ) != 0
    return c._key
    ").bind_var("id", refno_aql);
    let result: Vec<String> = arango_database.aql_query(aql).await?;
    let refnos = convert_refno_vec_from_vec_string(result);
    Ok(refnos)
}

/// 遍历refno只获取指定类型数组的refnos
pub async fn query_travel_children_with_types_aql(arango_database: &Database, refno: RefU64, att_types: &[&str]) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    FOR z in 0..10 INBOUND @id pdms_edges
    Filter z.noun in @nouns
    return {
        'refno':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':0,
    }")
        .bind_var("id", refno_aql)
        .bind_var("nouns", att_types);
    let result: Vec<PdmsElementAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
            if RefU64::from_url_refno(&v.owner).is_none() { continue; }
            r.push(EleTreeNode {
                refno,
                owner: RefU64::from_url_refno(&v.owner).unwrap(),
                name: v.name,
                noun: v.noun,
                children_count: 0,
            })
        }
    }
    Ok(r)
}

/// 遍历refno只获取指定类型的refno
pub async fn query_travel_children_with_type_aql(arango_database: &Database, refno: RefU64, att_type: &str) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    FOR z in 0..10 INBOUND @id pdms_edges
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
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
            if RefU64::from_url_refno(&v.owner).is_none() { continue; }
            r.push(EleTreeNode {
                refno,
                owner: RefU64::from_url_refno(&v.owner).unwrap(),
                name: v.name,
                noun: v.noun,
                children_count: 0,
            })
        }
    }
    Ok(r)
}

pub async fn query_refno_from_site_zone_name(arango_database: &Database, site_name: String, zone_name: String, att_type: String) -> anyhow::Result<Vec<RefU64>> {
    return if zone_name != "\"\"" {
        let aql = AqlQuery::new(r"
        FOR site IN pdms_eles
            FILTER site.noun == 'SITE' AND Contains(site.name , @site_name)
            FOR c IN 1 INBOUND site pdms_edges
                Filter Contains(c.name, @zone_name)
                FOR z in 1..4 INBOUND c pdms_edges
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
                FOR c IN 1..5 INBOUND site pdms_edges
                    FILTER c.noun == @noun
                    RETURN c._key")
            .bind_var("site_name", site_name)
            .bind_var("noun", att_type);
        let result: Vec<String> = arango_database.aql_query(aql).await?;
        Ok(convert_refno_vec_from_vec_string(result))
    };
}

/// 返回同层级的所有参考号，并按照 pdms 树的顺序排序
pub async fn query_sibl_level_refnos(refno: RefU64, database: &Database) -> anyhow::Result<Vec<RefU64>> {
    let refno_url = format!("pdms_eles/{}", refno.to_url_refno());
    // in 是该 refno 下面
    let aql_in = AqlQuery::new(r"
        for v in 1..1000 inbound @id sibl_edges
            return v._key")
        .bind_var("id", refno_url.clone());
    let result: Vec<String> = database.aql_query(aql_in).await.unwrap_or(Vec::new());
    if result.is_empty() { return Ok(vec![]); }
    let in_refnos = convert_refno_vec_from_vec_string(result);
    // out 是该 refno 上面
    let aql_out = AqlQuery::new(r"
        for v in 1..1000 outbound @id sibl_edges
            return v._key")
        .bind_var("id", refno_url);
    let result: Vec<String> = database.aql_query(aql_out).await?;
    let mut out_refnos = convert_refno_vec_from_vec_string(result);
    out_refnos.push(refno);
    Ok([out_refnos, in_refnos].concat())
}