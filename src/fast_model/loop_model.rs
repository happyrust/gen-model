use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::{SEND_INST_SIZE, get_generic_type, shared};
use aios_core::RefU64;
use aios_core::geometry::*;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::prim_geo::{Extrusion, Revolution};
use aios_core::shape::pdms_shape::{BrepShapeTrait, VerifiedShape};
use bevy_transform::components::Transform;
use dashmap::DashMap;
use glam::Vec3;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use std::mem::take;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

///处理带有loop的元件
pub async fn gen_loop_geos(
    db_option: Arc<DbOption>,
    loop_owner_refnos: &[RefnoEnum],
    sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let batch_size = db_option.gen_model_batch_size;
    let loop_owner_cnt = loop_owner_refnos.len();
    if loop_owner_cnt == 0 {
        return Ok(true);
    }
    //处理loop elements
    // 分块宽度与在飞数都从全局几何并发闸取（specs/023）：额度 = 1 时单块串行。
    let batch_size = crate::fast_model::concurrency::geometry_gate().chunk_size(loop_owner_cnt);
    let batch_chunks_cnt = loop_owner_cnt.div_ceil(batch_size);
    let mut handles = vec![];
    // dbg!(&loop_owner_refnos);
    let all_refnos = Arc::new(loop_owner_refnos.to_vec());
    for i in 0..batch_chunks_cnt {
        let all_loop_owner_refnos = all_refnos.clone();
        let sjus_map_clone = sjus_map_arc.clone();
        let sender = sender.clone();
        let handle = crate::fast_model::concurrency::spawn_gated_leaf(async move {
            let negative_nouns = shared::negative_noun_refs();
            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > loop_owner_cnt {
                end_idx = loop_owner_cnt;
            }
            println!("当前范围: {start_idx} ~ {end_idx}");
            let mut shape_insts_data = ShapeInstancesData::default();
            for j in start_idx..end_idx {
                let target_refno = all_loop_owner_refnos[j];
                let mut target_att = aios_core::get_named_attmap(target_refno)
                    .await
                    .unwrap_or_default();
                let target_type = target_att.get_type_str();
                let Ok(Some(mut trans_origin)) = aios_core::get_world_transform(target_refno).await
                else {
                    continue;
                };
                //判断父节点是否有SJUS，需要调整位置
                if (target_type == "FLOOR" || target_type == "PANE" || target_type == "GWALL")
                    && let Some(sjus_adjust) = sjus_map_clone.get(&target_refno)
                {
                    let offset = trans_origin.rotation.mul_vec3(sjus_adjust.value().0);
                    trans_origin.translation += offset;
                }

                if !shared::is_negative_noun(target_type) {
                    let neg_refnos =
                        aios_core::query_filter_children(target_refno, &negative_nouns)
                            .await
                            .unwrap_or_default();
                    // dbg!(&neg_refnos);
                    shape_insts_data.insert_negs(target_refno, &neg_refnos);
                    //检查是否有CMPF
                    let cmpf_refnos = aios_core::query_filter_children(target_refno, &["CMPF"])
                        .await
                        .unwrap_or_default();
                    if !cmpf_refnos.is_empty() {
                        //查询cmpf里面的元素
                        let cmpf_neg_refnos = aios_core::query_multi_filter_deep_children(
                            &cmpf_refnos,
                            &negative_nouns,
                        )
                        .await
                        .unwrap_or_default();
                        // dbg!(&cmpf_neg_refnos);
                        shape_insts_data.insert_negs(
                            target_refno,
                            &cmpf_neg_refnos.into_iter().map(|x| x).collect::<Vec<_>>(),
                        );
                    }
                }
                let mut geos_info = EleGeosInfo {
                    refno: target_refno,
                    sesno: target_att.sesno(),
                    cata_hash: None,
                    visible: true,
                    world_transform: trans_origin,
                    generic_type: get_generic_type(target_refno).await.unwrap_or_default(),
                    aabb: None,
                    flow_pt_indexs: vec![],
                    ..Default::default()
                };
                let mut geo_hash = 0;
                let mut item_trans = Transform::IDENTITY;
                let mut geo_param = PdmsGeoParam::Unknown;
                let Ok((verts, height)) = aios_core::fetch_loops_and_height(target_refno).await
                else {
                    continue;
                };
                // dbg!((&verts, height));
                match target_type {
                    "NREV" | "REVO" => {
                        let angle = target_att.get_f32("ANGL").unwrap_or_default();
                        if angle.abs() >= f32::EPSILON {
                            let revo = Box::new(Revolution {
                                verts,
                                angle,
                                ..Default::default()
                            });
                            if revo.check_valid() {
                                // dbg!(&revo);
                                item_trans = revo.get_trans();
                                geo_param =
                                    revo.convert_to_geo_param().unwrap_or(PdmsGeoParam::Unknown);
                                geo_hash = revo.hash_unit_mesh_params();
                            }
                        }
                    }
                    //todo 关于justline，可能需要jusline的信息才能判断中心点
                    "AEXTR" | "NXTR" | "EXTR" | "PANE" | "FLOOR" | "SCREED" | "GWALL" => {
                        if height < f32::EPSILON {
                            #[cfg(feature = "debug_model")]
                            println!("{}： 的height太小为: {}", target_refno, height);
                            continue;
                        }
                        // if loop_attr.get_type_str() == "NXTR" {
                        //     if let Some(parent_inst) =
                        //         shape_insts_data.get_inst_info(loop_attr.get_owner())
                        //     {
                        //         if let Some(h) =
                        //             parent_inst.aabb.map(|x| x.bounding_sphere().radius * 2.0)
                        //         {
                        //             height = height.min(h);
                        //             // dbg!(height);
                        //             println!("Height 太长，裁剪为: {}", height);
                        //         }
                        //     }
                        // };
                        //如果有多个loop，都放到 verts 里好了
                        let extrusion = Box::new(Extrusion {
                            verts,
                            height,
                            ..Default::default()
                        });
                        geo_param = extrusion
                            .convert_to_geo_param()
                            .unwrap_or(PdmsGeoParam::Unknown);
                        item_trans = extrusion.get_trans();
                        geo_hash = extrusion.hash_unit_mesh_params();
                    }
                    _ => {}
                }
                let visible = target_att.is_visible_by_level(None).unwrap_or(true);
                geos_info.visible = visible;
                if item_trans.is_nan() {
                    continue;
                }
                let tr: Transform = item_trans;
                //需要判断多个PLOO、LOOP的情况，第二个开始都是负实体
                let geom_inst = EleInstGeo {
                    geo_hash,
                    refno: target_refno,
                    pts: Default::default(),
                    aabb: None,
                    transform: tr,
                    visible,
                    is_tubi: false,
                    geo_param: geo_param.clone(),
                    geo_type: if shared::is_negative_noun(target_type) {
                        GeoBasicType::Neg
                    } else {
                        GeoBasicType::Pos
                    },
                    cata_neg_refnos: Default::default(),
                };
                geos_info.is_solid = geom_inst.geo_type == GeoBasicType::Pos;
                shape_insts_data.insert_geos_data(
                    target_refno.to_string(),
                    EleInstGeosData {
                        inst_key: geos_info.get_inst_key(),
                        refno: target_refno,
                        insts: vec![geom_inst.clone()],
                        aabb: None,
                        type_name: target_att.get_type_str().to_string(),
                    },
                );
                shape_insts_data.insert_info(target_refno, geos_info);

                if shape_insts_data.inst_cnt() >= SEND_INST_SIZE {
                    crate::fast_model::shape_save::send_shape_batch(
                        &sender,
                        std::mem::take(&mut shape_insts_data),
                    )
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("send loop shape instances failed: {error}")
                    })?;
                    // dbg!("Send loop insts data");
                }
            }

            if shape_insts_data.inst_cnt() > 0 {
                crate::fast_model::shape_save::send_shape_batch(&sender, shape_insts_data)
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("send loop shape instances failed: {error}")
                    })?;
                // dbg!("Send last loop insts data");
            }
            Ok::<_, anyhow::Error>(())
        });

        handles.push(handle);
    }
    for result in futures::future::join_all(take(&mut handles)).await {
        result??;
    }
    println!(
        "处理loops几何体: {} 花费时间: {} ms",
        loop_owner_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use aios_core::prim_geo::wire::gen_polyline;
    use glam::Vec3;

    #[test]
    fn structural_floor_extreme_fillet_remains_finite() {
        let vertices = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 24450.0, 0.0),
            Vec3::new(24450.0, 24450.0, 24450.0),
            Vec3::new(24450.0, 0.0, 0.0),
        ];

        let polyline = gen_polyline(&vertices).expect("extreme fillet must remain renderable");

        assert!(polyline.vertex_data.len() >= 3);
        assert!(
            polyline
                .vertex_data
                .iter()
                .all(|vertex| vertex.x.is_finite()
                    && vertex.y.is_finite()
                    && vertex.bulge.is_finite())
        );

        // 整体断言走生产同款判定路径（T044）：这段原先挂在 `occ` 的 BRep 后端上，
        // 而 CI 口径（ws,gen_model,manifold,project_hd）根本不编它——一条从来不跑的
        // 断言。现在量的就是 `gen_inst_meshes` 真正用的 manifold 网格。
        #[cfg(feature = "manifold")]
        {
            use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
            use aios_core::prim_geo::Extrusion;

            let param = PdmsGeoParam::PrimExtrusion(Extrusion {
                verts: vec![vertices],
                height: 100.0,
                ..Default::default()
            });
            let mesh = crate::fast_model::manifold_tessellate::tessellate_libgm_param(&param)
                .expect("extrusion must tessellate")
                .expect("an extrusion is a shape, not the not-a-shape verdict");
            assert!(
                mesh.vertices
                    .iter()
                    .all(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite()),
                "extreme fillet must not leak non-finite vertices into the mesh"
            );
            let aabb = crate::fast_model::mesh_primitives::compute_aabb(&mesh.vertices)
                .expect("a non-empty mesh must yield an aabb");
            assert!(aabb.mins.coords.iter().all(|value| value.is_finite()));
            assert!(aabb.maxs.coords.iter().all(|value| value.is_finite()));
        }
    }
}
