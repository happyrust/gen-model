use anyhow::Result;
use byteorder::{LittleEndian, ReadBytesExt};
use clap::{Arg, Command};
use flate2::read::ZlibDecoder;
use std::io::{Cursor, Read};
use tokio::fs;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("XKT v10 Validator")
        .version("1.0")
        .author("AIOS Database Team")
        .about("验证 XKT v10 文件格式")
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .value_name("FILE")
                .help("要验证的XKT文件路径")
                .required(true),
        )
        .get_matches();

    let file_path = matches.get_one::<String>("file").unwrap();

    println!("=== XKT v10 文件格式验证器 ===");
    println!("文件: {}", file_path);
    println!();

    // 读取文件
    let data = fs::read(file_path).await?;
    println!("文件大小: {} 字节", data.len());

    // 验证文件头
    validate_xkt_header(&data)?;

    Ok(())
}

fn validate_xkt_header(data: &[u8]) -> Result<()> {
    if data.len() < 8 {
        return Err(anyhow::anyhow!("文件太小，无法包含有效的XKT头"));
    }

    let mut cursor = Cursor::new(data);

    // 读取第一个32位整数（版本和压缩标志）
    let version_and_compression = cursor.read_u32::<LittleEndian>()?;
    let compression_flag = (version_and_compression >> 31) & 1;
    let version = version_and_compression & 0x7FFFFFFF;

    println!("XKT 版本: {}", version);
    println!("压缩标志: {}", if compression_flag == 1 { "已压缩" } else { "未压缩" });

    if version != 10 {
        println!("⚠️  警告: 版本不是10，而是{}", version);
    } else {
        println!("✅ 版本正确 (v10)");
    }

    // 读取元素数量
    let element_count = cursor.read_u32::<LittleEndian>()?;
    println!("数据段数量: {}", element_count);

    if element_count != 29 {
        println!("⚠️  警告: XKT v10应该有29个数据段，但找到了{}", element_count);
    } else {
        println!("✅ 数据段数量正确 (29个)");
    }

    // 读取每个元素的大小
    println!("\n数据段大小分布:");
    let mut total_data_size = 0;
    for i in 0..element_count {
        let size = cursor.read_u32::<LittleEndian>()?;
        total_data_size += size;

        let segment_name = match i {
            0 => "metadata",
            1 => "texture_data",
            2 => "each_texture_data_portion",
            3 => "each_texture_attributes",
            4 => "positions",
            5 => "normals",
            6 => "colors",
            7 => "uvs",
            8 => "indices",
            9 => "edge_indices",
            10 => "each_texture_set_textures",
            11 => "matrices",
            12 => "reused_geometries_decode_matrix",
            13 => "each_geometry_primitive_type",
            14 => "each_geometry_axis_label",
            15 => "each_geometry_positions_portion",
            16 => "each_geometry_normals_portion",
            17 => "each_geometry_colors_portion",
            18 => "each_geometry_uvs_portion",
            19 => "each_geometry_indices_portion",
            20 => "each_geometry_edge_indices_portion",
            21 => "each_mesh_geometries_portion",
            22 => "each_mesh_matrices_portion",
            23 => "each_mesh_texture_set",
            24 => "each_mesh_material_attributes",
            25 => "each_entity_id",
            26 => "each_entity_meshes_portion",
            27 => "each_tile_aabb",
            28 => "each_tile_entities_portion",
            _ => "unknown",
        };

        println!("  [{:2}] {:<35} {} 字节", i, segment_name, size);
    }

    let header_size = 4 + 4 + (element_count * 4);
    let expected_total_size = header_size + total_data_size;

    println!("\n文件结构分析:");
    println!("头部大小: {} 字节", header_size);
    println!("数据总大小: {} 字节", total_data_size);
    println!("预期文件大小: {} 字节", expected_total_size);
    println!("实际文件大小: {} 字节", data.len());

    if data.len() as u32 == expected_total_size {
        println!("✅ 文件大小匹配");
    } else {
        println!("⚠️  文件大小不匹配，差异: {} 字节", data.len() as i64 - expected_total_size as i64);
    }

    // 尝试解压第一个数据段（metadata）来验证压缩格式
    if element_count > 0 && compression_flag == 1 {
        println!("\n验证压缩格式:");
        let metadata_size = {
            let mut temp_cursor = Cursor::new(&data[8..12]);
            temp_cursor.read_u32::<LittleEndian>()?
        };

        let metadata_start = header_size as usize;
        let metadata_end = metadata_start + metadata_size as usize;

        if metadata_end <= data.len() {
            let compressed_metadata = &data[metadata_start..metadata_end];

            match decompress_zlib(compressed_metadata) {
                Ok(decompressed) => {
                    println!("✅ metadata段成功解压");
                    println!("   压缩前大小: {} 字节", decompressed.len());
                    println!("   压缩后大小: {} 字节", compressed_metadata.len());
                    println!("   压缩率: {:.1}%",
                        (1.0 - compressed_metadata.len() as f64 / decompressed.len() as f64) * 100.0);

                    // 尝试解析为JSON
                    if let Ok(json_str) = String::from_utf8(decompressed) {
                        if let Ok(_) = serde_json::from_str::<serde_json::Value>(&json_str) {
                            println!("✅ metadata是有效的JSON格式");
                        } else {
                            println!("⚠️  metadata不是有效的JSON格式");
                        }
                    } else {
                        println!("⚠️  metadata不是有效的UTF-8字符串");
                    }
                }
                Err(e) => {
                    println!("❌ metadata段解压失败: {}", e);
                }
            }
        } else {
            println!("❌ metadata段数据范围超出文件大小");
        }
    }

    println!("\n=== 验证完成 ===");
    Ok(())
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}