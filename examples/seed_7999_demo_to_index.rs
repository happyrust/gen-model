/// 将 examples/sctn_7999_demo.rs 的场景对象写入 SQLite R-Tree 索引
/// 便于后续基于 24383/86525 做支撑检测演示
///
/// 运行：
///   cargo run --example seed_7999_demo_to_index --features sqlite-index -- --index aabb_cache.sqlite

use anyhow::Result;
use clap::{Arg, Command};
use aios_database::spatial_index::SqliteSpatialIndex;
use aios_core::pdms_types::RefU64;
use nalgebra::Point3;
use parry3d::bounding_volume::Aabb;
use std::str::FromStr;

#[derive(Debug, Clone)]
struct Item { id: RefU64, bbox: Aabb, noun: &'static str }

fn scene() -> Vec<Item> {
    vec![
        // 目标SCTN - 24383/86525
        Item { id: RefU64::from_str("24383/86525").unwrap(), bbox: Aabb::new(Point3::new(100.0, 5.0, 20.0), Point3::new(110.0, 5.3, 20.6)), noun: "SCTN" },
        // 相邻桥架段
        Item { id: RefU64::from_str("24383/86526").unwrap(), bbox: Aabb::new(Point3::new(109.9, 5.0, 20.0), Point3::new(119.9, 5.3, 20.6)), noun: "SCTN" },
        Item { id: RefU64::from_str("24383/86527").unwrap(), bbox: Aabb::new(Point3::new(90.1, 5.0, 20.0), Point3::new(100.1, 5.3, 20.6)), noun: "SCTN" },
        Item { id: RefU64::from_str("24383/86528").unwrap(), bbox: Aabb::new(Point3::new(119.8, 5.0, 20.0), Point3::new(120.4, 8.0, 20.6)), noun: "SCTN" },
        // 支架
        Item { id: RefU64::from_str("24383/90001").unwrap(), bbox: Aabb::new(Point3::new(102.0, 0.0, 20.2), Point3::new(102.4, 5.0, 20.4)), noun: "SUPPO" },
        Item { id: RefU64::from_str("24383/90002").unwrap(), bbox: Aabb::new(Point3::new(107.0, 0.0, 20.2), Point3::new(107.4, 5.0, 20.4)), noun: "SUPPO" },
        // 穿越管道
        Item { id: RefU64::from_str("24383/50001").unwrap(), bbox: Aabb::new(Point3::new(105.0, 5.2, 19.5), Point3::new(105.3, 5.5, 21.5)), noun: "PIPE" },
        // 附近设备
        Item { id: RefU64::from_str("24383/60001").unwrap(), bbox: Aabb::new(Point3::new(108.0, 4.0, 19.0), Point3::new(112.0, 6.0, 22.0)), noun: "EQUI" },
    ]
}

#[tokio::main]
async fn main() -> Result<()> {
    let m = Command::new("Seed 7999 Demo To Index")
        .version("0.1")
        .about("将演示场景写入SQLite空间索引")
        .arg(Arg::new("index").long("index").required(false).help("索引文件，默认 aabb_cache.sqlite"))
        .get_matches();

    let path = m.get_one::<String>("index").cloned().unwrap_or_else(|| "aabb_cache.sqlite".into());
    let idx = SqliteSpatialIndex::new(&path)?;
    let items = scene();
    let mut cnt = 0usize;
    for it in items {
        idx.insert_aabb(it.id, &it.bbox, Some(it.noun))?;
        cnt += 1;
    }
    println!("已写入 {} 个元素到 {:?}", cnt, path);
    Ok(())
}

