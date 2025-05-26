use super::*;
use glam::Vec3;
use tokio;

/// 测试基本的 XKT 文件生成
async fn test_basic_xkt_generation_impl() -> anyhow::Result<()> {
    println!("开始测试基本 XKT 文件生成...");
    
    // 创建 XKT 文件
    let mut xkt_file = XKTFile::new();
    
    // 设置模型元数据
    xkt_file.model.metadata.title = "测试 PDMS 模型".to_string();
    xkt_file.model.metadata.author = "XKT 生成器测试".to_string();
    
    // 创建颜色方案
    let color_scheme = ColorScheme::new();
    
    // 创建几何体
    let box_geometry = XKTGeometry::create_box("box_geometry".to_string(), 2.0, 1.0, 1.0);
    let sphere_geometry = XKTGeometry::create_sphere("sphere_geometry".to_string(), 0.5, 16, 16);
    let cylinder_geometry = XKTGeometry::create_cylinder("cylinder_geometry".to_string(), 0.3, 2.0, 12);
    
    // 添加几何体到模型
    xkt_file.model.create_geometry(box_geometry)?;
    xkt_file.model.create_geometry(sphere_geometry)?;
    xkt_file.model.create_geometry(cylinder_geometry)?;
    
    // 创建材质
    let pipe_color = color_scheme.get_color_for_type("PIPE");
    let valve_color = color_scheme.get_color_for_type("VALVE");
    let equipment_color = color_scheme.get_color_for_type("EQUIPMENT");
    
    let pipe_material = XKTMaterial::create_color_material(
        "pipe_material".to_string(),
        "管道材质".to_string(),
        pipe_color,
    );
    let valve_material = XKTMaterial::create_metallic_material(
        "valve_material".to_string(),
        "阀门材质".to_string(),
        valve_color,
    );
    let equipment_material = XKTMaterial::create_plastic_material(
        "equipment_material".to_string(),
        "设备材质".to_string(),
        equipment_color,
    );
    
    // 添加材质到模型
    xkt_file.model.create_material(pipe_material)?;
    xkt_file.model.create_material(valve_material)?;
    xkt_file.model.create_material(equipment_material)?;
    
    // 创建网格
    let mut pipe_mesh = XKTMesh::new("pipe_mesh".to_string(), "box_geometry".to_string());
    pipe_mesh.set_material("pipe_material".to_string());
    pipe_mesh.set_position(Vec3::new(0.0, 0.0, 0.0));
    
    let mut valve_mesh = XKTMesh::new("valve_mesh".to_string(), "sphere_geometry".to_string());
    valve_mesh.set_material("valve_material".to_string());
    valve_mesh.set_position(Vec3::new(3.0, 0.0, 0.0));
    
    let mut equipment_mesh = XKTMesh::new("equipment_mesh".to_string(), "cylinder_geometry".to_string());
    equipment_mesh.set_material("equipment_material".to_string());
    equipment_mesh.set_position(Vec3::new(-3.0, 0.0, 0.0));
    equipment_mesh.set_rotation(Vec3::new(0.0, 0.0, std::f32::consts::PI / 2.0)); // 旋转90度
    
    // 添加网格到模型
    xkt_file.model.create_mesh(pipe_mesh)?;
    xkt_file.model.create_mesh(valve_mesh)?;
    xkt_file.model.create_mesh(equipment_mesh)?;
    
    // 创建实体
    let mut pipe_entity = XKTEntity::new("pipe_001".to_string(), "管道-001".to_string(), "PIPE".to_string());
    pipe_entity.add_mesh("pipe_mesh".to_string());
    pipe_entity.set_property("diameter".to_string(), "100".to_string());
    pipe_entity.set_property("material".to_string(), "Carbon Steel".to_string());
    
    let mut valve_entity = XKTEntity::new("valve_001".to_string(), "阀门-001".to_string(), "VALVE".to_string());
    valve_entity.add_mesh("valve_mesh".to_string());
    valve_entity.set_property("type".to_string(), "Gate Valve".to_string());
    valve_entity.set_property("size".to_string(), "4 inch".to_string());
    
    let mut equipment_entity = XKTEntity::new("equipment_001".to_string(), "设备-001".to_string(), "EQUIPMENT".to_string());
    equipment_entity.add_mesh("equipment_mesh".to_string());
    equipment_entity.set_property("type".to_string(), "Pump".to_string());
    equipment_entity.set_property("capacity".to_string(), "100 m3/h".to_string());
    
    // 添加实体到模型
    xkt_file.model.create_entity(pipe_entity)?;
    xkt_file.model.create_entity(valve_entity)?;
    xkt_file.model.create_entity(equipment_entity)?;
    
    // 完成模型构建
    xkt_file.model.finalize().await?;
    
    // 生成 XKT 文件
    let output_path = "test_output/basic_test.xkt";
    std::fs::create_dir_all("test_output").ok();
    
    // 测试未压缩版本
    xkt_file.save_to_file(output_path, false).await?;
    
    // 测试压缩版本
    let compressed_path = "test_output/basic_test_compressed.xkt";
    xkt_file.save_to_file(compressed_path, true).await?;
    
    // 验证文件是否生成
    assert!(std::path::Path::new(output_path).exists());
    assert!(std::path::Path::new(compressed_path).exists());
    
    // 验证压缩文件更小
    let uncompressed_size = std::fs::metadata(output_path)?.len();
    let compressed_size = std::fs::metadata(compressed_path)?.len();
    println!("未压缩文件大小: {} bytes", uncompressed_size);
    println!("压缩文件大小: {} bytes", compressed_size);
    
    // 验证文件头
    let file_data = std::fs::read(output_path)?;
    assert!(XKTReader::validate_header(&file_data)?);
    
    println!("基本 XKT 文件生成测试通过！");
    Ok(())
}

/// 测试复杂场景的 XKT 文件生成
async fn test_complex_scene_generation_impl() -> anyhow::Result<()> {
    println!("开始测试复杂场景 XKT 文件生成...");
    
    let mut xkt_file = XKTFile::new();
    xkt_file.model.metadata.title = "复杂 PDMS 场景".to_string();
    
    let color_scheme = ColorScheme::new();
    
    // 创建多种几何体
    let geometries = vec![
        ("box_small", XKTGeometry::create_box("box_small".to_string(), 0.5, 0.5, 0.5)),
        ("box_large", XKTGeometry::create_box("box_large".to_string(), 2.0, 1.0, 1.0)),
        ("sphere_small", XKTGeometry::create_sphere("sphere_small".to_string(), 0.3, 12, 12)),
        ("sphere_large", XKTGeometry::create_sphere("sphere_large".to_string(), 0.8, 20, 20)),
        ("cylinder_thin", XKTGeometry::create_cylinder("cylinder_thin".to_string(), 0.1, 3.0, 8)),
        ("cylinder_thick", XKTGeometry::create_cylinder("cylinder_thick".to_string(), 0.5, 1.5, 16)),
    ];
    
    for (_, geometry) in geometries {
        xkt_file.model.create_geometry(geometry)?;
    }
    
    // 创建多种材质
    let material_types = vec![
        ("PIPE", "管道"),
        ("VALVE", "阀门"),
        ("EQUIPMENT", "设备"),
        ("STRUCTURE", "结构"),
        ("INSTRUMENT", "仪表"),
        ("ELECTRICAL", "电气"),
    ];
    
    for (type_name, display_name) in material_types {
        let color = color_scheme.get_color_for_type(type_name);
        let material = XKTMaterial::create_color_material(
            format!("{}_material", type_name.to_lowercase()),
            format!("{} 材质", display_name),
            color,
        );
        xkt_file.model.create_material(material)?;
    }
    
    // 创建网格和实体的组合
    let components = vec![
        ("pipe_001", "管道-001", "PIPE", "box_large", Vec3::new(0.0, 0.0, 0.0)),
        ("pipe_002", "管道-002", "PIPE", "cylinder_thin", Vec3::new(2.0, 0.0, 0.0)),
        ("valve_001", "阀门-001", "VALVE", "sphere_small", Vec3::new(4.0, 0.0, 0.0)),
        ("valve_002", "阀门-002", "VALVE", "box_small", Vec3::new(6.0, 0.0, 0.0)),
        ("equipment_001", "设备-001", "EQUIPMENT", "cylinder_thick", Vec3::new(0.0, 2.0, 0.0)),
        ("equipment_002", "设备-002", "EQUIPMENT", "sphere_large", Vec3::new(2.0, 2.0, 0.0)),
        ("structure_001", "结构-001", "STRUCTURE", "box_large", Vec3::new(4.0, 2.0, 0.0)),
        ("instrument_001", "仪表-001", "INSTRUMENT", "sphere_small", Vec3::new(0.0, 4.0, 0.0)),
        ("electrical_001", "电气-001", "ELECTRICAL", "cylinder_thin", Vec3::new(2.0, 4.0, 0.0)),
    ];
    
    for (id, name, entity_type, geometry_id, position) in components {
        // 创建网格
        let mut mesh = XKTMesh::new(
            format!("{}_mesh", id),
            geometry_id.to_string(),
        );
        mesh.set_material(format!("{}_material", entity_type.to_lowercase()));
        mesh.set_position(position);
        
        // 添加一些随机旋转
        if entity_type == "PIPE" {
            mesh.set_rotation(Vec3::new(0.0, 0.0, std::f32::consts::PI / 4.0));
        }
        
        let mesh_id = mesh.id.clone();
        xkt_file.model.create_mesh(mesh)?;
        
        // 创建实体
        let mut entity = XKTEntity::new(id.to_string(), name.to_string(), entity_type.to_string());
        entity.add_mesh(mesh_id);
        
        // 添加属性
        entity.set_property("created_by".to_string(), "XKT Generator".to_string());
        entity.set_property("creation_date".to_string(), chrono::Utc::now().to_rfc3339());
        
        match entity_type {
            "PIPE" => {
                entity.set_property("diameter".to_string(), "150".to_string());
                entity.set_property("material".to_string(), "Stainless Steel".to_string());
            }
            "VALVE" => {
                entity.set_property("type".to_string(), "Ball Valve".to_string());
                entity.set_property("pressure_rating".to_string(), "PN16".to_string());
            }
            "EQUIPMENT" => {
                entity.set_property("type".to_string(), "Heat Exchanger".to_string());
                entity.set_property("capacity".to_string(), "500 kW".to_string());
            }
            _ => {}
        }
        
        xkt_file.model.create_entity(entity)?;
    }
    
    // 完成模型构建
    xkt_file.model.finalize().await?;
    
    // 生成文件
    let output_path = "test_output/complex_scene.xkt";
    xkt_file.save_to_file(output_path, true).await?;
    
    // 验证文件
    assert!(std::path::Path::new(output_path).exists());
    let file_data = std::fs::read(output_path)?;
    assert!(XKTReader::validate_header(&file_data)?);
    
    // 输出统计信息
    println!("复杂场景统计:");
    println!("  几何体数量: {}", xkt_file.model.stats.num_geometries);
    println!("  材质数量: {}", xkt_file.model.stats.num_materials);
    println!("  网格数量: {}", xkt_file.model.stats.num_meshes);
    println!("  实体数量: {}", xkt_file.model.stats.num_entities);
    println!("  三角形数量: {}", xkt_file.model.stats.num_triangles);
    println!("  顶点数量: {}", xkt_file.model.stats.num_vertices);
    
    println!("复杂场景 XKT 文件生成测试通过！");
    Ok(())
}

/// 测试颜色方案
#[test]
fn test_color_scheme() {
    println!("开始测试颜色方案...");
    
    let color_scheme = ColorScheme::new();
    
    // 测试预定义类型
    let pipe_color = color_scheme.get_color_for_type("PIPE");
    assert_eq!(pipe_color, Vec3::new(0.2, 0.4, 0.8));
    
    let valve_color = color_scheme.get_color_for_type("VALVE");
    assert_eq!(valve_color, Vec3::new(0.8, 0.2, 0.2));
    
    // 测试部分匹配
    let pipe_component_color = color_scheme.get_color_for_type("PIPE_COMPONENT");
    assert_eq!(pipe_component_color, Vec3::new(0.2, 0.4, 0.8));
    
    // 测试未知类型
    let unknown_color = color_scheme.get_color_for_type("UNKNOWN_TYPE");
    assert_eq!(unknown_color, Vec3::new(0.7, 0.7, 0.7)); // 默认颜色
    
    // 测试哈希颜色生成
    let hash_color1 = color_scheme.generate_hash_color("CUSTOM_TYPE_1");
    let hash_color2 = color_scheme.generate_hash_color("CUSTOM_TYPE_2");
    assert_ne!(hash_color1, hash_color2); // 不同类型应该生成不同颜色
    
    // 测试材质颜色
    let steel_color = color_scheme.get_material_color("STEEL");
    assert_eq!(steel_color, Vec3::new(0.7, 0.7, 0.8));
    
    println!("颜色方案测试通过！");
}

/// 测试几何体创建
#[test]
fn test_geometry_creation() {
    println!("开始测试几何体创建...");
    
    // 测试立方体
    let box_geo = XKTGeometry::create_box("test_box".to_string(), 2.0, 1.0, 1.0);
    assert_eq!(box_geo.geometry_type, XKTGeometryType::Triangles);
    assert_eq!(box_geo.vertex_count(), 24); // 立方体有24个顶点（每面4个）
    assert_eq!(box_geo.triangle_count(), 12); // 立方体有12个三角形（每面2个）
    assert!(box_geo.bounding_box.is_some());
    
    // 测试球体
    let sphere_geo = XKTGeometry::create_sphere("test_sphere".to_string(), 1.0, 8, 6);
    assert_eq!(sphere_geo.geometry_type, XKTGeometryType::Triangles);
    assert!(sphere_geo.vertex_count() > 0);
    assert!(sphere_geo.triangle_count() > 0);
    assert!(sphere_geo.bounding_box.is_some());
    
    // 测试圆柱体
    let cylinder_geo = XKTGeometry::create_cylinder("test_cylinder".to_string(), 0.5, 2.0, 8);
    assert_eq!(cylinder_geo.geometry_type, XKTGeometryType::Triangles);
    assert!(cylinder_geo.vertex_count() > 0);
    assert!(cylinder_geo.triangle_count() > 0);
    assert!(cylinder_geo.bounding_box.is_some());
    
    println!("几何体创建测试通过！");
}

/// 测试基本的 XKT 文件生成（包装函数）
#[tokio::test]
async fn test_basic_xkt_generation() -> anyhow::Result<()> {
    test_basic_xkt_generation_impl().await
}

/// 测试复杂场景的 XKT 文件生成（包装函数）
#[tokio::test]
async fn test_complex_scene_generation() -> anyhow::Result<()> {
    test_complex_scene_generation_impl().await
}

/// 运行所有测试
pub async fn run_all_tests() -> anyhow::Result<()> {
    println!("=== 开始运行 XKT 生成器测试套件 ===");
    
    // 创建输出目录
    std::fs::create_dir_all("test_output").ok();
    
    // 运行同步测试
    test_color_scheme();
    test_geometry_creation();
    
    // 运行异步测试
    test_basic_xkt_generation_impl().await?;
    test_complex_scene_generation_impl().await?;
    
    println!("=== 所有测试通过！ ===");
    Ok(())
}

#[tokio::test]
async fn test_xtk_database_generation() -> anyhow::Result<()> {
    use crate::fast_model::gen_model::{generate_xtk_from_database, ElementInfo};
    use aios_core::options::DbOption;
    use aios_core::RefnoEnum;
    
    println!("=== 测试 XTK 数据库生成 ===");
    
    // 创建测试用的参考号
    let test_refnos = vec![
        "12345/67890".into(),
        "23456/78901".into(),
    ];
    
    // 创建数据库选项
    let db_option = DbOption::default();
    
    // 创建输出目录
    std::fs::create_dir_all("test_output").ok();
    
    // 测试生成 XTK 文件
    match generate_xtk_from_database(
        test_refnos,
        "test_output/test_database_model.xkt",
        true,
        &db_option,
    ).await {
        Ok(_) => {
            println!("✅ XTK 数据库生成测试成功");
            
            // 验证文件是否存在
            assert!(std::path::Path::new("test_output/test_database_model.xkt").exists());
            
            // 验证文件大小
            let metadata = std::fs::metadata("test_output/test_database_model.xkt")?;
            assert!(metadata.len() > 0);
            
            println!("生成的文件大小: {} 字节", metadata.len());
        }
        Err(e) => {
            eprintln!("❌ XTK 数据库生成测试失败: {}", e);
            return Err(e);
        }
    }
    
    Ok(())
}

#[tokio::test]
async fn test_xtk_geometry_conversion() -> anyhow::Result<()> {
    use crate::fast_model::gen_model::create_geometry_from_geo_param;
    use aios_core::geometry::EleInstGeo;
    use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
    use aios_core::prim_geo::sbox::SBox;
    use bevy_transform::prelude::Transform;
    
    println!("=== 测试几何体转换 ===");
    
    // 创建测试用的几何参数
    let box_param = SBox {
        size: Vec3::new(2.0, 1.5, 1.0),
        center: Vec3::new(0.0, 0.0, 0.0),
    };
    
    let geo_instance = EleInstGeo {
        geo_hash: 12345,
        refno: "test/123".into(),
        pts: vec![],
        aabb: None,
        transform: Transform::IDENTITY,
        geo_param: PdmsGeoParam::PrimBox(box_param),
        visible: true,
        is_tubi: false,
        geo_type: aios_core::geometry::GeoBasicType::Pos,
        cata_neg_refnos: vec![],
    };
    
    // 测试几何体创建
    match create_geometry_from_geo_param("test_box", &[geo_instance]).await {
        Ok(geometry) => {
            println!("✅ 几何体转换测试成功");
            assert_eq!(geometry.id, "test_box");
            assert_eq!(geometry.geometry_type, crate::xkt_generator::XKTGeometryType::Triangles);
            assert!(!geometry.positions.is_empty());
            assert!(!geometry.indices.is_empty());
            
            println!("几何体顶点数: {}", geometry.positions.len() / 3);
            println!("几何体三角形数: {}", geometry.indices.len() / 3);
        }
        Err(e) => {
            eprintln!("❌ 几何体转换测试失败: {}", e);
            return Err(e);
        }
    }
    
    Ok(())
}

#[test]
fn test_element_info_creation() {
    use crate::fast_model::gen_model::ElementInfo;
    
    println!("=== 测试元素信息创建 ===");
    
    let element_info = ElementInfo {
        name: Some("测试管道".to_string()),
        type_name: "PIPE".to_string(),
    };
    
    assert_eq!(element_info.name, Some("测试管道".to_string()));
    assert_eq!(element_info.type_name, "PIPE");
    
    println!("✅ 元素信息创建测试成功");
} 