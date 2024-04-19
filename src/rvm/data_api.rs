use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::ops::Mul;
use std::str::FromStr;
use aios_core::pdms_types::*;
use aios_core::geom_types::{RvmGeoInfo, RvmGeoInfos, RvmInstGeo, RvmTubiGeoInfos};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam::PrimExtrusion;
use aios_core::prim_geo::extrusion::Extrusion;
use aios_core::shape::pdms_shape::PlantMesh;
use arangors_lite::AqlQuery;

use bevy_transform::prelude::Transform;
use glam::{Mat3, Mat3A, Quat, Vec3};
use id_tree::{NodeId, Tree};
use parry3d::bounding_volume::Aabb;
use parry3d::math::Vector;
use regex::Regex;
use crate::aql_api::children::{query_children_order_aql, query_travel_children_aql, query_travel_children_refnos_aql};
use crate::aql_api::pdms_mesh::{query_pdms_mesh_aql, query_pdms_mesh_from_hash_str_aql};
use crate::arangodb::ArDatabase;
use crate::graph_db::pdms_inst_arango::{query_compound_inst_hashes_aql, query_rvm_instance_data_from_refno_aql};
use crate::consts::{AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION, AQL_PDMS_INST_GEO_COLLECTION, AQL_PDMS_INST_INFO_COLLECTION, AQL_PDMS_INST_TUBI_COLLECTION};
use crate::rvm::elements::{create_rvm_file, gen_ancestor_data_str};
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
pub async fn create_refnos_rvm_data(select_refno: Vec<RefU64>, db_option: &DbOption, database: &ArDatabase) -> anyhow::Result<Vec<u8>> {
    let mut file_data = Vec::new();
    let head = create_head_data(db_option);
    file_data.push(head);
    // 默认放上根节点
    let root = gen_ancestor_data_str("root", Vec3::ZERO);
    file_data.push(root);
    let refnos = query_travel_children_refnos_aql(database, select_refno).await?;
    // 先查询经过负实体计算的
    let compound_insts = query_compound_inst_hashes_aql(refnos.clone(), database).await?;
    // 过滤掉负实体之后再查询
    let filter_refnos = filter_compound_refnos(refnos.clone(), &compound_insts);
    let refno_geo_infos = query_single_rvm_geo_instance_aql(refnos, database).await?;
    let tubi_infos = query_rvm_tubi_instances_aql(filter_refnos, database).await?;
    let refno_geo_infos_map = refno_geo_infos.clone()
        .into_iter()
        .map(|info| (info.refno, info))
        .collect::<HashMap<_, _>>();
    // 将extrusion单独提出来 , 该部分为 extrusion 中不为负实体得部分
    let mut use_mesh_geo_hashes: Vec<u64> = Vec::new();
    for geo in refno_geo_infos.iter() {
        for rvm_inst in geo.rvm_inst_geo.iter() {
            match rvm_inst.geo_param {
                PdmsGeoParam::PrimExtrusion(_) | PdmsGeoParam::PrimRevolution(_) | PdmsGeoParam::PrimLoft(_) | PdmsGeoParam::PrimPolyhedron(_) => {
                    let Ok(hash) = rvm_inst.geo_hash.parse() else { continue; };
                    use_mesh_geo_hashes.push(hash);
                    break;
                }
                _ => { continue; }
            }
        }
    }
    // 计算负实体 rvm 数据
    let compound_hashes = compound_insts.iter()
        .filter(|info| info.cata_hash.is_some())
        .map(|info| info.cata_hash.clone().unwrap())
        .collect::<Vec<_>>();
    let compound_mesh = query_pdms_mesh_from_hash_str_aql(database, compound_hashes).await?;
    for info in compound_insts {
        let mut info_vec = Vec::new();
        let Some(insts) = refno_geo_infos_map.get(&info.refno) else { continue; };
        // cntb
        info_vec.append(&mut gen_cntb_data());
        // name
        let mut name = gen_name_position_data(&info.refno.to_string(), insts.world_transform.translation);
        info_vec.append(&mut name);
        // prim
        let mut insts = insts.clone();
        let Some(hash) = info.cata_hash else { continue; };
        let mut prim_vec = Vec::new();
        for mut inst in insts.rvm_inst_geo.iter_mut() {
            inst.geo_hash = hash.to_string();
            let Some(mut data) = gen_prim_data_test(info.refno, inst, insts.world_transform,
                                                    false, &compound_mesh) else { continue; };
            prim_vec.append(&mut data);
            if prim_vec.is_empty() { continue; };
        }
        info_vec.append(&mut prim_vec);
        // cnte
        info_vec.append(&mut gen_cnte_data());
        file_data.push(info_vec)
    }
    // 通过查找pdms_mesh找到extrusion的数据
    let extrusion_mesh = if use_mesh_geo_hashes.is_empty() {
        PlantMeshesData::default()
    } else {
        query_pdms_mesh_aql(database, use_mesh_geo_hashes.iter()).await.unwrap_or_default()
    };
    // 不带负实体得元件
    for info in refno_geo_infos {
        let mut info_vec = Vec::new();
        // cntb
        info_vec.append(&mut gen_cntb_data());
        // name
        let mut name = gen_name_position_data(&info.refno.to_string(), info.world_transform.translation);
        info_vec.append(&mut name);
        // prim
        let mut prim_vec = Vec::new();
        for geo in info.rvm_inst_geo {
            let Some(mut data) = gen_prim_data_test(info.refno, &geo, info.world_transform,
                                                    &info.att_type == "CYLI", &extrusion_mesh) else { continue; };
            prim_vec.append(&mut data);
        }
        if prim_vec.is_empty() { continue; };
        info_vec.append(&mut prim_vec);
        // cnte
        info_vec.append(&mut gen_cnte_data());
        file_data.push(info_vec)
    }
    // 单独生成tubi
    for tubi in tubi_infos {
        // cntb
        file_data.push(gen_cntb_data());
        // name
        let name = gen_name_position_data(&tubi.refno.to_string(), tubi.world_transform.translation);
        file_data.push(name);
        // prim
        let mut prim_data = Vec::new();
        for geo in tubi.rvm_inst_geo {
            let Some(mut data) = gen_prim_data_test(tubi.refno, &geo, tubi.world_transform,
                                                    &tubi.att_type == "CYLI", &extrusion_mesh) else { continue; };
            prim_data.append(&mut data);
        }
        if prim_data.is_empty() { continue; } else { file_data.push(prim_data); };
        // cnte
        file_data.push(gen_cnte_data());
    }
    file_data.push(gen_cnte_data());
    file_data.push(gen_end_data());
    Ok(file_data.into_iter().flatten().collect())
}

/// 从inst中查询rvm需要的数据
pub async fn query_rvm_geo_instance_aql(database: &ArDatabase, refnos: Vec<RefU64>) -> anyhow::Result<Vec<RvmGeoInfos>> {
    let refnos = refnos.into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_string()))
        .collect::<Vec<_>>();
    // pub geo_type: GeoBasicType,
    let aql = AqlQuery::new("
    With @@pdms_eles,@@pdms_edges,@@pdms_inst_infos,@@pdms_inst_geos
    let hashes = (
    for id in @refnos
    for v,e in 0..10 inbound id @@pdms_edges
    let inst = document(@@pdms_inst_infos,v._key)
    filter inst != null
        return {
            'refno': inst._key,
            'noun' : v.noun,
            'world_transform': inst.world_transform,
            'hash':inst.cata_hash == null ? inst._key : inst.cata_hash,
            'geo_type': v.geo_type,
        }
    )
    for hash in hashes
        let inst = document(@@pdms_inst_geos,hash.hash).insts
        filter inst != null
        return {
            'refno': hash.refno,
            'att_type' : hash.noun,
            'world_transform' : hash.world_transform,
            'rvm_inst_geo': inst
    }")
        .bind_var("refnos", refnos)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@pdms_inst_infos", AQL_PDMS_INST_INFO_COLLECTION)
        .bind_var("@pdms_inst_geos", AQL_PDMS_INST_GEO_COLLECTION);
    let result = database.aql_query::<RvmGeoInfos>(aql).await?;
    Ok(result)
}

pub async fn query_single_rvm_geo_instance_aql(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<RvmGeoInfos>> {
    let refnos = refnos.into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_string()))
        .collect::<Vec<_>>();
    // pub geo_type: GeoBasicType,
    let aql = AqlQuery::new("
    With @@pdms_eles,@@pdms_edges,@@pdms_inst_infos,@@pdms_inst_geos
    let hashes = (
    for id in @refnos
    for v,e in 0 inbound id @@pdms_edges
    let inst = document(@@pdms_inst_infos,v._key)
    filter inst.geo_type not in ['CataCrossNeg','Neg']
    filter inst != null
        return {
            'refno': inst._key,
            'noun' : v.noun,
            'world_transform': inst.world_transform,
            'hash':inst.cata_hash == null ? inst._key : inst.cata_hash,
            'geo_type': inst.geo_type,
        }
    )
    for hash in hashes
        let inst = document(@@pdms_inst_geos,hash.hash).insts
        filter inst != null
        return {
            'refno': hash.refno,
            'att_type' : hash.noun,
            'world_transform' : hash.world_transform,
            'rvm_inst_geo': inst
    }")
        .bind_var("refnos", refnos)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@pdms_inst_infos", AQL_PDMS_INST_INFO_COLLECTION)
        .bind_var("@pdms_inst_geos", AQL_PDMS_INST_GEO_COLLECTION);
    let result = database.aql_query::<RvmGeoInfos>(aql).await?;
    Ok(result)
}


pub async fn query_rvm_tubi_instances_aql(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<RvmGeoInfos>> {
    let refnos = refnos.into_iter()
        .map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_string()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new("
    With @@pdms_eles,@@pdms_edges,@@pdms_inst_tubis,@@pdms_inst_geos
    let hashes = (
    for id in @refnos
    for v,e in 0 inbound id @@pdms_edges
    let inst = document(@@pdms_inst_tubis,v._key)
    filter inst != null
        return {
            'refno': inst._key,
            'noun' : v.noun,
            'aabb' : inst.aabb,
            'world_transform': inst.world_transform,
            'hash':inst.cata_hash == null ? inst._key : inst.cata_hash
        }
    )
    for hash in hashes
        let inst = document(@@pdms_inst_geos,hash.hash).insts
        filter inst != null
        return {
            'refno': hash.refno,
            'att_type' : hash.noun,
            'aabb' : hash.aabb,
            'world_transform' : hash.world_transform,
            'rvm_inst_geo': inst
    }")
        .bind_var("refnos", refnos)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@pdms_inst_tubis", AQL_PDMS_INST_TUBI_COLLECTION)
        .bind_var("@pdms_inst_geos", AQL_PDMS_INST_GEO_COLLECTION);
    let result = database.aql_query::<RvmTubiGeoInfos>(aql).await?;
    let result = result.into_iter().map(|r| r.into_rvmgeoinfos()).collect();
    Ok(result)
}

pub fn gen_prim_data_test(refno: RefU64, geo_instance: &RvmInstGeo, desi_transform: Transform,
                          b_desi_cyli: bool, mesh_data: &PlantMeshesData) -> Option<Vec<u8>> {
    let mut data = vec![];
    let geo_transform = geo_instance.transform;
    let mut transform =
        if geo_instance.is_tubi {
            desi_transform
        } else {
            desi_transform * geo_transform
        };
    let aabb = geo_instance.aabb.unwrap().scaled(&transform.scale.into());
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
        let Ok(hash) = geo_instance.geo_hash.parse::<u64>() else { return None; };
        //mesh 的单独处理，只变换相对坐标
        if mesh_data.meshes.contains_key(&hash) {
            let mesh = mesh_data.get_mesh(hash)?;
            let trans = Transform::from_scale(transform.scale);
            data.append(&mut gen_mesh_data(trans, mesh.clone()));
        } else {
            let mut geo = geo_instance.geo_param.convert_rvm_pri_data()?;
            data.append(&mut geo);
        }
    }
    Some(data)
}

///
pub fn gen_mesh_data(transform: Transform, mesh: PlantMesh) -> Vec<u8> {
    let mut data = Vec::new();
    data.append(&mut format!("     {} \r\n", mesh.vertices.len() / 3).into_bytes());
    let mut i = 0;
    while i + 2 <= mesh.vertices.len() {
        if i + 2 >= mesh.normals.len() { break; };
        data.append(&mut format!("     1 \r\n").into_bytes());
        data.append(&mut format!("     3 \r\n").into_bytes());
        let point_x = transform.transform_vec3(mesh.vertices[i]);
        let normal_x = transform.transform_vec3(mesh.normals[i]);
        let point_y = transform.transform_vec3(mesh.vertices[i + 1]);
        let normal_y = transform.transform_vec3(mesh.normals[i + 1]);
        let point_z = transform.transform_vec3(mesh.vertices[i + 2]);
        let normal_z = transform.transform_vec3(mesh.normals[i + 2]);

        data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", point_x.x, point_x.y, point_x.z).into_bytes());
        data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", normal_x.x, normal_x.y, normal_x.z).into_bytes());
        data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", point_y.x, point_y.y, point_y.z).into_bytes());
        data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", normal_y.x, normal_y.y, normal_y.z).into_bytes());
        data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", point_z.x, point_z.y, point_z.z).into_bytes());
        data.append(&mut format!("     {:.2}       {:.2}       {:.2}\r\n", normal_z.x, normal_z.y, normal_z.z).into_bytes());
        i += 3;
    }
    data
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

pub(crate) fn gen_cntb_data() -> Vec<u8> {
    format!("CNTB\r\n     1     2\r\n").into_bytes()
}

pub(crate) fn gen_cnte_data() -> Vec<u8> {
    format!("CNTE\r\n     1     2\r\n").into_bytes()
}

pub(crate) fn gen_end_data() -> Vec<u8> {
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

fn filter_compound_refnos(refnos: Vec<RefU64>, compound_refnos: &Vec<EleGeosInfo>) -> Vec<RefU64> {
    let compound_refnos = compound_refnos.iter().map(|refno| refno.refno).collect::<HashSet<RefU64>>();
    let refnos = refnos.into_iter().collect::<HashSet<RefU64>>();
    let difference = refnos.difference(&compound_refnos).map(|x| *x).collect::<Vec<_>>();
    difference
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

#[tokio::test]
async fn test_query_rvm_tubi_instances_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refnos = vec![RefU64::from_str("24383/73930").unwrap()];
    let tubi = query_rvm_tubi_instances_aql(refnos, &database).await?;
    dbg!(&tubi);
    Ok(())
}

#[tokio::test]
async fn test_query_rvm_geo_instance_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refnos = RefU64::from_str("17496/118446").unwrap();
    // let refnos = vec![RefU64::from_str("24381/100681").unwrap()];
    // let mut refnos = query_children_order_aql(&database, refnos[0]).await?
    //     .into_iter().map(|refno| refno.refno).collect::<Vec<_>>();
    let result = create_refnos_rvm_data(vec![refnos], &db_option, &database).await?;
    let mut file = std::fs::File::create("test.rvm").unwrap();
    file.write_all(&result).unwrap();
    Ok(())
}