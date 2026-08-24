use crate::fast_model::query_gm_params;
use aios_core::expression::query_cata::{query_axis_params, resolve_cata_comp};
use aios_core::expression::resolve::{SCOM_INFO_MAP, resolve_axis_param};
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::{PlinParam, ScomInfo};
use aios_core::{CataContext, NamedAttrMap, RefU64, RefnoEnum};
use anyhow::anyhow;
use std::collections::{BTreeMap, HashMap};

fn publish_scom_info(cata_refno: RefnoEnum, scom_info: ScomInfo) -> ScomInfo {
    SCOM_INFO_MAP.insert(cata_refno, scom_info.clone());
    scom_info
}

///收集SCOM的信息, 暂时慎用缓存
pub async fn get_or_create_scom_info(cata_refno: RefnoEnum) -> anyhow::Result<ScomInfo> {
    let scom_info = if let Some(info) = SCOM_INFO_MAP.get(&cata_refno) {
        info.value().clone()
    } else {
        let attr_map = aios_core::get_named_attmap(cata_refno).await?;
        let type_noun = attr_map.get_type_str();
        let ptref_name = match type_noun {
            "SPRF" => "PSTR",
            _ => "PTRE",
        };
        let mut axis_params = vec![];
        let mut axis_param_numbers = vec![];
        if let Some(ptre_refno) = attr_map.get_foreign_refno(ptref_name) {
            if let Ok(axis_param_map) = query_axis_params(ptre_refno).await {
                axis_params = axis_param_map.values().cloned().collect::<Vec<_>>();
                axis_param_numbers = axis_param_map.keys().cloned().collect::<Vec<_>>();
            }
        }
        let gmse_refno =
            aios_core::query_single_by_paths(cata_refno, &["->GMRE", "->GSTR"], &["REFNO"])
                .await
                .map(|x| x.get_refno_or_default())?;
        // #[cfg(debug_assertions)]
        // dbg!(gmse_refno);
        let gm_params = query_gm_params(gmse_refno).await?;
        let mut ngm_params = vec![];
        //-ve， 和design发生左右的负实体
        if let Some(gmse_refno) = attr_map.get_foreign_refno("NGMR") {
            ngm_params = query_gm_params(gmse_refno).await?;
        }

        let mut plin_map = HashMap::new();
        if let Some(pstr_refno) = attr_map.get_foreign_refno("PSTR") {
            let pstr_am = aios_core::get_children_named_attmaps(pstr_refno).await?;
            for a in pstr_am {
                if let Some(k) = a.get_as_string("PKEY") {
                    plin_map.insert(
                        k,
                        PlinParam {
                            vxy: [
                                a.get_as_string("PX").unwrap_or("0".to_string()),
                                a.get_as_string("PY").unwrap_or("0".to_string()),
                            ],
                            dxy: [
                                a.get_as_string("DX").unwrap_or("0".to_string()),
                                a.get_as_string("DY").unwrap_or("0".to_string()),
                            ],
                            plax: a.get_as_string("PLAX").unwrap_or("unset".to_string()),
                        },
                    );
                }
            }
        }
        let scom_info = ScomInfo {
            gtype: attr_map.get_as_string("GTYP").unwrap_or("unset".into()),
            dtse_params: vec![],
            gm_params,
            ngm_params,
            axis_params,
            params: attr_map
                .get_as_string("PARA")
                .unwrap_or_default()
                .replace("\n", " ")
                .replace("  ", " ")
                .into(),
            axis_param_numbers,
            attr_map,
            plin_map,
        };
        // RocksDB remains authoritative. This cache holds only the parsed
        // catalogue representation used by model generation, so later pages
        // do not repeat GMRE/GSTR/NGMR traversal and expression parsing.
        publish_scom_info(cata_refno, scom_info)
    };
    Ok(scom_info)
}

/// 求解axis的数值
pub async fn resolve_axis_params(
    refno: RefnoEnum,
    context: Option<CataContext>,
) -> anyhow::Result<BTreeMap<i32, CateAxisParam>> {
    let mut map = BTreeMap::new();
    let scom_refno = aios_core::get_cat_refno(refno).await?.unwrap_or_default();
    if !scom_refno.is_valid() {
        return Ok(Default::default());
    }
    let scom = get_or_create_scom_info(scom_refno).await?;
    let context = context.unwrap_or(aios_core::get_or_create_cata_context(refno, false).await?);
    for i in 0..scom.axis_params.len() {
        let axis = resolve_axis_param(&scom.axis_params[i], &scom, &context);
        map.insert(scom.axis_param_numbers[i], axis);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_scom_is_published_for_later_pages() {
        let refno: RefnoEnum = "1_987654".into();
        SCOM_INFO_MAP.remove(&refno);
        let mut expected = ScomInfo::default();
        expected.gtype = "CACHE_SENTINEL".into();

        publish_scom_info(refno, expected);

        assert_eq!(
            SCOM_INFO_MAP.get(&refno).map(|value| value.gtype.clone()),
            Some("CACHE_SENTINEL".into())
        );
        SCOM_INFO_MAP.remove(&refno);
    }

    /// BEND 24384/22456 uses this SCOM.  The on-demand CATA path persists the
    /// authoritative GMRE attribute but intentionally has no legacy `->GMRE`
    /// graph edge, so catalogue resolution must not depend on that edge.
    #[tokio::test]
    #[ignore = "requires the configured AvevaMarineSample SurrealDB"]
    async fn scom_geometry_resolves_from_stored_reference_attributes() {
        std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
        aios_core::init_surreal().await.unwrap();

        let info = get_or_create_scom_info("13244_56726".into()).await.unwrap();
        assert!(
            !info.gm_params.is_empty(),
            "SCOM.GMRE did not resolve geometry"
        );
    }

    /// 目录把几何挂在哪一层，这套 ACP1000 电缆槽里有两种形态，两种都得解得出来。
    ///
    /// 槽体 `/ACP1000-TFVL` 的 CATE 与 SCOM 指向同一个几何集；带角度的弯头
    /// （TBL60/TBR60/TBL90/TBR90）的 CATE 上 `GMRE` 是 `pe:0_0`，几何只挂在
    /// SCOM 上。2026-08-05 的 RVM 对拍量到这两种形态的结果天差地别：槽只差一个
    /// 导出口径，角度弯头的包围盒体积却与 E3D 基准差 67–97 倍。差异出在哪一步
    /// 尚未定论，但「解析不出来」这一步必须先排除掉，否则每次都要重查一遍。
    #[tokio::test]
    #[ignore = "requires the configured AvevaMarineSample SurrealDB"]
    async fn both_catalogue_shapes_resolve_geometry_from_the_scom() {
        std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
        aios_core::init_surreal().await.unwrap();

        // (SCOM refno, 名称, 该 SCOM 的几何集在库里的子节点数)
        let cases = [
            ("13244_51903", "/ACP1000-TFVL-100", 18),
            ("13244_56726", "/ACP1000-TBR-100", 28),
            ("13244_55889", "/ACP1000-TBR60-100X150", 37),
            ("13244_57306", "/ACP1000-TBL60-100X150", 37),
            ("13244_55598", "/ACP1000-TBR90-100X150", 37),
            ("13244_57013", "/ACP1000-TBL90-100X150", 37),
        ];

        for (refno, name, gmse_children) in cases {
            let info = get_or_create_scom_info(refno.into()).await.unwrap();
            println!(
                "{name:<24} gtype={:<5} gm_params={:<3} (几何集子节点 {gmse_children})",
                info.gtype,
                info.gm_params.len()
            );
            assert!(
                !info.gm_params.is_empty(),
                "{name} 的几何没解出来：几何集有 {gmse_children} 个子节点"
            );
        }
    }
}

///求解design component
pub async fn resolve_desi_comp(
    desi_refno: RefnoEnum,
    mut tubi_scom: Option<RefnoEnum>,
) -> anyhow::Result<CateGeomsInfo> {
    let desi_att = aios_core::get_named_attmap(desi_refno).await?;
    let is_tubi = tubi_scom.is_some();

    // #[cfg(debug_assertions)]
    // if is_tubi {
    //     dbg!(tubi_scom);
    // }

    let scom_ref = if let Some(scom) = tubi_scom {
        scom
    } else {
        let scom = aios_core::get_cat_refno(desi_refno)
            .await?
            .ok_or(anyhow::anyhow!(format!(
                "CAT引用不存在: {}",
                desi_refno.to_string()
            )))?;
        scom
    };
    // #[cfg(debug_assertions)]
    // if is_tubi {
    //     dbg!(scom_ref);
    // }

    let scom_info = get_or_create_scom_info(scom_ref).await?;
    // #[cfg(debug_assertions)]
    // dbg!(&scom_info);
    let context = aios_core::get_or_create_cata_context(desi_refno, is_tubi)
        .await
        .unwrap();

    let geom_info = resolve_cata_comp(&desi_att, &scom_info, Some(context));
    // dbg!(&geom_info);
    geom_info.map_err(|_| anyhow!("resolve_cata_comp failed"))
}

/// Resolve a design component from page-prefetched attributes and catalogue
/// identity. This avoids repeating the two hottest point reads while keeping
/// the catalogue-expression semantics identical to [`resolve_desi_comp`].
pub async fn resolve_desi_comp_prefetched(
    desi_att: &NamedAttrMap,
    scom_info: &ScomInfo,
    context: CataContext,
) -> anyhow::Result<CateGeomsInfo> {
    resolve_cata_comp(desi_att, scom_info, Some(context))
        .map_err(|_| anyhow!("resolve_cata_comp failed"))
}
