use std::env;
use aios_core::pdms_types::*;

use sqlx::{Error, Executor, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::api::children::{travel_children_eles, travel_children_without_leaf};
use crate::aql_api::children::query_travel_children_with_out_leaf_aql;
use crate::consts::PDMS_DATA_STATE;
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

/// 查找该节点下的所有子节点的data_state数据
pub async fn query_refnos_state(refno: RefU64, pool: &Pool<MySql>, arango_database: &ArDatabase) -> anyhow::Result<DataStateVec> {
    let refnos = query_travel_children_with_out_leaf_aql(arango_database, refno).await?;
    if refnos.len() == 0 { return Ok(DataStateVec::default()); }
    let mut r = vec![];
    let sql = gen_query_refnos_state_sql(refnos);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    match result {
        Ok(vals) => {
            for val in vals {
                let refno = RefU64(val.get::<i64, _>("ID") as u64);
                let att_type = val.get::<String, _>("TYPE");
                let name = val.get::<String, _>("NAME");
                let state = val.try_get::<String, _>("STATE").unwrap_or("unset".to_string());
                r.push(DataState {
                    refno,
                    att_type,
                    name,
                    state,
                })
            }
        }
        Err(e) => {
            dbg!(&e);
            dbg!(&sql);
        }
    }
    Ok(DataStateVec { data_states: r })
}

pub async fn query_refnos_scope(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<DataScopeVec> {
    let refnos = travel_children_without_leaf(refno, pool).await?;
    let mut r = vec![];
    let sql = gen_query_refnos_state_sql(refnos);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    match result {
        Ok(vals) => {
            for val in vals {
                let refno = RefU64(val.get::<i64, _>("ID") as u64);
                let att_type = val.get::<String, _>("TYPE");
                let name = val.get::<String, _>("NAME");
                r.push(DataScope {
                    refno,
                    att_type,
                    name,
                })
            }
        }
        Err(e) => {
            dbg!(e);
            dbg!(sql);
        }
    }
    Ok(DataScopeVec {
        data_scopes: r,
    })
}

/// 将设定的state值插入到数据库中
pub async fn insert_refnos_state(vals: DataScopeVec, state: String, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let sql = gen_insert_refnos_state_sql(vals, state);
    let result = pool.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
            dbg!(sql.as_str());
        }
    }
    Ok(())
}

fn gen_insert_refnos_state_sql(vals: DataScopeVec, state: String) -> String {
    let mut sql = String::new();
    let mut insert_sql = String::new();
    sql.push_str(&format!("REPLACE INTO {PDMS_DATA_STATE} (ID,STATE) VALUES "));
    for val in vals.data_scopes {
        if &val.att_type != "WORL" && &val.att_type != "SITE" {
            insert_sql.push_str(&format!("( {},'{}'),", val.refno.0, state));
        }
    }
    insert_sql.remove(insert_sql.len() - 1);
    sql.push_str(insert_sql.as_str());
    sql.push_str(";");
    sql
}

fn gen_query_refnos_state_sql(refnos: Vec<RefU64>) -> String {
    let mut sql = String::new();
    let mut refnos_sql = String::new();
    for refno in refnos {
        refnos_sql.push_str(&format!("{} ,", refno.0));
    }
    refnos_sql.remove(refnos_sql.len() - 1);
    // sql.push_str(&format!("SELECT ID,TYPE,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE ID IN ({}) AND IS_DEL = 0", refnos_sql));
    sql.push_str(&format!("SELECT A.ID ,A.TYPE,A.NAME,B.STATE FROM {PDMS_ELEMENTS_TABLE} A LEFT JOIN {PDMS_DATA_STATE} B ON A.ID = B.ID WHERE A.ID IN ({});", refnos_sql));
    sql
}


#[test]
fn test_gen_query_refnos_state_sql() {
    let refnos = vec![RefU64(0), RefU64(1)];
    let sql = gen_query_refnos_state_sql(refnos);
    println!("sql={:?}", sql);
}