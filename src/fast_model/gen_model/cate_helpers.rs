// Cate 模型生成辅助函数
//
// 提供 SJUS 对齐计算、NGMR 查询等工具函数

use aios_core::consts::{CIVIL_TYPES, NGMR_OWN_TYPES};
use aios_core::{NamedAttrMap, RefnoEnum};
use anyhow::Result;
use num_enum::{IntoPrimitive, TryFromPrimitive};

/// NGMR 移除类型枚举
///
/// 定义负几何体（NGMR）应用到哪些元素上
#[derive(Debug, Default, IntoPrimitive, Eq, PartialEq, TryFromPrimitive, Copy, Clone)]
#[repr(i32)]
pub enum NgmrRemovedType {
    /// 默认模式（根据 owner 类型判断）
    #[default]
    AsDefault = -1,
    /// 不应用到任何元素
    Nothing = 0,
    /// 应用到附着元素（CREF）
    Attached = 1,
    /// 应用到拥有者（owner）
    Owner = 2,
    /// 应用到当前元素自身
    Item = 3,
    /// 应用到附着元素和拥有者
    AttachedAndOwner = 4,
    /// 应用到附着元素和当前元素
    AttachedAndItem = 5,
    /// 应用到拥有者和当前元素
    OwnerAndItem = 6,
    /// 应用到所有（附着、拥有者、当前元素）
    All = 7,
}

/// 计算 SJUS 对齐偏移值
///
/// # 参数
/// - `sjus`: 对齐方式字符串（如 "UTOP", "UCEN", "UBOT" 等）
/// - `height`: 元素高度
///
/// # 返回
/// Z 轴方向的偏移值
#[inline]
pub fn cal_sjus_value(sjus: &str, height: f32) -> f32 {
    let off_z = if sjus == "UTOP" || sjus == "DTOP" || sjus == "TOP" {
        height
    } else if sjus == "UCEN" || sjus == "DCEN" || sjus == "CENT" {
        height / 2.0
    } else {
        0.0
    };
    off_z
}

/// 查询 NGMR（负几何体）的目标所有者
///
/// # 参数
/// - `refno`: 元素引用号
/// - `ngmr_geo_refno`: NGMR 几何体引用号
///
/// # 返回
/// 应该应用负几何体的目标元素列表
pub async fn query_ngmr_owner(
    refno: RefnoEnum,
    ngmr_geo_refno: RefnoEnum,
) -> Result<Vec<RefnoEnum>> {
    let att = aios_core::get_named_attmap(refno).await.unwrap_or_default();
    let owner = att.get_owner();
    let c_ref = att.get_foreign_refno("CREF");

    let ance_result = aios_core::query_filter_ancestors(refno.clone(), &NGMR_OWN_TYPES).await?;
    let o_ref = ance_result.into_iter().next();

    let geo_att = aios_core::get_named_attmap(ngmr_geo_refno)
        .await
        .unwrap_or_default();

    let removed_type =
        NgmrRemovedType::try_from(geo_att.get_i32("NAPP").unwrap_or(-1)).unwrap_or_default();

    let mut target_refnos = vec![];

    match removed_type {
        NgmrRemovedType::AsDefault => {
            if let Some(o_refno) = o_ref {
                let o_type = aios_core::get_type_name(o_refno).await.unwrap_or_default();
                if CIVIL_TYPES.contains(&o_type.as_str()) {
                    target_refnos.push(o_refno);
                }
            }
        }
        NgmrRemovedType::Nothing => {}
        NgmrRemovedType::Attached => {
            c_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::Owner => {
            o_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::Item => target_refnos.push(refno),
        NgmrRemovedType::AttachedAndOwner => {
            c_ref.map(|x| target_refnos.push(x));
            o_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::AttachedAndItem => {
            c_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno)
        }
        NgmrRemovedType::OwnerAndItem => {
            o_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno)
        }
        NgmrRemovedType::All => {
            c_ref.map(|x| target_refnos.push(x));
            o_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno);
        }
    }

    Ok(target_refnos)
}
