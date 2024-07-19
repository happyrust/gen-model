use std::{mem, panic};
use std::collections::HashMap;
use aios_core::*;
use aios_core::parsed_data::*;
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::pdms_data::{AxisParam, ScomInfo};
use glam::{Mat3, Quat, Vec2, Vec3};
use nom::Parser;
use regex::Regex;
use aios_core::tool::direction_parse::parse_expr_to_dir;
use crate::cata::resolve::resolve_axis_param;
use aios_core::tool::parse_to_dir::parse_to_direction;

#[test]
fn test_exp() {
    let input_exp = "PARAM 1 2 TIMES SUM PARAM 1 IPARAM 1";
    // dbg!(input_exp.replace("PARAM 1", "test"));
    let s = "PARAM 1";
    let re = Regex::new(format!(r"^{s}|\s{s}").as_str()).unwrap();
    let rs = "test";
    let new_exp = re
        .replace_all(input_exp, format!(" {rs} ").as_str())
        .to_string();
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
        println!(
            "{} {} {}",
            caps.get(1).map_or("", |m| m.as_str()),
            caps.get(2).map_or("", |m| m.as_str()),
            caps.get(3).map_or("", |m| m.as_str())
        );
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
    "MAX", "MIN", "COS", "SIN", "LOG", "ABS", "POW", "SQR", "NOT", "AND", "OR", "ATAN", "ACOS",
    "ATAN2", "ASIN", "INT", "OF", "MOD", "NEGATE", "SUM", "TANF", "TAN",
];


/// 解析成不同的几何体参数
pub fn resolve_to_cate_geo_params(gmse: &GmseParamData) -> anyhow::Result<CateGeoParam> {
    let geo = panic::catch_unwind(|| {
        match &gmse.type_name[..] {
            "SANN" => CateGeoParam::Profile(CateProfileParam::SANN(SannData {
                refno: gmse.refno,
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
            })),
            "SPRO" => {
                CateGeoParam::Profile(CateProfileParam::SPRO(SProfileData {
                    refno: gmse.refno,
                    verts: gmse.verts.iter().map(|x| x.truncate()).collect(),
                    frads: gmse.frads.clone(),
                    normal_axis: gmse.paxises[0].as_ref().map(|x| x.dir).flatten().unwrap_or(Vec3::Z),
                    plin_pos: gmse.plin_pos,
                    plin_axis: gmse.plin_plax,
                }))
            }
            "SREC" => {
                CateGeoParam::Profile(CateProfileParam::SREC(SRectData {
                    refno: gmse.refno,
                    center: Vec2::new(gmse.xyz[0], gmse.xyz[1]),
                    size: Vec2::new(gmse.lengths[0], gmse.lengths[1]),
                    dxy: gmse.dxy[0],
                    normal_axis: gmse.paxises[0].as_ref().map(|x| x.dir).flatten().unwrap_or(Vec3::Z),
                    plin_pos: gmse.plin_pos,
                    plin_axis: gmse.plin_plax,
                }))
            }
            "BOXI" => CateGeoParam::BoxImplied(CateBoxImpliedParam {
                axis: None,
                width: gmse.lengths[2],
                height: gmse.lengths[0],
                centre_line_flag: gmse.centre_line_flag,
                tube_flag: gmse.tube_flag,
            }),
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
            "LPYR" | "NLPY" => CateGeoParam::Pyramid(CatePyramidParam {
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
            }),
            "SSLC" | "NSSL" => {
                if gmse.paxises.len() >= 1 && gmse.diameters.len() >= 1 && gmse.shears.len() >= 4 {
                    // dbg!(&gmse);
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
                if gmse.paxises.len() >= 2 && gmse.diameters.len() >= 2 && gmse.distances.len() >= 2
                {
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
                        size: Vec3::new(gmse.lengths[0], gmse.lengths[1], gmse.lengths[2]),
                        offset: Vec3::new(gmse.xyz[0], gmse.xyz[1], gmse.xyz[2]),
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
            "SDSH" | "NSDS" => CateGeoParam::Dish(CateDishParam {
                refno: gmse.refno,
                axis: (gmse.paxises[0].clone()),
                dist_to_btm: gmse.distances[0],
                height: gmse.phei,
                diameter: gmse.diameters[0],
                radius: gmse.prad,
                centre_line_flag: gmse.centre_line_flag,
                tube_flag: gmse.tube_flag,
            }),
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
                    verts: gmse.verts.iter().zip(gmse.frads.iter()).map(|(v, d)| {
                        Vec3::new(v[0], v[1], *d)
                    }).collect(),
                    centre_line_flag: gmse.centre_line_flag,
                    tube_flag: gmse.tube_flag,
                })
            }
            "SLINE" => CateGeoParam::Sline(CateSplineParam {
                refno: gmse.refno,
                start_pt: vec![0.0; 3],
                end_pt: vec![0.0; 3],
                diameter: gmse.diameters[0],
                centre_line_flag: gmse.centre_line_flag,
                tube_flag: gmse.tube_flag,
            }),
            "SREV" | "NSRE" => CateGeoParam::Revolution(CateRevolutionParam {
                refno: gmse.refno,
                pa: (gmse.paxises[0].clone()),
                pb: (gmse.paxises[1].clone()),
                angle: gmse.pang,
                verts: gmse.verts.iter().zip(gmse.frads.iter()).map(|(v, d)| {
                    Vec3::new(v[0], v[1], *d)
                }).collect(),
                x: gmse.xyz[0],
                y: gmse.xyz[1],
                z: gmse.xyz[2],
                centre_line_flag: gmse.centre_line_flag,
                tube_flag: gmse.tube_flag,
            }),
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
            "TUBE" => CateGeoParam::TubeImplied(CateTubeImpliedParam {
                axis: None,
                diameter: gmse.diameters[0],
                centre_line_flag: gmse.centre_line_flag,
                tube_flag: gmse.tube_flag,
            }),
            _ => CateGeoParam::Unknown,
        }
    });
    // Ok(geo.expect(&format!("几何体生成出错, 数据: {:?}", &gmse)))
    geo.map_err(|x| anyhow::anyhow!(format!("几何体生成出错, 数据: {:?}", &gmse)))
}

pub fn resolve_axis(
    axis: &AxisParam,
    scom: &ScomInfo,
    context: &CataContext,
) -> anyhow::Result<(Vec3, Vec3, Vec3)> {
    let mut dir_str = axis.direction.trim();
    let mut ref_dir_str = axis.ref_direction.trim();
    let mut dir = Vec3::ZERO;
    let mut ref_dir = Vec3::ZERO;
    let mut pos = Vec3::ZERO;
    let re = Regex::new(r"^(-?)P(\d+)$").unwrap();
    if re.is_match(dir_str) {
        if let Some(cap) = re.captures(dir_str) {
            let is_neg = cap.get(1).map(|m| m.as_str() == "-").unwrap_or(false);
            let pnt_index = cap
                .get(2)
                .map_or("", |m| m.as_str())
                .parse::<i32>()
                .unwrap_or(-1);
            if let Some(indx) = scom.axis_param_numbers.iter().position(|&x| x == pnt_index) {
                let axis =
                    resolve_axis_param(&scom.axis_params[indx], scom, context);
                // if axis.dir.is_none() {
                //     return Err(anyhow::anyhow!("方向为空"));
                // }
                let flag = if is_neg { -1.0 } else { 1.0 };
                dir = flag * axis.dir.unwrap_or_default();
                pos = axis.pt;
            } else {
                return Err(anyhow::anyhow!("未找到点索引: {}", pnt_index));
            }
        }
    } else {
        dir = parse_str_axis_to_vec3(dir_str, context).unwrap_or_default();
    }

    if re.is_match(ref_dir_str) {
        if let Some(cap) = re.captures(ref_dir_str) {
            let is_neg = cap.get(1).map_or("", |m| m.as_str()) == "-";
            let pnt_indx = cap
                .get(2)
                .map_or("", |m| m.as_str())
                .parse::<i32>()
                .unwrap_or(-1);
            if let Some(indx) = scom.axis_param_numbers.iter().position(|&x| x == pnt_indx) {
                let mut axis =
                    resolve_axis_param(&scom.axis_params[indx], scom, context);
                // if axis.dir.is_none() {
                //     return Err(anyhow::anyhow!("方向为空"));
                // }
                let flag = if is_neg { -1.0 } else { 1.0 };
                ref_dir = flag * axis.dir.unwrap_or_default();
            } else {
                return Err(anyhow::anyhow!("未找到点索引: {}", pnt_indx));
            }
        }
    } else {
        //unset 不存在 ref dir的情况
        ref_dir = parse_str_axis_to_vec3(ref_dir_str, context).unwrap_or_default();
    }

    return Ok((dir.normalize_or_zero(), ref_dir.normalize_or_zero(), pos));
}

//Y is N and Z is U
pub fn parse_ori_str_to_quat(
    ori_str: &str,
    context: &CataContext,
) -> anyhow::Result<Quat> {
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

        let dir_str = strs[1]
            .trim()
            .replace("E", "X")
            .replace("W", "-X")
            .replace("N", "Y")
            .replace("S", "-Y")
            .replace("U", "Z")
            .replace("D", "-Z");
        // dbg!(&dir_str);
        let dir = parse_str_axis_to_vec3(&dir_str, context)?;
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

// pub fn parse_str_axis_to_vec3_or_default(
//     pdir: &str,
//     context: &CataContext,
// ) -> Vec3 {
//     parse_str_axis_to_vec3(pdir, context).unwrap_or(Vec3::ZERO)
// }

///解析表达式里的axis
pub fn parse_str_axis_to_vec3(
    pdir: &str,
    context: &CataContext,
) -> anyhow::Result<Vec3> {
    let pdir = pdir.trim();
    //TO X (NEG ( 20 )) Z ( 65 ), 直接解析就行了
    if pdir.starts_with("TO") {
        // dbg!(pdir);
        let v = parse_to_direction(pdir, Some(context))?.unwrap_or_default();
            // .(anyhow::anyhow!(format!("方向字符串: {} 不正确。", pdir)))?;
        // dbg!(v);
        return Ok(v.as_vec3());
    }
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
                let val_result =
                    eval_str_to_f64(&val_str, context, "ANGL")?.to_string();
                new_dir_str = dir_str.replace(&val_str, &val_result);

                let val_str = cap[4].to_string();
                let val_result =
                    eval_str_to_f64(&val_str, context, "ANGL")?.to_string();
                new_dir_str = new_dir_str.replace(&val_str, &val_result);
                is_three = true;
            }
        }

        if !is_three {
            // dbg!(is_three);
            // dbg!(&dir_str);
            let re = Regex::new(r"(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])").unwrap();
            for cap in re.captures_iter(&dir_str) {
                if cap.len() == 4 {
                    let val_str = cap[2].to_string();
                    let val_result =
                        eval_str_to_f64(&val_str, context, "ANGL")?.to_string();
                    new_dir_str = dir_str.replace(&val_str, &val_result);
                }
            }
            // dbg!(&new_dir_str);
        }
    }
    let dir_str = new_dir_str.replace(" ", "");
    let v = parse_expr_to_dir(&dir_str)
        .ok_or(anyhow::anyhow!(format!("方向字符串: {} 不正确。", pdir)))?;
    Ok(v.as_vec3())
}
