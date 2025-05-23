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
use std::collections::BTreeMap;

/// 保存element数据到版本管理
pub async fn save_pes(
    db_basic: &DbBasicData,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    db_num: i32,
    file_name: &str,
    db_type: &str,
    option: &DbOption,
    output: flume::Sender<SenderJsonsData>,
) -> anyhow::Result<()> {
    use itertools::Itertools;
    
    let keys = total_attr_map.iter().map(|x| *x.key()).collect::<Vec<_>>();
    let mut chunk_index = 0;
    let mut sql = String::new();
    
    // 用于收集dbnum_info_table的数据
    let mut dbnum_info_map: BTreeMap<u64, (i32, i32, i32, u64)> = BTreeMap::new(); // ref_0 -> (dbnum, count, max_sesno, max_ref1)
    
    for chunk in keys.chunks(option.pe_chunk as _) {
        let mut insert_jsons = Vec::new();
        for &refno in chunk {
            let att_map = total_attr_map.get(&refno).unwrap();
            let pe_data = att_map.pe(db_num);
            let json = pe_data.gen_sur_json(Some(refno.to_pe_key()));
            insert_jsons.push(json);
            
            // 从refno中提取ref_0和ref_1
            // RefU64内部是u64，在pe:ref_0_ref_1格式中，ref_0是高32位，ref_1是低32位
            let refno_u64 = refno.0;
            let ref_0 = (refno_u64 >> 32) as u64;
            let ref_1 = (refno_u64 & 0xFFFFFFFF) as u64;
            
            // 获取sesno，从pe_data中获取
            let sesno = pe_data.sesno as i32;
            
            // 更新dbnum_info_map
            dbnum_info_map.entry(ref_0)
                .and_modify(|(_, count, max_sesno, max_ref1)| {
                    *count += 1;
                    *max_sesno = (*max_sesno).max(sesno);
                    *max_ref1 = (*max_ref1).max(ref_1);
                })
                .or_insert((db_num, 1, sesno, ref_1));
        }
        
        output.send_async(SenderJsonsData::PEJson(insert_jsons)).await.expect("send pes error");
        chunk_index += 1;
    }
    
    // 生成dbnum_info_table的更新数据
    if !dbnum_info_map.is_empty() {
        let mut dbnum_info_updates = Vec::new();
        for (ref_0, (dbnum, count, max_sesno, max_ref1)) in dbnum_info_map {
            // 使用UPSERT语法，类似EVENT中的逻辑，现在添加file_name和db_type字段
            let sql = format!(
                "UPSERT dbnum_info_table:{} SET dbnum = {}, count = count?:0 + {}, sesno = math::max([sesno?:0, {}]), max_ref1 = math::max([max_ref1?:0, {}]), file_name = '{}', db_type = '{}';",
                ref_0, dbnum, count, max_sesno, max_ref1, file_name, db_type
            );
            dbnum_info_updates.push(sql);
        }
        
        // 分批发送dbnum_info更新
        for chunk in dbnum_info_updates.chunks(option.pe_chunk as _) {
            output.send_async(SenderJsonsData::DbnumInfoUpdate(chunk.to_vec())).await.expect("send dbnum_info error");
        }
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
