use crate::graph_db::structs::PdmsEleDataVersioned;
use crate::surreal_service::SUL_DB;
use aios_core::options::DbOption;
use aios_core::orm::pdms_element;
use aios_core::orm::pdms_element::Model;
use aios_core::pdms_types::*;
use dashmap::DashMap;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;
use sea_orm::entity::prelude::*;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use surrealdb::dbs::Response;
use surrealdb::sql::Thing;
use termnius_client::client::TDBClient;

pub async fn get_versioned_client(project: &str) -> TDBClient {
    let mut client = termnius_client::client::TDBClientBuilder::default()
        // .server_url("http://192.168.31.179:6363".to_string())
        .server_url("http://localhost:6363".to_string())
        .auth_info(termnius_client::client::AuthInfo::new())
        .session_info(termnius_client::client::SessionInfo::new())
        .repo_info(termnius_client::client::RepoInfo {
            team: "admin".to_string(),
            db: project.to_string(),
            branch: "main".to_string(),
            ref_val: None,
            repo: "local".to_string(),
            db_info: Default::default(),
            author: "".to_string(),
        })
        .build()
        .unwrap();

    client
}

const SQL_CHUNK_COUNT: usize = 1000;
// const SQL_CHUNK_COUNT: usize = 1;
const JSON_CHUNK_COUNT: usize = 10_000;

pub async fn save_versioned_pdms_eles(
    client: &TDBClient,
    total_attr_map: &DashMap<RefU64, WholeAttMap>,
    db_num: i32,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    let mut eles = Vec::with_capacity(total_attr_map.len());
    for kv in total_attr_map.iter() {
        let att_map: NamedAttrMap = kv.value().merge().into();
        let ele = PdmsEleDataVersioned {
            id: format!("PdmsElement/{}", kv.key().to_string()),
            refno: *kv.key(),
            owner: att_map.get_refno_by_att_or_default("OWNER"),
            name: att_map.get_string_or_default("NAME"),
            noun: att_map.get_type(),
            dbnum: db_num,
            cata_hash: None,
            status_tag: None,
        };
        eles.push(ele);
    }

    // let mut futures = FuturesUnordered::new();
    for result in eles.chunks(JSON_CHUNK_COUNT) {
        let json = serde_json::to_string(result)?;

        let doc_res = client
            .insert_doc(
                json.as_str(),
                "dpc",
                "Add Pdms Elements.",
                false,
                false,
                true,
            )
            .await
            .unwrap_or_default();
        dbg!(doc_res);

        // let project = db_option.project_name.clone();
        // futures.push(tokio::task::spawn(async move {
        //     // let mut conn = pool.get_conn().await.unwrap();
        //     let mut client = get_versioned_client(&project).await;
        //     // let info = client.db_info().await;
        //     // dbg!(info);
        //     let doc_res = client.insert_doc(json.as_str(), "dpc", "Add Pdms Elements.", false, false, true).await.unwrap_or_default();
        //     dbg!(doc_res);
        // }));
    }

    // while let Some(_) = futures.next().await { }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct Record {
    #[allow(dead_code)]
    id: Thing,
}

/// 保存element数据到版本管理
/// todo 后续再考虑 record links
// 先暂时使用relate的方式
pub async fn save_pdms_eles_to_versioned(
    db_option: &DbOption,
    project: &str,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    db_num: i32,
    children_map: &HashMap<RefU64, Vec<(RefU64, String)>>,
) -> anyhow::Result<()> {
    let mut model_chunks: Vec<Vec<Model>> = vec![];
    //是否需要定义SCHEMA
    // SUL_DB
    //     .query(format!(r#"
    //         DEFINE TABLE {0} SCHEMALESS;
    //         DEFINE FIELD owner ON {0} TYPE option<record<{0}>>;
    //     "#, &table_name))
    //     .await.unwrap();
    for chunk in &total_attr_map.into_iter().chunks(SQL_CHUNK_COUNT) {
        let mut model_chunk = vec![];
        for kv in chunk {
            let att_map = kv.value();
            let owner = att_map.get_refno_by_att_or_default("OWNER");
            let refno = *kv.key();
            let ele = pdms_element::Model {
                id: refno.to_string(),
                refno,
                owner,
                name: att_map.get_string_or_default("NAME"),
                noun: att_map.get_type(),
                dbnum: db_num,
                cata_hash: None,
                status_tag: None,
                version_tag: None,
                e3d_version: att_map.get_e3d_version(),
                lock: false,
            };
            model_chunk.push(ele);
        }
        model_chunks.push(model_chunk);
    }
    // let mut futures = FuturesUnordered::new();
    for models in model_chunks {
        //save to sql, todo 保存到tidb
        if false {
            // let db = sea_orm::Database::connect(&db_option.get_mysql_db_conn_str(project))
            //     .await
            //     .unwrap();
            // futures.push(tokio::task::spawn(async move {
            //   let test_models : Vec<Box<dyn ActiveModelTrait>> = vec![];
            // let _ = aios_core::orm::PdmsElement::insert_many(models).exec(&db).await;
            // }));
            // break;
        }
        SUL_DB
            .query("INSERT IGNORE INTO pe $values")
            .bind(("values", &models))
            .await
            .unwrap();
    }

    // 使用owner创建relate关系
    for kv in children_map {
        let owner = kv.0;
        let children = kv.1;
        if children.is_empty() {
            continue;
        }
        let relate_sqls = children
            .iter()
            .enumerate()
            .map(|(i, (child, _))| {
                format!("RELATE pe:{}->pe_owner->pe:{} set order_num = {}",
                    child.to_string(),
                    owner.to_string(),
                    i
                )
            })
            .collect::<Vec<String>>();
        SUL_DB.query(relate_sqls.join(";")).await.unwrap();
    }
    // while let Some(_) = futures.next().await { }

    Ok(())
}
