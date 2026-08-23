use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::{SEND_INST_SIZE, get_generic_type, shared};
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

fn invalid_brep_message(
    refno: impl std::fmt::Display,
    noun: &str,
    cylinder_dimensions: Option<(f32, f32)>,
) -> String {
    let dimensions = cylinder_dimensions
        .map(|(diameter, height)| format!("; DIAM={diameter}, HEIG={height}"))
        .unwrap_or_default();
    format!("primitive {refno} ({noun}) produced an invalid BREP shape{dimensions}")
}

/// 坏基本体与坏布尔是同一件事：记进 [`geom_error`](crate::data_interface::geom_error)
/// 这本账，然后跳过这一件，剩下的照常生成。
///
/// 这里曾经按 `targeted` 分叉——定向生成撞上一个坏基本体就 bail 掉整个生成根。
/// `targeted` 是请求模式（`debug_root_refnos.is_some()`）而不是正确性边界：源库里
/// 一个零尺寸的空 NCYL 因此让整棵 FRMW 常驻 500，同一份数据换个入口跑却只是少一
/// 件。数据坏了重试多少次都一样坏，记下来比把整根拖垮有用。
/// 控制台那行走 `println!` 而不是 `log::warn!`：`serve` 路径没有装 log 后端，
/// `log` 宏的输出一个字也落不到日志文件里，布尔那条链同样是 `println!`。
async fn skip_bad_primitive(target: &str, noun: &str, message: &str) {
    println!("基本体跳过: {message}");
    crate::data_interface::geom_error::note_primitive_skip(target, noun, message).await;
}

/// 生成基本体的几何数据
pub async fn gen_prim_geos(
    db_option: Arc<DbOption>,
    prim_refnos: &[RefnoEnum],
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    let targeted = db_option.debug_root_refnos.is_some();
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
        let handle =
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                let negative_nouns = shared::negative_noun_refs();
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
                    let mut trans_origin = match aios_core::get_world_transform(refno).await {
                        Ok(Some(transform)) => transform,
                        Ok(None) if targeted => {
                            anyhow::bail!("targeted primitive {refno} has no world transform")
                        }
                        Err(error) if targeted => anyhow::bail!(
                            "query targeted primitive {refno} world transform failed: {error:#}"
                        ),
                        _ => continue,
                    };
                    let mut geo_insts = vec![];
                    let mut transform = Transform::IDENTITY;

                    let attr = match aios_core::get_named_attmap(refno).await {
                        Ok(attr) => attr,
                        Err(error) if targeted => anyhow::bail!(
                            "query targeted primitive {refno} attributes failed: {error:#}"
                        ),
                        Err(_) => Default::default(),
                    };
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
                    let neg_limit_size: Option<f32> = if shared::is_negative_noun(cur_type) {
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
                            let v_att =
                                aios_core::query_filter_children_atts(pgo_refnos[0], &["POIN"])
                                    .await
                                    .unwrap_or_default();
                            // dbg!(v_att.len());
                            for (i, v) in v_att.into_iter().enumerate() {
                                // dbg!(&v);
                                let pos = v.get_position().unwrap_or_default();
                                verts_map.insert(v.get_refno_or_default(), pos);
                                // verts_map.insert(v.get_refno_or_default(), i);
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
                            let index_map =
                                index_loops.iter().fold(HashMap::new(), |mut map, x| {
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
                                if let Some(index_refnos) = index_map.get(&x.get_refno_or_default())
                                {
                                    // dbg!(index_refnos.len());
                                    //同一个分组下的，直接融合就可以
                                    map.entry(owner).or_insert_with(Vec::new).push(index_refnos);
                                }
                                map
                            });
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
                        } else {
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
                        let shape: Box<dyn BrepShapeTrait> = Box::new(Polyhedron {
                            polygons,
                            mesh: None,
                            is_polyhe,
                        });
                        Some(shape)
                    } else {
                        attr.create_brep_shape(neg_limit_size)
                    };
                    let Some(brep_shape) = brep_shape else {
                        let message =
                            format!("primitive {refno} ({cur_type}) produced no BREP shape");
                        skip_bad_primitive(&refno.to_pdms_str(), cur_type, &message).await;
                        continue;
                    };
                    if !brep_shape.check_valid() {
                        let cylinder_dimensions = matches!(cur_type, "CYLI" | "SLCY" | "NCYL")
                            .then(|| {
                                (
                                    attr.get_f32_or_default("DIAM"),
                                    attr.get_f32_or_default("HEIG"),
                                )
                            });
                        let message = invalid_brep_message(refno, cur_type, cylinder_dimensions);
                        skip_bad_primitive(&refno.to_pdms_str(), cur_type, &message).await;
                        continue;
                    }

                    transform = brep_shape.get_trans();
                    if transform.is_nan() {
                        let message =
                            format!("primitive {refno} ({cur_type}) produced a NaN transform");
                        skip_bad_primitive(&refno.to_pdms_str(), cur_type, &message).await;
                        continue;
                    }
                    if let Err(error) = crate::data_interface::geom_error::clear_primitive_failure(
                        &refno.to_pdms_str(),
                    )
                    .await
                    {
                        log::warn!(
                            "[geom_error] 基本体成功后的销账失败 target={}: {error:#}",
                            refno.to_pdms_str()
                        );
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
                        geo_type: if shared::is_negative_noun(cur_type) {
                            GeoBasicType::Neg
                        } else {
                            GeoBasicType::Pos
                        },
                        cata_neg_refnos: vec![],
                    };
                    geo_insts.push(inst_geo);
                    if geo_insts.len() > 0 {
                        let neg_refnos = aios_core::query_filter_children(refno, &negative_nouns)
                            .await
                            .unwrap_or_default();
                        shape_insts_data.insert_negs(refno, &neg_refnos);
                        // dbg!(&neg_refnos);
                        geos_info.is_solid =
                            geo_insts.iter().any(|x| x.geo_type == GeoBasicType::Pos);
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

                    if shape_insts_data.inst_cnt() >= SEND_INST_SIZE {
                        crate::fast_model::shape_save::send_shape_batch(
                            &sender,
                            std::mem::take(&mut shape_insts_data),
                        )
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!("send primitive shape instances failed: {error}")
                        })?;
                        // dbg!("Send prim insts data");
                    }
                }

                if shape_insts_data.inst_cnt() > 0 {
                    crate::fast_model::shape_save::send_shape_batch(&sender, shape_insts_data)
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!("send primitive shape instances failed: {error}")
                        })?;
                    // dbg!("Send last prim insts data");
                }
                Ok::<_, anyhow::Error>(())
            });

        handles.push(handle);
    }
    for result in futures::future::join_all(take(&mut handles)).await {
        result??;
    }
    println!(
        "处理常规基本几何体: {} 花费时间: {} ms",
        prim_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::invalid_brep_message;

    #[test]
    fn zero_sized_ncyl_reports_the_dimensions() {
        let message = invalid_brep_message("24381_38635", "NCYL", Some((0.0, 0.0)));
        assert!(message.contains("24381_38635 (NCYL)"), "{message}");
        assert!(message.contains("DIAM=0"), "{message}");
        assert!(message.contains("HEIG=0"), "{message}");
        assert!(message.contains("invalid BREP shape"), "{message}");
    }

    #[test]
    fn non_cylinder_invalid_brep_keeps_the_generic_diagnostic() {
        let message = invalid_brep_message("1_2", "BOX", None);
        assert_eq!(
            message,
            "primitive 1_2 (BOX) produced an invalid BREP shape"
        );
    }

    /// 2026-08-21 现场：源库里 `24381/38635` 是一个没名字也没尺寸的空 NCYL
    /// （`DIAM`/`HEIG` 都缺省成 0），OCC 建不出合法 BREP。定向生成为此 bail 掉整个
    /// 生成根，7997 sweep 里 FRMW `24381/38614` 就常驻 500、`regen_root` 连撞 5 次成
    /// 死信；同一份数据走全量入口却只是少画一件。请求模式不是正确性边界，坏数据
    /// 一律记账后跳过——与布尔那条链（`manifold_bool.rs` 的 `note_skip`）同一纪律。
    #[test]
    fn bad_primitive_data_is_ledgered_and_skipped_instead_of_failing_the_root() {
        let source = include_str!("prim_model.rs");
        let body = source.split_once("#[cfg(test)]").expect("test boundary").0;

        assert_eq!(
            body.matches("skip_bad_primitive(&refno.to_pdms_str()")
                .count(),
            3,
            "缺失 BREP、非法 BREP、NaN 变换三处都要走同一个记账跳过口: {body}"
        );
        for anchor in [
            "produced no BREP shape",
            "invalid_brep_message(refno,",
            "produced a NaN transform",
        ] {
            let site = body.split_once(anchor).expect("failure site").1;
            let next = site.split_once("continue;").expect("skip site").0;
            assert!(
                next.contains("skip_bad_primitive("),
                "{anchor} 必须先记账再跳过: {next}"
            );
            assert!(
                !next.contains("bail!"),
                "{anchor} 不得再把坏数据升级成生成失败: {next}"
            );
        }

        // 账本之外还得在控制台留一行：`serve` 没装 log 后端，`log::warn!` 落不下来。
        let helper = body
            .split_once("async fn skip_bad_primitive")
            .expect("skip helper")
            .1;
        assert!(
            helper.contains("println!(\"基本体跳过"),
            "跳过必须同时进控制台与账本: {helper}"
        );

        // 读不到就是读不到，与"读到了但数据是坏的"不是一回事：查询失败仍然硬失败。
        assert_eq!(
            body.matches("anyhow::bail!").count(),
            3,
            "只剩世界变换与属性查询这三处硬失败: {body}"
        );
        assert!(body.contains("has no world transform"), "{body}");
        assert!(body.contains("world transform failed"), "{body}");
        assert!(body.contains("attributes failed"), "{body}");
    }
}
