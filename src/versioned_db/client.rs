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
use petgraph::Directed;
use petgraph::Undirected;
use petgraph::algo::all_simple_paths;
use petgraph::graph::Graph;
use petgraph::graph::NodeIndex;
use petgraph::graphmap::GraphMap;
use petgraph::graphmap::UnGraphMap;
use petgraph::prelude::DiGraphMap;
use petgraph::visit::IntoEdgesDirected;
use rayon::prelude::*;
use sea_orm::entity::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Instant;
use std::sync::Arc;
use aios_core::options::DbOption;

// const JSON_CHUNK_COUNT: usize = 5_000;

// pub async fn save_versioned_pdms_eles(
//     client: &TDBClient,
//     total_attr_map: &DashMap<RefU64, WholeAttMap>,
//     db_num: i32,
//     db_option: &DbOption,
// ) -> anyhow::Result<()> {
//     let mut eles = Vec::with_capacity(total_attr_map.len());
//     for kv in total_attr_map.iter() {
//         let att_map: NamedAttrMap = kv.value().merge().into();
//         let ele = PdmsEleDataVersioned {
//             id: format!("PdmsElement/{}", kv.key().to_string()),
//             refno: *kv.key(),
//             owner: att_map.get_refno_by_att_or_default("OWNER"),
//             name: att_map.get_string_or_default("NAME"),
//             noun: att_map.get_type(),
//             dbnum: db_num,
//             cata_hash: None,
//             status_tag: None,
//         };
//         eles.push(ele);
//     }
//
//     // let mut futures = FuturesUnordered::new();
//     for result in eles.chunks(JSON_CHUNK_COUNT) {
//         let json = serde_json::to_string(result)?;
//
//         let doc_res = client
//             .insert_doc(
//                 json.as_str(),
//                 "dpc",
//                 "Add Pdms Elements.",
//                 false,
//                 false,
//                 true,
//             )
//             .await
//             .unwrap_or_default();
//         dbg!(doc_res);
//
//         // let project = db_option.project_name.clone();
//         // futures.push(tokio::task::spawn(async move {
//         //     // let mut conn = pool.get_conn().await.unwrap();
//         //     let mut client = get_versioned_client(&project).await;
//         //     // let info = client.db_info().await;
//         //     // dbg!(info);
//         //     let doc_res = client.insert_doc(json.as_str(), "dpc", "Add Pdms Elements.", false, false, true).await.unwrap_or_default();
//         //     dbg!(doc_res);
//         // }));
//     }
//
//
//
//     // while let Some(_) = futures.next().await { }
//
//     Ok(())
// }

/// 保存element数据到版本管理
/// todo 后续再考虑 record links
// 先暂时使用relate的方式
// #[tracing::instrument]
pub async fn save_pdms_eles_to_surreal(
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    db_num: i32,
    children_map: &HashMap<RefU64, Vec<(RefU64, String)>>,
    option: &DbOption,
) -> anyhow::Result<()> {
    use itertools::Itertools;
    let keys = total_attr_map.iter().map(|x| *x.key()).collect::<Vec<_>>();
    let mut model_chunks = keys.par_chunks(option.pe_chunk as _).map(|chunk| {
        let mut model_chunk = vec![];
        for &refno in chunk {
            let att_map = total_attr_map.get(&refno).unwrap();
            let owner = att_map.get_refno_by_att_or_default("OWNER");
            let noun = att_map.get_type();
            let ele = pe::SPdmsElement {
                id: refno.to_string(),
                refno,
                owner,
                name: att_map.get_string_or_default("NAME"),
                noun,
                dbnum: db_num,
                cata_hash: att_map.cal_cata_hash(),
                status_tag: None,
                version_tag: None,
                e3d_version: att_map.get_e3d_version(),
                lock: false,
                deleted: false,
            };

            model_chunk.push(ele);
        }
        model_chunk
    }).collect::<Vec<_>>();

    let mut time = Instant::now();
    let mut join_set = tokio::task::JoinSet::new();
    for models in model_chunks {
        let mut jsons_str = vec![];
        for m in models {
            jsons_str.push(m.gen_sur_json());
        }
        let sql = format!("INSERT IGNORE INTO pe [{}]", jsons_str.join(","));
        //手动修改，替换掉""
        join_set.spawn(async move {
            SUL_DB.query(sql).await.unwrap();
        });
    }
    while let Some(_) = join_set.join_next().await {}

    println!("Save pes task costs {} s", time.elapsed().as_secs_f32());

    let mut relate_join_set = tokio::task::JoinSet::new();
    // 使用owner创建relate关系
    let mut all_relate_sqls = vec![];
    time = Instant::now();
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
                format!(
                    "RELATE pe:{}->pe_owner->pe:{} set order_num = {}",
                    child.to_string(),
                    owner.to_string(),
                    i
                )
            })
            .collect::<Vec<String>>();
        all_relate_sqls.extend_from_slice(&relate_sqls);
    }
    let mut chunks = all_relate_sqls.chunks(option.pe_chunk as _);
    for mut s in chunks {
        let sql = s.into_iter().join(";");
        relate_join_set.spawn(async move {
            SUL_DB.query(sql).await.unwrap();
        });
    }
    while let Some(_) = relate_join_set.join_next().await {}
    println!("Relate pes task costs {} s", time.elapsed().as_secs_f32());
    Ok(())
}
