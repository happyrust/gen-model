use std::collections::{BTreeMap, HashMap};
use std::{mem, panic};
use std::str::FromStr;
use std::sync::Arc;
use aios_core::parsed_data::*;
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::pdms_data::{AxisParam, ScomInfo};
use aios_core::pdms_types::RefU64;
use aios_core::tiny_expr::expr_eval::interp;
use aios_core::tool::float_tool::*;
use anyhow::anyhow;
use glam::{Mat3, Quat, Vec2, Vec3};
use itertools::any;
use nom::Parser;
use regex::{Captures, NoExpand, Regex};
use tokio::runtime::Runtime;
use crate::cata::direction_parse::parse_expr_to_dir;
use crate::cata::polish_notation::Stack;
use crate::cata::resolve::resolve_axis_param;
use crate::data_interface::interface::PdmsDataInterface;
use crate::aql_api::children::query_pre_or_next_node;

use super::resolve::CataContext;

#[test]
fn test_exp() {
    let input_exp = "PARAM 1 2 TIMES SUM PARAM 1 IPARAM 1";
    // dbg!(input_exp.replace("PARAM 1", "test"));
    let s = "PARAM 1";
    let re = Regex::new(format!(r"^{s}|\s{s}").as_str()).unwrap();
    let rs = "test";
    let new_exp = re.replace_all(input_exp, format!(" {rs} ").as_str()).to_string();
    dbg!(new_exp);
}

#[test]
fn test_expression_regex() {
    let input_exp = "( ( ( -  DESP [1]/2 ) - DESP [2] - ATTRIB CPAR[3]  ) )";
    let new_exp = input_exp.replace("ATTRIB", "");
    let mut map = HashMap::new();

    map.insert("DESP1".to_string(), 1);
    map.insert("CPAR3".to_string(), 2);

    let re = Regex::new(r"(DESIGN?\s+)?([I|C|O)]?PARAM?)\s*(\d+)").unwrap();
    let input_exp = "DESIGN PARAM 1";
    for cap in re.captures_iter(&input_exp) {
        println!("{} {} {}", &cap[1], &cap[2], &cap[3]);
    }
    let input_exp = "CPARAM 1";
    if let Some(caps) = re.captures(&input_exp) {
        println!("{} {} {}", caps.get(1).map_or("", |m| m.as_str()), caps.get(2).map_or("", |m| m.as_str()),
                 caps.get(3).map_or("", |m| m.as_str()));
    }


    let input_exp = "DESIGN IPARA 1";
    for cap in re.captures_iter(&input_exp) {
        println!("{} {} {}", &cap[1], &cap[2], &cap[3]);
    }

    let input_exp = "( ATTRIB PARA[3] * TAN (  ANGL [2]/2 ) )";
    let input_exp = "( ATTRIB PARA[3] * TAN ( ATTRIB ANGL/2 ) )";
    // let input_exp = "TANF PARAM 3 DDANGLE";
    let new_exp = input_exp.replace("ATTRIB", "");
    let re = Regex::new(r"([A-Z]+[0-9]*)(\s*\[(\d+)\])?").unwrap();
    println!("Test :{input_exp}");
    for caps in re.captures_iter(&new_exp) {
        let c1 = caps.get(1).map_or("", |m| m.as_str());
        let c2 = caps.get(2).map_or("", |m| m.as_str());
        let c3 = caps.get(3).map_or("", |m| m.as_str());
        println!("{} {}", c1, c3);
    }
}

pub fn eval_str_to_f32<T: PdmsDataInterface>(input_expr: impl AsRef<str>,
                                             context: &CataContext, interface: Option<&T>) -> anyhow::Result<f32> {
    let input_expr = input_expr.as_ref().trim().to_uppercase();
    eval_str_to_f64(&input_expr, context, interface, true).map(|x| x as f32)
}

pub fn eval_str_to_f32_or_default<T: PdmsDataInterface>(input_expr: impl AsRef<str>,
                                                        context: &CataContext, interface: Option<&T>) -> f32 {
    eval_str_to_f32(input_expr, context, interface).unwrap_or(0.0)
}

//  SIN  00 00 03 85
//  COS  00 00 03 86
//  TAN  00 00 03 87
//  ASIN 00 00 03 88
//  ACOS 00 00 03 89
//  ATAN 00 00 03 8A
//  ATAN 00 00 03 8B //这是两个值
//
//  SQRT 00 00 03 E9
//  POW  00 00 03 EA
//  LOG  00 00 03 EB
//  ALOG 00 00 03 EC
//  INT  00 00 03 ED
//  NINT 00 00 03 EE
//  ABS  00 00 03 EF
//  MAX  00 00 03 F0
//  MIN  00 00 03 F1


pub const INTERNAL_PDMS_EXPRESS: [&'static str; 22] = [
    "MAX", "MIN", "COS", "SIN", "LOG", "ABS", "POW", "SQR", "NOT", "AND", "OR",
    "ATAN", "ACOS", "ATAN2", "ASIN", "INT", "OF", "MOD", "NEGATE", "SUM", "TANF", "TAN",
];


///评估表达式的值
pub fn eval_str_to_f64<T: PdmsDataInterface>(input_expr: &str,
                                             context: &CataContext,
                                             interface: Option<&T>,
                                             replace_err_by_zero: bool) -> anyhow::Result<f64> {
    if input_expr.is_empty() || input_expr == "UNSET" {
        return Ok(0.0);
    }
    //处理引用的情况 OF 的情况, 如果需要获取 att value，还是需要用数据库去获取值
    let mut new_exp = input_expr.replace("ATTRIB", "");
    if input_expr.contains(" OF ") {
        // // dbg!(&input_expr);
        let re = Regex::new(r"([A-Z\s]+) OF (PREV|NEXT|\d+/\d+)").unwrap();
        let interface = interface.ok_or(anyhow::anyhow!("unknown interface"))?;
        for caps in re.captures_iter(&input_expr) {
            let s = &caps[0];
            let c1 = caps.get(1).map_or("", |m| m.as_str());
            let c2 = caps.get(2).map_or("", |m| m.as_str());
            let refno_str = context.get("RS_DES_REFNO").unwrap().as_str();
            let refno = RefU64::from_refno_str(refno_str)?;
            let target_refno =
                match c2 {
                    "PREV" => interface.get_prev(refno)?,
                    "NEXT" => interface.get_next(refno)?,
                    refno_str => RefU64::from_str(refno_str).map_err(|_| anyhow!("wrong refno in of expr"))?
                };
            let att = interface.get_attr_from_localdb(target_refno)?;
            dbg!(&target_refno);
            if let Some(value) = att.get_as_string(c1) {
                new_exp = new_exp.replace(s, value.as_str());
            } else if let Some(v) = context.get(c1) {
                new_exp = new_exp.replace(s, v.as_str());
                dbg!(&new_exp);
            } else{
                // match c1 {
                //     _ => {
                //         return Err(anyhow!("wrong  of expression"));
                //     }
                // }
            }
            //     //是不是需要求解的属性, 比如 LBORE
            //     let value = match c1 {
            //         // "LBORE" => {
            //         //     //PRE
            //         //     //判断 cat_ref 是否是同一个
            //         //     // let cat_ref =
            //         // }
            //         _ => {
            //             // ref_att.get_as_string(c1).unwrap_or("DESP[1]".to_string())
            //             "DESP[1]".to_string()
            //         }
            //     };

        }
    }

    //说明：匹配带小数的情况 PARA[1.1]
    let re = Regex::new(r"(:?[A-Z_]+[0-9]*)(\s*\[?\s*(([1-9]\d*\.?\d*)|(0\.\d*[1-9]))\s*\]?)?").unwrap();
    // 将NEXT PREV 的值统一换成参考号，然后 context_params 要存储 参考号对应的 attr，要是它这个值没有求解，
    // 相当于要递归去求值
    let rpro_re = Regex::new(r"(RPRO)\s+([a-zA-Z0-9]+)").unwrap();
    if new_exp.contains("RPRO") {
        new_exp = rpro_re.replace_all(&new_exp, |caps: &Captures| {
            let key: String = format!("{}_{}", &caps[1], &caps[2]).into();
            let default_key: String = format!("{}_{}_default_expr", &caps[1], &caps[2]).into();
            let v = context.get(&key).map(|x| x.to_string()).unwrap_or("0".to_string());
            if let Ok(t) = eval_str_to_f64(&v, &context, interface, false) {
                t.to_string()
            } else {
                context.get(&default_key).map(|x| x.to_string()).unwrap_or("0".to_string())
            }
        }).trim().to_string();
    }

    let mut new_exp = new_exp.replace("DESIGN PARAM", "DESP").replace("DESIGN PARA", "DESP");
    ;
    let mut result_exp = new_exp.clone();
    //默认两次
    let mut found_replaced = false;
    let para_name_re = Regex::new(r"(DESI(GN)?\s+)?([I|C|O|A)]?PARA?M?)|DESP|(O|A|W|D)DESP?").unwrap();
    for _ in 0..100 {
        for caps in re.captures_iter(&new_exp) {
            let s = caps[0].trim();
            if INTERNAL_PDMS_EXPRESS.contains(&s) {
                continue;
            }
            let mut para_name = caps.get(1).map_or("", |m| m.as_str());
            let c2 = caps.get(2).map_or("", |m| m.as_str());
            let c3 = caps.get(3).map_or("", |m| m.as_str());

            //处理掉PARA 和 PARAM的区别
            let is_some_param = para_name_re.is_match(para_name);
            if is_some_param {
                if para_name.ends_with("M") {
                    para_name = &para_name[0..para_name.len() - 1];
                }
            }
            // 小数向下取整
            let mut k: String = format!("{}{}", para_name, c3.parse::<f32>().map(|x| x.floor().to_string()).unwrap_or_default()).into();

            if context.contains_key(&k) {
                result_exp = result_exp.replace(s, &context[&k]);
                found_replaced = true;
            } else if is_some_param {
                if !replace_err_by_zero {
                    return Err(anyhow::anyhow!(format!("{input_expr}： {} not found.", &k)));
                }
                println!("{input_expr}： {} not found, use 0.", &k);
                result_exp = result_exp.replace(s, " 0");
                found_replaced = true;
            }
        }
        //如果有RPRO 需要执行两次处理
        result_exp = result_exp.replace("ATTRIB", "");
        if result_exp.contains("RPRO") {
            result_exp = rpro_re.replace_all(&result_exp, |caps: &Captures| {
                let key: String = format!("{}_{}", &caps[1], &caps[2]).into();
                let default_key: String = format!("{}_{}_default_expr", &caps[1], &caps[2]).into();

                context.get(&key).map(|x| x.to_string()).unwrap_or(
                    context.get(&default_key).map(|x| x.to_string()).unwrap_or("0".to_string())
                )
            }).trim().to_string();
            found_replaced = true;
        }
        // dbg!(&result_exp);
        new_exp = result_exp.clone();
        if !found_replaced {
            break;
        }
        found_replaced = false;
    }
    // dbg!(&result_exp);
    let seg_strs: Vec<String> = result_exp.split_whitespace().map(|x| x.trim().into()).collect::<Vec<_>>();
    if seg_strs.len() == 0 {
        return Ok(0.0);
    }
    let mut result_string = String::new();
    let mut p_vals = vec![];
    for s in seg_strs {
        let upper_s = s.to_uppercase();
        match upper_s.as_str() {
            "TIMES" | "MULT" => p_vals.push("*".to_string()),
            "DIV" => p_vals.push("/".to_string()),
            "DDHEIGHT" => p_vals.push(context["DDHEIGHT"].to_string()),
            "DDRADIUS" => p_vals.push(context["DDRADIUS"].to_string()),
            "DDANGLE" => p_vals.push(context["DDANGLE"].to_string()),
            _ => {
                if upper_s.ends_with("mm") {
                    p_vals.push(upper_s[..upper_s.len() - 2].to_string());
                } else {
                    p_vals.push(upper_s.to_string())
                }
            }
        }
    }
    let mut i = 0;
    let mut new_vals = vec![];
    while i < p_vals.len() {
        if p_vals[i] == "TWICE" {
            if i + 1 < p_vals.len() {
                if let Ok(val) = p_vals[i + 1].parse::<f64>() {
                    let v = val * 2.0f64;
                    new_vals.push(v.to_string());
                }
            }
            i += 2;
        } else if p_vals[i] == "TANF" {
            if i + 2 < p_vals.len() {
                if let Ok(val) = p_vals[i + 1].parse::<f64>() {
                    if let Ok(angle) = p_vals[i + 2].parse::<f64>() {
                        {
                            let v = val * ((angle / 2.0).to_radians() as f64).tan();
                            new_vals.push(v.to_string());
                        }
                    }
                }
            }
            i += 3;
        } else {
            new_vals.push(p_vals[i].clone());
            i += 1;
        }
    }
    let mut i = 0;
    while i < new_vals.len() {
        if (new_vals[i] == "SUM" || new_vals[i] == "DIFFERENCE") && i < new_vals.len() - 2 {
            if new_vals[i] == "SUM" {
                result_string.push_str(&format!(
                    "({} {} {})",
                    new_vals[i + 1],
                    "+",
                    new_vals[i + 2]
                ));
            } else {
                result_string.push_str(&format!(
                    "({} {} {})",
                    new_vals[i + 1],
                    "-",
                    new_vals[i + 2]
                ));
            }
            i += 3;
        } else {
            result_string.push_str(new_vals[i].as_str());
            i += 1;
        }
        result_string.push_str(" ");
    }
    match interp(&result_string.to_lowercase()) {
        Ok(val) => {
            Ok(f64_round_3(val).into())
        }
        Err(_) => {
            return if let Ok(mut stack) = Stack::init(&result_string) {
                stack.eval().ok_or(anyhow::anyhow!(format!("后缀表达式求解失败 {}", &input_expr)))
            } else {
                // println!("输入表达式 : {}", &input_expr);
                // dbg!(&context);
                // println!("计算后表达式 : {}", &result_string);
                Err(anyhow::anyhow!(format!("求解失败 {}", &input_expr)))
            };
        }
    }
}

/// 解析成不同的几何体参数
pub fn resolve_to_cate_geo_params(gmse: &GmseParamData) -> anyhow::Result<CateGeoParam> {
    let geo = panic::catch_unwind(|| {
        match &gmse.type_name[..] {
            "SANN" => {
                CateGeoParam::Profile(CateProfileParam::SANN(SannData {
                    xy: Vec2::new(gmse.verts[0][0], gmse.verts[0][1]),
                    dxy: Vec2::new(gmse.dxy[0][0], gmse.dxy[0][1]),
                    paxis: (gmse.paxises[0].clone()),
                    pangle: gmse.pang as f32,
                    pradius: gmse.prad as f32,
                    pwidth: gmse.pwid as f32,
                    drad: gmse.drad as f32,
                    dwid: gmse.dwid as f32,
                    plin_pos: gmse.plin_pos,
                    plin_axis: gmse.plin_plax,
                }))
            }
            "SPRO" => {   //structural profile
                CateGeoParam::Profile(CateProfileParam::SPRO(SProfileData {
                    verts: gmse.verts.iter().map(|x| x.truncate()).collect(),
                    frads: gmse.frads.clone(),
                    normal_axis: gmse.paxises[0].as_ref().map(|x| x.dir).unwrap_or(Vec3::Z),
                    plin_pos: gmse.plin_pos,
                    plin_axis: gmse.plin_plax,
                }))
            }
            "SREC" => {   //structural profile
                CateGeoParam::Profile(CateProfileParam::SREC(SRectData {
                    center: Vec2::new(gmse.xyz[0], gmse.xyz[1]),
                    size: Vec2::new(gmse.lengths[0], gmse.lengths[1]),
                    dxy: gmse.dxy[0],
                    normal_axis: gmse.paxises[0].as_ref().map(|x| x.dir).unwrap_or(Vec3::Z),
                    plin_pos: gmse.plin_pos,
                    plin_axis: gmse.plin_plax,
                }))
            }
            "BOXI" => {
                CateGeoParam::BoxImplied(CateBoxImpliedParam {
                    axis: None,
                    width: gmse.lengths[2],
                    height: gmse.lengths[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "LCYL" | "NLCY" => {
                // 圆柱体
                CateGeoParam::LCylinder(CateLCylinderParam {
                    refno: gmse.refno,
                    axis: (gmse.paxises[0].clone()),
                    dist_to_btm: gmse.distances[1],
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                    dist_to_top: gmse.distances[2],
                })
            }
            "NSCY" | "SCYL" => {
                // 圆柱体
                CateGeoParam::SCylinder(CateSCylinderParam {
                    refno: gmse.refno,
                    axis: (gmse.paxises[0].clone()),
                    dist_to_btm: gmse.distances[0],
                    height: gmse.phei,
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "LINE" => {
                CateGeoParam::Line(CateLineParam {
                    refno: gmse.refno,
                    pa: (gmse.paxises[0].clone()),
                    pb: (gmse.paxises[1].clone()),
                    diameter: 0.0, //gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "LPYR" | "NLPY" => {
                CateGeoParam::Pyramid(CatePyramidParam {
                    refno: gmse.refno,
                    pa: (gmse.paxises[0].clone()),
                    pb: (gmse.paxises[1].clone()),
                    pc: (gmse.paxises[2].clone()),
                    x_bottom: gmse.xyz[3],
                    y_bottom: gmse.xyz[4],
                    x_top: gmse.xyz[5],
                    y_top: gmse.xyz[6],
                    dist_to_btm: gmse.distances[1],
                    dist_to_top: gmse.distances[2],
                    x_offset: gmse.xyz[7],
                    y_offset: gmse.xyz[8],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SSLC" | "NSSL" => {
                if gmse.paxises.len() >= 1 && gmse.diameters.len() >= 1 && gmse.shears.len() >= 4 {
                    CateGeoParam::SlopeBottomCylinder(CateSlopeBottomCylinderParam {
                        refno: gmse.refno,
                        axis: (gmse.paxises[0].clone()),
                        height: gmse.phei,
                        diameter: gmse.diameters[0],
                        dist_to_btm: gmse.distances[0],
                        x_shear: gmse.shears[0],
                        y_shear: gmse.shears[1],
                        alt_x_shear: gmse.shears[2],
                        alt_y_shear: gmse.shears[3],
                        centre_line_flag: gmse.centre_line_flag,
                        tube_flag: gmse.tube_flag,
                    })
                } else {
                    CateGeoParam::Unknown
                }
            }
            "LSNO" | "NLSN" => {
                if gmse.paxises.len() >= 2 && gmse.diameters.len() >= 2 && gmse.distances.len() >= 2 {
                    CateGeoParam::Snout(CateSnoutParam {
                        refno: gmse.refno,
                        pa: (gmse.paxises[0].clone()),
                        pb: (gmse.paxises[1].clone()),
                        dist_to_btm: gmse.distances[1],
                        dist_to_top: gmse.distances[2],
                        btm_diameter: gmse.diameters[1],
                        top_diameter: gmse.diameters[2],
                        offset: gmse.offset,
                        centre_line_flag: gmse.centre_line_flag,
                        tube_flag: gmse.tube_flag,
                    })
                } else {
                    CateGeoParam::Unknown
                }
            }
            "SBOX" | "NSBO" => {
                if gmse.lengths.len() >= 3 && gmse.xyz.len() >= 3 {
                    CateGeoParam::Box(CateBoxParam {
                        refno: gmse.refno,
                        size: Vec3::new(
                            gmse.lengths[0],
                            gmse.lengths[1],
                            gmse.lengths[2],
                        ),
                        offset: Vec3::new(
                            gmse.xyz[0],
                            gmse.xyz[1],
                            gmse.xyz[2],
                        ),
                        centre_line_flag: gmse.centre_line_flag,
                        tube_flag: gmse.tube_flag,
                    })
                } else {
                    CateGeoParam::Unknown
                }
            }
            "SCON" | "NSCO" => {
                // 圆锥
                CateGeoParam::Cone(CateSnoutParam {
                    refno: gmse.refno,
                    // axis: (gmse.paxises[0].clone()),
                    dist_to_btm: 0.0,
                    // diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                    pa: gmse.paxises[0].clone(),
                    pb: None,
                    dist_to_top: gmse.distances[0],
                    btm_diameter: 0.0,
                    top_diameter: gmse.diameters[0],
                    offset: 0.0,
                })
            }
            "SCTO" | "NSCT" => {
                // 弯管
                CateGeoParam::Torus(CateTorusParam {
                    refno: gmse.refno,
                    pa: (gmse.paxises[0].clone()),
                    pb: (gmse.paxises[1].clone()),
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SDSH" | "NSDS" => {
                CateGeoParam::Dish(CateDishParam {
                    refno: gmse.refno,
                    axis: (gmse.paxises[0].clone()),
                    dist_to_btm: gmse.distances[0],
                    height: gmse.phei,
                    diameter: gmse.diameters[0],
                    radius: gmse.prad,
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SEXT" | "NSEX" => {
                // dbg!(gmse);
                CateGeoParam::Extrusion(CateExtrusionParam {
                    refno: gmse.refno,
                    pa: (gmse.paxises[0].clone()),
                    pb: (gmse.paxises[1].clone()),
                    height: gmse.phei,
                    x: gmse.xyz[0],
                    y: gmse.xyz[1],
                    z: gmse.xyz[2],
                    verts: gmse.verts.clone(),
                    frads: gmse.frads.clone(),
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SLINE" => {
                CateGeoParam::Sline(CateSplineParam {
                    refno: gmse.refno,
                    start_pt: vec![0.0; 3],
                    end_pt: vec![0.0; 3],
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SREV" | "NSRE" => {
                CateGeoParam::Revolution(CateRevolutionParam {
                    refno: gmse.refno,
                    pa: (gmse.paxises[0].clone()),
                    pb: (gmse.paxises[1].clone()),
                    angle: gmse.pang,
                    verts: gmse.verts.clone(),
                    frads: gmse.frads.clone(),
                    x: gmse.xyz[0],
                    y: gmse.xyz[1],
                    z: gmse.xyz[2],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SRTO" | "NSRT" => {
                // 截面为矩形的弯管
                CateGeoParam::RectTorus(CateRectTorusParam {
                    refno: gmse.refno,
                    pa: (gmse.paxises[0].clone()),
                    pb: (gmse.paxises[1].clone()),
                    height: gmse.phei,
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SSPH" | "NSSP" => {
                // dbg!(&gmse);
                // 球
                CateGeoParam::Sphere(CateSphereParam {
                    refno: gmse.refno,
                    axis: (gmse.paxises[0].clone()),
                    dist_to_center: gmse.distances[0],
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "TUBE" => {
                CateGeoParam::TubeImplied(CateTubeImpliedParam {
                    axis: None,
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            _ => CateGeoParam::Unknown,
        }
    });
    // Ok(geo.expect(&format!("几何体生成出错, 数据: {:?}", &gmse)))
    geo.map_err(|x| anyhow::anyhow!(format!("几何体生成出错, 数据: {:?}", &gmse)))
}

pub fn resolve_dir_and_pos<T: PdmsDataInterface>(axis: &AxisParam,
                                                 scom: &ScomInfo,
                                                 context: &CataContext,
                                                 interface: Option<&T>) -> anyhow::Result<(Vec3, Vec3, Vec3)> {
    let mut dir_str = axis.direction.trim();
    let mut ref_dir_str = axis.ref_direction.trim();
    let mut dir = Vec3::ZERO;
    let mut ref_dir = Vec3::ZERO;
    let mut pos = Vec3::ZERO;

    let re = Regex::new(r"^(-?)P(\d+)$").unwrap();
    if re.is_match(dir_str) {
        if let Some(cap) = re.captures(dir_str) {
            let is_neg = cap.get(1).map_or("", |m| m.as_str()) == "-";
            let pnt_indx = cap.get(2).map_or("", |m| m.as_str()).parse::<i32>().unwrap_or(-1);
            if let Some(indx) = scom.axis_param_numbers.iter().position(|&x| x == pnt_indx) {
                let mut axis = resolve_axis_param(&scom.axis_params[indx], scom, context, interface)?;
                let flag = if is_neg { -1.0 } else { 1.0 };
                dir = flag * mem::take(&mut axis.dir);
                pos = flag * mem::take(&mut axis.pt);
            } else {
                return Err(anyhow::anyhow!("未找到点索引: {}", pnt_indx));
            }
        }
    } else {
        dir = parse_str_axis_to_vec3(dir_str, context, interface).unwrap_or(Vec3::Z);
    }

    if re.is_match(ref_dir_str) {
        if let Some(cap) = re.captures(ref_dir_str) {
            let is_neg = cap.get(1).map_or("", |m| m.as_str()) == "-";
            let pnt_indx = cap.get(2).map_or("", |m| m.as_str()).parse::<i32>().unwrap_or(-1);
            if let Some(indx) = scom.axis_param_numbers.iter().position(|&x| x == pnt_indx) {
                let mut axis = resolve_axis_param(&scom.axis_params[indx], scom, context, interface)?;
                let flag = if is_neg { -1.0 } else { 1.0 };
                ref_dir = flag * mem::take(&mut axis.dir);
            } else {
                return Err(anyhow::anyhow!("未找到点索引: {}", pnt_indx));
            }
        }
    } else {
        //unset 不存在 ref dir的情况
        ref_dir = parse_str_axis_to_vec3(ref_dir_str, context, interface).unwrap_or(Vec3::Y);
    }

    return Ok((dir, ref_dir, pos));
}

//Y is N and Z is U
pub fn parse_ori_str_to_quat<T: PdmsDataInterface>(ori_str: &str, context: &CataContext, interface: Option<&T>) -> anyhow::Result<Quat> {
    let dir_strs = ori_str.split(" and ").collect::<Vec<_>>();
    // dbg!(&dir_strs);
    if dir_strs.len() < 2 {
        return Err(anyhow::anyhow!("不是方位字符串"));
    };
    let mut mat = Mat3::IDENTITY;
    let mut comb_dir_str = String::new();
    for i in 0..2 {
        let d = dir_strs[i].trim();
        let strs = d.split("is").collect::<Vec<_>>();
        // dbg!(&strs);
        if strs.len() != 2 {
            return Err(anyhow::anyhow!("不是方位字符串"));
        }

        // dbg!(d.chars().next().unwrap());
        let f = strs[0].trim().to_uppercase();
        // dbg!(&f);

        let dir_str = strs[1].trim()
            .replace("E", "X")
            .replace("W", "-X")
            .replace("N", "Y")
            .replace("S", "-Y")
            .replace("U", "Z")
            .replace("D", "-Z");
        // dbg!(&dir_str);
        let dir = parse_str_axis_to_vec3(&dir_str, context, interface)?;
        // dbg!(dir);
        comb_dir_str.push_str(f.as_str());
        match f.as_str() {
            "X" => mat.x_axis = dir,
            "Y" => mat.y_axis = dir,
            "Z" => mat.z_axis = dir,
            _ => {}
        }
    }

    match comb_dir_str.as_str() {
        "XY" => mat.z_axis = mat.x_axis.cross(mat.y_axis).normalize_or_zero(),
        "YZ" => mat.x_axis = mat.y_axis.cross(mat.z_axis).normalize_or_zero(),
        "XZ" => mat.y_axis = mat.z_axis.cross(mat.x_axis).normalize_or_zero(),
        _ => {}
    }

    dbg!(&mat);

    Ok(Quat::from_mat3(&mat))
}

pub fn parse_str_axis_to_vec3<T: PdmsDataInterface>(pdir: &str, context: &CataContext, interface: Option<&T>) -> anyhow::Result<Vec3> {
    let dir_str = pdir.to_uppercase().replace("AXIS", "");
    let re = Regex::new(r"^(-?[X|Y|Z])$").unwrap();
    let mut new_dir_str = dir_str.clone();
    let mut not_single = false;
    if !re.is_match(&dir_str) {
        not_single = true;
        let mut is_three = false;

        let re = Regex::new(r"(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])").unwrap();
        for cap in re.captures_iter(&dir_str) {
            if cap.len() == 6 {
                let val_str = cap[2].to_string();
                let val_result = eval_str_to_f64(&val_str, context, interface, true)?.to_string();
                new_dir_str = dir_str.replace(&val_str, &val_result);

                let val_str = cap[4].to_string();
                let val_result = eval_str_to_f64(&val_str, context, interface, true)?.to_string();
                new_dir_str = new_dir_str.replace(&val_str, &val_result);
                is_three = true;
            }
        }

        if !is_three {
            // dbg!(is_three);
            let re = Regex::new(r"(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])").unwrap();
            for cap in re.captures_iter(&dir_str) {
                if cap.len() == 4 {
                    let val_str = cap[2].to_string();
                    // dbg!(&val_str);
                    let val_result = eval_str_to_f64(&val_str, context, interface, true).unwrap_or_default().to_string();
                    new_dir_str = dir_str.replace(&val_str, &val_result);
                    // dbg!(&new_dir_str);
                }
            }
        }
    }
    let dir_str = new_dir_str.replace(" ", "");
    let v = parse_expr_to_dir(&dir_str).ok_or(anyhow::anyhow!(format!("方向字符串: {} 不正确。", pdir)))?;
    Ok(v)
}


