use std::collections::BTreeMap;

use aios_core::{AttrMap, NamedAttrValue};
use aios_core::*;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::pdms_data::{AxisParam, GmParam, ScomInfo};
use aios_core::pdms_types::*;

use crate::cata::resolve::{resolve_axis_params, resolve_gms};
use crate::data_interface::interface::PdmsDataInterface;

///查询 Axis 参数
pub async fn query_axis_params(refno: RefU64) -> anyhow::Result<BTreeMap<i32, AxisParam>> {
    // 查找ptse
    let mut map = BTreeMap::new();
    // dbg!(refno);
    let children = aios_core::get_children_named_attmaps(refno).await?;

    for child in children {
        //plin不在收集范围
        if child.get_type_str() == "PLIN" {
            continue;
        }
        // dbg!(&child);
        let number = child.get_i32("NUMB").unwrap_or(-1);
        if let Some(axis) = get_axis_param(&child) {
            map.entry(number).or_insert(axis);
        }
    }
    Ok(map)
}

///查询gmse的参数
pub async fn query_gm_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: Option<&T>,
) -> anyhow::Result<Vec<GmParam>> {
    let interface = interface.ok_or(anyhow::anyhow!("unknown interface"))?;
    let mut gms = vec![];
    let refno = attr_map.get_refno().unwrap_or_default();
    let mut children = vec![];
    for c in aios_core::get_children_named_attmaps(refno).await? {
        if TOTAL_CATA_GEO_NOUN_NAMES.contains(&c.get_type_str()) {
            children.push(c.clone());
        } else {
            for cc in aios_core::get_children_named_attmaps(c.get_refno_or_default()).await? {
                if TOTAL_CATA_GEO_NOUN_NAMES.contains(&cc.get_type_str()) {
                    children.push(cc.clone());
                }
            }
        }
    }
    for geo_am in children {
        if !geo_am.is_visible_by_level(None).unwrap_or(true) {
            continue;
        }
        // dbg!(&geo_am);
        let is_spro = geo_am.get_type_str() == "SPRO"; //todo add other types
        gms.push(
            query_gm_param(&geo_am, is_spro)
                .await
                .unwrap_or_default(),
        );
    }
    Ok(gms)
}

///对元件库的SCOM Element进行求值计算
pub fn resolve_cata_comp(
    des_att: &NamedAttrMap,
    scom_info: &ScomInfo,
    context: Option<CataContext>,
) -> anyhow::Result<CateGeomsInfo> {
    let des_refno = des_att.get_refno().unwrap_or_default();
    let mut cur_context = context.unwrap_or_default();
    let cat_ref = scom_info.attr_map.get_refno().unwrap_or_default();

    let axis_param_map = resolve_axis_params(des_refno, scom_info, &cur_context);
    let jusl_param = if let Some(plin) = cur_context.get("JUSL") {
        if scom_info.plin_map.contains_key(plin.as_str()) {
            Some(scom_info.plin_map.get(plin.as_str()).unwrap().clone())
        } else if scom_info.plin_map.contains_key("NA") {
            Some(scom_info.plin_map.get("NA").unwrap().clone())
        } else {
            None
        }
    } else {
        None
    };
    let geometries = resolve_gms(
        des_refno,
        &scom_info.gm_params,
        &jusl_param,
        &cur_context,
        &axis_param_map,
    );
    
    let n_geometries = resolve_gms(
        des_refno,
        &scom_info.ngm_params,
        &jusl_param,
        &cur_context,
        &axis_param_map,
    );

    #[cfg(debug_assertions)]
    if get_db_option().debug_root_refnos.is_some(){
        // dbg!(&geometries);
        // dbg!(&n_geometries);
    }

    Ok(CateGeomsInfo {
        refno: cat_ref,
        geometries,
        n_geometries,
        axis_map: axis_param_map,
    })
}

///获得AxisParam
pub fn get_axis_param(attr_map: &NamedAttrMap) -> Option<AxisParam> {
    let type_name = attr_map.get_as_string("TYPE").unwrap_or_default();
    let pconnect = attr_map.get_as_string("PCON").unwrap_or_default();
    let pbore = attr_map.get_as_string("PBOR").unwrap_or_default();
    let pwidth = attr_map.get_as_string("PWID").unwrap_or_default();
    let pheight = attr_map.get_as_string("PHEI").unwrap_or_default();
    let refno = attr_map.get_refno()?;
    let number = attr_map.get_i32("NUMB").unwrap_or_default();
    let r = match type_name.as_ref() {
        "PTAX" => AxisParam {
            refno,
            type_name,
            number,
            x: "".into(),
            y: "".into(),
            z: "".into(),
            distance: attr_map.get_as_string("PDIS")?,
            direction: attr_map.get_as_string("PAXI")?,
            ref_direction: attr_map.get_as_string("PZAXI").unwrap_or_default(),
            pconnect,
            pbore,
            pwidth,
            pheight,
            pnt_index_str: None,
        },
        "PTCA" => AxisParam {
            refno,
            type_name,
            number,
            x: attr_map.get_as_string("PX")?,
            y: attr_map.get_as_string("PY")?,
            z: attr_map.get_as_string("PZ")?,
            distance: "".into(),
            direction: { attr_map.get_as_string("PTCD").unwrap_or("Y".into()) },
            ref_direction: attr_map.get_as_string("PZAXI").unwrap_or_default(),
            pconnect,
            pbore,
            pwidth,
            pheight,
            pnt_index_str: None,
        },
        "PTMI" => AxisParam {
            refno,
            type_name,
            number,
            x: attr_map.get_as_string("PX")?,
            y: attr_map.get_as_string("PY")?,
            z: attr_map.get_as_string("PZ")?,
            distance: "".into(),
            direction: attr_map.get_as_string("PAXI")?,
            ref_direction: attr_map.get_as_string("PZAXI").unwrap_or_default(),
            pconnect,
            pbore,
            pwidth,
            pheight,
            pnt_index_str: None,
        },
        "PTPOS" => {
            AxisParam {
                //todo need fix " TPOS OF CREF"   " TDIR OF CREF"
                refno,
                type_name,
                number,
                x: "".into(),
                y: "".into(),
                z: "".into(),
                distance: attr_map.get_as_string("PTCP").unwrap_or("0".into()),
                direction: attr_map.get_as_string("PTCD").unwrap_or("Y".into()),
                ref_direction: attr_map.get_as_string("PZAXI").unwrap_or_default(),
                pconnect,
                pbore,
                pwidth,
                pheight,
                pnt_index_str: attr_map.get_as_string("PTCPOS"),
            }
        }
        _ => AxisParam {
            refno,
            type_name,
            number,
            x: "".into(),
            y: "".into(),
            z: "".into(),
            distance: "".into(),
            direction: "".into(),
            ref_direction: "".into(),
            pconnect,
            pbore,
            pwidth,
            pheight,
            pnt_index_str: None,
        },
    };
    Some(r)
}

///获得gmse的params
pub async fn query_gm_param(
    a: &NamedAttrMap,
    is_spro: bool,
) -> Option<GmParam> {
    // dbg!(a);
    let mut paxises = a.get_attr_strings_without_default(&["PAXI", "PAAX", "PBAX", "PCAX"]);
    if let Some(val) = a.get_val("PTS") {
        match val {
            NamedAttrValue::IntArrayType(v) => {
                for s in v {
                    paxises.push(s.to_string().into());
                }
            }
            _ => {}
        }
    }
    if let Some(v) = a.get_as_string("PLAX") {
        paxises.push((v));
    }
    let centre_line_flag = a.get_bool("CLFL").unwrap_or(false);
    let tube_flag = a.get_bool("TUFL").unwrap_or(false);
    let mut verts = vec![];
    let mut frads = vec![];
    let mut dxy = vec![];
    let refno = a.get_refno().unwrap_or_default();
    let type_name = a.get_type_str();
    if type_name == "SEXT" || type_name == "NSEX" || type_name == "SREV" || type_name == "NSRE" {
        //先暂时不考虑负实体
        let children = aios_core::get_children_named_attmaps(refno).await.ok()?;
        for child in children {
            if let Some(r) = child.get_refno()
                && child.get_type_str() == "SLOO"
            {
                let vert_atts = aios_core::get_children_named_attmaps(r)
                    .await
                    .unwrap_or_default();
                // dbg!(&vert_atts);
                for a in vert_atts {
                    verts.push([
                        (a.get_as_string("PX").unwrap_or_default()),
                        (a.get_as_string("PY").unwrap_or_default()),
                        (a.get_as_string("PZ").unwrap_or_default()),
                    ]);
                    frads.push((a.get_as_string("PRAD").unwrap_or_default()));
                }
            }
        }
    } else {
        let cur_type = aios_core::get_type_name(refno).await.unwrap_or_default();
        if is_spro && cur_type.as_str() == "SPRO" {
            for a in aios_core::get_children_named_attmaps(refno)
                .await
                .ok()
                .unwrap_or_default()
            {
                verts.push([
                    (a.get_as_string("PX").unwrap_or_default()),
                    (a.get_as_string("PY").unwrap_or_default()),
                    (a.get_as_string("PZ").unwrap_or_default()),
                ]);
                frads.push((a.get_as_string("PRAD").unwrap_or_default()));
                dxy.push([
                    (a.get_as_string("DX").unwrap_or_default()),
                    (a.get_as_string("DY").unwrap_or_default()),
                ]);
            }
        } else {
            verts.push([
                (a.get_as_string("PX").unwrap_or_default()),
                (a.get_as_string("PY").unwrap_or_default()),
                (a.get_as_string("PZ").unwrap_or_default()),
            ]);
            frads.push((a.get_as_string("PRAD").unwrap_or_default()));
            dxy.push([
                (a.get_as_string("DX").unwrap_or_default()),
                (a.get_as_string("DY").unwrap_or_default()),
            ]);
        }
    }

    Some(GmParam {
        refno: a.get_refno().unwrap_or_default(),
        gm_type: a.get_type_str().to_owned(),
        prad: (a.get_as_string("PRAD").unwrap_or_default()),
        pang: (a.get_as_string("PANG").unwrap_or_default()),
        pwid: (a.get_as_string("PWID").unwrap_or_default()),
        diameters: a.get_attr_strings(&["PDIA", "PBDM", "PTDM", "DIAM"]),
        distances: a.get_attr_strings(&["PDIS", "PBDI", "PTDI"]),
        shears: a.get_attr_strings(&["PXTS", "PYTS", "PXBS", "PYBS"]),
        phei: (a.get_as_string("PHEI").unwrap_or_default()),
        offset: (a.get_as_string("POFF").unwrap_or_default()),
        lengths: a.get_attr_strings(&["PXLE", "PYLE", "PZLE"]),
        xyz: a.get_attr_strings(&[
            "PX", "PY", "PZ", "PBBT", "PCBT", "PBTP", "PCTP", "PBOF", "PCOF",
        ]),
        verts,
        frads,
        dxy,
        drad: (a.get_as_string("DRAD").unwrap_or_default()),
        dwid: (a.get_as_string("DWID").unwrap_or_default()),
        paxises, // 先pa_axis, 后pb_axis
        centre_line_flag,
        visible_flag: tube_flag,
    })
}
