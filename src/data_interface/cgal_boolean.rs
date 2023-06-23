use std::ptr::null;
use aios_core::shape::pdms_shape::PlantMesh;
use dashmap::DashMap;
use glam::Vec3;
use kigumi_sys::bindings::{boolean_mesh, free_mesh, KMesh};
use rayon::prelude::*;
use rayon::iter::ParallelIterator;

pub fn convert_to_kmesh(m: &PlantMesh) -> KMesh {
    KMesh {
        verices: m.vertices.as_ptr() as _,
        n_verts: m.vertices.len(),
        tri_verts: m.indices.as_ptr() as _,
        n_tris: m.indices.len() / 3,
    }
}

pub fn convert_to_plant_mesh(m: &KMesh) -> PlantMesh {
    if m.verices.is_null() || m.tri_verts.is_null() {
        return PlantMesh::default();
    }
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
        let mut final_mesh = convert_to_kmesh(&batch[0]);

        let new_batch = &batch[1..];
        let mut neg_meshes_map = DashMap::new();

        // batch[1].export_obj(false, "0.obj").unwrap();
        if new_batch.len() >= 1 {
            const batch_union_len: usize = 100;
            let batch_len = new_batch.len() / batch_union_len + 1;
            // (0..batch_len).into_par_iter().for_each(|x|{
            (0..batch_len).into_iter().for_each(|x| {
                let offset = x * batch_union_len;
                let mut neg_mesh = convert_to_kmesh(&new_batch[offset]);
                for i in (offset+1)..(batch_union_len + offset) {
                    if i >= new_batch.len() {
                        break;
                    }
                    let o = &new_batch[i];
                    let mut second_mesh = convert_to_kmesh(o);
                    let tmp = boolean_mesh(&mut neg_mesh, &mut second_mesh, 0);
                    if (*tmp).verices.is_null() {
                        dbg!(i);
                    }
                    // 第一个mesh不需要释放，后面发生计算了的，需要free
                    if i != 0 {
                        // free_mesh(&mut neg_mesh);
                    }
                    neg_mesh = *tmp;
                }
                let plant_mesh = convert_to_plant_mesh(&neg_mesh);
                // let new = plant_mesh.merge_without_normal(true).unwrap();
                plant_mesh.export_obj(false, &format!("{}.obj", x)).unwrap();
                neg_meshes_map.insert(x, plant_mesh);
            });

            for (i, mut neg) in neg_meshes_map.iter().enumerate() {
                let mut neg_mesh = convert_to_kmesh(neg.value());
                let tmp = boolean_mesh(&mut final_mesh, &mut neg_mesh, 3);
                // if i != 0 {
                //     free_mesh(&mut final_mesh);
                // }
                final_mesh = *tmp;
                break;
            }
        }
        let result = convert_to_plant_mesh(&final_mesh);
        result.export_obj(false, "result.obj").unwrap();
        result
    }
}
