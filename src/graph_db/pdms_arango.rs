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
use aios_core::create_attas_structs::{VirtualEmbedGraphNode, VirtualHoleGraphNode};
use aios_core::options::DbOption;
use aios_core::pdms_types::{PdmsElement, RefU64, RefU64Vec};
use aios_core::tool::db_tool::db1_hash;
use anyhow::anyhow;
use arangors_lite::collection::CollectionType;
use bevy::prelude::dbg;
use dashmap::{DashMap, DashSet};
use futures::future::ok;
use itertools::Itertools;
use log::info;
use parse_pdms_db::parse::WholeAttMap;
use regex::internal::Input;
use crate::api::attr::{query_foreign_refnos_from_table, query_implicit_attr};
use crate::api::children::query_contain_noun_refnos;
use crate::api::element::*;
use crate::api::project_mdb::query_db_nums_of_mdb;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::{DataDocument, ForeignEdges};
use crate::graph_db::structs::{PdmsEleGraphEdge, PdmsEleGraphEdgeWithKey, PdmsEleGraphNode};

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

//establish_basic_auth

pub async fn connect_arangodb_with_basic_auth(db_option: &DbOption) -> anyhow::Result<Database> {
    let conn = Connection::establish_basic_auth(&db_option.arangodb_url, &db_option.arangodb_user, &db_option.arangodb_password)
        .await?;
    Ok(conn.db(&db_option.arangodb_database).await?)
}

pub async fn create_arangodb_conn(database: &Database, collection_name: &str, collection_type: CollectionType) -> anyhow::Result<()> {
    match collection_type {
        CollectionType::Document => {
            let database = database.create_collection(collection_name).await;
            match database {
                Ok(_v) => {}
                Err(e) => {
                    match &e {
                        ClientError::Arango(error) => {
                            if error.code() != 409 {
                                dbg!(&e);
                            }
                        }
                        _ => {
                            dbg!(&e);
                        }
                    }
                }
            }
        }
        CollectionType::Edge => {
            let database = database.create_edge_collection(collection_name).await;
            match database {
                Ok(_v) => {}
                Err(e) => {
                    match &e {
                        ClientError::Arango(error) => {
                            if error.code() != 409 {
                                dbg!(&e);
                            }
                        }
                        _ => {
                            dbg!(&e);
                        }
                    }
                }
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
    for edge in edges.chunks(ARANGODB_SAVE_AMOUNT) {
        let json = serde_json::to_value(edge)?;
        save_arangodb_with_db_option(json, db_option, "pdms_edges").await?;
    }
    Ok(())
}

/// 保存虚拟孔洞数据到图数据库
pub async fn save_virtual_hole_value_to_arangodb(db_option: &DbOption) -> anyhow::Result<()> {
    //获取虚拟孔洞信息
    // let hole_data = insert_virtual_hole_data();
    // for data in hole_data.chunks(ARANGODB_SAVE_AMOUNT) {
    //     let json = serde_json::to_value(data)?;
    //     save_arangodb_with_db_option(json, db_option, "hole_data").await?;
    // }
    //
    // let embed_data = insert_virtual_embed_data();
    // for data in embed_data.chunks(ARANGODB_SAVE_AMOUNT) {
    //     let json = serde_json::to_value(data)?;
    //     save_arangodb_with_db_option(json, db_option, "embed_data").await?;
    // }

    Ok(())
}

///插入虚拟孔洞信息
// fn insert_virtual_hole_data() -> Vec<VirtualHoleGraphNode> {
//     let mut virtual_data = Vec::new();
//     let data = [
//         ("24383_46246", 1, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "1RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_66592", 2, "a2b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "2RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_380", 3, "a3b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "3RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_379", 4, "a4b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "4RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_381", 5, "a5b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "5RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_1955", 6, "a6b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "6RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_1967", 7, "a7b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "7RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_46246", 8, "a8b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "8RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_46246", 9, "a9b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "9RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("24383_46246", 10, "a10b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "10RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("23584_78701", 11, "a11b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "11RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("23584_78693", 12, "a12b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "12RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("23584_78694", 13, "a13b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "13RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("23584_78702", 14, "a14b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "14RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("15201_381", 15, "a15b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "15RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("15201_379", 16, "a16b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "16RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("15201_380", 17, "a17b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "17RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("15203_1955", 18, "a18b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "18RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("15203_1961", 19, "a19b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "19RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//         ("15203_1967", 20, "a20b7aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "FLOOR 16 of CFLOOR /1RS-WF04-F-C-F001", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[-9727.52,18702.21,3600]", "MODIFY", "SYSTEM", "2022/10/21/星期五 9:48:48", "RECT", "Y is Y and Z is -Z", "20RS04TT0012T", "24383/66569", "", "AFW", 273, 273, 0, 0, "[]", 0, "null", "null", "不锈钢材料", 6.5, 2, "φ250", 0.0, 0.0, 0, 0, "null"),
//     ];
//     for i in data {
//         let hole_data = VirtualHoleGraphNode {
//             _key: i.0.parse().unwrap(),
//             intelld: i.1,
//             code: i.2.parse().unwrap(),
//             relyitem: i.3.parse().unwrap(),
//             mainitem: i.4.parse().unwrap(),
//             speciality: i.5.parse().unwrap(),
//             position: i.6.parse().unwrap(),
//             holework: i.7.parse().unwrap(),
//             workby: i.8.parse().unwrap(),
//             time: i.9.parse().unwrap(),
//             shape: i.10.parse().unwrap(),
//             ori: i.11.parse().unwrap(),
//             itemref: i.12.parse().unwrap(),
//             mainitemref: i.13.parse().unwrap(),
//             openitem: i.14.parse().unwrap(),
//             plugtype: i.15.parse().unwrap(),
//             sizeheigh: i.16 as f32,
//             sizewidth: i.17 as f32,
//             bankwidth: i.18 as f32,
//             bankheight: i.19 as f32,
//             hotdis: i.20.parse().unwrap(),
//             heatthick: i.21 as f32,
//             refno: i.22.parse().unwrap(),
//             fittrefno: i.23.parse().unwrap(),
//             subsmeterial: i.24.parse().unwrap(),
//             substhickness: i.25,
//             icreate: i.26,
//             substype: i.27.parse().unwrap(),
//             extentlength1: i.28,
//             extentlength2: i.29,
//             second: i.30,
//             rehole: i.31,
//             note: i.32.parse().unwrap(),
//         };
//         virtual_data.push(hole_data);
//     }
//     dbg!(&virtual_data);
//     virtual_data
// }

///插入虚拟埋件信息
// fn insert_virtual_embed_data() -> Vec<VirtualEmbedGraphNode> {
//     let mut virtual_data = Vec::new();
//     let data = [
//         ("24383_46246", 21, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46246", "51RS04TT0012T", "AFW", ""),
//         ("24383_66592", 22, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46247", "52RS04TT0012T", "AFW", ""),
//         ("24383_380", 23, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46248", "53RS04TT0012T", "AFW", ""),
//         ("24383_379", 24, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46249", "54RS04TT0012T", "AFW", ""),
//         ("24383_381", 25, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46250", "55RS04TT0012T", "AFW", ""),
//         ("24383_1955", 26, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46251", "56RS04TT0012T", "AFW", ""),
//         ("24383_1967", 27, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46252", "57RS04TT0012T", "AFW", ""),
//         ("24383_46246", 28, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46253", "58RS04TT0012T", "AFW", ""),
//         ("24383_46246", 29, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46254", "59RS04TT0012T", "AFW", ""),
//         ("24383_46246", 30, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46255", "60RS04TT0012T", "AFW", ""),
//         ("23584_78701", 31, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46256", "61RS04TT0012T", "AFW", ""),
//         ("23584_78693", 32, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46257", "62RS04TT0012T", "AFW", ""),
//         ("23584_78694", 33, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46258", "63RS04TT0012T", "AFW", ""),
//         ("23584_78702", 34, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46259", "64RS04TT0012T", "AFW", ""),
//         ("15201_381", 35, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46260", "65RS04TT0012T", "AFW", ""),
//         ("15201_379", 36, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46261", "66RS04TT0012T", "AFW", ""),
//         ("15201_380", 37, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46262", "67RS04TT0012T", "AFW", ""),
//         ("15203_1955", 38, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46263", "68RS04TT0012T", "AFW", ""),
//         ("15203_1961", 39, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46264", "69RS04TT0012T", "AFW", ""),
//         ("15203_1967", 40, "a1aa1f2a-fd8b-4bdc-8d97-ffaa120ced7a", "CFLOOR1", "24383_46246", "CAP 1 of BRANCH /Copy-of-Copy-of-1WCC0200-168.3-NADB-R70-R312]R412", "管道", "[900,200.21,600]", "X is X and Z is -Z", "", "张三", "2022/12/21/星期一 9:48:48", "RECT", "2019/09/08", 567.89, 445.56, 50.0, 32.0, 273, 273.2, 0.0, 2000.2, "24383_46265", "70RS04TT0012T", "AFW", ""),
//     ];
//     for i in data {
//         let hole_data = VirtualEmbedGraphNode {
//             _key: i.0.parse().unwrap(),
//             intelld: i.1,
//             code: i.2.parse().unwrap(),
//             relyitem: i.3.parse().unwrap(),
//             relyitemref: i.4.parse().unwrap(),
//             mainitem: i.5.parse().unwrap(),
//             speciality: i.6.parse().unwrap(),
//             position: i.7.parse().unwrap(),
//             ori: i.8.parse().unwrap(),
//             work: i.9.parse().unwrap(),
//             workby: i.10.parse().unwrap(),
//             time: i.11.parse().unwrap(),
//             standertype: i.12.parse().unwrap(),
//             openitem: i.13.to_string(),
//             holework: i.14.to_string(),
//             sizelength: i.15,
//             sizewidth: i.16,
//             sizethickness: i.17 as f32,
//             minthickness: i.18 as f32,
//             load: i.19,
//             mindistance: i.20,
//             subsmeterial: i.21.to_string(),
//             fittid: i.22.parse().unwrap(),
//             _ref: i.23.parse().unwrap(),
//             shape: i.24.parse().unwrap(),
//             note: i.25.parse().unwrap(),
//         };
//         virtual_data.push(hole_data);
//     }
//     dbg!(&virtual_data);
//     virtual_data
// }

pub async fn sync_pdms_to_graph_db(mgr: Arc<AiosDBManager>, db_option: DbOption) -> anyhow::Result<()> {
    let mut time = Instant::now();
    for project in &db_option.included_projects {
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        let pool = AiosDBManager::get_db_pool(&default_conn, project).await.unwrap();
        let include_module = vec!["DESI", "CATA"];
        for module in include_module {
            // let mut handles = vec![];
            // 只保存 指定mdb的desi的numbdb
            let numbdbs = query_db_nums_of_mdb(&format!("/{}", db_option.mdb_name), module, &pool).await?;
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
                pending.push_back((site.refno, site.noun.clone()));
            }
            set_level_edges(sites, &mut sibl_edges).await?;
            // 遍历整个pdms树
            while pending.len() != 0 {
                let (pending_refno, pending_noun) = pending.pop_front().unwrap();
                if let Ok(children) = query_children_eles(pending_refno, project_db.value()).await {
                    if children.len() != 0 {
                        for child in &children {
                            pending.push_back(
                                (child.refno, child.noun.clone())
                            );
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
                    save_arangodb_with_database(json, sibl_collection, &database, false).await?;
                    if tubi_edges.len() != 0 {
                        let tubi_json = serde_json::to_value(&take(&mut tubi_edges))?;
                        save_arangodb_with_database(tubi_json, tubi_collection, &database, false).await?;
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
        let from_refno = (eles[i].refno);
        let to_refno = (eles[i - 1].refno);
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
    let mut aql_string = "LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }".to_string();
    if db_option.replace_dbs {
        aql_string = aql_string.replace("INSERT", "REPLACE");
    }
    let aql = AqlQuery::new(&aql_string)
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn save_arangodb_with_database(json: Value, collection: &str, database: &Database, replace: bool) -> anyhow::Result<()> {
    let mut aql_string = "LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }".to_string();
    if replace {
        aql_string = aql_string.replace("INSERT", "REPLACE");
    }
    let aql = AqlQuery::new(&aql_string)
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}

pub async fn remove_arangodb_with_refno_key(refnos: &Vec<RefU64>, collection: &str, database: &Database) -> anyhow::Result<bool> {
    let keys = refnos.into_iter().map(|refno| refno.to_url_refno()).collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "FOR D IN @DATA
                    REMOVE D IN @COLLECTION")
        .bind_var("data", keys)
        .bind_var("collection", collection);
    let result = database.aql_query::<Vec<()>>(aql).await;
    Ok(!result.is_err())
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
    let mut aql_string = "LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }".to_string();
    if db_option.replace_dbs {
        aql_string = aql_string.replace("INSERT", "REPLACE");
    }
    let aql = AqlQuery::new(&aql_string)
        .bind_var("@collection", collection)
        .bind_var("elements", json);
    let _result: Vec<()> = database.aql_query(aql).await?;
    Ok(())
}