use aios_core::expression::query_cata::query_gm_param;
use aios_core::pdms_data::GmParam;
use aios_core::pdms_types::{PdmsGenericType, TOTAL_CATA_GEO_NOUN_NAMES};
use aios_core::{RefU64, RefnoEnum};
use std::str::FromStr;

pub async fn query_gm_params(refno: RefnoEnum) -> anyhow::Result<Vec<GmParam>> {
    query_gm_params_with_dependencies(refno)
        .await
        .map(|(params, _)| params)
}

/// Resolve a geometry set while retaining every catalogue node actually read.
pub async fn query_gm_params_with_dependencies(
    refno: RefnoEnum,
) -> anyhow::Result<(Vec<GmParam>, Vec<RefnoEnum>)> {
    let mut gms = vec![];
    let mut children = vec![];
    let mut dependencies = vec![refno];
    for c in aios_core::get_children_named_attmaps(refno).await? {
        dependencies.push(c.get_refno_or_default());
        if TOTAL_CATA_GEO_NOUN_NAMES.contains(&c.get_type_str()) {
            children.push(c.clone());
        }
        //有可能嵌套负实体
        for cc in aios_core::get_children_named_attmaps(c.get_refno_or_default()).await? {
            dependencies.push(cc.get_refno_or_default());
            if TOTAL_CATA_GEO_NOUN_NAMES.contains(&cc.get_type_str()) {
                children.push(cc);
            }
        }
    }
    // dbg!(&children);
    for geo_am in children {
        //todo visible 不应该在这里执行过滤
        //后续如果需要使用这些不同等级的模型，需要切换
        // dbg!(&geo_am);
        if !geo_am.is_visible_by_level(None).unwrap_or(true) {
            continue;
        }
        let is_spro = geo_am.get_type_str() == "SPRO"; //todo add other types
        let (geom, nested_dependencies) = query_gm_param(&geo_am, is_spro).await?;
        dependencies.extend(nested_dependencies);
        // dbg!(&geom);

        gms.push(geom);
    }
    dependencies.sort_unstable();
    dependencies.dedup();
    Ok((gms, dependencies))
}

#[inline]
pub async fn get_generic_type(refno: RefnoEnum) -> anyhow::Result<PdmsGenericType> {
    let types = aios_core::get_ancestor_types(refno).await?;
    for t in types {
        if let Ok(generic) = PdmsGenericType::from_str(&t) {
            return Ok(generic);
        }
    }
    Ok(PdmsGenericType::UNKOWN)
}
