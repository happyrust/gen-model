use std::sync::Arc;
use aios_core::options::DbOption;
use aios_core::orm::pdms_element;
use aios_core::pdms_types::*;
use dashmap::DashMap;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use termnius_client::client::TDBClient;
use sea_orm::entity::prelude::*;
use crate::graph_db::structs::PdmsEleDataVersioned;
use itertools::Itertools;

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

const SQL_CHUNK_COUNT: usize = 500;
const JSON_CHUNK_COUNT: usize = 10_000;

pub async fn save_versioned_pdms_eles(client: &TDBClient, total_attr_map: &DashMap<RefU64, WholeAttMap>, db_num: i32, db_option: &DbOption) -> anyhow::Result<()> {
    let mut eles = Vec::with_capacity(total_attr_map.len());
    for kv in total_attr_map.iter() {
        let att_map: NamedAttrMap = kv.value().merge().into();
        let ele = PdmsEleDataVersioned{
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

        let doc_res = client.insert_doc(json.as_str(), "dpc", "Add Pdms Elements.", false, false, true).await.unwrap_or_default();
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


pub async fn save_pdms_eles_to_versioned(db_option: &DbOption, project: &str, total_attr_map: &DashMap<RefU64, WholeAttMap>, db_num: i32) -> anyhow::Result<()> {
    let mut model_chunks: Vec<Vec<pdms_element::ActiveModel>> = vec![];
    for chunk in &total_attr_map.into_iter().chunks(SQL_CHUNK_COUNT) {
        let mut model_chunk = vec![];
        for kv in chunk{
            let att_map: NamedAttrMap = kv.value().merge().into();

            model_chunk.push(pdms_element::Model{
                id: kv.key().to_refno_string(),
                refno: *kv.key(),
                owner: att_map.get_refno_by_att_or_default("OWNER"),
                name: att_map.get_string_or_default("NAME"),
                noun: att_map.get_type(),
                dbnum: db_num,
                cata_hash: None,
                status_tag: None,
            }.into());
        }
        model_chunks.push(model_chunk);
    }
    // let mut futures = FuturesUnordered::new();
    for models in model_chunks {

        let db = sea_orm::Database::connect(&db_option.get_mysql_db_conn_str(project))
            .await
            .unwrap();
        // futures.push(tokio::task::spawn(async move {
          let _ = aios_core::orm::PdmsElement::insert_many(models).exec(&db).await;
        // }));
        
        let test_box: aios_core::orm::BOX::ActiveModel = aios_core::orm::BOX::Model{
            id: "0/0".to_owned(),
            ..Default::default()
        }.into();

        let _ = test_box.insert(&db).await.unwrap();

        break;
    }

    // while let Some(_) = futures.next().await { }

    Ok(())
}
