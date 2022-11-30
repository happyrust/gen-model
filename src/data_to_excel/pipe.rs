use sqlx::{MySql, Pool};
use crate::api::element::query_types_refnos;

/// 获取二三维校验 pipe 需要的数据
pub async fn query_pipe_data(pool:&Pool<MySql>) -> anyhow::Result<()> {
    let pipes = query_types_refnos(&vec!["PIPE"],pool,None).await?;
    Ok(())
}