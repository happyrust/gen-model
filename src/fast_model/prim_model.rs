use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::gen_model::is_e3d_debug_enabled;
use crate::fast_model::{SEND_INST_SIZE, get_generic_type, shared};
use crate::{consts::*, e3d_dbg};
use aios_core::RefU64;
use aios_core::geometry::*;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::prim_geo::polyhedron::Polygon;
use aios_core::prim_geo::*;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use bevy_transform::components::Transform;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use parry3d::math::Isometry;
use std::collections::HashMap;
use std::mem::take;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

/// 生成基本体的几何数据
pub async fn gen_prim_geos(
    db_option: Arc<DbOption>,
    prim_refnos: &[RefnoEnum],
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let batch_size = db_option.gen_model_batch_size;
    let prim_cnt = prim_refnos.len();

    e3d_dbg!("[gen_prim_geos] 开始生成基本体几何数据");
    e3d_dbg!("[gen_prim_geos] 总数量: {}", prim_cnt);
    e3d_dbg!("[gen_prim_geos] 配置批次大小: {}", batch_size);

    if prim_cnt == 0 {
        e3d_dbg!("[gen_prim_geos] 警告: 基本体数量为0，直接返回");
        return Ok(true);
    }
    let mut batch_chunks_cnt = 8usize.min(prim_cnt.max(1));
    let mut batch_size = (prim_cnt + batch_chunks_cnt - 1) / batch_chunks_cnt;
    if batch_size == 0 {
        batch_size = 1;
    }
    //如果只有一个元件，就不分块了
    if batch_size == 1 {
        batch_chunks_cnt = prim_cnt;
    } else {
        batch_chunks_cnt = (prim_cnt + batch_size - 1) / batch_size;
    }

    e3d_dbg!(
        "[gen_prim_geos] 分块策略: {} 个批次, 每批 {} 个元素",
        batch_chunks_cnt,
        batch_size
    );

    let mut handles = vec![];
    let all_refnos = Arc::new(prim_refnos.to_vec());
    let processed_cnt = Arc::new(Mutex::new(prim_cnt));
    for i in 0..batch_chunks_cnt {
        let all_refnos = all_refnos.clone();
        let processed_cnt = processed_cnt.clone();
        let sender = sender.clone();
        let handle = tokio::spawn(async move {
            let batch_start_time = Instant::now();
            let mut shape_insts_data = ShapeInstancesData::default();
            let start_idx = i * batch_size;
            if start_idx >= prim_cnt {
                e3d_dbg!(
                    "[gen_prim_geos] 批次 {} 起始索引 {} 超出总长度 {}, 直接跳过",
                    i,
                    start_idx,
                    prim_cnt
                );
                return Ok::<_, anyhow::Error>(());
            }
            let mut end_idx = start_idx + batch_size;
            if end_idx > prim_cnt {
                end_idx = prim_cnt;
            }
            let batch_item_count = end_idx - start_idx;

            e3d_dbg!(
                "[gen_prim_geos] 批次 {} 开始: 索引范围 {} ~ {}, 共 {} 个元素",
                i,
                start_idx,
                end_idx,
                batch_item_count
            );

            e3d_dbg!("当前范围: {start_idx} ~ {end_idx}");
            let mut processed_in_batch = 0;
            let mut skipped_in_batch = 0;
            let mut sent_count = 0;

            for j in start_idx..end_idx {
                let refno = all_refnos[j];
                let remaining = {
                    let mut cnt = processed_cnt.lock().await;
                    *cnt -= 1;
                    *cnt
                };

                e3d_dbg!(
                    "批次 {} 处理索引 {}: refno={}, 剩余={}",
                    i,
                    j,
                    refno.to_string(),
                    remaining
                );

                let trans_result = aios_core::get_world_transform(refno).await;
                let Ok(Some(mut trans_origin)) = trans_result else {
                    skipped_in_batch += 1;
                    match trans_result {
                        Err(e) => {
                            e3d_dbg!(
                                "批次 {} 跳过 refno={}: 获取世界变换失败 - {:?}",
                                i,
                                refno.to_string(),
                                e
                            );
                        }
                        Ok(None) => {
                            e3d_dbg!(
                                "批次 {} 跳过 refno={}: 世界变换为 None",
                                i,
                                refno.to_string()
                            );
                        }
                        _ => {}
                    }
                    continue;
                };
                let mut geo_insts = vec![];
                let mut transform = Transform::IDENTITY;

                let attr = aios_core::get_named_attmap(refno).await.unwrap_or_default();
                let visible = attr.is_visible_by_level(None).unwrap_or(true);
                let mut geos_info = EleGeosInfo {
                    refno,
                    sesno: attr.sesno(),
                    visible,
                    generic_type: get_generic_type(refno).await.unwrap_or_default(),
                    aabb: None,
                    world_transform: trans_origin,
                    ..Default::default()
                };
                let mut geo_param = PdmsGeoParam::Unknown;
                let cur_type = attr.get_type_str();

                e3d_dbg!(
                    "批次 {} 处理 refno={}, type={}, visible={}",
                    i,
                    refno.to_string(),
                    cur_type,
                    visible
                );

                //需要限制负实体的大小，太大，导致负运算失败
                let neg_limit_size: Option<f32> = if GENRAL_NEG_NOUN_NAMES.contains(&cur_type) {
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
                let csg_shape = if cur_type == "POHE" || cur_type == "POLYHE" {
                    let pgo_refnos = aios_core::get_children_refnos(refno)
                        .await
                        .unwrap_or_default();
                    //需要检查第一个是不是POLPTL 类型
                    if pgo_refnos.is_empty() {
                        continue;
                    }
                    let first_type = aios_core::get_type_name(pgo_refnos[0])
                        .await
                        .unwrap_or_default();
                    // dbg!(&first_type);
                    let mut polygons = vec![];
                    let mut is_polyhe = false;
                    if first_type == "POLPTL" {
                        is_polyhe = true;
                        // let mut plant_mesh = PlantMesh::default();
                        let mut verts_map = HashMap::new();
                        let v_att = aios_core::collect_descendant_filter_ids(
                            &[pgo_refnos[0]],
                            &["POIN"],
                            None,
                        )
                        .await
                        .unwrap_or_default();
                        // dbg!(v_att.len());
                        for (i, v) in v_att.into_iter().enumerate() {
                            // dbg!(&v);
                            let v_attmap = aios_core::get_named_attmap(v).await.unwrap_or_default();
                            let pos = v_attmap.get_position().unwrap_or_default();
                            verts_map.insert(v, pos);
                            // verts_map.insert(v, i);
                        }
                        let index_loops =
                            aios_core::query_filter_deep_children_atts(refno, &["LOOPTS"])
                                .await
                                .unwrap_or_default();
                        // dbg!(index_loops.len());
                        // let tmp_refnos = index_loops.iter().map(|x| x.get_owner()).collect::<Vec<_>>();
                        // dbg!(&tmp_refnos);
                        // dbg!(tmp_refnos.len());
                        //按照 owner 进行分组，生成hashmap
                        let index_map = index_loops.iter().fold(HashMap::new(), |mut map, x| {
                            let owner = x.get_owner();
                            let vx_refnos = x.get_refno_vec("VXREF").unwrap_or_default();
                            //同一个分组下的，直接融合就可以
                            map.entry(owner).or_insert_with(Vec::new).extend(vx_refnos);
                            map
                        });
                        // dbg!(index_map.len());
                        let loop_atts =
                            aios_core::query_filter_deep_children_atts(refno, &["POLOOP"])
                                .await
                                .unwrap_or_default();
                        // dbg!(loop_atts.len());
                        let loops_map = loop_atts.iter().fold(HashMap::new(), |mut map, x| {
                            let owner = x.get_owner();
                            if let Some(index_refnos) = index_map.get(&x.get_refno_or_default()) {
                                // dbg!(index_refnos.len());
                                //同一个分组下的，直接融合就可以
                                map.entry(owner).or_insert_with(Vec::new).push(index_refnos);
                            }
                            map
                        });
                        for (_, v) in loops_map {
                            let mut loops = vec![];
                            for l in v {
                                let mut verts: Vec<Vec3> = vec![];
                                for index_refno in l {
                                    if let Some(vert) = verts_map.get(index_refno) {
                                        verts.push(*vert);
                                    }
                                }
                                loops.push(verts);
                            }
                            polygons.push(Polygon { loops });
                        }
                    } else {
                        for pgo_refno in pgo_refnos {
                            let mut verts = vec![];
                            // 使用新的泛型函数接口
                            let v_att = aios_core::collect_children_filter_attrs(pgo_refno, &[])
                                .await
                                .unwrap_or_default();
                            for v in v_att {
                                // dbg!(&v);
                                verts.push(v.get_position().unwrap_or_default());
                            }
                            polygons.push(Polygon { loops: vec![verts] });
                        }
                    }

                    // dbg!(&polygons);
                    let shape: Box<dyn BrepShapeTrait> = Box::new(Polyhedron {
                        polygons,
                        mesh: None,
                        is_polyhe,
                    });
                    Some(shape)
                } else {
                    attr.create_csg_shape(neg_limit_size)
                };
                let Some(csg_shape) = csg_shape else {
                    skipped_in_batch += 1;
                    e3d_dbg!(
                        "批次 {} 跳过 refno={}: 无法创建csg_shape",
                        i,
                        refno.to_string()
                    );
                    continue;
                };
                if !csg_shape.check_valid() {
                    skipped_in_batch += 1;
                    e3d_dbg!(
                        "批次 {} 跳过 refno={}: csg_shape验证失败",
                        i,
                        refno.to_string()
                    );
                    continue;
                }

                transform = csg_shape.get_trans();
                if transform.translation.is_nan()
                    || transform.rotation.is_nan()
                    || transform.scale.is_nan()
                {
                    skipped_in_batch += 1;
                    e3d_dbg!(
                        "批次 {} 跳过 refno={}: transform包含NaN值",
                        i,
                        refno.to_string()
                    );
                    continue;
                }
                geo_param = csg_shape
                    .convert_to_geo_param()
                    .unwrap_or(PdmsGeoParam::Unknown);
                let geo_hash = csg_shape.hash_unit_mesh_params();
                // dbg!(geo_hash);
                let inst_geo = EleInstGeo {
                    geo_hash,
                    refno,
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
                    let neg_refnos = aios_core::collect_descendant_filter_ids(
                        &[refno],
                        &GENRAL_NEG_NOUN_NAMES,
                        None,
                    )
                    .await
                    .unwrap_or_default();

                    if !neg_refnos.is_empty() {
                        e3d_dbg!(
                            "批次 {} refno={} 找到 {} 个负实体",
                            i,
                            refno.to_string(),
                            neg_refnos.len()
                        );
                    }

                    shape_insts_data.insert_negs(refno, &neg_refnos);
                    // dbg!(&neg_refnos);
                    geos_info.is_solid = geo_insts.iter().any(|x| x.geo_type == GeoBasicType::Pos);
                    let inst_key = geos_info.get_inst_key();
                    shape_insts_data.insert_geos_data(
                        inst_key.clone(),
                        EleInstGeosData {
                            inst_key,
                            refno,
                            insts: geo_insts,
                            aabb: None,
                            type_name: attr.get_type_str().to_string(),
                        },
                    );
                    shape_insts_data.insert_info(refno, geos_info);
                    processed_in_batch += 1;
                }

                if shape_insts_data.inst_cnt() >= SEND_INST_SIZE {
                    let inst_cnt = shape_insts_data.inst_cnt();
                    e3d_dbg!(
                        "[gen_prim_geos] 批次 {} 发送中间数据: {} 个实例",
                        i,
                        inst_cnt
                    );
                    sender
                        .send(std::mem::take(&mut shape_insts_data))
                        .expect("send prim shape_insts_data error");
                    sent_count += 1;
                    // dbg!("Send prim insts data");
                }
            }

            if shape_insts_data.inst_cnt() > 0 {
                let inst_cnt = shape_insts_data.inst_cnt();
                e3d_dbg!(
                    "[gen_prim_geos] 批次 {} 发送最后数据: {} 个实例",
                    i,
                    inst_cnt
                );
                sender
                    .send(shape_insts_data)
                    .expect("send prim shape_insts_data error");
                sent_count += 1;
                // dbg!("Send last prim insts data");
            }

            let batch_elapsed = batch_start_time.elapsed();
            e3d_dbg!(
                "[gen_prim_geos] 批次 {} 完成: 处理 {}/{} 个, 跳过 {} 个, 发送 {} 次, 耗时 {} ms",
                i,
                processed_in_batch,
                batch_item_count,
                skipped_in_batch,
                sent_count,
                batch_elapsed.as_millis()
            );

            Ok::<_, anyhow::Error>(())
        });

        handles.push(handle);
    }
    e3d_dbg!(
        "[gen_prim_geos] 等待所有 {} 个批次任务完成...",
        handles.len()
    );
    let results = futures::future::join_all(take(&mut handles)).await;

    let mut success_count = 0;
    let mut error_count = 0;
    for (idx, result) in results.iter().enumerate() {
        match result {
            Ok(Ok(_)) => success_count += 1,
            Ok(Err(e)) => {
                error_count += 1;
                e3d_dbg!("[gen_prim_geos] 批次 {} 执行错误: {:?}", idx, e);
            }
            Err(e) => {
                error_count += 1;
                e3d_dbg!("[gen_prim_geos] 批次 {} 任务失败: {:?}", idx, e);
            }
        }
    }

    let total_elapsed = t.elapsed();
    e3d_dbg!(
        "[gen_prim_geos] 完成! 总数: {}, 成功批次: {}, 失败批次: {}, 总耗时: {} ms",
        prim_cnt,
        success_count,
        error_count,
        total_elapsed.as_millis()
    );

    if is_e3d_debug_enabled() {
        println!(
            "处理常规基本几何体: {} 花费时间: {} ms",
            prim_cnt,
            total_elapsed.as_millis()
        );
    }
    Ok(true)
}
