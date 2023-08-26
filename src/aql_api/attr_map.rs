use crate::api::attr::query_attr;
use crate::aql_api::*;
use crate::consts::{
    AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION, AQL_SIBL_EDGES_COLLECTION,
};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
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
use std::str::FromStr;

pub type IndexNamedAttMap = IndexMap<String, NamedAttrValue>;

impl AiosDBManager {

    /// 获取属性集合,每次请求只处理同一个类型的数据, 指定返回哪些属性名称的数据，如果为空，则为全部 by dpc
    ///
    pub async fn get_named_attrs(
        &self,
        refnos: impl IntoIterator<Item = &RefU64>,
        att_names: impl IntoIterator<Item = &str>,
        get_children: bool,
        noun_filter: impl IntoIterator<Item = &str>,
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
            let Ok(attr) = self.get_attr_from_localdb(refno) else {
                continue;
            };
            let mut attr_values = IndexNamedAttMap::new();
            let world_transform = if contains_trans {
                self.get_world_transform(refno)
                    .await
                    .unwrap_or(None)
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
}
