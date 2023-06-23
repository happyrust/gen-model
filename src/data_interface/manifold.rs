use std::alloc::{alloc, Layout};
use std::panic;
use aios_core::shape::pdms_shape::PlantMesh;

use glam::{Mat4, Vec3};
use itertools::Itertools;
use manifold_sys::bindings::*;

#[derive(Clone, Deref, DerefMut)]
pub struct ManifoldRust {
    pub ptr: *mut ManifoldManifold,
}

impl ManifoldRust {
    pub fn new() -> Self {
        unsafe {
            let sz = manifold_manifold_size();
            let layout = Layout::from_size_align(sz, 32).unwrap();
            let ptr = manifold_empty(alloc(layout) as _);
            Self {
                ptr,
            }
        }
    }

    pub fn from_mesh(m: &ManifoldMeshRust) -> Self {
        unsafe {
            let mut manifold = Self::new();
            let ptr = manifold_of_meshgl(manifold.ptr as _, m.ptr);
            manifold
        }
    }

    pub fn get_mesh(&self) -> ManifoldMeshRust{
        unsafe {
            let mesh = ManifoldMeshRust::new();
            manifold_get_meshgl(mesh.ptr as _, self.ptr);
            mesh
        }
    }

    pub fn num_tri(&self) -> u32 {
        unsafe {
            manifold_num_tri(self.ptr) as _
        }
    }

    pub fn get_properties(&self) -> ManifoldProperties {
        unsafe {
            manifold_get_properties(self.ptr)
        }
    }


    ///不支持subtact
    pub fn batch_boolean(batch: &[Self], op: ManifoldOpType) -> Self {
        unsafe {
            let sz = manifold_manifold_vec_size();
            let layout = Layout::from_size_align(sz, 32).unwrap();
            let m_vec = manifold_manifold_vec(alloc(layout) as _, batch.len());
            for b in batch {
                manifold_manifold_vec_push_back(m_vec, b.ptr);
            }
            let mut result = Self::new();
            // manifold_batch_boolean(result.ptr as _, m_vec, ManifoldOpType_MANIFOLD_SUBTRACT);
            manifold_batch_boolean(result.ptr as _, m_vec, op);
            result
        }
    }


    pub fn batch_boolean_subtract(batch: &[Self]) -> Self {
        unsafe {
            let mut result = Self::new();
            if batch.len() == 0 { return result; }
            if batch.len() == 1 { return batch[0].clone(); }
            let mut pos = batch[0].clone();
            if batch.len() >= 2 {
                for (i, b) in batch[1..].iter().enumerate() {
                    manifold_difference(result.ptr as _, pos.ptr, b.ptr);
                    pos.ptr = result.ptr;
                }
            }
            result
        }
    }




    pub fn destroy(&self) {
        unsafe {
            manifold_delete_manifold(self.ptr);
        }
    }
}

#[derive(Clone, Deref, DerefMut)]
pub struct ManifoldMeshRust {
    pub ptr: *mut ManifoldMeshGL,
}

impl ManifoldMeshRust {
    pub fn new() -> Self {
        unsafe {
            let sz = manifold_meshgl_size();
            let layout = Layout::from_size_align(sz, 32).unwrap();
            Self {
                ptr: alloc(layout) as _,
            }
        }
    }
    pub fn num_tri(&self) -> u32 {
        unsafe {
            manifold_meshgl_num_tri(self.ptr) as _
        }
    }

    pub fn merge(&mut self) -> bool{
        unsafe {
            manifold_meshgl_merge(self.ptr) != 0
        }
    }
    //
    // pub fn into_csg_mesh(&self) -> csg::Mesh {
    //     let mut triangles = Vec::new();
    //
    //     unsafe {
    //         let len = manifold_meshgl_tri_length(self.ptr as _);
    //         if len == 0 {
    //             return csg::Mesh::default();
    //         }
    //         let prop_num = manifold_meshgl_num_prop(self.ptr) as usize;
    //         // dbg!(prop_num);
    //         let vert_num = manifold_meshgl_num_vert(self.ptr) as usize;
    //         // dbg!(vert_num);
    //         let tri_num = manifold_meshgl_num_tri(self.ptr) as usize;
    //         // dbg!(tri_num);
    //
    //         let mut vertices: Vec<f32> = Vec::with_capacity(vert_num * prop_num);
    //         vertices.resize(vert_num * prop_num, 0.0);
    //         let mut indices: Vec<u32> = Vec::with_capacity(tri_num * 3);
    //         indices.resize(tri_num * 3, 0);
    //
    //         manifold_meshgl_vert_properties(vertices.as_mut_ptr() as _, self.ptr);
    //         manifold_meshgl_tri_verts(indices.as_mut_ptr() as _, self.ptr);
    //
    //         for c in indices.chunks(3) {
    //             let i = c[0] as usize;
    //             let j = c[1] as usize;
    //             let k = c[2] as usize;
    //             triangles.push(csg::Triangle {
    //                 a: csg::Pt3 { x: vertices[prop_num * i + 0] as f64, y: vertices[prop_num * i + 1] as f64, z: vertices[prop_num * i + 2] as f64 },
    //                 b: csg::Pt3 { x: vertices[prop_num * j + 0] as f64, y: vertices[prop_num * j + 1] as f64, z: vertices[prop_num * j + 2] as f64 },
    //                 c: csg::Pt3 { x: vertices[prop_num * k + 0] as f64, y: vertices[prop_num * k + 1] as f64, z: vertices[prop_num * k + 2] as f64 },
    //             })
    //
    //         }
    //
    //     }
    //
    //
    //     csg::Mesh::from_triangles(triangles)
    // }

    pub fn direct_to_plant_mesh(&self) -> PlantMesh {
        unsafe {
            let len = manifold_meshgl_tri_length(self.ptr as _);
            if len == 0 {
                return PlantMesh::default();
            }
            let prop_num = manifold_meshgl_num_prop(self.ptr) as usize;
            // dbg!(prop_num);
            let vert_num = manifold_meshgl_num_vert(self.ptr) as usize;
            // dbg!(vert_num);
            let tri_num = manifold_meshgl_num_tri(self.ptr) as usize;
            // dbg!(tri_num);

            let mut p: Vec<f32> = Vec::with_capacity(vert_num * prop_num);
            p.resize(vert_num * prop_num, 0.0);
            let mut indices: Vec<u32> = Vec::with_capacity(tri_num * 3);
            indices.resize(tri_num * 3, 0);

            let mut vertices = Vec::with_capacity(vert_num);
            manifold_meshgl_vert_properties(p.as_mut_ptr() as _, self.ptr);
            manifold_meshgl_tri_verts(indices.as_mut_ptr() as _, self.ptr);

            for i in 0..vert_num {
                vertices.push(Vec3::new(p[prop_num * i + 0], p[prop_num * i + 1], p[prop_num * i + 2]));
            }


            PlantMesh {
                indices,
                vertices,
                normals: vec![],
                wire_vertices: vec![],
            }
        }
    }
}

impl From<(&PlantMesh, &Mat4)> for ManifoldMeshRust {
    fn from(c: (&PlantMesh, &Mat4)) -> Self {
        let m = c.0;
        let t = c.1;
        unsafe {
            let mesh = ManifoldMeshRust::new();
            let mut verts = Vec::with_capacity(m.vertices.len() * 3);
            for v in m.vertices.clone() {
                let pt = t.transform_point3(Vec3::from(v));
                verts.push(pt[0]);
                verts.push(pt[1]);
                verts.push(pt[2]);
            }
            manifold_meshgl(mesh.ptr as _,
                            verts.as_ptr(), m.vertices.len(), 3,
                            m.indices.as_ptr(), m.indices.len() / 3);
            mesh
        }
    }
}

impl From<(&PlantMesh, &Mat4)> for ManifoldRust {
    fn from(m: (&PlantMesh, &Mat4)) -> Self {
        unsafe {
            let mesh: ManifoldMeshRust = m.into();
            Self::from_mesh(&mesh)
        }
    }
}

impl From<ManifoldRust> for PlantMesh {
    fn from(m: ManifoldRust) -> Self {
        unsafe {
            let mesh = ManifoldMeshRust::new();
            manifold_get_meshgl(mesh.ptr as _, m.ptr);
            let len = manifold_meshgl_tri_length(mesh.ptr as _);
            if len == 0 {
                return Self::default();
            }
            // dbg!(len);

            let prop_num = manifold_meshgl_num_prop(mesh.ptr) as usize;
            // dbg!(prop_num);
            let vert_num = manifold_meshgl_num_vert(mesh.ptr) as usize;
            // dbg!(vert_num);
            let tri_num = manifold_meshgl_num_tri(mesh.ptr) as usize;
            // dbg!(tri_num);

            let mut p: Vec<f32> = Vec::with_capacity(vert_num * prop_num);
            p.resize(vert_num * prop_num, 0.0);
            let mut old_indices: Vec<u32> = Vec::with_capacity(tri_num * 3);
            old_indices.resize(tri_num * 3, 0);

            let mut vert = Vec::with_capacity(vert_num);
            manifold_meshgl_vert_properties(p.as_mut_ptr() as _, mesh.ptr);
            manifold_meshgl_tri_verts(old_indices.as_mut_ptr() as _, mesh.ptr);

            for i in 0..vert_num {
                vert.push([p[prop_num * i + 0], p[prop_num * i + 1], p[prop_num * i + 2]]);
            }

            let index_num = tri_num * 3;
            let mut indices = Vec::with_capacity(index_num);
            let mut normals = Vec::with_capacity(index_num);
            let mut vertices = Vec::with_capacity(index_num);

            for (i, c) in old_indices.chunks(3).enumerate() {
                let a: Vec3 = Vec3::from(vert[c[0] as usize].clone());
                let b: Vec3 = Vec3::from(vert[c[1] as usize].clone());
                let c: Vec3 = Vec3::from(vert[c[2] as usize].clone());

                let normal = ((b - a).cross(c - a)).normalize();

                vertices.push(a.into());
                vertices.push(b.into());
                vertices.push(c.into());

                normals.push(normal);
                normals.push(normal);
                normals.push(normal);
                let i = i as u32;
                indices.push(i * 3 + 0);
                indices.push(i* 3 + 1);
                indices.push(i* 3 + 2);
            }

            Self {
                indices,
                vertices,
                normals,
                wire_vertices: vec![],
            }
        }
    }

}
