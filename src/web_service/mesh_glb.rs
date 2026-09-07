//! `.mesh`（rkyv 归档的 [`PlantMesh`]）→ GLB（glTF 2.0 二进制）的现场转换。
//!
//! 浏览器端（plant3d-web）拿不动 rkyv 归档——那套布局与 rkyv 0.7 / glam 的内存形状
//! 强耦合，任何一边升级前端就静默读歪。所以格式转换放在服务端这一侧，前端复用它
//! 已有的 GLB 解析（`parseGlbGeometry`：只读 `meshes[0].primitives[0]` 的
//! `POSITION` / `NORMAL` / `indices`）。`geo_hash` 是内容寻址键，同一个 hash 的 GLB
//! 永不改变，HTTP 层因此可以给 `immutable` 缓存。
//!
//! 只做**一个三角面 primitive**：`wire_vertices`（硬边折线）不进 GLB——前端现有解析
//! 不读第二个 primitive，塞进去只会白涨体积；要边线时另开 primitive，不改这一个。

use aios_core::shape::pdms_shape::PlantMesh;
use serde_json::json;

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"
const COMPONENT_F32: u32 = 5126;
const COMPONENT_U32: u32 = 5125;
const TARGET_ARRAY_BUFFER: u32 = 34962;
const TARGET_ELEMENT_ARRAY_BUFFER: u32 = 34963;
const MODE_TRIANGLES: u32 = 4;

/// 把一份 [`PlantMesh`] 编成一个自包含的 GLB 字节串。
///
/// 拒绝的形状（返回 `Err`，不吐半个文件）：没有顶点、索引数不是 3 的倍数、索引越界。
/// 这些都是数据本身的终局，重发同一个 hash 还是同一个结果，调用方该按 422 而不是
/// 500 对外说话。法向量数量与顶点数不等时**省掉** `NORMAL`（前端会自己算或走平面着色），
/// 不拿错位的法向量凑数。
pub fn plant_mesh_to_glb(mesh: &PlantMesh) -> anyhow::Result<Vec<u8>> {
    let vertex_count = mesh.vertices.len();
    anyhow::ensure!(vertex_count > 0, "mesh 没有顶点，无法编成 GLB");
    anyhow::ensure!(
        !mesh.indices.is_empty() && mesh.indices.len() % 3 == 0,
        "mesh 索引数 {} 不是 3 的倍数（或为空），不是三角网格",
        mesh.indices.len()
    );
    if let Some(bad) = mesh
        .indices
        .iter()
        .find(|index| **index as usize >= vertex_count)
    {
        anyhow::bail!("mesh 索引 {bad} 越界（顶点数 {vertex_count}）");
    }
    let with_normals = mesh.normals.len() == vertex_count;

    // BIN：positions | normals? | indices，每段起点天然 4 字节对齐（12 字节 / 4 字节元素）。
    let positions_len = vertex_count * 12;
    let normals_len = if with_normals { vertex_count * 12 } else { 0 };
    let indices_len = mesh.indices.len() * 4;
    let mut bin = Vec::with_capacity(positions_len + normals_len + indices_len);
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for vertex in &mesh.vertices {
        let components = [vertex.x, vertex.y, vertex.z];
        for (axis, value) in components.iter().enumerate() {
            min[axis] = min[axis].min(*value);
            max[axis] = max[axis].max(*value);
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    if with_normals {
        for normal in &mesh.normals {
            for value in [normal.x, normal.y, normal.z] {
                bin.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    for index in &mesh.indices {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    debug_assert_eq!(bin.len(), positions_len + normals_len + indices_len);

    let mut buffer_views = vec![json!({
        "buffer": 0,
        "byteOffset": 0,
        "byteLength": positions_len,
        "target": TARGET_ARRAY_BUFFER,
    })];
    let mut accessors = vec![json!({
        "bufferView": 0,
        "componentType": COMPONENT_F32,
        "count": vertex_count,
        "type": "VEC3",
        "min": min,
        "max": max,
    })];
    let mut attributes = json!({ "POSITION": 0 });
    if with_normals {
        buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": positions_len,
            "byteLength": normals_len,
            "target": TARGET_ARRAY_BUFFER,
        }));
        accessors.push(json!({
            "bufferView": 1,
            "componentType": COMPONENT_F32,
            "count": vertex_count,
            "type": "VEC3",
        }));
        attributes["NORMAL"] = json!(1);
    }
    let indices_view = buffer_views.len();
    buffer_views.push(json!({
        "buffer": 0,
        "byteOffset": positions_len + normals_len,
        "byteLength": indices_len,
        "target": TARGET_ELEMENT_ARRAY_BUFFER,
    }));
    let indices_accessor = accessors.len();
    accessors.push(json!({
        "bufferView": indices_view,
        "componentType": COMPONENT_U32,
        "count": mesh.indices.len(),
        "type": "SCALAR",
    }));

    let document = json!({
        "asset": { "version": "2.0", "generator": "gen-model plant-mesh→glb" },
        "buffers": [{ "byteLength": bin.len() }],
        "bufferViews": buffer_views,
        "accessors": accessors,
        "meshes": [{
            "primitives": [{
                "attributes": attributes,
                "indices": indices_accessor,
                "mode": MODE_TRIANGLES,
            }]
        }],
        "nodes": [{ "mesh": 0 }],
        "scenes": [{ "nodes": [0] }],
        "scene": 0,
    });
    let mut json_chunk = serde_json::to_vec(&document)?;
    pad_to_4(&mut json_chunk, b' ');
    pad_to_4(&mut bin, 0);

    let total = 12 + 8 + json_chunk.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json_chunk);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&bin);
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// `geo_hash` 合法字符集：内容寻址键只由数字、字母与下划线组成。
///
/// 这道门同时挡目录穿越——路径拼接前只放行这一个字符集，`..`、`/`、`\` 连门都进不了。
pub fn is_valid_geo_hash(stem: &str) -> bool {
    !stem.is_empty()
        && stem.len() <= 128
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn pad_to_4(bytes: &mut Vec<u8>, filler: u8) {
    while bytes.len() % 4 != 0 {
        bytes.push(filler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn triangle() -> PlantMesh {
        PlantMesh {
            indices: vec![0, 1, 2],
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(0.0, 5.0, 2.5),
            ],
            normals: vec![Vec3::Z, Vec3::Z, Vec3::Z],
            wire_vertices: Vec::new(),
            aabb: None,
        }
    }

    /// 解出 GLB 的 JSON 文档与 BIN 段，形状不对就 panic——这是下面每条测试的地基。
    fn split(glb: &[u8]) -> (serde_json::Value, Vec<u8>) {
        let u32_at = |offset: usize| {
            u32::from_le_bytes(glb[offset..offset + 4].try_into().expect("4 bytes"))
        };
        assert_eq!(u32_at(0), GLB_MAGIC, "magic");
        assert_eq!(u32_at(4), GLB_VERSION, "version");
        assert_eq!(u32_at(8) as usize, glb.len(), "总长度必须等于文件长度");
        let json_len = u32_at(12) as usize;
        assert_eq!(u32_at(16), CHUNK_JSON);
        assert_eq!(json_len % 4, 0, "JSON chunk 必须 4 字节对齐");
        let json_bytes = &glb[20..20 + json_len];
        let document: serde_json::Value =
            serde_json::from_slice(json_bytes).expect("JSON chunk 必须是合法 JSON（空格填充）");
        let bin_offset = 20 + json_len;
        let bin_len = u32_at(bin_offset) as usize;
        assert_eq!(u32_at(bin_offset + 4), CHUNK_BIN);
        assert_eq!(bin_len % 4, 0, "BIN chunk 必须 4 字节对齐");
        let bin = glb[bin_offset + 8..bin_offset + 8 + bin_len].to_vec();
        assert_eq!(
            bin_offset + 8 + bin_len,
            glb.len(),
            "BIN 之后不能有多余字节"
        );
        (document, bin)
    }

    #[test]
    fn a_triangle_becomes_one_indexed_primitive_with_positions_and_normals() {
        let glb = plant_mesh_to_glb(&triangle()).expect("三角形必须能编");
        let (document, bin) = split(&glb);

        let primitive = &document["meshes"][0]["primitives"][0];
        assert_eq!(primitive["mode"], MODE_TRIANGLES);
        assert_eq!(primitive["attributes"]["POSITION"], 0);
        assert_eq!(primitive["attributes"]["NORMAL"], 1);
        assert_eq!(primitive["indices"], 2);

        let accessors = document["accessors"].as_array().expect("accessors");
        assert_eq!(accessors[0]["count"], 3);
        assert_eq!(accessors[0]["componentType"], COMPONENT_F32);
        assert_eq!(accessors[0]["min"], json!([0.0, 0.0, 0.0]));
        assert_eq!(accessors[0]["max"], json!([10.0, 5.0, 2.5]));
        assert_eq!(accessors[2]["count"], 3);
        assert_eq!(accessors[2]["componentType"], COMPONENT_U32);
        assert_eq!(accessors[2]["type"], "SCALAR");

        // BIN 段：3 顶点 ×12 + 3 法向 ×12 + 3 索引 ×4 = 84 字节，已经 4 对齐。
        assert_eq!(document["buffers"][0]["byteLength"], 84);
        assert_eq!(bin.len(), 84);
        let second_vertex_x = f32::from_le_bytes(bin[12..16].try_into().unwrap());
        assert_eq!(second_vertex_x, 10.0);
        let last_index = u32::from_le_bytes(bin[80..84].try_into().unwrap());
        assert_eq!(last_index, 2);
    }

    /// 法向量数量对不上就整段省掉：错位的法向量比没有法向量更坏——它会把光照画歪
    /// 而没有任何东西报错。
    #[test]
    fn mismatched_normals_are_dropped_instead_of_misaligned() {
        let mut mesh = triangle();
        mesh.normals.pop();
        let glb = plant_mesh_to_glb(&mesh).expect("缺法向量仍可编");
        let (document, bin) = split(&glb);
        let attributes = &document["meshes"][0]["primitives"][0]["attributes"];
        assert!(
            attributes.get("NORMAL").is_none(),
            "法向量数量不等时不得写 NORMAL"
        );
        assert_eq!(document["accessors"].as_array().unwrap().len(), 2);
        assert_eq!(bin.len(), 3 * 12 + 3 * 4);
    }

    #[test]
    fn json_chunk_padding_keeps_the_document_parseable_and_bin_offset_aligned() {
        // 顶点数变化会让 JSON 长度在 4 的余数上打转；对几种长度都要求解得开。
        for extra in 0..7 {
            let mut mesh = triangle();
            for i in 0..extra {
                mesh.vertices.push(Vec3::new(i as f32, 0.0, 0.0));
                mesh.normals.push(Vec3::Z);
            }
            let glb = plant_mesh_to_glb(&mesh).expect("必须能编");
            let (document, _) = split(&glb);
            assert_eq!(document["accessors"][0]["count"], 3 + extra);
        }
    }

    #[test]
    fn degenerate_meshes_are_rejected_before_any_bytes_are_written() {
        let mut empty = triangle();
        empty.vertices.clear();
        assert!(plant_mesh_to_glb(&empty).is_err(), "没有顶点");

        let mut ragged = triangle();
        ragged.indices.push(0);
        assert!(plant_mesh_to_glb(&ragged).is_err(), "索引数不是 3 的倍数");

        let mut out_of_range = triangle();
        out_of_range.indices[2] = 7;
        assert!(plant_mesh_to_glb(&out_of_range).is_err(), "索引越界");
    }

    #[test]
    fn geo_hash_gate_admits_content_keys_and_rejects_path_tricks() {
        for ok in [
            "1",
            "10000256467819498479",
            "e3d_baked_v2_deadbeef",
            "AbC_09",
        ] {
            assert!(is_valid_geo_hash(ok), "{ok}");
        }
        for bad in [
            "",
            "..",
            "../1",
            "a/b",
            "a\\b",
            "1.mesh",
            "x y",
            "中文",
            &"a".repeat(129),
        ] {
            assert!(!is_valid_geo_hash(bad), "{bad:?} 不该放行");
        }
    }

    /// 仓里三份单位几何（单位盒 / 单位圆柱 / 单位球）必须能整份转出——它们是前端
    /// 本地造几何的对照物，也是 `/api/v1/meshes/{hash}.glb` 最常被命中的三个键。
    /// 顺带把包围盒打出来，给前端 `getUnitTubiGeometry()` / `getUnitSphereGeometry()`
    /// 对参数用（`cargo test -- --nocapture`）。
    #[test]
    fn repository_unit_meshes_convert_end_to_end() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/meshes");
        let mut seen = 0;
        for stem in ["1", "2", "3"] {
            let path = root.join(format!("{stem}.mesh"));
            if !path.is_file() {
                continue;
            }
            let mesh = PlantMesh::des_mesh_file(&path).expect("仓内单位几何必须可读");
            let glb = plant_mesh_to_glb(&mesh).expect("单位几何必须能编成 GLB");
            let (document, _) = split(&glb);
            println!(
                "unit mesh {stem}: vertices={} triangles={} min={} max={}",
                mesh.vertices.len(),
                mesh.indices.len() / 3,
                document["accessors"][0]["min"],
                document["accessors"][0]["max"]
            );
            seen += 1;
        }
        if seen == 0 {
            eprintln!("assets/meshes 下没有 1/2/3.mesh，跳过实物转换");
        }
    }
}
