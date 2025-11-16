use crate::fast_model::query_gm_params;
use crate::fast_model::{debug_model, debug_model_debug, debug_model_trace};
use aios_core::consts::WORD_HASH;
use aios_core::expression::query_cata::{query_axis_params, resolve_cata_comp};
use aios_core::expression::resolve::{SCOM_INFO_MAP, resolve_axis_param};
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::{PlinParam, ScomInfo};
use aios_core::{CataContext, RefU64, RefnoEnum, SUL_DB};
use anyhow::anyhow;
use std::collections::{BTreeMap, HashMap};

/// 查询 DESI 元素的 IPARAM 数据
/// 使用 SurrealDB 的 fn::get_ipara 函数
async fn query_iparam_from_desi(desi_refno: RefnoEnum) -> anyhow::Result<Vec<f32>> {
    let sql = format!("return fn::get_ipara(pe:{})", desi_refno.to_string());

    let mut response = SUL_DB.query(sql).await?;
    let result: Option<Vec<f32>> = response.take(0)?;

    Ok(result.unwrap_or(vec![]))
}

///收集SCOM的信息, 暂时慎用缓存
pub async fn get_or_create_scom_info(cata_refno: RefnoEnum) -> anyhow::Result<ScomInfo> {
    // ⚠️  调试模式下清除缓存，确保使用最新的参数
    if aios_core::is_debug_model_enabled() {
        SCOM_INFO_MAP.remove(&cata_refno);
        debug_model_debug!("Cleared SCOM_INFO_MAP cache for {}", cata_refno);
    }

    let scom_info = if let Some(info) = SCOM_INFO_MAP.get(&cata_refno) {
        info.value().clone()
    } else {
        let attr_map = aios_core::get_named_attmap(cata_refno).await?;
        let type_noun = attr_map.get_type_str();
        let ptref_name = match type_noun {
            "SPRF" => "PSTR",
            _ => "PTRE",
        };
        let mut axis_params = vec![];
        let mut axis_param_numbers = vec![];
        if let Some(ptre_refno) = attr_map.get_foreign_refno(ptref_name) {
            if let Ok(axis_param_map) = query_axis_params(ptre_refno).await {
                axis_params = axis_param_map.values().cloned().collect::<Vec<_>>();
                axis_param_numbers = axis_param_map.keys().cloned().collect::<Vec<_>>();
            }
        }
        let gmse_refno =
            aios_core::query_single_by_paths(cata_refno, &["->GMRE", "->GSTR"], &["REFNO"])
                .await
                .map(|x| x.get_refno_or_default())?;
        debug_model_trace!("gmse_refno: {:?}", gmse_refno);
        let gm_params = query_gm_params(gmse_refno).await?;
        let mut ngm_params = vec![];
        //-ve， 和design发生左右的负实体
        if let Some(gmse_refno) = attr_map.get_foreign_refno("NGMR") {
            ngm_params = query_gm_params(gmse_refno).await?;
        }

        let mut plin_map = HashMap::new();
        if let Some(pstr_refno) = attr_map.get_foreign_refno("PSTR") {
            // 使用新的泛型函数接口
            let pstr_am = aios_core::collect_children_filter_attrs(pstr_refno, &[]).await?;
            for a in pstr_am {
                if let Some(k) = a.get_as_string("PKEY") {
                    plin_map.insert(
                        k,
                        PlinParam {
                            vxy: [
                                a.get_as_string("PX").unwrap_or("0".to_string()),
                                a.get_as_string("PY").unwrap_or("0".to_string()),
                            ],
                            dxy: [
                                a.get_as_string("DX").unwrap_or("0".to_string()),
                                a.get_as_string("DY").unwrap_or("0".to_string()),
                            ],
                            plax: a.get_as_string("PLAX").unwrap_or("unset".to_string()),
                        },
                    );
                }
            }
        }
        ScomInfo {
            gtype: attr_map.get_as_string("GTYP").unwrap_or("unset".into()),
            dtse_params: vec![],
            gm_params,
            ngm_params,
            axis_params,
            params: attr_map
                .get_as_string("PARA")
                .unwrap_or_default()
                .replace("\n", " ")
                .replace("  ", " ")
                .into(),
            axis_param_numbers,
            attr_map,
            plin_map,
        }
    };
    Ok(scom_info)
}

/// 求解axis的数值
pub async fn resolve_axis_params(
    refno: RefnoEnum,
    context: Option<CataContext>,
) -> anyhow::Result<BTreeMap<i32, CateAxisParam>> {
    let mut map = BTreeMap::new();
    let scom_refno = aios_core::get_cat_refno(refno).await?.unwrap_or_default();
    if !scom_refno.is_valid() {
        return Ok(Default::default());
    }
    let scom = get_or_create_scom_info(scom_refno).await?;
    let context = context.unwrap_or(aios_core::get_or_create_cata_context(refno, false).await?);
    for i in 0..scom.axis_params.len() {
        let axis = resolve_axis_param(&scom.axis_params[i], &scom, &context);
        map.insert(scom.axis_param_numbers[i], axis);
    }
    Ok(map)
}

///求解design component
pub async fn resolve_desi_comp(
    desi_refno: RefnoEnum,
    mut tubi_scom: Option<RefnoEnum>,
) -> anyhow::Result<CateGeomsInfo> {
    let desi_att = aios_core::get_named_attmap(desi_refno).await?;
    let is_tubi = tubi_scom.is_some();

    let scom_ref = if let Some(scom) = tubi_scom {
        scom
    } else {
        let scom = aios_core::get_cat_refno(desi_refno)
            .await?
            .ok_or(anyhow::anyhow!(format!(
                "CAT引用不存在: {}",
                desi_refno.to_string()
            )))?;
        scom
    };
    debug_model_trace!("scom_ref: {:?}", &scom_ref);
    let scom_info = get_or_create_scom_info(scom_ref).await?;
    debug_model_trace!("scom_info: {:?}", &scom_info);
    let mut context = aios_core::get_or_create_cata_context(desi_refno, is_tubi)
        .await
        .unwrap();

    // 🔍 调试：打印 DESI 的 DESP 数据
    if let Ok(desi_attmap) = aios_core::get_named_attmap(desi_refno).await {
        if let Some(desp) = desi_attmap.get_f32_vec("DESP") {
            debug_model_trace!("   ✅ DESP array: {:?}", desp);
            if let Some(unipar) = desi_attmap.get_i32_vec("UNIPAR") {
                debug_model_trace!("   UNIPAR array: {:?}", unipar);

                use aios_core::consts::WORD_HASH;
                use aios_core::tool::db_tool::db1_dehash;

                for (i, (&value, &utype)) in desp.iter().zip(unipar.iter()).enumerate() {
                    if utype == WORD_HASH as i32 {
                        let word = db1_dehash(value as u32);
                        debug_model_trace!(
                            "      DESP[{}] = {} ⚠️  类型=WORD, dehash='{}'",
                            i,
                            value,
                            word
                        );
                    } else {
                        debug_model_trace!("      DESP[{}] = {} ✅ 类型=数值", i, value);
                    }
                }
            }
        } else {
            debug_model_trace!("   ⚠️  DESI 没有 DESP 属性");
        }
    }

    // 添加 SCOM 的 PARA 数组到 context 中
    // PARA 字符串格式: " 100.000 100.000 534980.000 ..."
    // 需要解析为: "PARAM 0" = "100.0", "PARAM 1" = "100.0", ...
    // 注意：表达式解析器会将 "PARAM" 截断为 "PARA"（去掉末尾的 "M"）
    // 所以需要同时添加 "PARA0", "PARAM0", "PARAM 0" 等多个版本
    let para_str = &scom_info.params;
    let para_values: Vec<&str> = para_str.split_whitespace().collect();

    // 🔍 调试输出：打印 PARA 字符串和解析结果
    debug_model_trace!(
        "🔍 [SCOM PARA] desi_refno={:?}, scom_refno={:?}",
        desi_refno,
        scom_ref
    );
    debug_model_trace!("   PARA string: '{}'", para_str);
    debug_model_trace!("   Parsed values: {:?}", para_values);

    // 🔍 查询 SurrealDB 中的原始 PARA 数据和 UNIPAR
    let unipar_vec = if let Ok(scom_attmap) = aios_core::get_named_attmap(scom_ref).await {
        if let Some(raw_para) = scom_attmap.get_as_string("PARA") {
            debug_model_trace!("   🔍 SurrealDB 原始 PARA: '{}'", raw_para);
        }
        // 打印 SCOM 的其他关键属性
        debug_model_trace!("   🔍 SCOM name: {:?}", scom_attmap.get_as_string("NAME"));
        debug_model_trace!("   🔍 SCOM noun: {:?}", scom_attmap.get_type_str());

        // 🔍 关键：查询 UNIPAR（参数类型描述）
        if let Some(unipar) = scom_attmap.get_i32_vec("UNIPAR") {
            debug_model_trace!("   🔍 UNIPAR (参数类型): {:?}", unipar);

            // 分析每个 PARA 值的类型
            use aios_core::tool::db_tool::db1_dehash;

            for (i, (value, &utype)) in para_values.iter().zip(unipar.iter()).enumerate() {
                if utype == WORD_HASH as i32 {
                    // 尝试 dehash WORD 类型的值
                    // 先解析为 f32，再转为 u32
                    if let Ok(num_value) = value.parse::<f32>() {
                        let word = db1_dehash(num_value as u32);
                        debug_model_trace!(
                            "      PARA[{}] = {} ⚠️  类型=WORD, dehash='{}'",
                            i,
                            value,
                            word
                        );
                    } else {
                        debug_model_trace!(
                            "      PARA[{}] = {} ⚠️  类型=WORD (无法解析为数字)",
                            i,
                            value
                        );
                    }
                } else {
                    debug_model_trace!("      PARA[{}] = {} ✅ 类型=数值 (几何尺寸)", i, value);
                }
            }
            Some(unipar)
        } else {
            debug_model_trace!("   ⚠️  SCOM 没有 UNIPAR 属性");
            None
        }
    } else {
        None
    };

    // ⚠️  重要修复：
    // 1. 表达式中的 "PARAM" 实际上是指 SCOM 的 PARA 参数
    // 2. "PARAM 2" 会被转换为 "PARA2"（索引从 1 开始）
    // 3. 需要将 SCOM 的 PARA 添加为 "PARA1", "PARA2", ... 等键名
    // 4. 但是需要过滤掉 WORD 类型的参数（UNIPAR[i] = 623723）

    // 添加 SCOM 的 PARA 为 PARA1, PARA2, ...（索引从 1 开始）
    for (i, value) in para_values.iter().enumerate() {
        let is_word_type = unipar_vec
            .as_ref()
            .and_then(|unipar| unipar.get(i))
            .map(|&u| u == WORD_HASH as i32)
            .unwrap_or(false);

        if is_word_type {
            // WORD 类型的参数不应该用于几何计算，使用默认值 0.0
            debug_model_trace!(
                "   ⚠️  PARA[{}] 是 WORD 类型，PARA{} 使用默认值 0.0",
                i,
                i + 1
            );
            context.insert(format!("PARA{}", i + 1), "0.0".to_string());
        } else {
            context.insert(format!("PARA{}", i + 1), value.to_string());
        }

        // 同时添加 CPAR（Catalogue Parameter）
        context.insert(format!("CPAR{}", i + 1), value.to_string());
    }

    // 添加 IPARAM 数据到 context 中
    // 使用 SurrealDB 的 fn::get_ipara 函数查询
    // 注意：表达式解析器会将 "IPARAM" 截断为 "IPARA"（去掉末尾的 "M"）
    match query_iparam_from_desi(desi_refno).await {
        Ok(iparams) => {
            debug_model_debug!("IPARAM query result: {:?}", iparams);

            for (i, value) in iparams.iter().enumerate() {
                // IPARAM 使用空格分隔的键名，索引从 1 开始（根据注释中的 IPAR1, IPARM1）
                // 添加所有可能的键名格式
                context.insert(format!("IPARAM {}", i + 1), value.to_string());
                context.insert(format!("IPARAM{}", i + 1), value.to_string());
                context.insert(format!("IPARA {}", i + 1), value.to_string());
                context.insert(format!("IPARA{}", i + 1), value.to_string());
                context.insert(format!("IPAR {}", i + 1), value.to_string());
                context.insert(format!("IPAR{}", i + 1), value.to_string());
                context.insert(format!("IPARM {}", i + 1), value.to_string());
                context.insert(format!("IPARM{}", i + 1), value.to_string());
            }
        }
        Err(e) => {
            crate::smart_debug_error!("Failed to query IPARAM: {}", e);
        }
    }

    crate::smart_debug_model_debug!("=== DEBUG: CataContext for {} ===", desi_refno.to_string());
    crate::smart_debug_model_debug!("Context entries count: {}", context.context.len());
    crate::smart_debug_model_debug!("PARA string: {}", para_str);
    crate::smart_debug_model_debug!("Parsed {} PARAM values", para_values.len());
    // 打印所有 PARAM 相关的键值对
    if aios_core::is_debug_model_enabled() {
        for entry in context.context.iter() {
            let key = entry.key();
            let value = entry.value();
            if key.contains("PARAM") || key.contains("PARA") || key.contains("IPARAM") {
                // debug_model_debug!("  {} = {}", key, value);
            }
        }
    }
    // debug_model_debug!("=== END Context ===");

    let geom_info = resolve_cata_comp(&desi_att, &scom_info, Some(context));
    debug_model_trace!("geom_info: {:?}", &geom_info);
    geom_info.map_err(|_| anyhow!("resolve_cata_comp failed"))
}
