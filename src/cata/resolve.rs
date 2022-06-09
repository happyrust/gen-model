use std::collections::{BTreeMap, HashMap};
use std::ops::Neg;
use aios_core::parsed_data::{CateAxisParam, GmseParamData};
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::pdms_data::{AxisParam, GmParam, ScomInfo};
use aios_core::pdms_types::RefU64;
use aios_core::tool::db_tool::db1_dehash;
use anyhow::anyhow;
use smol_str::SmolStr;
use crate::cata::query_cata::{DDANGLE_STR, DDHEIGHT_STR, DDRADIUS_STR};
use crate::cata::resolve_helper::{eval_str_to_f32, eval_str_to_f64, parse_str_axis_to_vec3, resolve_dir_and_pos, resolve_to_cate_geo_params};

/// 求解axis的数值, 得到 {num:  }
pub fn resolve_axis_params(
    scom: &ScomInfo,
    context: &HashMap<SmolStr, SmolStr>,
) -> BTreeMap<i32, CateAxisParam> {
    let mut map = BTreeMap::new();
    for i in 0..scom.axis_params.len() {
        if let Some(axis) = resolve_axis_param(&scom.axis_params[i], scom, context) {
            map.insert(scom.axis_param_numbers[i], axis);
        }
    }
    map
}

///求解几何体，允许出错的情况，出错的需要跳过
pub fn resolve_gms(
    gmse_raw_paras: &[GmParam],
    context: &HashMap<SmolStr, SmolStr>,
    axis_params: &BTreeMap<i32, CateAxisParam>,
) -> Vec<CateGeoParam> {
    gmse_raw_paras
        .iter()
        .filter_map(|g| {
            if g.visible_flag{
                let r = resolve_paragon_gm_params(&g, context, axis_params);
                return match r {
                    Ok(v) => {
                        Some(v)
                    }
                    Err(e) => {
                        dbg!(e);
                        // dbg!(g);
                        // dbg!(context);
                        None
                    }
                }
            }else{
                None
            }
        })
        .collect::<_>()
}

/// 解析gmes的参数
pub fn resolve_paragon_gm_params(
    gm_param: &GmParam,
    context: &HashMap<SmolStr, SmolStr>,
    axis_params: &BTreeMap<i32, CateAxisParam>,
) -> anyhow::Result<CateGeoParam> {
    // if gm_param.refno != RefU64::from_two_nums(15194, 4258) {
    //     return Ok(CateGeoParam::Unknown);
    // }
    if let Ok(gm_data) = resolve_gmse_params(gm_param, context, axis_params){
        resolve_to_cate_geo_params(&gm_data)
    }else{
        Err(anyhow!(format!("几何数据解析失败: {:?}", gm_param)))
    }
}

pub fn resolve_gmse_params(
    gm: &GmParam,
    context: &HashMap<SmolStr, SmolStr>,
    axis_param_map: &BTreeMap<i32, CateAxisParam>,
) -> anyhow::Result<GmseParamData> {
    let angle = context[DDANGLE_STR].parse::<f32>().unwrap_or(0.0).to_radians();
    let radius = context[DDRADIUS_STR].parse::<f32>().unwrap_or(0.0);
    let height = context[DDHEIGHT_STR].parse::<f32>().unwrap_or(0.0);
    let diameters = gm.diameters
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context))
        .collect::<anyhow::Result<_>>()?;

    let distances = gm.distances
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context))
        .collect::<anyhow::Result<_>>()?;

    let shears = gm.shears
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context))
        .collect::<anyhow::Result<_>>()?;

    let verts = gm.verts
        .iter()
        .try_fold::<_, _, anyhow::Result<_>>(vec![], |mut acc, exp| {
            let f0 = eval_str_to_f32(exp[0].as_str(), context)? as f32;
            let f1 = eval_str_to_f32(exp[1].as_str(), context)? as f32;
            acc.push([f0, f1]);
            Ok(acc)
        })?;

    let phei = eval_str_to_f32(&gm.phei, context)?;
    let offset = eval_str_to_f32(&gm.offset, context)?;

    let pang = eval_str_to_f32(&gm.pang, context)?;
    let pwid = eval_str_to_f32(&gm.pwid, context)?;
    let drad = eval_str_to_f32(&gm.drad, context)?;
    let dwid = eval_str_to_f32(&gm.dwid, context)?;

    let mut prads = gm.prads
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context))
        .collect::<anyhow::Result<_>>()?;

    let prad = eval_str_to_f32(&gm.prad, context)?;

    let dxy = gm.dxy
        .iter()
        .try_fold::<_, _, anyhow::Result<_>>(vec![], |mut acc, exp| {
            let f0 = eval_str_to_f32(exp[0].as_str(), context)? as f32;
            let f1 = eval_str_to_f32(exp[1].as_str(), context)? as f32;
            acc.push([f0, f1]);
            Ok(acc)
        })?;

    let box_lengths = gm.box_lengths
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context))
        .collect::<anyhow::Result<_>>()?;

    let xyz = gm.xyz
        .iter()
        .map(|exp| eval_str_to_f32(&exp, context))
        .collect::<anyhow::Result<_>>()?;

    let mut paxises: Vec<CateAxisParam> = Vec::new();
    for name in gm.paxises.iter() {
        if name != "" {
            let (is_negative, name) = if name.starts_with('-') {
                (true, &name[1..])
            } else {
                (false, &name[..])
            };
            match &name[0..1] {
                "P" => {
                    if let Ok(index) = name.trim()[1..].parse::<i32>() {
                        if index == 0 {
                            paxises.push(CateAxisParam::zero());
                        } else {
                            if axis_param_map.contains_key(&index) {
                                paxises.push(if is_negative {
                                    axis_param_map[&index].clone().neg()
                                } else {
                                    axis_param_map[&index].clone()
                                });
                            } else {
                                return Err(anyhow!("Axis index not exist".to_string()));
                            }
                        }
                    }
                }
                "T" => {}
                _ => {
                    let ddangle = context["DDANGLE"].parse::<f64>().unwrap_or(0.0f64);
                    let dir = parse_str_axis_to_vec3(name, ddangle);
                    let axis = CateAxisParam {
                        pt: vec![0.0f64, 0.0, 0.0],
                        dir: dir.to_vec(),
                        pconnect: "".to_string(),
                        pbore: 0.0,
                    };
                    paxises.push(if is_negative { axis.neg() } else { axis });
                }
            }
        }
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
        prads,
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
    })
}

pub fn resolve_axis_param(
    axis_param: &AxisParam,
    scom: &ScomInfo,
    context: &HashMap<SmolStr, SmolStr>,
) -> Option<CateAxisParam> {
    let ddangle = context["DDANGLE"].parse::<f64>().unwrap_or(0.0f64);
    let key: SmolStr = axis_param.pconnect.replace("\n", "").replace(" ", "").into();
    let pconnect = if context.contains_key(&key) {
        let tmp = context[&key].parse::<u32>().unwrap_or(0u32);
        db1_dehash(tmp)
    } else {
        "".to_string()
    };
    let pbore = eval_str_to_f64(&axis_param.pbore, &context).unwrap_or_default();
    let type_name = axis_param.attr_map.get_type_cloned()?;
    match type_name.as_str() {
        "PTAX" => {
            let d = eval_str_to_f64(&axis_param.distance, &context).unwrap_or_default();
            let (dir, pos) = resolve_dir_and_pos(axis_param, ddangle, scom, context);
            Some(CateAxisParam {
                pt: vec![d * dir[0] + pos[0], d * dir[1] + pos[1], d * dir[2] + pos[2]],
                dir: dir.to_vec(),
                pconnect,
                pbore,
            })
        }
        "PTCA" | "PTMI" => {
            let x = eval_str_to_f64(&axis_param.x, &context).unwrap_or_default();
            let y = eval_str_to_f64(&axis_param.y, &context).unwrap_or_default();
            let z = eval_str_to_f64(&axis_param.z, &context).unwrap_or_default();
            // //dbg!(axis_param.attr_map.to_string_hashmap());
            let (dir, pos) = resolve_dir_and_pos(axis_param, ddangle, scom, context);
            Some(CateAxisParam { pt: vec![pos[0] + x, pos[1] + y, pos[2] + z], dir: dir.to_vec(), pconnect, pbore })
        }
        "PTPOS" => {
            let (dir, pos) = resolve_dir_and_pos(axis_param, ddangle, scom, context);
            let pnt_index_str = axis_param.attr_map.get_as_string("PTCPOS").unwrap_or_default();
            let paras = pnt_index_str.split_whitespace().map(|x| x.trim().to_owned()).collect::<Vec<_>>();
            if paras.len() == 2 {
                let pnt_index = paras[1].parse::<i32>().unwrap_or(i32::MAX);
                if let Some(indx) = scom.axis_param_numbers.iter().position(|&x| x == pnt_index) {
                    if let Some(axis) = resolve_axis_param(&scom.axis_params[indx], scom, context) {
                        Some(CateAxisParam { pt: axis.pt, dir: dir.to_vec(), pconnect, pbore })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None
    }
}

pub fn convert_to_context_key(expr: &str, i: &mut usize, strs: &Vec<SmolStr>) -> Option<SmolStr> {
    match expr {
        "PARA" | "PARAM" | "CPAR" => {
            *i += 1;
            Some(format!("PARAM{}", strs[*i]).into())
        }
        "ANGL" => {
            Some("ANGL".into())
        }
        "IPAR" | "IPARAM" => {
            *i += 1;
            //先忽略保温层厚度
            Some(format!("IPARAM{}", strs[*i]).into())
        }
        "DESP" | "DDESP"  => {
            *i += 1;
            Some(format!("DESP{}", strs[*i]).into())
        }
        "DESIGN PARAM" => {
            *i += 2;
            Some(format!("DESP{}", strs[*i]).into())
        }
        _ => {
            Some("".into())
        }
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
pub fn parse_to_f32(input: &[u8]) -> f32 {
    (f32::from_be_bytes(input.try_into().unwrap()) * 100.0).round() / 100.0
}

#[inline]
pub fn parse_to_f64(input: &[u8]) -> f64 {
    if let [a, b, c, d, e, f, g, h] = input[..8] {
        return (f64::from_be_bytes([e, f, g, h, a, b, c, d]) * 100.0).round() / 100.0;
    } else {
        return 0.0;
    }
}


#[inline]
pub fn convert_u32_to_noun(input: &[u8]) -> SmolStr {
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