use std::collections::HashMap;
use std::str::FromStr;
use aios_core::options::DbOption;
use aios_core::pdms_types::{CataHashRefnoKV, EleTreeNode, GENRAL_NEG_NOUN_NAMES, PdmsElement, RefU64, RefU64Vec};
use aios_core::three_dimensional_review::{VagueSearchCondition, VagueSearchRequest};
use bb8_arangodb::arangors::{AqlQuery, Database};
use bitvec::ptr::replace;
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool};
use crate::api::attr::query_attr;
use crate::aql_api::*;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::data_interface::tidb_manager::{AiosDBManager};
use crate::graph_db::pdms_arango::ArDatabase;

pub async fn query_children_aql(arango_db: &ArDatabase, refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    for z in 1 inbound @id pdms_edges
        return {
        '_key':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':length(for c in 1 inbound z._id pdms_edges
                            return 1 ),
    }").bind_var("id", refno_aql).build();
    let results: Vec<PdmsElement> = arango_db.aql_query(aql).await.unwrap();
    Ok(results)
}

pub async fn query_children_order_aql(adb: &ArDatabase, refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
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
            '_key':child._key,
            'owner':child.owner,
            'name':child.name,
            'noun':child.noun,
            'version':0,
            'children_count':length(for c in 1 inbound child._id pdms_edges
                                return 1 ),
        }").bind_var("id", refno_aql).build();
    let results: Vec<PdmsElement> = adb.aql_query(aql).await?;
    Ok(results)
}

pub async fn query_children_refnos_aql(arango_database: &ArDatabase, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    for z in 1 inbound @id pdms_edges
        return  z._key ").bind_var("id", refno_aql).build();
    let result: Vec<String> = arango_database.aql_query(aql).await?;
    Ok(convert_refno_vec_from_vec_string(result))
}

/// 找到该节点同级的上一个节点
pub async fn query_brother_node_front(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<(RefU64, String)>> {
    // let key = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    // let aql = AqlQuery::builder().query("\
    //     for v in 1 outbound @key sibl_edges
    //         return { '_key' : v._key, 'noun': v.noun }
    // ").bind_var("key", key).build();
    // let mut result: Vec<PdmsRefnoTypeAql> = database.aql_query(aql).await?;
    // if result.is_empty() { return Ok(None); }
    // let result = result.remove(0);
    // let refno = RefU64::from_url_refno(&result.refno);
    // if refno.is_none() { return Ok(None); }
    // return Ok(Some((refno.unwrap(), result.noun)));
    return Ok(None);
}

/// 获取refno的children，返回Vec<(RefU64, String)>
pub async fn query_children_with_name_aql(arango_database: &ArDatabase, refno: RefU64) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    FOR z in 1 INBOUND @id pdms_edges
        return {
            'refno':z._key,
            'name':z.name,
        }
    ").bind_var("id", refno_aql).build();
    let result: Vec<PdmsRefnoNameAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
            r.push((refno, v.name));
        }
    }
    Ok(r)
}


/// 查找该参考号的owner和 owner的type
pub async fn query_owner_with_type_aql(arango_database: &ArDatabase, refno: RefU64) -> anyhow::Result<Option<(RefU64, String)>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    FOR o in 1 OUTBOUND @id pdms_edges
        return {
            'refno':o._key,
            'noun':o.noun,
        }").bind_var("id", refno_aql).build();
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
pub async fn query_ancestor_till_type_aql(arango_database: &ArDatabase, refno: RefU64, att_type: &str) -> anyhow::Result<Option<Vec<RefU64>>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    for o in 1..10 outbound @id pdms_edges
        PRUNE o.noun == @noun
        return o._key")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .build();
    let mut result: Vec<String> = arango_database.aql_query(aql).await?;
    if result.len() == 0 { return Ok(None); };
    let r = convert_refno_vec_from_vec_string(result);
    Ok(Some(r))
}

pub async fn query_ancestor_with_name_till_type_aql(arango_database: &ArDatabase, refno: RefU64, att_type: &str) -> anyhow::Result<Vec<PdmsRefnoNameAql>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    for o in 0..10 outbound @id pdms_edges
        PRUNE o.noun == @noun
        return { refno:o._key, name:o.name }")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .build();
    let mut result: Vec<PdmsRefnoNameAql> = arango_database.aql_query(aql).await?;
    if result.len() == 0 { return Ok(vec![]); };
    Ok(result)
}

/// 向上遍历父节点，到某个类型停止，返回该类型的 name
pub async fn query_ancestor_name_of_type_aql(arango_database: &ArDatabase, refno: RefU64, att_type: &str) -> anyhow::Result<Option<String>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    for o in 0..10 outbound @id pdms_edges
        Filter o.noun == @noun
        return o.name")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .build();
    let mut result: Vec<String> = arango_database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
    Ok(Some(result.remove(0)))
}

/// 遍历refno获取所有子节点的PdmsElement
pub async fn query_deep_children_refnos_fuzzy(arango_database: &ArDatabase, refno: RefU64, nouns: &[&str]) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    FOR z in 1..2 INBOUND @id pdms_edges
    filter z._key != null
    filter z.noun in @nouns
    return z._key
    ").bind_var("id", refno_aql)
        .bind_var("nouns", nouns)
        .build();
    let results: Vec<RefU64> = arango_database.aql_query::<String>(aql).await?.iter()
        .map(|x| RefU64::from_str(x).unwrap_or_default()).collect();
    Ok(results)
}

/// 遍历refno获取所有子节点的PdmsElement
pub async fn query_travel_children_aql(arango_database: &ArDatabase, refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    FOR z in 1..2 INBOUND @id pdms_edges
    filter z._key != null
    return {
        'refno':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':0,
    }
    ").bind_var("id", refno_aql)
        .build();
    let results: Vec<PdmsElement> = arango_database.aql_query(aql).await.unwrap();
    Ok(results)
}

/// 遍历该refno的所有子节点，不包含叶子节点
pub async fn query_travel_children_with_out_leaf_aql(arango_database: &ArDatabase, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    for c in 1..10 inbound @id pdms_edges
    filter length(
        for z in 1 inbound c._id pdms_edges
            return 1
        ) != 0
    return c._key
    ").bind_var("id", refno_aql)
        .build();
    let result: Vec<String> = arango_database.aql_query(aql).await?;
    let refnos = convert_refno_vec_from_vec_string(result);
    Ok(refnos)
}

/// 遍历refno只获取指定类型数组的refnos
pub async fn query_travel_children_with_types_and_cata_hash(arango_database: &ArDatabase, refno: RefU64,
                                                            att_types: &[&str], check_parent: bool, skip_exist: bool) -> anyhow::Result<Vec<CataHashRefnoKV>> {
    // let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if check_parent {
        AqlQuery::builder().query("\
FOR v,e,p in 0..10 INBOUND @id pdms_edges
    let parent = p.vertices[-2]
    filter v.cata_hash != null
    filter parent.noun in @nouns
    let s = document(pdms_inst_geos, to_string(v.cata_hash))
    filter !@skip_exist or s == null
    COLLECT exist = s, cata_group=v.cata_hash into g
    return {
        cata_hash: cata_group,
        exist_geo: exist,
        group_refnos: g[*].v._key,
    }
    ")
            .bind_var("skip_exist", skip_exist)
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .build()
    }else{
        AqlQuery::builder().query("\
FOR v,e,p in 0..10 INBOUND @id pdms_edges
    filter v.noun in @nouns
    filter v.cata_hash != null
    let s = document(pdms_inst_geos, to_string(v.cata_hash))
    filter !@skip_exist or s == null
    COLLECT exist = s, cata_group=v.cata_hash into g
    return {
        cata_hash: cata_group,
        exist_geo: exist,
        group_refnos: g[*].v._key,
    }
    ")
            .bind_var("skip_exist", skip_exist)
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .build()
    };
    // dbg!(&aql);
    let r: Vec<CataHashRefnoKV> = arango_database.aql_query(aql).await?;
    Ok(r)
}

/// 遍历refno只获取指定类型数组的refnos
pub async fn query_travel_children_with_types_aql(arango_database: &ArDatabase, refno: RefU64, att_types: &[&str], is_parent: bool) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if is_parent {
        AqlQuery::builder().query("\
    FOR v,e,p in 0..10 INBOUND @id pdms_edges
    let parent = p.vertices[-2]
    Filter parent.noun in @nouns
    return v")
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .build()
    }else{
        AqlQuery::builder().query("\
    FOR v in 0..10 INBOUND @id pdms_edges
    Filter v.noun in @nouns
    return v")
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .build()
    };
    // dbg!(&aql);
    let result: Vec<PdmsElement> = arango_database.aql_query(aql).await?;
    // dbg!(result.len());
    for v in result {
        r.push(EleTreeNode {
            refno: v.refno,
            owner: v.owner,
            name: v.name,
            noun: v.noun,
            children_count: 0,
        });
    }
    Ok(r)
}

/// 遍历refno只获取指定类型的refno
/// refno: 指定该参考号下面所有的节点来进行过滤
/// att_type: 需要查找的类型
/// 实例  : query_travel_children_with_type_aql(&database,RefU64::from_refno_str("23584/107").unwrap(),"BRAN" )
pub async fn query_travel_children_with_type_aql(arango_database: &ArDatabase, refno: RefU64, att_type: &str) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
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
        .bind_var("noun", att_type)
        .build();
    let result: Vec<PdmsElement> = arango_database.aql_query(aql).await?;
    for v in result {
        r.push(EleTreeNode {
            refno: v.refno,
            owner: v.owner,
            name: v.name,
            noun: v.noun,
            children_count: 0,
        });
    }
    Ok(r)
}

pub async fn query_refnos_travel_children_with_type_aql(arango_database: &ArDatabase, refnos: &[RefU64], att_type: Vec<&str>) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = refnos.into_iter().map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::builder().query("\
    let eles = ( for refno in @id
    FOR z in 0..100 INBOUND refno pdms_edges
        filter POSITION(@noun,z.noun)
        return {
            'refno':z._key,
            'owner':z.owner,
            'name':z.name,
            'noun':z.noun,
            'version':0,
            'children_count':0,
        })
    return UNIQUE(eles)")
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .build();
    let result: Vec<Vec<PdmsElement>> = arango_database.aql_query(aql).await?;
    let result = result.into_iter().flatten().collect::<Vec<_>>();
    for v in result {
        r.push(EleTreeNode {
            refno: v.refno,
            owner: v.owner,
            name: v.name,
            noun: v.noun,
            children_count: 0,
        })
    }
    Ok(r)
}

pub async fn query_refno_from_site_zone_name(arango_database: &ArDatabase, site_name: String, zone_name: String, att_type: String) -> anyhow::Result<Vec<RefU64>> {
    return if zone_name != "\"\"" {
        let aql = AqlQuery::builder().query(r"
        FOR site IN pdms_eles
            FILTER site.noun == 'SITE' AND Contains(site.name , @site_name)
            FOR c IN 1 INBOUND site pdms_edges
                Filter Contains(c.name, @zone_name)
                FOR z in 1..4 INBOUND c pdms_edges
                    Filter z.noun == @noun
                    RETURN z._key")
            .bind_var("site_name", site_name)
            .bind_var("zone_name", zone_name)
            .bind_var("noun", att_type)
            .build();
        let result: Vec<String> = arango_database.aql_query(aql).await?;
        Ok(convert_refno_vec_from_vec_string(result))
    } else {
        let aql = AqlQuery::builder().query(r"
        FOR site IN pdms_eles
            FILTER site.noun == 'SITE' AND Contains(site.name , @site_name)
                FOR c IN 1..5 INBOUND site pdms_edges
                    FILTER c.noun == @noun
                    RETURN c._key")
            .bind_var("site_name", site_name)
            .bind_var("noun", att_type)
            .build();
        let result: Vec<String> = arango_database.aql_query(aql).await?;
        Ok(convert_refno_vec_from_vec_string(result))
    };
}

/// 返回同层级的所有参考号，并按照 pdms 树的顺序排序
pub async fn query_sibl_level_refnos(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<RefU64>> {
    let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    // in 是该 refno 下面
    let aql_in = AqlQuery::builder().query(r"
        for v in 1..1000 inbound @id sibl_edges
            return v._key")
        .bind_var("id", refno_url.clone())
        .build();
    let result: Vec<String> = database.aql_query(aql_in).await.unwrap_or(Vec::new());
    if result.is_empty() { return Ok(vec![]); }
    let in_refnos = convert_refno_vec_from_vec_string(result);
    // out 是该 refno 上面
    let aql_out = AqlQuery::builder().query(r"
        for v in 1..1000 outbound @id sibl_edges
            return v._key")
        .bind_var("id", refno_url)
        .build();
    let result: Vec<String> = database.aql_query(aql_out).await?;
    let mut out_refnos = convert_refno_vec_from_vec_string(result);
    out_refnos.push(refno);
    Ok([out_refnos, in_refnos].concat())
}

/// 返回该参考号的上一个或者下一个节点的attr，b_pre: true 上一个 , false 下一个
pub async fn query_pre_or_next_node(refno: RefU64, b_pre: bool, database: &ArDatabase, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<AttrMap>> {
    let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if b_pre {
        AqlQuery::builder().query("\
        for v in 1 outbound @key sibl_edges
            return v._key
    ").bind_var("key", refno_url).build()
    } else {
        AqlQuery::builder().query("\
        for v in 1 inbound @key sibl_edges
            return v._key
    ").bind_var("key", refno_url)
            .build()
    };
    let aql_result = database.aql_query::<String>(aql).await;
    // 如果为该层第一个或者最后一个 则返回 None
    if aql_result.is_err() { return Ok(None); }
    let aql_result = aql_result.unwrap();
    let aql_result = convert_refno_vec_from_vec_string(aql_result);
    if aql_result.is_empty() { return Ok(None); }
    let attr = query_attr(aql_result[0], aios_mgr, None).await?;
    Ok(Some(attr))
}


/// 返回指定参考号下所有的负实体以及和负实体同级的其他节点
pub async fn query_travel_children_filter_negative_sibl_nodes(refno: RefU64, database: &ArDatabase) -> anyhow::Result<HashMap<RefU64, Vec<PdmsElement>>> {
    let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    // todo negatives并没有去掉同层级的其他节点，同层级的所有节点都查询了一遍，应该一层只用查询一遍
    let aql = AqlQuery::builder().query("\
        let negatives = ( FOR v in 0..10 INBOUND @key pdms_edges
                    filter POSITION(@negative_nouns, v.noun)
                    return v._id )
        let sibls = ( for negative in negatives
                for v in 0..1000 ANY negative sibl_edges
                return {
                    'refno':v._key,
                    'owner':v.owner,
                    'name':v.name,
                    'noun':v.noun,
                    'version':0,
                    'children_count':0,
                } )
        return UNIQUE(sibls)"
    ).bind_var("key", refno_url)
        .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec()).build();
    let results = database.aql_query::<Vec<PdmsElement>>(aql).await?;
    let mut negative_map = HashMap::new();
    for result in results {
        for r in result {
            negative_map.entry(r.owner).or_insert_with(Vec::new).push(r);
        }
    }
    Ok(negative_map)
}

/// 过滤掉同层级拥有负实体的参考号
pub async fn filter_negative_sibl_from_refnos(refnos: &Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<RefU64>> {
    let keys = refnos.into_iter().map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::builder().query("\
        for refno in @keys
        let contains_negative = ( for v in 0..1000 ANY refno sibl_edges
                            filter POSITION(@negative_nouns ,v.noun)
                            return 1 )
        filter Length(contains_negative) == 0
        return refno
    ").bind_var("keys", keys).bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec()).build();
    let result = database.aql_query::<String>(aql).await?;
    Ok(result.into_iter().filter_map(|r| RefU64::from_arangodb_refno_str(&r)).collect::<Vec<_>>())
}

/// 用户自定义条件模糊查询 aql
pub async fn vague_query_refnos_user_set_aql(request: VagueSearchRequest, database: &ArDatabase) -> anyhow::Result<Vec<(RefU64, String)>> {
    let keys = request.filter_refnos.into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno())).collect::<Vec<_>>();
    // 生成aql模板
    let aql = format!("\
    for refno in {}
        for v in 0..1000 inbound refno pdms_edges
        @@filter_condition
        return {{
            'refno':v._key,
            'name':v.name,
        }} ", serde_json::to_string(&keys).unwrap_or("[]".to_string()));
    // 拼接过滤条件
    let mut filter_condition = String::new();
    for (key, (condition, value)) in request.filter_condition {
        let key = key.to_lowercase().replace("type", "noun");
        let value_aql = if value.contains("*") {
            // 替换通配符
            let value = value.replace("*", "%");
            format!("like '{}'", value)
        } else {
            format!("== '{}'", value)
        };
        match condition {
            VagueSearchCondition::And => {
                filter_condition.push_str(&format!("filter v.{} {} ", key, value_aql));
            }
            VagueSearchCondition::Or => {
                filter_condition.push_str(&format!("|| v.{} {} ", key, value_aql));
            }
            VagueSearchCondition::Not => {
                let value_aql = if value_aql.starts_with("=") { value_aql[1..].to_string() } else { value_aql };
                filter_condition.push_str(&format!("filter v.{} !{} ", key, value_aql));
            }
        }
    }
    // 将aql和过滤条件合并在一起
    let aql = aql.replace("@@filter_condition", &filter_condition);
    let aql = AqlQuery::builder().query(&aql).build();
    let mut r = Vec::new();
    let result: Vec<PdmsRefnoNameAql> = database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
            r.push((refno, v.name));
        }
    }
    Ok(r)
}

#[tokio::test]
async fn test_query_travel_children_filter_negative_sibl_nodes() -> anyhow::Result<()> {
    // use config::{Config, ConfigError, Environment, File};
    // let s = Config::builder()
    //     .add_source(File::with_name("DbOption"))
    //     .build()?;
    // let db_option: DbOption = s.try_deserialize().unwrap();
    // let database = get_arangodb_conn_from_db_option(&db_option).await?;
    // let refno = RefU64::from_refno_str("17496/79566").unwrap();
    // let result = query_travel_children_filter_negative_sibl_nodes(refno, &database).await?;
    // dbg!(&result);
    Ok(())
}