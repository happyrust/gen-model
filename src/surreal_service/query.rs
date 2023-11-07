use crate::surreal_service::{SUL_DB, SUL_DB_ASYNC};
use aios_core::types::*;
use aios_core::{NamedAttrMap, RefU64};

///通过surql查询属性数据
pub async fn get_named_attmap(refno: RefU64) -> anyhow::Result<NamedAttrMap> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_attmap_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    let o: SurlValue = response.take(1).unwrap();
    let named_attmap: NamedAttrMap = o.into();
    Ok(named_attmap)
}
