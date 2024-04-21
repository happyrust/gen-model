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
use parse_pdms_db::parse::DbBasicData;
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
use sea_orm::entity::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use tokio::time::Instant;

fn gen_full_name(
    refno: RefU64,
    db_basic: &DbBasicData,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
) -> String {
    let mut cur_refno = refno;
    let mut name_ancestors = vec![];
    let children_map = &db_basic.children_map;
    let mut found_exist_name = false;
    let is_debug = refno == "17496/269118".into();
    while cur_refno.is_valid() {
        if let Some(cur_att) = total_attr_map.get(&cur_refno) {
            if let Some(name) = cur_att.get_name() {
                name_ancestors.push(name);
                if is_debug {
                    dbg!(&name_ancestors);
                }
                found_exist_name = true;
            } else {
                let owner = cur_att.get_owner();
                let noun = cur_att.get_type_str();
                let mut noun_idx = 1;
                if let Some(children) = children_map.get(&owner) {
                    //需要再保存一个noun index ，即这个ele 在children中是同类型的第几个
                    noun_idx = children
                        .iter()
                        .filter(|(_, n)| n == noun)
                        .position(|(c, _)| *c == refno)
                        .unwrap_or_default()
                        + 1;
                }
                name_ancestors.push(format!("{} {}", noun, noun_idx));
                if is_debug {
                    dbg!(&name_ancestors);
                }
            }
            cur_refno = cur_att.get_owner();
            if is_debug {
                dbg!(cur_refno);
                dbg!(&total_attr_map.get(&cur_refno));
            }
        } else {
            break;
        }
    }
    if name_ancestors.is_empty() {
        "unset".to_owned()
    } else if name_ancestors.len() == 1 {
        name_ancestors.pop().unwrap()
    } else {
        name_ancestors.join(" OF ")
    }
}

fn gen_default_name(
    refno: RefU64,
    db_basic: &DbBasicData,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
) -> String {
    let Some(cur_att) = total_attr_map.get(&refno) else {
        return "unset".into();
    };
    let owner = cur_att.get_owner();
    let noun = cur_att.get_type_str();
    let mut noun_idx = 1;
    if let Some(children) = db_basic.children_map.get(&owner) {
        //需要再保存一个noun index ，即这个ele 在children中是同类型的第几个
        noun_idx = children
            .iter()
            .filter(|(_, n)| n == noun)
            .position(|(c, _)| *c == refno)
            .unwrap_or_default()
            + 1;
    }
    format!("{} {}", noun, noun_idx)
}

/// 保存element数据到版本管理
pub async fn save_pes(
    db_basic: &DbBasicData,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    db_num: i32,
    option: &DbOption,
    output: flume::Sender<String>,
) -> anyhow::Result<()> {
    use itertools::Itertools;
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

    let mut exist_refnos: HashSet<RefU64> = HashSet::new();
    let children_map = &db_basic.children_map;
    for chunk in keys.chunks(option.pe_chunk as _) {
        //是否需要覆盖保存数据库
        if option.replace_dbs {
            let pes = chunk.iter().map(|x| x.to_pe_key()).join(",");
            let mut resp = SUL_DB
                .query(format!("SELECT VALUE id FROM [{pes}];"))
                .await?;
            // dbg!(&resp);
            let refnos: Vec<RefU64> = resp.take(0).unwrap();
            exist_refnos.extend(refnos);
            if !exist_refnos.is_empty() {
                // dbg!(exist_refnos.len());
            }
        }
        let mut insert_jsons_str = String::new();
        let mut update_sql_str = String::new();
        for &refno in chunk {
            if is_debug && !debug_refnos.contains(&refno) {
                continue;
            }
            let att_map = total_attr_map.get(&refno).unwrap();
            let owner = att_map.get_refno_by_att_or_default("OWNER");
            let noun = att_map.get_type();
            let name = att_map.get_string("NAME");

            let ele = SPdmsElement {
                refno,
                owner,
                is_default_name: name.is_none(),
                name: name.unwrap_or(gen_default_name(refno, db_basic, total_attr_map)),
                noun,
                dbnum: db_num,
                cata_hash: att_map.cal_cata_hash(),
                status_tag: None,
                version_tag: None,
                e3d_version: att_map.get_e3d_version(),
                lock: false,
            };
            let json = ele.gen_sur_json();
            if exist_refnos.contains(&refno) {
                update_sql_str
                    .push_str(format!("UPDATE {} CONTENT {};", refno.to_pe_key(), json).as_str());
                // dbg!(&update_sql_str);
            } else {
                insert_jsons_str.push_str(&json);
                insert_jsons_str.push_str(",");
            }
        }
        let sql = format!(
            "INSERT IGNORE INTO pe [{}]; {update_sql_str}",
            insert_jsons_str
        );
        // println!("开始发送: {}", chunk.len());
        output.send(sql).expect("send pes error");
    }
    Ok(())
}

// fn cal_depth(db_basic_data: &DbBasicData, refno: RefU64) -> usize{
//     let mut depth = 0;
//     if db_basic_data.children_map.contains_key() {
//
//     }
//     0
// }

pub async fn save_pe_relates(db_basic: &DbBasicData, output: flume::Sender<String>) {
    //todo 增加删除已有owner的逻辑
    let mut all_relate_sqls = vec![];
    for kv in &db_basic.children_map {
        let owner = kv.0;
        let children = kv.1;
        if children.is_empty() {
            continue;
        }
        let relate_sqls = children
            .iter()
            .enumerate()
            .map(|(i, (child, c_noun))| {
                let cp = child.to_pe_key();
                let op = owner.to_pe_key();
                format!("RELATE {0}->pe_owner:[{1}, {i}]->{1};", cp, op,)
            })
            .collect::<Vec<String>>();
        all_relate_sqls.extend_from_slice(&relate_sqls);
        if all_relate_sqls.len() >= 2000 {
            let sql = all_relate_sqls.join("");
            all_relate_sqls.clear();
            // dbg!(sql.len());
            output.send(sql).expect("send pe_relates error");
            // break;
        }
    }
    if !all_relate_sqls.is_empty() {
        let sql = all_relate_sqls.join("");
        output.send(sql).expect("send pe_relates error");
    }
}
