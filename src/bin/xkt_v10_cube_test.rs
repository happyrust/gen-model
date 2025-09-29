use aios_database::xkt_generator::*;
use glam;
use clap::{Arg, Command};
use tokio;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Command::new("XKT v10 Cube Test")
        .version("1.0")
        .author("AIOS Database Team")
        .about("生成符合 XKT v10 标准规范的测试立方体")
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("FILE")
                .help("输出文件路径")
                .default_value("./output/cube_v10_standard.xkt"),
        )
        .arg(
            Arg::new("size")
                .short('s')
                .long("size")
                .value_name("SIZE")
                .help("立方体大小")
                .default_value("1.0"),
        )
        .get_matches();

    let output_path = matches.get_one::<String>("output").unwrap();
    let size: f32 = matches.get_one::<String>("size").unwrap().parse()?;

    println!("=== XKT v10 标准立方体生成器 ===");
    println!("输出文件: {}", output_path);
    println!("立方体大小: {}", size);
    println!();

    // 创建立方体几何体
    println!("创建立方体几何体...");
    let cube_geometry = XKTGeometry::create_box("cube_geo".to_string(), size, size, size);

    // 创建XKT文件
    let mut xkt_file = XKTFile::new();

    // 添加几何体到模型
    println!("添加几何体到模型...");
    xkt_file.model.create_geometry(cube_geometry)?;

    // 创建材质
    println!("创建材质...");
    let mut material = XKTMaterial::new("cube_material".to_string(), "Cube Material".to_string());
    material.metallic = 0.0;
    material.roughness = 0.5;
    material.diffuse = glam::Vec3::new(1.0, 1.0, 1.0); // 白色
    xkt_file.model.create_material(material)?;

    // 创建网格
    println!("创建网格...");
    let mut cube_mesh = XKTMesh::new("cube_mesh".to_string(), "cube_geo".to_string());
    cube_mesh.set_material("cube_material".to_string());
    xkt_file.model.create_mesh(cube_mesh)?;

    // 创建实体
    println!("创建实体...");
    let mut entity = XKTEntity::new(
        "cube_entity".to_string(),
        "Test Cube".to_string(),
        "IfcBuildingElementProxy".to_string(),
    );
    entity.add_mesh("cube_mesh".to_string());
    xkt_file.model.create_entity(entity)?;

    // 确保输出目录存在
    if let Some(parent) = std::path::Path::new(output_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // 使用新的XKT v10标准写入器保存
    println!("保存为标准 XKT v10 格式文件...");
    xkt_file.save_to_file_v10(output_path).await?;

    println!();
    println!("=== 生成完成 ===");
    println!("已生成符合 XKT v10 标准规范的立方体文件: {}", output_path);
    println!();
    println!("文件特性:");
    println!("- 严格按照 XKT v10 规范实现");
    println!("- 包含29个标准数据段");
    println!("- 位置数据量化为16位无符号整数");
    println!("- 法向量oct-encoding压缩为8位整数");
    println!("- 使用zlib压缩所有数据段");
    println!("- 包含边缘索引用于线框渲染");
    println!("- 支持瓦片分块（当前为单瓦片）");
    println!();
    println!("该文件可以直接使用 xeokit 查看器加载和显示。");
    println!("更多信息请访问: https://xeokit.github.io/");

    // 验证文件大小
    let metadata = tokio::fs::metadata(output_path).await?;
    println!("文件大小: {} 字节", metadata.len());

    Ok(())
}