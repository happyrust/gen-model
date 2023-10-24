use crate::api::attr::query_attr;
use crate::aql_api::*;
use crate::consts::{AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION, AQL_PDMS_INST_GEO_COLLECTION, AQL_PDMS_INST_INFO_COLLECTION, AQL_SIBL_EDGES_COLLECTION};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use aios_core::options::DbOption;
use aios_core::pdms_types::{
    CataHashRefnoKV, EleTreeNode, NamedAttrMap, NamedAttrValue, PdmsElement, RefU64, RefU64Vec,
    GENRAL_NEG_NOUN_NAMES,
};
use aios_core::pdms_user::*;
use aios_core::three_dimensional_review::VagueSearchCondition::And;
use aios_core::three_dimensional_review::*;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
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
use aios_core::pdms_pluggin::heat_dissipation::InstPointMap;
use crate::graph_db::structs::{PdmsEleEdge, PdmsEleData, PdmsMdbEdge};

pub type IndexNamedAttMap = IndexMap<String, NamedAttrValue>;

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct QueryAttsParam {
    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub refnos: Vec<RefU64>,
    #[serde(default)]
    pub att_names: Vec<String>,
    #[serde(default)]
    pub noun_filter: Vec<String>,
    #[serde(default)]
    pub get_children: bool,
}

impl AiosDBManager {
    /// 获取属性集合,每次请求只处理同一个类型的数据, 指定返回哪些属性名称的数据，如果为空，则为全部 by dpc
    pub async fn get_named_attrs_by_param(
        &self,
        param: &QueryAttsParam,
    ) -> anyhow::Result<Vec<IndexNamedAttMap>> {
        self.query_named_attrs(
            &param.refnos,
            param.att_names.iter().map(|x| x.as_str()),
            param.noun_filter.iter().map(|x| x.as_str()),
            param.get_children,
        ).await
    }

    /// 获取属性集合,每次请求只处理同一个类型的数据, 指定返回哪些属性名称的数据，如果为空，则为全部 by dpc
    pub async fn query_named_attrs(
        &self,
        refnos: impl IntoIterator<Item=&RefU64>,
        att_names: impl IntoIterator<Item=&str>,
        noun_filter: impl IntoIterator<Item=&str>,
        get_children: bool,
    ) -> anyhow::Result<Vec<IndexNamedAttMap>> {
        let att_names_vec = att_names.into_iter().collect::<Vec<&str>>();
        let mut contains_trans = att_names_vec.iter().any(|&x| x == "W_POS" || x == "W_ORI");
        let filter_set = noun_filter.into_iter().collect::<HashSet<_>>();
        let mut result = vec![];
        let target_refnos: Vec<RefU64> = if get_children {
            refnos
                .into_iter()
                .map(|x| self.get_children_from_localdb(*x).unwrap_or_default().0)
                .flatten()
                .filter(|x| filter_set.is_empty() ||
                    filter_set.contains(&self.get_type_name(*x).as_str())
                )
                .collect::<Vec<_>>()
        } else {
            refnos.into_iter().cloned().filter(|x| filter_set.is_empty() ||
                filter_set.contains(&self.get_type_name(*x).as_str())
            ).collect::<Vec<_>>()
        };
        for refno in target_refnos {
            let mut attr_values = IndexNamedAttMap::new();
            let Ok(attr) = self.get_attr_from_localdb(refno) else {
                result.push(attr_values);
                continue;
            };
            let world_transform = if contains_trans {
                self.get_world_transform(refno)
                    .unwrap_or_default()
                    .unwrap_or_default()
            } else {
                Default::default()
            };

            for &name in att_names_vec.iter() {
                let translation = world_transform.translation;
                if name == "W_POS" {
                    attr_values.insert(
                        name.to_owned(),
                        NamedAttrValue::F32VecType(vec![
                            translation.x,
                            translation.y,
                            translation.z,
                        ]),
                    );
                } else if name == "W_ORI" {
                    let rotation = quat_to_pdms_ori_str(&world_transform.rotation);
                    attr_values.insert(name.to_owned(), NamedAttrValue::StringType(rotation));
                } else if let Some(value) = attr.get_att_by_name(name) {
                    attr_values.insert(name.to_owned(), NamedAttrValue::from(value));
                } else {
                    attr_values.insert(name.to_owned(), NamedAttrValue::StringType("".to_string()));
                }
            }

            result.push(attr_values);
        }

        Ok(result)
    }


    ///通过指定的过滤条件来查询
    pub async fn query_ele_nodes_by_expression(&self, expression: &str) -> anyhow::Result<Vec<PdmsEleData>> {
        let aql_string = format!(r#"
            with pdms_eles
            for v in pdms_eles
                filter {}
                sort v.order
                return distinct v
        "#, expression);
        let aql =
            AqlQuery::new(aql_string.as_str());

        let result = self.get_arango_db().await?
            .aql_query::<PdmsEleData>(aql).await.unwrap();
        Ok(result)
    }

    ///通过指定的过滤条件来查询
    pub async fn query_ele_edges_by_expression(&self, expression: &str) -> anyhow::Result<Vec<PdmsEleEdge>> {
        let aql_string = format!(r#"
            with pdms_edges
            for v in pdms_edges
                filter {}
                sort v.order
                return distinct v
        "#, expression);
        let aql =
            AqlQuery::new(aql_string.as_str());

        let result = self.get_arango_db().await?
            .aql_query::<PdmsEleEdge>(aql).await?;
        Ok(result)
    }

    ///通过指定的过滤条件来查询
    pub async fn query_mdb_by_expression(&self, expression: &str) -> anyhow::Result<Vec<PdmsMdbEdge>> {
        let aql_string = format!(r#"
            with pdms_mdbs
            for v in pdms_mdbs
                filter {}
                sort v.order
                return distinct v
        "#, expression);
        let aql =
            AqlQuery::new(aql_string.as_str());

        let result = self.get_arango_db().await?
            .aql_query::<PdmsMdbEdge>(aql).await?;
        Ok(result)
    }


    // pub async fn query_world_element(&self, expression: &str) -> anyhow::Result<PdmsElement> {
    // let aql_string = format!(r#"
    //     with pdms_mdbs, pdms_eles
    //     for v,e,p in pdms_mdbs
    //         filter {}
    //         let doc = document(pdms_eles, v._key)
    //         return
    //         { '_key':child._key,
    //     'owner':child.owner,
    //     'name':child.name,
    //     'noun':child.noun,
    //     'order': child.order,
    //     'children_count':length(for c in 1 inbound child._id pdms_edges
    //                         return 1 )}
    //
    //
    // "#, expression);
    // let aql =
    //     AqlQuery::new(aql_string.as_str());
    //
    // let result = self.get_arango_db().await?
    //     .aql_query::<PdmsMdbEdge>(aql).await?;
    // Ok(result)
    // }
}

/// 通过 refnos 查询 对应的 name
pub async fn query_names_from_refnos_aql(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsRefnoNameAql>> {
    let ids = RefU64::to_arangodb_ids(&AQL_PDMS_ELES_COLLECTION, refnos);
    let aql = AqlQuery::new("
    with @@pdms_eles
    for id in @ids
    let ele = document(id)
    return {
        'refno': ele._key,
        'name': ele.name,
    }").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("ids", ids);
    let result = database.aql_query::<PdmsRefnoNameAql>(aql).await?;
    Ok(result)
}

/// 查询多个参考号的点集
pub async fn query_refnos_point_map_aql(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<InstPointMap>> {
    let ids = RefU64::to_arangodb_ids(&AQL_PDMS_ELES_COLLECTION, refnos);
    let aql = AqlQuery::new("
    with @@pdms_eles,@@pdms_edges,@@pdms_inst_infos,@@pdms_inst_geos
    for id in @ids
        let v = document(id)
        filter v != null
        let cata_hash = document(@@pdms_inst_infos,v._key)
        let hash = cata_hash.cata_hash == null ? cata_hash._key : cata_hash.cata_hash
        let geo = document(@@pdms_inst_geos,hash)
        filter geo != null
        return {
            'refno': v._key,
            'att_type': v.noun,
            'ptset_map': geo.ptset_map
        }").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@pdms_inst_infos", AQL_PDMS_INST_INFO_COLLECTION)
        .bind_var("@pdms_inst_geos", AQL_PDMS_INST_GEO_COLLECTION)
        .bind_var("ids", ids);
    let result = database.aql_query::<InstPointMap>(aql).await?;
    Ok(result)
}