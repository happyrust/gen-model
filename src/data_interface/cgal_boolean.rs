use aios_core::shape::pdms_shape::PlantMesh;
use glam::Vec3;
use kigumi_sys::bindings::{boolean_mesh, free_mesh, KMesh};

pub fn convert_to_kmesh(m: &PlantMesh) -> KMesh {
    KMesh {
        verices: m.vertices.as_ptr() as _,
        n_verts: m.vertices.len(),
        tri_verts: m.indices.as_ptr() as _,
        n_tris: m.indices.len() / 3,
    }
}

pub fn convert_to_plant_mesh(m: &KMesh) -> PlantMesh {
    let mut mesh = PlantMesh {
        vertices: unsafe { std::slice::from_raw_parts(m.verices as *const _, m.n_verts).to_vec() },
        normals: vec![],
        indices: unsafe {
            std::slice::from_raw_parts(m.tri_verts as *const _, m.n_tris * 3).to_vec()
        },
        wire_vertices: vec![],
    };
    mesh.cal_normals();
    mesh
}

pub fn batch_boolean_subtract(batch: &[PlantMesh]) -> PlantMesh {
    unsafe {
        if batch.len() == 0 {
            return PlantMesh::default();
        }
        if batch.len() == 1 {
            return batch[0].clone();
        }
        let mut pos_mesh = convert_to_kmesh(&batch[0]);
        let mut neg_mesh = convert_to_kmesh(&batch[1]);
        // batch[1].export_obj(false, "0.obj").unwrap();
        if batch.len() >= 2 {
            for (i, b) in batch[2..].iter().enumerate() {
                // b.export_obj(false, &format!("{}.obj", i + 1)).unwrap();
                dbg!(i);
                if i >= 50 {
                    break;
                }
                // let c = convert_to_plant_mesh(&final_mesh);
                // let c = convert_to_plant_mesh(&final_mesh);
                // dbg!(c.vertices.len());
                // // let merged = c.merge_without_normal(true).unwrap();
                // dbg!(merged.vertices.len());
                // let mut src_mesh = convert_to_kmesh(&final_mesh);

                // let second_merge = b.merge_without_normal(true).unwrap();
                let mut second_mesh = convert_to_kmesh(b);
                neg_mesh = *boolean_mesh(&mut neg_mesh, &mut second_mesh, 0);
                // 第一个mesh不需要释放，后面发生计算了的，需要free
                if i != 0 {
                    // free_mesh(&mut src_mesh);
                }
            }
        }
        let final_mesh = *boolean_mesh(&mut pos_mesh, &mut neg_mesh, 3);
        let result = convert_to_plant_mesh(&final_mesh);
        result.export_obj(false, "result.obj").unwrap();
        result
    }
}
