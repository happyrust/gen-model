//! 测试 STWALL 模型布尔运算流程分析
//!
//! 测试 17496_106028 节点的布尔运算完整流程：
//! 1. 使用 gen_all_geos_data 生成指定节点的实例数据和 mesh
//! 2. 执行布尔运算
//! 3. 查询和验证负实体关系
//! 4. 检查布尔运算结果
//! 5. 导出 OBJ 文件验证

use aios_core::{
    get_db_option, query_negative_entities, RefnoEnum, SurrealQueryExt,
    init_test_surreal, SUL_DB,
};
use aios_database::fast_model::export_model::export_obj::prepare_obj_export;
use aios_database::fast_model::export_model::model_exporter::CommonExportConfig;
use aios_database::fast_model::gen_model::gen_all_geos_data;
use aios_database::fast_model::unit_converter::UnitConverter;
use aios_database::options::DbOptionExt;
use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use surrealdb::types::{self as surrealdb_types, SurrealValue};

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║   STWALL 模型测试：生成 + 负实体 + 布尔运算 + OBJ导出   ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    // 初始化数据库连接
    init_test_surreal().await?;

    let meshes_path = get_db_option().get_meshes_path();
    println!("✓ 数据库已连接");
    println!("✓ Mesh 基路径: {}\n", meshes_path.display());

    // 测试目标 - STWALL (17496_106028) has negative entity 17496_142306
    let test_refno: RefnoEnum = "17496_106028".into();
    let neg_refno: RefnoEnum = "17496_142306".into();

    println!("📋 测试目标:");
    println!("   正实体 (STWALL): {}", test_refno);
    println!("   预期负实体:      {}\n", neg_refno);

    // 步骤 1: 使用 gen_all_geos_data 生成指定节点数据和执行布尔运算
    println!("╭─────────────────────────────────────────╮");
    println!("│ 步骤 1: 生成 mesh 和执行布尔运算       │");
    println!("╰─────────────────────────────────────────╯");

    // 配置数据库选项
    let base_option = get_db_option();
    let mut db_option = DbOptionExt::from(base_option.clone());
    db_option.inner.gen_mesh = true;
    db_option.inner.apply_boolean_operation = true;
    db_option.full_noun_mode = false;

    println!("   配置:");
    println!("      gen_mesh: {}", db_option.inner.gen_mesh);
    println!("      apply_boolean_operation: {}", db_option.inner.apply_boolean_operation);
    println!("      full_noun_mode: {}", db_option.full_noun_mode);

    // 指定要生成的 refno（只需要正实体，负实体会自动处理）
    let manual_refnos = vec![test_refno];

    println!("\n   ▶ 调用 gen_all_geos_data...");
    println!("      manual_refnos: {:?}", manual_refnos);

    let success = gen_all_geos_data(
        manual_refnos,
        &db_option,
        None,  // 不使用增量更新
        None,  // 不使用历史 sesno
    ).await?;

    if success {
        println!("   ✅ gen_all_geos_data 执行成功");
        println!("      - 实例数据已生成");
        println!("      - Mesh 已生成");
        println!("      - 布尔运算已执行");
    } else {
        println!("   ⚠️ gen_all_geos_data 返回 false");
    }

    // 步骤 2: 查询负实体关系
    println!("\n╭─────────────────────────────────────────╮");
    println!("│ 步骤 2: 验证负实体关系                  │");
    println!("╰─────────────────────────────────────────╯");
    let neg_refnos = query_negative_entities_wrapper(test_refno).await?;

    // 步骤 2.5: 分析变换矩阵和包围盒
    println!("\n╭─────────────────────────────────────────╮");
    println!("│ 步骤 2.5: 分析变换矩阵和包围盒          │");
    println!("╰─────────────────────────────────────────╯");
    analyze_transforms_and_aabb(test_refno, &neg_refnos).await?;

    // 步骤 3: 检查布尔运算结果
    println!("\n╭─────────────────────────────────────────╮");
    println!("│ 步骤 3: 检查布尔运算结果                │");
    println!("╰─────────────────────────────────────────╯");
    check_boolean_result(test_refno).await?;

    // 步骤 4: 导出 OBJ 文件
    println!("\n╭─────────────────────────────────────────╮");
    println!("│ 步骤 4: 导出 OBJ 模型文件               │");
    println!("╰─────────────────────────────────────────╯");
    export_obj_models(test_refno, &neg_refnos).await?;

    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║                    ✅ 测试全部完成                        ║");
    println!("╚═══════════════════════════════════════════════════════════╝");

    Ok(())
}

/// 查询负实体关系
async fn query_negative_entities_wrapper(refno: RefnoEnum) -> Result<Vec<RefnoEnum>> {
    println!("   正实体: {}", refno);

    let neg_refnos = query_negative_entities(refno).await?;

    if neg_refnos.is_empty() {
        println!("   ⚠️ 未找到负实体关系");
        println!("      这可能表示：");
        println!("      - neg_relate 表中没有数据");
        println!("      - 需要先运行 Full Noun 模式生成");
        return Ok(vec![]);
    }

    println!("   ✅ 找到 {} 个负实体:", neg_refnos.len());
    for (i, neg) in neg_refnos.iter().enumerate() {
        println!("      {}. {}", i + 1, neg);
    }

    Ok(neg_refnos)
}

/// 检查布尔运算结果
async fn check_boolean_result(refno: RefnoEnum) -> Result<()> {
    let inst_key = refno.to_inst_relate_key();
    let inst_sql = format!(
        "SELECT in, bad_bool, booled_id FROM {}",
        inst_key
    );

    #[derive(Debug, Deserialize, SurrealValue)]
    struct InstRelateRow {
        #[serde(rename = "in")]
        in_pe: Option<RefnoEnum>,
        bad_bool: Option<bool>,
        booled_id: Option<String>,
    }

    let rows: Vec<InstRelateRow> = SUL_DB.query_take(&inst_sql, 0).await?;

    if rows.is_empty() {
        println!("   ❌ inst_relate 记录不存在");
        return Ok(());
    }

    for row in rows {
        let bad_bool = row.bad_bool.unwrap_or(false);
        let booled_id = row.booled_id.as_deref().unwrap_or("null");

        if bad_bool {
            println!("   ⚠️ 布尔运算标记为失败 (bad_bool=true)");
        } else if booled_id != "null" {
            println!("   ✅ 布尔运算成功");
            println!("      booled_id: {}", booled_id);
        } else {
            println!("   ⚠️ 未执行布尔运算 (booled_id=null)");
        }
    }

    Ok(())
}

/// 导出 OBJ 模型文件
async fn export_obj_models(pos_refno: RefnoEnum, neg_refnos: &[RefnoEnum]) -> Result<()> {
    let output_dir = PathBuf::from("./test_output");
    fs::create_dir_all(&output_dir)?;

    let meshes_path = get_db_option().get_meshes_path();

    // 准备导出配置
    let config = CommonExportConfig {
        include_descendants: false,
        filter_nouns: None,
        verbose: true,
        unit_converter: UnitConverter::default(),
        use_basic_materials: false,
    };

    // 导出正实体（布尔运算后）
    println!("   ▶ 导出正实体（布尔后）...");
    let pos_output = output_dir.join(format!(
        "stwall_{}_boolean.obj",
        pos_refno.to_string().replace('/', "_")
    ));

    let prepared = prepare_obj_export(&[pos_refno], &meshes_path, &config).await?;
    prepared
        .mesh
        .export_obj(false, pos_output.to_str().unwrap())?;

    println!("      ✓ 已导出: {}", pos_output.display());
    println!(
        "         顶点数: {}, 面数: {}",
        prepared.mesh.vertices.len(),
        prepared.mesh.indices.len() / 3
    );

    // 导出负实体
    for (i, neg_refno) in neg_refnos.iter().enumerate() {
        let neg_output = output_dir.join(format!(
            "stwall_neg{}_{}.obj",
            i + 1,
            neg_refno.to_string().replace('/', "_")
        ));

        println!("   ▶ 导出负实体 {}...", i + 1);

        let prepared = prepare_obj_export(&[*neg_refno], &meshes_path, &config).await?;
        prepared
            .mesh
            .export_obj(false, neg_output.to_str().unwrap())?;

        println!("      ✓ 已导出: {}", neg_output.display());
        println!(
            "         顶点数: {}, 面数: {}",
            prepared.mesh.vertices.len(),
            prepared.mesh.indices.len() / 3
        );
    }

    println!("\n   📁 所有文件已导出到: {}", output_dir.display());
    println!("      可使用 Blender、MeshLab 等工具查看");

    Ok(())
}

/// 分析正实体和负实体的变换矩阵和包围盒
async fn analyze_transforms_and_aabb(
    pos_refno: RefnoEnum,
    neg_refnos: &[RefnoEnum],
) -> Result<()> {
    // // 使用项目中已有的查询接口获取布尔运算需要的数据
    // use aios_core::query_manifold_boolean_operations_batch;

    // println!("   🔍 使用布尔运算查询接口分析...\n");

    // // 构建包含正实体和负实体的完整列表进行查询
    // let mut all_refnos = vec![pos_refno];
    // all_refnos.extend(neg_refnos.iter().cloned());

    // match query_manifold_boolean_operations_batch(&all_refnos).await {
    //     Ok(queries) => {
    //         println!("   ✅ 成功查询到 {} 个布尔运算目标", queries.len());

    //         for query in &queries {
    //             println!("\n   📍 正实体: {}", query.pos_refno);
    //             println!("      mesh_id: {:?}", query.mesh_id);
    //             println!("      world_trans (位置): [{:.3}, {:.3}, {:.3}]",
    //                 query.wt.translation.x, query.wt.translation.y, query.wt.translation.z);
    //             println!("      world_trans (旋转): [{:.3}, {:.3}, {:.3}, {:.3}]",
    //                 query.wt.rotation.x, query.wt.rotation.y, query.wt.rotation.z, query.wt.rotation.w);

    //             if let Some(ref aabb) = query.aabb {
    //                 let min = &aabb.0.mins;
    //                 let max = &aabb.0.maxs;
    //                 println!("      AABB min: [{:.3}, {:.3}, {:.3}]", min.x, min.y, min.z);
    //                 println!("      AABB max: [{:.3}, {:.3}, {:.3}]", max.x, max.y, max.z);
    //             }

    //             println!("      负实体数量: {}", query.negs.len());
    //             for (i, neg) in query.negs.iter().enumerate() {
    //                 println!("\n      负实体 {}: {:?}", i + 1, neg.neg_refno);
    //                 if let Some(ref carrier) = neg.carrier {
    //                     println!("         carrier: {:?}", carrier);
    //                 }
    //                 if let Some(ref trans) = neg.transform {
    //                     println!("         transform (位置): [{:.3}, {:.3}, {:.3}]",
    //                         trans.translation.x, trans.translation.y, trans.translation.z);
    //                     println!("         transform (旋转): [{:.3}, {:.3}, {:.3}, {:.3}]",
    //                         trans.rotation.x, trans.rotation.y, trans.rotation.z, trans.rotation.w);
    //                 }
    //                 if let Some(ref aabb) = neg.aabb {
    //                     let min = &aabb.0.mins;
    //                     let max = &aabb.0.maxs;
    //                     println!("         AABB min: [{:.3}, {:.3}, {:.3}]", min.x, min.y, min.z);
    //                     println!("         AABB max: [{:.3}, {:.3}, {:.3}]", max.x, max.y, max.z);
    //                 }
    //                 println!("         mesh_id: {:?}", neg.mesh_id);
    //             }
    //         }

    //         if queries.is_empty() {
    //             println!("\n   ⚠️ 没有找到需要布尔运算的目标！");
    //             println!("      可能原因：");
    //             println!("      1. neg_relate 表中没有数据");
    //             println!("      2. 负实体没有对应的 mesh 或 inst_relate 记录");
    //         }
    //     }
    //     Err(e) => {
    //         println!("   ❌ 查询失败: {}", e);
    //     }
    // }

    Ok(())
}
