use crate::fast_model::query_gm_params;
use aios_core::expression::query_cata::{query_axis_params, resolve_cata_comp};
use aios_core::expression::resolve::{SCOM_INFO_MAP, resolve_axis_param};
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::{PlinParam, ScomInfo};
use aios_core::{CataContext, RefU64, RefnoEnum};
use anyhow::anyhow;
use std::collections::{BTreeMap, HashMap};

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
        ScomInfo {
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
        }
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
