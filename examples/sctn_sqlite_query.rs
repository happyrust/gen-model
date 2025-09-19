/// 使用SQLite空间索引查询指定SCTN并进行接触检测
///
/// 依赖特性: --features sqlite-index
/// 运行示例:
///   cargo run --example sctn_sqlite_query --features sqlite-index -- \
///     --target 24383/86525 --index aabb_cache.sqlite --radius 1.0 --limit 50
use anyhow::{Result, anyhow};
use clap::{Arg, Command};
use std::path::PathBuf;

use aios_core::pdms_types::RefU64;
use aios_database::spatial_index::SqliteSpatialIndex;
use nalgebra::{Isometry3, Vector3};
use parry3d::{bounding_volume::Aabb, query::contact, shape::Cuboid};

fn parse_refno(s: &str) -> Result<RefU64> {
    use std::str::FromStr;
    RefU64::from_str(s).map_err(|_| anyhow!("无效的RefNo格式: {}", s))
}

fn cuboid_from_aabb(aabb: &Aabb) -> (Cuboid, Isometry3<f32>) {
    let half_extents = (aabb.maxs - aabb.mins) * 0.5;
    let center = aabb.center();
    let shape = Cuboid::new(Vector3::new(
        half_extents.x.max(1e-6),
        half_extents.y.max(1e-6),
        half_extents.z.max(1e-6),
    ));
    let iso = Isometry3::translation(center.x, center.y, center.z);
    (shape, iso)
}

fn expand_aabb(aabb: &Aabb, r: f32) -> Aabb {
    let mins = aabb.mins - Vector3::new(r, r, r);
    let maxs = aabb.maxs + Vector3::new(r, r, r);
    Aabb::new(mins, maxs)
}

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("SCTN SQLite Query")
        .version("0.1")
        .about("从SQLite R-Tree空间索引查询并检测SCTN接触")
        .arg(
            Arg::new("target")
                .long("target")
                .value_name("REFNO")
                .help("目标SCTN参考号，例如 24383/86525")
                .required(true),
        )
        .arg(
            Arg::new("index")
                .long("index")
                .value_name("FILE")
                .help("SQLite索引文件路径，默认读取项目根目录 aabb_cache.sqlite")
                .required(false),
        )
        .arg(
            Arg::new("radius")
                .long("radius")
                .value_name("M")
                .help("查询半径(米): 扩展包围盒进行邻域检索")
                .default_value("1.0"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_name("N")
                .help("最多检查的邻居数量")
                .default_value("100"),
        )
        .arg(
            Arg::new("tolerance")
                .long("tolerance")
                .value_name("M")
                .help("接触检测容差(米)")
                .default_value("0.05"),
        )
        .get_matches();

    let target_str = matches.get_one::<String>("target").unwrap();
    let target = parse_refno(target_str)?;
    let radius: f32 = matches
        .get_one::<String>("radius")
        .unwrap()
        .parse()
        .unwrap_or(1.0);
    let limit: usize = matches
        .get_one::<String>("limit")
        .unwrap()
        .parse()
        .unwrap_or(100);
    let tolerance: f32 = matches
        .get_one::<String>("tolerance")
        .unwrap()
        .parse()
        .unwrap_or(0.05);

    // 索引路径
    let index_path = matches
        .get_one::<String>("index")
        .map(|s| PathBuf::from(s))
        .unwrap_or_else(|| SqliteSpatialIndex::default_path());

    println!("==============================================");
    println!("SCTN SQLite 空间索引查询与接触检测");
    println!("索引文件: {:?}", index_path);
    println!("目标: {}", target.0);
    println!(
        "查询半径: {:.2} m, 容差: {:.2} m, 限制: {} 个",
        radius, tolerance, limit
    );
    println!("==============================================\n");

    if !SqliteSpatialIndex::is_enabled() {
        eprintln!("警告: 未启用 sqlite-index 特性或 DbOption.toml 未设置 enable_sqlite_rtree=true");
    }

    let index = SqliteSpatialIndex::new(&index_path)?;

    // 获取目标AABB
    let Some(target_bbox) = index.get_aabb(target)? else {
        return Err(anyhow!("在索引中未找到目标SCTN: {}", target.0));
    };

    println!(
        "目标包围盒: ({:.3},{:.3},{:.3}) - ({:.3},{:.3},{:.3})",
        target_bbox.mins.x,
        target_bbox.mins.y,
        target_bbox.mins.z,
        target_bbox.maxs.x,
        target_bbox.maxs.y,
        target_bbox.maxs.z
    );

    // 扩展查询并获取邻居
    let query = expand_aabb(&target_bbox, radius);
    let mut neighbors = index.query_intersect(&query)?;
    // 过滤自身
    neighbors.retain(|r| *r != target);
    if neighbors.len() > limit {
        neighbors.truncate(limit);
    }

    println!("\n邻域候选数量: {}", neighbors.len());

    // 目标几何体
    let (shape_t, iso_t) = cuboid_from_aabb(&target_bbox);

    let mut n_contact = 0usize;
    let mut n_proximity = 0usize;

    for r in neighbors {
        if let Some(b) = index.get_aabb(r)? {
            let (shape_o, iso_o) = cuboid_from_aabb(&b);
            match contact(&iso_t, &shape_t, &iso_o, &shape_o, tolerance) {
                Ok(Some(c)) => {
                    if c.dist < -tolerance {
                        n_contact += 1;
                        println!("❌ 穿透: {} 深度 {:.3} m", r.0, -c.dist);
                    } else if c.dist.abs() <= 1e-3 {
                        n_contact += 1;
                        println!("⚠️  表面接触: {}", r.0);
                    } else if c.dist < tolerance {
                        n_proximity += 1;
                        println!("ℹ️  接近: {} 距离 {:.3} m", r.0, c.dist);
                    }
                }
                _ => { /* 无交互，忽略 */ }
            }
        }
    }

    println!(
        "\n结果统计: 接触/碰撞 {} 个, 接近 {} 个",
        n_contact, n_proximity
    );
    Ok(())
}
