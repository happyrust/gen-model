use std::io::Write;
use sqlx::Row;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use crate::consts::*;
use arangors_lite::{AqlQuery, ClientError, Connection, Database};
use std::collections::{HashMap, VecDeque};
use std::mem::take;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use aios_core::pdms_types::{PdmsElement, RefU64};
use anyhow::anyhow;
use arangors_lite::collection::CollectionType;
use dashmap::DashSet;
use futures::future::ok;
use crate::api::attr::{query_foreign_refnos_from_table, query_implicit_attr};
use crate::api::children::query_contain_noun_refnos;
use crate::api::element::{query_children, query_children_eles, query_mdb_dbnos, query_types_refnos, query_world, query_world_children_eles};
use crate::api::project_mdb::query_mdb_contain_numbdb;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::{ForeignEdges};
use crate::graph_db::structs::{PdmsEleGraphEdge, PdmsEleGraphNode};
use crate::helper::qualified_table_name;
use crate::options::DbOption;

pub async fn sync_pdms_to_graph_db(mgr: Arc<AiosDBManager>, db_option: DbOption) -> anyhow::Result<()> {
    let mut time = Instant::now();
    for project in &db_option.included_projects {
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        let pool = AiosDBManager::get_db_pool(&default_conn, project).await.unwrap();
        let include_module = vec!["CATA"];
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
                            let edge = PdmsEleGraphEdge {
                                _from: format!("{}/{refno_str}", &collection),
                                _to: format!("{}/{owner_str}", &collection),
                            };
                            eles.push(element);
                            edges.push(edge);
                        }
                        let database_clone = mgr.get_arangodb_conn().await?;
                        // let handle = tokio::spawn(async move {
                        let json = serde_json::to_value(&take(&mut eles))?;
                        let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection")
                            .bind_var("@collection", collection)
                            .bind_var("elements", json);
                        let _result: Vec<()> = database_clone.aql_query(aql).await?;

                        let json = serde_json::to_value(&take(&mut edges))?;
                        let aql = AqlQuery::new("LET data = @edges
                    FOR d IN data
                        INSERT d INTO @@collection")
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
                    let json = serde_json::to_value(&take(&mut sibl_edges))?;
                    save_arangodb(json, mgr.clone(), sibl_collection).await?;
                    if tubi_edges.len() != 0 {
                        let tubi_json = serde_json::to_value(&take(&mut tubi_edges))?;
                        save_arangodb(tubi_json, mgr.clone(), tubi_collection).await?;
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
        let edge = PdmsEleGraphEdge {
            _from: format!("{}/{}", "pdms_eles", eles[i].refno.replace("/", "_")),
            _to: format!("{}/{}", "pdms_eles", eles[i - 1].refno.replace("/", "_")),
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
                        _from: format!("{}/{}", collection, spre.to_url_refno()),
                        _to: format!("{}/{}", collection, catr.to_url_refno()),
                        foreign_type: "CATR".to_string(),
                    }
                );
                spre_set.insert(spre);
                // 获得 catr 的 ptre gmre dtre
                if catr_set.contains(&catr) { continue; }
                if let Some(refno_basic) = mgr.get_refno_basic(catr) {
                    if let Some(project_db) = mgr.get_project_db(catr) {
                        let att = query_implicit_attr(catr, refno_basic.value(), &project_db, Some(catr_foreign_refs.clone())).await?;
                        for catr_foreign_type in &catr_foreign_refs {
                            if let Some(ptre) = att.get_val(catr_foreign_type) {
                                let ptre_refno = ptre.refno_value().unwrap_or(RefU64(0));
                                if *ptre_refno == 0 { continue; }
                                spre_edges.push(ForeignEdges {
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

pub async fn save_dtse_value_to_arangodb(db_option:&DbOption)


pub async fn save_arangodb(json: Value, mgr: Arc<AiosDBManager>, collection: &str) -> anyhow::Result<()> {
    let database = mgr.get_arangodb_conn().await?;
    let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection")
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_with_db_option(json: Value, db_option: &DbOption, collection: &str) -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(&db_option.arangodb_url, "root", "")
        .await?;
    let database = conn.db("pdms").await?;
    let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }" )
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_with_db_option_create_collection(json: Value, db_option: &DbOption, collection: &str, collection_type:CollectionType ) -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(&db_option.arangodb_url, "root", "")
        .await?;
    let database = conn.db("pdms").await?;
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
                        INSERT d INTO @@collection")
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}