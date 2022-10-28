use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use dashmap::DashMap;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::api::attr::{query_full_attr};
use crate::api::children::query_ancestor_of_type;
use crate::api::element::query_name;
use crate::aql_api::children::query_ancestor_till_type_aql;
use crate::data_interface::tidb_manager::AiosDBManager;

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct AdminData {
    pub team_name: String,
    pub name: String,
    pub s_type: String,
    pub db_type: String,
    pub db_no: i32,
    pub claim: String,
    pub desc: String,
}

pub async fn query_all_db_infos(mgr: Arc<AiosDBManager>) -> anyhow::Result<Vec<AdminData>> {
    let mut r = vec![];
    let db_option = &mgr.db_option;
    let mut team_name_map = DashMap::new();
    if let Some(project_db) = mgr.project_map.get(&db_option.project_name) {
        let all_db_refnos = query_all_db_refnos(project_db.value()).await?;
        for db_refno in all_db_refnos {
            let db_attr = query_full_attr(db_refno, &mgr, Some(vec!["NUMBDB","STYP"])).await?;

            let team_refno = query_ancestor_of_type(db_refno, "TEAM", project_db.value()).await?;
            if team_refno.is_none() { continue; }
            let team_refno = team_refno.unwrap();

            let team_name = if !team_name_map.contains_key(&team_refno) {
                let team_name = query_name(team_refno, project_db.value()).await?;
                team_name_map.insert(team_refno, team_name.clone());
                team_name
            } else {
                team_name_map.get(&team_refno).unwrap().to_string()
            };

            let db_name = db_attr.get_name().to_string();
            let s_type = db_attr.get_str("STYP").unwrap_or("0");
            let mut names = db_name.split('/').collect::<Vec<_>>();
            if names.len() < 2 { continue; }
            let mut name = String::new();
            for n in names {
                name.push_str(n);
            }
            // let db_type = db_types.get(1).unwrap().to_string();
            let numbdb = db_attr.get_i32("NUMBDB").unwrap_or(0);
            let claim = db_attr.get_i32("CLAI").unwrap_or(0);
            let desc = db_attr.get_str("DESC").unwrap_or("unset");
            let stype = match_stype(s_type);
            let claim = match_claim_data(claim);
            r.push(AdminData {
                team_name:team_name[1..].to_string(),
                name:name[1..].to_string(),
                s_type: stype,
                db_type:"MASTER".to_string(),
                db_no: numbdb,
                claim,
                desc: desc.to_string(),
            })
        }
    }
    Ok(r)
}

pub async fn query_all_db_refnos(pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut r = vec![];
    let sql = gen_query_all_db_refnos_sql();
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(results) => {
            for result in results {
                let refno = RefU64(result.get::<i64, _>("ID") as u64);
                r.push(refno);
            }
        }
        Err(error) => {
            dbg!(&error);
        }
    }
    Ok(r)
}

fn match_stype(input: &str) -> String {
    match input {
        "1" => { "DESI".to_string() }
        "2" => { "CATA".to_string() }
        "4" => { "PROP".to_string() }
        "6" => { "ISOD".to_string() }
        "7" => { "PADD".to_string() }
        "8" => { "DICT".to_string() }
        "9" => { "ENGI".to_string() }
        "14" => { "SCHE".to_string() }
        _ => { "".to_string() }
    }
}

fn match_claim_data(input: i32) -> String {
    match input {
        0 => { "unset".to_string() }
        2 => { "Implicit".to_string() }
        _ => { "".to_string() }
    }
}


fn gen_query_all_db_refnos_sql() -> String {
    let mut sql = String::new();
    sql.push_str("SELECT ID FROM DB");
    sql
}