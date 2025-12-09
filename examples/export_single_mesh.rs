//! 导出单个 mesh 为 OBJ 格式
use aios_core::shape::pdms_shape::PlantMesh;
use std::fs::File;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: {} <mesh_id>", args[0]);
        println!("Example: {} 17293411045193599063", args[0]);
        return Ok(());
    }

    let mesh_id = &args[1];
    let mesh_path = format!("assets/meshes/lod_L1/{}_L1.mesh", mesh_id);
    let output_path = format!("test_output/{}_mesh.obj", mesh_id);

    println!("读取 mesh: {}", mesh_path);

    // 使用 PlantMesh::des_mesh_file 反序列化
    let mesh = PlantMesh::des_mesh_file(&mesh_path)?;

    println!("Mesh 顶点数: {}", mesh.vertices.len());
    println!("Mesh 三角形数: {}", mesh.indices.len() / 3);

    // 计算 AABB
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for v in &mesh.vertices {
        min[0] = min[0].min(v[0]);
        min[1] = min[1].min(v[1]);
        min[2] = min[2].min(v[2]);
        max[0] = max[0].max(v[0]);
        max[1] = max[1].max(v[1]);
        max[2] = max[2].max(v[2]);
    }
    println!("AABB: mins={:?}, maxs={:?}", min, max);

    // 导出为 OBJ
    let mut obj_file = File::create(&output_path)?;
    writeln!(obj_file, "# Mesh ID: {}", mesh_id)?;
    writeln!(obj_file, "# Vertices: {}", mesh.vertices.len())?;
    writeln!(obj_file, "# Triangles: {}", mesh.indices.len() / 3)?;

    // 写入顶点
    for v in &mesh.vertices {
        writeln!(obj_file, "v {} {} {}", v[0], v[1], v[2])?;
    }

    // 写入法线
    for n in &mesh.normals {
        writeln!(obj_file, "vn {} {} {}", n[0], n[1], n[2])?;
    }

    // 写入面
    for i in (0..mesh.indices.len()).step_by(3) {
        let i1 = mesh.indices[i] + 1;
        let i2 = mesh.indices[i + 1] + 1;
        let i3 = mesh.indices[i + 2] + 1;
        writeln!(obj_file, "f {}//{} {}//{} {}//{}", i1, i1, i2, i2, i3, i3)?;
    }

    println!("导出成功: {}", output_path);
    Ok(())
}
