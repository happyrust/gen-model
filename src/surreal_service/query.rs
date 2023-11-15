use crate::surreal_service::{SUL_DB, SUL_DB_ASYNC};
use aios_core::types::*;
use aios_core::{NamedAttrMap, RefU64};
use aios_core::pe::SPdmsElement;
use surrealdb::sql::{Thing, Value};

///通过surql查询pe数据
pub async fn get_pe(refno: RefU64) -> anyhow::Result<Option<SPdmsElement>> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_pe_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    let pe: Option<SPdmsElement> = response.take(0)?;
    Ok(pe)
}

///查询到祖先节点列表
pub async fn get_ancestor(refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_ancestor_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    let s = response.take::<Vec<String>>(1)?;
    Ok(s.into_iter().map(|s| s.as_str().into()).collect())
}

pub async fn get_ancestor_attmaps(refno: RefU64) -> anyhow::Result<Vec<NamedAttrMap>> {
    let mut response = SUL_DB
        .query(include_str!(
            "../../schemas/query_ancestor_attmaps_by_refno.surql"
        ))
        .bind(("refno", refno.to_string()))
        .await?;
    let o: SurlValue = response.take(1)?;
    let os: Vec<SurlValue> = o.try_into().unwrap();
    let named_attmaps: Vec<NamedAttrMap> = os.into_iter().map(|x| x.into()).collect();
    Ok(named_attmaps)
}

pub async fn get_type_name(refno: RefU64) -> anyhow::Result<String> {
    let mut response = SUL_DB
        .query(r#"return (select value noun from only (type::thing("pe", $refno)));"#)
        .bind(("refno", refno.to_string()))
        .await?;
    let type_name: Option<String> = response.take(0)?;
    Ok(type_name.unwrap_or_default())
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

pub async fn get_cat_refno(refno: RefU64) -> anyhow::Result<Option<RefU64>> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_cata_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    let o: Option<String> = response.take(1)?;
    Ok(o.map(|x| x.into()))
}

pub async fn get_cat_attmap(refno: RefU64) -> anyhow::Result<NamedAttrMap> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_cata_attmap.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    let o: SurlValue = response.take(1)?;
    // dbg!(&o);
    let named_attmap: NamedAttrMap = o.into();
    Ok(named_attmap)
}

// pub async fn get_named_attmaps(refnos: &[RefU64]) -> anyhow::Result<Vec<NamedAttrMap>> {
//     let mut response = SUL_DB
//         .query(include_str!("../../schemas/query_attmap_by_refno.surql"))
//         .bind(("refno", refno.to_string()))
//         .await?;
//     let o: SurlValue = response.take(0)?;
//     let named_attmap: NamedAttrMap = o.into();
//     Ok(named_attmap)
// }

pub async fn get_children_named_attmaps(refno: RefU64) -> anyhow::Result<Vec<NamedAttrMap>> {
    let mut response = SUL_DB
        .query(include_str!(
            "../../schemas/query_children_attmap_by_refno.surql"
        ))
        .bind(("refno", refno.to_string()))
        .await?;
    let o: SurlValue = response.take(0)?;
    let os: Vec<SurlValue> = o.try_into().unwrap();
    let named_attmaps: Vec<NamedAttrMap> = os.into_iter().map(|x| x.into()).collect();
    Ok(named_attmaps)
}

///获得children
pub async fn get_children_refnos(refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    let mut response = SUL_DB
        .query(include_str!("../../schemas/query_children_by_refno.surql"))
        .bind(("refno", refno.to_string()))
        .await?;
    // dbg!(&response);
    let refnos: Vec<RefU64> = response
        .take::<Vec<String>>(0)?
        .into_iter()
        .map(|s| s.as_str().into())
        .collect();
    Ok(refnos)
}
