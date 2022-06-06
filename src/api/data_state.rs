use aios_core::pdms_types::{DataScope, DataScopeVec, DataState, DataStateVec, RefU64};
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::api::children::travel_children_eles;
use crate::consts::PDMS_DATA_STATE;
use crate::consts::PDMS_ELEMENTS_TABLE;

/// 查找该节点下的所有子节点的data_state数据
pub async fn query_refnos_state(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<DataStateVec> {
    let refnos = travel_children_eles(refno, pool).await?;
    let mut r = vec![];
    let sql = gen_query_refnos_state_sql(refnos);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    match result {
        Ok(vals) => {
            for val in vals {
                let refno = RefU64(val.get::<i64, _>("id") as u64);
                let att_type = val.get::<String, _>("type");
                let name = val.get::<String, _>("name");
                let state = val.get::<String, _>("state");
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
    let refnos = travel_children_eles(refno, pool).await?;
    let mut r = vec![];
    let sql = gen_query_refnos_scope_sql(refnos);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
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

fn gen_query_refnos_state_sql(refnos: Vec<RefU64>) -> String {
    let mut sql = String::new();
    let mut refnos_sql = String::new();
    for refno in refnos {
        refnos_sql.push_str(&format!("{} ,", refno.0));
    }
    refnos_sql.remove(refnos_sql.len() - 1);
    sql.push_str(&format!("SELECT * FROM {PDMS_DATA_STATE} WHERE ID IN ({})", refnos_sql));
    sql
}

fn gen_query_refnos_scope_sql(refnos: Vec<RefU64>) -> String {
    let mut sql = String::new();
    let mut refnos_sql = String::new();
    for refno in refnos {
        refnos_sql.push_str(&format!("{} ,", refno.0));
    }
    refnos_sql.remove(refnos_sql.len() - 1);
    sql.push_str(&format!("SELECT ID,TYPE,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE ID IN ({}) AND IS_DEL = 0", refnos_sql));
    sql
}

#[test]
fn test_gen_query_refnos_state_sql() {
    let refnos = vec![RefU64(0), RefU64(1)];
    let sql = gen_query_refnos_state_sql(refnos);
    println!("sql={:?}", sql);
}