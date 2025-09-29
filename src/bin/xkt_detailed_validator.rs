use anyhow::Result;
use byteorder::{LittleEndian, ReadBytesExt};
use clap::{Arg, Command};
use flate2::read::ZlibDecoder;
use std::io::{Cursor, Read};
use tokio::fs;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("XKT Detailed Validator")
        .version("1.0")
        .about("详细分析 XKT 文件的内部结构和数据段")
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

    println!("=== XKT 详细结构分析器 ===");
    println!("文件: {}", file_path);
    println!();

    // 读取文件
    let data = fs::read(file_path).await?;
    println!("文件大小: {} 字节", data.len());

    // 详细分析文件结构
    analyze_xkt_structure(&data)?;

    Ok(())
}

fn analyze_xkt_structure(data: &[u8]) -> Result<()> {
    if data.len() < 8 {
        return Err(anyhow::anyhow!("文件太小"));
    }

    let mut cursor = Cursor::new(data);

    // 读取头部
    let version_and_compression = cursor.read_u32::<LittleEndian>()?;
    let compression_flag = (version_and_compression >> 31) & 1;
    let version = version_and_compression & 0x7FFFFFFF;

    println!("头部信息:");
    println!("  版本: {}", version);
    println!("  压缩: {}", if compression_flag == 1 { "是" } else { "否" });

    let element_count = cursor.read_u32::<LittleEndian>()?;
    println!("  数据段数量: {}", element_count);

    // 读取每个数据段的大小
    let mut segment_sizes = Vec::new();
    for i in 0..element_count {
        let size = cursor.read_u32::<LittleEndian>()?;
        segment_sizes.push(size);
        println!("  段[{}]: {} 字节", i, size);
    }

    let header_size = 8 + (element_count * 4) as usize;
    println!("\n头部大小: {} 字节", header_size);

    // 验证每个数据段
    let mut current_offset = header_size;
    for (i, &size) in segment_sizes.iter().enumerate() {
        let start = current_offset;
        let end = start + size as usize;

        if end > data.len() {
            println!("❌ 段[{}]: 数据超出文件边界 ({}..{} > {})", i, start, end, data.len());
            continue;
        }

        let segment_data = &data[start..end];

        println!("\n段[{}] 详细分析:", i);
        println!("  偏移: {} - {}", start, end);
        println!("  大小: {} 字节", size);

        // 如果是压缩数据，尝试解压
        if compression_flag == 1 {
            match decompress_zlib(segment_data) {
                Ok(decompressed) => {
                    println!("  解压成功: {} 字节 -> {} 字节", size, decompressed.len());

                    // 分析解压后的数据
                    analyze_decompressed_data(i, &decompressed);
                }
                Err(e) => {
                    println!("  解压失败: {}", e);
                }
            }
        } else {
            println!("  未压缩数据");
            analyze_decompressed_data(i, segment_data);
        }

        current_offset = end;
    }

    // 检查总大小
    let expected_size = header_size + segment_sizes.iter().sum::<u32>() as usize;
    println!("\n总体验证:");
    println!("  预期大小: {} 字节", expected_size);
    println!("  实际大小: {} 字节", data.len());

    if expected_size == data.len() {
        println!("  ✅ 大小匹配");
    } else {
        println!("  ❌ 大小不匹配，差异: {} 字节", data.len() as i64 - expected_size as i64);
    }

    Ok(())
}

fn analyze_decompressed_data(segment_index: usize, data: &[u8]) {
    let segment_names = [
        "metadata", "texture_data", "each_texture_data_portion", "each_texture_attributes",
        "positions", "normals", "colors", "uvs", "indices", "edge_indices",
        "each_texture_set_textures", "matrices", "reused_geometries_decode_matrix",
        "each_geometry_primitive_type", "each_geometry_axis_label", "each_geometry_positions_portion",
        "each_geometry_normals_portion", "each_geometry_colors_portion", "each_geometry_uvs_portion",
        "each_geometry_indices_portion", "each_geometry_edge_indices_portion", "each_mesh_geometries_portion",
        "each_mesh_matrices_portion", "each_mesh_texture_set", "each_mesh_material_attributes",
        "each_entity_id", "each_entity_meshes_portion", "each_tile_aabb", "each_tile_entities_portion"
    ];

    let segment_name = segment_names.get(segment_index).unwrap_or(&"unknown");

    match segment_name {
        &"metadata" | &"each_geometry_axis_label" | &"each_entity_id" => {
            // JSON或字符串数据
            if let Ok(text) = String::from_utf8(data.to_vec()) {
                println!("  内容类型: JSON/字符串");
                println!("  内容预览: {}", &text[..text.len().min(100)]);
            } else {
                println!("  内容类型: 二进制数据");
            }
        }
        &"positions" => {
            // 16位无符号整数数组
            if data.len() % 2 == 0 {
                println!("  内容类型: Uint16Array");
                println!("  元素数量: {}", data.len() / 2);
                println!("  ✅ 大小对齐 (2字节)");
            } else {
                println!("  ❌ Uint16Array 大小不对齐: {} 字节", data.len());
            }
        }
        &"normals" => {
            // 8位有符号整数数组
            println!("  内容类型: Int8Array");
            println!("  元素数量: {}", data.len());
        }
        &"colors" | &"each_geometry_primitive_type" | &"each_mesh_material_attributes" => {
            // 8位无符号整数数组
            println!("  内容类型: Uint8Array");
            println!("  元素数量: {}", data.len());
        }
        &"indices" | &"edge_indices" | &"each_texture_data_portion" | &"each_geometry_positions_portion" |
        &"each_geometry_normals_portion" | &"each_geometry_colors_portion" | &"each_geometry_uvs_portion" |
        &"each_geometry_indices_portion" | &"each_geometry_edge_indices_portion" | &"each_mesh_geometries_portion" |
        &"each_mesh_matrices_portion" | &"each_entity_meshes_portion" | &"each_tile_entities_portion" => {
            // 32位无符号整数数组
            if data.len() % 4 == 0 {
                println!("  内容类型: Uint32Array");
                println!("  元素数量: {}", data.len() / 4);
                println!("  ✅ 大小对齐 (4字节)");
            } else {
                println!("  ❌ Uint32Array 大小不对齐: {} 字节 (应为4的倍数)", data.len());
            }
        }
        &"matrices" | &"reused_geometries_decode_matrix" | &"uvs" => {
            // 32位浮点数组
            if data.len() % 4 == 0 {
                println!("  内容类型: Float32Array");
                println!("  元素数量: {}", data.len() / 4);
                println!("  ✅ 大小对齐 (4字节)");
            } else {
                println!("  ❌ Float32Array 大小不对齐: {} 字节", data.len());
            }
        }
        &"each_tile_aabb" => {
            // 64位浮点数组
            if data.len() % 8 == 0 {
                println!("  内容类型: Float64Array");
                println!("  元素数量: {}", data.len() / 8);
                println!("  ✅ 大小对齐 (8字节)");
            } else {
                println!("  ❌ Float64Array 大小不对齐: {} 字节", data.len());
            }
        }
        &"each_mesh_texture_set" | &"each_texture_set_textures" => {
            // 32位有符号整数数组
            if data.len() % 4 == 0 {
                println!("  内容类型: Int32Array");
                println!("  元素数量: {}", data.len() / 4);
                println!("  ✅ 大小对齐 (4字节)");
            } else {
                println!("  ❌ Int32Array 大小不对齐: {} 字节", data.len());
            }
        }
        _ => {
            println!("  内容类型: 未知");
        }
    }
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}