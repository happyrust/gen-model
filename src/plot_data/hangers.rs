use std::collections::HashMap;
use aios_core::pdms_types::RefU64;
use arangors_lite::Database;
use dashmap::DashMap;
use sea_orm::sea_query::IndexType::Hash;
use sqlx::{MySql, Pool, Row};
use crate::api::children::travel_children_with_type;
use crate::aql_api::children::{query_travel_children_aql, query_travel_children_with_type_aql};
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PclaData {
    pub spre_name: String,
    pub count: u32,
    pub unit_weight: String,
}

/// 提前将支吊架出图需要的数据存储在图数据库中
pub async fn save_hangers_data(pool: &Pool<MySql>, database: &Database) -> anyhow::Result<()> {
    let all_hanger_refnos = get_all_hangers_with_atta(pool).await?;
    for (atta_name, hanger_map) in all_hanger_refnos {
        let rest_refno = hanger_map.get("REST");
        let stru_refno = hanger_map.get("STRU");
        if rest_refno.is_none() || stru_refno.is_none() { continue; }
        let rest_refno = rest_refno.unwrap();
        let stru_refno = stru_refno.unwrap();
        // 查找 pcla 的 数据
        let mut pcla_datas = vec![];
        let pcla_refnos = query_travel_children_with_type_aql(database, *rest_refno.value(), "PCLA").await?;
        let mut pcla_map = HashMap::new(); // pcla 只记录 spre的 name 和 相同 spre的数量
        for pcla_refno in pcla_refnos {
            let spre_name = query_foreign_name_aql(pcla_refno.refno, vec!["SPRE", "SPRE"], database).await?;
            if spre_name.is_none() { continue; }
            let count = pcla_map.entry(spre_name.unwrap()).or_insert(0);
            *count += 1;
        }
        for pcla_data in pcla_map {
            pcla_datas.push(PclaData{
                spre_name: pcla_data.0,
                count: pcla_data.1,
                unit_weight: "".to_string()
            });
        }
        // 查找 stru下的所有参考号
        let stru_children = query_travel_children_aql(database, *stru_refno.value()).await?;
        for stru_child in stru_children {
            // 统计 sctn 需要的数据
            if stru_child.noun == "SCTN" {}
        }
    }
    Ok(())
}

/// 根据atta的名字获取所有的支吊架
async fn get_all_hangers_with_atta(pool: &Pool<MySql>) -> anyhow::Result<DashMap<String, DashMap<String, RefU64>>> {
    let atta_name = "R320.060"; // 先拿这一个做测试
    // 找到 atta的名称对应的支吊架 STRU 和 REST 两种
    let mut hangers_map: DashMap<String, DashMap<String, RefU64>> = DashMap::new(); // key -> atta_name , value ->  map : key att_type(REST/STRU) value:refno
    let sql = gen_query_stru_and_rest_with_atta_name_sql(atta_name);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    if results.is_err() { return Ok(hangers_map); }
    let results = results.unwrap();
    for result in results {
        let refno = RefU64(result.get::<i64, _>("ID") as u64);
        let att_type = result.get::<String, _>("TYPE");
        let _name = result.get::<String, _>("NAME");
        if att_type == "STRU" || att_type == "REST" {
            hangers_map.entry(atta_name.to_string()).or_insert_with(DashMap::new).entry(att_type).or_insert(refno);
        }
    }
    Ok(hangers_map)
}

fn gen_query_stru_and_rest_with_atta_name_sql(atta_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,TYPE,NAME FROM PDMS_ELEMENT WHERE NAME LIKE '{}' AND TYPE IN ( 'STRU','REST','ATTA')", atta_name));
    sql
}

#[test]
fn test_hash_map_count() {
    let text = "Hello world good world good world";

    let mut map = HashMap::new();
    for word in text.split_whitespace() {
        let count = map.entry(word).or_insert(0);
        *count += 1;
    }

    println!("{:#?}", map);
}