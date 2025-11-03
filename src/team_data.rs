use crate::consts::TEAM_DATA_TABLE;
use aios_core::aios_db_mgr::PdmsDataInterface;
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::error::init_query_error;
use aios_core::{RefU64, SUL_DB, init_test_surreal, query_filter_ancestors};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

///Admin模块DB信息
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct SysDBData {
    pub team_name: String,
    pub name: String,
    pub s_type: String,
    pub db_type: String,
    pub db_no: i32,
    pub claim: String,
    pub desc: String,
}

///同步system db的信息
pub async fn sync_team_data(mgr: &AiosDBMgr) -> anyhow::Result<()> {
    match query_all_db_refnos().await {
        Ok(db_refnos) => {
            let mut r = vec![];
            let mut team_name_map: HashMap<RefU64, String> = HashMap::new();
            for refno in db_refnos {
                // 找到所属的team
                let team = query_filter_ancestors(refno.into(), &vec!["TEAM"]).await?;
                if team.is_empty() {
                    continue;
                };
                let team_refno = team[0].refno();
                let team_name = if team_name_map.contains_key(&team_refno) {
                    team_name_map.get(&team_refno).unwrap().to_string()
                } else {
                    let Ok(team_name) = mgr.get_name(team_refno).await else {
                        continue;
                    };
                    team_name_map.entry(team_refno).or_insert(team_name.clone());
                    team_name
                };

                // 获取db的属性
                let db_attr = mgr.get_attr(refno).await?;
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
                let numbdb = db_attr.get_i32("NUMBDB").unwrap_or(0);
                let claim = db_attr.get_i32("CLAI").unwrap_or(0);
                let desc = db_attr.get_str("DESC").unwrap_or("unset");
                let stype = match_stype(s_type);
                let claim = match_claim_data(claim);
                r.push(SysDBData {
                    team_name: team_name[1..].to_string(),
                    name: name[1..].to_string(),
                    s_type: stype,
                    db_type: "MASTER".to_string(),
                    db_no: numbdb,
                    claim,
                    desc: desc.to_string(),
                });
            }
            // 保存数据
            #[cfg(feature = "sql")]
            {
                let pool = mgr.get_project_pool().await?;
                let table_sql = gen_create_team_data_sql();
                let result = sqlx::query(&table_sql).execute(&pool).await;
                if let Err(e) = result {
                    dbg!(&e);
                }
                let data_sql = gen_save_team_data_sql(r);
                let result = sqlx::query(&data_sql).execute(&pool).await;
                if let Err(e) = result {
                    dbg!(&data_sql);
                    dbg!(&e);
                }
            }
        }
        Err(e) => init_query_error("", e, &std::panic::Location::caller().to_string()),
    }
    Ok(())
}

/// 查询 DB 类型的所有参考号
async fn query_all_db_refnos() -> anyhow::Result<Vec<RefU64>> {
    let sql = "SELECT value REFNO FROM DB".to_string();
    let mut result = SUL_DB.query(sql).await?;
    let refnos: Vec<RefU64> = result.take(0)?;
    Ok(refnos)
}

fn match_stype(input: &str) -> String {
    match input {
        "1" => "DESI".to_string(),
        "2" => "CATA".to_string(),
        "4" => "PROP".to_string(),
        "6" => "ISOD".to_string(),
        "7" => "PADD".to_string(),
        "8" => "DICT".to_string(),
        "9" => "ENGI".to_string(),
        "14" => "SCHE".to_string(),
        _ => "".to_string(),
    }
}

fn match_claim_data(input: i32) -> String {
    match input {
        0 => "unset".to_string(),
        2 => "Implicit".to_string(),
        _ => "".to_string(),
    }
}

fn gen_create_team_data_sql() -> String {
    let mut sql = String::new();
    sql.push_str(
        format!(
            "
    CREATE TABLE IF NOT EXISTS {TEAM_DATA_TABLE} (
        TEAM_NAME VARCHAR(100) NOT NULL,
        NAME VARCHAR(100) PRIMARY KEY,
        S_TYPE VARCHAR(50) ,
        DB_TYPE VARCHAR(50) ,
        DB_NO INT ,
        CLAIM VARCHAR(50) ,
        `DESC` VARCHAR(255) )"
        )
        .as_str(),
    );
    sql
}

fn gen_save_team_data_sql(data: Vec<SysDBData>) -> String {
    let mut sql = String::from(&format!(
        "INSERT IGNORE INTO {TEAM_DATA_TABLE} (TEAM_NAME, NAME, S_TYPE, DB_TYPE, DB_NO, CLAIM, `DESC`) VALUES"
    ));
    let b_empty = data.is_empty();
    for d in data {
        sql.push_str(
            &format!(
                "('{}','{}','{}','{}',{},'{}','{}'),",
                d.team_name, d.name, d.s_type, d.db_type, d.db_no, d.claim, d.desc
            )
            .as_str(),
        )
    }
    if !b_empty {
        sql.remove(sql.len() - 1);
    }
    sql.push_str(";");
    sql
}

#[tokio::test]
async fn test_ancestor() -> anyhow::Result<()> {
    init_test_surreal().await;
    let refno = RefU64::from_str("24575/2195").unwrap();
    let team = query_filter_ancestors(refno.into(), &vec!["TEAM"]).await?;
    dbg!(&team);
    Ok(())
}
