use crate::api::element::gen_pdms_element_insert_sql;
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::versioned_db::database::{SenderJsonsData};
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::db::*;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::tool::db_tool::db1_hash;
use aios_core::SUL_DB;
use config::File;
use dashmap::DashMap;
use dashmap::DashSet;
use futures::StreamExt;
use itertools::Itertools;
use log::{error, info};
use petgraph::algo::all_simple_paths;
use petgraph::graph::Graph;
use petgraph::graph::NodeIndex;
use petgraph::graphmap::GraphMap;
use petgraph::graphmap::UnGraphMap;
use petgraph::prelude::DiGraphMap;
use petgraph::visit::IntoEdgesDirected;
use petgraph::Directed;
use petgraph::Undirected;
use rayon::prelude::*;
#[cfg(feature = "sql")]
use sqlx::Executor;
#[cfg(feature = "sql")]
use sqlx::{MySql, Pool};
use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// 保存element数据到版本管理
pub async fn save_pes(
    db_basic: &DbBasicData,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    db_num: i32,
    option: &DbOption,
    output: flume::Sender<SenderJsonsData>,
) -> anyhow::Result<()> {
    use itertools::Itertools;
    let keys = total_attr_map.iter().map(|x| *x.key()).collect::<Vec<_>>();
    let mut chunk_index = 0;
    let mut sql = String::new();
    for chunk in keys.chunks(option.pe_chunk as _) {
        let mut insert_jsons = Vec::new();
        for &refno in chunk {
            let att_map = total_attr_map.get(&refno).unwrap();
            let json = att_map.pe(db_num).gen_sur_json(None, Some(refno.to_pe_key()));
            insert_jsons.push(json);
        }
        output.send_async(SenderJsonsData::PEJson(insert_jsons)).await.expect("send pes error");
        chunk_index += 1;
    }
    Ok(())
}

#[cfg(feature = "sql")]
pub async fn save_pes_mysql(
    db_basic: &DbBasicData,
    project: &str,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    project_maps: &HashMap<String, Pool<MySql>>,
    option: &DbOption,
    db_num: i32,
    output: &flume::Sender<SenderSql>,
) {
    let keys = total_attr_map.iter().map(|x| *x.key()).collect::<Vec<_>>();
    let debug_refnos: Vec<RefU64> = option
        .debug_root_refnos
        .as_ref()
        .map(|x| {
            x.iter()
                .map(|x| RefU64::from_str(x).unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let is_debug = !debug_refnos.is_empty();

    let children_map = &db_basic.children_map;

    for chunk in keys.chunks(option.pe_chunk as _) {
        let mut insert_sql = String::new();
        for &refno in chunk {
            if is_debug && !debug_refnos.contains(&refno) {
                continue;
            }
            let att_map = total_attr_map.get(&refno).unwrap();
            let sql = gen_pdms_element_insert_sql(att_map.value(), db_num, children_map);
            if !sql.is_empty() {
                insert_sql.push_str(&sql);
            }
        }
        let mut sql = format!(
            "INSERT IGNORE INTO {PDMS_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, NUMBDB , ORDER_NUM,CHILDREN_COUNT, IS_DEL  ) VALUES {insert_sql}", );
        if option.replace_dbs {
            sql = sql.replace("INSERT IGNORE", "REPLACE");
        }
        sql.remove(sql.len() - 1);
        // output.send(MysqlSql((project.to_string(),sql))).expect("send pdmselement mysql sql failed");
        let Some(pool) = project_maps.get(project) else {
            continue;
        };
        let mut conn = pool.acquire().await.expect("get pool failed");
        match conn.execute(sql.as_str()).await {
            Ok(_) => {}
            Err(e) => {
                dbg!(e.to_string());
                dbg!(&sql);
            }
        }
    }
}


//使用insert relations 去保存图数据关联关系
pub async fn save_pe_relates(db_basic: &DbBasicData, output: flume::Sender<SenderJsonsData>) {
    let mut all_relate_jsons = vec![];
    for kv in &db_basic.children_map {
        let owner = kv.0;
        let children = kv.1;
        if children.is_empty() {
            continue;
        }
        let relate_json = children
            .iter()
            .enumerate()
            .map(|(i, child)| {
                let cp = child.to_pe_key();
                let op = owner.to_pe_key();
                format!("{{ id: pe_owner:[{1}, {i}], in: {0}, out: {1} }}", cp, op)
            })
            .collect::<Vec<String>>();
        all_relate_jsons.extend_from_slice(&relate_json);
        if all_relate_jsons.len() >= 500 {
            output.send(SenderJsonsData::PERelateJson(std::mem::take(&mut all_relate_jsons))).expect("send pe_relates error");
        }
    }
    if !all_relate_jsons.is_empty() {
        output.send(SenderJsonsData::PERelateJson(std::mem::take(&mut all_relate_jsons))).expect("send pe_relates error");
    }
}
