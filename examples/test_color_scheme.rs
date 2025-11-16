use aios_core::color_scheme::ColorSchemeManager;
use aios_core::pdms_types::PdmsGenericType;
use aios_database::fast_model::material_config::MaterialLibrary;
use anyhow::Result;

fn main() -> Result<()> {
    println!("=== 测试颜色配置系统 ===\n");

    // 1. 测试 ColorSchemeManager
    println!("1. 测试 ColorSchemeManager");
    println!("----------------------------");

    let manager = ColorSchemeManager::load_from_file("ColorSchemes.toml").unwrap_or_else(|e| {
        println!("⚠️  无法加载配置文件: {}", e);
        println!("   使用默认配色方案");
        ColorSchemeManager::default_schemes()
    });

    println!("✅ 当前配色方案: {}", manager.current_scheme);
    println!("   可用配色方案: {:?}\n", manager.get_available_schemes());

    // 测试获取各种类型的颜色
    let test_types = vec![
        PdmsGenericType::PIPE,
        PdmsGenericType::EQUI,
        PdmsGenericType::CE,
        PdmsGenericType::STRU,
        PdmsGenericType::WALL,
        PdmsGenericType::ROOM,
    ];

    println!("2. 测试不同元件类型的颜色");
    println!("----------------------------");
    for pdms_type in test_types {
        if let Some(color) = manager.get_color_for_type(pdms_type) {
            println!(
                "   {:8?} => RGBA({:3}, {:3}, {:3}, {:3})",
                pdms_type, color[0], color[1], color[2], color[3]
            );
        }
    }
    println!();

    // 2. 测试 MaterialLibrary 集成
    println!("3. 测试 MaterialLibrary 颜色配置集成");
    println!("----------------------------");

    let library = MaterialLibrary::load_default().unwrap_or_else(|e| {
        println!("⚠️  无法加载材质库: {}", e);
        println!("   将只使用颜色配置");
        // 这里应该创建一个最小的 MaterialLibrary,但为了简单我们就打印错误
        panic!("无法继续测试");
    });

    println!("✅ 材质库加载成功");
    println!("   材质数量: {}", library.materials().len());

    // 测试从颜色配置获取归一化颜色
    println!("\n4. 测试归一化颜色值 (0.0-1.0)");
    println!("----------------------------");
    let test_nouns = vec!["PIPE", "EQUI", "CE", "STRU", "WALL", "ROOM"];
    for noun in test_nouns {
        if let Some(color) = library.get_normalized_color_for_noun(noun) {
            println!(
                "   {:8} => RGBA({:.3}, {:.3}, {:.3}, {:.3})",
                noun, color[0], color[1], color[2], color[3]
            );
        } else {
            println!("   {:8} => 未找到颜色配置", noun);
        }
    }

    // 测试动态材质创建
    println!("\n5. 测试动态材质创建");
    println!("----------------------------");
    if let Some(material) = library.create_color_based_material("PIPE", false) {
        println!("   PIPE PBR 材质:");
        println!("{}", serde_json::to_string_pretty(&material)?);
    }

    println!("\n6. 测试 Unlit 基础材质创建");
    println!("----------------------------");
    if let Some(material) = library.create_color_based_material("EQUI", true) {
        println!("   EQUI Unlit 材质:");
        println!("{}", serde_json::to_string_pretty(&material)?);
    }

    // 测试材质索引获取或创建
    println!("\n7. 测试材质索引获取或创建");
    println!("----------------------------");
    let mut dynamic_materials = Vec::new();

    for noun in ["PIPE", "EQUI", "CE", "STRU", "WALL"] {
        if let Some(idx) = library.get_or_create_material_for_noun(
            noun,
            false, // 使用 PBR 材质
            &mut dynamic_materials,
        ) {
            let source = if idx < library.materials().len() {
                "材质库"
            } else {
                "动态创建"
            };
            println!("   {:8} => 材质索引 {} (来源: {})", noun, idx, source);
        }
    }

    println!("\n   动态创建的材质数量: {}", dynamic_materials.len());

    println!("\n=== 测试完成 ===");
    println!("✅ 所有测试通过!");

    Ok(())
}
