// 实用工具函数
//
// 从旧 gen_model.rs 迁移的辅助函数

use anyhow::Result;
use aios_core::RefnoEnum;
use aios_core::prim_geo::tubing::TubiSize;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use crate::fast_model::resolve_desi_comp;

/// 检查是否启用 E3D 调试模式
#[allow(dead_code)]
pub fn is_e3d_debug_enabled() -> bool {
    #[cfg(feature = "debug_e3d")]
    {
        false // TODO: 需要从原来的 E3D_DEBUG_ENABLED 获取
    }
    #[cfg(not(feature = "debug_e3d"))]
    {
        false
    }
}

/// 检查是否启用 E3D info 模式
#[allow(dead_code)]
pub fn is_e3d_info_enabled() -> bool {
    #[cfg(feature = "debug_e3d")]
    {
        false // TODO: 需要从原来的 E3D_INFO_ENABLED 获取
    }
    #[cfg(not(feature = "debug_e3d"))]
    {
        false
    }
}

/// 检查是否启用 E3D trace 模式
#[allow(dead_code)]
pub fn is_e3d_trace_enabled() -> bool {
    #[cfg(feature = "debug_e3d")]
    {
        false // TODO: 需要从原来的 E3D_TRACE_ENABLED 获取
    }
    #[cfg(not(feature = "debug_e3d"))]
    {
        false
    }
}

/// 查询 Tubi 尺寸
///
/// 从旧 gen_model.rs 迁移，用于 cata_model
pub async fn query_tubi_size(
    refno: RefnoEnum,
    tubi_cat_ref: RefnoEnum,
    is_hang: bool,
) -> Result<TubiSize> {
    let tubi_geoms_info = resolve_desi_comp(refno, Some(tubi_cat_ref))
        .await
        .unwrap_or_default();

    // 从几何参数查询尺寸
    for geom in &tubi_geoms_info.geometries {
        if let BoxImplied(d) = geom {
            return Ok(TubiSize::BoxSize((d.height, d.width)));
        } else if let TubeImplied(d) = geom {
            return Ok(TubiSize::BoreSize(d.diameter));
        }
    }

    // 从属性映射查询
    if let Ok(cat_att) = aios_core::get_named_attmap(tubi_cat_ref).await {
        let params = cat_att.get_f32_vec("PARA").unwrap_or_default();
        if params.len() >= 2 {
            let tubi_bore = params[if is_hang { 0 } else { 1 }] as f32;
            return Ok(TubiSize::BoreSize(tubi_bore));
        }
    }

    Ok(TubiSize::None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_query_tubi_size_none() {
        // 测试不存在的 refno 返回 None
        let result = query_tubi_size(
            RefnoEnum::RefU64(999999),
            RefnoEnum::RefU64(999999),
            false,
        )
        .await;

        assert!(result.is_ok());
        if let Ok(size) = result {
            assert!(matches!(size, TubiSize::None));
        }
    }
}
