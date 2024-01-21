use crate::arangodb::{ArDatabase, ArPool};
use crate::consts::AQL_PDMS_EDGES_COLLECTION;
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_hash;
use bb8_arangodb::arangors_lite::collection::CollectionType;
use bb8_arangodb::arangors_lite::{AqlQuery, ClientError};
use bb8_arangodb::bb8::Pool;
use bb8_arangodb::{ArangoConnectionManager, AuthenticationMethod};
use dashmap::{DashMap, DashSet};
use itertools::Itertools;
use serde_json::value::Value;
use sqlx::Row;
use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::take;
use std::sync::Arc;

///创建arangodb的连接池
pub async fn connect_arangodb(db_option: &DbOption) -> anyhow::Result<ArPool> {
    let manager = ArangoConnectionManager::new(
        db_option.arangodb_url.to_string(),
        AuthenticationMethod::JWTAuth(
            db_option.arangodb_user.to_string(),
            db_option.arangodb_password.to_string(),
        ),
    );
    Ok(Pool::builder().max_size(100).build(manager).await?)
}

///创建arango的document
pub async fn create_arango_document(
    database: &ArDatabase,
    collection_name: &str,
    collection_type: CollectionType,
) -> anyhow::Result<()> {
    match collection_type {
        CollectionType::Document => {
            let database = database.create_collection(collection_name).await;
            match database {
                Ok(_v) => {}
                Err(e) => match &e {
                    ClientError::Arango(error) => {
                        if error.code() != 409 {
                            dbg!(&e);
                        }
                    }
                    _ => {
                        dbg!(&e);
                    }
                },
            }
        }
        CollectionType::Edge => {
            let database = database.create_edge_collection(collection_name).await;
            match database {
                Ok(_v) => {}
                Err(e) => match &e {
                    ClientError::Arango(error) => {
                        if error.code() != 409 {
                            dbg!(&e);
                        }
                    }
                    _ => {
                        dbg!(&e);
                    }
                },
            }
        }
    }
    Ok(())
}

/// 在同步的时候就将 pdms_element 保存到图数据库
pub async fn save_pdms_element_to_arango(
    database: &ArDatabase,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    children_map: &HashMap<RefU64, Vec<(RefU64, String)>>,
    dbnum: i32,
) -> anyhow::Result<()> {
    Ok(())
}

/// 保存虚拟孔洞数据到图数据库
pub async fn save_virtual_hole_value_to_arangodb(db_option: &DbOption) -> anyhow::Result<()> {
    //获取虚拟孔洞信息
    // let hole_data = insert_virtual_hole_data();
    // for data in hole_data.chunks(ARANGODB_SAVE_AMOUNT) {
    //     let json = serde_json::to_value(data)?;
    //     save_arangodb_with_db_option(database, json,  "hole_data").await?;
    // }
    //
    // let embed_data = insert_virtual_embed_data();
    // for data in embed_data.chunks(ARANGODB_SAVE_AMOUNT) {
    //     let json = serde_json::to_value(data)?;
    //     save_arangodb_with_db_option(database, json,  "embed_data").await?;
    // }

    Ok(())
}

///保存层级关系到图数据库
pub async fn save_pdms_level_edges_in_sync(
    database: &ArDatabase,
    children_map: &HashMap<RefU64, Vec<(RefU64, String)>>,
) -> anyhow::Result<()> {
    Ok(())
}

/// 将外部引用的参考号保存到图数据库中
pub async fn save_foreign_refno_edges_in_sync(
    database: &ArDatabase,
    foreign_refnos_map: DashMap<RefU64, DashMap<String, RefU64>>,
) -> anyhow::Result<()> {
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
            let sites =
                query_world_children_eles(&mgr.db_option.mdb_name, module, project_db.value())
                    .await?;
            // 从site开始将所有 query_children的参考号放入队列中
            for site in &sites {
                pending.push_back((site.refno, site.noun.clone()));
            }
            set_level_edges(sites, &mut sibl_edges).await?;
            // 遍历整个pdms树
            while pending.len() != 0 {
                let (pending_refno, pending_noun) = pending.pop_front().unwrap();
                if let Ok(children) = query_children_eles(pending_refno, project_db.value()).await {
                    if children.len() != 0 {
                        for child in &children {
                            pending.push_back((child.refno, child.noun.clone()));
                        }
                        // 管道先按兄弟关系保存
                        if pending_noun == "BRAN" {
                            set_level_edges(children.clone(), &mut tubi_edges).await?;
                        }
                        set_level_edges(children, &mut sibl_edges).await?;
                    }
                }
                if sibl_edges.len() > 1000 {
                    let database = mgr.get_arango_db().await?;
                    let json = serde_json::to_value(&take(&mut sibl_edges))?;
                    save_arangodb_doc(json, sibl_collection, &database, false).await?;
                    if tubi_edges.len() != 0 {
                        let tubi_json = serde_json::to_value(&take(&mut tubi_edges))?;
                        save_arangodb_doc(tubi_json, tubi_collection, &database, false).await?;
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

pub async fn sync_foreign_refno_to_graph_db(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
    Ok(())
}

/// 将dtse下的data中的dkey和ppro保存到图数据库中
pub async fn save_dtse_value_to_arangodb(
    database: &ArDatabase,
    type_ele_map: &DashMap<u32, HashSet<RefU64>>,
    total_attr_map: &DashMap<RefU64, WholeAttMap>,
) -> anyhow::Result<()> {
    if let Some(data_refnos) = type_ele_map.get(&db1_hash("DATA")) {
        let mut result = vec![];
        for data_refno in data_refnos.value() {
            let whole_attr = total_attr_map.get(data_refno);
            if whole_attr.is_none() {
                continue;
            }
            let implicit_attr = &whole_attr.unwrap().implicit_attmap;
            let d_key = implicit_attr.get_str("DKEY");
            let ppro = implicit_attr.get_str("PPRO");
            let dpro = implicit_attr.get_str("DPRO");
            if d_key.is_none() || ppro.is_none() {
                continue;
            }
            result.push(DataDocument {
                _key: data_refno.to_string(),
                dkey: d_key.unwrap().to_string(),
                ppro: ppro.unwrap().to_string(),
                dpro: dpro.unwrap().to_string(),
            })
        }
        let json = serde_json::to_value(&result)?;
        save_arangodb_with_db_option(database, json, "data_eles").await?;
    }
    Ok(())
}

pub async fn save_arangodb(
    mgr: Arc<AiosDBManager>,
    json: Value,
    collection: &str,
) -> anyhow::Result<()> {
    let database = mgr.get_arango_db().await?;
    let aql = AqlQuery::new(r#"
        with @@collection
        LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true, overwriteMode: "replace" }"#)
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_with_db_option(
    database: &ArDatabase,
    json: Value,
    collection: &str,
) -> anyhow::Result<()> {
    let mut aql_string = r#"
                with @@collection
                LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true, overwriteMode: "replace" }"#.to_string();
    let aql = AqlQuery::new(&aql_string)
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

///删除edge数据库的数据
pub async fn remove_edges_arangodb(
    database: &ArDatabase,
    keys: &[String],
    collection: &str,
) -> anyhow::Result<()> {
    let mut aql_string = r#"
      with @@collection
      FOR k IN @keys
        REMOVE { _key: k } IN @@collection
  "#
    .to_string();
    let aql = AqlQuery::new(&aql_string)
        .bind_var("@collection", collection)
        .bind_var("keys", keys);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_doc(
    json: Value,
    collection: &str,
    database: &ArDatabase,
    replace: bool,
) -> anyhow::Result<()> {
    let mut aql_str = if replace {
        r#"
        with @@collection
        LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true, overwriteMode: "replace" }"#
    } else {
        r#"
        with @@collection
        LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true}"#
    };
    let aql = AqlQuery::new(aql_str)
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn update_arangodb_doc(
    key: &str,
    value: Value,
    collection: &str,
    database: &ArDatabase,
) -> anyhow::Result<()> {
    let aql_str = AqlQuery::new(
        "
        With @@collection
        let doc = document(@@collection,@key)
        update doc with @value in @@collection",
    )
    .bind_var("@collection", collection)
    .bind_var("key", key)
    .bind_var("value", value);
    let _result: Vec<()> = database.aql_query(aql_str).await?;
    Ok(())
}

pub async fn remove_arangodb_with_refno_key(
    refnos: &Vec<RefU64>,
    collection: &str,
    database: &ArDatabase,
) -> anyhow::Result<bool> {
    let keys = refnos
        .into_iter()
        .map(|refno| refno.to_string())
        .collect::<Vec<_>>();
    dbg!(&keys);
    let aql = AqlQuery::new(
        "
            with @@COLLECTION
            FOR D IN @keys
                    REMOVE D IN @@COLLECTION",
    )
    .bind_var("@COLLECTION", collection)
    .bind_var("keys", keys);
    let result = database.aql_query::<Vec<()>>(aql).await;
    Ok(!result.is_err())
}

pub async fn save_arangodb_with_db_option_create_collection(
    database: &ArDatabase,
    json: Value,
    collection: &str,
    collection_type: CollectionType,
) -> anyhow::Result<()> {
    match collection_type {
        CollectionType::Document => {
            database.create_collection(collection).await?;
        }
        CollectionType::Edge => {
            database.create_edge_collection(collection).await?;
        }
    }
    let mut aql_string = r#"
    with @@collection
    LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true, overwriteMode: "replace" }"#.to_string();
    // if db_option.replace_dbs {
    //     aql_string = aql_string.replace("INSERT", "REPLACE");
    // }
    let aql = AqlQuery::new(&aql_string)
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn query_arangodb_with_refno_key(
    key: String,
    collection: &str,
    database: &ArDatabase,
) -> anyhow::Result<bool> {
    dbg!(&key);
    let aql = AqlQuery::new(" with @@COLLECTION return document(@@COLLECTION,@_key)._key")
        .bind_var("@COLLECTION", collection)
        .bind_var("_key", key);
    if let Ok(result) = database.aql_query::<String>(aql).await {
        return Ok(true);
    } else {
        return Ok(false);
    }
}
