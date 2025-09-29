use aios_database::xkt_generator::*;

fn main() -> anyhow::Result<()> {
    println!("=== 调试量化过程 ===");

    // 创建立方体几何体
    let cube_geometry = XKTGeometry::create_box("debug_cube".to_string(), 1.0, 1.0, 1.0);

    println!("立方体几何数据:");
    println!("  位置数量: {}", cube_geometry.positions.len());
    println!("  前几个位置:");
    for (i, chunk) in cube_geometry.positions.chunks(3).enumerate().take(8) {
        if chunk.len() == 3 {
            println!("    顶点[{}]: ({:.3}, {:.3}, {:.3})", i, chunk[0], chunk[1], chunk[2]);
        }
    }

    // 手动计算边界框
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut max_z = f32::NEG_INFINITY;

    for chunk in cube_geometry.positions.chunks(3) {
        if chunk.len() == 3 {
            min_x = min_x.min(chunk[0]);
            min_y = min_y.min(chunk[1]);
            min_z = min_z.min(chunk[2]);
            max_x = max_x.max(chunk[0]);
            max_y = max_y.max(chunk[1]);
            max_z = max_z.max(chunk[2]);
        }
    }

    println!("\n边界框:");
    println!("  最小值: ({:.3}, {:.3}, {:.3})", min_x, min_y, min_z);
    println!("  最大值: ({:.3}, {:.3}, {:.3})", max_x, max_y, max_z);

    let range_x = max_x - min_x;
    let range_y = max_y - min_y;
    let range_z = max_z - min_z;

    println!("  范围: ({:.3}, {:.3}, {:.3})", range_x, range_y, range_z);

    let scale_x = if range_x > 0.0 { 65535.0 / range_x } else { 1.0 };
    let scale_y = if range_y > 0.0 { 65535.0 / range_y } else { 1.0 };
    let scale_z = if range_z > 0.0 { 65535.0 / range_z } else { 1.0 };

    println!("  缩放因子: ({:.3}, {:.3}, {:.3})", scale_x, scale_y, scale_z);

    // 创建解量化矩阵 (使用与XKT写入器相同的逻辑)
    let scale_x = range_x / 65535.0;
    let scale_y = range_y / 65535.0;
    let scale_z = range_z / 65535.0;

    let decode_matrix = [
        scale_x, 0.0, 0.0, 0.0,
        0.0, scale_y, 0.0, 0.0,
        0.0, 0.0, scale_z, 0.0,
        min_x, min_y, min_z, 1.0,
    ];

    println!("  缩放值: scale_x={:.6}, scale_y={:.6}, scale_z={:.6}", scale_x, scale_y, scale_z);

    println!("\n解量化矩阵:");
    for row in 0..4 {
        print!("  [");
        for col in 0..4 {
            let idx = row * 4 + col;
            print!("{:8.4}", decode_matrix[idx]);
            if col < 3 { print!(", "); }
        }
        println!("]");
    }

    // 测试几个顶点的量化和反量化
    println!("\n量化测试:");
    for (i, chunk) in cube_geometry.positions.chunks(3).enumerate().take(4) {
        if chunk.len() == 3 {
            let qx = ((chunk[0] - min_x) * scale_x).round().clamp(0.0, 65535.0) as u16;
            let qy = ((chunk[1] - min_y) * scale_y).round().clamp(0.0, 65535.0) as u16;
            let qz = ((chunk[2] - min_z) * scale_z).round().clamp(0.0, 65535.0) as u16;

            // 反量化
            let rx = qx as f32 * decode_matrix[0] + decode_matrix[12];
            let ry = qy as f32 * decode_matrix[5] + decode_matrix[13];
            let rz = qz as f32 * decode_matrix[10] + decode_matrix[14];

            println!("  顶点[{}]:", i);
            println!("    原始: ({:.3}, {:.3}, {:.3})", chunk[0], chunk[1], chunk[2]);
            println!("    量化: ({}, {}, {})", qx, qy, qz);
            println!("    恢复: ({:.3}, {:.3}, {:.3})", rx, ry, rz);
        }
    }

    Ok(())
}