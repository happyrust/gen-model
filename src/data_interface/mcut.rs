use std::fs::File;
use std::io::{BufWriter, Write};
use std::mem::size_of;
use aios_core::shape::pdms_shape::PlantMesh;
use glam::Vec3;
use mcut_sys::bindings::*;
use mcut_sys::bindings::McConnectedComponentData::*;
use mcut_sys::bindings::McConnectedComponentType::*;
use mcut_sys::bindings::McContextCreationFlags::*;
use mcut_sys::bindings::McDispatchFlags::*;
use mcut_sys::bindings::McFragmentLocation::*;
use mcut_sys::bindings::McPatchLocation::*;

pub fn batch_boolean_subtract(batch: &[PlantMesh]) -> Option<PlantMesh> {
    unsafe {
        if batch.len() == 0 { return None; }
        let first_mesh = batch[0].clone();
        if batch.len() == 1 { return Some(first_mesh); }
        let mut src_model = first_mesh;
        // let mut pos = batch[0].clone();
        // if batch.len() >= 2 {
        //     for (i, b) in batch[1..].iter().enumerate() {
        //         manifold_difference(result.ptr as _, pos.ptr, b.ptr);
        //         pos.ptr = result.ptr;
        //     }
        // }
        for (i, cut_model) in batch[1..].iter().enumerate() {
            // if i == 1 {
            //     break;
            // }
            let mut context = 0 as McContext;
            let err = mcCreateContext(&mut context, MC_DEBUG as u32);
            // mcDebugMessageControl(context, McDebugSource::MC_DEBUG_SOURCE_ALL,
            //                       McDebugType::MC_DEBUG_TYPE_ALL, McDebugSeverity::MC_DEBUG_SEVERITY_ALL, 1);
            let bool_op_flags = MC_DISPATCH_FILTER_FRAGMENT_SEALING_INSIDE as u32 | MC_DISPATCH_FILTER_FRAGMENT_LOCATION_BELOW as u32;
            //MC_DISPATCH_FILTER_FRAGMENT_SEALING_OUTSIDE | MC_DISPATCH_FILTER_FRAGMENT_LOCATION_ABOVE
            // let bool_op_flags = MC_DISPATCH_FILTER_FRAGMENT_SEALING_OUTSIDE as u32 | MC_DISPATCH_FILTER_FRAGMENT_LOCATION_ABOVE as u32;
            let src_pos_vec = &src_model.vertices;
            let cut_pos_vec = &cut_model.vertices;

            let src_face_cnt = src_model.indices.len() as u32 / 3;
            let cut_face_cnt = cut_model.indices.len() as u32 / 3;

            let src_face_sizes = (0..src_face_cnt).into_iter().map(|_| 3u32).collect::<Vec<_>>();
            let cut_face_sizes = (0..cut_face_cnt).into_iter().map(|_| 3u32).collect::<Vec<_>>();

            let src_indices = &src_model.indices;
            let cut_indices = &cut_model.indices;

            let err = mcDispatch(
                context,
                MC_DISPATCH_VERTEX_ARRAY_FLOAT as u32 | // vertices are in array of f32
                    MC_DISPATCH_ENFORCE_GENERAL_POSITION as u32 | // perturb if necessary
                    bool_op_flags, // filter flags which specify the type of output we want
                // source mesh
                src_pos_vec.as_ptr() as *const McVoid,
                src_indices.as_ptr() as *const u32,
                src_face_sizes.as_ptr(),
                src_pos_vec.len() as u32,
                src_face_cnt,
                // cut mesh
                cut_pos_vec.as_ptr() as *const McVoid,
                cut_indices.as_ptr() as *const u32,
                cut_face_sizes.as_ptr(),
                cut_pos_vec.len() as u32,
                cut_face_cnt,
            );
            dbg!(err);

            let mut num_conn_comps = 0;
            let err = mcGetConnectedComponents(context,
                                               MC_CONNECTED_COMPONENT_TYPE_FRAGMENT, 0, 0 as _, &mut num_conn_comps);

            println!("connected components: {}\n", num_conn_comps);

            let mut connected_components = Vec::new();
            connected_components.resize(num_conn_comps as usize, MC_NULL_HANDLE as McConnectedComponent);
            let err = mcGetConnectedComponents(context, MC_CONNECTED_COMPONENT_TYPE_FRAGMENT,
                                               connected_components.len() as u32, connected_components.as_mut_ptr(), 0 as _);

            dbg!(err);

            if connected_components.is_empty() {
                continue;
            }

            let mut conn_comp = connected_components[0];
            let mut num_bytes = 0;
            let err = mcGetConnectedComponentData(context, conn_comp,
                                                  MC_CONNECTED_COMPONENT_DATA_VERTEX_FLOAT as _, 0, 0 as _, &mut num_bytes);
            let cc_vertex_count = (num_bytes / (size_of::<f32>() as McSize * 3)) as u32;
            dbg!(cc_vertex_count);
            // std::vector<double> cc_vertices((uint64_t)cc_vertex_count * 3u, 0);
            let mut new_mesh = PlantMesh::default();
            // let mut cc_vertices = Vec::new();
            new_mesh.vertices.resize(cc_vertex_count as usize, Vec3::ZERO);
            let err = mcGetConnectedComponentData(context, conn_comp,
                                                  MC_CONNECTED_COMPONENT_DATA_VERTEX_FLOAT as _,
                                                  num_bytes, new_mesh.vertices.as_mut_ptr() as _, 0 as _);

            let mut num_bytes = 0;
            let err = mcGetConnectedComponentData(context, conn_comp, MC_CONNECTED_COMPONENT_DATA_FACE_TRIANGULATION as _
                                                  , 0, 0 as _, &mut num_bytes);
            dbg!(err);
            // std::vector<uint32_t> cc_face_indices(num_bytes / sizeof(uint32_t), 0);
            // let mut cc_face_indices = Vec::new();
            new_mesh.indices.resize((num_bytes as usize / size_of::<u32>()), 0);
            // dbg!(cc_face_indices.len());
            let err = mcGetConnectedComponentData(context, conn_comp,
                                                  MC_CONNECTED_COMPONENT_DATA_FACE_TRIANGULATION as _,
                                                  num_bytes, new_mesh.indices.as_mut_ptr() as _, 0 as _);

            let mut patch_location = McPatchLocation::MC_PATCH_LOCATION_ALL;

            let err = mcGetConnectedComponentData(context, conn_comp, MC_CONNECTED_COMPONENT_DATA_PATCH_LOCATION as _,
                                                  size_of::<McPatchLocation>() as McSize, &mut patch_location as *mut _ as _, 0 as _);

            dbg!(patch_location);
            let mut fragment_location = McFragmentLocation::MC_FRAGMENT_LOCATION_ALL;
            let err = mcGetConnectedComponentData(context, conn_comp, MC_CONNECTED_COMPONENT_DATA_FRAGMENT_LOCATION as _,
                                                  size_of::<McFragmentLocation>() as McSize, &mut fragment_location as *mut _ as _, 0 as _);

            dbg!(fragment_location);
            let reverse_winding_order = (fragment_location == MC_FRAGMENT_LOCATION_BELOW) && (patch_location == MC_PATCH_LOCATION_OUTSIDE);

            export_vertices(new_mesh.vertices.as_slice(),new_mesh.indices.as_slice(),
                            reverse_winding_order, "test.obj").expect("TODO: panic message");



            src_model = new_mesh;
            dbg!(src_model.indices.len());
            dbg!(src_model.vertices.len());
        }


        Some(src_model)
    }
}

fn export_vertices(vertex_data: &[Vec3], indices: &[u32], reverse: bool, file_path: &str) -> std::io::Result<()> {
    let mut buffer = BufWriter::new(File::create(file_path)?);
    buffer.write_all(b"# List of geometric vertices, with (x, y, z [,w]) coordinates, w is optional and defaults to 1.0.\n")?;
    for vd in vertex_data {
        buffer.write_all(
            format!(
                "v {:.3} {:.3} {:.3} 1.0\n",
                vd[0] as f32, vd[1] as f32, vd[2] as f32
            )
                .as_ref(),
        )?;
    }
    buffer.write_all(b"# Polygonal face element\n")?;
    for id in indices.chunks(3) {
        if reverse {
            buffer.write_all(
                format!(
                    "f {} {} {}\n",
                    id[2] + 1,
                    id[1] + 1,
                    id[0] + 1,
                ).as_ref(),
            )?;
        }else {
            buffer.write_all(
                format!(
                    "f {} {} {}\n",
                    id[0] + 1,
                    id[1] + 1,
                    id[2] + 1,
                ).as_ref(),
            )?;
        }

    }

    buffer.flush()?;
    Ok(())
}

