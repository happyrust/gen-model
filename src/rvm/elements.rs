use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::Arc;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;

use bevy_transform::prelude::Transform;
use bitvec::macros::internal::funty::Floating;
use dashmap::{DashMap, DashSet};
use futures::future::BoxFuture;
use futures::FutureExt;
use glam::Vec3;
use id_tree::{Node, Tree};
use id_tree::InsertBehavior::{AsRoot, UnderNode};
use nom::character::is_alphabetic;
use parry3d::bounding_volume::Aabb;
use sqlx::{MySql, Pool};
use crate::api::attr::{query_implicit_attr, query_position_from_id};
use crate::api::element::{query_children_eles, query_ele_node};
use crate::aql_api::children::*;
use crate::aql_api::PdmsRefnoNameAql;
use crate::arangodb::ArDatabase;
use aios_core::get_db_option;
use aios_core::db_pool::get_project_pool;
use crate::graph_db::pdms_inst_arango::*;
use crate::rvm::data_api::*;
use crate::rvm::head::{create_head_data, create_tail_data};
use std::str::FromStr;

pub async fn create_rvm_file(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<u8>> {
    let mut data = vec![];
    let database = aios_mgr.get_arango_db().await?;
    let db_option = &aios_mgr.db_option;
    let db_option = get_db_option();
    let pool = get_project_pool(&db_option).await?;
    // TODO: The original code was looking up project pool by refno, but now we use the global option
    let ancestor = query_ancestor_with_name_till_type_aql(&database, refno, "SITE").await?;
    if ancestor.is_empty() { return Ok(data); }
    let cntb_len = ancestor.len();
    data.append(&mut create_head_data(db_option));
    let mut ancestor_data = create_ancestor_data(refno, ancestor, aios_mgr).await.unwrap_or((vec![], Vec3::ZERO));
    data.append(&mut ancestor_data.0);
    // let mut element_data = vec![];
    // let element = create_element_data(refno, aios_mgr, &mut element_data, ancestor_data.1, &database, &pool).await;
    // let element = create_element_data_tree(refno, aios_mgr, ancestor_data.1, &database, &pool).await;
    let element = create_element_data_rvm_tree(refno, &database, &pool, aios_mgr).await.unwrap();
    // if let Ok(tree) = element {
    let tree = element;
    data.append(&mut gen_data_from_tree(tree));
    // }
    data.append(&mut create_tail_data(cntb_len));
    Ok(data)
}

pub async fn create_owner_data(refno: RefU64, aios_mgr: &AiosDBManager, database: &ArDatabase) -> anyhow::Result<Vec<u8>> {
    let mut data = vec![];
    let ancestor = query_ancestor_with_name_till_type_aql(database, refno, "SITE").await?;
    if ancestor.is_empty() { return Ok(data); }
    let mut ancestor_data = create_ancestor_data(refno, ancestor, aios_mgr).await.unwrap_or((vec![], Vec3::ZERO));
    let db_option = get_db_option();
    let pool = get_project_pool(&db_option).await?;
    // TODO: The original code was looking up project pool by refno, but now we use the global option
    let mut element_data = vec![];
    data.append(&mut ancestor_data.0);
    if let Ok(_) = create_element_data(refno, aios_mgr, &mut data, ancestor_data.1, database, &pool).await {
        data.append(&mut element_data);
    }
    Ok(data)
}

async fn create_ancestor_data(refno: RefU64, ancestor: Vec<PdmsRefnoNameAql>, aios_mgr: &AiosDBManager) -> anyhow::Result<(Vec<u8>, Vec3)> {
    let mut data = vec![];
    let mut current_position = Vec3::ZERO;
    for refno_name in ancestor.into_iter().rev() {
        let ancestor_refno = RefU64::from_str(&refno_name.refno);
        if ancestor_refno.is_err() { continue; }
        let ancestor_refno = ancestor_refno.unwrap();
        if ancestor_refno == refno { continue; }
        let pos = query_position_from_id(ancestor_refno, aios_mgr).await?.unwrap_or(Vec3::ZERO);
        current_position = current_position + pos;
        data.append(&mut gen_ancestor_data_str(&refno_name.name, current_position));
    }
    Ok((data, current_position))
}

/// position: ancestor到本层级的相对坐标
async fn create_element_data(refno: RefU64, aios_mgr: &AiosDBManager, mut data: &mut Vec<u8>, position: Vec3, database: &ArDatabase, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let children = query_children_eles(refno, &pool).await?;
    for child in children {
        let refno = child.refno;
        let instance = query_rvm_instance_data_from_refno_aql(refno, &database).await?;
        if instance.is_none() { continue; }
        let instance = instance.unwrap();
        // 如果模型中所有得类型 visible 都为 false 就跳过
        let mut b_visible = 0;
        for data in &instance.data {
            if !data.visible { b_visible += 1; }
        }
        if b_visible >= instance.data.len() {
            continue;
        }
        let shape_data = convert_shape_type_data(refno, aios_mgr).await?;

        if let Some(shape_data) = shape_data {
            data.append(&mut gen_cntb_data());
            let pos = query_position_from_id(refno, aios_mgr).await?.unwrap_or(Vec3::ZERO) + position;
            data.append(&mut gen_name_position_data(&child.name, pos));
            // data.append(&mut gen_prim_data(instance, shape_data, ShapeModule::Desi));
        } else {
            // let mut cata_element_data = create_cata_element_data(refno, instance, &database).await?;
            // if !cata_element_data.is_empty() {
            //     data.append(&mut gen_cntb_data());
            //     let pos = query_position_from_id(refno, aios_mgr).await?.unwrap_or(Vec3::ZERO) + position;
            //     data.append(&mut gen_name_position_data(&child.name, pos));
            //     data.append(&mut cata_element_data);
            // }
        }
        data.append(&mut gen_cnte_data());
    }
    Ok(())
}

async fn create_element_data_tree(cur_refno: RefU64, aios_mgr: &AiosDBManager, mut position: Vec3, database: &ArDatabase, pool: &Pool<MySql>) -> anyhow::Result<Tree<(RefU64, Vec<u8>)>> {
    let mut pending_children = VecDeque::new();
    let current_element = query_ele_node(cur_refno, pool).await; // 返回选中节点的elenode数据
    if current_element.is_err() { return Ok(Tree::default()); }
    let mut node_id_map = HashMap::new();
    let current_element = current_element.unwrap();

    let mut tree = Tree::new();
    // 将选中节点设为头节点
    let current_element = PdmsElement {
        refno: current_element.refno,
        owner: current_element.owner,
        name: current_element.name,
        noun: current_element.noun,
        version: 0,
        children_count: 0,
    };
    pending_children.push_back(current_element.clone());

    while !pending_children.is_empty() {
        let child = pending_children.pop_front().unwrap();
        let mut data = Vec::new();
        let refno = child.refno;
        let children = query_children_eles(refno, &pool).await?;
        pending_children.extend(children.into_iter());
        let instance = query_rvm_instance_data_from_refno_aql(refno, &database).await?;
        let pos = query_position_from_id(refno, aios_mgr).await?.unwrap_or(Vec3::ZERO) + position;
        if refno == cur_refno {
            position = pos;
        }
        // 有模型的 refno
        if let Some(instance) = instance {
            // 如果模型中所有得类型 visible 都为 false 就跳过
            let mut b_visible = 0;
            for data in &instance.data {
                if !data.visible { b_visible += 1; }
            }
            if b_visible >= instance.data.len() {
                continue;
            }
            let shape_data = convert_shape_type_data(refno, aios_mgr).await?;
            // 算绝对坐标，传入的 position 是选中节点以上的坐标之和，没有算上自己，在这里把选中坐标也加上
            data.append(&mut gen_name_position_data(&child.name, pos));
            if let Some(shape_data) = shape_data {
                // data.append(&mut gen_prim_data(instance, shape_data,ShapeModule::DESI));
            } else {
                // let mut cata_element_data = create_cata_element_data(refno, instance, &database).await?;
                // if !cata_element_data.is_empty() {
                //     data.append(&mut cata_element_data);
                // }
            }
        } else {
            // 没有模型的节点
            data.append(&mut gen_name_position_data(&child.name, pos));
        }
        if let None = tree.root_node_id() {
            let root = tree.insert(Node::new((refno, data)), AsRoot)?;
            node_id_map.entry(refno).or_insert(root);
        } else {
            let owner = child.owner;
            if let Some(node_id) = node_id_map.get(&owner) {
                let id = tree.insert(Node::new((refno, data)), UnderNode(node_id))?;
                node_id_map.entry(refno).or_insert(id);
            }
        }
    }
    Ok(tree)
}

async fn create_element_data_rvm_tree(cur_refno: RefU64, database: &ArDatabase, pool: &Pool<MySql>, aios_mgr: &AiosDBManager) -> anyhow::Result<Tree<(RefU64, Vec<u8>)>> {
    // let current_element = query_ele_node(cur_refno, pool).await; // 返回选中节点的elenode数据
    // if current_element.is_err() { return Ok(Tree::default()); }
    // let mut node_id_map = HashMap::new();
    // let current_element: PdmsElement = current_element.unwrap().into();
    //
    let mut tree = Tree::new();
    // // 将选中节点设为头节点
    // let pos = query_position_from_id(cur_refno, aios_mgr).await?.unwrap_or(Vec3::ZERO);
    // let root = tree.insert(Node::new((cur_refno, gen_name_position_data(&current_element.name, pos))), AsRoot)?;
    // node_id_map.entry(cur_refno).or_insert(root);
    // // 先将树生成，然后挂数据
    // let mut pending_children = VecDeque::new();
    // let children = query_children_order_aql(database, cur_refno).await?;
    // pending_children.extend(children);
    // while !pending_children.is_empty() {
    //     let cur_ele = pending_children.pop_front().unwrap();
    //     let refno = cur_ele.refno;
    //     if let Some(owner_id) = node_id_map.get(&cur_ele.owner) {
    //         let pos = query_position_from_id(refno, aios_mgr).await?.unwrap_or(Vec3::ZERO);
    //         let cur_node_id = tree.insert(Node::new((refno, gen_name_position_data(&cur_ele.name, pos))), UnderNode(owner_id))?;
    //         node_id_map.entry(refno).or_insert(cur_node_id);
    //     }
    //     let cur_children = query_children_order_aql(database, refno).await?;
    //     for cur_child in cur_children {
    //         if GENRAL_NEG_NOUN_NAMES.contains(&cur_child.noun.as_str()) {
    //             continue;
    //         }
    //         pending_children.push_back(cur_child);
    //     }
    // }
    // let instance = query_instance_with_refno_in_arangodb(cur_refno, &database).await?;
    // if instance.is_none() { return Ok(tree); }
    // let instances = instance.unwrap();
    //
    // for instance in &instances {
    //     // let mut data = Vec::new();
    //     // let refno = instance.refno;
    //     // let child = query_ele_node(refno, pool).await?;
    //     // if GENRAL_NEG_NOUN_NAMES.contains(&child.noun.as_str()) { continue; }
    //     // let mut b_desi_cyli = &child.noun == "CYLI";
    //     // let pos = instance.world_transform.translation;
    //     // // let mut b_visible = 0;
    //     // // for data in &instance.geo_basics {
    //     // //     if !data.visible { b_visible += 1; }
    //     // // }
    //     // // if b_visible >= instance.geo_basics.len() {
    //     // //     continue;
    //     // // }
    //     // data.append(&mut gen_name_position_data(&child.name, pos));
    //     // let desi_transform = instance.world_transform;
    //     // for geo_instance in &instance.geo_basics {
    //     //     data.append(&mut gen_prim_data_test(geo_instance, desi_transform, b_desi_cyli));
    //     // }
    //     //
    //     // if let Some(node_id) = node_id_map.get(&refno) {
    //     //     let mut tree_data = tree.get_mut(node_id).unwrap().data_mut();
    //     //     tree_data.1 = data;
    //     // }
    // }
    Ok(tree)
}

pub async fn convert_shape_type_data(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let pool = aios_mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(None); }
    let (_, pool) = pool.unwrap();
    let cache_basic = aios_mgr.get_refno_basic(refno);
    if cache_basic.is_none() { return Ok(None); }
    let cache_basic = cache_basic.unwrap();
    return match cache_basic.table.to_uppercase().as_str() {
        "BOX" => { get_box_shape_data(refno, cache_basic.value(), &pool).await }
        "CYLI" => { get_cylinder_shape_data(refno, cache_basic.value(), &pool).await }
        "CONE" => { get_cone_shape_data(refno, cache_basic.value(), &pool).await }
        "CTOR" => { get_ctor_shape_data(refno, cache_basic.value(), &pool).await }
        "DISH" => { get_dish_shape_data(refno, cache_basic.value(), &pool).await }
        "PYRA" => { get_pyramid_shape_data(refno, cache_basic.value(), &pool).await }
        "RTOR" => { get_rtor_shape_data(refno, cache_basic.value(), &pool).await }
        _ => { Ok(None) }
    };
}

// 已废弃: cache 模块已移除，以下所有函数已注释
/*
async fn get_box_shape_data(refno: RefU64, cache_basic: &CachedRefBasic, pool: &Pool<MySql>) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let attr = query_implicit_attr(refno, cache_basic, pool, Some(vec!["XLEN", "YLEN", "ZLEN"])).await?;
    let x_length = attr.get_f32("XLEN");
    let y_length = attr.get_f32("YLEN");
    let z_length = attr.get_f32("ZLEN");
    if x_length.is_none() || y_length.is_none() || z_length.is_none() { return Ok(None); }
    let x_length = x_length.unwrap();
    let y_length = y_length.unwrap();
    let z_length = z_length.unwrap();
    Ok(Some(RvmShapeTypeData::Box([x_length, y_length, z_length])))
}

async fn get_cylinder_shape_data(refno: RefU64, cache_basic: &CachedRefBasic, pool: &Pool<MySql>) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let attr = query_implicit_attr(refno, cache_basic, pool, Some(vec!["DIAM", "HEIG"])).await?;
    let diameter = attr.get_f32("DIAM");
    let height = attr.get_f32("HEIG");
    if diameter.is_none() || height.is_none() { return Ok(None); }
    let diameter = (diameter.unwrap() * 50.0).round() / 100.0;
    let height = height.unwrap();
    Ok(Some(RvmShapeTypeData::Cylinder([diameter, height])))
}

async fn get_cone_shape_data(refno: RefU64, cache_basic: &CachedRefBasic, pool: &Pool<MySql>) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let attr = query_implicit_attr(refno, cache_basic, pool, Some(vec!["DTOP", "DBOT", "HEIG"])).await?;
    let top_radius = attr.get_f32("DTOP");
    let bottom_radius = attr.get_f32("DBOT");
    let height = attr.get_f32("HEIG");
    if top_radius.is_none() || bottom_radius.is_none() || height.is_none() { return Ok(None); }
    let top_radius = (top_radius.unwrap() * 50.0).round() / 100.0;
    let bottom_radius = (bottom_radius.unwrap() * 50.0).round() / 100.0;
    let height = height.unwrap();
    Ok(Some(RvmShapeTypeData::Snout([bottom_radius, top_radius, height, 0.0, 0., 0., 0., 0., 0.])))
}

async fn get_dish_shape_data(refno: RefU64, cache_basic: &CachedRefBasic, pool: &Pool<MySql>) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let attr = query_implicit_attr(refno, cache_basic, pool, Some(vec!["DIAM", "HEIG"])).await?;
    let diameter = attr.get_f32("DIAM");
    let height = attr.get_f32("HEIG");
    if diameter.is_none() || height.is_none() { return Ok(None); }
    let top_radius = (diameter.unwrap() * 50.0).round() / 100.0;
    let height = (height.unwrap() * 100.0).round() / 100.0;
    Ok(Some(RvmShapeTypeData::SphericalDish([top_radius, height])))
}

async fn get_ctor_shape_data(refno: RefU64, cache_basic: &CachedRefBasic, pool: &Pool<MySql>) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let attr = query_implicit_attr(refno, cache_basic, pool, Some(vec!["RINS", "ROUT", "ANGL"])).await?;
    let r_inside = attr.get_f32("RINS");
    let r_outside = attr.get_f32("ROUT");
    let angl = attr.get_f32("ANGL");
    if r_inside.is_none() || r_outside.is_none() || angl.is_none() { return Ok(None); }
    let r_inside = r_inside.unwrap();
    let r_outside = r_outside.unwrap();
    let arc_length_radius = ((r_inside + r_outside) / 2.0 * 100.0).round() / 100.0; // 弧长的半径
    let radius = ((r_outside - r_inside) / 2.0 * 100.0).round() / 100.0; // 内圆半径
    let angl = (angl.unwrap() / 180.0 * f32::PI * 10000000.0).round() / 10000000.0;
    Ok(Some(RvmShapeTypeData::CircularTorus([arc_length_radius, radius, angl])))
}

async fn get_pyramid_shape_data(refno: RefU64, cache_basic: &CachedRefBasic, pool: &Pool<MySql>) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let attr = query_implicit_attr(refno, cache_basic, pool, Some(vec!["XTOP", "YTOP", "XBOT", "YBOT", "HEIG", "XOFF", "YOFF"])).await?;
    let x_top = attr.get_f32("XTOP");
    let y_top = attr.get_f32("YTOP");
    let x_bottom = attr.get_f32("XBOT");
    let y_bottom = attr.get_f32("YBOT");
    let x_offset = attr.get_f32("XOFF");
    let y_offset = attr.get_f32("YOFF");
    let height = attr.get_f32("HEIG");
    if x_top.is_none() || y_top.is_none() || x_bottom.is_none() || y_bottom.is_none() || x_offset.is_none() || y_offset.is_none() || height.is_none() { return Ok(None); }
    let x_top = (x_top.unwrap() * 100.0).round() / 100.0;
    let y_top = (y_top.unwrap() * 100.0).round() / 100.0;
    let x_bottom = (x_bottom.unwrap() * 100.0).round() / 100.0;
    let y_bottom = (y_bottom.unwrap() * 100.0).round() / 100.0;
    let x_offset = (x_offset.unwrap() * 100.0).round() / 100.0;
    let y_offset = (y_offset.unwrap() * 100.0).round() / 100.0;
    let height = (height.unwrap() * 100.0).round() / 100.0;
    Ok(Some(RvmShapeTypeData::Pyramid([x_bottom, y_bottom, x_top, y_top, x_offset, y_offset, height])))
}

async fn get_rtor_shape_data(refno: RefU64, cache_basic: &CachedRefBasic, pool: &Pool<MySql>) -> anyhow::Result<Option<RvmShapeTypeData>> {
    let attr = query_implicit_attr(refno, cache_basic, pool, Some(vec!["RINS", "ROUT", "HEIG", "ANGL"])).await?;
    let r_inside = attr.get_f32("RINS");
    let r_outside = attr.get_f32("ROUT");
    let angle = attr.get_f32("ANGL");
    let height = attr.get_f32("HEIG");
    if r_inside.is_none() || r_outside.is_none() || height.is_none() || angle.is_none() { return Ok(None); }
    let r_inside = r_inside.unwrap();
    let r_outside = r_outside.unwrap();
    let angle = angle.unwrap();
    let height = height.unwrap();
    let angle = (angle / 180.0 * f32::PI * 10000000.0).round() / 10000000.0;
    Ok(Some(RvmShapeTypeData::RectangularTorus([r_inside, r_outside, height, angle])))
}
*/

pub(crate) fn gen_ancestor_data_str(name: &str, pos: Vec3) -> Vec<u8> {
    format!("CNTB\r\n     1     2\r\n{}\r\n          {:.2}          {:.2}          {:.2}\r\n     1\r\n", name, pos.x, pos.y, pos.z).into_bytes()
}

#[tokio::test]
async fn test_create_rvm_file() -> anyhow::Result<()> {
    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let refno = RefU64::from_str("23584/5417").unwrap();
    let data = create_rvm_file(refno, &mgr).await?;
    let mut file = std::fs::File::create("test_rvm.rvm").unwrap();
    file.write_all(&data).unwrap();
    Ok(())
}

#[tokio::test]
async fn test_create_owner_data() -> anyhow::Result<()> {
    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let refno = RefU64::from_str("23584/5495").unwrap();
    let database = mgr.get_arango_db().await?;
    let data = create_owner_data(refno, &mgr, &database).await?;
    let mut file = std::fs::File::create("test_rvm.txt").unwrap();
    file.write_all(&data).unwrap();
    Ok(())
}

/// 元件库的圆柱 h/2 问题
#[tokio::test]
async fn test_cylinder_height() -> anyhow::Result<()> {
    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let refno = RefU64::from_str("23584/107").unwrap();
    let data = create_rvm_file(refno, &mgr).await?;
    let mut file = std::fs::File::create("test_rvm.rvm").unwrap();
    file.write_all(&data).unwrap();
    Ok(())
}

/// 管道生成的测试
/// 1. aabb 保存的值和 rvm的值对不上 ， 2. 管道的 tubi 没保存
#[tokio::test]
async fn test_cata_aabb() -> anyhow::Result<()> {
    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let refno = RefU64::from_str("23584/5515").unwrap();
    let data = create_rvm_file(refno, &mgr).await?;
    let mut file = std::fs::File::create("test_rvm.rvm").unwrap();
    file.write_all(&data).unwrap();
    Ok(())
}
