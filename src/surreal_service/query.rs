use crate::surreal_service::{SUL_DB, SUL_DB_ASYNC};
use aios_core::types::*;
use aios_core::{NamedAttrMap, RefU64};
use aios_core::orm::pdms_element;

///通过surql查询pe数据
pub async fn get_pe(refno: RefU64) -> anyhow::Result<Option<pdms_element::Model>> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_pe_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    // dbg!(&response);
    let pe: Option<pdms_element::Model> = response.take(0)?;
    // dbg!(&pe);
    Ok(pe)
}

///通过surql查询属性数据
pub async fn get_named_attmap(refno: RefU64) -> anyhow::Result<NamedAttrMap> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_attmap_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    let o: SurlValue = response.take(0)?;
    let named_attmap: NamedAttrMap = o.into();
    Ok(named_attmap)
}

pub async fn get_children_named_attmaps(refno: RefU64) -> anyhow::Result<Vec<NamedAttrMap>> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_children_attmap_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    let o: SurlValue = response.take(0)?;
    let os: Vec<SurlValue> = o.try_into().unwrap();
    // dbg!(os.len());
    let named_attmap: Vec<NamedAttrMap> = os.into_iter().map(|x| x.into()).collect();
    Ok(named_attmap)
}


///获得children
pub async fn get_children_refnos(refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_children_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    // dbg!(&response);
    let refnos: Vec<RefU64> = response.take::<Vec<String>>(0)?
        .into_iter().map(|s| s.as_str().into()).collect();
    Ok(refnos)
}
