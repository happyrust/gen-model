//! 集成测试：SqliteSpatialIndex 的高级空间查询能力

use aios_database::spatial_index::{
    SqliteSpatialIndex, SpatialQueryBackend, QueryOptions, SortBy, SortOrder
};
use aios_core::RefU64;
use parry3d::bounding_volume::Aabb;
use nalgebra::{Point3, Vector3};

#[cfg(feature = "sqlite-index")]
fn aabb(minx: f32, miny: f32, minz: f32, maxx: f32, maxy: f32, maxz: f32) -> Aabb {
    Aabb::new(Point3::new(minx, miny, minz), Point3::new(maxx, maxy, maxz))
}

#[cfg(feature = "sqlite-index")]
fn setup_index() -> anyhow::Result<(SqliteSpatialIndex, tempfile::TempDir)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("aabb_cache.sqlite");
    let index = SqliteSpatialIndex::new(path)?;

    // 插入测试数据
    // id=1 PIPE  (-1..1)
    index.insert_aabb(RefU64(1), &aabb(-1.0, -0.5, -0.5, 1.0, 0.5, 0.5), Some("PIPE"))?;
    // id=2 SUPPORT (5..6)
    index.insert_aabb(RefU64(2), &aabb(5.0, -0.5, -0.5, 6.0, 0.5, 0.5), Some("SUPPORT"))?;
    // id=3 EQUI (10..11)
    index.insert_aabb(RefU64(3), &aabb(10.0, -0.5, -0.5, 11.0, 0.5, 0.5), Some("EQUI"))?;
    // id=4 PIPE (2.2..2.5) 小盒
    index.insert_aabb(RefU64(4), &aabb(2.2, -0.2, -0.2, 2.5, 0.2, 0.2), Some("PIPE"))?;

    Ok((index, dir))
}

#[cfg(feature = "sqlite-index")]
#[test]
fn test_intersect_with_type_filter() -> anyhow::Result<()> {
    let (index, _guard) = setup_index()?;
    let query = aabb(-2.0, -2.0, -2.0, 7.0, 2.0, 2.0);
    let mut opts = QueryOptions::default();
    opts.types = vec!["PIPE".into()];
    opts.include_bbox = true;
    let hits = index.query_intersect_hits(&query, &opts)?;
    let ids: Vec<u64> = hits.into_iter().map(|h| h.refno.0).collect();
    assert_eq!(ids, vec![1, 4]);
    Ok(())
}

#[cfg(feature = "sqlite-index")]
#[test]
fn test_contains_query() -> anyhow::Result<()> {
    let (index, _guard) = setup_index()?;
    // 一个大盒，应该只完全包含 id=4
    let container = aabb(2.0, -1.0, -1.0, 3.0, 1.0, 1.0);
    let mut opts = QueryOptions::default();
    opts.include_bbox = true;
    let hits = index.query_contains_hits(&container, &opts)?;
    let ids: Vec<u64> = hits.into_iter().map(|h| h.refno.0).collect();
    assert_eq!(ids, vec![4]);
    Ok(())
}

#[cfg(feature = "sqlite-index")]
#[test]
fn test_knn_query() -> anyhow::Result<()> {
    let (index, _guard) = setup_index()?;
    let p = Point3::new(5.2, 0.0, 0.0);
    let opts = QueryOptions::default();
    let hits = index.query_nearest_to_point(p, 2, Some(0.5), &opts)?;
    let ids: Vec<u64> = hits.into_iter().map(|h| h.refno.0).collect();
    // 离 5.2 最近的应为 id=2(5..6)，其次 id=1(-1..1) 或 id=4(2.2..2.5)
    assert_eq!(ids.first().copied(), Some(2));
    assert_eq!(ids.len(), 2);
    Ok(())
}

#[cfg(feature = "sqlite-index")]
#[test]
fn test_ray_query() -> anyhow::Result<()> {
    let (index, _guard) = setup_index()?;
    let origin = Point3::new(-10.0, 0.0, 0.0);
    let dir = Vector3::new(1.0, 0.0, 0.0);
    let mut opts = QueryOptions::default();
    opts.limit = Some(3);
    let hits = index.query_ray_hits(origin, dir, 100.0, &opts)?;
    let ids: Vec<u64> = hits.into_iter().map(|h| h.refno.0).collect();
    // 沿 +X 射线，命中顺序应为 1 -> 4 -> 2 -> 3（但限制为前 3 个）
    assert_eq!(ids, vec![1, 4, 2]);
    Ok(())
}
