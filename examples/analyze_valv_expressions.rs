//! 分析阀门 21491_16837 的表达式生成问题
//!
//! 运行: cargo run --example analyze_valv_expressions

use aios_core::{RefnoEnum, get_named_attmap, init_surreal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🔍 分析阀门 21491_16837 的表达式生成问题");
    println!("{}", "=".repeat(70));

    // 1. 初始化数据库
    println!("\n📦 步骤 1: 初始化数据库连接...");
    let _db = init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 启用 debug 模式
    aios_core::set_debug_model_enabled(true);

    let target_refno = RefnoEnum::from("21491_16837");
    println!("\n🎯 目标阀门: {}", target_refno);

    // 2. 查询阀门基本信息
    println!("\n📋 步骤 2: 查询阀门基本信息...");
    let attr_map = get_named_attmap(target_refno).await?;
    println!("   NOUN: {}", attr_map.get_type_str());
    println!("   NAME: {:?}", attr_map.get_as_string("NAME"));
    println!("   CATA_HASH: {:?}", attr_map.get_as_string("CATA_HASH"));
    println!("   OWNER: {:?}", attr_map.get_owner());

    // 3. 查询 CAT 引用（元件库引用）
    println!("\n📚 步骤 3: 查询 CAT 引用（SPRF/SPRE）...");
    let cat_refno = aios_core::get_cat_refno(target_refno).await?;
    println!("   CAT refno: {:?}", cat_refno);

    if let Some(cat_ref) = cat_refno {
        // 4. 查询 SCOM 属性
        println!("\n📖 步骤 4: 查询 SCOM/SPRF 属性...");
        let cat_attr = get_named_attmap(cat_ref).await?;
        println!("   SCOM NOUN: {}", cat_attr.get_type_str());
        println!("   SCOM NAME: {:?}", cat_attr.get_as_string("NAME"));
        println!("   GTYP: {:?}", cat_attr.get_as_string("GTYP"));
        println!("   PARA: {:?}", cat_attr.get_as_string("PARA"));
        println!("   UNIPAR: {:?}", cat_attr.get_i32_vec("UNIPAR"));

        // 5. 查询 GMSE（几何集合）引用
        println!("\n📐 步骤 5: 查询几何集合引用 (GMRE/GSTR)...");
        let gmse_result =
            aios_core::query_single_by_paths(cat_ref, &["->GMRE", "->GSTR"], &["REFNO"]).await;
        println!("   GMSE 查询结果: {:?}", gmse_result);

        if let Ok(gmse_attr) = gmse_result {
            let gmse_refno = gmse_attr.get_refno_or_default();
            if gmse_refno.is_valid() {
                println!("   GMSE refno: {}", gmse_refno);

                // 6. 查询几何体子元素
                println!("\n🔺 步骤 6: 查询几何体子元素...");
                let geo_children = aios_core::get_children_named_attmaps(gmse_refno).await?;
                println!("   几何体子元素数量: {}", geo_children.len());

                for (i, geo) in geo_children.iter().enumerate() {
                    println!("\n   --- 几何体 {} ---", i);
                    println!("   类型: {}", geo.get_type_str());
                    println!("   REFNO: {:?}", geo.get_refno());

                    // 打印关键几何表达式
                    let attrs = [
                        "PDIA", "PBDM", "PTDM", "DIAM", "PDIS", "PBDI", "PTDI", "PHEI", "PRAD",
                        "PANG", "PWID", "POFF", "PX", "PY", "PZ", "PAXI", "PBAX", "PCAX",
                    ];

                    for attr in attrs {
                        if let Some(val) = geo.get_as_string(attr) {
                            if !val.is_empty() && val != "unset" && val != "0" {
                                // 检查是否包含表达式
                                let has_expr = val.contains("PARAM")
                                    || val.contains("PARA")
                                    || val.contains("DESP")
                                    || val.contains("ATTRIB")
                                    || val.contains("*")
                                    || val.contains("/")
                                    || val.contains("+")
                                    || val.contains("-")
                                    || val.contains("IF")
                                    || val.contains("THEN");

                                let marker = if has_expr { "📝" } else { "  " };
                                println!("   {} {}: {}", marker, attr, val);
                            }
                        }
                    }
                }
            }
        }

        // 7. 查询 PTRE（Point Reference）- 轴点定义
        println!("\n📍 步骤 7: 查询轴点定义 (PTRE)...");
        if let Some(ptre_refno) = cat_attr.get_foreign_refno("PTRE") {
            println!("   PTRE refno: {}", ptre_refno);
            let axis_children = aios_core::get_children_named_attmaps(ptre_refno).await?;
            println!("   轴点子元素数量: {}", axis_children.len());

            for (i, axis) in axis_children.iter().enumerate() {
                println!("\n   --- 轴点 {} ---", i);
                println!("   类型: {}", axis.get_type_str());
                println!("   NUMB: {:?}", axis.get_i32("NUMB"));

                let attrs = [
                    "PDIS", "PAXI", "PZAXI", "PX", "PY", "PZ", "PCON", "PBOR", "PWID", "PHEI",
                    "PTCD", "PTCP", "PTCPOS",
                ];
                for attr in attrs {
                    if let Some(val) = axis.get_as_string(attr) {
                        if !val.is_empty() && val != "unset" {
                            let has_expr = val.contains("PARAM") || val.contains("PARA");
                            let marker = if has_expr { "📝" } else { "  " };
                            println!("   {} {}: {}", marker, attr, val);
                        }
                    }
                }
            }
        }
    } else {
        println!("   ⚠️  未找到 CAT 引用！这可能是问题所在。");
    }

    // 8. 查询阀门的子元素（DESI）
    println!("\n🔧 步骤 8: 查询阀门的子元素 (DESI)...");
    let desi_children = aios_core::collect_children_filter_attrs(target_refno, &[]).await?;
    println!("   DESI 子元素数量: {}", desi_children.len());

    for (i, desi) in desi_children.iter().enumerate() {
        println!("\n   --- DESI {} ---", i);
        println!("   类型: {}", desi.get_type_str());
        println!("   REFNO: {:?}", desi.get_refno());
        println!("   NAME: {:?}", desi.get_as_string("NAME"));

        // 查询 DESI 的 DESP 参数
        if let Some(desp) = desi.get_f32_vec("DESP") {
            println!("   DESP: {:?}", desp);
        }
        if let Some(unipar) = desi.get_i32_vec("UNIPAR") {
            println!("   UNIPAR: {:?}", unipar);
        }

        // 查询 DESI 的 CAT 引用
        if let Some(desi_refno) = desi.get_refno() {
            if let Ok(Some(desi_cat)) = aios_core::get_cat_refno(desi_refno).await {
                println!("   DESI CAT: {}", desi_cat);
            }
        }
    }

    // 9. 尝试解析并生成模型
    println!("\n🔨 步骤 9: 尝试解析表达式并生成模型...");

    // 使用 resolve_desi_comp 来分析表达式
    use aios_database::fast_model::resolve_desi_comp;

    // 如果有 CAT 引用，尝试解析
    if let Some(cat_ref) = cat_refno {
        println!("   尝试解析 CAT: {}", cat_ref);

        match resolve_desi_comp(target_refno, None).await {
            Ok(geoms_info) => {
                println!("   ✅ 解析成功!");
                println!("   几何体数量: {}", geoms_info.geometries.len());
                println!("   负几何体数量: {}", geoms_info.n_geometries.len());

                for (i, geo) in geoms_info.geometries.iter().enumerate() {
                    println!("\n   --- 解析后几何体 {} ---", i);
                    println!("   {:?}", geo);
                }
            }
            Err(e) => {
                println!("   ❌ 解析失败: {}", e);
            }
        }
    }

    println!("\n{}", "=".repeat(70));
    println!("🏁 分析完成");

    Ok(())
}
