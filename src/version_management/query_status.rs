use aios_core::pdms_types::RefU64;
use crate::version_management::{RefnoStatusDifference};
use aios_core::data_state::RefnoStatusInfo;
use aios_core::get_db_option;
#[cfg(feature = "sql")]
use aios_core::db_pool::get_project_pool;
use sqlx::{Executor, MySql, Pool, Row};
use crate::aql_api::children::query_travel_children_aql;
use std::str::FromStr;

/// 查询该节点所有的数据状态,只返回状态信息，不返回attrmap
///
/// 按照版本赋值顺序返回
#[cfg(feature = "sql")]
pub async fn query_refno_all_status(refno: String) -> Vec<RefnoStatusInfo> {
    let db_option = get_db_option();
    if let Ok(pool) = get_project_pool(&db_option).await {
        if let Ok(result) = query_all_version(&pool, refno).await {
            return result;
        }
    };
    vec![]
}

#[cfg(not(feature = "sql"))]
pub async fn query_refno_all_status(_refno: String) -> Vec<RefnoStatusInfo> {
    vec![]
}


/// 查询某个节点两个版本之间的差异数据
///
/// 如果为新增或者删除，则不进行对比，old_content 和 new_content 返回空即可
///
/// refno为需比较的节点参考号，old_status为两比较版本中的旧版，new_status为两比较版本中的新版
pub async fn query_difference_between_two_status(refno: &str, old_status: &str, new_status: &str) -> RefnoStatusDifference {
    return RefnoStatusDifference::default();
}


///判断选择的大版本号是否正确
///
/// 查询需要进行版本更新的所有节点的当前版本号，若存在版本号高于选中的大版本号，则选中的大版本号无效，返回false，否则返回true,同时返回需设置新状态的refnos
///
///refnos为选中节点的参考号，status为设置的版本号
pub async fn judge_selected_version_number(database: &ArDatabase, refnos: Vec<RefU64>, status: String) -> anyhow::Result<(bool, Option<Vec<RefU64>>)> {
    //1.找到当前refno下所有子孙节点的refno
    let mut refno_vec = vec![];
    for refno in refnos {
        let mut result = query_travel_children_aql(database, refno).await?;
        let mut result = result.iter().map(|x| x.refno).collect();
        refno_vec.append(&mut result)
    }
    //2.遍历这些refno对应的status，一旦遇到某个refno的status高于所要设置的status就break，返回false
    //Ok((false,None))

    //3.若循环结束，则返回true
    Ok((true, Some(refno_vec)))
}


pub async fn query_all_version(pool: &Pool<MySql>, refno: String) -> anyhow::Result<Vec<RefnoStatusInfo>> {
    let sql = gen_query_all_version_sql(refno);
    let val = sqlx::query(&sql).fetch_all(pool).await;
    return match val {
        Ok(vals) => {
            let mut result = vec![];
            for val in vals {
                let refno = val.get::<String, _>("refno");
                let status = val.get::<String, _>("status");
                let user = val.get::<String, _>("user");
                let time = val.get::<String, _>("time");
                let note = val.get::<String, _>("note");
                let refno = RefU64::from_str(&refno).unwrap_or_default();
                result.push(RefnoStatusInfo {
                    refno,
                    status,
                    user,
                    time,
                    note,
                });
            }
            Ok(result)
        }
        Err(e) => {
            dbg!(&e);
            Ok(vec![])
        }
    };
}


fn gen_query_all_version_sql(refno: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT refno,status,user,time,note FROM data_status WHERE refno = '{}'", refno));
    sql
}

