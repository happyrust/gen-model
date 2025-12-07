use std::collections::{HashMap, hash_map::Entry};

use aios_core::geometry::ShapeInstancesData;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::rs_surreal::delete_inst_relate_cascade;
use aios_core::types::*;
use aios_core::{SUL_DB, SurrealQueryExt, get_db_option};
use bevy_transform::prelude::Transform;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use itertools::Itertools;
use rkyv::vec;
use tokio::task::JoinHandle;

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::debug_model_debug;
// use crate::fast_model::EXIST_MESH_GEOS;
use chrono;
use std::fs::OpenOptions;
use std::io::Write;

/// 保存 instance 数据到数据库（事务化批处理版本）
pub async fn save_instance_data_optimize(
    inst_mgr: &ShapeInstancesData,
    replace_exist: bool,
) -> anyhow::Result<()> {
    debug_model_debug!(
        "save_instance_data_optimize start: inst_info={}, inst_geo_keys={}, tubi_keys={}, replace_exist={}",
        inst_mgr.inst_info_map.len(),
        inst_mgr.inst_geos_map.len(),
        inst_mgr.inst_tubi_map.len(),
        replace_exist
    );
    const CHUNK_SIZE: usize = 300;
    const MAX_TX_STATEMENTS: usize = 4;
    const MAX_CONCURRENT_TX: usize = 6;

    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    if let Entry::Vacant(entry) = transform_map.entry(0) {
        entry.insert(serde_json::to_string(&Transform::IDENTITY)?);
    }
    let mut vec3_map: HashMap<u64, String> = HashMap::new();

    // 收集 Neg 和 CataCrossNeg 类型的 geo_relate 映射
    // neg_geo_by_carrier: key=carrier_refno -> value=Vec<geo_relate_id>
    //   用于 neg_relate: 通过负实体 refno 找到其所有 Neg 类型的 geo_relate
    // cata_cross_neg_geo_map: key=(carrier_refno, geom_refno) -> value=Vec<geo_relate_id>
    //   用于 ngmr_relate: 通过 (负载体, ngmr_geom_refno) 找到对应的 CataCrossNeg geo_relate
    let mut neg_geo_by_carrier: HashMap<RefnoEnum, Vec<u64>> = HashMap::new();
    let mut cata_cross_neg_geo_map: HashMap<(RefnoEnum, RefnoEnum), Vec<u64>> = HashMap::new();
    if replace_exist {
        let refnos: Vec<RefnoEnum> = inst_mgr.inst_info_map.keys().copied().collect();
        debug_model_debug!(
            "save_instance_data_optimize deleting existing inst_relate for {} refnos",
            refnos.len()
        );
        delete_inst_relate_cascade(&refnos, CHUNK_SIZE).await?;
    }

    // inst_geo & geo_relate
    let mut geo_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
    let mut inst_geo_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);
    let mut geo_relate_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

    for inst_geo_data in inst_mgr.inst_geos_map.values() {
        for inst in &inst_geo_data.insts {
            // #region agent log
            if let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open("/Volumes/DPC/work/plant-code/rs-plant3-d/.cursor/debug.log")
            {
                let (pdia, phei, btm0, btm1, top0, top1) = match &inst.geo_param {
                    PdmsGeoParam::PrimSCylinder(s) => (
                        s.pdia,
                        s.phei,
                        s.btm_shear_angles[0],
                        s.btm_shear_angles[1],
                        s.top_shear_angles[0],
                        s.top_shear_angles[1],
                    ),
                    _ => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
                };
                let geo_type = match &inst.geo_param {
                    PdmsGeoParam::PrimBox(_) => "PrimBox",
                    PdmsGeoParam::PrimLSnout(_) => "PrimLSnout",
                    PdmsGeoParam::PrimDish(_) => "PrimDish",
                    PdmsGeoParam::PrimSphere(_) => "PrimSphere",
                    PdmsGeoParam::PrimCTorus(_) => "PrimCTorus",
                    PdmsGeoParam::PrimRTorus(_) => "PrimRTorus",
                    PdmsGeoParam::PrimPyramid(_) => "PrimPyramid",
                    PdmsGeoParam::PrimLPyramid(_) => "PrimLPyramid",
                    PdmsGeoParam::PrimSCylinder(_) => "PrimSCylinder",
                    PdmsGeoParam::PrimLCylinder(_) => "PrimLCylinder",
                    PdmsGeoParam::PrimRevolution(_) => "PrimRevolution",
                    PdmsGeoParam::PrimExtrusion(_) => "PrimExtrusion",
                    PdmsGeoParam::PrimPolyhedron(_) => "PrimPolyhedron",
                    PdmsGeoParam::PrimLoft(_) => "PrimLoft",
                    PdmsGeoParam::CompoundShape => "CompoundShape",
                    PdmsGeoParam::Unknown => "Unknown",
                };
                let _ = writeln!(
                    f,
                    r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H4","location":"pdms_inst.rs:save_instance_data_optimize","message":"inst_geo_buffer push","data":{{"geo_hash":{},"refno":"{}","geo_type":"{}","unit_flag":{},"pdia":{},"phei":{},"btm":[{},{}],"top":[{},{}]}},"timestamp":{}}}"#,
                    inst.geo_hash,
                    inst.refno.to_string(),
                    geo_type,
                    inst.unit_flag,
                    pdia,
                    phei,
                    btm0,
                    btm1,
                    top0,
                    top1,
                    chrono::Utc::now().timestamp_millis()
                );
            }
            // #endregion
            if inst.transform.translation.is_nan()
                || inst.transform.rotation.is_nan()
                || inst.transform.scale.is_nan()
            {
                debug_model_debug!(
                    "[WARN] skip inst geo due to NaN transform: refno={:?}, geo_hash={}",
                    inst.refno,
                    inst.geo_hash
                );
                continue;
            }

            let transform_hash = gen_bytes_hash(&inst.transform);
            if let Entry::Vacant(entry) = transform_map.entry(transform_hash) {
                entry.insert(serde_json::to_string(&inst.transform)?);
            }

            let key_pts = inst.geo_param.key_points();
            let mut pt_hashes = Vec::with_capacity(key_pts.len());
            for key_pt in key_pts {
                let pts_hash = key_pt.gen_hash();
                pt_hashes.push(format!("vec3:⟨{}⟩", pts_hash));
                if let Entry::Vacant(entry) = vec3_map.entry(pts_hash) {
                    entry.insert(serde_json::to_string(&key_pt)?);
                }
            }

            let cat_negs_str = if !inst.cata_neg_refnos.is_empty() {
                format!(
                    ", cata_neg: [{}]",
                    inst.cata_neg_refnos.iter().map(|x| x.to_pe_key()).join(",")
                )
            } else {
                String::new()
            };

            let relate_json = format!(
                r#"in: inst_info:⟨{0}⟩, out: inst_geo:⟨{1}⟩, trans: trans:⟨{2}⟩, geom_refno: pe:{3}, pts: [{4}], geo_type: '{5}', visible: {6} {7}"#,
                inst_geo_data.id(),
                inst.geo_hash,
                transform_hash,
                inst.refno,
                pt_hashes.join(","),
                inst.geo_type.to_string(),
                inst.visible,
                cat_negs_str
            );
            let relate_id = gen_bytes_hash(&relate_json);
            geo_relate_buffer.push(format!("{{ {relate_json}, id: '{relate_id}' }}"));

            // 收集 Neg 和 CataCrossNeg 类型的 geo_relate 映射
            // carrier_refno: 拥有这个 geo_relate 的实体
            // geom_refno: inst.refno (geo_relate 中的 geom_refno 字段)
            use aios_core::geometry::GeoBasicType;
            let carrier_refno = inst_geo_data.refno;
            let geom_refno = inst.refno;
            match inst.geo_type {
                GeoBasicType::Neg => {
                    // neg_relate: 按 carrier_refno 收集所有 Neg geo_relate
                    neg_geo_by_carrier
                        .entry(carrier_refno)
                        .or_insert_with(Vec::new)
                        .push(relate_id);
                }
                GeoBasicType::CataCrossNeg => {
                    // ngmr_relate: 按 (carrier_refno, geom_refno) 收集 CataCrossNeg geo_relate
                    cata_cross_neg_geo_map
                        .entry((carrier_refno, geom_refno))
                        .or_insert_with(Vec::new)
                        .push(relate_id);
                }
                _ => {}
            }

            // 直接使用 EleInstGeo，它已经包含了正确的 unit_flag
            // #region agent log
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/Volumes/DPC/work/plant-code/rs-plant3-d/.cursor/debug.log")
            {
                // 尝试提取圆柱/斜切圆柱的关键参数；其他类型留零
                let (pdia, phei, btm0, btm1, top0, top1, unit_flag, is_sscl) = match &inst.geo_param
                {
                    aios_core::parsed_data::geo_params_data::PdmsGeoParam::PrimSCylinder(s) => (
                        s.pdia,
                        s.phei,
                        s.btm_shear_angles[0],
                        s.btm_shear_angles[1],
                        s.top_shear_angles[0],
                        s.top_shear_angles[1],
                        s.unit_flag,
                        s.is_sscl(),
                    ),
                    _ => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, inst.unit_flag, false),
                };
                let _ = writeln!(
                    f,
                    r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H3","location":"pdms_inst.rs:save_instance_data_optimize","message":"push inst_geo","data":{{"geo_hash":{},"refno":"{}","geo_type":"{}","pdia":{},"phei":{},"btm":[{},{}],"top":[{},{}],"unit_flag":{},"is_sscl":{},"inst_geo_buffer_len":{}}},"timestamp":{}}}"#,
                    inst.geo_hash,
                    inst.refno.to_string(),
                    inst.geo_type.to_string(),
                    pdia,
                    phei,
                    btm0,
                    btm1,
                    top0,
                    top1,
                    unit_flag,
                    is_sscl,
                    inst_geo_buffer.len() + 1,
                    chrono::Utc::now().timestamp_millis()
                );
            }
            // #endregion
            inst_geo_buffer.push(inst.gen_unit_geo_sur_json());

            if inst_geo_buffer.len() >= CHUNK_SIZE {
                let statement = format!(
                    "INSERT IGNORE INTO {} [{}];",
                    stringify!(inst_geo),
                    inst_geo_buffer.join(",")
                );
                geo_batcher.push(statement).await?;
                inst_geo_buffer.clear();
            }

            if geo_relate_buffer.len() >= CHUNK_SIZE {
                let statement = format!(
                    "INSERT RELATION INTO geo_relate [{}];",
                    geo_relate_buffer.join(",")
                );
                geo_batcher.push(statement).await?;
                geo_relate_buffer.clear();
            }
        }
    }

    if !inst_geo_buffer.is_empty() {
        let statement = format!(
            "INSERT IGNORE INTO {} [{}];",
            stringify!(inst_geo),
            inst_geo_buffer.join(",")
        );
        geo_batcher.push(statement).await?;
        debug_model_debug!(
            "save_instance_data_optimize flushing remaining inst_geo records: {}",
            inst_geo_buffer.len()
        );
    }

    if !geo_relate_buffer.is_empty() {
        let statement = format!(
            "INSERT RELATION INTO geo_relate [{}];",
            geo_relate_buffer.join(",")
        );
        geo_batcher.push(statement).await?;
        debug_model_debug!(
            "save_instance_data_optimize flushing remaining geo_relate records: {}",
            geo_relate_buffer.len()
        );
    }

    geo_batcher.finish().await?;

    // tubi -> aabb & transform maps
    for tubi in inst_mgr.inst_tubi_map.values() {
        if let Some(aabb) = tubi.aabb {
            let aabb_hash = gen_bytes_hash(&aabb);
            if let Entry::Vacant(entry) = aabb_map.entry(aabb_hash) {
                entry.insert(serde_json::to_string(&aabb)?);
            }
        }

        let transform_hash = gen_bytes_hash(&tubi.world_transform);
        if let Entry::Vacant(entry) = transform_map.entry(transform_hash) {
            entry.insert(serde_json::to_string(&tubi.world_transform)?);
        }
    }

    // neg_relate - 新结构
    // 关系方向：切割几何 -[neg_relate]-> 正实体
    // - in: geo_relate ID (切割几何)
    // - out: 正实体 refno (被减实体)
    // - pe: 负实体 refno (负载体，原来的 in)
    // 查询时：SELECT in.* FROM pe:正实体<-neg_relate 直接获取切割几何
    if !inst_mgr.neg_relate_map.is_empty() {
        println!("🔍 [DEBUG] 开始创建 neg_relate 关系 (新结构: in=geo_relate):");
        for (target, refnos) in &inst_mgr.neg_relate_map {
            println!("  目标: {}, 负实体数量: {}", target, refnos.len());
        }

        let mut neg_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut neg_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (target, neg_refnos) in &inst_mgr.neg_relate_map {
            for neg_refno in neg_refnos.iter() {
                // 查找该负实体的所有 Neg geo_relate
                if let Some(geo_relate_ids) = neg_geo_by_carrier.get(neg_refno) {
                    for geo_relate_id in geo_relate_ids.iter() {
                        // ID 简化：[geo_relate_id, target_pe] 唯一确定一条关系
                        neg_buffer.push(format!(
                            "{{ in: geo_relate:⟨{0}⟩, id: ['{0}', {2}], out: {2}, pe: {1} }}",
                            geo_relate_id,         // 切割几何
                            neg_refno.to_pe_key(), // 负载体
                            target.to_pe_key(),    // 正实体（被减实体）
                        ));

                        if neg_buffer.len() >= CHUNK_SIZE {
                            let statement = format!(
                                "INSERT RELATION INTO neg_relate [{}];",
                                neg_buffer.join(",")
                            );
                            neg_batcher.push(statement).await?;
                            neg_buffer.clear();
                        }
                    }
                } else {
                    // 没有找到 geo_relate，记录警告但不创建关系
                    debug_model_debug!(
                        "[WARN] neg_relate: 负实体 {} 没有找到 Neg 类型的 geo_relate",
                        neg_refno
                    );
                }
            }
        }

        if !neg_buffer.is_empty() {
            let statement = format!(
                "INSERT RELATION INTO neg_relate [{}];",
                neg_buffer.join(",")
            );
            neg_batcher.push(statement).await?;
        }

        neg_batcher.finish().await?;
    }

    // ngmr_relate - 新结构
    // 关系方向：切割几何 -[ngmr_relate]-> 正实体
    // - in: geo_relate ID (CataCrossNeg 切割几何)
    // - out: 目标k (正实体)
    // - pe: ele_refno (负载体，原来的 in)
    // - ngmr: ngmr_geom_refno (NGMR 几何引用，保留用于调试)
    // 查询时：SELECT in.* FROM pe:正实体<-ngmr_relate 直接获取切割几何
    if !inst_mgr.ngmr_neg_relate_map.is_empty() {
        println!("🔍 [DEBUG] 开始创建 ngmr_relate 关系 (新结构: in=geo_relate):");
        for (k, refnos) in &inst_mgr.ngmr_neg_relate_map {
            println!("  目标: {}, NGMR 数量: {}", k, refnos.len());
        }

        let mut ngmr_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut ngmr_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (target_k, refnos) in &inst_mgr.ngmr_neg_relate_map {
            let target_pe = target_k.to_pe_key();
            for (ele_refno, ngmr_geom_refno) in refnos {
                // 查找该 (负载体, ngmr_geom_refno) 的 CataCrossNeg geo_relate
                let key = (*ele_refno, *ngmr_geom_refno);
                if let Some(geo_relate_ids) = cata_cross_neg_geo_map.get(&key) {
                    for geo_relate_id in geo_relate_ids.iter() {
                        let ele_pe = ele_refno.to_pe_key();
                        let ngmr_pe = ngmr_geom_refno.to_pe_key();
                        // ID 简化：[geo_relate_id, target_pe] 唯一确定一条关系
                        ngmr_buffer.push(format!(
                            "{{ in: geo_relate:⟨{0}⟩, id: ['{0}', {2}], out: {2}, pe: {1}, ngmr: {3} }}",
                            geo_relate_id,  // 切割几何
                            ele_pe,         // 负载体
                            target_pe,      // 正实体（目标）
                            ngmr_pe         // NGMR 几何引用
                        ));

                        if ngmr_buffer.len() >= CHUNK_SIZE {
                            let statement = format!(
                                "INSERT RELATION INTO ngmr_relate [{}];",
                                ngmr_buffer.join(",")
                            );
                            ngmr_batcher.push(statement).await?;
                            ngmr_buffer.clear();
                        }
                    }
                } else {
                    // 没有找到 geo_relate，记录警告但不创建关系
                    debug_model_debug!(
                        "[WARN] ngmr_relate: 负载体 {} + ngmr {} 没有找到 CataCrossNeg 类型的 geo_relate",
                        ele_refno,
                        ngmr_geom_refno
                    );
                }
            }
        }

        if !ngmr_buffer.is_empty() {
            let statement = format!(
                "INSERT RELATION INTO ngmr_relate [{}];",
                ngmr_buffer.join(",")
            );
            ngmr_batcher.push(statement).await?;
        }

        ngmr_batcher.finish().await?;
    }

    // inst_info & inst_relate
    let mut inst_keys: Vec<RefnoEnum> = Vec::with_capacity(inst_mgr.inst_info_map.len());
    debug_model_debug!(
        "🔍 [DEBUG] inst_info_map keys: {:?}",
        inst_mgr.inst_info_map.keys().collect::<Vec<&RefnoEnum>>()
    );
    let mut inst_info_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
    let mut inst_info_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);
    let mut inst_relate_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
    let mut inst_relate_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

    for (key, info) in &inst_mgr.inst_info_map {
        inst_keys.push(*key);

        if info.world_transform.translation.is_nan()
            || info.world_transform.rotation.is_nan()
            || info.world_transform.scale.is_nan()
        {
            continue;
        }

        // 使用压缩格式存储 ptset（减少约 70-80% 存储空间）
        inst_info_buffer.push(info.gen_sur_json_compact(false));
        if inst_info_buffer.len() >= CHUNK_SIZE {
            let statement = format!(
                "INSERT IGNORE INTO {} [{}];",
                stringify!(inst_info),
                inst_info_buffer.join(",")
            );
            inst_info_batcher.push(statement).await?;
            inst_info_buffer.clear();
        }

        let transform_hash = gen_bytes_hash(&info.world_transform);
        if let Entry::Vacant(entry) = transform_map.entry(transform_hash) {
            entry.insert(serde_json::to_string(&info.world_transform)?);
        }

        let relate_sql = format!(
            "{{id: {0}, in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, generic: '{4}', zone_refno: fn::find_ancestor_type({1}, 'ZONE'), dt: fn::ses_date({1}), has_cata_neg: {5}, solid: {6}, owner_refno: {7}, owner_type: '{8}'}}",
            key.to_inst_relate_key(),
            key.to_pe_key(),
            info.id_str(),
            transform_hash,
            info.generic_type.to_string(),
            info.has_cata_neg,
            info.is_solid,
            info.owner_refno.to_pe_key(),
            info.owner_type,
        );

        inst_relate_buffer.push(relate_sql);
        if inst_relate_buffer.len() >= CHUNK_SIZE {
            let statement = format!(
                "INSERT RELATION INTO inst_relate [{}];",
                inst_relate_buffer.join(",")
            );
            inst_relate_batcher.push(statement).await?;
            inst_relate_buffer.clear();
        }
    }

    if !inst_relate_buffer.is_empty() {
        let statement = format!(
            "INSERT RELATION INTO inst_relate [{}];",
            inst_relate_buffer.join(",")
        );
        inst_relate_batcher.push(statement).await?;
        debug_model_debug!(
            "save_instance_data_optimize flushing inst_relate from inst_info_map: {}",
            inst_relate_buffer.len()
        );
    }

    // 为 inst_tubi_map 也创建 inst_relate 记录（BRAN/HANG Tubing 几何体）
    if !inst_mgr.inst_tubi_map.is_empty() {
        debug_model_debug!(
            "save_instance_data_optimize processing inst_tubi_map: {} Tubing records",
            inst_mgr.inst_tubi_map.len()
        );

        let mut tubi_relate_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (key, info) in &inst_mgr.inst_tubi_map {
            inst_keys.push(*key);

            if info.world_transform.translation.is_nan()
                || info.world_transform.rotation.is_nan()
                || info.world_transform.scale.is_nan()
            {
                continue;
            }

            let transform_hash = gen_bytes_hash(&info.world_transform);
            if let Entry::Vacant(entry) = transform_map.entry(transform_hash) {
                entry.insert(serde_json::to_string(&info.world_transform)?);
            }

            // 为 Tubing 创建 inst_relate 记录
            let relate_sql = format!(
                "{{id: {0}, in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, generic: '{4}', zone_refno: fn::find_ancestor_type({1}, 'ZONE'), dt: fn::ses_date({1}), has_cata_neg: {5}, solid: {6}, owner_refno: {7}, owner_type: '{8}'}}",
                key.to_inst_relate_key(),
                key.to_pe_key(),
                info.id_str(),
                transform_hash,
                info.generic_type.to_string(),
                info.has_cata_neg,
                info.is_solid,
                info.owner_refno.to_pe_key(),
                info.owner_type,
            );

            tubi_relate_buffer.push(relate_sql);
            if tubi_relate_buffer.len() >= CHUNK_SIZE {
                let statement = format!(
                    "INSERT RELATION INTO inst_relate [{}];",
                    tubi_relate_buffer.join(",")
                );
                inst_relate_batcher.push(statement).await?;
                tubi_relate_buffer.clear();
            }
        }

        if !tubi_relate_buffer.is_empty() {
            let statement = format!(
                "INSERT RELATION INTO inst_relate [{}];",
                tubi_relate_buffer.join(",")
            );
            inst_relate_batcher.push(statement).await?;
            debug_model_debug!(
                "save_instance_data_optimize flushing inst_relate from inst_tubi_map: {}",
                tubi_relate_buffer.len()
            );
        }
    }

    if !inst_info_buffer.is_empty() {
        let statement = format!(
            "INSERT IGNORE INTO {} [{}];",
            stringify!(inst_info),
            inst_info_buffer.join(",")
        );
        inst_info_batcher.push(statement).await?;
        debug_model_debug!(
            "save_instance_data_optimize flushing remaining inst_info records: {}",
            inst_info_buffer.len()
        );
    }

    // NOTE: 暂时跳过 has_inst 标记更新，后续单独处理以避免阻塞调试

    debug_model_debug!("🔍 [DEBUG] Finishing inst_relate_batcher...");
    inst_relate_batcher.finish().await?;
    debug_model_debug!("✅ [DEBUG] inst_relate_batcher finished successfully");

    // 调试：确认 inst_relate 是否已写入数据库
    if !inst_keys.is_empty() {
        let pe_list = inst_keys.iter().map(|k| k.to_pe_key()).join(",");
        let verify_sql = format!(
            "SELECT count() AS cnt FROM inst_relate WHERE in IN [{}];",
            pe_list
        );
        match SUL_DB.query_response(&verify_sql).await {
            Ok(mut resp) => match resp.take::<Vec<serde_json::Value>>(0) {
                Ok(counts) => debug_model_debug!(
                    "🔍 [DEBUG] inst_relate verify counts for [{}]: {:?}",
                    pe_list,
                    counts
                ),
                Err(err) => debug_model_debug!(
                    "❌ [DEBUG] inst_relate verify take failed (sql: {}): {}",
                    verify_sql,
                    err
                ),
            },
            Err(e) => {
                debug_model_debug!(
                    "❌ [DEBUG] inst_relate verify query failed (sql: {}): {}",
                    verify_sql,
                    e
                );
            }
        }
    }

    debug_model_debug!("🔍 [DEBUG] Finishing inst_info_batcher...");
    inst_info_batcher.finish().await?;
    debug_model_debug!("✅ [DEBUG] inst_info_batcher finished successfully");

    // aabb
    if !aabb_map.is_empty() {
        let mut aabb_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut json_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (&hash, value) in &aabb_map {
            json_buffer.push(format!("{{'id':aabb:⟨{}⟩, 'd':{}}}", hash, value));
            if json_buffer.len() >= CHUNK_SIZE {
                let statement = format!("INSERT IGNORE INTO aabb [{}];", json_buffer.join(","));
                aabb_batcher.push(statement).await?;
                json_buffer.clear();
            }
        }

        if !json_buffer.is_empty() {
            let statement = format!("INSERT IGNORE INTO aabb [{}];", json_buffer.join(","));
            aabb_batcher.push(statement).await?;
        }

        aabb_batcher.finish().await?;
    }

    // transform
    if !transform_map.is_empty() {
        let mut transform_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut json_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (&hash, value) in &transform_map {
            json_buffer.push(format!("{{'id':trans:⟨{}⟩, 'd':{}}}", hash, value));
            if json_buffer.len() >= CHUNK_SIZE {
                let statement = format!("INSERT IGNORE INTO trans [{}];", json_buffer.join(","));
                transform_batcher.push(statement).await?;
                json_buffer.clear();
            }
        }

        if !json_buffer.is_empty() {
            let statement = format!("INSERT IGNORE INTO trans [{}];", json_buffer.join(","));
            transform_batcher.push(statement).await?;
        }

        transform_batcher.finish().await?;
    }

    // vec3
    if !vec3_map.is_empty() {
        let mut vec3_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut json_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (&hash, value) in &vec3_map {
            json_buffer.push(format!("{{'id':vec3:⟨{}⟩, 'd':{}}}", hash, value));
            if json_buffer.len() >= CHUNK_SIZE {
                let statement = format!("INSERT IGNORE INTO vec3 [{}];", json_buffer.join(","));
                vec3_batcher.push(statement).await?;
                json_buffer.clear();
            }
        }

        if !json_buffer.is_empty() {
            let statement = format!("INSERT IGNORE INTO vec3 [{}];", json_buffer.join(","));
            vec3_batcher.push(statement).await?;
        }

        vec3_batcher.finish().await?;
    }

    debug_model_debug!(
        "save_instance_data_optimize finish: inst_info={}, inst_geo={}, tubi={}, neg={}, ngmr={}",
        inst_mgr.inst_info_map.len(),
        inst_mgr.inst_geos_map.len(),
        inst_mgr.inst_tubi_map.len(),
        inst_mgr.neg_relate_map.len(),
        inst_mgr.ngmr_neg_relate_map.len()
    );

    Ok(())
}

struct TransactionBatcher {
    max_statements: usize,
    max_concurrent: usize,
    pending: Vec<String>,
    tasks: FuturesUnordered<JoinHandle<anyhow::Result<()>>>,
}

impl TransactionBatcher {
    fn new(max_statements: usize, max_concurrent: usize) -> Self {
        let max_statements = max_statements.max(1);
        let max_concurrent = max_concurrent.max(1);
        Self {
            max_statements,
            max_concurrent,
            pending: Vec::with_capacity(max_statements),
            tasks: FuturesUnordered::new(),
        }
    }

    async fn push(&mut self, statement: String) -> anyhow::Result<()> {
        if statement.trim().is_empty() {
            return Ok(());
        }

        self.pending.push(statement);
        if self.pending.len() >= self.max_statements {
            self.flush().await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }

        let statements = std::mem::take(&mut self.pending);
        let query = build_transaction_block(&statements);
        let db = SUL_DB.clone();
        let debug_query = query.clone();
        // debug_model_debug!(
        //     "🔍 [DEBUG] TransactionBatcher flushing {} statements:\n{}",
        //     statements.len(),
        //     debug_query
        // );

        self.tasks.push(tokio::spawn(async move {
            match db.query(query).await {
                Ok(resp) => {
                    // debug_model_debug!("✅ [DEBUG] TransactionBatcher query executed successfully: {:?}", resp);
                    Ok(())
                }
                Err(err) => {
                    debug_model_debug!("❌ [DEBUG] TransactionBatcher query error: {}", err);
                    Err(anyhow::Error::from(err))
                }
            }
        }));

        self.await_if_needed().await
    }

    async fn await_if_needed(&mut self) -> anyhow::Result<()> {
        while self.tasks.len() >= self.max_concurrent {
            if let Some(result) = self.tasks.next().await {
                match result {
                    Ok(inner) => inner?,
                    Err(join_err) => return Err(join_err.into()),
                }
            }
        }
        Ok(())
    }

    async fn finish(mut self) -> anyhow::Result<()> {
        if !self.pending.is_empty() {
            self.flush().await?;
        }

        while let Some(result) = self.tasks.next().await {
            match result {
                Ok(inner) => inner?,
                Err(join_err) => return Err(join_err.into()),
            }
        }

        Ok(())
    }
}

fn build_transaction_block(statements: &[String]) -> String {
    let estimated_len = statements.iter().map(|s| s.len() + 2).sum::<usize>() + 32;
    let mut block = String::with_capacity(estimated_len);
    block.push_str("BEGIN TRANSACTION;\n");
    for stmt in statements {
        let trimmed = stmt.trim_end();
        block.push_str(trimmed);
        if !trimmed.ends_with(';') {
            block.push(';');
        }
        block.push('\n');
    }
    block.push_str("COMMIT TRANSACTION;");
    block
}
