use std::alloc::{alloc, Layout};
use std::panic;
use aios_core::shape::pdms_shape::PlantMesh;
use bevy::render::render_resource::encase::private::RuntimeSizedArray;
use glam::{Mat4, Vec3};
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

    pub fn manifold_num_tri(&self) -> u32 {
        unsafe {
            let result = panic::catch_unwind(|| {
                // panic!("oh no!");
                manifold_num_tri(self.ptr) as _
            });
            if let Err(e) = result {
                panic::resume_unwind(e);
            }
            result.unwrap_or(0)
        }
    }

    pub fn manifold_get_properties(&self) -> ManifoldProperties {
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
}

impl From<(&PlantMesh, &Mat4)> for ManifoldRust {
    fn from(c: (&PlantMesh, &Mat4)) -> Self {
        let m = c.0;
        let t = c.1;
        unsafe {
            let mut verts = Vec::with_capacity(m.vertices.len() * 3);
            for v in m.vertices.clone() {
                let pt = t.transform_point3(Vec3::from(v));
                verts.push(pt[0]);
                verts.push(pt[1]);
                verts.push(pt[2]);
            }
            let mesh = ManifoldMeshRust::new();
            manifold_meshgl(mesh.ptr as _,
                            verts.as_ptr(), m.vertices.len(), 3,
                            m.indices.as_ptr(), m.indices.len() / 3);

            let manifold = Self::from_mesh(&mesh);
            manifold
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
            dbg!(len);

            let prop_num = manifold_meshgl_num_prop(mesh.ptr) as usize;
            dbg!(prop_num);
            let vert_num = manifold_meshgl_num_vert(mesh.ptr) as usize;
            dbg!(vert_num);
            let tri_num = manifold_meshgl_num_tri(mesh.ptr) as usize;
            dbg!(tri_num);

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

                let normal: [f32; 3] = ((b - a).cross(c - a)).into();

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
