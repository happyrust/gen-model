use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use aios_core::AttrVal;
use aios_core::pdms_types::*;
use dashmap::DashMap;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::api::attr::{query_explicit_attr};
use crate::api::children::query_ancestor_of_type;
use crate::api::element::query_name;
use crate::aql_api::children::{query_ancestor_name_of_type_aql, query_ancestor_till_type_aql, query_ancestor_till_types_aql};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::consts::TEAM_DATA_TABLE;
use aios_core::pdms_user::PdmsElementWithUser;
use crate::data_interface::interface::PdmsDataInterface;


///管理员信息
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

///同步system db的信息
pub async fn sync_system_db(mgr: &AiosDBManager) -> anyhow::Result<()> {
    let mut team_name_map = DashMap::new();
    let database = mgr.get_arango_db().await?;
    for project_db in mgr.project_map.iter() {
        let mut r = vec![];
        let all_db_refnos = query_all_db_refnos(project_db.value()).await?;
        for db_refno in all_db_refnos {
            // let db_attr = mgr.get_attr(db_refno).await;
            let Ok(db_attr) = aios_core::get_named_attmap(db_refno).await else{
                continue;
            };
            let team_refno = query_ancestor_till_types_aql(&database, db_refno, vec!["TEAM"]).await?;
            if team_refno.is_none() {
                continue;
            }
            let team_refno = team_refno.unwrap().refno;

            let team_name = if !team_name_map.contains_key(&team_refno) {
                let team_name = query_name(team_refno, project_db.value()).await?;
                team_name_map.insert(team_refno, team_name.clone());
                team_name
            } else {
                team_name_map.get(&team_refno).unwrap().to_string()
            };

            let db_name = db_attr.get_name_or_default();
            let s_type = db_attr.get_str("STYP").unwrap_or("0");
            let mut names = db_name.split('/').collect::<Vec<_>>();
            if names.len() < 2 {
                continue;
            }
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
                team_name: team_name[1..].to_string(),
                name: name[1..].to_string(),
                s_type: stype,
                db_type: "MASTER".to_string(),
                db_no: numbdb,
                claim,
                desc: desc.to_string(),
            })
        }
        let table_sql = gen_create_team_data_sql();
        let result = sqlx::query(&table_sql).execute(project_db.value()).await;
        if let Err(e) = result { dbg!(&e); }
        let data_sql = gen_save_team_data_sql(r);
        let result = sqlx::query(&data_sql).execute(project_db.value()).await;
        if let Err(e) = result {
            dbg!(&data_sql);
            dbg!(&e);
        }
    }
    Ok(())
}

pub async fn query_all_db_refnos(pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut r = vec![];
    let sql = gen_query_all_db_refnos_sql();
    let results = sqlx::query(&sql).fetch_all(pool).await;
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

pub async fn get_pdms_tree_user(elements: Vec<PdmsElement>, aios_mgr: &AiosDBManager) -> Vec<PdmsElementWithUser> {
    let mut data = Vec::new();
    for element in elements {
        let refno = element.refno;
        let need_query_user_noun = vec!["PIPE", "SITE", "ZONE", "BRAN", "EQUI", "STRU", "HVAC", "REST"];
        let mut final_user = String::new();
        if need_query_user_noun.contains(&element.noun.as_str()) {
            let Some((_, pool)) =
                aios_mgr.get_project_pool_by_refno(refno).await else {
                continue;
            };
            if let Ok(explicit_attr) = query_explicit_attr(refno, &pool).await {
                if let Some(user) = explicit_attr.map.get(&642952117) {
                    if let AttrVal::StringType(u) = user {
                        final_user = u.to_string();
                    }
                }
            }
        }
        data.push(PdmsElementWithUser::from_pdms_element(element, &final_user));
    }
    data
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

