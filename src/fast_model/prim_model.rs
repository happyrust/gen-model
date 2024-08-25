use std::collections::HashMap;
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::{get_generic_type, SEND_INST_SIZE, shared};
use aios_core::geometry::*;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::prim_geo::polyhedron::Polygon;
use aios_core::prim_geo::*;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
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
    db_option: Arc<DbOption>,
    prim_refnos: &[RefU64],
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let batch_size = db_option.gen_model_batch_size;
    let prim_cnt = prim_refnos.len();
    if prim_cnt == 0 {
        return Ok(true);
    }
    let mut batch_chunks_cnt = 8;
    let mut batch_size = prim_cnt / batch_chunks_cnt + 1;
    //如果只有一个元件，就不分块了
    if batch_size == 1 {
        batch_chunks_cnt = prim_cnt;
    }
    let mut handles = vec![];
    let all_refnos = Arc::new(prim_refnos.to_vec());
    let processed_cnt = Arc::new(Mutex::new(prim_cnt));
    for i in 0..batch_chunks_cnt {
        let all_refnos = all_refnos.clone();
        let processed_cnt = processed_cnt.clone();
        let sender = sender.clone();
        let handle = tokio::spawn(async move {
            let mut shape_insts_data = ShapeInstancesData::default();
            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > prim_cnt as usize {
                end_idx = prim_cnt as usize;
            }
            println!("当前范围: {start_idx} ~ {end_idx}");
            for j in start_idx..end_idx {
                let refno = all_refnos[j];
                // println!(
                //     "正在处理基本体的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                //     j,
                //     refno.to_string(),
                //     processed_cnt.lock().await.to_owned()
                // );
                *processed_cnt.lock().await -= 1;
                let Ok(Some(mut trans_origin)) = aios_core::get_world_transform(refno).await else {
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
                //需要限制负实体的大小，太大，导致负运算失败
                let neg_limit_size: Option<f32> =
                    if GENRAL_NEG_NOUN_NAMES.contains(&cur_type) {
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
                let brep_shape = if cur_type == "POHE" || cur_type == "POLYHE" {
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
                        let v_att = aios_core::query_filter_children_atts(pgo_refnos[0], &["POIN"])
                            .await
                            .unwrap_or_default();
                        // dbg!(v_att.len());
                        for (i, v) in v_att.into_iter().enumerate() {
                            // dbg!(&v);
                            let pos = v.get_position().unwrap_or_default();
                            verts_map.insert(v.get_refno_or_default(), pos);
                            // verts_map.insert(v.get_refno_or_default(), i);
                        }
                        let index_loops = aios_core::query_filter_deep_children_atts(
                            refno,
                            &["LOOPTS"],
                        ).await.unwrap_or_default();
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
                        let loop_atts = aios_core::query_filter_deep_children_atts(refno, &["POLOOP"])
                            .await
                            .unwrap_or_default();
                        // dbg!(loop_atts.len());
                        let loops_map = loop_atts.iter().fold(HashMap::new(), |mut map, x| {
                            let owner = x.get_owner();
                            let index_refnos = index_map.get(&x.get_refno_or_default()).unwrap();
                            // dbg!(index_refnos.len());
                            //同一个分组下的，直接融合就可以
                            map.entry(owner).or_insert_with(Vec::new).push(index_refnos);
                            map
                        });
                        // for (k, v) in &loops_map {
                        //     if v.len() > 1 {
                        //         dbg!(k);
                        //     }
                        // }
                        for (_, v) in loops_map {
                            let mut loops = vec![];
                            for l in v {
                                let mut verts = vec![];
                                for index_refno in l {
                                    if let Some(vert) = verts_map.get(index_refno) {
                                        verts.push(vert.clone());
                                    }
                                }
                                loops.push(verts);
                            }
                            polygons.push(Polygon { loops });
                        }
                    }else{
                        for pgo_refno in pgo_refnos {
                            let mut verts = vec![];
                            let v_att = aios_core::get_children_named_attmaps(pgo_refno)
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
                    let shape: Box<dyn BrepShapeTrait> = Box::new(Polyhedron { polygons, mesh: None, is_polyhe });
                    Some(shape)
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
                    let neg_refnos =
                        aios_core::query_filter_children(refno, &GENRAL_NEG_NOUN_NAMES)
                            .await
                            .unwrap_or_default();
                    shape_insts_data.insert_negs(refno, &neg_refnos);
                    // dbg!(&neg_refnos);
                    geos_info.is_solid = geo_insts.iter().any(|x| x.geo_type == GeoBasicType::Pos);
                    shape_insts_data.insert_geos_data(
                        refno.to_string(),
                        EleInstGeosData {
                            inst_key: geos_info.get_inst_key(),
                            refno,
                            insts: geo_insts,
                            aabb: None,
                            type_name: attr.get_type_str().to_string(),
                        },
                    );
                    shape_insts_data.insert_info(refno, geos_info);
                }

                if shape_insts_data.inst_cnt() >=  SEND_INST_SIZE {
                    sender
                        .send(std::mem::take(&mut shape_insts_data))
                        .expect("send prim shape_insts_data error");
                    // dbg!("Send prim insts data");
                }
            }

            if shape_insts_data.inst_cnt() > 0 {
                sender
                    .send(shape_insts_data)
                    .expect("send prim shape_insts_data error");
                // dbg!("Send last prim insts data");
            }
            Ok::<_, anyhow::Error>(())
        });

        handles.push(handle);
    }
    futures::future::join_all(take(&mut handles)).await;
    println!(
        "处理常规基本几何体: {} 花费时间: {} ms",
        prim_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}
