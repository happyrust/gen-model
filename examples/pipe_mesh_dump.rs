//! Read the production PlantMesh format for incremental geometry verification.
use aios_core::shape::pdms_shape::PlantMesh;

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    anyhow::ensure!(
        args.len() == 2,
        "usage: pipe_mesh_dump INPUT.mesh OUTPUT.json"
    );
    let path = std::path::PathBuf::from(&args[0]);
    let mesh = PlantMesh::des_mesh_file(&path)?;
    let data = serde_json::json!({
        "vertices": mesh.vertices.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
        "normals": mesh.normals.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
        "indices": mesh.indices,
    });
    std::fs::write(&args[1], serde_json::to_vec(&data)?)?;
    println!(
        "vertices={} indices={}",
        mesh.vertices.len(),
        mesh.indices.len()
    );
    Ok(())
}
