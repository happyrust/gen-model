use anyhow::Result;
use byteorder::{LittleEndian, ReadBytesExt};
use clap::{Arg, Command};
use flate2::read::ZlibDecoder;
use std::io::{Cursor, Read};
use tokio::fs;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("XKT Data Analyzer")
        .version("1.0")
        .about("深度分析XKT文件的实际数据内容")
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .value_name("FILE")
                .help("要分析的XKT文件路径")
                .required(true),
        )
        .get_matches();

    let file_path = matches.get_one::<String>("file").unwrap();

    println!("=== XKT 数据内容深度分析 ===");
    println!("文件: {}", file_path);
    println!();

    let data = fs::read(file_path).await?;
    analyze_xkt_data(&data)?;

    Ok(())
}

fn analyze_xkt_data(data: &[u8]) -> Result<()> {
    let mut cursor = Cursor::new(data);

    // 读取头部
    let version_and_compression = cursor.read_u32::<LittleEndian>()?;
    let compression_flag = (version_and_compression >> 31) & 1;
    let version = version_and_compression & 0x7FFFFFFF;
    let element_count = cursor.read_u32::<LittleEndian>()?;

    println!("文件头部:");
    println!("  版本: {}", version);
    println!("  压缩: {}", if compression_flag == 1 { "是" } else { "否" });
    println!("  数据段数量: {}", element_count);

    // 读取数据段大小
    let mut segment_sizes = Vec::new();
    for _ in 0..element_count {
        let size = cursor.read_u32::<LittleEndian>()?;
        segment_sizes.push(size);
    }

    let header_size = (2 + element_count) * 4;
    let mut current_offset = header_size as usize;

    // 重要数据段的索引
    let metadata_idx = 0;
    let positions_idx = 4;
    let normals_idx = 5;
    let indices_idx = 8;
    let edge_indices_idx = 9;
    let reused_geometries_decode_matrix_idx = 12;
    let each_entity_id_idx = 25;
    let each_tile_aabb_idx = 27;

    for (i, &size) in segment_sizes.iter().enumerate() {
        let start = current_offset;
        let end = start + size as usize;

        if end > data.len() {
            println!("❌ 段[{}]: 数据超出文件边界", i);
            continue;
        }

        let segment_data = &data[start..end];

        // 解压数据
        let decompressed_data = if compression_flag == 1 {
            match decompress_zlib(segment_data) {
                Ok(data) => data,
                Err(_) => {
                    println!("❌ 段[{}]: 解压失败", i);
                    current_offset = end;
                    continue;
                }
            }
        } else {
            segment_data.to_vec()
        };

        // 分析关键数据段
        match i {
            idx if idx == metadata_idx => {
                analyze_metadata(&decompressed_data);
            }
            idx if idx == positions_idx => {
                analyze_positions(&decompressed_data);
            }
            idx if idx == normals_idx => {
                analyze_normals(&decompressed_data);
            }
            idx if idx == indices_idx => {
                analyze_indices(&decompressed_data);
            }
            idx if idx == edge_indices_idx => {
                analyze_edge_indices(&decompressed_data);
            }
            idx if idx == reused_geometries_decode_matrix_idx => {
                analyze_decode_matrix(&decompressed_data);
            }
            idx if idx == each_entity_id_idx => {
                analyze_entity_ids(&decompressed_data);
            }
            idx if idx == each_tile_aabb_idx => {
                analyze_tile_aabb(&decompressed_data);
            }
            _ => {
                // 跳过其他段
            }
        }

        current_offset = end;
    }

    Ok(())
}

fn analyze_metadata(data: &[u8]) {
    println!("\n📋 元数据分析:");
    if let Ok(text) = String::from_utf8(data.to_vec()) {
        println!("  内容: {}", text);

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(meta_objects) = json.get("metaObjects").and_then(|v| v.as_array()) {
                println!("  对象数量: {}", meta_objects.len());
                for (i, obj) in meta_objects.iter().enumerate() {
                    if let (Some(id), Some(name), Some(obj_type)) = (
                        obj.get("id").and_then(|v| v.as_str()),
                        obj.get("name").and_then(|v| v.as_str()),
                        obj.get("type").and_then(|v| v.as_str())
                    ) {
                        println!("    对象[{}]: {} ({}) - {}", i, name, id, obj_type);
                    }
                }
            }
        }
    }
}

fn analyze_positions(data: &[u8]) {
    println!("\n📍 位置数据分析:");
    println!("  原始大小: {} 字节", data.len());

    if data.len() % 2 == 0 {
        let vertex_count = data.len() / 2 / 3; // 每个顶点3个u16
        println!("  顶点数量: {}", vertex_count);

        // 读取前几个顶点
        let mut cursor = Cursor::new(data);
        println!("  前几个量化坐标:");
        for i in 0..vertex_count.min(8) {
            if let (Ok(x), Ok(y), Ok(z)) = (
                cursor.read_u16::<LittleEndian>(),
                cursor.read_u16::<LittleEndian>(),
                cursor.read_u16::<LittleEndian>()
            ) {
                println!("    顶点[{}]: ({}, {}, {})", i, x, y, z);
            }
        }
    } else {
        println!("  ❌ 位置数据大小不正确");
    }
}

fn analyze_normals(data: &[u8]) {
    println!("\n🎯 法向量数据分析:");
    println!("  原始大小: {} 字节", data.len());

    let normal_count = data.len() / 2; // 每个法向量2个i8
    println!("  法向量数量: {}", normal_count);

    if !data.is_empty() {
        println!("  前几个oct编码法向量:");
        for i in 0..normal_count.min(8) {
            if i * 2 + 1 < data.len() {
                let x = data[i * 2] as i8;
                let y = data[i * 2 + 1] as i8;
                println!("    法向量[{}]: ({}, {})", i, x, y);
            }
        }
    }
}

fn analyze_indices(data: &[u8]) {
    println!("\n🔺 索引数据分析:");
    println!("  原始大小: {} 字节", data.len());

    if data.len() % 4 == 0 {
        let index_count = data.len() / 4;
        let triangle_count = index_count / 3;
        println!("  索引数量: {}", index_count);
        println!("  三角形数量: {}", triangle_count);

        let mut cursor = Cursor::new(data);
        println!("  前几个三角形:");
        for i in 0..triangle_count.min(4) {
            if let (Ok(a), Ok(b), Ok(c)) = (
                cursor.read_u32::<LittleEndian>(),
                cursor.read_u32::<LittleEndian>(),
                cursor.read_u32::<LittleEndian>()
            ) {
                println!("    三角形[{}]: ({}, {}, {})", i, a, b, c);
            }
        }
    } else {
        println!("  ❌ 索引数据大小不正确");
    }
}

fn analyze_edge_indices(data: &[u8]) {
    println!("\n📏 边缘索引数据分析:");
    println!("  原始大小: {} 字节", data.len());

    if data.len() % 4 == 0 {
        let edge_index_count = data.len() / 4;
        let edge_count = edge_index_count / 2;
        println!("  边缘索引数量: {}", edge_index_count);
        println!("  边缘数量: {}", edge_count);

        let mut cursor = Cursor::new(data);
        println!("  前几条边:");
        for i in 0..edge_count.min(6) {
            if let (Ok(a), Ok(b)) = (
                cursor.read_u32::<LittleEndian>(),
                cursor.read_u32::<LittleEndian>()
            ) {
                println!("    边[{}]: ({}, {})", i, a, b);
            }
        }
    }
}

fn analyze_decode_matrix(data: &[u8]) {
    println!("\n🔄 解量化矩阵分析:");
    println!("  原始大小: {} 字节", data.len());

    if data.len() == 64 { // 16个float32
        let mut cursor = Cursor::new(data);
        println!("  4x4矩阵:");
        for row in 0..4 {
            print!("    [");
            for col in 0..4 {
                if let Ok(value) = cursor.read_f32::<LittleEndian>() {
                    print!("{:12.8}", value);
                    if col < 3 { print!(", "); }
                }
            }
            println!("]");
        }
    }
}

fn analyze_entity_ids(data: &[u8]) {
    println!("\n🏷️  实体ID分析:");
    if let Ok(text) = String::from_utf8(data.to_vec()) {
        println!("  内容: {}", text);
        if let Ok(ids) = serde_json::from_str::<Vec<String>>(&text) {
            println!("  实体数量: {}", ids.len());
            for (i, id) in ids.iter().enumerate() {
                println!("    实体[{}]: {}", i, id);
            }
        }
    }
}

fn analyze_tile_aabb(data: &[u8]) {
    println!("\n📦 瓦片包围盒分析:");
    println!("  原始大小: {} 字节", data.len());

    if data.len() == 48 { // 6个float64
        let mut cursor = Cursor::new(data);
        if let (Ok(min_x), Ok(min_y), Ok(min_z), Ok(max_x), Ok(max_y), Ok(max_z)) = (
            cursor.read_f64::<LittleEndian>(),
            cursor.read_f64::<LittleEndian>(),
            cursor.read_f64::<LittleEndian>(),
            cursor.read_f64::<LittleEndian>(),
            cursor.read_f64::<LittleEndian>(),
            cursor.read_f64::<LittleEndian>()
        ) {
            println!("  包围盒: [{:.3}, {:.3}, {:.3}] 到 [{:.3}, {:.3}, {:.3}]",
                min_x, min_y, min_z, max_x, max_y, max_z);
        }
    }
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}