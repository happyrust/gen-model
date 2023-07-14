use std::io::Write;
use std::ops::Mul;
use aios_core::pdms_types::{EleInstGeo, EleGeosInfo, RefU64};
use aios_core::geom_types::{RvmGeoInfo, RvmGeoInfos, RvmInstGeo};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use arangors_lite::AqlQuery;
use bb8_arangodb::arangors_lite::Database;
use bevy_transform::prelude::Transform;
use glam::{Mat3, Mat3A, Quat, Vec3};
use id_tree::{NodeId, Tree};
use parry3d::bounding_volume::Aabb;
use parry3d::math::Vector;
use regex::Regex;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::graph_db::pdms_inst_arango::query_rvm_instance_data_from_refno_aql;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::rvm::head::create_head_data;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

#[derive(Debug, Clone)]
pub enum ShapeModule {
    Desi,
    Cata,
}

/// rvm 格式类型
#[derive(Debug, Clone)]
pub enum RvmShapeTypeData {
    /// 0: bottom width, 1: bottom length , 2:top width, 3:top length ,4:x offset, 5: y offset, 6: height
    Pyramid([f32; 7]),
    /// 长 宽 高
    Box([f32; 3]),
    /// 0:弧长半径, 1:矩形的宽, 2: 矩形的长 3: 角度: π/n
    RectangularTorus([f32; 4]),
    /// 0:弧长半径, 1: 圆半径 2: 角度: π/n
    CircularTorus([f32; 3]),
    /// 0:radius 1: height
    EllipticalDish([f32; 2]),
    /// 半径 高
    SphericalDish([f32; 2]),
    /// 0: bottom radius 1 : top radius 2: height 3: offset
    Snout([f32; 9]),
    /// 半径 高
    Cylinder([f32; 2]),
    /// 球体
    Sphere,
    /// 0: 1: 长度(mm)
    Line([f32; 2]),
    /// 多面体
    FacetGroup,
}

impl RvmShapeTypeData {
    /// 获得 ShapeType在 Prim种代表的数字
    pub fn get_shape_number(&self) -> u8 {
        match self {
            RvmShapeTypeData::Pyramid(_) => 1,
            RvmShapeTypeData::Box(_) => 2,
            RvmShapeTypeData::RectangularTorus(_) => 3,
            RvmShapeTypeData::CircularTorus(_) => 4,
            RvmShapeTypeData::EllipticalDish(_) => 5,
            RvmShapeTypeData::SphericalDish(_) => 6,
            RvmShapeTypeData::Snout(_) => 7,
            RvmShapeTypeData::Cylinder(_) => 8,
            RvmShapeTypeData::Sphere => 9,
            RvmShapeTypeData::Line(_) => 10,
            RvmShapeTypeData::FacetGroup => 11,
        }
    }
    pub fn convert_shape_type_to_bytes(&self) -> Vec<u8> {
        let mut data = vec![];
        match &self {
            RvmShapeTypeData::Pyramid(array) => {
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", array[0], array[1], array[2], array[3]).into_bytes());
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}\r\n", array[4], array[5], array[6]).into_bytes());
            }
            RvmShapeTypeData::Box(array) => {
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}\r\n", array[0], array[1], array[2]).into_bytes());
            }
            RvmShapeTypeData::RectangularTorus(array) => {
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", array[0], array[1], array[2], array[3]).into_bytes());
            }
            RvmShapeTypeData::CircularTorus(array) => {
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}\r\n", array[0], array[1], array[2]).into_bytes());
            }
            RvmShapeTypeData::EllipticalDish(array) => {
                data.append(&mut format!("     {:.7}     {:.7}\r\n", array[0], array[1]).into_bytes());
            }
            RvmShapeTypeData::SphericalDish(arr) => {
                data.append(&mut format!("     {:.7}     {:.7}\r\n", arr[0], arr[1]).into_bytes());
            }
            RvmShapeTypeData::Snout(array) => {
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}     {:.7}\r\n", array[0], array[1], array[2], array[3], array[4]).into_bytes());
                data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", array[5], array[6], array[7], array[8]).into_bytes());
            }
            RvmShapeTypeData::Cylinder(array) => {
                data.append(&mut format!("     {:.7}     {:.7}\r\n", array[0], array[1]).into_bytes());
            }
            RvmShapeTypeData::Line(arr) => {
                data.append(&mut format!("     {:.7}     {:.7}\r\n", arr[0], arr[1]).into_bytes());
            }
            _ => {}
        }
        data
    }
}

// type_data: prim 最后一列不同 att_type 存放的数据不一样
// pub fn gen_prim_data(rvm_instance: RvmInstGeo, shape_type: RvmShapeTypeData, shape_module: ShapeModule) -> Vec<u8> {
//     let mut data = vec![];
//     if rvm_instance.aabb.is_none() { return data; }
//     let aabb = rvm_instance.aabb.unwrap();
//     data.append(&mut gen_prim_head_data());
//     data.append(&mut format!("     {}\r\n", shape_type.get_shape_number()).into_bytes());
//     data.append(&mut gen_prim_scale_position_data(rvm_instance.transform.rotation, Vec3::ONE,
//                                                   rvm_instance.transform.translation));
//     match shape_module {
//         ShapeModule::Desi => { data.append(&mut gen_desi_prim_aabb_data(aabb, /*rvm_instance.world_transform,*/ &PdmsGeoParam::default())); }
//         ShapeModule::Cata => { data.append(&mut gen_cata_prim_aabb_data(aabb)); }
//     }
//
//     data.append(&mut shape_type.convert_shape_type_to_bytes());
//     data
// }

/// 生成 rvm 文件
pub async fn create_refnos_rvm_data(refnos:Vec<RefU64>,db_option:&DbOption,database:&ArDatabase) -> anyhow::Result<Vec<u8>> {
    let mut file_data = Vec::new();
    let head = create_head_data(db_option);

    let refno_geo_infos = query_rvm_geo_instance_aql(refnos,database).await?;
    Ok(file_data)
}

/// 从inst中查询rvm需要的数据
pub async fn query_rvm_geo_instance_aql(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<RvmGeoInfos>> {
    let refnos = refnos.into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno()))
        .collect::<Vec<_>>();
    let refno = refnos[0].clone(); // todo 先拿一个测试
    let aql = AqlQuery::new("let hashes = (
    for v,e in 0..100 inbound @id pdms_edges
    let inst = document('pdms_inst_infos',v._key)
    filter inst != null
        return {
            'refno': inst._key,
            'noun' : v.noun,
            'world_transform': inst.world_transform,
            'hash':inst.cata_hash == null ? inst._key : inst.cata_hash
        }
    )
    for hash in hashes
        let inst = document('pdms_inst_geos',hash.hash).insts
        filter inst != null
        return {
            'refno': hash.refno,
            'att_type' : hash.noun,
            'world_transform' : hash.world_transform,
            'rvm_inst_geo': inst
    }").bind_var("id", refno);
    let result = database.aql_query::<RvmGeoInfos>(aql).await?;
    Ok(result)
}

pub fn gen_prim_data_test(geo_instance: &RvmInstGeo, desi_transform: Transform, b_desi_cyli: bool) -> Vec<u8> {
    let mut data = vec![];
    let geo_transform = geo_instance.transform;
    let mut transform =
        if geo_instance.is_tubi {
            geo_transform
        } else {
            desi_transform * geo_transform
        };
    let aabb = geo_instance.aabb.unwrap().scaled(&Vector::new(desi_transform.scale.x, desi_transform.scale.y, desi_transform.scale.z));
    if let Some(num) = geo_instance.geo_param.into_rvm_pri_num() {
        // tubi 不需要和desi进行变换
        let translation = {
            match &geo_instance.geo_param {
                PdmsGeoParam::PrimSCylinder(data) => {
                    if !data.center_in_mid || b_desi_cyli {
                        transform.translation + transform.rotation.mul_vec3(Vec3::new(0.0, 0.0, data.phei / 2.0))
                    } else {
                        transform.translation
                    }
                }
                _ => {
                    transform.translation
                }
            }
        };
        data.append(&mut gen_prim_head_data());
        data.append(&mut format!("     {}\r\n", num).into_bytes());
        data.append(&mut gen_prim_scale_position_data(transform.rotation, Vec3::ONE,
                                                      translation));
        data.append(&mut gen_desi_prim_aabb_data(aabb, /*geo_instance.transform,*/ &geo_instance.geo_param));
        data.append(&mut geo_instance.geo_param.convert_rvm_pri_data());
    }
    data
}

#[tokio::test]
async fn test_query_rvm_geo_instance_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refnos = vec![RefU64::from_refno_str("24383/73933").unwrap()];
    let infos = query_rvm_geo_instance_aql(refnos, &database).await?;
    let mut result = Vec::new();
    for info in infos {
        for geo in info.rvm_inst_geo {
            let data = gen_prim_data_test(&geo, info.world_transform, &info.att_type == "CYLI");
            result.push(data);
        }
    }
    let mut file = std::fs::File::create("test.rvm").unwrap();
    file.write_all(&result.into_iter().flatten().collect::<Vec<u8>>()).unwrap();
    Ok(())
}

pub fn gen_data_from_tree(tree: Tree<(RefU64, Vec<u8>)>) -> Vec<u8> {
    let mut data = Vec::new();
    let root = tree.root_node_id();
    if root.is_none() { return data; }
    let root = root.unwrap();
    // 递归生成数据
    gen_data_recursion(&mut data, &tree, root);
    data
}

fn gen_data_recursion(mut data: &mut Vec<u8>, tree: &Tree<(RefU64, Vec<u8>)>, current_node: &NodeId) {
    if let Ok(node) = tree.get(current_node) {
        let node_data = node.data();
        data.append(&mut gen_cntb_data());
        data.append(&mut node_data.1.clone());
        for child in node.children() {
            gen_data_recursion(data, tree, child);
        }
        data.append(&mut gen_cnte_data());
    }
}

pub fn gen_cntb_data() -> Vec<u8> {
    format!("CNTB\r\n     1     2\r\n").into_bytes()
}

pub fn gen_cnte_data() -> Vec<u8> {
    format!("CNTE\r\n     1     2\r\n").into_bytes()
}

pub fn gen_end_data() -> Vec<u8> {
    format!("END:\r\n     1     1\r\n").into_bytes()
}

pub fn gen_name_position_data(name: &str, position: Vec3) -> Vec<u8> {
    format!("{name}\r\n       {:.2}       {:.2}       {:.2}\r\n     1\r\n", position.x, position.y, position.z).into_bytes()
}

fn gen_prim_head_data() -> Vec<u8> {
    format!("PRIM\r\n     1     1\r\n").into_bytes()
}

fn gen_prim_scale_position_data(rotation: Quat, scale: Vec3, position: Vec3) -> Vec<u8> {
    let mut data = Vec::new();
    let rotation_mat = Mat3::from_quat(rotation);

    let x_axis = rotation_mat.x_axis.normalize();
    let y_axis = rotation_mat.y_axis.normalize();
    let z_axis = rotation_mat.z_axis.normalize();

    let mut position_x = position.x;
    let mut position_y = position.y;
    let mut position_z = position.z;

    data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", x_axis.x / 1000.0, y_axis.x / 1000.0, z_axis.x / 1000.0, position_x / 1000.0).into_bytes());
    data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", x_axis.y / 1000.0, y_axis.y / 1000.0, z_axis.y / 1000.0, position_y / 1000.0).into_bytes());
    data.append(&mut format!("     {:.7}     {:.7}     {:.7}     {:.7}\r\n", x_axis.z / 1000.0, y_axis.z / 1000.0, z_axis.z / 1000.0, position_z / 1000.0).into_bytes());
    data
}

fn gen_desi_prim_aabb_data(a: Aabb, /*world_transform: Transform,*/ geo_param: &PdmsGeoParam) -> Vec<u8> {
    let max = if let PdmsGeoParam::PrimSCylinder(data) = geo_param {
        if data.center_in_mid {
            Vec3::from((a.maxs.x, a.maxs.y, a.maxs.z / 2.0))
        } else {
            // Vec3::from((aabb.maxs.x, aabb.maxs.y, aabb.maxs.z))
            Vec3::from((a.maxs.x, a.maxs.y, a.maxs.z / 2.0))
        }
    } else {
        Vec3::from((a.maxs.x, a.maxs.y, a.maxs.z))
    };

    let min = if let PdmsGeoParam::PrimSCylinder(data) = geo_param {
        if data.center_in_mid {
            Vec3::from((a.mins.x, a.mins.y, -a.maxs.z / 2.0))
        } else {
            // Vec3::from((aabb.mins.x, aabb.mins.y, aabb.mins.z))
            Vec3::from((a.mins.x, a.mins.y, -a.maxs.z / 2.0))
        }
    } else {
        Vec3::from((a.mins.x, a.mins.y, a.mins.z))
    };

    let mut data = Vec::new();
    data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", min.x, min.y, min.z).into_bytes());
    data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", max.x, max.y, max.z).into_bytes());
    data
}

fn gen_cata_prim_aabb_data(aabb: Aabb) -> Vec<u8> {
    let mut data = Vec::new();
    data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", aabb.mins.x, aabb.mins.y, aabb.mins.z).into_bytes());
    data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", aabb.maxs.x, aabb.maxs.y, aabb.maxs.z).into_bytes());
    data
}

fn keep_2_decimals_from_vec3(input: Vec3) -> Vec3 {
    let x = (input.x * 100.0).round() / 100.0;
    let y = (input.y * 100.0).round() / 100.0;
    let z = (input.z * 100.0).round() / 100.0;
    Vec3::from_array([x, y, z])
}

pub fn keep_2_decimals_from_f32(input: f32) -> f32 {
    (input * 100.0).round() / 100.0
}

/// 正则匹配字符串中的数字
pub fn get_num_from_str(input: &str) -> Option<i32> {
    let regex = Regex::new(r"[0-9]+([.]{1}[0-9]+){0,1}").unwrap();
    if let Some(captures) = regex.captures(input) {
        if let Ok(r) = captures[0].parse::<i32>() {
            return Some(r);
        }
    }
    None
}

#[test]
fn test_str_split() {
    let regex = Regex::new(r"[0-9]+([.]{1}[0-9]+){0,1}").unwrap();
    let str = "RSDTT0001K";
    // let result = &str[3..];
    if let Some(captures) = regex.captures(str) {
        dbg!(&captures[0]);
    }
}