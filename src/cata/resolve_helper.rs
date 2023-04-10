use std::collections::{BTreeMap, HashMap};
use std::{mem, panic};
use aios_core::parsed_data::*;
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::pdms_data::{AxisParam, ScomInfo};
use aios_core::tiny_expr::expr_eval::interp;
use aios_core::tool::float_tool::*;
use anyhow::anyhow;
use glam::{Vec2, Vec3};
use itertools::any;
use nom::Parser;
use regex::{Captures, NoExpand, Regex};
use smol_str::SmolStr;
use crate::cata::direction_parse::parse_expr_to_dir;
use crate::cata::polish_notation::Stack;
use crate::cata::resolve::resolve_axis_param;

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

pub fn eval_str_to_f32(input_expr: &str, context: &BTreeMap<SmolStr, SmolStr>) -> anyhow::Result<f32> {
    eval_str_to_f64(input_expr, context).map(|x| x as f32)
}

///评估表达式的值
pub fn eval_str_to_f64(input_expr: &str, context: &BTreeMap<SmolStr, SmolStr>) -> anyhow::Result<f64> {
    let input_expr = input_expr.trim().to_uppercase();
    if input_expr.is_empty() || input_expr == "UNSET" {
        return Ok(0.0);
    }
    //处理简单情况
    if let Ok(val) = interp(&input_expr.to_lowercase()) {
        return Ok(f64_round_3(val).into());
    }
    let re = Regex::new(r"([A-Z_]+[0-9]*)(\s*\[\s*(\d+)\s*\])?").unwrap();
    let mut new_exp = input_expr.replace("ATTRIB", "");
    let rpro_re = Regex::new(r"(RPRO)\s+(\S+)").unwrap();
    if new_exp.contains("RPRO") {
        new_exp = rpro_re.replace_all(&new_exp, |caps: &Captures| {
            format!("{}_{}", &caps[1], &caps[2])
        }).trim().to_string();
    }
    let mut result_exp = new_exp.clone();
    // dbg!(&result_exp);
    //默认两次
    let mut found_replaced = false;
    for _ in 0..5 {
        for caps in re.captures_iter(&new_exp) {
            let s = &caps[0];
            let c1 = caps.get(1).map_or("", |m| m.as_str());
            let c2 = caps.get(2).map_or("", |m| m.as_str());
            let c3 = caps.get(3).map_or("", |m| m.as_str());
            // println!("{} {}", c1, c3);
            let k: SmolStr = format!("{}{}", c1, c3).into();
            if context.contains_key(&k) {
                result_exp = result_exp.replace(s, &context[&k]);
                found_replaced = true;
                // dbg!(&result_exp);
            } else if c1 == "DESI" || c1 == "DESP" {
                result_exp = result_exp.replace(s, "0.0");
            }
        }
        //如果有RPRO 需要执行两次处理
        result_exp = result_exp.replace("ATTRIB", "");
        if result_exp.contains("RPRO") {
            result_exp = rpro_re.replace_all(&result_exp, |caps: &Captures| {
                format!("{}_{}", &caps[1], &caps[2])
            }).trim().to_string();
        }
        new_exp = result_exp.clone();
        if !found_replaced {
            break;
        }
        found_replaced = false;
    }
    //因为 attrib 的原因，这里还需要再执行一遍处理，以防止有可能出现
    //处理出现 DESIGN IPARA 1 这种没有 “[]”的情况
    // dbg!(&result_exp);
    let re = Regex::new(r"(DESIGN?\s+)?([I|C|O|A)]?PARAM?)\s*(\d+)").unwrap();
    let mut new_exp = result_exp.clone();
    for caps in re.captures_iter(&result_exp) {
        let s = &caps[0];
        let c1 = caps.get(1).map_or("", |m| m.as_str());
        let c2 = caps.get(2).map_or("", |m| m.as_str());
        let c3 = caps.get(3).map_or("", |m| m.as_str());
        let mut k = SmolStr::new("");
        if c1.starts_with("DESIGN") {
            k = format!("DESI{}", c3).into();  //design's params
        } else {
            if c2.starts_with("IPAR") {
                k = format!("IPARA{}", c3).into();
            } else if c2.starts_with("CPAR") {
                k = format!("IPARA{}", c3).into();
            } else if c2.starts_with("PARA") || c2.starts_with("APAR") {
                k = format!("PARA{}", c3).into();
            } else if c2.starts_with("OPAR") {
                k = format!("OPAR{}", c3).into();
            } else if c2.starts_with("DDES") || c2.starts_with("ADES") {
                k = format!("DESI{}", c3).into();
            } else if c2.starts_with("ODES") || c2.starts_with("WDES") {
                k = format!("DESI{}", c3).into();
            }
        }
        if context.contains_key(&k) {
            //need to replace whole word
            let re = Regex::new(format!(r"^{s}|\s{s}").as_str()).unwrap();
            let rs = if context.contains_key(&k) { &*context[&k] } else { "0.0" };
            new_exp = re.replace_all(&new_exp, format!(" {rs} ").as_str()).to_string();
        }
    }
    let seg_strs: Vec<SmolStr> = new_exp.split_whitespace().map(|x| x.trim().into()).collect::<Vec<_>>();
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
                stack.eval().ok_or(anyhow!(format!("后缀表达式求解失败 {}", &input_expr)))
            } else {
                dbg!(&input_expr);
                dbg!(&result_string);
                Err(anyhow!(format!("求解失败 {}", &input_expr)))
            }
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
                    paxis: Some(gmse.paxises[0].clone()),
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
                    verts: gmse.verts.clone(),
                    frads: gmse.frads.clone(),
                    normal_axis: Vec3::from(gmse.paxises[0].dir),
                    plin_pos: gmse.plin_pos,
                    plin_axis: gmse.plin_plax,
                }))
            }
            "BOXI" => {
                let z_length = if gmse.box_lengths.len() >= 3 {
                    gmse.box_lengths[2]
                } else {
                    gmse.box_lengths[1]
                };
                CateGeoParam::Boxi(CateBoxImpliedParam {
                    axis: Some(gmse.paxises[0].clone()),
                    x_length: gmse.box_lengths[0],
                    z_length,
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "LCYL" => {
                // 圆柱体
                CateGeoParam::LCylinder(CateLCylinderParam {
                    refno: gmse.refno,
                    axis: Some(gmse.paxises[0].clone()),
                    dist_to_btm: gmse.distances[0],
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                    dist_to_top: gmse.distances[1],
                })
            }
           "NSCY" | "SCYL" => {
                // 圆柱体
                CateGeoParam::SCylinder(CateSCylinderParam {
                    refno: gmse.refno,
                    axis: Some(gmse.paxises[0].clone()),
                    dist_to_btm: gmse.distances[0],
                    height: gmse.phei,
                    diameter: gmse.diameters.get(0).map(|x| *x).unwrap_or_default(),
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "LINE" => {
                CateGeoParam::Line(CateLineParam {
                    refno: gmse.refno,
                    pa: Some(gmse.paxises[0].clone()),
                    pb: Some(gmse.paxises[1].clone()),
                    diameter: 0.0, //gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "LPYR" => {
                CateGeoParam::Pyramid(CatePyramidParam {
                    refno: gmse.refno,
                    pa: Some(gmse.paxises[0].clone()),
                    pb: Some(gmse.paxises[1].clone()),
                    pc: Some(gmse.paxises[2].clone()),
                    x_bottom: gmse.xyz[0],
                    y_bottom: gmse.xyz[1],
                    x_top: gmse.xyz[2],
                    y_top: gmse.xyz[3],
                    dist_to_btm: gmse.distances[0],
                    dist_to_top: gmse.distances[1],
                    x_offset: gmse.xyz[4],
                    y_offset: gmse.xyz[5],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SSLC" => {
                if gmse.paxises.len() >= 1 && gmse.diameters.len() >= 1 && gmse.shears.len() >= 4 {
                    CateGeoParam::SlopeBottomCylinder(CateSlopeBottomCylinderParam {
                        refno: gmse.refno,
                        axis: Some(gmse.paxises[0].clone()),
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
            "LSNO" => {
                if gmse.paxises.len() >= 2 && gmse.diameters.len() >= 2 && gmse.distances.len() >= 2 {
                    CateGeoParam::Snout(CateSnoutParam {
                        refno: gmse.refno,
                        pa: Some(gmse.paxises[0].clone()),
                        pb: Some(gmse.paxises[1].clone()),
                        dist_to_btm: gmse.distances[0],
                        dist_to_top: gmse.distances[1],
                        btm_diameter: gmse.diameters[0],
                        top_diameter: gmse.diameters[1],
                        offset: gmse.offset,
                        centre_line_flag: gmse.centre_line_flag,
                        tube_flag: gmse.tube_flag,
                    })
                } else {
                    CateGeoParam::Unknown
                }
            }
            "SBOX" => {
                if gmse.box_lengths.len() >= 3 && gmse.xyz.len() >= 3 {
                    CateGeoParam::Box(CateBoxParam {
                        refno: gmse.refno,
                        size: vec![
                            gmse.box_lengths[0],
                            gmse.box_lengths[1],
                            gmse.box_lengths[2],
                        ],
                        offset: vec![
                            gmse.xyz[0],
                            gmse.xyz[1],
                            gmse.xyz[2],
                        ],
                        centre_line_flag: gmse.centre_line_flag,
                        tube_flag: gmse.tube_flag,
                    })
                } else {
                    CateGeoParam::Unknown
                }
            }
            "SCON" => {
                // 圆锥
                CateGeoParam::Cone(CateConeParam {
                    refno: gmse.refno,
                    axis: Some(gmse.paxises[0].clone()),
                    dist_to_btm: gmse.distances[0],
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SCTO" => {
                // 弯管
                CateGeoParam::Torus(CateTorusParam {
                    refno: gmse.refno,
                    pa: Some(gmse.paxises[0].clone()),
                    pb: Some(gmse.paxises[1].clone()),
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            // "SDIS" => {
            // 圆片
            // Some(CateGeoParam::Disc(CateDiscParam {
            //     axis: Some(gmse.paxises[0].clone()),
            //     dist_to_btm: gmse.distances[0],
            //     diameter: gmse.diameters[0],
            //     centre_line_flag: gmse.centre_line_flag,
            //     tube_flag: gmse.tube_flag,
            // }))
            // }
            "SDSH" => {
                CateGeoParam::Dish(CateDishParam {
                    refno: gmse.refno,
                    axis: Some(gmse.paxises[0].clone()),
                    dist_to_btm: gmse.distances[0],
                    height: gmse.phei,
                    diameter: gmse.diameters[0],
                    radius: gmse.radius,
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SEXT" => {
                CateGeoParam::Extrusion(CateExtrusionParam {
                    refno: gmse.refno,
                    pa: Some(gmse.paxises[0].clone()),
                    pb: Some(gmse.paxises[1].clone()),
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
            "SREV" => {
                CateGeoParam::Revolution(CateRevolutionParam {
                    refno: gmse.refno,
                    pa: Some(gmse.paxises[0].clone()),
                    pb: Some(gmse.paxises[1].clone()),
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
            "SRTO" => {
                // 截面为矩形的弯管
                CateGeoParam::RectTorus(CateRectTorusParam {
                    refno: gmse.refno,
                    pa: Some(gmse.paxises[0].clone()),
                    pb: Some(gmse.paxises[1].clone()),
                    height: gmse.phei,
                    diameter: gmse.diameters[0],
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            // "SSLC" => {
            //todo
            // Some(CateGeoParam::SlopeBottomCylinder(CateSlopeBottomCylinderParam {
            //     axis: Some(gmse.paxises[0].clone()),
            //     height: gmse.phei,
            //     diameter: gmse.diameters[0],
            //     distance: gmse.distances[0],
            //     x_shear: 0.0,
            //     y_shear: 0.0,
            //     alt_x_shear: 0.0,
            //     alt_y_shear: 0.0,
            //     centre_line_flag: gmse.centre_line_flag,
            //     tube_flag: gmse.tube_flag,
            // }))
            // }
            "SSPH" => {
                // 球
                CateGeoParam::Sphere(CateSphereParam {
                    refno: gmse.refno,
                    axis: Some(gmse.paxises[0].clone()),
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
    geo.map_err(|x| anyhow!(format!("几何体生成出错, 数据: {:?}", &gmse)))
}

pub fn resolve_dir_and_pos(axis: &AxisParam,
                           scom: &ScomInfo,
                           context: &BTreeMap<SmolStr, SmolStr>) -> (Vec3, Vec3) {
    let mut dir_str = axis.direction.trim();
    let mut dir = Vec3::ZERO;
    let mut pos = Vec3::ZERO;

    let re = Regex::new(r"^P\d+$").unwrap();
    if re.is_match(dir_str) {
        let pnt_indx = dir_str[1..].parse::<i32>().unwrap_or(i32::MAX);
        if let Some(indx) = scom.axis_param_numbers.iter().position(|&x| x == pnt_indx) {
            if let Some(mut axis) = resolve_axis_param(&scom.axis_params[indx], scom, context) {
                dir = mem::take(&mut axis.dir);
                pos = mem::take(&mut axis.pt);
            }
        }
    } else {
        dir = parse_str_axis_to_vec3(dir_str, context).into();
    }
    return (dir, pos);
}

pub fn parse_str_axis_to_vec3(pdir: &str, context: &BTreeMap<SmolStr, SmolStr>) -> Vec3 {
    // dbg!(pdir);
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
                let val_result = eval_str_to_f64(&val_str, context).unwrap_or_default().to_string();
                new_dir_str = dir_str.replace(&val_str, &val_result);

                let val_str = cap[4].to_string();
                let val_result = eval_str_to_f64(&val_str, context).unwrap_or_default().to_string();
                new_dir_str = new_dir_str.replace(&val_str, &val_result);
                // dbg!(&new_dir_str);
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
                    let val_result = eval_str_to_f64(&val_str, context).unwrap_or_default().to_string();
                    new_dir_str = dir_str.replace(&val_str, &val_result);
                    // dbg!(&new_dir_str);
                }
            }
        }
    }
    // dbg!(&new_dir_str);
    let v = parse_expr_to_dir(&new_dir_str.replace(" ", ""));
    if not_single {
        // dbg!(&new_dir_str);
        // dbg!(&v);
    }
    Vec3::new(f32_round_2(v[0]), f32_round_2(v[1]), f32_round_2(v[2]))
}


#[test]
fn parse_3_axis() {
    // let str = "X ( 45 )  Y ( 35 ) Z";
    //-X (DESIGN PARAM 14 ) -Y
    let mut context = BTreeMap::new();
    context.insert("DESI14".into(), "30.0".into());
    context.insert("DESI13".into(), "30.0".into());
    context.insert("DDANGLE".into(), "45.0".into());
    context.insert("PARAM 2".into(), "30.0".into());
    context.insert("RPRO_CPAR".into(), "DESIGN PARAM 14".into());
    let str = "X ( RPRO_CPAR )  Y ( DESIGN PARAM 13 ) Z";
    // let str = "X ( DESIGN PARAM 14 )  Y ";
    let str = "X (60.0)  Y ";
    let str = "X ( 45 )  Y ( 35 ) Z";
    let str = "TANF PARAM 2 DDANGLE";
    let r = eval_str_to_f64(str, &context);
    dbg!(r);
}

//AXIS -Y ( ATAN ( ( DESP[2 ] / 2 + DESP[10 ] ) / ( DESP[3 ] / 2 - DESP[11 ] ) ) ) X
#[test]
fn parse_axis() {
    // let str = "X ( 45 )  Y ( 35 ) Z";
    //-X (DESIGN PARAM 14 ) -Y
    let mut context = BTreeMap::new();
    context.insert("DESP4".into(), "800.0".into());
    context.insert("DESP5".into(), "300.0".into());
    context.insert("DESP10".into(), "200.0".into());
    context.insert("DESP11".into(), "0.0".into());
    // context.insert("RPRO_CPAR".into(), "DESIGN PARAM 14".into());
    let str = "AXIS -Y ( ATAN ( ( DESP[2 ] / 2 + DESP[10 ] ) / ( DESP[3 ] / 2 - DESP[11 ] ) ) ) X";
    let r = parse_str_axis_to_vec3(str, &context);
    dbg!(r);
    //AXIS -Y ( ATANT ( 0 - DESP[10 ] - ( DESP[4 ] - DESP[5 ] ) / 2 , 0 - DESP[11 ] ) ) -X
    let str = "AXIS -Y (ATANT((DESP[10]-(DESP[4]-DESP[5])/2),(0-DESP[11]))) X";
    let r = parse_str_axis_to_vec3(str, &context);
    dbg!(r);
}


//[(.*[^-])([-?X|Y|Z])]?
#[test]
fn test_parse_dir() {
    let re = Regex::new(r"(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])").unwrap();
    let target = "-X (DESIGN PARAM 14 ) -Y";
    // let target = "-X";
    let target = target.trim();
    let target = "-X ( DESIGN PARAM 14 ) -Y ( DESIGN PARAM 19 ) -Z";

    // let re = Regex::new(r"(DESIGN?\s+)?([I|C|O)]?PARAM?)\s*(\d+)").unwrap();
    // let input_exp = "DESIGN PARAM 1";
    // dbg!(caps.into_iter().len());
    for cap in re.captures_iter(&target) {
        dbg!(cap.len());
        // dbg!(&cap[0]);
        dbg!(&cap[1]);
        dbg!(&cap[2]);
        dbg!(&cap[3]);
        dbg!(&cap[4]);
        dbg!(&cap[5]);
        // dbg!(&cap[4]);
        // println!("{} {} {} {}", &cap[1], &cap[2], &cap[3], &cap[4]);
    }
}

#[test]
fn test_rpro() {
    use regex::Captures;
    let s = "RPRO_TLEN";
    // let rpro_regex = Regex::new(r"RPRO\s*([A-Z]+[0-9]*)").unwrap();
    // let mut new_exp = rpro_regex.replace_all(&new_exp, "");
    // dbg!(new_exp);


    let re = Regex::new(r"([A-Z]+[0-9]*)(\s*\[(\d+)\])?").unwrap();
    for caps in re.captures_iter(s) {
        dbg!(&caps[0]);
    }

    let re = Regex::new(r"(RPRO)\s+(\S+)").unwrap();
    let result = re.replace(s, |caps: &Captures| {
        format!("{}_{}", &caps[1], &caps[2])
    });
    dbg!(result);
}

#[test]
fn test_math_exp() {
    let expr = "MAX ( ( ( - 31 ) + 60 ), 29.2 )";
    let context = BTreeMap::new();
    dbg!(eval_str_to_f64(expr, &context)).expect("TODO: panic message");
}
#[test]
fn test_interp() {
    let input_str = "((0.5*500*TAN(/2)+(500+2)*TAN(3/2)*COS(3))/2-((-(500/2+2)*TAN(3/2)+2*COS((90-3)))/2)";
    let result = interp(&input_str.to_lowercase()).unwrap();
    dbg!(&result);
}