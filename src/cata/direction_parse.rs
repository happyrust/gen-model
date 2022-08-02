use glam::*;
use nom::number::complete:: float;
use nom::*;
use std::collections::HashMap;
use aios_core::tool::float_tool::vec3_round_2;
use lazy_static::lazy_static;
use nom::branch::alt;
use nom::combinator::{opt, recognize, complete};
use nom::sequence::{delimited, pair};
use nom::bytes::complete::*;

lazy_static! {
    pub static ref AXISES_MAP: HashMap<&'static str, Vec3> = {
        let mut s = HashMap::new();
        s.insert("X", Vec3::X);
        s.insert("Y", Vec3::Y);
        s.insert("Z", Vec3::Z);
        s.insert("-X", -Vec3::X);
        s.insert("-Y", -Vec3::Y);
        s.insert("-Z", -Vec3::Z);
        s
    };
}


#[derive(Debug, Default)]
struct Rotation {
    axis: Vec3,
    angle: f32,
}

#[derive(Debug, Default)]
struct RotationStruct {
    origin_axis: Vec3,
    rot1: Option<Rotation>,
    rot2: Option<Rotation>,
}

pub fn signed_axis(input: &str) -> IResult<&str, (Option<&str>, &str)> {
    pair(
        opt(tag("-")),  // maybe sign?
        alt((tag("X") , tag("Y"), tag("Z")))
    )(input)
}

pub fn parse_angle(input: &str) -> IResult<&str, f32> {
    alt((
        float,
        delimited( tag("("), float, tag(")") )
    ))(input)
}

fn parse_axis_rotation(input: &str) -> IResult<&str, Rotation> {
    let (input, angle) = parse_angle(input)?;
    let (input, axis) = recognize(signed_axis)(input)?;
    Ok((input, Rotation{
        axis: *AXISES_MAP.get(axis).unwrap(),
        angle,
    }))
}

fn parse_rotation_struct(input: &str) -> IResult<&str, RotationStruct> {
    let (input, axis) = recognize(signed_axis)(input)?;
    let (input, rot1) = opt(complete(parse_axis_rotation))(input)?;
    let (input, rot2) = opt(complete(parse_axis_rotation))(input)?;
    Ok((input, RotationStruct{
        origin_axis: *AXISES_MAP.get(axis).unwrap(),
        rot1,
        rot2,
    }))
}

///解析expression到direction
pub fn parse_expr_to_dir(expr: &str) -> Vec3 {
    if let Ok((_, res)) = parse_rotation_struct(expr) {
        let mut axis = res.origin_axis;
        if res.rot1.is_some() {
            let rot1 = res.rot1.as_ref().unwrap();
            let target_axis = axis.cross(rot1.axis);
            let quat1 = Quat::from_axis_angle(target_axis, rot1.angle.to_radians());
            axis = quat1 * axis;
            if res.rot2.is_some() {
                let rot2 = res.rot2.as_ref().unwrap();
                let target_axis = axis.cross(rot2.axis);
                let quat2 = Quat::from_axis_angle(target_axis, rot2.angle.to_radians());
                axis = quat2 * axis;
            }
        }
        return vec3_round_2(axis);
    }
    Vec3::ZERO
}

#[test]
fn test_parse_vector() {
    let test_str = "X30Y";
    let dir = parse_expr_to_dir(test_str);
    dbg!(dir);
    let dir = parse_rotation_struct(test_str).unwrap();
    dbg!(&dir);
    dbg!(AXISES_MAP.get("-X"));
    let test_str = "-X(59)Y";
    let res= parse_expr_to_dir(test_str);
    println!("{:?}",res);
    let test_str = "Z(90.0)Y";
    let res= parse_expr_to_dir(test_str);
    println!("{:?}",res);
    let test_str = "-Y45Z";
    let res= parse_expr_to_dir(test_str);
    println!("test_str: {:?}",res);
    let test_str = "Y45-Z";
    let res= parse_expr_to_dir(test_str);
    println!("test_str: {:?}",res);
    //Z DESIGN PARAM 14Y
}
