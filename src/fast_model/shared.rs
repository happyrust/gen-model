use bevy_transform::components::Transform;
use parry3d::bounding_volume::*;
use parry3d::math::*;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Negative-geometry nouns from the decoded positive-equivalent dictionary
/// plus the established catalogue/geomset negatives that do not expose that
/// field in the offline snapshot.
pub fn negative_noun_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names = parse_pdms_db::dict::default_noun_capabilities()
            .positive_equivalents()
            .into_iter()
            .map(|(negative, _)| negative)
            .collect::<BTreeSet<_>>();
        names.extend(
            aios_core::pdms_types::TOTAL_NEG_NOUN_NAMES
                .iter()
                .map(|noun| (*noun).to_string()),
        );
        names.into_iter().collect()
    })
}

pub fn negative_noun_refs() -> Vec<&'static str> {
    negative_noun_names().iter().map(String::as_str).collect()
}

pub fn is_negative_noun(noun: &str) -> bool {
    let noun = noun.trim().to_ascii_uppercase();
    negative_noun_names()
        .iter()
        .any(|candidate| candidate == &noun)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_nouns_follow_positive_equivalent_dictionary() {
        for noun in [
            "NBOX", "NCON", "NCTO", "NCYL", "NDIS", "NPOLYH", "NPYR", "NREV", "NRTO", "NSLC",
            "NSNO", "NXTR", "NLCY", "NSBO", "NSCY", "NSCO", "NLSN", "NSSP", "NSCT", "NSRT", "NSDS",
            "NSSL", "NLPY", "NSEX", "NSRE",
        ] {
            assert!(is_negative_noun(noun), "{noun}");
        }
        assert!(!is_negative_noun("NOZZ"));
    }
}
