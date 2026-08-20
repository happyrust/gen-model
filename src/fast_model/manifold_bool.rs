use crate::data_interface::geom_error::{self, BOOL_NEG, BOOL_POS};
use crate::fast_model::manifold_csg::{load_manifold, manifold_to_plant_mesh, subtract_negatives};
use crate::fast_model::{CataNegGroup, GmGeoData, ManiGeoTransQuery, NegInfo};
use aios_core::error::{init_deserialize_error, init_query_error};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{RefnoEnum, get_inst_relate_keys, init_test_surreal};
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use glam::DMat4;
use manifold_csg::Manifold;
use nalgebra::Isometry;
use parry3d::bounding_volume::Aabb;
use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

/// 与生产 `query_valid_insts` 同一口径：切洞网格 id 是 `{refno.latest()}_{sesno}`。
/// OCC / manifold 都必须写这个 id，比较与渲染才读得到切洞结果。
pub(crate) fn design_boolean_mesh_id(refno: &RefnoEnum, sesno: u32) -> String {
    format!("{}_{}", refno.latest(), sesno)
}

fn render_catalogue_manifold_result_write(
    new_id: u64,
    aabb_id: impl Display,
    inst_info_id: impl Display,
    geom_refno: impl Display,
) -> String {
    format!(
        "upsert inst_geo:⟨{new_id}⟩ set meshed = true, aabb = {aabb_id};\
         INSERT RELATION IGNORE INTO geo_relate [{{ id: geo_relate:[{inst_info_id}, inst_geo:⟨{new_id}⟩], \
         in: {inst_info_id}, out: inst_geo:⟨{new_id}⟩, geom_refno: pe:⟨{geom_refno}⟩, \
         geo_type: 'Pos', trans: trans:⟨0⟩, visible: true }}];\
         update {inst_info_id}<-inst_relate set booled=true;"
    )
}

/// 处理元件库有负实体的布尔运算
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的布尔运算结果
/// * `dir` - 模型文件目录路径
pub async fn apply_cata_neg_boolean_manifold(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
    failure_policy: geom_error::GeometryFailurePolicy,
) -> anyhow::Result<()> {
    let inst_keys = get_inst_relate_keys(refnos);

    let mut sql = format!(
        r#" select in as refno, (->inst_info)[0] as inst_info_id, (select value array::flatten([geom_refno, cata_neg])
            from ->inst_info->geo_relate where visible and !out.bad and cata_neg!=none) as boolean_group
            from {inst_keys} where in.id != none and (->inst_info)[0]!=none and has_cata_neg "#
    );

    if !replace_exist {
        sql.push_str("and !bad_bool and !booled");
    }

    // println!("sql is {}", &sql);
    let mut response = crate::data_interface::staging::active_data_db()
        .query(sql)
        .await?
        .check()?;
    let mut params: Vec<CataNegGroup> = response.take(0)?;
    // dbg!(&params);
    if params.is_empty() {
        return Ok(());
    }

    let mut tasks = Vec::new();
    let chunk = (params.len() / 16).max(1);
    // let chunk = params.len();
    // dbg!(&params);
    for chunk in params.chunks(chunk) {
        let group = chunk.to_vec();
        let dir_clone = dir.clone();
        let task = crate::data_interface::staging::write_context::spawn_with_staged_io(
            async move {
                for g in group {
                    let pes = g
                        .boolean_group
                        .iter()
                        .flatten()
                        .map(|x| x.to_pe_key())
                        .collect::<Vec<_>>()
                        .join(",");
                    // dbg!(g.refno);
                    let sql = format!(
                        r#"
                    select record::id(out) as id, geom_refno, trans.d as trans, out.param as param, out.aabb as aabb_id
                    from {}->inst_relate->inst_info->geo_relate
                    where !out.bad and geom_refno in [{}]  and out.aabb!=none and out.param!=none"#,
                        g.refno.to_pe_key(),
                        pes
                    );
                    // println!("geom sql is {}", &sql);
                    let mut resp = crate::data_interface::staging::active_data_db()
                        .query(&sql)
                        .await?
                        .check()?;
                    let gms = resp.take::<Vec<GmGeoData>>(0).map_err(|error| {
                        anyhow!("decode catalogue manifold inputs failed: {error}")
                    })?;
                    // dbg!(&gms);

                    let mut update_sql = String::new();
                    'group: for bg in g.boolean_group {
                        let Some(pos) = gms.iter().find(|x| x.geom_refno == bg[0]) else {
                            if failure_policy == geom_error::GeometryFailurePolicy::Required {
                                return Err(anyhow!(
                                    "required catalogue positive geometry missing for {} geom={}",
                                    g.refno,
                                    bg[0]
                                ));
                            }
                            update_sql.push_str(&format!(
                                "update {}<-inst_relate set bad_bool=true;",
                                &g.inst_info_id,
                            ));
                            continue;
                        };

                        #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                        println!("正在负实体计算的mesh hash: {}", &pos.id);

                        // 网格进不了 manifold（不闭合 / 退化 / 自交）是这块几何自身的
                        // 确定性毛病，重试多少次都是同一句 NotManifold。抛出去会让整个
                        // 生成根连撞 MAX_ATTEMPTS 判死，同批其它元素跟着没模型、模型
                        // 阶段永远不就绪，所以这里只跳过这一件、标 bad_bool 并出声。
                        let pos_manifold = match load_manifold(
                            &dir_clone,
                            &pos.id,
                            pos.trans.compute_matrix().as_dmat4(),
                            false,
                        ) {
                            Ok(manifold) => manifold,
                            Err(error) => {
                                println!(
                                    "目录布尔运算跳过: 正实体载入失败（{error:#}），refno: {} geom: {}",
                                    &g.refno, bg[0]
                                );
                                geom_error::note_skip(
                                    BOOL_POS,
                                    &g.refno.to_pdms_str(),
                                    &pos.id,
                                    &format!("{error:#}"),
                                )
                                .await;
                                if failure_policy == geom_error::GeometryFailurePolicy::Required {
                                    return Err(anyhow!(
                                        "required catalogue positive manifold failed for {} geom={}: {error:#}",
                                        g.refno,
                                        pos.id
                                    ));
                                }
                                update_sql.push_str(&format!(
                                    "update {}<-inst_relate set bad_bool=true;",
                                    &g.inst_info_id,
                                ));
                                continue;
                            }
                        };

                        // dbg!(&update_sql);
                        let mut neg_manifolds = vec![];
                        //负实体的精度要比正实体大
                        for &neg in bg.iter().skip(1) {
                            let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                                if failure_policy == geom_error::GeometryFailurePolicy::Required {
                                    return Err(anyhow!(
                                        "required catalogue negative geometry missing for {} geom={neg}",
                                        g.refno
                                    ));
                                }
                                continue;
                            };
                            let m = neg_geo.trans.compute_matrix().as_dmat4();
                            match load_manifold(&dir_clone, &neg_geo.id, m, true) {
                                Ok(manifold) => neg_manifolds.push(manifold),
                                // 少减一个负实体就是悄悄发一件少切了洞的几何，比整件
                                // 不切更难发现——所以坏一个负实体就整件跳过。
                                Err(error) => {
                                    println!(
                                        "目录布尔运算跳过: 负实体载入失败（{error:#}），refno: {} geom: {neg}",
                                        &g.refno
                                    );
                                    geom_error::note_skip(
                                        BOOL_NEG,
                                        &g.refno.to_pdms_str(),
                                        &neg_geo.id,
                                        &format!("{error:#}"),
                                    )
                                    .await;
                                    if failure_policy == geom_error::GeometryFailurePolicy::Required
                                    {
                                        return Err(anyhow!(
                                            "required catalogue negative manifold failed for {} geom={}: {error:#}",
                                            g.refno,
                                            neg_geo.id
                                        ));
                                    }
                                    update_sql.push_str(&format!(
                                        "update {}<-inst_relate set bad_bool=true;",
                                        &g.inst_info_id,
                                    ));
                                    continue 'group;
                                }
                            }
                        }
                        //没有负实体也要加上为_b后缀，表示已经进行过分析计算了。
                        // if !neg_manifolds.is_empty()
                        {
                            let new_id = g.refno.hash_with_another_refno(bg[0]);
                            let final_manifold = subtract_negatives(pos_manifold, &neg_manifolds);
                            let mesh = manifold_to_plant_mesh(&final_manifold);
                            // 与设计路径同一条 T025 门：差集为空不落盘、不写 inst_geo，
                            // 标记 bad_bool 出声，禁止空网格顶掉可见几何。
                            if mesh.indices.len() < 3 || mesh.vertices.len() < 3 {
                                println!(
                                    "目录布尔运算失败: 差集为空（verts={} idx={}），refno: {} geom: {}",
                                    mesh.vertices.len(),
                                    mesh.indices.len(),
                                    &g.refno,
                                    bg[0]
                                );
                                if failure_policy == geom_error::GeometryFailurePolicy::Required {
                                    return Err(anyhow!(
                                        "required catalogue boolean difference is empty for {} geom={}",
                                        g.refno,
                                        bg[0]
                                    ));
                                }
                                update_sql.push_str(&format!(
                                    "update {}<-inst_relate set bad_bool=true;",
                                    &g.inst_info_id,
                                ));
                                continue;
                            }
                            #[cfg(feature = "debug_model")]
                            mesh.export_obj(false, &format!("{}.obj", g.refno));
                            //保存到文件到dir下
                            mesh.ser_to_file(&dir_clone.join(format!("{}.mesh", new_id)))
                                .map_err(|error| {
                                    anyhow!("save catalogue boolean mesh {new_id} failed: {error}")
                                })?;
                            {
                                // `new_id` is deterministic. A previous attempt may have committed
                                // this statement before a later statement in the batch failed, so a
                                // durable pending retry must update the same row instead of failing
                                // forever with "record already exists".
                                update_sql.push_str(&render_catalogue_manifold_result_write(
                                    new_id,
                                    &pos.aabb_id,
                                    &g.inst_info_id,
                                    format!("{}_b", bg[0]),
                                ));
                                // 做成了就把这一件的降级记录销掉：几何换过、目录修过
                                // 之后清单还挂着旧行，比没有清单更误导人。
                                geom_error::note_success(&g.refno.to_pdms_str()).await;
                                // dbg!(&update_sql);
                            }
                        }
                    }
                    if !update_sql.is_empty() {
                        crate::surreal_retry::execute_model_write(
                            &update_sql,
                            "persist catalogue manifold result",
                        )
                        .await?;
                    }
                }
                Ok::<(), anyhow::Error>(())
            },
        );
        tasks.push(task);
    }
    // dbg!(tasks.len());
    let task_results = futures::future::join_all(tasks).await;
    for result in task_results {
        let result =
            result.map_err(|error| anyhow!("catalogue manifold worker join failed: {error}"))?;
        result?;
    }
    #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
    println!("元件库的负实体计算{:?}完成", refnos);
    Ok(())
}

/// 对多个实例进行布尔运算
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的布尔运算结果
/// * `dir` - 模型文件目录路径
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示布尔运算是否成功
pub async fn apply_insts_boolean_manifold(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
    failure_policy: geom_error::GeometryFailurePolicy,
) -> anyhow::Result<()> {
    for refno in refnos {
        apply_insts_boolean_manifold_single(*refno, replace_exist, dir.clone(), failure_policy)
            .await?;
    }
    Ok(())
}

/// 对实例进行布尔运算
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的布尔运算结果
/// * `dir` - 模型文件目录路径
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示布尔运算是否成功
pub async fn apply_insts_boolean_manifold_single(
    refno: RefnoEnum,
    replace_exist: bool,
    dir: PathBuf,
    failure_policy: geom_error::GeometryFailurePolicy,
) -> anyhow::Result<()> {
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    //排除不在这个范围内的ngrm geom refno
    //这里需要截断传进来的参考号数量
    let mut sql = format!(
        r#"
        select
                in as refno,
                in.sesno as sesno,
                in.noun as noun,
                world_trans.d as wt,
                aabb.d as aabb,
                (select value [record::id(out), trans.d] from out->geo_relate where geo_type in ["Compound", "Pos"] and trans.d != NONE ) as ts,
                (select value [in, world_trans.d,
                    (select record::id(out) as id, geo_type, trans.d as trans, out.aabb.d as aabb
                    from array::flatten(out->geo_relate) where trans.d != NONE and ( geo_type=="Neg" or (geo_type=="CataCrossNeg"
                        and geom_refno in (select value ngmr from pe:{refno}<-ngmr_relate) ) ))]
                        from array::flatten([array::flatten(in<-neg_relate.in->inst_relate), array::flatten(in<-ngmr_relate.in->inst_relate)]) where world_trans.d!=none
                ) as neg_ts
             from inst_relate:{refno} where in.id != none and !bad_bool and ((in<-neg_relate)[0] != none or in<-ngmr_relate[0] != none) and aabb.d != NONE
        "#
    );
    // if !replace_exist {
    //     sql.push_str(" and booled_id != none");
    // }
    match crate::data_interface::staging::active_data_db()
        .query(&sql)
        .await
    {
        Ok(response) => {
            let mut response = response.check()?;
            match response.take::<Vec<ManiGeoTransQuery>>(0) {
                Ok(boolean_query) => {
                    // dbg!(&boolean_query);
                    let chunk = (boolean_query.len() / 16).max(1);
                    //排除有NREV的情况，因为NREV的布尔计算不是很准，还要判断这个NREV的包围盒和实体的包围盒是否差不多大
                    for chunk in boolean_query.chunks(chunk) {
                        let group = chunk.to_vec();
                        let dir_clone = dir.clone();
                        {
                            let mut update_sql = String::new();
                            'element: for mut b in group {
                                let inst_relate_id = b.refno.to_table_key("inst_relate");
                                let mut pos_manifolds = vec![];
                                for (pos_id, pos_t) in b.ts.iter() {
                                    #[cfg(any(
                                        feature = "debug_model",
                                        feature = "debug_model_no_obj"
                                    ))]
                                    println!("正在负实体计算的mesh hash: {}", &pos_id);
                                    // 与目录路径同一条门：载不进 manifold 的网格是确定性
                                    // 坏件，抛出去只会把整个生成根拖成死信。
                                    match load_manifold(
                                        &dir_clone,
                                        pos_id,
                                        pos_t.compute_matrix().as_dmat4(),
                                        false,
                                    ) {
                                        Ok(manifold) => pos_manifolds.push(manifold),
                                        Err(error) => {
                                            println!(
                                                "布尔运算跳过: 正实体载入失败（{error:#}），refno: {} geom: {pos_id}",
                                                &b.refno
                                            );
                                            geom_error::note_skip(
                                                BOOL_POS,
                                                &b.refno.to_pdms_str(),
                                                pos_id,
                                                &format!("{error:#}"),
                                            )
                                            .await;
                                            if failure_policy
                                                == geom_error::GeometryFailurePolicy::Required
                                            {
                                                return Err(anyhow!(
                                                    "required design positive manifold failed for {} geom={pos_id}: {error:#}",
                                                    b.refno
                                                ));
                                            }
                                            update_sql.push_str(&format!(
                                                "update {} set bad_bool=true;",
                                                &inst_relate_id
                                            ));
                                            continue 'element;
                                        }
                                    }
                                }
                                //没有实体的情况，下次就不要再继续计算布尔运算了
                                if pos_manifolds.is_empty() {
                                    println!(
                                        "布尔运算失败: 没有找到正实体 manifold, refno: {}",
                                        &b.refno
                                    );
                                    if failure_policy == geom_error::GeometryFailurePolicy::Required
                                    {
                                        return Err(anyhow!(
                                            "required design positive manifold set is empty for {}",
                                            b.refno
                                        ));
                                    }
                                    update_sql.push_str(&format!(
                                        "update {} set bad_bool=true;",
                                        &inst_relate_id
                                    ));
                                    continue;
                                };
                                let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();
                                let pos_manifold = if pos_manifolds.len() == 1 {
                                    pos_manifolds.pop().expect("len==1")
                                } else {
                                    Manifold::batch_union(&pos_manifolds)
                                };
                                if pos_manifold.num_tri() == 0 {
                                    println!(
                                        "布尔运算失败: 正实体 manifold 没有三角形, refno: {}",
                                        &b.refno
                                    );
                                    if failure_policy == geom_error::GeometryFailurePolicy::Required
                                    {
                                        return Err(anyhow!(
                                            "required design positive manifold has no triangles for {}",
                                            b.refno
                                        ));
                                    }
                                    update_sql.push_str(&format!(
                                        "update {} set bad_bool=true;",
                                        &inst_relate_id
                                    ));
                                    continue;
                                };
                                #[cfg(feature = "debug_model")]
                                {
                                    let pos_mesh = manifold_to_plant_mesh(&pos_manifold);
                                    pos_mesh.export_obj(false, "pos_t.obj").unwrap();
                                }

                                let mut neg_manifolds = vec![];
                                for (neg_refno, mut neg_t, negs) in b.neg_ts.into_iter() {
                                    for NegInfo {
                                        id, trans, aabb, ..
                                    } in negs
                                    {
                                        let Some(mut neg_aabb) = aabb else {
                                            continue;
                                        };
                                        let m = inverse_mat
                                            * neg_t.compute_matrix().as_dmat4()
                                            * trans.compute_matrix().as_dmat4();
                                        let manifold = match load_manifold(&dir_clone, &id, m, true)
                                        {
                                            Ok(manifold) => manifold,
                                            // 少减一个负实体等于悄悄发一件少切了洞的
                                            // 几何，比整件不切更难发现——整件跳过。
                                            Err(error) => {
                                                println!(
                                                    "布尔运算跳过: 负实体载入失败（{error:#}），refno: {} 来自: {neg_refno} geom: {id}",
                                                    &b.refno
                                                );
                                                geom_error::note_skip(
                                                    BOOL_NEG,
                                                    &b.refno.to_pdms_str(),
                                                    &id,
                                                    &format!("{error:#}"),
                                                )
                                                .await;
                                                if failure_policy
                                                    == geom_error::GeometryFailurePolicy::Required
                                                {
                                                    return Err(anyhow!(
                                                        "required design negative manifold failed for {} geom={id}: {error:#}",
                                                        b.refno
                                                    ));
                                                }
                                                update_sql.push_str(&format!(
                                                    "update {} set bad_bool=true;",
                                                    &inst_relate_id
                                                ));
                                                continue 'element;
                                            }
                                        };
                                        #[cfg(feature = "debug_model")]
                                        {
                                            let neg_mesh = manifold_to_plant_mesh(&manifold);
                                            neg_mesh
                                                .export_obj(false, &format!("{}_t.obj", neg_refno))
                                                .unwrap();
                                        }
                                        neg_manifolds.push(manifold);
                                    }
                                }
                                if !neg_manifolds.is_empty() {
                                    let final_manifold =
                                        subtract_negatives(pos_manifold, &neg_manifolds);
                                    #[cfg(feature = "debug_model")]
                                    dbg!(final_manifold.num_tri());
                                    let mesh = manifold_to_plant_mesh(&final_manifold);
                                    // T025 / ADR-029 决策 3：空差集是失败不是结果——不写盘、
                                    // 不覆盖 booled_id，标记 bad_bool 出声，禁止渲染端悄悄少件。
                                    if mesh.indices.len() < 3 || mesh.vertices.len() < 3 {
                                        println!(
                                            "布尔运算失败: 差集为空（verts={} idx={}），不覆盖 booled_id, refno: {}",
                                            mesh.vertices.len(),
                                            mesh.indices.len(),
                                            &b.refno
                                        );
                                        if failure_policy
                                            == geom_error::GeometryFailurePolicy::Required
                                        {
                                            return Err(anyhow!(
                                                "required design boolean difference is empty for {}",
                                                b.refno
                                            ));
                                        }
                                        update_sql.push_str(&format!(
                                            "update {} set bad_bool=true;",
                                            &inst_relate_id
                                        ));
                                        continue;
                                    }
                                    #[cfg(feature = "debug_model")]
                                    mesh.export_obj(false, &format!("{}.obj", b.refno));
                                    let mesh_id = design_boolean_mesh_id(&b.refno, b.sesno);
                                    //保存到文件到dir下
                                    mesh.ser_to_file(&dir_clone.join(format!("{}.mesh", mesh_id)))
                                        .map_err(|error| {
                                            anyhow!(
                                                "save design boolean mesh for {} failed: {error}",
                                                b.refno
                                            )
                                        })?;
                                    let mesh_id_literal =
                                        crate::data_interface::dbnum_state::escape_surql_str(
                                            &mesh_id,
                                        );
                                    update_sql.push_str(&format!(
                                        "update {} set booled_id='{}', insts_flat=[{{geo_hash:'{}'}}];",
                                        &inst_relate_id, mesh_id_literal, mesh_id_literal
                                    ));
                                    // 做成了就销账，别让修好的件一直挂在降级清单上。
                                    geom_error::note_success(&b.refno.to_pdms_str()).await;
                                }
                                // dbg!(&update_sql);
                            }
                            if !update_sql.is_empty() {
                                crate::surreal_retry::execute_model_write(
                                    &update_sql,
                                    "persist design manifold result",
                                )
                                .await?;
                            }
                        }
                    }
                }
                Err(e) => {
                    init_deserialize_error(
                        "Vec<ManiGeoTransQuery>",
                        &e,
                        &sql,
                        &std::panic::Location::caller().to_string(),
                    );
                    return Err(anyhow!(e.to_string()));
                }
            }
        }
        Err(e) => {
            init_query_error(&sql, &e, &std::panic::Location::caller().to_string());
            return Err(anyhow!(e.to_string()));
        }
    }
    #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
    println!("design的负实体计算{}完成", refno);
    Ok(())
}

#[test]
fn catalogue_manifold_inst_geo_write_is_idempotent() {
    let source = include_str!("manifold_bool.rs");
    let catalogue = source
        .split_once("pub async fn apply_cata_neg_boolean_manifold(")
        .expect("catalogue manifold function")
        .1
        .split_once("pub async fn apply_insts_boolean_manifold(")
        .expect("catalogue manifold boundary")
        .0;

    assert!(
        catalogue.contains("render_catalogue_manifold_result_write("),
        "{catalogue}"
    );
    assert!(!catalogue.contains("create inst_geo"), "{catalogue}");
    assert!(
        !catalogue.contains("INSERT RELATION INTO geo_relate"),
        "{catalogue}"
    );
}

#[tokio::test]
async fn catalogue_manifold_inst_geo_upsert_replays_and_refreshes_aabb() {
    use surrealdb::engine::any::connect;
    use surrealdb::sql::Thing;

    let db = connect("mem://").await.expect("mem boots");
    db.use_ns("manifold_idempotency")
        .use_db("model")
        .await
        .expect("select test database");

    db.query(
        "CREATE pe:root; CREATE inst_info:test; \
         RELATE pe:root->inst_relate->inst_info:test SET booled = false;",
    )
    .await
    .expect("seed manifold relation endpoints")
    .check()
    .expect("seed manifold relation statements");

    for aabb in ["aabb:⟨first⟩", "aabb:⟨second⟩"] {
        db.query(render_catalogue_manifold_result_write(
            42,
            aabb,
            "inst_info:test",
            "root_b",
        ))
        .await
        .expect("execute catalogue manifold result write")
        .check()
        .expect("catalogue manifold result statements");
    }

    let mut response = db
        .query(
            "SELECT VALUE aabb FROM inst_geo:⟨42⟩; \
             RETURN array::len(SELECT VALUE id FROM geo_relate); \
             SELECT VALUE booled FROM inst_relate;",
        )
        .await
        .expect("read replayed manifold row")
        .check()
        .expect("read replayed manifold statement");
    let aabb: Option<Thing> = response.take(0).expect("decode aabb link");
    assert_eq!(
        aabb.map(|thing| thing.to_string()).as_deref(),
        Some("aabb:second")
    );
    assert_eq!(response.take::<Option<usize>>(1).unwrap(), Some(1));
    assert_eq!(response.take::<Option<bool>>(2).unwrap(), Some(true));
}

#[test]
fn design_boolean_mesh_id_matches_query_valid_insts() {
    let refno: RefnoEnum = "17496_116569".into();
    assert_eq!(design_boolean_mesh_id(&refno, 716), "17496_116569_716");
}

#[test]
fn manifold_io_uses_the_staged_router_and_propagates_worker_failures() {
    crate::data_interface::staging::replay_safe::validate_statement(
        "INSERT RELATION INTO geo_relate [{ id: geo_relate:[inst_info:a, inst_geo:b], \
         in: inst_info:a, out: inst_geo:b, geom_refno: pe:c, geo_type: 'Pos' }];",
    )
    .expect("deterministic manifold relation is ReplaySafe");

    let source = include_str!("manifold_bool.rs");
    let catalogue = source
        .split_once("pub async fn apply_cata_neg_boolean_manifold(")
        .expect("catalogue manifold function")
        .1
        .split_once("pub async fn apply_insts_boolean_manifold(")
        .expect("catalogue manifold boundary")
        .0;
    assert!(catalogue.contains("active_data_db()"), "{catalogue}");
    assert!(catalogue.contains("execute_model_write("), "{catalogue}");
    assert!(catalogue.contains("join_all(tasks).await"), "{catalogue}");
    assert!(!catalogue.contains("try_join_all(tasks)"), "{catalogue}");
    assert!(
        catalogue.contains("for result in task_results"),
        "{catalogue}"
    );
    assert!(!catalogue.contains("SUL_DB"), "{catalogue}");

    let design = source
        .split_once("pub async fn apply_insts_boolean_manifold_single(")
        .expect("design manifold function")
        .1
        .split_once("fn manifold_io_uses_the_staged_router")
        .expect("design manifold boundary")
        .0;
    assert!(design.contains("active_data_db()"), "{design}");
    assert!(design.contains("execute_model_write("), "{design}");
    assert!(!design.contains("SUL_DB"), "{design}");
    assert!(
        design.contains("subtract_negatives"),
        "设计布尔必须走 manifold-csg 适配层"
    );
    assert!(
        !design.contains("ManifoldRust") && !catalogue.contains("ManifoldRust"),
        "不得再调用 aios-core 旧 manifold-sys FFI"
    );
}

/// T025 / ADR-029 决策 3：空差集是失败不是结果。设计与目录两条生产路径都必须
/// 在写盘/写库前拦下空网格并标记 `bad_bool`，不得覆盖已有 `booled_id`。
///
/// 本测试必须放在 `manifold_io_uses_the_staged_router…` 之后：上面几个源码切片
/// 断言按「首次出现」切函数体，本测试体里的字面量不能落进它们的切片范围。
#[test]
fn empty_difference_is_bad_bool_not_a_silent_swallow() {
    let source = include_str!("manifold_bool.rs");
    let design = source
        .split_once("pub async fn apply_insts_boolean_manifold_single(")
        .expect("design manifold function")
        .1
        .split_once("fn manifold_io_uses_the_staged_router")
        .expect("design manifold boundary")
        .0;
    assert!(design.contains("不覆盖 booled_id"), "{design}");
    assert!(
        design.contains("set bad_bool=true")
            && design.contains("set booled_id='")
            && design.contains("insts_flat=[{{geo_hash:'"),
        "{design}"
    );
    assert!(
        !design.contains("found_need_occ") && !design.contains("if !success"),
        "死代码不得回流"
    );

    let catalogue = source
        .split_once("pub async fn apply_cata_neg_boolean_manifold(")
        .expect("catalogue manifold function")
        .1
        .split_once("pub async fn apply_insts_boolean_manifold(")
        .expect("catalogue manifold boundary")
        .0;
    assert!(catalogue.contains("差集为空"), "目录路径同样不得静默吞件");
    for body in [design, catalogue] {
        let empty = body
            .split_once("差集为空")
            .expect("empty-difference gate")
            .1;
        assert!(
            empty.contains("GeometryFailurePolicy::Required"),
            "Required must reject an empty boolean result: {empty}"
        );
    }
}

/// 载不进 manifold 的网格必须服从调用入口显式选择的失败策略。
///
/// 2026-08-19 现场：BEND `24384/23259` 的正实体报 `manifold3d status: NotManifold`，
/// 这一句 `?` 出去之后 BRAN `/C-OR-1R345-C` 的 `regen_root` 连撞 5 次到死信，同一
/// 批里另外 9 个根做完了也没用——`model_ready` 就此永远停在 false。坏几何是确定性
/// 的，窗口外只跳过这一件并记账；暂存窗口则必须把错误上浮，阻断水位。
///
/// 与上面几个切片断言同理，本测试必须放在切片边界之后。
#[test]
fn manifold_ingest_failure_has_required_and_best_effort_paths() {
    let source = include_str!("manifold_bool.rs");
    let catalogue = source
        .split_once("pub async fn apply_cata_neg_boolean_manifold(")
        .expect("catalogue manifold function")
        .1
        .split_once("pub async fn apply_insts_boolean_manifold(")
        .expect("catalogue manifold boundary")
        .0;
    let design = source
        .split_once("pub async fn apply_insts_boolean_manifold_single(")
        .expect("design manifold function")
        .1
        .split_once("fn manifold_io_uses_the_staged_router")
        .expect("design manifold boundary")
        .0;

    for body in [catalogue, design] {
        assert!(body.contains("正实体载入失败"), "{body}");
        assert!(body.contains("负实体载入失败"), "{body}");
        assert!(body.contains("set bad_bool=true"), "{body}");
        assert!(
            body.contains("GeometryFailurePolicy::Required"),
            "暂存必需策略必须显式上浮: {body}"
        );
        assert!(
            body.contains("required catalogue") || body.contains("required design"),
            "必需策略错误要保留路径语义: {body}"
        );
        // 控制台那句会滚走。跳过必须同时落一行可查的账，否则「这个件的洞没切」
        // 事后没有任何查询能说得出来。
        assert!(body.contains("geom_error::note_skip("), "{body}");
        assert!(
            body.contains("BOOL_POS") && body.contains("BOOL_NEG"),
            "正负两侧都要归类，否则清单答不出栽在哪一边: {body}"
        );
        assert!(body.contains("geom_error::note_success("), "{body}");
    }
}

#[test]
fn test_json_refno_parse_error() {
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub(crate) struct TestJson {
        pub refno: RefnoEnum,
        // pub noun: String,
        // pub wt: Transform,
        // pub aabb: Aabb,
        // pub ts: Vec<(String, Transform)>,
        pub neg_ts: Vec<(Transform, Vec<NegInfo>)>,
    }

    let test_json = r#"
        {
            "refno": { "tb": "pe", "id": { "String": "17496_172792" } },
             "neg_ts": [
      [
        [
          {
            "rotation": [0.47776905, 0.5212838, 0.5212838, 0.47776905],
            "scale": [1.0, 1.0, 1.0],
            "translation": [-4751.5884, 10621.164, 4649.75]
          }
        ,

          {
            "aabb": {
              "maxs": [0.5, 0.49786708, 1.0],
              "mins": [-0.5, -0.49786708, 0.0]
            },
            "geo_type": "Neg",
            "id": "2",
            "trans": {
              "rotation": [0.0, 0.0, 0.0, 1.0],
              "scale": [17.0, 17.0, 16.0],
              "translation": [0.0, 0.0, -8.0]
            }
          }
        ]
      ],
      [
        [
          {
            "rotation": [0.47776905, 0.5212838, 0.5212838, 0.47776905],
            "scale": [1.0, 1.0, 1.0],
            "translation": [-4736.3726, 10446.827, 4649.75]
          }
        ,

          {
            "aabb": {
              "maxs": [0.5, 0.49786708, 1.0],
              "mins": [-0.5, -0.49786708, 0.0]
            },
            "geo_type": "Neg",
            "id": "2",
            "trans": {
              "rotation": [0.0, 0.0, 0.0, 1.0],
              "scale": [17.0, 17.0, 16.0],
              "translation": [0.0, 0.0, -8.0]
            }
          }
        ]
      ]
    ]
        }
    "#;

    let result = serde_json::from_str::<TestJson>(test_json);
    dbg!(result);

    // let refno:RefnoEnum = "17496_172792".into();
    // let path: PathBuf = "assets/meshes".into();
    // apply_insts_boolean_manifold_single(refno, false, path).await.unwrap();
}

#[tokio::test]
#[ignore = "manual integration: requires the configured Surreal project and mesh files"]
async fn test_boolean_refno_parse_error() {
    init_test_surreal().await;

    let refno: RefnoEnum = "17496_172792".into();
    let path: PathBuf = "assets/meshes".into();
    apply_insts_boolean_manifold_single(
        refno,
        false,
        path,
        geom_error::GeometryFailurePolicy::BestEffortFallback,
    )
    .await
    .unwrap();
}
