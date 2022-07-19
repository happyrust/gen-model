use dashmap::DashMap;
use std::collections::{BTreeMap, HashMap};
use aios_core::parsed_data::GeomsInfo;
use aios_core::pdms_data::{AxisParam, GmParam, ScomInfo};
use aios_core::pdms_types::{AttrMap, RefU64};
use aios_core::pdms_types::AttrVal::IntArrayType;
use anyhow::anyhow;
use log::{error, info};
use smol_str::SmolStr;
use crate::cata::resolve::{resolve_axis_params, resolve_gms};
use crate::data_interface::interface::PdmsDataInterface;

pub const DDHEIGHT_STR: &'static str = "DDHEIGHT";
pub const DDRADIUS_STR: &'static str = "DDRADIUS";
pub const DDANGLE_STR: &'static str = "DDANGLE";


///求解design component
pub async fn resolve_desi_comp<T: PdmsDataInterface>(
    refno: RefU64,
    interface: &T,
    is_debug: bool,
) -> anyhow::Result<GeomsInfo> {
    let attr_map = interface.get_attr(refno).await?;
    let mut scom_ref = None;
    if let Some(catref) = attr_map.get_foreign_refno("CATR") {
        let c_att = interface.get_attr(catref).await?;
        if c_att.contains_attr_name("CATR") {
            scom_ref = c_att.get_foreign_refno("CATR");
        } else {
            scom_ref = Some(catref);
        }
    } else {
        let spre_ref = attr_map.get_foreign_refno("SPRE").unwrap_or_default();
        let spre = interface.get_attr(spre_ref).await?;
        if spre.contains_attr_name("CATR") {
            scom_ref = spre.get_foreign_refno("CATR");
        }
    };
    let scom_ref = scom_ref.ok_or(anyhow!(format!("SCOM not exist in element: {}", refno.to_refno_str())))?;
    if !scom_ref.is_valid() {
        return Err(anyhow!("Scom ref is invalid".to_string()));
    }
    let scom_info = query_scom_info(scom_ref, interface).await?;
    if is_debug {
        dbg!(&scom_info);
    }
    let mut context: BTreeMap<SmolStr, SmolStr> = BTreeMap::new();
    context.insert("DESI_REFNO".into(), refno.to_refno_str());
    let mut desp = attr_map.get_f64_vec("DESP").unwrap_or_default();
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
    let height = attr_map.get_as_string("HEIG").unwrap_or("0.0".into());
    context.insert(DDHEIGHT_STR.into(), SmolStr::new(height.clone()));
    context.insert("HEIG".into(), SmolStr::new(height));
    let angle = attr_map.get_as_string("ANGL").unwrap_or("0.0".into());
    context.insert(DDANGLE_STR.into(), SmolStr::new(angle.clone()));
    context.insert("ANGL".into(), SmolStr::new(angle));
    let radi = attr_map.get_as_string("RADI").unwrap_or("0.0".into());
    context.insert(DDRADIUS_STR.into(), SmolStr::new(radi.clone()));
    context.insert("RADI".into(), SmolStr::new(radi));
    let mut geom_info = resolve_cata_comp(&scom_info, interface, Some(context), is_debug).await;
    if geom_info.is_err() {
        error!("{:?}",geom_info.as_ref().err());
        error!("{:?}",attr_map.to_string_hashmap());
    }
    geom_info
}


///整合SCOM对应的临时数据
pub async fn query_scom_info<T: PdmsDataInterface>(
    refno: RefU64,
    interface: &T,
) -> anyhow::Result<ScomInfo> {
    let attr_map = interface.get_attr(refno).await?;
    let type_noun = attr_map.get_type_cloned().ok_or(anyhow!("Scom att not correct".to_string()))?;
    let is_sprf = type_noun == "SPRF";
    let ptref_name = if is_sprf { "PSTR" } else { "PTRE" };
    let mut axis_params = vec![];
    let mut axis_param_numbers = vec![];
    if let Some(ptre_refno) = attr_map.get_foreign_refno(ptref_name) {
        if let Ok(ptre_am) = interface.get_attr(ptre_refno).await {
            let axis_param_map = query_axis_params(&ptre_am, interface).await?;
            axis_params = axis_param_map.values().cloned().collect::<Vec<_>>();
            axis_param_numbers = axis_param_map.keys().cloned().collect::<Vec<_>>();
        }
        // if ptre_refno.to_refno_str() == "15192/77158" {
        //     dbg!(&axis_params);
        // }
    }
    let gmref_name = if is_sprf { "GSTR" } else { "GMRE" };
    let mut gm_params = vec![];
    if let Some(gmse_refno) = attr_map.get_foreign_refno(gmref_name) {
        let gmse_am = interface.get_attr(gmse_refno).await?;
        gm_params = query_gm_params(&gmse_am, interface).await?;
    } else {
        //没有geometry 的引用
        // dbg!(attr_map.to_string_hashmap());
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
    })
}

///查询 Axis 参数
pub async fn query_axis_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: &T,
) -> anyhow::Result<BTreeMap<i32, AxisParam>> {
    // 查找ptse
    let mut map = BTreeMap::new();
    let refno = attr_map.get_refno().unwrap_or_default();
    let children = interface.get_children_attrs(refno).await?;

    for child in children {
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
    interface: &T,
) -> anyhow::Result<Vec<GmParam>> {
    let mut gms = vec![];
    let refno = attr_map.get_refno().unwrap_or_default();
    let children = interface.get_children_attrs(refno).await?;
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
    let params = scom_info.attr_map.get_f64_vec("PARA").unwrap_or_default();
    for i in 0..params.len() {
        cur_context.insert(format!("OPAR{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("APAR{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("CPAR{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("PARA{}", i + 1).into(), params[i].to_string().into());
        cur_context.insert(format!("IPARA{}", i + 1).into(), "0".to_string().into());
        cur_context.insert(format!("IPAR{}", i + 1).into(), "0".to_string().into());
    }

    let axis_map = resolve_axis_params(scom_info, &cur_context);
    if is_debug {
        dbg!(&cur_context);
        dbg!(&axis_map);
    }
    let geometries = resolve_gms(&scom_info.gm_params, &cur_context, &axis_map);
    if is_debug {
        dbg!(&geometries);
    }
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
pub async fn query_gm_param(att_map: &AttrMap, interface: &dyn PdmsDataInterface, has_chidren: bool) -> Option<GmParam> {
    let mut paxises = att_map.get_attr_strings_without_default(&["PAXI", "PAAX", "PBAX", "PCAX"]);
    if let Some(val) = att_map.get_val("PTS") {
        match val {
            IntArrayType(v) => {
                for s in v {
                    paxises.push(s.to_string().into());
                }
            }
            _ => {}
        }
    }
    if let Some(v) = att_map.get_as_string("PLAX") {
        paxises.push(SmolStr::new(v));
    }
    let centre_line_flag = att_map.get_bool("CLFL").unwrap_or(false);
    let tube_flag = att_map.get_bool("TUFL").unwrap_or(false);
    let mut verts = vec![];
    let mut prads = vec![];
    let mut dxy = vec![];
    let refno = att_map.get_refno().unwrap_or_default();
    let type_name = att_map.get_type();
    if type_name == "SEXT" || type_name == "SREV" {
        //先暂时不考虑负实体
        let children = interface.get_children_attrs(refno).await.ok()?;
        for child in children {
            if let Some(r) = child.get_refno() && child.get_type() == "SLOO" {
                for a in interface.get_children_attrs(r).await.unwrap_or_default() {
                    verts.push([SmolStr::new(a.get_as_string("PX").unwrap_or_default()),
                        SmolStr::new(a.get_as_string("PY").unwrap_or_default())]);
                    prads.push(SmolStr::new(a.get_as_string("PRAD").unwrap_or_default()));
                }
            }
        }
    } else {
        if has_chidren {
            for a in interface.get_children_attrs(refno).await.ok()? {
                verts.push([SmolStr::new(a.get_as_string("PX").unwrap_or_default()), SmolStr::new(a.get_as_string("PY").unwrap_or_default())]);
                dxy.push([SmolStr::new(a.get_as_string("DX").unwrap_or_default()), SmolStr::new(a.get_as_string("DY").unwrap_or_default())]);
            }
        } else {
            verts.push([SmolStr::new(att_map.get_as_string("PX").unwrap_or_default()),
                SmolStr::new(att_map.get_as_string("PY").unwrap_or_default())]);
            dxy.push([SmolStr::new(att_map.get_as_string("DX").unwrap_or_default()), SmolStr::new(att_map.get_as_string("DY").unwrap_or_default())]);
        }
    }

    Some(GmParam {
        refno: att_map.get_refno().unwrap_or_default(),
        gm_type: att_map.get_type_cloned().unwrap_or_default(),
        prad: SmolStr::new(att_map.get_as_string("PRAD").unwrap_or_default()),
        pang: SmolStr::new(att_map.get_as_string("PANG").unwrap_or_default()),
        pwid: SmolStr::new(att_map.get_as_string("PWID").unwrap_or_default()),
        diameters: att_map.get_attr_strings_without_default(&["PDIA", "PBDM", "PTDM", "DIAM"]),
        distances: att_map.get_attr_strings(&["PDIS", "PBDI", "PTDI"]),
        shears: att_map.get_attr_strings(&["PXTS", "PYTS", "PXBS", "PYBS"]),
        phei: SmolStr::new(att_map.get_as_string("PHEI").unwrap_or_default()),
        offset: SmolStr::new(att_map.get_as_string("POFF").unwrap_or_default()),
        box_lengths: att_map.get_attr_strings(&["PXLE", "PYLE", "PZLE"]),
        xyz: att_map.get_attr_strings(&["PX", "PY", "PZ", "PBBT", "PCBT", "PBTP", "PCTP", "PBOF", "PCOF"]),
        verts,
        prads,
        dxy,
        drad: SmolStr::new(att_map.get_as_string("DRAD").unwrap_or_default()),
        dwid: SmolStr::new(att_map.get_as_string("DWID").unwrap_or_default()),
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
