use bevy_transform::components::Transform;
use parry3d::bounding_volume::*;
use parry3d::math::*;
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
