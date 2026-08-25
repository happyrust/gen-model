use crate::fast_model::cata_cache::{
    CataLoadError, CataReadScope, LoadedScomInfo, active_read_scope, global_cache,
};
use crate::fast_model::query_gm_params_with_dependencies;
use aios_core::expression::query_cata::{query_axis_params, resolve_cata_comp};
use aios_core::expression::resolve::resolve_axis_param;
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::{PlinParam, ScomInfo};
use aios_core::{CataContext, NamedAttrMap, RefU64, RefnoEnum};
use anyhow::anyhow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Catalogue `GTYP=ANCI` describes an attachment/connection anchor. E3D does
/// not emit an independent solid for it, and the authoritative AMS catalogue
/// records therefore legitimately carry neither GMRE nor GSTR.
pub(crate) fn scom_is_intentionally_non_renderable(gtype: &str) -> bool {
    gtype.eq_ignore_ascii_case("ANCI")
}

/// Load one SCOM from the requested authority. Staged reads never enter the
/// committed resident cache.
pub async fn get_or_create_scom_info(
    scope: CataReadScope,
    cata_refno: RefnoEnum,
) -> Result<Arc<ScomInfo>, CataLoadError> {
    let caller_holds_geometry = crate::fast_model::concurrency::is_geometry_task();
    global_cache()
        .get_or_load(scope, cata_refno, move || async move {
            if caller_holds_geometry {
                load_scom_info(cata_refno).await
            } else {
                crate::fast_model::concurrency::run_geometry_shared(load_scom_info(cata_refno))
                    .await
            }
        })
        .await
}

async fn load_scom_info(cata_refno: RefnoEnum) -> Result<LoadedScomInfo, CataLoadError> {
    let result = async {
        let mut dependencies = vec![cata_refno];
        let attr_map = aios_core::get_named_attmap(cata_refno).await?;
        let type_noun = attr_map.get_type_str();
        let gtype = attr_map.get_as_string("GTYP").unwrap_or("unset".into());
        let ptref_name = match type_noun {
            "SPRF" => "PSTR",
            _ => "PTRE",
        };
        let mut axis_params = vec![];
        let mut axis_param_numbers = vec![];
        if let Some(ptre_refno) = attr_map.get_foreign_refno(ptref_name) {
            dependencies.push(ptre_refno);
            let axis_param_map = query_axis_params(ptre_refno).await?;
            dependencies.extend(axis_param_map.values().map(|axis| axis.refno));
            axis_params = axis_param_map.values().cloned().collect::<Vec<_>>();
            axis_param_numbers = axis_param_map.keys().cloned().collect::<Vec<_>>();
        }
        let gmse_refno = attr_map
            .get_foreign_refno("GMRE")
            .or_else(|| attr_map.get_foreign_refno("GSTR"))
            .unwrap_or_default();
        if !gmse_refno.is_valid() && !scom_is_intentionally_non_renderable(&gtype) {
            return Err(anyhow!("SCOM {cata_refno} 缺少有效 GMRE/GSTR"));
        }
        let gm_params = if gmse_refno.is_valid() {
            let (gm_params, gm_dependencies) =
                query_gm_params_with_dependencies(gmse_refno).await?;
            dependencies.extend(gm_dependencies);
            gm_params
        } else {
            Vec::new()
        };
        let mut ngm_params = vec![];
        //-ve， 和design发生左右的负实体
        if let Some(gmse_refno) = attr_map.get_foreign_refno("NGMR") {
            let (params, ngm_dependencies) = query_gm_params_with_dependencies(gmse_refno).await?;
            ngm_params = params;
            dependencies.extend(ngm_dependencies);
        }

        let mut plin_map = HashMap::new();
        if let Some(pstr_refno) = attr_map.get_foreign_refno("PSTR") {
            dependencies.push(pstr_refno);
            let pstr_am = aios_core::get_children_named_attmaps(pstr_refno).await?;
            for a in pstr_am {
                dependencies.push(a.get_refno_or_default());
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
            gtype,
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
        dependencies.sort_unstable();
        dependencies.dedup();
        let estimated_bytes = serde_json::to_vec(&scom_info)?.len() as u64;
        Ok(LoadedScomInfo {
            info: Arc::new(scom_info),
            dependencies: dependencies.into(),
            estimated_bytes,
        })
    }
    .await;
    result.map_err(|error: anyhow::Error| {
        let message = format!("{error:#}");
        if message.contains("缺少有效 GMRE/GSTR") || message.contains("catalogue geometry") {
            CataLoadError::CatalogueDefect(message.into())
        } else {
            CataLoadError::Database(message.into())
        }
    })
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
    let scom = get_or_create_scom_info(active_read_scope(), scom_refno).await?;
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
    fn only_ancillary_catalogue_type_may_omit_geometry_refs() {
        assert!(scom_is_intentionally_non_renderable("ANCI"));
        assert!(scom_is_intentionally_non_renderable("anci"));
        assert!(!scom_is_intentionally_non_renderable("TUBE"));
        assert!(!scom_is_intentionally_non_renderable("EQUI"));
    }

    /// BEND 24384/22456 uses this SCOM.  The on-demand CATA path persists the
    /// authoritative GMRE attribute but intentionally has no legacy `->GMRE`
    /// graph edge, so catalogue resolution must not depend on that edge.
    #[tokio::test]
    #[ignore = "requires the configured AvevaMarineSample SurrealDB"]
    async fn scom_geometry_resolves_from_stored_reference_attributes() {
        std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
        aios_core::init_surreal().await.unwrap();

        let info = get_or_create_scom_info(active_read_scope(), "13244_56726".into())
            .await
            .unwrap();
        assert!(
            !info.gm_params.is_empty(),
            "SCOM.GMRE did not resolve geometry"
        );
    }

    #[tokio::test]
    #[ignore = "requires the AMS7997 RocksDB fixture"]
    async fn ams7997_rrbend_resolves_all_visible_catalogue_primitives() {
        use aios_core::prim_geo::category::convert_to_brep_shapes;

        std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
        aios_core::init_surreal().await.unwrap();

        let scom = get_or_create_scom_info(active_read_scope(), "15194_2111".into())
            .await
            .expect("load /RRBEND SCOM");
        println!("RRBEND_SCOM={scom:#?}");
        assert_eq!(
            scom.gm_params.len(),
            7,
            "RRBEND has seven renderable GMSE children after level/type filtering"
        );

        let resolved = resolve_desi_comp("24381_100848".into(), None)
            .await
            .expect("resolve AMS7997 BEND");
        println!("RRBEND_RESOLVED={resolved:#?}");
        assert!(
            !resolved.geometries.is_empty(),
            "a renderable RRBEND must not resolve to an empty geometry set"
        );
        let converted = resolved
            .geometries
            .iter()
            .filter_map(convert_to_brep_shapes)
            .collect::<Vec<_>>();
        println!(
            "RRBEND_CONVERTED=resolved:{} converted:{} refs:{:?}",
            resolved.geometries.len(),
            converted.len(),
            converted
                .iter()
                .map(|shape| shape.refno)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            converted.len(),
            resolved.geometries.len(),
            "every resolved RRBEND primitive must convert to a BREP shape"
        );
        assert_eq!(converted.len(), 7, "RRBEND must produce seven BREP shapes");
    }

    #[tokio::test]
    #[ignore = "requires the AMS7997 RocksDB fixture"]
    async fn ams7997_trns_reports_authoritative_scylinder_axes() {
        std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
        aios_core::init_surreal().await.unwrap();

        for root in ["24381_100864", "24381_100872"] {
            let resolved = resolve_desi_comp(root.into(), None)
                .await
                .unwrap_or_else(|err| panic!("resolve AMS7997 TRNS {root}: {err:#}"));
            println!("TRNS_RESOLVED root={root} value={resolved:#?}");
            let cylinders = resolved
                .geometries
                .iter()
                .filter_map(|geometry| match geometry {
                    aios_core::parsed_data::geo_params_data::CateGeoParam::SCylinder(param) => {
                        Some(param)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            println!("TRNS_SCYLINDERS root={root} cylinders={cylinders:#?}");
            assert_eq!(
                cylinders.len(),
                3,
                "TRNS must resolve three SCYL primitives"
            );
            assert!(
                cylinders.iter().all(|cylinder| cylinder.axis.is_some()),
                "every TRNS SCYL must retain its resolved source axis"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires the AMS7997 RocksDB fixture"]
    async fn ams7997_threeway_reports_resolved_catalogue_composition() {
        use aios_core::prim_geo::category::convert_to_brep_shapes;

        std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
        aios_core::init_surreal().await.unwrap();
        let resolved = resolve_desi_comp("24381_100890".into(), None)
            .await
            .expect("resolve AMS7997 THREEWAY");
        println!("THREEWAY_RESOLVED={resolved:#?}");
        let converted = resolved
            .geometries
            .iter()
            .map(|geometry| {
                let converted = convert_to_brep_shapes(geometry);
                println!("THREEWAY_CONVERT source={geometry:#?} converted={converted:#?}");
                converted
            })
            .collect::<Vec<_>>();
        assert_eq!(
            resolved.geometries.len(),
            11,
            "THREEWAY source primitive count"
        );
        assert!(
            converted.iter().all(Option::is_some),
            "every THREEWAY source primitive must have an explicit conversion result"
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
            let info = get_or_create_scom_info(active_read_scope(), refno.into())
                .await
                .unwrap();
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

    let scom_info = get_or_create_scom_info(active_read_scope(), scom_ref).await?;
    // #[cfg(debug_assertions)]
    // dbg!(&scom_info);
    let context = aios_core::get_or_create_cata_context(desi_refno, is_tubi).await?;

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
