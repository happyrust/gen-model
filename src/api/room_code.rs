use aios_core::pdms_types::{PdmsElement, RefU64};
use arangors_lite::{AqlQuery, Database};
use sqlx::{MySql, Pool, Row};
use crate::api::element::query_ele_nodes_by_refnos;
use crate::aql_api::convert_refno_vec_from_vec_string;
use crate::aql_api::pdms_room::query_all_need_compute_room_refno;

/// 查询房间号
pub async fn query_room_code(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<String>> {
    let sql = gen_query_room_code_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(val) => {
            Ok(Some(val.get::<String, _>("ROOM_NAME")))
        }
        Err(e) => {
            Ok(None)
        }
    };
}

/// 查找所有房间节点，暂时按 1516 命名格式过滤
pub async fn query_room_nodes(dbno:&Vec<i32>,pool:&Pool<MySql>) -> anyhow::Result<Vec<PdmsElement>> {
    let room_infos = query_all_need_compute_room_refno(
        dbno,
        "FRMW",
        Some("-RM"),
        pool,
    ).await?;
    let room_infos = room_infos.into_iter().map(|x| x.0).collect::<Vec<_>>();
    let nodes = query_ele_nodes_by_refnos(room_infos,pool).await?;
    Ok(nodes.into_iter().map(|x| x.into()).collect())
}

fn gen_query_room_code_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ROOM_NAME FROM ROOM_CODE WHERE REFNO = {}", refno.0));
    sql
}