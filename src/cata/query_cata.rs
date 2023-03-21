use dashmap::DashMap;
use std::collections::{BTreeMap, HashMap};
use aios_core::parsed_data::GeomsInfo;
use aios_core::pdms_data::{AxisParam, GmParam, PlinParam, ScomInfo};
use aios_core::pdms_types::{AttrMap, RefU64};
use aios_core::pdms_types::AttrVal::IntArrayType;
use anyhow::anyhow;
use dashmap::mapref::one::Ref;
use log::{error, info};
use sled::pin;
use smol_str::SmolStr;
use crate::cata::resolve::{resolve_axis_params, resolve_gms};
use crate::data_interface::interface::PdmsDataInterface;
use crate::defines::CACHED_SCOM_INFO_MAP;

pub const DDHEIGHT_STR: &'static str = "DDHEIGHT";
pub const DDRADIUS_STR: &'static str = "DDRADIUS";
pub const DDANGLE_STR: &'static str = "DDANGLE";


///求解design component
pub async fn resolve_desi_comp<T: PdmsDataInterface>(
    refno: RefU64,
    mut scom_ref: Option<RefU64>,
    interface: &T,
    is_debug: bool,
) -> anyhow::Result<GeomsInfo> {
    let desi_att = interface.get_attr(refno).await?;
    if scom_ref.is_none() {
        if let Some(catref) = desi_att.get_foreign_refno("CATR") {
            let c_att = interface.get_attr(catref).await?;
            if c_att.contains_attr_name("CATR") {
                scom_ref = c_att.get_foreign_refno("CATR");
            } else {
                scom_ref = Some(catref);
            }
        } else {
            let spre_ref = desi_att.get_foreign_refno("SPRE").unwrap_or_default();
            let spre = interface.get_attr(spre_ref).await?;
            if spre.contains_attr_name("CATR") {
                scom_ref = spre.get_foreign_refno("CATR");
            } else {
                // SFIT 的 scom 和 spre 是同一个
                scom_ref = Some(spre_ref);
            }
        }
    }

    // if is_debug {

    // }
    let scom_ref = scom_ref.ok_or(anyhow!(format!("SCOM not exist in element: {}", refno.to_refno_str())))?;
    if !scom_ref.is_valid() {
        return Err(anyhow!("Scom ref is invalid".to_string()));
    }

    //缓存备用
    if !CACHED_SCOM_INFO_MAP.contains_key(&scom_ref) {
        if let Ok(mut scom_info) = query_scom_info(scom_ref, interface, is_debug).await {
            CACHED_SCOM_INFO_MAP.insert(scom_ref, &scom_info).unwrap();
        } else {
            let error_info = format!("元件库: {} 解析出错", scom_ref.to_refno_string());
            println!("{}", &error_info);
            return Err(anyhow!(error_info));
        };
    }
    let scom_info = CACHED_SCOM_INFO_MAP.get(&scom_ref).unwrap();
    // if is_debug {
    //     dbg!(&scom_info.value());
    // }
    let mut context: BTreeMap<SmolStr, SmolStr> = BTreeMap::new();
    if let Some(v) = desi_att.get_as_string("JUSL") {
        context.insert("JUSL".into(), v.into());
    }
    context.insert("DESI_REFNO".into(), refno.to_refno_str());
    let mut desp = desi_att.get_f64_vec("DESP").unwrap_or_default();
    for i in 0..desp.len() {
        context.insert(
            format!("DESI{}", i + 1).into(),
            desp[i].to_string().into(),
        );
        context.insert(
            format!("DDES{}", i + 1).into(),
            desp[i].to_string().into(),
        );
        context.insert(
            format!("DESP{}", i + 1).into(),
            desp[i].to_string().into(),
        );
    }
    let height = desi_att.get_as_string("HEIG").unwrap_or("0.0".into());
    context.insert(DDHEIGHT_STR.into(), SmolStr::new(height.clone()));
    context.insert("HEIG".into(), SmolStr::new(height));
    let angle = desi_att.get_as_string("ANGL").unwrap_or("0.0".into());
    context.insert(DDANGLE_STR.into(), SmolStr::new(angle.clone()));
    context.insert("ANGL".into(), SmolStr::new(angle));
    let radi = desi_att.get_as_string("RADI").unwrap_or("0.0".into());
    context.insert(DDRADIUS_STR.into(), SmolStr::new(radi.clone()));
    context.insert("RADI".into(), SmolStr::new(radi));
    let geom_info = resolve_cata_comp(scom_info.value(), interface, Some(context), is_debug).await;
    if geom_info.is_err() {
        error!("{:?}",geom_info.as_ref().err());
        error!("{:?}",desi_att.to_string_hashmap());
    }
    if refno == RefU64::from_refno_str("23584/5398").unwrap() {
        dbg!(geom_info.as_ref().unwrap().geometries.clone());
    }
    geom_info
}


///整合SCOM对应的临时数据
pub async fn query_scom_info<T: PdmsDataInterface>(
    refno: RefU64,
    interface: &T,
    is_debug: bool,
) -> anyhow::Result<ScomInfo> {
    let attr_map = interface.get_attr(refno).await.unwrap();
    let type_noun = attr_map.get_type_cloned().ok_or(anyhow!(format!("{:?} Scom att not correct", refno)))?;
    let is_sprf = type_noun == "SPRF";
    let ptref_name = if is_sprf { "PSTR" } else { "PTRE" };
    let mut axis_params = vec![];
    let mut axis_param_numbers = vec![];
    if let Some(ptre_refno) = attr_map.get_foreign_refno(ptref_name) {
        if let Ok(ptre_am) = interface.get_attr(ptre_refno).await {
            if let Ok(axis_param_map) = query_axis_params(&ptre_am, interface, is_debug).await {
                // if is_debug {
                //     dbg!(&axis_param_map);
                // }
                axis_params = axis_param_map.values().cloned().collect::<Vec<_>>();
                axis_param_numbers = axis_param_map.keys().cloned().collect::<Vec<_>>();
            }
        }
    }
    let gmref_name = if is_sprf { "GSTR" } else { "GMRE" };
    let mut gm_params = vec![];
    if let Some(gmse_refno) = attr_map.get_foreign_refno(gmref_name) {
        let gmse_am = interface.get_attr(gmse_refno).await?;
        gm_params = query_gm_params(&gmse_am, interface).await?;
        if is_debug {
            dbg!(&gm_params);
        }
    }

    let mut plin_map = HashMap::new();
    if let Some(pstr_refno) = attr_map.get_foreign_refno("PSTR") {
        let pstr_am = interface.get_children_attrs(pstr_refno).await?;
        for a in pstr_am {
            if let Some(k) = a.get_as_string("PKEY") {
                plin_map.insert(
                    k,
                    PlinParam {
                        vxy: [a.get_as_string("PX").unwrap_or("0".to_string()), a.get_as_string("PY").unwrap_or("0".to_string())],
                        dxy: [a.get_as_string("DX").unwrap_or("0".to_string()), a.get_as_string("DY").unwrap_or("0".to_string())],
                        plax: a.get_as_string("PLAX").unwrap_or("unset".to_string()),
                    },
                );
            }
        }
    }
    Ok(ScomInfo {
        gtype: SmolStr::new(attr_map.get_as_string("GTYP").unwrap_or("unset".into())),
        dtse_params: vec![],
        gm_params,
        axis_params,
        params: attr_map
            .get_as_string("PARA")
            .unwrap_or_default()
            .replace("\n", " ")
            .replace("  ", " ").into(),
        axis_param_numbers,
        attr_map,
        plin_map,
    })
}

///查询 Axis 参数
pub async fn query_axis_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: &T,
    is_debug: bool,
) -> anyhow::Result<BTreeMap<i32, AxisParam>> {
    // 查找ptse
    let mut map = BTreeMap::new();
    let refno = attr_map.get_refno().unwrap_or_default();
    let children = interface.get_children_attrs(refno).await.unwrap();

    for child in children {
        let number = child.get_i32("NUMB").unwrap_or(-1);
        if is_debug {
            // dbg!(&child);
        }
        if let Some(axis) = get_axis_param(&child) {
            map.entry(number).or_insert(axis);
        }
    }
    Ok(map)
}

///查询gmse的参数
pub async fn query_gm_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: &T,
) -> anyhow::Result<Vec<GmParam>> {
    let mut gms = vec![];
    let refno = attr_map.get_refno().unwrap_or_default();
    // let children = interface.get_children_attrs(refno).await?;
    let children = interface.get_travel_children_attrs(refno).await?;
    for child in children {
        //暂时把 Level 的判断加到这里
        if !child.is_visible_by_level(None).unwrap_or(true) {
            continue;
        }
        let has_children = child.get_type_cloned().unwrap_or_default() == "SPRO";//todo add other types
        gms.push(query_gm_param(&child, interface, has_children).await.unwrap_or_default());
    }
    Ok(gms)
}


///对元件库的SCOM Element进行求值计算
pub async fn resolve_cata_comp<T: PdmsDataInterface>(
    scom_info: &ScomInfo,
    interface: &T,
    context: Option<BTreeMap<SmolStr, SmolStr>>,
    is_debug: bool,
) -> anyhow::Result<GeomsInfo> {
    let mut cur_context = context.unwrap_or_default();
    //默认值
    cur_context
        .entry(DDHEIGHT_STR.into())
        .or_insert("0.0".into());
    cur_context
        .entry(DDRADIUS_STR.into())
        .or_insert("0.0".into());
    cur_context
        .entry(DDANGLE_STR.into())
        .or_insert("0.0".into());
    //获取DTSE的expression
    process_dtse_params(&scom_info.attr_map, interface, &mut cur_context).await;

    //保温层厚度
    cur_context.insert("IPARA0".into(), "0".into());
    cur_context.insert("IPARA".into(), "0".into());
    //PARA
    // dbg!(scom_info.attr_map.to_string_hashmap());
    let params = scom_info.attr_map.get_f64_vec("PARA").unwrap_or_default();
    for i in 0..params.len() {
        cur_context.insert(format!("OPAR{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("APAR{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("CPAR{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("PARA{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("PARAM{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("IPARA{}", i + 1).into(), "0".to_string().into());
        cur_context.insert(format!("IPAR{}", i + 1).into(), "0".to_string().into());
    }
    let axis_map = resolve_axis_params(scom_info, &cur_context);
    // if is_debug {
    //     dbg!(&cur_context);
    //     dbg!(&axis_map);
    // }
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
    let geometries = resolve_gms(&scom_info.gm_params, &jusl_param, &cur_context, &axis_map);
    Ok(GeomsInfo {
        geometries,
        axis_map,
    })
}

///获得AxisParam
pub fn get_axis_param(attr_map: &AttrMap) -> Option<AxisParam> {
    let type_name = attr_map.get_as_smol_str("TYPE")?;
    let pconnect = attr_map.get_as_smol_str("PCON")?;
    let pbore = attr_map.get_as_smol_str("PBOR")?;
    let refno = attr_map.get_refno()?;
    let number = attr_map.get_i32("NUMB")?;
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
            pconnect,
            pbore,
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
            direction: {
                attr_map.get_as_smol_str("PTCD").unwrap_or("Y".into())
            },
            pconnect,
            pbore,
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
            pconnect,
            pbore,
            pnt_index_str: None,
        },
        "PTPOS" => {
            AxisParam {   //todo need fix " TPOS OF CREF"   " TDIR OF CREF"
                refno,
                type_name,
                number,
                x: "".into(),
                y: "".into(),
                z: "".into(),
                distance: attr_map.get_as_smol_str("PTCP").unwrap_or("0".into()),
                direction: attr_map.get_as_smol_str("PTCD").unwrap_or("Y".into()),
                pconnect,
                pbore,
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
            pconnect,
            pbore,
            pnt_index_str: None,
        },
    };
    Some(r)
}

///获得gmse的params
pub async fn query_gm_param(a: &AttrMap, interface: &dyn PdmsDataInterface, has_chidren: bool) -> Option<GmParam> {
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
        paxises.push(SmolStr::new(v));
    }
    let centre_line_flag = a.get_bool("CLFL").unwrap_or(false);
    let tube_flag = a.get_bool("TUFL").unwrap_or(false);
    let mut verts = vec![];
    let mut frads = vec![];
    let mut dxy = vec![];
    let refno = a.get_refno().unwrap_or_default();
    let type_name = a.get_type();
    if type_name == "SEXT" || type_name == "SREV" {
        //先暂时不考虑负实体
        let children = interface.get_children_attrs(refno).await.ok()?;
        for child in children {
            if let Some(r) = child.get_refno() && child.get_type() == "SLOO" {
                for a in interface.get_children_attrs(r).await.unwrap_or_default() {
                    verts.push([SmolStr::new(a.get_as_string("PX").unwrap_or_default()),
                        SmolStr::new(a.get_as_string("PY").unwrap_or_default()),
                        SmolStr::new(a.get_as_string("PZ").unwrap_or_default())
                    ]);
                    frads.push(SmolStr::new(a.get_as_string("PRAD").unwrap_or_default()));
                }
            }
        }
    } else {
        if has_chidren {
            for a in interface.get_children_attrs(refno).await.ok()? {
                verts.push([SmolStr::new(a.get_as_string("PX").unwrap_or_default())
                    , SmolStr::new(a.get_as_string("PY").unwrap_or_default()),
                    SmolStr::new(a.get_as_string("PZ").unwrap_or_default())
                ]);
                frads.push(SmolStr::new(a.get_as_string("PRAD").unwrap_or_default()));
                dxy.push([SmolStr::new(a.get_as_string("DX").unwrap_or_default()), SmolStr::new(a.get_as_string("DY").unwrap_or_default())]);
            }
        } else {
            verts.push([SmolStr::new(a.get_as_string("PX").unwrap_or_default()),
                SmolStr::new(a.get_as_string("PY").unwrap_or_default()),
                SmolStr::new(a.get_as_string("PZ").unwrap_or_default())
            ]);
            frads.push(SmolStr::new(a.get_as_string("PRAD").unwrap_or_default()));
            dxy.push([SmolStr::new(a.get_as_string("DX").unwrap_or_default()),
                SmolStr::new(a.get_as_string("DY").unwrap_or_default())]);
        }
    }

    Some(GmParam {
        refno: a.get_refno().unwrap_or_default(),
        gm_type: a.get_type_cloned().unwrap_or_default(),
        prad: SmolStr::new(a.get_as_string("PRAD").unwrap_or_default()),
        pang: SmolStr::new(a.get_as_string("PANG").unwrap_or_default()),
        pwid: SmolStr::new(a.get_as_string("PWID").unwrap_or_default()),
        diameters: a.get_attr_strings_without_default(&["PDIA", "PBDM", "PTDM", "DIAM"]),
        distances: a.get_attr_strings(&["PDIS", "PBDI", "PTDI"]),
        shears: a.get_attr_strings(&["PXTS", "PYTS", "PXBS", "PYBS"]),
        phei: SmolStr::new(a.get_as_string("PHEI").unwrap_or_default()),
        offset: SmolStr::new(a.get_as_string("POFF").unwrap_or_default()),
        box_lengths: a.get_attr_strings(&["PXLE", "PYLE", "PZLE"]),
        xyz: a.get_attr_strings(&["PX", "PY", "PZ", "PBBT", "PCBT", "PBTP", "PCTP", "PBOF", "PCOF"]),
        verts,
        frads,
        dxy,
        drad: SmolStr::new(a.get_as_string("DRAD").unwrap_or_default()),
        dwid: SmolStr::new(a.get_as_string("DWID").unwrap_or_default()),
        paxises, // 先pa_axis, 后pb_axis
        centre_line_flag,
        visible_flag: tube_flag,
    })
}

///获得dtse的参数信息
pub async fn process_dtse_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: &T,
    context: &mut BTreeMap<SmolStr, SmolStr>,
) -> Option<bool> {
    let dtre_refno = attr_map.get_foreign_refno("DTRE")?;
    let children = interface.get_children_attrs(dtre_refno).await.unwrap_or_default();
    for child in children {
        let key = SmolStr::new(format!("RPRO_{}", child.get_as_string("DKEY")?));
        let exp = SmolStr::new(child.get_as_string("PPRO")?);
        let default_key = format!("{}_default_expr", key);
        let default_expr = SmolStr::new(child.get_as_string("DPRO")?);
        context.insert(key, exp);
        context.insert(default_key.into(), default_expr);
    }
    Some(true)
}
