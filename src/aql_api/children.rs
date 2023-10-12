use crate::api::attr::{query_attr, query_uda_ukey};
use crate::aql_api::*;
use crate::consts::{AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION, AQL_ROOM_ELES_COLLECTION, AQL_SIBL_EDGES_COLLECTION};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use aios_core::options::DbOption;
use aios_core::pdms_types::{
    CataHashRefnoKV, EleTreeNode, PdmsElement, RefU64, RefU64Vec, GENRAL_NEG_NOUN_NAMES,
};
use aios_core::pdms_user::*;
use aios_core::three_dimensional_review::VagueSearchCondition::And;
use aios_core::three_dimensional_review::*;
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use bitvec::ptr::replace;
use dashmap::DashMap;
use indexmap::IndexMap;
use itertools::Itertools;
use parry3d::partitioning::QbvhDataGenerator;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use sqlx::{MySql, Pool};
use std::collections::{HashMap, HashSet};
use std::process::id;
use std::str::FromStr;
use aios_core::tool::db_tool::{db1_dehash, db1_dehash_const, db1_hash, db1_hash_const};
use crate::data_interface::interface::PdmsDataInterface;

pub async fn query_children_eles(
    arango_db: &ArDatabase,
    refno: RefU64,
) -> anyhow::Result<Vec<PdmsElement>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles, @@pdms_edges
    for z in 1 inbound @id pdms_edges
        return {
        '_key':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':length(for c in 1 inbound z._id pdms_edges
                            return 1 ),
    }",
    )
        .bind_var("id", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let results: Vec<PdmsElement> = arango_db.aql_query(aql).await.unwrap();
    Ok(results)
}


pub async fn query_children_order_aql(
    adb: &ArDatabase,
    refno: RefU64,
) -> anyhow::Result<Vec<PdmsElement>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "\
    WITH @@pdms_eles,@@pdms_edges,@@sibl_edges
    let datas = (
    for v,e in 1 inbound @id @@pdms_edges
        filter v!= null
        return v._id )

    let backs = (
    for v in 1..1000 inbound datas[0] @@sibl_edges
        return v )

    let front = (
    for v in 0..1000 outbound datas[0] @@sibl_edges
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
        }",
    )
        .bind_var("id", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@sibl_edges", AQL_SIBL_EDGES_COLLECTION);
    let results: Vec<PdmsElement> = adb.aql_query(aql).await?;
    Ok(results)
}

///查询子节点refno集合
pub async fn query_children_refnos(
    arango_database: &ArDatabase,
    refno: RefU64,
) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles, @@pdms_edges
    for z in 1 inbound @id @@pdms_edges
        sort z.order
        return  z._key ",
    )
        .bind_var("id", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result: Vec<String> = arango_database.aql_query(aql).await?;
    Ok(convert_refno_vec_from_vec_string(result))
}

/// 找到该节点同级的上一个节点
pub async fn query_brother_node_front(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<Option<(RefU64, String)>> {
    return Ok(None);
}

/// 获取refno的children，返回Vec<(RefU64, String)>
pub async fn query_children_with_name_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    FOR z in 1 INBOUND @id @@pdms_edges
        return {
            'refno':z._key,
            'name':z.name,
        }
    ",
    )
        .bind_var("id", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result: Vec<PdmsRefnoNameAql> = arango_database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
            r.push((refno, v.name));
        }
    }
    Ok(r)
}

/// 查找该参考号的owner和 owner的type
pub async fn query_owner_with_type_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
) -> anyhow::Result<Option<(RefU64, String)>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    FOR o in 1 OUTBOUND @id @@pdms_edges
        return {
            'refno':o._key,
            'noun':o.noun,
        }",
    )
        .bind_var("id", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
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
pub async fn query_ancestor_till_type_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
    att_type: &str,
) -> anyhow::Result<Option<Vec<RefU64>>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    for o in 1..10 outbound @id @@pdms_edges
        PRUNE o.noun == @noun
        return o._key",
    )
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let mut result: Vec<String> = arango_database.aql_query(aql).await?;
    if result.len() == 0 {
        return Ok(None);
    };
    let r = convert_refno_vec_from_vec_string(result);
    Ok(Some(r))
}

/// 向上遍历父节点直到某个类型集合，只返回类型为该att_types中的数据
pub async fn query_ancestor_till_types_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
    att_types: Vec<&str>,
) -> anyhow::Result<Option<PdmsElement>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    for o in 1..10 outbound @id @@pdms_edges
        PRUNE o.noun in @nouns
        FILTER o.noun in @nouns
        return {
            '_key':o._key,
            'owner':o.owner,
            'name':o.name,
            'noun':o.noun,
            'version':0,
            'children_count':0,
        }",
    )
        .bind_var("id", refno_aql)
        .bind_var("nouns", att_types)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result = arango_database.aql_query::<PdmsElement>(aql).await?;
    if result.is_empty() {
        return Ok(None);
    }
    Ok(Some(result[0].clone()))
}

/// 向上遍历多个节点父节点直到某个类型集合，只返回类型为该att_types中的数据
pub async fn query_refnos_ancestor_till_types_aql(
    arango_database: &ArDatabase,
    refnos: Vec<RefU64>,
    att_types: Vec<&str>,
) -> anyhow::Result<Vec<PdmsElement>> {
    let refnos = refnos
        .into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    for id in @ids
    for o in 1..10 outbound id @@pdms_edges
        PRUNE o.noun in @nouns
        FILTER o.noun in @nouns
        return {
            '_key':o._key,
            'owner':o.owner,
            'name':o.name,
            'noun':o.noun,
            'version':0,
            'children_count':0,
        }",
    )
        .bind_var("ids", refnos)
        .bind_var("nouns", att_types)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result = arango_database.aql_query::<PdmsElement>(aql).await?;
    Ok(result)
}

pub async fn query_ancestor_with_name_till_type_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
    att_type: &str,
) -> anyhow::Result<Vec<PdmsRefnoNameAql>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    for o in 0..10 outbound @id @@pdms_edges
        PRUNE o.noun == @noun
        return { refno:o._key, name:o.name }",
    )
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let mut result: Vec<PdmsRefnoNameAql> = arango_database.aql_query(aql).await?;
    if result.len() == 0 {
        return Ok(vec![]);
    };
    Ok(result)
}

/// 向上遍历父节点，到某个类型停止，返回该类型的 name
pub async fn query_ancestor_name_of_type_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
    att_type: &str,
) -> anyhow::Result<Option<String>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "
    With @@pdms_eles, @@pdms_edges
    for o in 0..10 outbound @id @@pdms_edges
        Filter o.noun == @noun
        return o.name",
    )
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let mut result: Vec<String> = arango_database.aql_query(aql).await?;
    if result.is_empty() {
        return Ok(None);
    }
    Ok(Some(result.remove(0)))
}

/// 获取多个节点向上遍历到指定类型的参考号和name
pub async fn query_refnos_ancestor_with_name_till_type_aql(arango_database: &ArDatabase, refnos: Vec<RefU64>, att_types: Vec<String>) -> anyhow::Result<Vec<PdmsOwnerNameAql>> {
    let refno_aql = refnos.into_iter().map(|refno| refno.to_url_refno()).collect::<Vec<_>>();
    let aql = AqlQuery::new("
    With @@pdms_eles,@@pdms_edges
    for refno in @refnos
    for v in 0..10 outbound concat('pdms_eles/',refno) @@pdms_edges
        filter v!= null
        filter v.noun in @nouns
        return {
            'refno':refno,
            'owner':v._key,
            'owner_noun':v.noun,
            'owner_name':v.name
        }").bind_var("refnos", refno_aql)
        .bind_var("nouns", att_types)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result: Vec<PdmsOwnerNameAql> = arango_database.aql_query(aql).await?;
    Ok(result)
}

/// 查询多个refno对应的owner的name,refno，type
pub async fn query_refnos_owner_with_name_till_type_aql(arango_database: &ArDatabase, refnos: Vec<RefU64>) -> anyhow::Result<Vec<PdmsOwnerNameAql>> {
    let refno_aql = refnos.into_iter().map(|refno| refno.to_url_refno()).collect::<Vec<_>>();
    let aql = AqlQuery::new("
    With @@pdms_eles,@@pdms_edges
    for refno in @refnos
    for v in 1 outbound concat('pdms_eles/',refno) @@pdms_edges
        filter v!= null
        return {
            'refno':refno,
            'owner':v._key,
            'owner_noun':v.noun,
            'owner_name':v.name
        }").bind_var("refnos", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result: Vec<PdmsOwnerNameAql> = arango_database.aql_query(aql).await?;
    Ok(result)
}

/// 通过name查询参考号
pub async fn query_refnos_from_names(names: Vec<String>, database: &ArDatabase, filter_types: Option<Vec<String>>)
                                     -> anyhow::Result<Vec<PdmsElement>> {
    let names = names
        .into_iter()
        .map(|name| if !name.starts_with("/") { format!("/{}", name) } else { name })
        .collect::<Vec<_>>();
    let mut aql_str = "
    with @@pdms_eles
    for e in @@pdms_eles
    //filter e.noun in @filter_nouns
    filter e.name in @names
    return {
        '_key':e._key,
        'owner':e.owner,
        'name':e.name,
        'noun':e.noun,
        'version':0,
        'children_count':0,
    }".to_string();
    if filter_types.is_some() {
        aql_str = aql_str.replace("//", "");
    }
    let aql = AqlQuery::new(aql_str.as_str())
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("names", names)
        .bind_var("filter_nouns", filter_types.unwrap_or(vec![]));
    let result = database.aql_query::<PdmsElement>(aql).await?;
    Ok(result)
}

/// 通过name集合返回对应的参考号
///
/// 使用 fulltext 索引方式
pub async fn query_refnos_from_names_fulltext(names: Vec<String>, database: &ArDatabase) -> anyhow::Result<DashMap<String, PdmsElement>> {
    // 去掉 name 开头的 /
    let full_text_names = names.iter().map(|name| {
        let name = if name.starts_with("/") { name[1..].to_string() } else { name.to_string() };
        replace_symbols(&name)
    }).collect::<Vec<String>>();
    // 通过name 模糊查询对应的参考号等信息
    let aql = AqlQuery::new("
    with @@pdms_eles
    for name in @names
        for e in fulltext(@@pdms_eles,'name',name)
            return {
            '_key':e._key,
            'owner':e.owner,
            'name':e.name,
            'noun':e.noun,
            'version':0,
            'children_count':0,
        }
    ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("names", full_text_names);
    let result = database.aql_query::<PdmsElement>(aql).await?;
    // 通过传入值与数据库模糊查询返回值对比，匹配需要的值
    let mut map = DashMap::new();
    // 数据库中取值的 name 都是带有 /, 传参names与其统一
    let names = names.into_iter()
        .map(|name| if name.starts_with("/") { name } else { format!("/{}", name) })
        .collect::<Vec<String>>();
    for r in result {
        for name in &names {
            if &r.name == name {
                map.entry(name.to_string()).or_insert(r);
                break;
            }
        }
    }
    Ok(map)
}

/// 查找对应mdb的 word 节点
///
/// module ： DESI，CATA等
pub async fn query_mdb_world_fulltext(mdb: &str, module: &str, database: &ArDatabase) -> anyhow::Result<Option<PdmsElement>> {
    let mdb_name = replace_symbols(mdb);
    dbg!(&mdb_name);
    // 将 mdb_name存在返回的name中，方便判断是否为请求的mdb_name，word的name都是 /*
    let aql = AqlQuery::new("
    with @@pdms_eles,@@pdms_edges
    for e in fulltext(@@pdms_edges,'mdb_name',@mdb)
        filter e.db_type == @module
        let ele = document(e._from)
        filter ele != null
        return {
            '_key':ele._key,
            'owner':ele.owner,
            'name':e.mdb_name,
            'noun':ele.noun,
            'version':0,
            'children_count':1,
        }
    ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("mdb", mdb_name)
        .bind_var("module", module);
    let result = database.aql_query::<PdmsElement>(aql).await?;
    dbg!(&result);
    // 判断从数据库中返回的值中，哪个是需要的
    let mdb = format!("/{}", mdb);
    for r in result {
        if r.name == mdb {
            // 将word的name还原回去
            return Ok(Some(PdmsElement {
                refno: r.refno,
                owner: r.owner,
                name: "/*".to_string(),
                noun: r.noun,
                version: r.version,
                children_count: r.children_count,
            }));
        }
    }
    Ok(None)
}

/// 将字符串 符号都转为 ，
fn replace_symbols(input: &str) -> String {
    // let mut result = String::new();
    // for c in input.chars() {
    //     if c.is_alphanumeric() {
    //         result.push(c);
    //     } else {
    //         result.push(',');
    //     }
    // }
    input.to_string()
}

///搜索沿着路径查询目标节点
#[serde_as]
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct SearchAlongParam {
    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub refnos: Vec<RefU64>,
    //需要匹配的名称
    pub fuzzy: Vec<String>,
    //需要排除在外的名称匹配
    // #[serde(default)]
    // pub exclude: Vec<String>,
    pub path_nouns: Vec<String>,
    #[serde(default)]
    pub children_nouns: Vec<String>,
    #[serde(default)]
    pub ancestor_nouns: Vec<String>,
    #[serde(default)]
    pub only_path_nodes: bool,
    #[serde(default)]
    pub include_path_nodes: bool,
}

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Default, Debug, Deref, DerefMut)]
pub struct SearchAlongResult(pub Vec<RefnoAncestorsTuple>);

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct RefnoAncestorsTuple(#[serde_as(as = "(DisplayFromStr, Vec<DisplayFromStr>)")] pub (RefU64, Vec<RefU64>));

impl AiosDBManager {
    ///沿着路径搜索目标节点
    #[inline]
    pub async fn search_refnos_along_path_by_param(
        &self,
        param: &SearchAlongParam,
    ) -> anyhow::Result<SearchAlongResult> {
        self.search_refnos_along_path(
            &param.refnos,
            param.fuzzy.iter().map(|x| x.as_str()),
            param.path_nouns.iter().map(|x| x.as_str()),
            param.children_nouns.iter().map(|x| x.as_str()),
            param.ancestor_nouns.iter().map(|x| x.as_str()),
            param.only_path_nodes,
            param.include_path_nodes,
        )
            .await
    }

    ///沿着路径搜索目标节点
    #[inline]
    pub async fn search_refnos_along_path(
        &self,
        refnos: impl IntoIterator<Item=&RefU64>,
        fuzzy: impl IntoIterator<Item=&str>,
        path_nouns: impl IntoIterator<Item=&str>,
        children_nouns: impl IntoIterator<Item=&str>,
        ancestor_nouns: impl IntoIterator<Item=&str>,
        only_path_nodes: bool,
        include_path_nodes: bool,
    ) -> anyhow::Result<SearchAlongResult> {
        let mut target_refnos = refnos.into_iter().cloned().collect::<Vec<_>>();
        if target_refnos.is_empty() {
            target_refnos = self.get_site_refnos().await?;
        }
        let arango_db = self.get_arango_db().await?;
        search_refnos_along_path_arango(
            &arango_db,
            &target_refnos,
            fuzzy,
            path_nouns,
            children_nouns,
            ancestor_nouns,
            only_path_nodes,
            include_path_nodes,
        )
            .await
    }
}

///沿着路径搜索目标节点, 并且返回沿途的ancestor参考号（可选）
pub async fn search_refnos_along_path_arango(
    database: &ArDatabase,
    refnos: impl IntoIterator<Item=&RefU64>,
    fuzzy: impl IntoIterator<Item=&str>,
    path_nouns: impl IntoIterator<Item=&str>,
    children_nouns: impl IntoIterator<Item=&str>,
    ancestor_nouns: impl IntoIterator<Item=&str>,
    only_path_nodes: bool,
    include_path_nodes: bool,
) -> anyhow::Result<SearchAlongResult> {
    let ids = refnos
        .into_iter()
        .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
        .collect::<Vec<_>>();

    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    for id in @ids
        let path_len = @only_path_nodes ? LENGTH(@path_nouns) : 10
        FOR v,e,p in 0..path_len INBOUND id pdms_edges
            prune (LENGTH(p.edges) < LENGTH(@fuzzy) ? !(CHAR_LENGTH(@fuzzy[LENGTH(p.edges)]) == 0 || CONTAINS(v.name, @fuzzy[LENGTH(p.edges)])) : @only_path_nodes) or
                (LENGTH(p.edges) < LENGTH(@path_nouns) ? (v.noun != @path_nouns[LENGTH(p.edges)]) : @only_path_nodes)

            let beyond = LENGTH(p.edges) >= LENGTH(@fuzzy)
            filter beyond ? true : ( @include_path_nodes and CONTAINS(v.name, @fuzzy[LENGTH(p.edges)]))

            filter LENGTH(@children_nouns) == 0  or (v.noun in @children_nouns)
            SORT LENGTH(p.edges), v.order
            return [v._key, (for a in p.vertices filter LENGTH(@ancestor_nouns) !=0 and (a.noun in @ancestor_nouns) return a._key)]
    ")
        .bind_var("ids", ids)
        .bind_var("path_nouns", path_nouns.into_iter().collect::<Vec<_>>())
        .bind_var(
            "children_nouns",
            children_nouns.into_iter().collect::<Vec<_>>(),
        )
        .bind_var("fuzzy", fuzzy.into_iter().collect::<Vec<_>>())
        .bind_var("ancestor_nouns", ancestor_nouns.into_iter().collect::<Vec<_>>())
        .bind_var("only_path_nodes", only_path_nodes)
        .bind_var("include_path_nodes", include_path_nodes)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let m: Vec<RefnoAncestorsTuple> = database
        .aql_query::<RefnoAncestorsTuple>(aql)
        .await?
        .into_iter()
        .collect();
    Ok(SearchAlongResult(m))
}

/// 遍历参考号下获取所有指定类型的子节点参考号
pub async fn query_deep_children_refnos_fuzzy(
    database: &ArDatabase,
    refnos: impl IntoIterator<Item=&RefU64>,
    nouns: &[&str],
) -> anyhow::Result<Vec<RefU64>> {
    let ids = refnos
        .into_iter()
        .map(|x| x.format_url_name(AQL_PDMS_ELES_COLLECTION))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    for id in @ids
        FOR z in 0..10 INBOUND id @@pdms_edges
        filter z._key != null
        filter z.noun in @nouns
        return z._key
    ", ).bind_var("ids", ids)
        .bind_var("nouns", nouns)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let results: Vec<RefU64> = database
        .aql_query::<String>(aql)
        .await?
        .iter()
        .map(|x| RefU64::from_str(x).unwrap_or_default())
        .collect();
    Ok(results)
}

/// 遍历refno获取所有子节点的PdmsElement
pub async fn query_travel_children_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
) -> anyhow::Result<Vec<PdmsElement>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    FOR z in 1..2 INBOUND @id @@pdms_edges
    filter z._key != null
    return {
        '_key':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':0,
    }", ).bind_var("id", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let results: Vec<PdmsElement> = arango_database.aql_query(aql).await.unwrap();
    Ok(results)
}

/// 遍历该refno的所有子节点包含自己，并只返回参考号
pub async fn query_travel_children_refnos_aql(
    arango_database: &ArDatabase,
    refno: Vec<RefU64>,
) -> anyhow::Result<Vec<RefU64>> {
    let ids = refno.into_iter()
        .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    for id in @ids
    for c in 0..10 inbound id @@pdms_edges
    return c._key
    ", ).bind_var("ids", ids)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result: Vec<String> = arango_database.aql_query(aql).await?;
    let refnos = convert_refno_vec_from_vec_string(result);
    Ok(refnos)
}

/// 遍历该refno的所有子节点，不包含叶子节点
pub async fn query_travel_children_with_out_leaf_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    for c in 1..10 inbound @id @@pdms_edges
    filter length(
        for z in 1 inbound c._id @@pdms_edges
            return 1
        ) != 0
    return c._key
    ", ).bind_var("id", refno_aql)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result: Vec<String> = arango_database.aql_query(aql).await?;
    let refnos = convert_refno_vec_from_vec_string(result);
    Ok(refnos)
}

/// 遍历refno只获取指定类型数组的refnos
pub async fn query_travel_children_with_types_and_cata_hash(
    arango_database: &ArDatabase,
    refno: RefU64,
    att_types: &[&str],
    check_parent: bool,
    skip_exist: bool,
) -> anyhow::Result<Vec<CataHashRefnoKV>> {
    // let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if check_parent {
        AqlQuery::new(
            "\
With @@pdms_eles,@@pdms_edges
FOR v,e,p in 0..10 INBOUND @id @@pdms_edges
    let parent = p.vertices[-2]
    filter v.cata_hash != null
    filter parent.noun in @nouns
    let s = document(pdms_inst_geos, to_string(v.cata_hash))
    filter !@skip_exist or s == null
    COLLECT exist = s, cata_group=to_string(v.cata_hash) into g
    return {
        cata_hash: cata_group,
        exist_geo: exist,
        group_refnos: g[*].v._key,
    }
    ", ).bind_var("skip_exist", skip_exist)
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
    } else {
        AqlQuery::new(
            "\
With @@pdms_eles,@@pdms_edges
FOR v,e,p in 0..10 INBOUND @id @@pdms_edges
    filter v.noun in @nouns
    filter v.cata_hash != null
    let s = document(pdms_inst_geos, to_string(v.cata_hash))
    filter !@skip_exist or s == null
    COLLECT exist = s, cata_group=to_string(v.cata_hash) into g
    return {
        cata_hash: cata_group,
        exist_geo: exist,
        group_refnos: g[*].v._key,
    }
    ",
        )
            .bind_var("skip_exist", skip_exist)
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
    };
    // dbg!(&aql);
    let r: Vec<CataHashRefnoKV> = arango_database.aql_query(aql).await?;
    Ok(r)
}

/// 遍历refno只获取指定类型数组的refnos
///
/// is_parent : 指定 parent的类型
pub async fn query_travel_children_with_types_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
    att_types: &[&str],
    is_parent: bool,
) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if is_parent {
        AqlQuery::new(
            "\
    With @@pdms_eles,@@pdms_edges
    FOR v,e,p in 0..10 INBOUND @id @@pdms_edges
    let parent = p.vertices[-2]
    Filter parent.noun in @nouns
    return v",
        )
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
    } else {
        AqlQuery::new(
            "\
    With @@pdms_eles,@@pdms_edges
    FOR v in 0..10 INBOUND @id @@pdms_edges
    Filter v.noun in @nouns
    return v",
        )
            .bind_var("id", refno_aql)
            .bind_var("nouns", att_types)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
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
pub async fn query_travel_children_with_type_aql(
    arango_database: &ArDatabase,
    refno: RefU64,
    att_type: &str,
) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    FOR z in 0..10 INBOUND @id @@pdms_edges
    Filter z.noun == @noun
    return {
        '_key':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':0,
    }",
    )
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
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

pub async fn query_refnos_travel_children_with_type_aql(
    arango_database: &ArDatabase,
    refnos: &[RefU64],
    att_type: Vec<String>,
) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = refnos
        .into_iter()
        .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "\
    With @@pdms_eles,@@pdms_edges
    let eles = ( for refno in @id
    FOR z in 0..100 INBOUND refno @@pdms_edges
        filter POSITION(@noun,z.noun)
        return {
            '_key':z._key,
            'owner':z.owner,
            'name':z.name,
            'noun':z.noun,
            'version':0,
            'children_count':0,
        })
    return UNIQUE(eles)",
    )
        .bind_var("id", refno_aql)
        .bind_var("noun", att_type)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
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

pub async fn query_refno_from_site_zone_name(
    arango_database: &ArDatabase,
    site_name: String,
    zone_name: String,
    att_type: String,
) -> anyhow::Result<Vec<RefU64>> {
    return if zone_name != "\"\"" {
        let aql = AqlQuery::new(
            r"
        With @@pdms_eles,@@pdms_edges
        FOR site IN @@pdms_eles
            FILTER site.noun == 'SITE' AND Contains(site.name , @site_name)
            FOR c IN 1 INBOUND site pdms_edges
                Filter Contains(c.name, @zone_name)
                FOR z in 1..4 INBOUND c pdms_edges
                    Filter z.noun == @noun
                    RETURN z._key",
        )
            .bind_var("site_name", site_name)
            .bind_var("zone_name", zone_name)
            .bind_var("noun", att_type)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
        let result: Vec<String> = arango_database.aql_query(aql).await?;
        Ok(convert_refno_vec_from_vec_string(result))
    } else {
        let aql = AqlQuery::new(
            r"
        With @@pdms_eles,@@pdms_edges
        FOR site IN @@pdms_eles
            FILTER site.noun == 'SITE' AND Contains(site.name , @site_name)
                FOR c IN 1..5 INBOUND site pdms_edges
                    FILTER c.noun == @noun
                    RETURN c._key",
        )
            .bind_var("site_name", site_name)
            .bind_var("noun", att_type)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
        let result: Vec<String> = arango_database.aql_query(aql).await?;
        Ok(convert_refno_vec_from_vec_string(result))
    };
}

/// 返回同层级的所有参考号，并按照 pdms 树的顺序排序
pub async fn query_sibl_level_refnos(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<Vec<RefU64>> {
    let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    // in 是该 refno 下面
    let aql_in = AqlQuery::new(
        r"
        With @@pdms_eles,@@sibl_edges
        for v in 1..1000 inbound @id sibl_edges
            return v._key",
    )
        .bind_var("id", refno_url.clone())
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@sibl_edges", AQL_SIBL_EDGES_COLLECTION);
    let result: Vec<String> = database.aql_query(aql_in).await.unwrap_or(Vec::new());
    if result.is_empty() {
        return Ok(vec![]);
    }
    let in_refnos = convert_refno_vec_from_vec_string(result);
    // out 是该 refno 上面
    let aql_out = AqlQuery::new(
        r"
        With @@pdms_eles,@@sibl_edges
        for v in 1..1000 outbound @id @@sibl_edges
            return v._key",
    )
        .bind_var("id", refno_url)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@sibl_edges", AQL_SIBL_EDGES_COLLECTION);
    let result: Vec<String> = database.aql_query(aql_out).await?;
    let mut out_refnos = convert_refno_vec_from_vec_string(result);
    out_refnos.push(refno);
    Ok([out_refnos, in_refnos].concat())
}

/// 返回该参考号的上一个或者下一个节点的attr，b_pre: true 上一个 , false 下一个
pub async fn query_pre_or_next_node(
    refno: RefU64,
    b_pre: bool,
    database: &ArDatabase,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<Option<AttrMap>> {
    let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if b_pre {
        AqlQuery::new(
            "\
        With @@pdms_eles,@@sibl_edges
        for v in 1 outbound @key @@sibl_edges
            return v._key
    ",
        )
            .bind_var("key", refno_url)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@sibl_edges", AQL_SIBL_EDGES_COLLECTION)
    } else {
        AqlQuery::new(
            "\
        With @@pdms_eles,@@sibl_edges
        for v in 1 inbound @key @@sibl_edges
            return v._key
    ",
        )
            .bind_var("key", refno_url)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@sibl_edges", AQL_SIBL_EDGES_COLLECTION)
    };
    let aql_result = database.aql_query::<String>(aql).await;
    // 如果为该层第一个或者最后一个 则返回 None
    if aql_result.is_err() {
        return Ok(None);
    }
    let aql_result = aql_result.unwrap();
    let aql_result = convert_refno_vec_from_vec_string(aql_result);
    if aql_result.is_empty() {
        return Ok(None);
    }
    let attr = query_attr(aql_result[0], aios_mgr, None).await?;
    Ok(Some(attr))
}

/// 返回指定参考号下所有的负实体以及和负实体同级的其他节点
pub async fn query_travel_children_filter_negative_sibl_nodes(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<HashMap<RefU64, Vec<PdmsElement>>> {
    let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    // todo negatives并没有去掉同层级的其他节点，同层级的所有节点都查询了一遍，应该一层只用查询一遍
    let aql = AqlQuery::new(
        "\
        With @@pdms_eles,@@pdms_edges,@@sibl_edges
        let negatives = ( FOR v in 0..10 INBOUND @key @@pdms_edges
                    filter POSITION(@negative_nouns, v.noun)
                    return v._id )
        let sibls = ( for negative in negatives
                for v in 0..1000 ANY negative @@sibl_edges
                return {
                    '_key':v._key,
                    'owner':v.owner,
                    'name':v.name,
                    'noun':v.noun,
                    'version':0,
                    'children_count':0,
                } )
        return UNIQUE(sibls)",
    )
        .bind_var("key", refno_url)
        .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec())
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@sibl_edges", AQL_SIBL_EDGES_COLLECTION);
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
pub async fn filter_negative_sibl_from_refnos(
    refnos: &Vec<RefU64>,
    database: &ArDatabase,
) -> anyhow::Result<Vec<RefU64>> {
    let keys = refnos
        .into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "\
        With @@pdms_eles,@@pdms_edges
        for refno in @keys
        let contains_negative = ( for v in 0..1000 ANY refno @@sibl_edges
                            filter POSITION(@negative_nouns ,v.noun)
                            return 1 )
        filter Length(contains_negative) == 0
        return refno
    ",
    )
        .bind_var("keys", keys)
        .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec())
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result = database.aql_query::<String>(aql).await?;
    Ok(result
        .into_iter()
        .filter_map(|r| RefU64::from_arangodb_refno_str(&r))
        .collect::<Vec<_>>())
}

/// 用户自定义条件模糊查询 aql
pub async fn vague_query_refnos_user_set_aql(
    request: VagueSearchRequest,
    database: &ArDatabase,
) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut keys = request
        .filter_refnos
        .into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno()))
        .collect::<Vec<_>>();
    let mut condition_map = HashSet::new();
    request.filter_condition.iter().for_each(|x| {
        if x.0 == "MAJOR" {
            condition_map.insert(x.1.clone());
        }
    });
    // 对专业进行过滤
    // 先进行专业过滤查询
    if !condition_map.is_empty() {
        let aql = format!(
            "
            With {AQL_PDMS_ELES_COLLECTION}
            for refno in {}
                let v = document(refno)
                @@major_filter_condition
                return v._id",
            serde_json::to_string(&keys).unwrap_or("[]".to_string())
        );
        let mut filter_conditions = String::new();
        for condition in &condition_map {
            match condition.0 {
                VagueSearchCondition::And => {
                    filter_conditions.push_str(&format!(
                        "filter v.major == '{}' OR v.noun != 'SITE' ",
                        condition.1
                    ));
                }
                VagueSearchCondition::Or => {
                    if condition_map.len() > 1 {
                        filter_conditions.push_str(&format!(
                            "|| v.major == '{}' OR v.noun != 'SITE' ",
                            condition.1
                        ));
                    }
                }
                VagueSearchCondition::Not => {
                    filter_conditions.push_str(&format!(
                        "filter v.major != '{}' OR v.noun != 'SITE' ",
                        condition.1
                    ));
                }
            }
        }
        let aql = aql.replace("@@major_filter_condition", &filter_conditions);
        let aql = AqlQuery::new(&aql);
        let result: Vec<String> = database.aql_query(aql).await?;
        keys = result;
    }
    // 生成aql模板
    let aql = format!(
        "\
    With {AQL_PDMS_ELES_COLLECTION}
    for refno in {}
        for v in 0..1000 inbound refno {AQL_PDMS_EDGES_COLLECTION}
        @@filter_condition
        return {{
            'refno':v._key,
            'name':v.name,
        }} ",
        serde_json::to_string(&keys).unwrap_or("[]".to_string())
    );
    // 拼接过滤条件
    let mut filter_condition = String::new();
    for (key, (condition, value)) in request.filter_condition {
        if &key == "MAJOR" {
            continue;
        };
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
                let value_aql = if value_aql.starts_with("=") {
                    value_aql[1..].to_string()
                } else {
                    value_aql
                };
                filter_condition.push_str(&format!("filter v.{} !{} ", key, value_aql));
            }
        }
    }
    // 将aql和过滤条件合并在一起
    let aql = aql.replace("@@filter_condition", &filter_condition);
    let aql = AqlQuery::new(&aql);
    let mut r = Vec::new();
    let result: Vec<PdmsRefnoNameAql> = database.aql_query(aql).await?;
    for v in result {
        if let Some(refno) = RefU64::from_url_refno(&v.refno) {
            r.push((refno, v.name));
        }
    }
    Ok(r)
}

/// 查询节点属于哪个专业和专业下的具体分类
pub async fn query_refnos_belong_major(
    refnos: Vec<RefU64>,
    database: &ArDatabase,
) -> anyhow::Result<Vec<RefnoMajor>> {
    let ids = refnos
        .into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    for id in @ids
    for v,e,p in 0..10 outbound id @@pdms_edges
    filter v != null
    filter v.major != null
    return {
        'refno':p.vertices[0]._key,
        'owner':v.owner,
        'name':v.name,
        'noun':v.noun,
        'major':v.major,
    }",
    )
        .bind_var("ids", ids)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result = database.aql_query::<PdmsElementWithMajor>(aql).await?;
    let mut majors = HashMap::new();
    // 查询到该参考号分别在zone和site下属于哪个专业，并将这两个专业代码合并到一个结构体下
    for r in result {
        if majors.contains_key(&r.refno) {
            let mut major: &mut RefnoMajor = majors.get_mut(&r.refno).unwrap();
            if &r.noun == "SITE" {
                major.major = r.major;
            } else {
                major.major_classify = r.major;
            }
        } else {
            if &r.noun == "SITE" {
                majors.entry(r.refno).or_insert(RefnoMajor {
                    refno: r.refno.to_refno_str(),
                    major: r.major,
                    major_classify: "".to_string(),
                });
            } else {
                majors.entry(r.refno).or_insert(RefnoMajor {
                    refno: r.refno.to_refno_str(),
                    major: "".to_string(),
                    major_classify: r.major,
                });
            }
        }
    }
    Ok(majors.into_iter().map(|major| major.1).collect::<Vec<_>>())
}

/// 查询参考号集合分别属于哪个层级
///
/// 层级：从owner一直到某个类型的name
pub async fn query_refnos_belong_level_aql(
    refno: Vec<RefU64>,
    att_type: &str,
    database: &ArDatabase,
) -> anyhow::Result<Vec<VagueSearchExportAqlData>> {
    let refnos = refno
        .into_iter()
        .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
        .collect::<Vec<String>>();
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    for refno in @refnos
        let level = ( for o in 1..10 outbound refno @@pdms_edges
                PRUNE o.noun == @noun
                return o.name  )
        let element = document(refno)
        return {
            'refno':element._key,
            'name': element.name,
            'level':level,
            'att_type':element.noun
        }",
    )
        .bind_var("refnos", refnos)
        .bind_var("noun", att_type)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result = database.aql_query::<VagueSearchExportAqlData>(aql).await?;
    Ok(result)
}

/// 选择的参考号以下的节点中，通过 name 查找其参考号
pub async fn query_refno_from_names_under_select_refno(select_refnos: Vec<RefU64>, names: Vec<String>, database: &ArDatabase)
                                                       -> anyhow::Result<Vec<PdmsRefnoNameAql>> {
    let ids = RefU64::to_arangodb_ids(AQL_PDMS_ELES_COLLECTION, select_refnos);
    let aql = AqlQuery::new("\
    With @@pdms_eles,@@pdms_edges
    for id in @ids
    for v in 0..20 inbound id @@pdms_edges
        filter v != null
        filter v.name in @names
        return {
            'refno': v._key,
            'name': v.name,
        }
    ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("ids", ids)
        .bind_var("names", names);
    let result = database.aql_query::<PdmsRefnoNameAql>(aql).await?;
    Ok(result)
}

/// 查找选中节点以下的uda type,
///
/// base_type ： uda type 基于的基础类型 例如: ZONE 等
pub async fn get_uda_type_refnos_from_select_refnos(select_refnos: Vec<RefU64>,
                                                    uda_type: &str,
                                                    base_type: &str,
                                                    aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<PdmsElement>> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    // 因为typex是解析时已经dehash过了,不是对应的udna，
    // 传入得uda_type是udna，需要统一转为 db1_dehash_const 的值
    let uda_type_ukey = db1_hash(format!(":{}", uda_type).as_str());
    let uda_type_db_dehash = db1_dehash_const(uda_type_ukey);
    // 先查找到所有的 base_type ， 再通过 typex 进行过滤
    let type_refnos = query_refnos_travel_children_with_type_aql(&database,
                                                                 &select_refnos, vec![base_type.to_string()]).await?;
    for refno in type_refnos {
        let Ok(attr) = aios_mgr.get_attr(refno.refno).await else { continue; };
        let typex = attr.get_typex().to_string();
        if uda_type_db_dehash == typex {
            result.push(refno.into());
        }
    }
    Ok(result)
}

/// 查询该节点下某个类型节点的name，包含选中节点的name,并返回该节点
pub async fn query_refnos_contains_select_name(select_refnos: Vec<RefU64>, att_type: &str, database: &ArDatabase) -> anyhow::Result<Vec<PdmsElement>> {
    let ids = RefU64::to_arangodb_ids(&AQL_PDMS_ELES_COLLECTION, select_refnos);
    let aql = AqlQuery::new("
    with @@pdms_eles,@@pdms_edges
    for id in @ids
    let ele = document(id)
    let ele_name = SUBSTRING(ele.name,1) // 去掉name前面的 '/'
    for v in 0..5 inbound id pdms_edges
    filter v.noun == @noun
    filter CONTAINS(v.name,ele_name )
    return {
            '_key':v._key,
            'owner':v.owner,
            'name':v.name,
            'noun':v.noun,
            'version':0,
            'children_count':0,
        }").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("ids", ids)
        .bind_var("noun", att_type);
    let result = database.aql_query::<PdmsElement>(aql).await?;
    Ok(result)
}

/// 查询选中节点下 某些类型的节点的数据，且经过了某些类型
///
/// through_types ： 经过的类型
///
/// final_types： 最终收集的某些类型的 pdms_element 信息
pub async fn query_type_refnos_through_types(select_refnos: Vec<RefU64>, through_types: Vec<String>,
                                             final_types: Vec<String>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsElement>> {
    let ids = RefU64::to_arangodb_ids(AQL_PDMS_ELES_COLLECTION, select_refnos);
    let aql = AqlQuery::new("
    with @@pdms_eles,@@pdms_edges
    for id in @ids
    let owners = (
    for v in 0..5 inbound id pdms_edges
        filter v != null
        filter v.noun in @through_types
        return v
    )
    for owner in owners
        for o in 0..10 inbound owner._id pdms_edges
        filter o != null
        filter o.noun in @final_types
        return {
            '_key':o._key,
            'owner':owner._key,
            'name':o.name,
            'noun':o.noun,
            'version':0,
            'children_count':0,
        }
    ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("ids", ids)
        .bind_var("through_types", through_types)
        .bind_var("final_types", final_types);
    let result = database.aql_query::<PdmsElement>(aql).await?;
    Ok(result)
}

/// 通过房间名查询房间所属的site
pub async fn query_room_belong_site_name(rooms: Vec<String>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsOwnerNameAql>> {
    let aql = AqlQuery::new("
    With @@pdms_eles,@@pdms_edges,@@room_eles
    for r in @@room_eles
    filter r.name in @rooms
    for v in 1..5 outbound concat('pdms_eles/',r._key) pdms_edges
        filter v != null
        filter v.noun == 'SITE'
        return {
            refno: r._key,
            name:r.name,
            owner: v._key ,
            owner_noun: v.noun,
            owner_name: v.name
        }").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@room_eles", AQL_ROOM_ELES_COLLECTION)
        .bind_var("rooms", rooms);
    let result = database.aql_query::<PdmsOwnerNameAql>(aql).await?;
    Ok(result)
}

/// owner的children中，第一个类型为 att_type 的 element
///
/// filter_noun 找到第一个 att_type为 filter_noun 的数据
pub async fn query_first_children(refnos: Vec<RefU64>, filter_noun: &str, database: &ArDatabase) -> anyhow::Result<Vec<PdmsElement>> {
    let ids = refnos.into_iter().map(|refno| refno.to_url_refno()).collect::<Vec<String>>();
    // 若 filter_noun 以 ! 开头 则排除某类型后，取第一个 例如 "!ATTA"
    let filter_str = if filter_noun.starts_with("!") {
        format!("filter c.noun != '{}'", &filter_noun[1..])
    } else if filter_noun.is_empty() {
        // 若 filter_noun 为空，则不做过滤
        format!("// empty")
    } else {
        // 若 filter_noun 为正常值,则只需要第一个出现为某类型的元素
        format!("filter c.noun == '{}'", filter_noun)
    };
    // 生成查询 aql
    let aql_str = format!(r#"
    With @@pdms_eles,@@pdms_edges
    for id in @ids
    let owner = (
    for v in 1 outbound concat('pdms_eles/',id) pdms_edges
        filter v != null
        return {{
            '_id':v._id,
            'key':id,
        }}
    )
    let r = (
    for o in owner
        for c,e in 1 inbound o._id pdms_edges
        filter c != null
        //filter_noun
        sort e.order
        limit 1
        return {{
            _key:c._key,
            owner:o.key,
            name:c.name,
            noun:c.noun,
            version:0,
            children_count:0,
    }})
    return r[0]
    "#);
    // 对传入的filter_noun 的不同情况进行替换
    let filter_aql_str = aql_str.replace("//filter_noun", &filter_str);
    let aql = AqlQuery::new(filter_aql_str.as_str())
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("ids", ids);
    let result = database.aql_query::<PdmsElement>(aql).await?;
    Ok(result)
}

#[tokio::test]
async fn test_vague_query_refnos_user_set_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let request = VagueSearchRequest {
        filter_refnos: vec![
            RefU64::from_refno_str("24383/66456").unwrap(),
            RefU64::from_refno_str("24381/100675").unwrap(),
        ],
        filter_condition: vec![
            ("MAJOR".to_string(), (And, "T".to_string())),
            ("NAME".to_string(), (And, "*WCC*".to_string())),
            ("TYPE".to_string(), (And, "PIPE".to_string())),
        ],
    };
    let result = vague_query_refnos_user_set_aql(request, &database).await?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_query_refnos_from_names() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let names = vec!["/MAIJIAN-NEW-VARY-NGMS".to_string()];
    let result = query_refnos_from_names(names, &database, None).await?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_get_uda_type_refnos_from_select_refnos() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let select_refnos = vec![RefU64::from_url_refno("9304_2").unwrap()];
    let refnos = get_uda_type_refnos_from_select_refnos(select_refnos, "STDMODELITEM", "ZONE", &aios_mgr).await?;
    dbg!(&refnos);
    Ok(())
}

#[tokio::test]
async fn test_query_refnos_from_names_fulltext() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let names = vec!["1WCC778VN".to_string(), "/1WCC0578".to_string(), "/-RX-CCV-R02-13".to_string()];
    let result = query_refnos_from_names_fulltext(names, &database).await?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_query_first_children() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refnos = vec![RefU64::from_refno_str("24383/66687").unwrap()];
    let result = query_first_children(refnos.clone(), "VALV", &database).await?;
    dbg!(&result);
    let result = query_first_children(refnos.clone(), "!ATTA", &database).await?;
    dbg!(&result);
    let result = query_first_children(refnos, "", &database).await?;
    dbg!(&result);
    Ok(())
}