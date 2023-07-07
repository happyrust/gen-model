use crate::cata::resolve::{resolve_axis_params, resolve_gms};
use crate::data_interface::interface::PdmsDataInterface;
// use crate::defines::CACHED_SCOM_INFO_MAP;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::pdms_data::{AxisParam, GmParam, PlinParam, ScomInfo};
use aios_core::pdms_types::AttrVal::IntArrayType;
use aios_core::pdms_types::{AttrMap, RefU64, TOTAL_CATA_GEO_NOUN_NAMES, TOTAL_GEO_NOUN_NAMES};
use anyhow::anyhow;
use dashmap::mapref::one::Ref;
use dashmap::DashMap;
use log::{error, info};
use sled::pin;
use std::collections::{BTreeMap, HashMap};
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use glam::Vec3;
use tokio::sync::RwLock;
use crate::cata::consts::{DDANGLE_STR, DDHEIGHT_STR, DDRADIUS_STR};


///求解design component
pub async fn resolve_desi_comp<T: PdmsDataInterface>(
    interface: Option<&T>,
    refno: RefU64,
    mut scom_ref: Option<RefU64>,
    scom_info_map: &RwLock<HashMap<RefU64, ScomInfo>>,
) -> anyhow::Result<CateGeomsInfo> {
    let interface = interface.ok_or(anyhow!("unknown interface"))?;
    let desi_att = interface.get_attr_from_localdb(refno)?;
    //todo 改到使用图数据库去查找
    if scom_ref.is_none() {
        if let Some(spre_ref) = desi_att.get_foreign_refno("SPRE") {
            // dbg!(spre_ref);
            let spre = interface.get_attr_from_localdb(spre_ref).unwrap_or_default();
            if spre.contains_attr_name("CATR") {
                scom_ref = spre.get_foreign_refno("CATR");
            } else {
                // SFIT 的 scom 和 spre 是同一个
                scom_ref = Some(spre_ref);
            }
        } else {
            if let Some(catref) = desi_att.get_foreign_refno("CATR") {
                let c_att = interface.get_attr_from_localdb(catref).unwrap_or_default();
                if c_att.get_type() == "TABITE" {
                    let tmp_ref = c_att.get_foreign_refno("PRTREF").unwrap_or_default();
                    let t_att = interface.get_attr_from_localdb(tmp_ref)?;
                    scom_ref = t_att.get_foreign_refno("CATR");
                } else if c_att.get_type() == "SPCO"{
                    scom_ref = c_att.get_foreign_refno("CATR");
                } else {
                    scom_ref = Some(catref);
                }
            }
        }
    }
    // dbg!(scom_ref);
    let scom_ref = scom_ref.ok_or(anyhow!(format!(
        "SCOM not exist in element: {}",
        refno.to_refno_str()
    )))?;
    if !scom_ref.is_valid() {
        println!("{} 的CAT引用不存在，为 {}", refno.to_refno_str(), scom_ref.to_refno_str());
        return Ok(Default::default());
    }
    //缓存备用
    if !scom_info_map.read().await.contains_key(&scom_ref) {
        match query_scom_info(scom_ref, Some(interface)).await {
            Ok(scom_info) => {
                scom_info_map.write().await.insert(scom_ref, scom_info);
            }
            Err(e) => {
                let error_info = format!("Design的元件：{} 使用的元件库: {} 解析出错 {}",
                                         refno.to_refno_string(), scom_ref.to_refno_string(), e.to_string());
                println!("{}", &error_info);
                return Err(anyhow!(error_info));
            }
        }
    }
    let scom_read = scom_info_map.read().await;
    let scom_info = scom_read.get(&scom_ref).unwrap();
    // dbg!(&scom_info.gm_params);
    // dbg!(&scom_info.axis_params);
    let mut context: BTreeMap<String, String> = BTreeMap::new();
    if let Some(v) = desi_att.get_as_string("JUSL") {
        context.insert("JUSL".into(), v.into());
    }
    context.insert("DESI_REFNO".into(), refno.to_refno_str());
    let mut desp = desi_att.get_f64_vec("DESP").unwrap_or_default();
    for i in 0..desp.len() {
        context.insert(format!("DESI{}", i + 1).into(), desp[i].to_string().into());
        context.insert(format!("DDES{}", i + 1).into(), desp[i].to_string().into());
        context.insert(format!("DESP{}", i + 1).into(), desp[i].to_string().into());
    }
    let height = desi_att.get_as_string("HEIG").unwrap_or("0.0".into());
    context.insert(DDHEIGHT_STR.into(), (height.clone()));
    context.insert("HEIG".into(), (height));
    let angle = desi_att.get_as_string("ANGL").unwrap_or("0.0".into());
    context.insert(DDANGLE_STR.into(), (angle.clone()));
    context.insert("ANGL".into(), (angle));
    let radi = desi_att.get_as_string("RADI").unwrap_or("0.0".into());
    context.insert(DDRADIUS_STR.into(), (radi.clone()));
    context.insert("RADI".into(), (radi));
    let geom_info = resolve_cata_comp(refno, &scom_info, Some(interface), Some(context)).await;
    // dbg!(&geom_info);
    if geom_info.is_err() {
        error!("{:?}", geom_info.as_ref().err());
        error!("{:?}", desi_att.to_string_hashmap());
    }
    geom_info
}

///整合SCOM对应的临时数据
pub async fn query_scom_info<T: PdmsDataInterface>(
    refno: RefU64,
    interface: Option<&T>,
) -> anyhow::Result<ScomInfo> {
    let interface = interface.ok_or(anyhow!("unknown interface"))?;
    let attr_map = interface.get_attr_from_localdb(refno)?;
    let type_noun = attr_map
        .get_type_cloned()
        .ok_or(anyhow!(format!("{} 元件库属性不正确: {:?}", refno.to_refno_string(), &attr_map)))?;
    let ptref_name = match type_noun.as_str() {
        "SPRF" => "PSTR",
        _ => "PTRE"
    };
    let mut axis_params = vec![];
    let mut axis_param_numbers = vec![];
    if let Some(ptre_refno) = attr_map.get_foreign_refno(ptref_name) {
        if let Ok(ptre_am) = interface.get_attr_from_localdb(ptre_refno) {
            if let Ok(axis_param_map) = query_axis_params(&ptre_am, Some(interface)).await {
                axis_params = axis_param_map.values().cloned().collect::<Vec<_>>();
                axis_param_numbers = axis_param_map.keys().cloned().collect::<Vec<_>>();
            }
        }
    }
    let gmref_name = match type_noun.as_str() {
        "SPRF" => "GSTR",
        _ => "GMRE",
    };
    let mut gm_params = vec![];
    if let Some(gmse_refno) = attr_map.get_foreign_refno(gmref_name) {
        if let Ok(gmse_am) = interface.get_attr_from_localdb(gmse_refno) {
            gm_params = query_gm_params(&gmse_am, Some(interface)).await?;
        }
    }
    let mut plin_map = HashMap::new();
    if let Some(pstr_refno) = attr_map.get_foreign_refno("PSTR") {
        let pstr_am = interface.get_children_attrs(pstr_refno)?;
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
    // dbg!(&plin_map);
    Ok(ScomInfo {
        gtype: attr_map.get_as_string("GTYP").unwrap_or("unset".into()),
        dtse_params: vec![],
        gm_params,
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
    })
}

///查询 Axis 参数
pub async fn query_axis_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: Option<&T>,
) -> anyhow::Result<BTreeMap<i32, AxisParam>> {
    // 查找ptse
    let interface = interface.ok_or(anyhow!("unknown interface"))?;
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
pub async fn query_gm_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: Option<&T>,
) -> anyhow::Result<Vec<GmParam>> {
    let interface = interface.ok_or(anyhow!("unknown interface"))?;
    let mut gms = vec![];
    let refno = attr_map.get_refno().unwrap_or_default();
    let children = interface.get_travel_children_attrs(refno, &TOTAL_CATA_GEO_NOUN_NAMES).await.unwrap();
    for geo_am in children {
        if !geo_am.is_visible_by_level(None).unwrap_or(true) {
            continue;
        }
        let has_children = geo_am.get_type_cloned().unwrap_or_default() == "SPRO"; //todo add other types
        gms.push(
            query_gm_param(&geo_am, interface, has_children)
                .await
                .unwrap_or_default(),
        );
    }
    Ok(gms)
}

///对元件库的SCOM Element进行求值计算
pub async fn resolve_cata_comp<T: PdmsDataInterface>(
    des_refno: RefU64,
    scom_info: &ScomInfo,
    interface: Option<&T>,
    context: Option<BTreeMap<String, String>>,
) -> anyhow::Result<CateGeomsInfo> {
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

    let cat_ref = scom_info.attr_map.get_refno().unwrap_or_default();
    //获取DTSE的expression
    process_dtse_params(&scom_info.attr_map, interface, &mut cur_context).await;

    cur_context.insert("RS_DES_REFNO".into(), des_refno.to_refno_str());
    cur_context.insert("RS_SCOM_REFNO".into(), scom_info.attr_map.get_refno().unwrap().to_refno_str());

    //保温层厚度
    //PARA
    let params = scom_info.attr_map.get_f64_vec("PARA").unwrap_or_default();
    //OPAR的信息收集
    let int = interface.as_ref().unwrap();
    // dbg!(&params);
    for i in 0..params.len() {
        cur_context.insert(
            format!("CPAR{}", i + 1).into(),
            params[i].to_string().into(),
        );
        cur_context.insert(
            format!("PARA{}", i + 1).into(),
            params[i].to_string().into(),
        );
        cur_context.insert(
            format!("PARAM{}", i + 1).into(),
            params[i].to_string().into(),
        );
        cur_context.insert(format!("IPAR{}", i + 1).into(), "0".to_string().into());
    }

    if let Ok(Some(parent_cat_ref)) = int
        .query_first_foreign_along_path(des_refno, &["SPRE", "CATR"], &["SPRE", "CATR"], &[])
        .await{
        if let Ok(parent_cat_am) = interface.as_ref().unwrap().get_attr_from_localdb(parent_cat_ref){
            let params = parent_cat_am.get_f64_vec("PARA").unwrap_or_default();
            for i in 0..params.len() {
                cur_context.insert(
                    format!("OPAR{}", i + 1).into(),
                    params[i].to_string().into(),
                );
            }
            let desp = parent_cat_am.get_f64_vec("DESP").unwrap_or_default();
            for i in 0..desp.len() {
                cur_context.insert(
                    format!("ODES{}", i + 1).into(),
                    desp[i].to_string().into(),
                );
            }
        }

    }

    if let Ok(link_cat_refs) = int
        .query_foreign_refnos(&[des_refno], &[&["CREF"], &["SPRE", "CATR"]], &["SPRE", "CATR"],&[], 4)
        .await{
        if !link_cat_refs.is_empty() {
            let link_cat_ref = link_cat_refs[0];
            if let Ok(link_cat_am) = interface.as_ref().unwrap().get_attr_from_localdb(link_cat_ref) {
                let params = link_cat_am.get_f64_vec("PARA").unwrap_or_default();
                for i in 0..params.len() {
                    cur_context.insert(
                        format!("APAR{}", i + 1).into(),
                        params[i].to_string().into(),
                    );
                }
                let desp = link_cat_am.get_f64_vec("DESP").unwrap_or_default();
                for i in 0..desp.len() {
                    cur_context.insert(
                        format!("ADES{}", i + 1).into(),
                        desp[i].to_string().into(),
                    );
                }
            }
        }
    }


    let axis_map = resolve_axis_params(scom_info, &cur_context, interface);
    // dbg!(&scom_info.axis_params);
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
    //说明: 需要传递 interface, 因为可能需要取属性值
    // dbg!(&scom_info.gm_params);
    // dbg!(&scom_info.axis_params);
    let geometries = resolve_gms(des_refno, &scom_info.gm_params, &jusl_param, &cur_context, &axis_map, interface);
    for geometry in &geometries {
        if let CateGeoParam::Pyramid(l) = geometry {
            dbg!(&l);
        }
    }
    // dbg!(&geometries);
    Ok(CateGeomsInfo {
        refno: cat_ref,
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
            direction: { attr_map.get_as_smol_str("PTCD").unwrap_or("Y".into()) },
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
pub async fn query_gm_param(
    a: &AttrMap,
    interface: &dyn PdmsDataInterface,
    has_children: bool,
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
        if has_children {
            for a in interface.get_children_attrs(refno).ok()? {
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
        gm_type: a.get_type_cloned().unwrap_or_default(),
        prad: (a.get_as_string("PRAD").unwrap_or_default()),
        pang: (a.get_as_string("PANG").unwrap_or_default()),
        pwid: (a.get_as_string("PWID").unwrap_or_default()),
        diameters: a.get_attr_strings_without_default(&["PDIA", "PBDM", "PTDM", "DIAM"]),
        distances: a.get_attr_strings(&["PDIS", "PBDI", "PTDI"]),
        shears: a.get_attr_strings(&["PXTS", "PYTS", "PXBS", "PYBS"]),
        phei: (a.get_as_string("PHEI").unwrap_or_default()),
        offset: (a.get_as_string("POFF").unwrap_or_default()),
        box_lengths: a.get_attr_strings(&["PXLE", "PYLE", "PZLE"]),
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

///获得dtse的参数信息
pub async fn process_dtse_params<T: PdmsDataInterface>(
    attr_map: &AttrMap,
    interface: Option<&T>,
    context: &mut BTreeMap<String, String>,
) -> Option<bool> {
    let interface = interface?;
    let dtre_refno = attr_map.get_foreign_refno("DTRE")?;
    let children = interface
        .get_children_attrs(dtre_refno)
        .ok()?;
    for child in children {
        let key = (format!("RPRO_{}", child.get_as_string("DKEY")?));
        let exp = (child.get_as_string("PPRO")?);
        let default_key = format!("{}_default_expr", key);
        let default_expr = (child.get_as_string("DPRO")?);
        context.insert(key, exp);
        context.insert(default_key.into(), default_expr);
    }
    Some(true)
}
