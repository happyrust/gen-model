use aios_core::pdms_types::RefU64;
use crate::version_management::{RefnoStatusDifference};
use aios_core::data_state::RefnoStatusInfo;
use sqlx::{Executor, MySql, Pool, Row};
use crate::data_interface::tidb_manager::AiosDBManager;


/// 查询该节点所有的数据状态,只返回状态信息，不返回attrmap
///
/// 按照版本赋值顺序返回
pub async fn query_refno_all_status(aios_mgr: &AiosDBManager, refno: String) -> Vec<RefnoStatusInfo> {
    if let Some(pool) = aios_mgr.get_project_pool(&aios_mgr.db_option.project_name) {
        if let Ok(result) = query_all_version(&pool, refno).await {
            return result;
        }
    };
    vec![]
}


/// 查询某个节点两个版本之间的差异数据
///
/// 如果为新增或者删除，则不进行对比，old_content 和 new_content 返回空即可
pub async fn query_difference_between_two_status(refno: &str, old_status: &str, new_status: &str) -> RefnoStatusDifference {
   return RefnoStatusDifference::default();
}


///判断选择的大版本号是否正确
///
/// 查询需要进行版本更新的所有节点的当前版本号，若存在版本号高于选中的大版本号，则选中的大版本号无效，返回false，否则返回true

pub async fn judge_selected_version_number(refnos: Vec<RefU64>, status: &str) -> bool {
    todo!()
}

///查询与当前节点有关联的所有refno
pub async fn query_related_refnos(refno: RefU64) -> Vec<RefU64> {
    todo!()
}


pub async fn query_all_version(pool: &Pool<MySql>, refno: String) -> anyhow::Result<Vec<RefnoStatusInfo>> {
    let sql = gen_query_all_version_sql(refno);
    let val = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    return match val {
        Ok(vals) => {
            let mut result = vec![];
            for val in vals {
                let refno = val.get::<String, _>("refno");
                let status = val.get::<String, _>("status");
                let user = val.get::<String, _>("user");
                let time = val.get::<String, _>("time");
                let note = val.get::<String, _>("note");
                let refno = RefU64::from_url_refno(&refno).unwrap_or_default();
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

