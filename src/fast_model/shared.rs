use bevy_transform::components::Transform;
use parry3d::math::*;
use parry3d::bounding_volume::*;
///针对aabb，应用transform
#[inline]
pub fn aabb_apply_transform(aabb: &Aabb, t: &Transform) -> Aabb {
    let a = aabb.scaled(&t.scale.into());
    let transformed_aabb = a.transform_by(&Isometry {
        rotation: t.rotation.into(),
        translation: t.translation.into(),
    });
    transformed_aabb
}
