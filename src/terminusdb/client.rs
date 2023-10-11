use aios_core::pdms_types::{NamedAttrMap, RefU64, WholeAttMap};
use dashmap::DashMap;
use termnius_client::client::TDBClient;
use crate::graph_db::structs::PdmsEleDataVersioned;

pub async fn get_versioned_client() -> TDBClient {
    let mut client = termnius_client::client::TDBClientBuilder::default()
        .server_url("http://localhost:6363".to_string())
        .auth_info(termnius_client::client::AuthInfo::new())
        .session_info(termnius_client::client::SessionInfo::new())
        .repo_info(termnius_client::client::RepoInfo {
            team: "admin".to_string(),
            db: "e3d".to_string(),
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

const CHUNK_COUNT: usize = 10_000;
// const CHUNK_COUNT: usize = 1;

pub async fn save_versioned_pdms_eles(client: &TDBClient, total_attr_map: &DashMap<RefU64, WholeAttMap>, db_num: i32) -> anyhow::Result<()> {
    let mut eles = Vec::with_capacity(total_attr_map.len());
    for kv in total_attr_map.iter() {
        let att_map: NamedAttrMap = kv.value().merge().into();
        let ele = PdmsEleDataVersioned{
            id: format!("PdmsElement/{}", kv.key().to_string()),
            refno: *kv.key(),
            owner: att_map.get_refno_by_att_or_default("OWNER"),
            name: att_map.get_string_or_default("NAME"),
            noun: att_map.get_type(),
            // order: 0,
            dbnum: db_num,
            cata_hash: None,
        };
        eles.push(ele);
    }

    for result in eles.chunks(CHUNK_COUNT) {
        let json = serde_json::to_string(result)?;
        // dbg!(&json);
        // let json = "[{\"@type\":\"PdmsElement\",\"refno\":\"25688_33250\",\"owner\":{\"@ref\":\"25688_33246\"},\"name\":\"\",\"noun\":\"PAVE\",\"dbnum\":1112,\"cata_hash\":\"0\"}]".to_string();
        let doc_res = client.insert_doc(json.as_str(), "dpc", "Add Elements", false, false, false).await?;
        dbg!(doc_res);
        // continue;
        // save_arangodb_with_db_option(database, json, AQL_PDMS_ELES_COLLECTION).await?;
    }


    Ok(())
}
