use aios_core::pdms_types::RefU64;
use aios_core::virtual_hole::{CircleHoleSize, HoleBlockageMethod, HoleSize, RectHoleSize};
use crate::api::attr::{query_explicit_attr, query_refno_uda_value};
use crate::api::ssc_data::get_ancestor_till_type;
use crate::aql_api::children::{query_ancestor_till_types_aql, query_children_order_aql};
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::data_center_api::data_api::get_refno_desp;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 计算fitt这种元件库为负实体的孔洞的尺寸
/// refno ： fitt等的参考号
pub async fn get_virtual_hole_size(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<HoleSize>> {
    let database = aios_mgr.get_arango_db().await?;
    // 找到catr中的ngmr
    let ngmr_refno = query_foreign_refno_aql( &database,refno, &vec!["SPRE", "NGMR"]).await?;
    if ngmr_refno.is_none() { return Ok(None); };
    // 找到ngmr下的所有负实体，只需要第一个，孔洞默认只有方形和圆形两种，代表只能有一个负实体
    let ngmr_children = query_children_order_aql(&database, ngmr_refno.unwrap()).await?;
    if ngmr_children.is_empty() || ngmr_children.len() > 1 { return Ok(None); }
    // 获取负实体的尺寸
    let desp = get_refno_desp(ngmr_children[0].refno, aios_mgr).await?;
    match ngmr_children[0].noun.as_str() {
        "NBOX" => {
            if desp.len() < 2 {
                Ok(None)
            } else {
                Ok(Some(HoleSize::Rect(RectHoleSize {
                    length: desp[0] as f32,
                    width: desp[1] as f32,
                })))
            }
        }
        "NLCY" => {
            if desp.is_empty() {
                Ok(None)
            } else {
                Ok(Some(HoleSize::Circle(CircleHoleSize {
                    radius: desp[0] as f32,
                })))
            }
        }
        _ => { Ok(None) }
    }
}

/// 计算fitt这种元件库为负实体的孔洞的体积
/// refno ： fitt等的参考号
/// hole_size: get_virtual_hole_size() 通过desp获取对应形状孔洞的尺寸
/// 该方法只需要计算所依赖的墙或者板的宽度即可
pub async fn get_virtual_hole_volume(refno: RefU64, hole_site: &HoleSize, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<f32>> {
    Ok(None)
}

/// 根据封堵方式和水淹高度获取封堵材料
/// flooded_height:水淹高度，通过水淹高度插件计算
pub async fn get_hole_blockage_method(refno: RefU64, flooded_height: f32, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<HoleBlockageMethod>> {
    let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(refno).await else { return Ok(None); };
    let database = aios_mgr.get_arango_db().await?;
    let Some(blockage_material) = query_refno_uda_value(refno, "JGOBHNOTE", &pool).await? else { return Ok(None); };
    let blockage_material = blockage_material.string_value();
    match blockage_material.as_str() {
        "AFW" => {
            if flooded_height > 2000.0 {
                // 获取所属墙的厚度
                let Some(wall_refno) = query_ancestor_till_types_aql(&database, refno,
                                                                     vec!["SWALL", "GWALL", "WALL", "PANE", "FLOOR"]).await? else { return Ok(None); };
                let thickness = get_wall_thickness(wall_refno.refno, aios_mgr).await?;
                Ok(Some(HoleBlockageMethod {
                    method: "⾼密硅酮封堵".to_string(),
                    thickness,
                }))
            } else {
                Ok(Some(HoleBlockageMethod {
                    method: "⾼低密硅酮封堵".to_string(),
                    thickness: 200.0,
                }))
            }
        }
        "AFWB" => {
            // 获取所属墙的厚度
            let Some(wall_refno) = query_ancestor_till_types_aql(&database, refno,
                                                                 vec!["SWALL", "GWALL", "WALL", "PANE", "FLOOR"]).await? else { return Ok(None); };
            let thickness = get_wall_thickness(wall_refno.refno, aios_mgr).await?;
            Ok(Some(HoleBlockageMethod {
                method: "⾼密硅酮封堵".to_string(),
                thickness,
            }))
        }
        "MCT+AFW" => {
            Ok(Some(HoleBlockageMethod {
                method: "低密硅酮材料".to_string(),
                thickness: 200.0,
            }))
        }
        _ => { Ok(None) }
    }
}

/// 返回墙的厚度
/// 部分孔洞高度需要依赖墙的厚度
pub async fn get_wall_thickness(wall_refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<f32> {
    Ok(0.0)
}