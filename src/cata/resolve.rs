use std::collections::{BTreeMap, HashMap};
use std::default::default;
use std::ops::Neg;
use std::panic;
use std::sync::Arc;
use aios_core::parsed_data::{CateAxisParam, GmseParamData};
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::pdms_data::{AxisParam, GmParam, PlinParam, ScomInfo};
use aios_core::pdms_types::RefU64;
use aios_core::tool::db_tool::db1_dehash;
use anyhow::anyhow;
use bb8_arangodb::arangors_lite::Database;
use glam::{Vec2, Vec3};
use crate::aql_api::dtse_attr::query_dtse_ppro_from_catr_refno;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::para_value::query_para_value;
use crate::cata::consts::{DDANGLE_STR, DDHEIGHT_STR, DDRADIUS_STR};
use crate::cata::resolve_helper::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;


/// 求解axis的数值
pub fn resolve_axis_params<T: PdmsDataInterface>(
    refno: RefU64,
    scom: &ScomInfo,
    context: &BTreeMap<String, String>,
    interface: Option<&T>,
) -> BTreeMap<i32, CateAxisParam> {
    let mut map = BTreeMap::new();
    // dbg!(&scom.axis_params);
    for i in 0..scom.axis_params.len() {
        // dbg!(&scom.axis_params[i]);
        match resolve_axis_param(&scom.axis_params[i], scom, context, interface) {
            Ok(axis) => {
                // dbg!(&axis);
                map.insert(scom.axis_param_numbers[i], axis);
            }
            Err(e) => {
                println!("{} resolve_axis_params 出错： {:?}", refno, &e);
            }
        }
    }
    map
}

///求解几何体，允许出错的情况，出错的需要跳过
pub fn resolve_gms<T: PdmsDataInterface>(
    des_refno: RefU64,
    gmse_raw_paras: &[GmParam],
    jusl_param: &Option<PlinParam>,
    context: &BTreeMap<String, String>,
    axis_params: &BTreeMap<i32, CateAxisParam>,
    interface: Option<&T>,
) -> Vec<CateGeoParam> {
    gmse_raw_paras
        .iter()
        .filter_map(|g| {
            if g.visible_flag {
                if g.gm_type == "SPRO" && g.verts.is_empty() {
                    return None;
                }
                let r = resolve_paragon_gm_params(des_refno, &g, jusl_param, context, axis_params, interface);
                return match r {
                    Ok(v) => {
                        Some(v)
                    }
                    Err(e) => {
                        // dbg!(g);
                        println!("{}", e);
                        None
                    }
                };
            } else {
                None
            }
        })
        .collect::<_>()
}

/// 解析gmes的参数
pub fn resolve_paragon_gm_params<T: PdmsDataInterface>(
    des_refno: RefU64,
    gm_param: &GmParam,
    jusl_param: &Option<PlinParam>,
    context: &BTreeMap<String, String>,
    axis_params: &BTreeMap<i32, CateAxisParam>,
    interface: Option<&T>,
) -> anyhow::Result<CateGeoParam> {
    match resolve_gmse_params(gm_param, jusl_param, context, axis_params, interface) {
        Ok(gm_data) => {
            panic::catch_unwind(|| {
                resolve_to_cate_geo_params(&gm_data)
                    .expect("resolve geom failed")
            })
                .map_err(|e| anyhow!("元件库求解失败."))
        }
        Err(e) => {
            Err(anyhow!(format!("几何数据解析失败: {:?}, 原因：{}", des_refno.to_refno_string(), &e)))
        }
    }
}

/// 元件库表达式相关的参数
#[derive(Debug, Default, Clone)]
pub struct CataExprContext {
    pub params: Vec<f64>,
    pub dtse_expr_map: HashMap<String, String>,
    pub dtse_default_map: HashMap<String, String>,
    // pub context: HashMap<String, String>,
}

impl CataExprContext {
    pub async fn create(des_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Self>> {
        let catr_refno = query_foreign_refno_aql(&database, des_refno, &["SPRE", "CATR"]).await?;
        if catr_refno.is_none() { return Ok(None); }
        let catr_refno = catr_refno.unwrap();
        let params = query_para_value(catr_refno, &database).await?;
        if params.is_none() { return Ok(None); }
        let dtse_map = query_dtse_ppro_from_catr_refno(catr_refno, &database).await?;
        if dtse_map.is_none() { return Ok(None); }
        let mut dtse_expr_map = HashMap::new();
        let mut dtse_default_map = HashMap::new();
        for (k, v) in dtse_map.unwrap().into_iter() {
            dtse_expr_map.entry(k.clone()).or_insert(v.ppro);
            dtse_default_map.entry(k).or_insert(v.dpro);
        }
        Ok(Some(Self {
            params: params.unwrap(),
            dtse_expr_map,
            dtse_default_map,
        }))
    }
    //需要获取design的数据
    pub async fn build(&self, mgr: &AiosDBManager, des_refno: RefU64) -> BTreeMap<String, String> {
        let mut context: BTreeMap<String, String> = Default::default();
        if let Ok(attr_map) = mgr.get_attr(des_refno).await {
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
            let height: String = attr_map.get_as_string("HEIG").unwrap_or("0.0".into()).into();
            context.insert(DDHEIGHT_STR.into(), height.clone());
            context.insert("HEIG".into(), height);
            let angle: String = attr_map.get_as_string("ANGL").unwrap_or("0.0".into()).into();
            context.insert(DDANGLE_STR.into(), angle.clone());
            context.insert("ANGL".into(), angle);
            let radi: String = attr_map.get_as_string("RADI").unwrap_or("0.0".into()).into();
            context.insert(DDRADIUS_STR.into(), radi.clone());
            context.insert("RADI".into(), radi);
        } else {
            //默认值
            context
                .entry(DDHEIGHT_STR.into())
                .or_insert("0.0".into());
            context
                .entry(DDRADIUS_STR.into())
                .or_insert("0.0".into());
            context
                .entry(DDANGLE_STR.into())
                .or_insert("0.0".into());
        }

        //获取DTSE的expression
        // process_dtse_params(&scom_info.attr_map, interface, &mut cur_context).await;

        //保温层厚度
        context.insert("IPARA0".into(), "0".into());
        context.insert("IPARA".into(), "0".into());

        // let parent_cat_ref = interface

        for i in 0..self.params.len() {
            //todo OPAR需要去有catalog的父节点里去找
            context.insert(format!("OPAR{}", i + 1).into(), self.params[i].to_string().into());
            context.insert(format!("APAR{}", i + 1).into(), self.params[i].to_string().into());
            context.insert(format!("CPAR{}", i + 1).into(), self.params[i].to_string().into());
            context.insert(format!("PARA{}", i + 1).into(), self.params[i].to_string().into());
            context.insert(format!("IPARA{}", i + 1).into(), "0".to_string().into());
            context.insert(format!("IPAR{}", i + 1).into(), "0".to_string().into());
        }
        context
    }
}


pub fn resolve_gmse_params<T: PdmsDataInterface>(
    gm: &GmParam,
    jusl_param: &Option<PlinParam>,
    context: &BTreeMap<String, String>,
    axis_param_map: &BTreeMap<i32, CateAxisParam>,
    interface: Option<&T>,
) -> anyhow::Result<GmseParamData> {
    let angle = context[DDANGLE_STR].parse::<f32>().unwrap_or(0.0).to_radians();
    let radius = context[DDRADIUS_STR].parse::<f32>().unwrap_or(0.0);
    let height = context[DDHEIGHT_STR].parse::<f32>().unwrap_or(0.0);
    let diameters = gm.diameters
        .iter()
        .map(|exp| eval_str_to_f32(exp, context, interface))
        .collect::<anyhow::Result<_>>()?;

    let distances = gm.distances
        .iter()
        .map(|exp| eval_str_to_f32(exp, context, interface))
        .collect::<anyhow::Result<_>>()?;

    let shears = gm.shears
        .iter()
        .map(|exp| eval_str_to_f32(exp, context, interface))
        .collect::<anyhow::Result<_>>()?;

    let mut verts = vec![];
    for vert in &gm.verts {
        if let f0 = eval_str_to_f32(&vert[0], context, interface)? &&
            let f1 = eval_str_to_f32(&vert[1], context, interface)? &&
            let f2 = eval_str_to_f32(&vert[2].as_str(), context, interface)?
        {
            verts.push(Vec3::new(f0, f1, f2));
        }
    }

    let phei = eval_str_to_f32_or_default(&gm.phei, context, interface);
    let offset = eval_str_to_f32_or_default(&gm.offset, context, interface);

    let pang = eval_str_to_f32_or_default(&gm.pang, context, interface);
    let pwid = eval_str_to_f32_or_default(&gm.pwid, context, interface);
    let drad = eval_str_to_f32_or_default(&gm.drad, context, interface);
    let dwid = eval_str_to_f32_or_default(&gm.dwid, context, interface);

    let mut frads = gm.frads
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context, interface))
        .collect::<anyhow::Result<_>>()?;

    let prad = eval_str_to_f32(&gm.prad, context, interface)?;

    let dxy = gm.dxy
        .iter()
        .try_fold::<_, _, anyhow::Result<_>>(vec![], |mut acc, exp| {
            let f0 = eval_str_to_f32(&exp[0], context, interface)? as f32;
            let f1 = eval_str_to_f32(&exp[1], context, interface)? as f32;
            acc.push([f0, f1]);
            Ok(acc)
        })?;

    let box_lengths = gm.box_lengths
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context, interface))
        .collect::<anyhow::Result<_>>()?;

    let xyz = gm.xyz
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context, interface))
        .collect::<anyhow::Result<_>>()?;

    let mut paxises: Vec<Option<CateAxisParam>> = Vec::new();
    for axis_str in gm.paxises.iter() {
        let mut axis = axis_str.trim();
        if axis.is_empty() { continue; }
        let p_axis = axis.starts_with("P");
        let p_axis_neg = axis.starts_with("-P");
        //针对P方向
        if p_axis || p_axis_neg {
            if p_axis_neg {
                axis = &axis[1..];
            }
            if let Ok(index) = axis[1..].parse::<i32>() {
                if axis_param_map.contains_key(&index) {
                    paxises.push(Some(if p_axis_neg {
                        axis_param_map[&index].clone().neg()
                    } else {
                        axis_param_map[&index].clone()
                    }));
                } else {
                    paxises.push(None);
                    println!("Axis: '{axis_str}' index not exist");
                }
            }
        } else {
            let dir = parse_str_axis_to_vec3(axis, context, interface)?;
            let axis = CateAxisParam {
                refno: Default::default(),
                number: 0,
                pt: Default::default(),
                dir,
                pconnect: "".to_string(),
                pbore: 0.0,
                pwidth: 0.0,
                pheight: 0.0,
            };
            paxises.push(Some(axis));
        }
    }
    let mut plin_verts = Vec2::ZERO;
    let mut plin_plax = Vec3::X;
    if let Some(jusl) = jusl_param {
        //直接把 jusl_dxy加上
        plin_verts = Vec2::new(eval_str_to_f32(&jusl.vxy[0], context, interface)?,
                               eval_str_to_f32(&jusl.vxy[1], context, interface)?)
            + Vec2::new(eval_str_to_f32(&jusl.dxy[0], context, interface)?,
                        eval_str_to_f32(&jusl.dxy[1], context, interface)?);

        plin_plax = parse_str_axis_to_vec3(&jusl.plax, context, interface)?;
    }
    let type_name = gm.gm_type.clone();
    Ok(GmseParamData {
        refno: gm.refno,
        type_name,
        radius,
        angle,
        height,
        pwid,
        prad,
        plin_pos: plin_verts,
        frads,
        pang,
        diameters,
        distances,
        shears,
        phei,
        offset,
        verts,
        dxy,
        drad,
        dwid,
        box_lengths,
        xyz,
        paxises,
        centre_line_flag: gm.centre_line_flag,
        tube_flag: gm.visible_flag,
        plin_plax,
    })
}

pub fn resolve_axis_param<T: PdmsDataInterface>(
    axis_param: &AxisParam,
    scom: &ScomInfo,
    context: &BTreeMap<String, String>,
    interface: Option<&T>,
) -> anyhow::Result<CateAxisParam> {
    let key: String = axis_param.pconnect.replace("\n", "").replace(" ", "").into();
    let pconnect = if context.contains_key(&key) {
        let tmp = context[&key].parse::<u32>().unwrap_or(0u32);
        db1_dehash(tmp)
    } else {
        key.clone()
    };
    let number = axis_param.number;
    let pbore = eval_str_to_f32(&axis_param.pbore, &context, interface)?;
    let pwidth = eval_str_to_f32(&axis_param.pwidth, &context, interface)?;
    let pheight = eval_str_to_f32(&axis_param.pheight, &context, interface)?;
    match axis_param.type_name.as_str() {
        "PTAX" => {
            let d = eval_str_to_f32(&axis_param.distance, &context, interface)?;
            let (dir, pos) = resolve_dir_and_pos(axis_param, scom, context, interface)?;
            Ok(CateAxisParam {
                refno: axis_param.refno,
                number,
                pt: Vec3::new(d * dir[0] + pos[0], d * dir[1] + pos[1], d * dir[2] + pos[2]),
                dir,
                pconnect,
                pbore,
                pwidth,
                pheight,
            })
        }
        "PTCA" | "PTMI" => {
            let x = eval_str_to_f32(&axis_param.x, &context, interface)?;
            let y = eval_str_to_f32(&axis_param.y, &context, interface)?;
            let z = eval_str_to_f32(&axis_param.z, &context, interface)?;
            let (dir, pos) = resolve_dir_and_pos(axis_param, scom, context, interface)?;
            Ok(CateAxisParam {
                refno: axis_param.refno,
                number,
                pt: Vec3::new(pos[0] + x, pos[1] + y, pos[2] + z),
                dir,
                pconnect,
                pbore,
                pwidth,
                pheight,
            })
        }
        "PTPOS" => {
            let (dir, pos) = resolve_dir_and_pos(axis_param, scom, context, interface)?;
            let pnt_index_str = axis_param.pnt_index_str.as_ref().ok_or(anyhow!("pnt_index_str 错误"))?;
            let paras = pnt_index_str.split_whitespace().map(|x| x.trim().to_owned()).collect::<Vec<_>>();
            if paras.len() == 2 {
                let pnt_index = paras[1].parse::<i32>().unwrap_or(i32::MAX);
                if let Some(indx) = scom.axis_param_numbers.iter().position(|&x| x == pnt_index) {
                    let axis = resolve_axis_param(&scom.axis_params[indx], scom, context, interface)?;
                    return Ok(CateAxisParam {
                        refno: axis_param.refno,
                        number,
                        pt: axis.pt,
                        dir,
                        pconnect,
                        pbore,
                        pwidth,
                        pheight,
                    });
                }
            }
            return Ok(CateAxisParam::default());
        }
        _ => Ok(CateAxisParam::default())
    }
}

#[inline]
pub fn parse_to_u16(input: &[u8]) -> u16 {
    u16::from_be_bytes(input.try_into().unwrap())
}

#[inline]
pub fn parse_to_i16(input: &[u8]) -> i16 {
    i16::from_be_bytes(input.try_into().unwrap())
}

#[inline]
pub fn parse_to_i32(input: &[u8]) -> i32 {
    i32::from_be_bytes(input.try_into().unwrap())
}

#[inline]
pub fn parse_to_u32(input: &[u8]) -> u32 {
    u32::from_be_bytes(input.try_into().unwrap())
}

#[inline]
pub fn parse_to_u64(input: &[u8]) -> u64 {
    u64::from_be_bytes(input.try_into().unwrap())
}

#[inline]
pub fn parse_to_i64(input: &[u8]) -> i64 {
    i64::from_be_bytes(input.try_into().unwrap())
}

#[inline]
pub fn parse_to_f32(input: &[u8]) -> f32 {
    (f32::from_be_bytes(input.try_into().unwrap()) * 100.0).round() / 100.0
}

#[inline]
pub fn parse_to_f64(input: &[u8]) -> f64 {
    return if let [a, b, c, d, e, f, g, h] = input[..8] {
        (f64::from_be_bytes([e, f, g, h, a, b, c, d]) * 100.0).round() / 100.0
    } else {
        0.0
    };
}


#[inline]
pub fn convert_u32_to_noun(input: &[u8]) -> String {
    db1_dehash(parse_to_u32(input.try_into().unwrap())).into()
}

#[inline]
pub fn parse_to_f64_arr(input: &[u8]) -> [f64; 3] {
    let mut data = [0f64; 3];
    for i in 0..3 {
        data[i] = parse_to_f64(&input[i * 8..i * 8 + 8]);
    }
    data
}

#[inline]
pub fn parse_to_f32_arr(input: &[u8]) -> [f64; 3] {
    let mut data = [0f64; 3];
    for i in 0..3 {
        data[i] = parse_to_f32(&input[i * 4..i * 4 + 4]) as f64;
    }
    data
}