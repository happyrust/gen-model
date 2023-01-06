use std::io::Write;
use sqlx::Row;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use crate::consts::*;
use arangors_lite::{AqlQuery, ClientError, Collection, Connection, Database};
use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::take;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use aios_core::pdms_types::{PdmsElement, RefU64, RefU64Vec};
use aios_core::tool::db_tool::db1_hash;
use anyhow::anyhow;
use arangors_lite::collection::CollectionType;
use dashmap::{DashMap, DashSet};
use futures::future::ok;
use itertools::Itertools;
use parse_pdms_db::parse::WholeAttMap;
use crate::api::attr::{query_foreign_refnos_from_table, query_implicit_attr};
use crate::api::children::query_contain_noun_refnos;
use crate::api::element::*;
use crate::api::project_mdb::query_mdb_contain_numbdb;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::{DataDocument, ForeignEdges};
use crate::graph_db::structs::{PdmsEleGraphEdge, PdmsEleGraphEdgeWithKey, PdmsEleGraphNode};
use crate::helper::qualified_table_name;
use crate::options::DbOption;

/// 根据 db_option 的 project_name 创建 arangodb 的 database
pub async fn set_arangodb_database_from_db_option(db_option: &DbOption) -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(&db_option.arangodb_url, &db_option.arangodb_user, &db_option.arangodb_password)
        .await?;
    let _ = conn.create_database(&db_option.arangodb_database).await;
    Ok(())
}

pub async fn get_arangodb_conn_from_db_option(db_option: &DbOption) -> anyhow::Result<Database> {
    let conn = Connection::establish_jwt(&db_option.arangodb_url, &db_option.arangodb_user, &db_option.arangodb_password)
        .await?;
    Ok(conn.db(&db_option.arangodb_database).await?)
}

pub async fn create_arangodb_conn(database: &Database, collection_name: &str, collection_type: CollectionType) -> anyhow::Result<()> {
    match collection_type {
        CollectionType::Document => {
            let database = database.create_collection(collection_name).await;
            match database {
                Ok(_v) => {}
                Err(e) => { dbg!(&e); }
            }
        }
        CollectionType::Edge => {
            let database = database.create_edge_collection(collection_name).await;
            match database {
                Ok(_v) => {}
                Err(e) => { dbg!(&e); }
            }
        }
    }
    Ok(())
}

/// 在同步的时候就将 pdms_element 保存到图数据库
pub async fn save_pdms_element_in_sync(db_option: &DbOption, total_attr_map: &DashMap<RefU64, WholeAttMap>
                                       , children_map: &HashMap<RefU64, RefU64Vec>, dbnum: i32) -> anyhow::Result<()> {
    let mut results = Vec::new();
    let mut edges = Vec::new();
    for (refno, whole_attr) in total_attr_map.clone() {
        let owner = whole_attr.implicit_attmap.get_owner();
        if owner.is_none() { continue; }
        let owner = owner.unwrap();
        let owner_str = owner.to_url_refno();
        let name = get_name(total_attr_map, &children_map, refno);
        let noun = whole_attr.implicit_attmap.get_type();
        let pdms_element = PdmsEleGraphNode {
            _key: refno.to_url_refno(),
            owner: owner_str.clone(),
            name,
            noun: noun.to_string(),
            version: 0,
            dbnum,
        };
        let key = refno.hash_with_another_refno(owner);
        let pdms_edges = PdmsEleGraphEdgeWithKey {
            _key: key.to_string(),
            _from: format!("{}/{}", "pdms_eles", refno.to_url_refno()),
            _to: format!("{}/{}", "pdms_eles", owner_str),
        };
        results.push(pdms_element);
        edges.push(pdms_edges);
    }
    for result in results.chunks(ARANGODB_SAVE_AMOUNT) {
        let json = serde_json::to_value(result)?;
        save_arangodb_with_db_option(json, db_option, "pdms_eles").await?;
    }
    dbg!(&edges.len());
    for edge in edges.chunks(ARANGODB_SAVE_AMOUNT) {
        let json = serde_json::to_value(edge)?;
        save_arangodb_with_db_option(json, db_option, "pdms_edges").await?;
    }
    Ok(())
}

pub async fn sync_pdms_to_graph_db(mgr: Arc<AiosDBManager>, db_option: DbOption) -> anyhow::Result<()> {
    let mut time = Instant::now();
    for project in &db_option.included_projects {
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        let pool = AiosDBManager::get_db_pool(&default_conn, project).await.unwrap();
        let include_module = vec!["DESI", "CATA"];
        for module in include_module {
            // let mut handles = vec![];
            // 只保存 指定mdb的desi的numbdb
            let numbdbs = query_mdb_contain_numbdb(&format!("/{}", db_option.mdb_name), module, &pool).await?;
            let mut numbdbs_sql = String::new();
            for numbdb in numbdbs {
                numbdbs_sql.push_str(&format!("{} ,", numbdb));
            }
            numbdbs_sql.remove(numbdbs_sql.len() - 1);

            let sql = format!("SELECT ID, OWNER, TYPE, NAME, NUMBDB  FROM {PDMS_ELEMENTS_TABLE} WHERE NUMBDB IN ({})", numbdbs_sql);
            let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
            let collection = "pdms_eles";
            let pdms_edge_collection = "pdms_edges";
            match results {
                Ok(vals) => {
                    //需不需要按照db numbder 来分别去生成
                    for val_chunk in vals.chunks(1000) {
                        let mut eles = vec![];
                        let mut edges = vec![];
                        for val in val_chunk {
                            let refno = (val.get::<i64, _>("ID") as u64).into();
                            let owner = (val.get::<i64, _>("OWNER") as u64).into();
                            let name = val.get::<String, _>("NAME");
                            let type_name = val.get::<String, _>("TYPE");
                            let dbnum = val.get::<i32, _>("NUMBDB");
                            let refno_str = RefU64(refno).to_refno_normal_string();
                            let owner_str = RefU64(owner).to_refno_normal_string();
                            let element = PdmsEleGraphNode {
                                _key: refno_str.clone(),
                                owner: owner_str.clone(),
                                name,
                                noun: type_name,
                                version: 0,
                                dbnum,
                            };
                            let key = RefU64(refno).hash_with_another_refno(RefU64(owner));
                            let edge = PdmsEleGraphEdgeWithKey {
                                _key: key.to_string(),
                                _from: format!("{}/{refno_str}", &collection),
                                _to: format!("{}/{owner_str}", &collection),
                            };
                            eles.push(element);
                            edges.push(edge);
                        }
                        let database_clone = mgr.get_arangodb_conn().await?;
                        // let handle = tokio::spawn(async move {
                        let json = serde_json::to_value(&take(&mut eles))?;
                        //     let aql = AqlQuery::new("LET data = @elements
                        // FOR d IN data
                        //     INSERT d INTO @@collection OPTIONS { ignoreErrors: true } ")
                        //         .bind_var("@collection", collection)
                        //         .bind_var("elements", json);
                        //     let _result: Vec<()> = database_clone.aql_query(aql).await?;

                        let json = serde_json::to_value(&take(&mut edges))?;
                        let aql = AqlQuery::new("LET data = @edges
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }")
                            .bind_var("@collection", pdms_edge_collection)
                            .bind_var("edges", json);
                        let _result: Vec<()> = database_clone.aql_query(aql).await?;
                        // });
                        // handles.push(handle);
                    }
                    // futures::future::join_all(take(&mut handles)).await;
                }
                Err(e) => {
                    dbg!(&e);
                    dbg!(sql);
                    return Err(anyhow!(e.to_string()));
                }
            }
        }
    }
    println!("sync graph db costs: {}ms", time.elapsed().as_millis());
    Ok(())
}

pub async fn save_pdms_level_edges_in_sync(db_option: &DbOption, children_map: &HashMap<RefU64, RefU64Vec>) -> anyhow::Result<()> {
    let mut results = vec![];
    for (_refno, children_map) in children_map {
        if children_map.len() == 0 { continue; }
        for i in 1..children_map.len() {
            let from_refno = children_map[i];
            let to_refno = children_map[i - 1];
            let edge = PdmsEleGraphEdgeWithKey {
                _key: from_refno.hash_with_another_refno(to_refno).to_string(),
                _from: format!("{}/{}", "pdms_eles", from_refno.to_url_refno()),
                _to: format!("{}/{}", "pdms_eles", to_refno.to_url_refno()),
            };
            results.push(edge);
        }
    }
    if !results.is_empty() {
        for result in results.chunks(ARANGODB_SAVE_AMOUNT) {
            let json = serde_json::to_value(result)?;
            save_arangodb_with_db_option(json, db_option, "sibl_edges").await?;
        }
    }
    Ok(())
}

/// 将外部引用的参考号保存到图数据库中
pub async fn save_foreign_refno_edges_in_sync(db_option: &DbOption, foreign_refnos_map: DashMap<RefU64, DashMap<String, RefU64>>) -> anyhow::Result<()> {
    let mut foreign_edges = vec![];
    let mut foreign_edges_refnos = DashSet::new(); // 防止edges重复
    for foreign_refnos in foreign_refnos_map.into_iter() {
        let refno = foreign_refnos.0;
        if foreign_edges_refnos.contains(&refno) { continue; }
        foreign_edges_refnos.insert(refno);
        for (foreign_type, foreign_refno) in foreign_refnos.1 {
            if foreign_refno == RefU64(0) { continue; }
            let key = refno.hash_with_another_refno(foreign_refno);
            foreign_edges.push(ForeignEdges {
                _key: key.to_string(),
                _from: format!("{}/{}", "pdms_eles", refno.to_url_refno()),
                _to: format!("{}/{}", "pdms_eles", foreign_refno.to_url_refno()),
                foreign_type,
            })
        }
    }
    if foreign_edges.len() > 0 {
        for foreign_edge in foreign_edges.chunks(ARANGODB_SAVE_AMOUNT) {
            let json = serde_json::to_value(foreign_edge)?;
            save_arangodb_with_db_option(json, &db_option, "foreign_edges").await?;
        }
    }
    Ok(())
}

/// 将 bran下的元件连接关系保存到 tube_edges 中
pub async fn sync_pdms_level_edges_to_graph_db(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
    let mut sibl_edges = vec![];
    let mut tubi_edges = vec![];
    let sibl_collection = "sibl_edges";
    let tubi_collection = "tubi_edges";
    let project = &mgr.db_option.project_name;
    if let Some(project_db) = mgr.project_map.get(project) {
        let include_module = vec!["DESI", "CATA"];
        for module in include_module {
            let mut pending = VecDeque::new();
            // world 层级就不管了 直接从site层级开始
            let sites = query_world_children_eles(&mgr.db_option.mdb_name, module, project_db.value()).await?;
            // 从site开始将所有 query_children的参考号放入队列中
            for site in &sites {
                pending.push_back((RefU64::from_refno_str(&site.refno)?, site.noun.clone()));
            }
            set_level_edges(sites, &mut sibl_edges).await?;
            // 遍历整个pdms树
            while pending.len() != 0 {
                let (pending_refno, pending_noun) = pending.pop_front().unwrap();
                if let Ok(children) = query_children_eles(pending_refno, project_db.value()).await {
                    if children.len() != 0 {
                        for child in &children {
                            pending.push_back((
                                RefU64::from_refno_str(&child.refno).unwrap(), child.noun.clone()
                            ));
                        }
                        // 管道先按兄弟关系保存
                        if pending_noun == "BRAN" {
                            set_level_edges(children.clone(), &mut tubi_edges).await?;
                        }
                        set_level_edges(children, &mut sibl_edges).await?;
                    }
                }
                if sibl_edges.len() > 1000 {
                    let database = mgr.get_arangodb_conn().await?;
                    let json = serde_json::to_value(&take(&mut sibl_edges))?;
                    save_arangodb_with_database(json, sibl_collection, &database).await?;
                    if tubi_edges.len() != 0 {
                        let tubi_json = serde_json::to_value(&take(&mut tubi_edges))?;
                        save_arangodb_with_database(tubi_json, tubi_collection, &database).await?;
                    }
                }
            }
            // });
            // handles.push(handle);
        }
        // futures::future::join_all(take(&mut handles)).await;
    }
    Ok(())
}

/// 将同级 children 赋上连接关系
async fn set_level_edges(eles: Vec<PdmsElement>, mut edges: &mut Vec<PdmsEleGraphEdge>) -> anyhow::Result<()> {
    for i in 1..eles.len() {
        let from_refno = RefU64::from_refno_str(&eles[i].refno);
        let to_refno = RefU64::from_refno_str(&eles[i - 1].refno);
        if from_refno.is_err() || to_refno.is_err() { continue; }
        let from_refno = from_refno.unwrap();
        let to_refno = to_refno.unwrap();
        let edge = PdmsEleGraphEdge {
            _key: from_refno.hash_with_another_refno(to_refno).to_string(),
            _from: format!("{}/{}", "pdms_eles", from_refno.to_url_refno()),
            _to: format!("{}/{}", "pdms_eles", to_refno.to_url_refno()),
        };
        edges.push(edge);
    }
    Ok(())
}

/// 将pdms spre catr 等外键连接关系保存到图数据库 edges
pub async fn sync_foreign_refno_to_graph_db(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
    let mut spre_set = DashSet::new();
    let mut catr_set: DashSet<RefU64> = DashSet::new();
    let mut spre_edges = vec![];
    let mut spre_foreign_refs = vec!["SPRE", "CATR"];
    let catr_foreign_refs = vec!["PTRE", "GMRE", "DTRE"];
    let collection = "pdms_eles";
    let edges_collection = "foreign_edges";
    for project in &mgr.projects {
        if let Some(project_db) = mgr.project_map.get(project) {
            // 找到所有带有spre或catr属性的元件
            // for foreign in &spre_foreign_refs {
            //     let tables = query_contain_noun_refnos(foreign.to_string(), project_db.value()).await?;
            //     for table_name in tables {
            //         if table_name.to_lowercase() == "spco" || table_name.to_lowercase() == "scom" { continue; } // 排除 spco 中的 catr 引用
            //         let refnos = query_foreign_refnos_from_table(foreign, table_name.as_str(), project_db.value()).await?;
            //         for (refno, foreign_refno) in refnos {
            //             spre_edges.push(
            //                 ForeignEdges {
            //                     _from: format!("{}/{}", collection, refno.to_url_refno()),
            //                     _to: format!("{}/{}", collection, foreign_refno.to_url_refno()),
            //                     foreign_type: foreign.to_string(),
            //                 }
            //             );
            //         }
            //         if spre_edges.len() > 1000 {
            //             let json = serde_json::to_value(&take(&mut spre_edges))?;
            //             save_arangodb(json, mgr.clone(), edges_collection).await?;
            //         }
            //     }
            // }
            // 找到所有的 spco  自身 refno就是 spre ，另一个返回值就是 catr
            let results = query_foreign_refnos_from_table("CATR", "SPCO", project_db.value()).await?;
            for (spre, catr) in results {
                if *catr == 0 { continue; }
                if spre_set.contains(&spre) { continue; }
                // spre 到 catr 的边
                spre_edges.push(
                    ForeignEdges {
                        _key: spre.hash_with_another_refno(catr).to_string(),
                        _from: format!("{}/{}", collection, spre.to_url_refno()),
                        _to: format!("{}/{}", collection, catr.to_url_refno()),
                        foreign_type: "CATR".to_string(),
                    }
                );
                spre_set.insert(spre);
                // 获得 catr 的 ptre gmre dtre
                if catr_set.contains(&catr) { continue; }
                if let Some(refno_basic) = mgr.get_refno_basic(catr) {
                    if let Some((_, project_db)) = mgr.get_project_pool_by_refno(catr).await {
                        let att = query_implicit_attr(catr, refno_basic.value(), &project_db, Some(catr_foreign_refs.clone())).await?;
                        for catr_foreign_type in &catr_foreign_refs {
                            if let Some(ptre) = att.get_val(catr_foreign_type) {
                                let ptre_refno = ptre.refno_value().unwrap_or(RefU64(0));
                                if *ptre_refno == 0 { continue; }
                                spre_edges.push(ForeignEdges {
                                    _key: catr.hash_with_another_refno(ptre_refno).to_string(),
                                    _from: format!("{}/{}", collection, catr.to_url_refno()),
                                    _to: format!("{}/{}", collection, ptre_refno.to_url_refno()),
                                    foreign_type: catr_foreign_type.to_string(),
                                });
                            }
                        }
                        catr_set.insert(catr);
                    }
                }
                // 分量保存
                if spre_edges.len() > 1000 {
                    let json = serde_json::to_value(&take(&mut spre_edges))?;
                    save_arangodb(json, mgr.clone(), edges_collection).await?;
                }
            }
        }
    }
    let json = serde_json::to_value(&take(&mut spre_edges))?;
    save_arangodb(json, mgr.clone(), edges_collection).await?;
    Ok(())
}

/// 将dtse下的data中的dkey和ppro保存到图数据库中
pub async fn save_dtse_value_to_arangodb(db_option: &DbOption, type_ele_map: &DashMap<u32,
    HashSet<RefU64>>, total_attr_map: &DashMap<RefU64, WholeAttMap>) -> anyhow::Result<()> {
    if let Some(data_refnos) = type_ele_map.get(&db1_hash("DATA")) {
        let mut result = vec![];
        for data_refno in data_refnos.value() {
            let whole_attr = total_attr_map.get(data_refno);
            if whole_attr.is_none() { continue; }
            let implicit_attr = &whole_attr.unwrap().implicit_attmap;
            let d_key = implicit_attr.get_str("DKEY");
            let ppro = implicit_attr.get_str("PPRO");
            let dpro = implicit_attr.get_str("DPRO");
            if d_key.is_none() || ppro.is_none() { continue; }
            result.push(DataDocument {
                _key: data_refno.to_url_refno(),
                dkey: d_key.unwrap().to_string(),
                ppro: ppro.unwrap().to_string(),
                dpro: dpro.unwrap().to_string(),
            })
        }
        let json = serde_json::to_value(&result)?;
        save_arangodb_with_db_option(json, db_option, "data_eles").await?;
    }
    Ok(())
}


pub async fn save_arangodb(json: Value, mgr: Arc<AiosDBManager>, collection: &str) -> anyhow::Result<()> {
    let database = mgr.get_arangodb_conn().await?;
    let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }")
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_with_db_option(json: Value, db_option: &DbOption, collection: &str) -> anyhow::Result<()> {
    let database = get_arangodb_conn_from_db_option(db_option).await?;
    let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }")
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_with_database(json: Value, collection: &str, database: &Database) -> anyhow::Result<()> {
    let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }")
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_with_db_option_create_collection(json: Value, db_option: &DbOption, collection: &str, collection_type: CollectionType) -> anyhow::Result<()> {
    let database = get_arangodb_conn_from_db_option(db_option).await?;
    match collection_type {
        CollectionType::Document => {
            database.create_collection(collection).await?;
        }
        CollectionType::Edge => {
            database.create_edge_collection(collection).await?;
        }
    }
    let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }")
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}