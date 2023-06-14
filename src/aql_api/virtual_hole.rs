use aios_core::pdms_types::RefU64;
use aios_core::virtual_hole::{CircleHoleSize, HoleSize, RectHoleSize};
use crate::aql_api::children::query_children_order_aql;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::data_center_api::data_api::get_refno_desp;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 计算fitt这种元件库为负实体的孔洞的尺寸
/// refno ： fitt等的参考号
pub async fn get_virtual_hole_size(refno:RefU64,aios_mgr:&AiosDBManager) -> anyhow::Result<Option<HoleSize>> {
    let database = aios_mgr.get_arango_db().await?;
    // 找到catr中的ngmr
    let ngmr_refno = query_foreign_refno_aql(refno,&vec!["SPRE","NGMR"],&database).await?;
    if ngmr_refno.is_none() { return Ok(None) };
    // 找到ngmr下的所有负实体，只需要第一个，孔洞默认只有方形和圆形两种，代表只能有一个负实体
    let ngmr_children = query_children_order_aql(&database,ngmr_refno.unwrap()).await?;
    if ngmr_children.is_empty() || ngmr_children.len() > 1 { return Ok(None) }
    // 获取负实体的尺寸
    let desp = get_refno_desp(ngmr_children[0].refno,aios_mgr).await?;
    let size = match ngmr_children[0].noun.as_str() {
        "NBOX" => {
            if desp.len() < 2 {
                None
            } else {
                Some(HoleSize::Rect(RectHoleSize{
                length: desp[0] as f32,
                width: desp[1] as f32,
            })) }
        }
        "NLCY" => {
            if desp.is_empty() {
                None
            } else {
                Some(HoleSize::Circle(CircleHoleSize{
                    radius: desp[0] as f32,
                }))
            }
        }
        _ => { None }
    };
    Ok(size)
}