use anyhow::{Result, anyhow};
use byteorder::{LittleEndian, WriteBytesExt};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use glam::{Mat4, Vec3};
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use tokio::fs::File;

use super::{XKTEntity, XKTFile, XKTGeometry, XKTGeometryType, XKTMesh};

const MAX_QUANT_VALUE: f32 = 65535.0;
const SECTION_ORDER: usize = 29;

pub struct XKTProperWriter {
    compression_level: u32,
}

impl XKTProperWriter {
    pub fn new() -> Self {
        Self {
            compression_level: 6,
        }
    }

    pub fn with_compression_level(mut self, level: u32) -> Self {
        self.compression_level = level.min(9);
        self
    }

    pub async fn write_to_file(
        &self,
        xkt_file: &XKTFile,
        path: &str,
        compress: bool,
    ) -> Result<()> {
        let bytes = self.write_to_bytes(xkt_file, compress)?;
        let mut file = File::create(path).await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &bytes).await?;
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        println!("XKT v10 file written to: {}", path);
        println!("File size: {} bytes", bytes.len());
        Ok(())
    }

    pub fn write_to_bytes(&self, xkt_file: &XKTFile, compress: bool) -> Result<Vec<u8>> {
        let sections = self.build_sections(xkt_file, compress)?;
        let mut buffer = Vec::new();
        buffer.write_u32::<LittleEndian>(10)?; // XKT v10

        let mut offsets = Vec::with_capacity(sections.len());
        let mut offset = 4 + (sections.len() * 4) as u32;
        for section in &sections {
            offsets.push(offset);
            offset += section.len() as u32;
        }

        for off in offsets {
            buffer.write_u32::<LittleEndian>(off)?;
        }

        for section in sections {
            buffer.extend_from_slice(&section);
        }

        Ok(buffer)
    }

    fn build_sections(&self, xkt_file: &XKTFile, compress: bool) -> Result<Vec<Vec<u8>>> {
        let model = &xkt_file.model;
        let geometries = &model.geometries_list;
        let meshes = &model.meshes_list;
        let entities = &model.entities_list;

        if geometries.is_empty() {
            return Err(anyhow!("模型中没有几何体"));
        }

        let geometry_index_map: HashMap<_, _> = geometries
            .iter()
            .enumerate()
            .map(|(idx, geom)| (geom.id.clone(), idx))
            .collect();

        let mesh_map: HashMap<_, _> = meshes.iter().map(|mesh| (mesh.id.clone(), mesh)).collect();

        let mut global_min = Vec3::splat(f32::INFINITY);
        let mut global_max = Vec3::splat(f32::NEG_INFINITY);
        for geometry in geometries {
            for chunk in geometry.positions.chunks(3) {
                if chunk.len() == 3 {
                    let p = Vec3::new(chunk[0], chunk[1], chunk[2]);
                    global_min = global_min.min(p);
                    global_max = global_max.max(p);
                }
            }
        }

        if !global_min.min_element().is_finite() || !global_max.max_element().is_finite() {
            global_min = Vec3::new(-1.0, -1.0, -1.0);
            global_max = Vec3::new(1.0, 1.0, 1.0);
        }

        let decode_matrix = create_decode_matrix(global_min, global_max);

        let mut positions = Vec::<u16>::new();
        let mut normals = Vec::<i8>::new();
        let mut colors = Vec::<u8>::new();
        let mut uvs = Vec::<f32>::new();
        let mut indices = Vec::<u32>::new();
        let mut edge_indices = Vec::<u32>::new();
        let mut geometry_positions_portion = Vec::<u32>::with_capacity(geometries.len());
        let mut geometry_normals_portion = Vec::<u32>::with_capacity(geometries.len());
        let mut geometry_colors_portion = Vec::<u32>::with_capacity(geometries.len());
        let mut geometry_uvs_portion = Vec::<u32>::with_capacity(geometries.len());
        let mut geometry_indices_portion = Vec::<u32>::with_capacity(geometries.len());
        let mut geometry_edge_portion = Vec::<u32>::with_capacity(geometries.len());
        let mut geometry_axis_labels = Vec::<String>::with_capacity(geometries.len());
        let mut geometry_primitive_types = Vec::<u8>::with_capacity(geometries.len());

        let mut position_offset = 0u32;
        let mut normal_offset = 0u32;
        let mut color_offset = 0u32;
        let mut uv_offset = 0u32;
        let mut index_offset = 0u32;
        let mut edge_offset = 0u32;

        for geometry in geometries {
            geometry_positions_portion.push(position_offset);
            let quantized_positions =
                quantize_positions(&geometry.positions, global_min, global_max);
            position_offset += quantized_positions.len() as u32;
            positions.extend_from_slice(&quantized_positions);

            geometry_normals_portion.push(normal_offset);
            if let Some(ref normals_vec) = geometry.normals {
                for chunk in normals_vec.chunks(3) {
                    if chunk.len() == 3 {
                        let (x, y) = oct_encode(chunk[0], chunk[1], chunk[2]);
                        normals.push(x);
                        normals.push(y);
                        normals.push(0);
                        normal_offset += 3;
                    }
                }
            }

            geometry_colors_portion.push(color_offset);
            if let Some(ref colors_vec) = geometry.colors {
                for chunk in colors_vec.chunks(3) {
                    if chunk.len() == 3 {
                        colors.push((chunk[0] * 255.0) as u8);
                        colors.push((chunk[1] * 255.0) as u8);
                        colors.push((chunk[2] * 255.0) as u8);
                        colors.push(255u8);
                        color_offset += 4;
                    }
                }
            }

            geometry_uvs_portion.push(uv_offset);
            if let Some(ref uv_vec) = geometry.uv {
                uvs.extend_from_slice(uv_vec);
                uv_offset += uv_vec.len() as u32;
            }

            geometry_indices_portion.push(index_offset);
            indices.extend_from_slice(&geometry.indices);
            index_offset += geometry.indices.len() as u32;

            geometry_edge_portion.push(edge_offset);
            let edges = build_edge_indices(&geometry.indices);
            edge_indices.extend_from_slice(&edges);
            edge_offset += edges.len() as u32;

            geometry_axis_labels.push(geometry.axis_label.clone().unwrap_or_default());
            geometry_primitive_types.push(map_primitive_type(geometry));
        }

        let mut matrices = Vec::<f32>::new();
        let mut mesh_geometries_portion = Vec::<u32>::new();
        let mut mesh_matrices_portion = Vec::<u32>::new();
        let mut mesh_texture_set = Vec::<i32>::new();
        let mut mesh_material_attributes = Vec::<u8>::new();

        let mut entity_ids = Vec::<String>::new();
        let mut entity_mesh_portion = Vec::<u32>::new();
        let mut mesh_counts_per_geometry = vec![0usize; geometries.len()];

        for mesh in meshes {
            if let Some(&geometry_idx) = geometry_index_map.get(&mesh.geometry_id) {
                mesh_counts_per_geometry[geometry_idx] += 1;
            }
        }

        let mut matrix_offset = 0u32;
        for entity in entities {
            entity_ids.push(entity.id.clone());
            entity_mesh_portion.push(mesh_geometries_portion.len() as u32);

            for mesh_id in &entity.mesh_ids {
                let mesh = mesh_map
                    .get(mesh_id)
                    .ok_or_else(|| anyhow!("无法找到网格 {}", mesh_id))?;
                let geometry_idx = *geometry_index_map
                    .get(&mesh.geometry_id)
                    .ok_or_else(|| anyhow!("无法找到几何体 {}", mesh.geometry_id))?;

                mesh_geometries_portion.push(geometry_idx as u32);

                if mesh_counts_per_geometry[geometry_idx] > 1 {
                    let matrix = mesh.matrix.unwrap_or_else(|| compute_mesh_matrix(mesh));
                    matrices.extend_from_slice(&matrix);
                    mesh_matrices_portion.push(matrix_offset);
                    matrix_offset += 16;
                } else {
                    mesh_matrices_portion.push(0);
                }

                mesh_texture_set.push(-1);

                let color = mesh.color;
                mesh_material_attributes.push((color.x.clamp(0.0, 1.0) * 255.0) as u8);
                mesh_material_attributes.push((color.y.clamp(0.0, 1.0) * 255.0) as u8);
                mesh_material_attributes.push((color.z.clamp(0.0, 1.0) * 255.0) as u8);
                mesh_material_attributes.push((mesh.opacity.clamp(0.0, 1.0) * 255.0) as u8);
                mesh_material_attributes.push((mesh.metallic.clamp(0.0, 1.0) * 255.0) as u8);
                mesh_material_attributes.push((mesh.roughness.clamp(0.0, 1.0) * 255.0) as u8);
            }
        }

        let tile_aabb = compute_tile_aabb(global_min, global_max);
        let tile_entities_portion = if entity_ids.is_empty() {
            vec![]
        } else {
            vec![0u32]
        };

        let metadata = json!({
            "id": model.id,
            "projectId": "project",
            "revisionId": "1",
            "author": model.metadata.author,
            "createdAt": model.metadata.created,
            "creatingApplication": model.metadata.application,
            "schema": model.metadata.schema,
            "propertySets": [],
            "metaObjects": entities.iter().map(|entity| json!({
                "id": entity.id,
                "type": entity.entity_type,
                "name": entity.name,
            })).collect::<Vec<_>>()
        });

        let mut sections = Vec::with_capacity(SECTION_ORDER);
        sections.push(self.compress_buffer(&serde_json::to_vec(&metadata)?, compress)?);
        sections.push(self.compress_buffer(&[], compress)?); // textureData
        sections.push(self.compress_buffer(&[], compress)?); // eachTextureDataPortion
        sections.push(self.compress_buffer(&[], compress)?); // eachTextureAttributes
        sections.push(self.compress_buffer(&u16_to_le_bytes(&positions), compress)?);
        sections.push(self.compress_buffer(&i8_to_le_bytes(&normals), compress)?);
        sections.push(self.compress_buffer(&colors, compress)?);
        sections.push(self.compress_buffer(&f32_to_le_bytes(&uvs), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&indices), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&edge_indices), compress)?);
        sections.push(self.compress_buffer(&[], compress)?); // eachTextureSetTextures
        sections.push(self.compress_buffer(&f32_to_le_bytes(&matrices), compress)?);
        sections.push(self.compress_buffer(&f32_to_le_bytes(&decode_matrix), compress)?);
        sections.push(self.compress_buffer(&geometry_primitive_types, compress)?);
        sections.push(self.compress_buffer(&serde_json::to_vec(&geometry_axis_labels)?, compress)?);
        sections
            .push(self.compress_buffer(&u32_to_le_bytes(&geometry_positions_portion), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&geometry_normals_portion), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&geometry_colors_portion), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&geometry_uvs_portion), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&geometry_indices_portion), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&geometry_edge_portion), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&mesh_geometries_portion), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&mesh_matrices_portion), compress)?);
        sections.push(self.compress_buffer(&i32_to_le_bytes(&mesh_texture_set), compress)?);
        sections.push(self.compress_buffer(&mesh_material_attributes, compress)?);
        sections.push(self.compress_buffer(&serde_json::to_vec(&entity_ids)?, compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&entity_mesh_portion), compress)?);
        sections.push(self.compress_buffer(&f64_to_le_bytes(&tile_aabb), compress)?);
        sections.push(self.compress_buffer(&u32_to_le_bytes(&tile_entities_portion), compress)?);

        while sections.len() < SECTION_ORDER {
            sections.push(self.compress_buffer(&[], compress)?);
        }

        Ok(sections)
    }

    fn compress_buffer(&self, buffer: &[u8], compress: bool) -> Result<Vec<u8>> {
        if !compress {
            return Ok(buffer.to_vec());
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(self.compression_level));
        encoder.write_all(buffer)?;
        Ok(encoder.finish()?)
    }
}

impl Default for XKTProperWriter {
    fn default() -> Self {
        Self::new()
    }
}

fn quantize_positions(positions: &[f32], min: Vec3, max: Vec3) -> Vec<u16> {
    let mut quantized = Vec::with_capacity(positions.len());
    let range = (max - min).max(Vec3::splat(1e-6));

    for chunk in positions.chunks(3) {
        if chunk.len() == 3 {
            let pos = Vec3::new(chunk[0], chunk[1], chunk[2]);
            let local = (pos - min) / range;
            quantized.push((local.x.clamp(0.0, 1.0) * MAX_QUANT_VALUE).round() as u16);
            quantized.push((local.y.clamp(0.0, 1.0) * MAX_QUANT_VALUE).round() as u16);
            quantized.push((local.z.clamp(0.0, 1.0) * MAX_QUANT_VALUE).round() as u16);
        }
    }

    quantized
}

fn create_decode_matrix(min: Vec3, max: Vec3) -> Vec<f32> {
    let range = (max - min).max(Vec3::splat(1e-6)) / MAX_QUANT_VALUE;
    let scale = Mat4::from_scale(range);
    let translation = Mat4::from_translation(min);
    (translation * scale).to_cols_array().to_vec()
}

fn oct_encode(x: f32, y: f32, z: f32) -> (i8, i8) {
    let mut v = Vec3::new(x, y, z);
    if v.length_squared() == 0.0 {
        v = Vec3::Z;
    }
    v = v.normalize();
    let l = v.x.abs() + v.y.abs() + v.z.abs();
    let mut projection = Vec3::new(v.x / l, v.y / l, v.z / l);
    if projection.z < 0.0 {
        projection.x = (1.0 - projection.y.abs()) * projection.x.signum();
        projection.y = (1.0 - projection.x.abs()) * projection.y.signum();
    }
    let oct_x = (projection.x * 127.0).round().clamp(-128.0, 127.0) as i8;
    let oct_y = (projection.y * 127.0).round().clamp(-128.0, 127.0) as i8;
    (oct_x, oct_y)
}

fn build_edge_indices(indices: &[u32]) -> Vec<u32> {
    use std::collections::BTreeSet;
    let mut set = BTreeSet::new();
    for tri in indices.chunks(3) {
        if tri.len() == 3 {
            let a = tri[0];
            let b = tri[1];
            let c = tri[2];
            set.insert((a.min(b), a.max(b)));
            set.insert((b.min(c), b.max(c)));
            set.insert((c.min(a), c.max(a)));
        }
    }
    let mut edges = Vec::with_capacity(set.len() * 2);
    for (a, b) in set {
        edges.push(a);
        edges.push(b);
    }
    edges
}

fn map_primitive_type(geometry: &XKTGeometry) -> u8 {
    match geometry.geometry_type {
        XKTGeometryType::Triangles => 1,
        XKTGeometryType::Lines => 3,
        XKTGeometryType::Points => 2,
        XKTGeometryType::LineStrip => 4,
        XKTGeometryType::LineLoop => 4,
        XKTGeometryType::TriangleStrip => 5,
        XKTGeometryType::TriangleFan => 6,
        XKTGeometryType::AxisLabel => 7,
    }
}

fn compute_mesh_matrix(mesh: &XKTMesh) -> [f32; 16] {
    if let Some(matrix) = mesh.matrix {
        matrix
    } else {
        let translation = Mat4::from_translation(mesh.position);
        let rotation = Mat4::from_euler(
            glam::EulerRot::XYZ,
            mesh.rotation.x,
            mesh.rotation.y,
            mesh.rotation.z,
        );
        let scale = Mat4::from_scale(mesh.scale);
        (translation * rotation * scale).to_cols_array()
    }
}

fn compute_tile_aabb(min: Vec3, max: Vec3) -> Vec<f64> {
    vec![
        min.x as f64,
        min.y as f64,
        min.z as f64,
        max.x as f64,
        max.y as f64,
        max.z as f64,
    ]
}

fn u16_to_le_bytes(data: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2);
    for value in data {
        out.write_u16::<LittleEndian>(*value).unwrap();
    }
    out
}

fn u32_to_le_bytes(data: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4);
    for value in data {
        out.write_u32::<LittleEndian>(*value).unwrap();
    }
    out
}

fn i32_to_le_bytes(data: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4);
    for value in data {
        out.write_i32::<LittleEndian>(*value).unwrap();
    }
    out
}

fn f32_to_le_bytes(data: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4);
    for value in data {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn f64_to_le_bytes(data: &[f64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 8);
    for value in data {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn i8_to_le_bytes(data: &[i8]) -> Vec<u8> {
    data.iter().map(|v| *v as u8).collect()
}
