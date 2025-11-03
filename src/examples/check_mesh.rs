use aios_core::shape::pdms_shape::PlantMesh;

fn main() {
    let mesh = PlantMesh::des_mesh_file("assets/meshes/2.mesh").unwrap();
    println!("几何体 2:");
    println!("  vertices: {}", mesh.vertices.len());
    println!("  normals: {}", mesh.normals.len());
    println!("  indices: {}", mesh.indices.len());
    if let (Some(&min), Some(&max)) = (mesh.indices.iter().min(), mesh.indices.iter().max()) {
        println!("  索引范围: [{}..={}]", min, max);
    }
    println!("  前10个索引: {:?}", &mesh.indices[..mesh.indices.len().min(10)]);
}



