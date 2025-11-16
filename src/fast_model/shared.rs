use aios_core::RefnoEnum;
use aios_core::types::NamedAttrMap;
use bevy_transform::components::Transform;
use parry3d::bounding_volume::*;
use parry3d::math::*;

/// 从 NamedAttrMap 中获取 owner 信息
///
/// # 参数
/// * `attr` - 元素属性映射
///
/// # 返回
/// (owner_refno, owner_type) 元组
pub async fn get_owner_info_from_attr(attr: &NamedAttrMap) -> (RefnoEnum, String) {
    let owner_refno = attr.get_owner();
    // 检查 RefnoEnum 是否为默认值（表示没有 owner）
    if owner_refno != RefnoEnum::default() {
        if let Ok(Some(owner_pe)) = aios_core::get_pe(owner_refno).await {
            let owner_type_str = owner_pe.get_type_str();
            return (owner_refno, owner_type_str.to_string());
        }
    }
    (RefnoEnum::default(), String::new())
}

///针对aabb，应用transform
/// 针对aabb，应用transform
///
/// # 参数
///
/// * `aabb` - 输入的AABB包围盒
/// * `t` - Transform变换组件
///
/// # 返回
///
/// 变换后的AABB包围盒
#[inline]
pub fn aabb_apply_transform(aabb: &Aabb, t: &Transform) -> Aabb {
    let a = aabb.scaled(&t.scale.into());
    let transformed_aabb = a.transform_by(&Isometry {
        rotation: t.rotation.into(),
        translation: t.translation.into(),
    });
    transformed_aabb
}
