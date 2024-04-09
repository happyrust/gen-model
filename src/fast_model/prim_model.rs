use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::shared;
use aios_core::geometry::*;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::prim_geo::facet::*;
use aios_core::prim_geo::*;
use aios_core::shape::pdms_shape::{BrepShapeTrait, VerifiedShape};
use aios_core::RefU64;
use bevy_transform::components::Transform;
use parry3d::bounding_volume::Aabb;
use parry3d::math::Isometry;
use std::mem::take;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

/// 生成基本体的几何数据
pub async fn gen_prim_geos(
    mgr: Arc<AiosDBManager>,
    instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    prim_refnos: &[RefU64],
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let db_option = &mgr.db_option;
    let batch_size = db_option.gen_model_batch_size;
    let prim_cnt = prim_refnos.len();
    if prim_cnt == 0 {
        return Ok(true);
    }
    let batch_chunks_cnt = prim_cnt / batch_size + 1;
    let mut handles = vec![];
    let all_refnos = Arc::new(prim_refnos.to_vec());
    let processed_cnt = Arc::new(Mutex::new(prim_cnt));
    for i in 0..batch_chunks_cnt as usize {
        let mgr_clone = mgr.clone();
        let instance_mgr = instance_mgr.clone();

        let all_refnos = all_refnos.clone();
        let processed_cnt = processed_cnt.clone();
        let handle = tokio::spawn(async move {
            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > prim_cnt as usize {
                end_idx = prim_cnt as usize;
            }
            for j in start_idx..end_idx {
                let mut shape_insts_data = instance_mgr.write().await;
                let refno = all_refnos[j];
                println!(
                    "正在处理基本体的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                    j,
                    refno.to_string(),
                    processed_cnt.lock().await.to_owned()
                );
                *processed_cnt.lock().await -= 1;
                let Ok(Some(mut trans_origin)) = mgr_clone.get_world_transform(refno).await else {
                    continue;
                };
                let mut geo_insts = vec![];
                let mut transform = Transform::IDENTITY;

                let attr = aios_core::get_named_attmap(refno).await.unwrap_or_default();
                let visible = attr.is_visible_by_level(None).unwrap_or(true);
                let mut geos_info = EleGeosInfo {
                    refno,
                    visible,
                    generic_type: mgr_clone.get_generic_type(refno).await,
                    aabb: None,
                    world_transform: trans_origin,
                    ..Default::default()
                };
                let mut geo_param = PdmsGeoParam::Unknown;
                //需要限制负实体的大小，太大，导致负运算失败
                let neg_limit_size: Option<f32> = if GENRAL_NEG_NOUN_NAMES.contains(&attr.get_type_str()) {
                    // if let Some(parent_inst) = shape_insts_data.inst_info_map.get(&attr.get_owner()) {
                    //     parent_inst
                    //         .aabb
                    //         .map(|x| x.bounding_sphere().radius * 4.0)
                    // } else {
                        //负实体默认的最大大小，不能超过
                       Some(1000_000.0)
                    // }
                } else {
                    None
                };
                // dbg!((attr.get_type_str(), refno, neg_limit_size));
                //多面体的处理
                let brep_shape = if attr.get_type_str() == "POHE" {
                    let pgo_refnos = aios_core::get_children_refnos(refno)
                        .await
                        .unwrap_or_default();
                    let mut polygons = vec![];
                    for pgo_refno in pgo_refnos {
                        let mut verts = vec![];
                        let v_att = aios_core::get_children_named_attmaps(pgo_refno)
                            .await
                            .unwrap_or_default();
                        for v in v_att {
                            verts.push(v.get_position().unwrap_or_default());
                        }
                    }
                    let obj: Box<dyn BrepShapeTrait> = Box::new(Polyhedron { polygons });
                    Some(obj)
                } else {
                    attr.create_brep_shape(neg_limit_size)
                };
                let Some(brep_shape) = brep_shape else {
                    continue;
                };
                if !brep_shape.check_valid() {
                    continue;
                }

                transform = brep_shape.get_trans();
                if transform.is_nan() {
                    continue;
                }
                geo_param = brep_shape
                    .convert_to_geo_param()
                    .unwrap_or(PdmsGeoParam::Unknown);
                let geo_hash = brep_shape.hash_unit_mesh_params();
                let inst_geo = EleInstGeo {
                    geo_hash,
                    refno,
                    owner_pos_refnos: Default::default(),
                    pts: Default::default(),
                    aabb: None,
                    transform,
                    geo_param,
                    visible,
                    is_tubi: false,
                    geo_type: if attr.is_neg() {
                        GeoBasicType::Neg
                    } else {
                        GeoBasicType::Pos
                    },
                    cata_neg_refnos: vec![],
                };
                geo_insts.push(inst_geo);
                if geo_insts.len() > 0 {
                    geos_info.neg_refnos =
                        aios_core::query_filter_children(refno, &GENRAL_NEG_NOUN_NAMES)
                            .await
                            .unwrap_or_default();
                    // dbg!(&neg_refnos);

                    shape_insts_data.insert_info(refno, geos_info);
                    shape_insts_data.insert_geos_data(
                        refno.to_string(),
                        EleInstGeosData {
                            inst_key: refno.to_string(),
                            refno,
                            insts: geo_insts,
                            aabb: None,
                            type_name: attr.get_type_str().to_string(),
                            ptset_map: Default::default(),
                        },
                    );
                }
            }
        });
        handles.push(handle);
        if !db_option.multi_threads {
            if !handles.is_empty() {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
    }
    futures::future::join_all(take(&mut handles)).await;
    println!(
        "处理常规基本几何体: {} 花费时间: {} ms",
        prim_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}
