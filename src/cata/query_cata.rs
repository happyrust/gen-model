use crate::cata::resolve::{resolve_axis_params, resolve_gms};
use crate::data_interface::interface::PdmsDataInterface;
// use crate::defines::CACHED_SCOM_INFO_MAP;
use crate::cata::consts::{DDANGLE_STR, DDHEIGHT_STR, DDRADIUS_STR};
use aios_core::data_center::AttrValue;
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::{AxisParam, GmParam, PlinParam, ScomInfo};
use aios_core::pdms_types::AttrVal::IntArrayType;
use aios_core::pdms_types::{
    AttrMap, AttrVal, RefU64, TOTAL_CATA_GEO_NOUN_NAMES, TOTAL_GEO_NOUN_NAMES,
};
use aios_core::tool::db_tool::db1_dehash;
use anyhow::anyhow;
use dashmap::mapref::one::Ref;
use dashmap::DashMap;
use glam::Vec3;
use log::{error, info};
use sled::pin;
use std::collections::{BTreeMap, HashMap};
use tokio::sync::RwLock;

use super::resolve::CataContext;

///求解design component
pub fn resolve_desi_comp<T: PdmsDataInterface>(
    interface: Option<&T>,
    desi_refno: RefU64,
    mut scom_ref_option: Option<RefU64>,
    // scom_info_map: &RwLock<HashMap<RefU64, ScomInfo>>,
    //传入额外的参数进来，用于解析轴线参数
    desi_axis_map: Option<&BTreeMap<i32, CateAxisParam>>,
) -> anyhow::Result<CateGeomsInfo> {
    let interface = interface.ok_or(anyhow::anyhow!("unknown interface"))?;
    let desi_att = interface.get_attr_from_localdb(desi_refno)?;
    //todo 改到使用图数据库去查找
    if scom_ref_option.is_none() {
        scom_ref_option = interface.get_cat_ref(desi_refno);
    }
    // dbg!(scom_ref);
    let scom_ref = scom_ref_option.ok_or(anyhow::anyhow!(format!(
        "SCOM not exist in element: {}",
        desi_refno.to_refno_str()
    )))?;
    if !scom_ref.is_valid() {
        println!(
            "{} 的CAT引用不存在，为 {}",
            desi_refno.to_refno_str(),
            scom_ref.to_refno_str()
        );
        return Ok(Default::default());
    }
    // dbg!(scom_ref);
    let scom_info = interface.get_or_create_scom_info(scom_ref)?;
    // dbg!(&scom_info.gm_params);
    // dbg!(&scom_info.axis_params);
    let mut context = interface.get_or_create_cata_context(desi_refno, desi_axis_map)?;
    
    let geom_info = resolve_cata_comp(&desi_att, &scom_info, Some(interface), Some(context));
    // dbg!(&geom_info.as_ref().unwrap().n_geometries);
    if geom_info.is_err() {
        error!("{:?}", geom_info.as_ref().err());
        error!("{:?}", desi_att.to_string_hashmap());
    }
    geom_info
}


///查询 Axis 参数
pub fn query_axis_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: Option<&T>,
) -> anyhow::Result<BTreeMap<i32, AxisParam>> {
    // 查找ptse
    let interface = interface.ok_or(anyhow::anyhow!("unknown interface"))?;
    let mut map = BTreeMap::new();
    let refno = attr_map.get_refno().unwrap_or_default();
    let children = interface.get_children_attrs(refno)?;

    for child in children {
        let number = child.get_i32("NUMB").unwrap_or(-1);
        if let Some(axis) = get_axis_param(&child) {
            map.entry(number).or_insert(axis);
        }
    }
    Ok(map)
}

///查询gmse的参数
pub fn query_gm_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: Option<&T>,
) -> anyhow::Result<Vec<GmParam>> {
    let interface = interface.ok_or(anyhow::anyhow!("unknown interface"))?;
    let mut gms = vec![];
    let refno = attr_map.get_refno().unwrap_or_default();
    let mut children = vec![];
    for c in interface.get_children_attrs(refno)? {
        if TOTAL_CATA_GEO_NOUN_NAMES.contains(&c.get_type()) {
            children.push(c.clone());
        } else {
            for cc in interface.get_children_attrs(c.get_refno().unwrap_or_default())? {
                if TOTAL_CATA_GEO_NOUN_NAMES.contains(&cc.get_type()) {
                    children.push(cc.clone());
                }
            }
        }
    }
    for geo_am in children {
        if !geo_am.is_visible_by_level(None).unwrap_or(true) {
            continue;
        }
        dbg!(&geo_am);
        let is_spro = geo_am.get_type() == "SPRO"; //todo add other types
        gms.push(query_gm_param(&geo_am, interface, is_spro).unwrap_or_default());
    }
    Ok(gms)
}

///对元件库的SCOM Element进行求值计算
pub fn resolve_cata_comp<T: PdmsDataInterface>(
    des_att: &AttrMap,
    scom_info: &ScomInfo,
    interface: Option<&T>,
    context: Option<CataContext>,
) -> anyhow::Result<CateGeomsInfo> {
    let interface = interface.ok_or(anyhow::anyhow!("unknown interface"))?;
    let des_refno = des_att.get_refno().unwrap_or_default();
    let mut cur_context = context.unwrap_or_default();
    let cat_ref = scom_info.attr_map.get_refno().unwrap_or_default();

    let axis_map = resolve_axis_params(des_refno, scom_info, &cur_context, interface);
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
        &axis_map,
        Some(interface),
    );
    let n_geometries = resolve_gms(
        des_refno,
        &scom_info.ngm_params,
        &jusl_param,
        &cur_context,
        &axis_map,
        Some(interface),
    );
    Ok(CateGeomsInfo {
        refno: cat_ref,
        geometries,
        n_geometries,
        axis_map,
    })
}

///获得AxisParam
pub fn get_axis_param(attr_map: &AttrMap) -> Option<AxisParam> {
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
            distance: attr_map.get_as_smol_str("PDIS")?,
            direction: attr_map.get_as_smol_str("PAXI")?,
            ref_direction: attr_map.get_as_smol_str("PZAXI").unwrap_or_default(),
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
            x: attr_map.get_as_smol_str("PX")?,
            y: attr_map.get_as_smol_str("PY")?,
            z: attr_map.get_as_smol_str("PZ")?,
            distance: "".into(),
            direction: { attr_map.get_as_smol_str("PTCD").unwrap_or("Y".into()) },
            ref_direction: attr_map.get_as_smol_str("PZAXI").unwrap_or_default(),
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
            x: attr_map.get_as_smol_str("PX")?,
            y: attr_map.get_as_smol_str("PY")?,
            z: attr_map.get_as_smol_str("PZ")?,
            distance: "".into(),
            direction: attr_map.get_as_smol_str("PAXI")?,
            ref_direction: attr_map.get_as_smol_str("PZAXI").unwrap_or_default(),
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
                distance: attr_map.get_as_smol_str("PTCP").unwrap_or("0".into()),
                direction: attr_map.get_as_smol_str("PTCD").unwrap_or("Y".into()),
                ref_direction: attr_map.get_as_smol_str("PZAXI").unwrap_or_default(),
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
pub fn query_gm_param(
    a: &AttrMap,
    interface: &dyn PdmsDataInterface,
    is_spro: bool,
) -> Option<GmParam> {
    let mut paxises = a.get_attr_strings_without_default(&["PAXI", "PAAX", "PBAX", "PCAX"]);
    if let Some(val) = a.get_val("PTS") {
        match val {
            IntArrayType(v) => {
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
    let type_name = a.get_type();
    if type_name == "SEXT" || type_name == "NSEX" || type_name == "SREV" || type_name == "NSRE" {
        //先暂时不考虑负实体
        let children = interface.get_children_attrs(refno).ok()?;
        for child in children {
            if let Some(r) = child.get_refno() && child.get_type() == "SLOO" {
                for a in interface.get_children_attrs(r).unwrap_or_default() {
                    verts.push([(a.get_as_string("PX").unwrap_or_default()),
                        (a.get_as_string("PY").unwrap_or_default()),
                        (a.get_as_string("PZ").unwrap_or_default())
                    ]);
                    frads.push((a.get_as_string("PRAD").unwrap_or_default()));
                }
            }
        }
    } else {
        let cur_type = interface.get_type_name(refno);
        if is_spro && cur_type.as_str() == "SPRO" {
            for a in interface.get_children_attrs(refno).ok().unwrap_or_default() {
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
        gm_type: a.get_type().to_owned(),
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

