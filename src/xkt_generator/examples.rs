use super::*;
use glam::Vec3;
use std::f32::consts::PI;

/// 创建一个简单的桌子模型示例
pub async fn create_table_example() -> anyhow::Result<()> {
    println!("创建桌子模型示例...");
    
    let mut xkt_file = XKTFile::new();
    xkt_file.model.metadata.title = "简单桌子模型".to_string();
    xkt_file.model.metadata.author = "XKT Generator Example".to_string();
    
    let color_scheme = ColorScheme::new();
    
    // 创建几何体 - 立方体用于桌面和桌腿
    let box_geometry = XKTGeometry::create_box("box_geometry".to_string(), 1.0, 1.0, 1.0);
    xkt_file.model.create_geometry(box_geometry)?;
    
    // 创建材质
    let wood_color = color_scheme.get_material_color("WOOD");
    let wood_material = XKTMaterial::create_color_material(
        "wood_material".to_string(),
        "木材".to_string(),
        wood_color,
    );
    xkt_file.model.create_material(wood_material)?;
    
    // 创建桌面网格
    let mut table_top_mesh = XKTMesh::new("table_top_mesh".to_string(), "box_geometry".to_string());
    table_top_mesh.set_material("wood_material".to_string());
    table_top_mesh.set_position(Vec3::new(0.0, 0.0, 0.0));
    table_top_mesh.set_scale(Vec3::new(6.0, 0.5, 4.0)); // 桌面：宽6，厚0.5，深4
    xkt_file.model.create_mesh(table_top_mesh)?;
    
    // 创建四条桌腿
    let leg_positions = vec![
        Vec3::new(-2.5, -1.5, -1.5), // 左前腿
        Vec3::new(2.5, -1.5, -1.5),  // 右前腿
        Vec3::new(-2.5, -1.5, 1.5),  // 左后腿
        Vec3::new(2.5, -1.5, 1.5),   // 右后腿
    ];
    
    for (i, position) in leg_positions.iter().enumerate() {
        let mut leg_mesh = XKTMesh::new(
            format!("table_leg_{}_mesh", i + 1),
            "box_geometry".to_string(),
        );
        leg_mesh.set_material("wood_material".to_string());
        leg_mesh.set_position(*position);
        leg_mesh.set_scale(Vec3::new(0.3, 3.0, 0.3)); // 桌腿：宽0.3，高3，深0.3
        xkt_file.model.create_mesh(leg_mesh)?;
    }
    
    // 创建实体
    let mut table_entity = XKTEntity::new("table_001".to_string(), "桌子-001".to_string(), "FURNITURE".to_string());
    table_entity.add_mesh("table_top_mesh".to_string());
    table_entity.add_mesh("table_leg_1_mesh".to_string());
    table_entity.add_mesh("table_leg_2_mesh".to_string());
    table_entity.add_mesh("table_leg_3_mesh".to_string());
    table_entity.add_mesh("table_leg_4_mesh".to_string());
    
    table_entity.set_property("type".to_string(), "Dining Table".to_string());
    table_entity.set_property("material".to_string(), "Oak Wood".to_string());
    table_entity.set_property("dimensions".to_string(), "6x4x3 units".to_string());
    
    xkt_file.model.create_entity(table_entity)?;
    
    // 完成并保存
    xkt_file.model.finalize().await?;
    
    std::fs::create_dir_all("examples_output").ok();
    xkt_file.save_to_file("examples_output/table_example.xkt", true).await?;
    
    println!("桌子模型已保存到 examples_output/table_example.xkt");
    Ok(())
}

/// 创建一个简单的管道系统示例
pub async fn create_piping_system_example() -> anyhow::Result<()> {
    println!("创建管道系统示例...");
    
    let mut xkt_file = XKTFile::new();
    xkt_file.model.metadata.title = "简单管道系统".to_string();
    xkt_file.model.metadata.author = "XKT Generator Example".to_string();
    
    let color_scheme = ColorScheme::new();
    
    // 创建几何体
    let cylinder_geometry = XKTGeometry::create_cylinder("cylinder_geometry".to_string(), 1.0, 1.0, 16);
    let sphere_geometry = XKTGeometry::create_sphere("sphere_geometry".to_string(), 1.0, 16, 16);
    let box_geometry = XKTGeometry::create_box("box_geometry".to_string(), 1.0, 1.0, 1.0);
    
    xkt_file.model.create_geometry(cylinder_geometry)?;
    xkt_file.model.create_geometry(sphere_geometry)?;
    xkt_file.model.create_geometry(box_geometry)?;
    
    // 创建材质
    let pipe_color = color_scheme.get_color_for_type("PIPE");
    let valve_color = color_scheme.get_color_for_type("VALVE");
    let equipment_color = color_scheme.get_color_for_type("EQUIPMENT");
    
    let pipe_material = XKTMaterial::create_metallic_material(
        "pipe_material".to_string(),
        "管道材质".to_string(),
        pipe_color,
    );
    let valve_material = XKTMaterial::create_metallic_material(
        "valve_material".to_string(),
        "阀门材质".to_string(),
        valve_color,
    );
    let equipment_material = XKTMaterial::create_color_material(
        "equipment_material".to_string(),
        "设备材质".to_string(),
        equipment_color,
    );
    
    xkt_file.model.create_material(pipe_material)?;
    xkt_file.model.create_material(valve_material)?;
    xkt_file.model.create_material(equipment_material)?;
    
    // 创建主管道
    let mut main_pipe_mesh = XKTMesh::new("main_pipe_mesh".to_string(), "cylinder_geometry".to_string());
    main_pipe_mesh.set_material("pipe_material".to_string());
    main_pipe_mesh.set_position(Vec3::new(0.0, 0.0, 0.0));
    main_pipe_mesh.set_scale(Vec3::new(0.2, 10.0, 0.2)); // 细长的管道
    main_pipe_mesh.set_rotation(Vec3::new(0.0, 0.0, PI / 2.0)); // 水平放置
    xkt_file.model.create_mesh(main_pipe_mesh)?;
    
    // 创建分支管道
    let mut branch_pipe_mesh = XKTMesh::new("branch_pipe_mesh".to_string(), "cylinder_geometry".to_string());
    branch_pipe_mesh.set_material("pipe_material".to_string());
    branch_pipe_mesh.set_position(Vec3::new(0.0, 3.0, 0.0));
    branch_pipe_mesh.set_scale(Vec3::new(0.15, 6.0, 0.15));
    xkt_file.model.create_mesh(branch_pipe_mesh)?;
    
    // 创建阀门
    let mut valve_mesh = XKTMesh::new("valve_mesh".to_string(), "sphere_geometry".to_string());
    valve_mesh.set_material("valve_material".to_string());
    valve_mesh.set_position(Vec3::new(2.0, 0.0, 0.0));
    valve_mesh.set_scale(Vec3::new(0.5, 0.5, 0.5));
    xkt_file.model.create_mesh(valve_mesh)?;
    
    // 创建设备（泵）
    let mut pump_mesh = XKTMesh::new("pump_mesh".to_string(), "box_geometry".to_string());
    pump_mesh.set_material("equipment_material".to_string());
    pump_mesh.set_position(Vec3::new(-4.0, 0.0, 0.0));
    pump_mesh.set_scale(Vec3::new(1.5, 1.0, 1.0));
    xkt_file.model.create_mesh(pump_mesh)?;
    
    // 创建实体
    let mut main_pipe_entity = XKTEntity::new("pipe_main_001".to_string(), "主管道-001".to_string(), "PIPE".to_string());
    main_pipe_entity.add_mesh("main_pipe_mesh".to_string());
    main_pipe_entity.set_property("diameter".to_string(), "200".to_string());
    main_pipe_entity.set_property("material".to_string(), "Stainless Steel 316L".to_string());
    main_pipe_entity.set_property("pressure_rating".to_string(), "PN25".to_string());
    
    let mut branch_pipe_entity = XKTEntity::new("pipe_branch_001".to_string(), "分支管道-001".to_string(), "PIPE".to_string());
    branch_pipe_entity.add_mesh("branch_pipe_mesh".to_string());
    branch_pipe_entity.set_property("diameter".to_string(), "150".to_string());
    branch_pipe_entity.set_property("material".to_string(), "Stainless Steel 316L".to_string());
    
    let mut valve_entity = XKTEntity::new("valve_001".to_string(), "球阀-001".to_string(), "VALVE".to_string());
    valve_entity.add_mesh("valve_mesh".to_string());
    valve_entity.set_property("type".to_string(), "Ball Valve".to_string());
    valve_entity.set_property("size".to_string(), "8 inch".to_string());
    valve_entity.set_property("pressure_rating".to_string(), "PN25".to_string());
    
    let mut pump_entity = XKTEntity::new("pump_001".to_string(), "离心泵-001".to_string(), "EQUIPMENT".to_string());
    pump_entity.add_mesh("pump_mesh".to_string());
    pump_entity.set_property("type".to_string(), "Centrifugal Pump".to_string());
    pump_entity.set_property("capacity".to_string(), "500 m3/h".to_string());
    pump_entity.set_property("head".to_string(), "50 m".to_string());
    pump_entity.set_property("power".to_string(), "75 kW".to_string());
    
    xkt_file.model.create_entity(main_pipe_entity)?;
    xkt_file.model.create_entity(branch_pipe_entity)?;
    xkt_file.model.create_entity(valve_entity)?;
    xkt_file.model.create_entity(pump_entity)?;
    
    // 完成并保存
    xkt_file.model.finalize().await?;
    
    std::fs::create_dir_all("examples_output").ok();
    xkt_file.save_to_file("examples_output/piping_system_example.xkt", true).await?;
    
    println!("管道系统已保存到 examples_output/piping_system_example.xkt");
    Ok(())
}

/// 创建一个工厂布局示例
pub async fn create_factory_layout_example() -> anyhow::Result<()> {
    println!("创建工厂布局示例...");
    
    let mut xkt_file = XKTFile::new();
    xkt_file.model.metadata.title = "工厂布局示例".to_string();
    xkt_file.model.metadata.author = "XKT Generator Example".to_string();
    
    let color_scheme = ColorScheme::new();
    
    // 创建几何体
    let box_geometry = XKTGeometry::create_box("box_geometry".to_string(), 1.0, 1.0, 1.0);
    let cylinder_geometry = XKTGeometry::create_cylinder("cylinder_geometry".to_string(), 1.0, 1.0, 16);
    let sphere_geometry = XKTGeometry::create_sphere("sphere_geometry".to_string(), 1.0, 16, 16);
    
    xkt_file.model.create_geometry(box_geometry)?;
    xkt_file.model.create_geometry(cylinder_geometry)?;
    xkt_file.model.create_geometry(sphere_geometry)?;
    
    // 创建材质
    let structure_color = color_scheme.get_color_for_type("STRUCTURE");
    let equipment_color = color_scheme.get_color_for_type("EQUIPMENT");
    let pipe_color = color_scheme.get_color_for_type("PIPE");
    let electrical_color = color_scheme.get_color_for_type("ELECTRICAL");
    
    let structure_material = XKTMaterial::create_metallic_material(
        "structure_material".to_string(),
        "结构材质".to_string(),
        structure_color,
    );
    let equipment_material = XKTMaterial::create_color_material(
        "equipment_material".to_string(),
        "设备材质".to_string(),
        equipment_color,
    );
    let pipe_material = XKTMaterial::create_metallic_material(
        "pipe_material".to_string(),
        "管道材质".to_string(),
        pipe_color,
    );
    let electrical_material = XKTMaterial::create_plastic_material(
        "electrical_material".to_string(),
        "电气材质".to_string(),
        electrical_color,
    );
    
    xkt_file.model.create_material(structure_material)?;
    xkt_file.model.create_material(equipment_material)?;
    xkt_file.model.create_material(pipe_material)?;
    xkt_file.model.create_material(electrical_material)?;
    
    // 创建建筑结构
    let building_components = vec![
        ("foundation", "地基", Vec3::new(0.0, -2.0, 0.0), Vec3::new(20.0, 1.0, 15.0)),
        ("column_1", "柱子-1", Vec3::new(-8.0, 2.0, -6.0), Vec3::new(0.5, 8.0, 0.5)),
        ("column_2", "柱子-2", Vec3::new(8.0, 2.0, -6.0), Vec3::new(0.5, 8.0, 0.5)),
        ("column_3", "柱子-3", Vec3::new(-8.0, 2.0, 6.0), Vec3::new(0.5, 8.0, 0.5)),
        ("column_4", "柱子-4", Vec3::new(8.0, 2.0, 6.0), Vec3::new(0.5, 8.0, 0.5)),
        ("beam_1", "梁-1", Vec3::new(0.0, 6.0, -6.0), Vec3::new(16.0, 0.5, 0.5)),
        ("beam_2", "梁-2", Vec3::new(0.0, 6.0, 6.0), Vec3::new(16.0, 0.5, 0.5)),
        ("beam_3", "梁-3", Vec3::new(-8.0, 6.0, 0.0), Vec3::new(0.5, 0.5, 12.0)),
        ("beam_4", "梁-4", Vec3::new(8.0, 6.0, 0.0), Vec3::new(0.5, 0.5, 12.0)),
    ];
    
    for (id, name, position, scale) in building_components {
        let mut mesh = XKTMesh::new(format!("{}_mesh", id), "box_geometry".to_string());
        mesh.set_material("structure_material".to_string());
        mesh.set_position(position);
        mesh.set_scale(scale);
        
        let mesh_id = mesh.id.clone();
        xkt_file.model.create_mesh(mesh)?;
        
        let mut entity = XKTEntity::new(id.to_string(), name.to_string(), "STRUCTURE".to_string());
        entity.add_mesh(mesh_id);
        entity.set_property("material".to_string(), "Structural Steel".to_string());
        entity.set_property("grade".to_string(), "S355".to_string());
        
        xkt_file.model.create_entity(entity)?;
    }
    
    // 创建设备
    let equipment_list = vec![
        ("reactor_001", "反应器-001", Vec3::new(-4.0, 1.0, 0.0), Vec3::new(2.0, 4.0, 2.0)),
        ("heat_exchanger_001", "换热器-001", Vec3::new(4.0, 1.0, 0.0), Vec3::new(1.5, 3.0, 1.5)),
        ("pump_001", "泵-001", Vec3::new(-4.0, 0.0, -4.0), Vec3::new(1.0, 1.0, 1.5)),
        ("compressor_001", "压缩机-001", Vec3::new(4.0, 0.0, 4.0), Vec3::new(1.5, 1.5, 2.0)),
    ];
    
    for (id, name, position, scale) in equipment_list {
        let geometry = if id.contains("pump") || id.contains("compressor") {
            "box_geometry"
        } else {
            "cylinder_geometry"
        };
        
        let mut mesh = XKTMesh::new(format!("{}_mesh", id), geometry.to_string());
        mesh.set_material("equipment_material".to_string());
        mesh.set_position(position);
        mesh.set_scale(scale);
        
        let mesh_id = mesh.id.clone();
        xkt_file.model.create_mesh(mesh)?;
        
        let mut entity = XKTEntity::new(id.to_string(), name.to_string(), "EQUIPMENT".to_string());
        entity.add_mesh(mesh_id);
        entity.set_property("manufacturer".to_string(), "ABC Equipment Co.".to_string());
        entity.set_property("model".to_string(), format!("{}-2024", id.to_uppercase()));
        
        if id.contains("reactor") {
            entity.set_property("volume".to_string(), "10 m3".to_string());
            entity.set_property("pressure".to_string(), "10 bar".to_string());
        } else if id.contains("heat_exchanger") {
            entity.set_property("area".to_string(), "50 m2".to_string());
            entity.set_property("duty".to_string(), "1 MW".to_string());
        } else if id.contains("pump") {
            entity.set_property("flow_rate".to_string(), "100 m3/h".to_string());
            entity.set_property("head".to_string(), "30 m".to_string());
        } else if id.contains("compressor") {
            entity.set_property("capacity".to_string(), "1000 Nm3/h".to_string());
            entity.set_property("pressure_ratio".to_string(), "3:1".to_string());
        }
        
        xkt_file.model.create_entity(entity)?;
    }
    
    // 创建管道连接
    let pipe_connections = vec![
        ("pipe_001", "管道-001", Vec3::new(-2.0, 1.0, 0.0), Vec3::new(0.1, 4.0, 0.1), Vec3::new(0.0, 0.0, PI/2.0)),
        ("pipe_002", "管道-002", Vec3::new(2.0, 1.0, 0.0), Vec3::new(0.1, 4.0, 0.1), Vec3::new(0.0, 0.0, PI/2.0)),
        ("pipe_003", "管道-003", Vec3::new(0.0, 2.0, -2.0), Vec3::new(0.1, 4.0, 0.1), Vec3::new(0.0, 0.0, 0.0)),
        ("pipe_004", "管道-004", Vec3::new(0.0, 2.0, 2.0), Vec3::new(0.1, 4.0, 0.1), Vec3::new(0.0, 0.0, 0.0)),
    ];
    
    for (id, name, position, scale, rotation) in pipe_connections {
        let mut mesh = XKTMesh::new(format!("{}_mesh", id), "cylinder_geometry".to_string());
        mesh.set_material("pipe_material".to_string());
        mesh.set_position(position);
        mesh.set_scale(scale);
        mesh.set_rotation(rotation);
        
        let mesh_id = mesh.id.clone();
        xkt_file.model.create_mesh(mesh)?;
        
        let mut entity = XKTEntity::new(id.to_string(), name.to_string(), "PIPE".to_string());
        entity.add_mesh(mesh_id);
        entity.set_property("diameter".to_string(), "100".to_string());
        entity.set_property("material".to_string(), "Stainless Steel 316L".to_string());
        entity.set_property("insulation".to_string(), "Mineral Wool".to_string());
        
        xkt_file.model.create_entity(entity)?;
    }
    
    // 完成并保存
    xkt_file.model.finalize().await?;
    
    std::fs::create_dir_all("examples_output").ok();
    xkt_file.save_to_file("examples_output/factory_layout_example.xkt", true).await?;
    
    println!("工厂布局已保存到 examples_output/factory_layout_example.xkt");
    Ok(())
}

/// 运行所有示例
pub async fn run_all_examples() -> anyhow::Result<()> {
    println!("=== 开始运行 XKT 生成器示例 ===");
    
    create_table_example().await?;
    create_piping_system_example().await?;
    create_factory_layout_example().await?;
    
    println!("=== 所有示例已完成！ ===");
    println!("生成的文件位于 examples_output/ 目录中");
    Ok(())
}

/// 从数据库生成 XTK 文件的示例
pub async fn example_generate_xtk_from_database() -> anyhow::Result<()> {
    use crate::fast_model::gen_model::{generate_xtk_from_database, generate_xtk_by_dbno};
    use aios_core::options::DbOption;
    use aios_core::RefnoEnum;
    
    println!("=== 从数据库生成 XTK 文件示例 ===");
    
    // 创建数据库选项配置
    let db_option = DbOption::default();
    
    // 示例1: 根据指定的参考号列表生成 XTK
    let refnos = vec![
        "12345/67890".into(),
        "23456/78901".into(),
        "34567/89012".into(),
    ];
    
    println!("示例1: 根据参考号列表生成 XTK");
    match generate_xtk_from_database(
        refnos,
        "examples_output/database_model_by_refnos.xkt",
        true, // 启用压缩
        &db_option,
    ).await {
        Ok(_) => println!("✅ 成功生成 database_model_by_refnos.xkt"),
        Err(e) => eprintln!("❌ 生成失败: {}", e),
    }
    
    // 示例2: 根据数据库号生成整个数据库的 XTK
    println!("\n示例2: 根据数据库号生成 XTK");
    match generate_xtk_by_dbno(
        1, // 数据库号
        "examples_output/database_model_full.xkt",
        true, // 启用压缩
        &db_option,
    ).await {
        Ok(_) => println!("✅ 成功生成 database_model_full.xkt"),
        Err(e) => eprintln!("❌ 生成失败: {}", e),
    }
    
    println!("\n=== XTK 数据库导出示例完成 ===");
    Ok(())
}

/// 批量生成多个数据库的 XTK 文件
pub async fn example_batch_generate_xtk() -> anyhow::Result<()> {
    use crate::fast_model::gen_model::generate_xtk_by_dbno;
    use aios_core::options::DbOption;
    
    println!("=== 批量生成 XTK 文件示例 ===");
    
    let db_option = DbOption::default();
    let database_numbers = vec![1, 2, 3, 4, 5]; // 要导出的数据库号列表
    
    std::fs::create_dir_all("examples_output/batch_export").ok();
    
    for dbno in database_numbers {
        let output_path = format!("examples_output/batch_export/database_{}.xkt", dbno);
        
        println!("正在处理数据库号: {}", dbno);
        match generate_xtk_by_dbno(dbno, &output_path, true, &db_option).await {
            Ok(_) => println!("✅ 数据库 {} 导出成功", dbno),
            Err(e) => eprintln!("❌ 数据库 {} 导出失败: {}", dbno, e),
        }
    }
    
    println!("=== 批量导出完成 ===");
    Ok(())
}

/// 生成带有自定义过滤条件的 XTK 文件
pub async fn example_filtered_xtk_generation() -> anyhow::Result<()> {
    use crate::fast_model::gen_model::generate_xtk_from_database;
    use aios_core::options::DbOption;
    use aios_core::{query_type_refnos_by_dbnum, RefnoEnum};
    
    println!("=== 过滤生成 XTK 文件示例 ===");
    
    let db_option = DbOption::default();
    
    // 查询特定类型的参考号（例如只导出管道相关的元素）
    let pipe_types = ["PIPE", "ELBO", "TEE", "REDU"];
    let all_refnos = query_type_refnos_by_dbnum(&pipe_types, 1, None, false).await?;
    
    println!("找到 {} 个管道相关元素", all_refnos.len());
    
    // 只导出管道系统
    match generate_xtk_from_database(
        all_refnos,
        "examples_output/piping_system.xkt",
        true,
        &db_option,
    ).await {
        Ok(_) => println!("✅ 管道系统 XTK 文件生成成功"),
        Err(e) => eprintln!("❌ 管道系统导出失败: {}", e),
    }
    
    println!("=== 过滤导出完成 ===");
    Ok(())
} 