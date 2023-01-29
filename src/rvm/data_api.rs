use aios_core::pdms_types::RefU64;
use aios_core::rvm_types::RvmGeoInfo;
use arangors_lite::Database;
use glam::{Quat, Vec3};
use parry3d::bounding_volume::Aabb;
use crate::graph_db::pdms_inst_arango::query_rvm_instance_data_from_refno_aql;

#[derive(Debug)]
pub enum ShapeTypeData {
    Pyramid,
    // 长 宽 高
    Box([f32; 3]),
    RectangularTorus,
    // 0:弧长, 1: radius
    CircularTorus([f32; 3]),
    EllipticalDish,
    // 半径 高
    SphericalDish([f32; 2]),
    // 0: bottom radius 1 : top radius 2: height
    Snout([f32; 9]),
    // 半径 高
    Cylinder([f32; 2]),
    Sphere,
    Line,
    FacetGroup,
}

impl ShapeTypeData {
    /// 获得 ShapeType在 Prim种代表的数字
    pub fn get_shape_number(&self) -> u8 {
        match self {
            ShapeTypeData::Pyramid => 1,
            ShapeTypeData::Box(_) => 2,
            ShapeTypeData::RectangularTorus => 3,
            ShapeTypeData::CircularTorus(_) => 4,
            ShapeTypeData::EllipticalDish => 5,
            ShapeTypeData::SphericalDish(_) => 6,
            ShapeTypeData::Snout(_) => 7,
            ShapeTypeData::Cylinder(_) => 8,
            ShapeTypeData::Sphere => 9,
            ShapeTypeData::Line => 10,
            ShapeTypeData::FacetGroup => 11,
        }
    }
    pub fn convert_shape_type_to_bytes(&self) -> Vec<u8> {
        let mut data = vec![];
        match &self {
            ShapeTypeData::Box(arr) => {
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}\r\n", arr[0], arr[1], arr[2]).into_bytes());
            }
            ShapeTypeData::SphericalDish(arr) => {
                data.append(&mut format!("     {:.7}     {:.7}\r\n", arr[0], arr[1]).into_bytes());
            }
            ShapeTypeData::Snout(array) => {
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}     {:.7}\r\n", array[0], array[1], array[2], array[3], array[4]).into_bytes());
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", array[5], array[6], array[7], array[8]).into_bytes());
            }
            ShapeTypeData::Cylinder(arr) => {
                data.append(&mut format!("     {:.7}     {:.7}\r\n", arr[0], arr[1]).into_bytes());
            }
            _ => {}
        }
        data
    }
}

// type_data: prim 最后一列不同 att_type 存放的数据不一样
pub fn gen_prim_data(rvm_instance: RvmGeoInfo, shape_type: ShapeTypeData) -> Vec<u8> {
    let mut data = vec![];
    if rvm_instance.aabb.is_none() { return data; }
    let aabb = rvm_instance.aabb.unwrap();
    data.append(&mut gen_prim_head_data());
    data.append(&mut format!("     {}\r\n", shape_type.get_shape_number()).into_bytes());
    data.append(&mut gen_prim_scale_position_data(rvm_instance.world_transform.2, rvm_instance.world_transform.1));
    data.append(&mut gen_prim_aabb_data(aabb, rvm_instance.world_transform));
    data.append(&mut shape_type.convert_shape_type_to_bytes());
    data
}

pub fn gen_cntb_data() -> Vec<u8> {
    format!("CNTB\r\n     1     2\r\n").into_bytes()
}

pub fn gen_cnte_data() -> Vec<u8> {
    format!("CNTE\r\n     1     2\r\n").into_bytes()
}

pub fn gen_name_position_data(name: &str, position: Vec3) -> Vec<u8> {
    format!("{name}\r\n       {:.2}       {:.2}       {:.2}\r\n     \r\n", position.x, position.y, position.z).into_bytes()
}

fn gen_prim_head_data() -> Vec<u8> {
    format!("PRIM\r\n     1     1\r\n").into_bytes()
}

fn gen_prim_scale_position_data(scale: Vec3, position: Vec3) -> Vec<u8> {
    let mut data = Vec::new();
    let scale_x = scale.x * 0.001;
    let scale_y = scale.y * 0.001;
    let scale_z = scale.z * 0.001;
    data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", scale_x, 0.0, 0.0, position.x).into_bytes());
    data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", 0.0, scale_y, 0.0, position.y).into_bytes());
    data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", 0.0, 0.0, scale_z, position.z).into_bytes());
    data
}

fn gen_prim_aabb_data(aabb: Aabb, world_transform: (Quat, Vec3, Vec3)) -> Vec<u8> {
    let transform = bevy::prelude::Transform {
        translation: world_transform.1,
        rotation: world_transform.0,
        scale: world_transform.2,
    };
    let inverse = transform.compute_matrix().inverse();
    let min = Vec3::from((aabb.mins.x, aabb.mins.y, aabb.mins.z));
    let max = Vec3::from((aabb.maxs.x, aabb.maxs.y, aabb.maxs.z));
    let min_bbox = inverse.transform_point3(min);
    let max_bbox = inverse.transform_point3(max);
    let mut data = Vec::new();
    data.append(&mut format!("       {:.2}       {:.2}       {:.2}\r\n", min_bbox.x, min_bbox.y, min_bbox.z).into_bytes());
    data.append(&mut format!("       {:.2}       {:.2}       {:.2}\r\n", max_bbox.x, max_bbox.y, max_bbox.z).into_bytes());
    data
}