use aios_database::xkt_generator::*;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== XKT v10 调试分析 ===");

    // 创建立方体几何体
    let cube_geometry = XKTGeometry::create_box("cube_geo".to_string(), 1.0, 1.0, 1.0);
    println!("几何体信息:");
    println!("  ID: {}", cube_geometry.id);
    println!("  顶点数量: {}", cube_geometry.positions.len() / 3);
    println!("  索引数量: {}", cube_geometry.indices.len());
    println!("  包围盒: {:?}", cube_geometry.bounding_box);

    // 创建XKT文件
    let mut xkt_file = XKTFile::new();

    // 添加几何体
    xkt_file.model.create_geometry(cube_geometry)?;
    println!("\n模型中的几何体数量: {}", xkt_file.model.geometries.len());

    // 创建材质
    let mut material = XKTMaterial::new("cube_material".to_string(), "Cube Material".to_string());
    material.metallic = 0.0;
    material.roughness = 0.5;
    material.diffuse = glam::Vec3::new(1.0, 1.0, 1.0);
    xkt_file.model.create_material(material)?;
    println!("模型中的材质数量: {}", xkt_file.model.materials.len());

    // 创建网格
    let mut cube_mesh = XKTMesh::new("cube_mesh".to_string(), "cube_geo".to_string());
    cube_mesh.set_material("cube_material".to_string());
    xkt_file.model.create_mesh(cube_mesh)?;
    println!("模型中的网格数量: {}", xkt_file.model.meshes.len());

    // 创建实体
    let mut entity = XKTEntity::new(
        "cube_entity".to_string(),
        "Test Cube".to_string(),
        "IfcBuildingElementProxy".to_string(),
    );
    entity.add_mesh("cube_mesh".to_string());
    xkt_file.model.create_entity(entity)?;
    println!("模型中的实体数量: {}", xkt_file.model.entities.len());

    // 检查实体的网格引用
    if let Some(entity) = xkt_file.model.entities.get("cube_entity") {
        println!("实体 '{}' 的网格引用: {:?}", entity.id, entity.mesh_ids);
    }

    // 检查网格的几何体引用
    if let Some(mesh) = xkt_file.model.meshes.get("cube_mesh") {
        println!("网格 '{}' 的几何体引用: {}", mesh.id, mesh.geometry_id);
        println!("网格材质引用: {:?}", mesh.material_id);
    }

    // 最终化模型
    xkt_file.model.finalize().await?;
    println!("\n模型最终化完成");
    println!("最终实体列表长度: {}", xkt_file.model.entities_list.len());
    println!("最终网格列表长度: {}", xkt_file.model.meshes_list.len());

    // 检查最终化后的数据
    for (index, entity) in xkt_file.model.entities_list.iter().enumerate() {
        println!("实体[{}]: ID={}, 网格数量={}", index, entity.id, entity.mesh_ids.len());
    }

    for (index, mesh) in xkt_file.model.meshes_list.iter().enumerate() {
        println!("网格[{}]: ID={}, 几何体={}", index, mesh.id, mesh.geometry_id);
    }

    // 生成XKT文件并分析
    println!("\n=== 生成调试XKT文件 ===");
    xkt_file.save_to_file_v10("output/debug_cube_v10.xkt").await?;

    Ok(())
}