use anyhow::Result;
use glam::Vec3;
use std::process::Command;
use uuid::Uuid;

use aios_database::xkt_generator::{XKTEntity, XKTFile, XKTGeometry, XKTMesh};

#[test]
fn small_cube_is_parseable_by_xeokit_sections() -> Result<()> {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!("cube_{}.xkt", Uuid::new_v4()));

    let mut xkt_file = XKTFile::new();

    let cube_geometry = XKTGeometry::create_box("cube_geo".into(), 1.0, 1.0, 1.0);
    xkt_file.model.create_geometry(cube_geometry)?;

    let mut cube_mesh = XKTMesh::new("cube_mesh".into(), "cube_geo".into());
    cube_mesh.set_position(Vec3::ZERO);
    xkt_file.model.create_mesh(cube_mesh)?;

    let mut cube_entity =
        XKTEntity::new("cube_entity".into(), "TestCube".into(), "IfcBlock".into());
    cube_entity.add_mesh("cube_mesh".into());
    xkt_file.model.create_entity(cube_entity)?;

    {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(xkt_file.save_to_file(file_path.to_string_lossy().as_ref(), true))?;
    }

    let output = Command::new("node")
        .arg("tests/scripts/verify_xkt_load.js")
        .arg(&file_path)
        .output()?;

    if !output.status.success() {
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("xeokit section validation failed");
    }

    Ok(())
}
