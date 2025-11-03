use aios_core::RefU64;
use anyhow::Result;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 空间查询结果项
#[derive(Debug, Clone)]
pub struct SpatialHit {
    pub refno: RefU64,
    pub bbox: Option<Aabb>,
    pub distance: Option<f32>,
}

/// 排序方向
#[derive(Debug, Clone, Copy)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// 排序字段
#[derive(Debug, Clone)]
pub enum SortBy {
    Id(SortOrder),
    DistanceTo(Point3<f32>),
}

/// 查询参数
#[derive(Debug, Clone)]
pub struct QueryOptions {
    pub types: Vec<String>,
    pub tolerance: f32,
    pub limit: Option<usize>,
    pub include_bbox: bool,
    pub sort: Option<SortBy>,
    pub exclude: Vec<RefU64>,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            types: vec![],
            tolerance: 0.0,
            limit: None,
            include_bbox: false,
            sort: None,
            exclude: vec![],
        }
    }
}

/// 空间查询后端统一接口
pub trait SpatialQueryBackend {
    fn query_intersect_hits(
        &self,
        query: &Aabb,
        opts: &QueryOptions,
    ) -> anyhow::Result<Vec<SpatialHit>>;
    fn query_contains_hits(
        &self,
        query: &Aabb,
        opts: &QueryOptions,
    ) -> anyhow::Result<Vec<SpatialHit>>;
    fn query_nearest_to_point(
        &self,
        point: Point3<f32>,
        k: usize,
        search_radius: Option<f32>,
        opts: &QueryOptions,
    ) -> anyhow::Result<Vec<SpatialHit>>;
    fn query_ray_hits(
        &self,
        origin: Point3<f32>,
        dir: Vector3<f32>,
        max_distance: f32,
        opts: &QueryOptions,
    ) -> anyhow::Result<Vec<SpatialHit>>;
}

#[cfg(feature = "sqlite-index")]
use rusqlite::{Connection, OptionalExtension, params};

/// SQLite-based spatial index for AABB queries using R*-tree
/// This replaces the global AABB_CACHE with a more modular approach
pub struct SqliteSpatialIndex {
    path: PathBuf,
}

impl SqliteSpatialIndex {
    /// Create a new spatial index with the given path
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let index = Self {
            path: path.as_ref().to_path_buf(),
        };
        index.init_schema()?;
        Ok(index)
    }

    /// Create with default path
    pub fn with_default_path() -> Result<Self> {
        let path = Self::default_path();
        Self::new(path)
    }

    /// Get the default path for the SQLite database
    pub fn default_path() -> PathBuf {
        #[cfg(feature = "sqlite-index")]
        {
            use config as cfg;
            // Allow override via DbOption.toml key: sqlite_index_path
            if let Ok(built) = cfg::Config::builder()
                .add_source(cfg::File::with_name("DbOption").required(false))
                .build()
            {
                if let Ok(path) = built.get_string("sqlite_index_path") {
                    return PathBuf::from(path);
                }
            }
        }
        PathBuf::from("aabb_cache.sqlite")
    }

    /// Check if SQLite index is enabled via configuration
    pub fn is_enabled() -> bool {
        #[cfg(feature = "sqlite-index")]
        {
            use config as cfg;
            // Read from DbOption.toml if present; default false
            let mut s = cfg::Config::builder();
            if std::path::Path::new("DbOption.toml").exists() {
                s = s.add_source(cfg::File::with_name("DbOption"));
            }
            if let Ok(built) = s.build() {
                // 兼容两个键：enable_sqlite_rtree 与 sqlite_index_enabled
                built
                    .get_bool("enable_sqlite_rtree")
                    .ok()
                    .or_else(|| built.get_bool("sqlite_index_enabled").ok())
                    .unwrap_or(false)
            } else {
                false
            }
        }
        #[cfg(not(feature = "sqlite-index"))]
        false
    }

    #[cfg(feature = "sqlite-index")]
    fn get_connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        Self::configure_connection(&conn)?;
        Ok(conn)
    }

    #[cfg(feature = "sqlite-index")]
    fn configure_connection(conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", 10000)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    }

    /// Initialize the database schema
    #[cfg(feature = "sqlite-index")]
    pub fn init_schema(&self) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS items (
                id INTEGER PRIMARY KEY,
                noun TEXT
            );
            
            -- 3D AABB RTree: id, [min_x, max_x], [min_y, max_y], [min_z, max_z]
            CREATE VIRTUAL TABLE IF NOT EXISTS aabb_index USING rtree(
                id, min_x, max_x, min_y, max_y, min_z, max_z
            );
            
            -- Create index on noun for faster lookups
            CREATE INDEX IF NOT EXISTS idx_items_noun ON items(noun);
            "#,
        )?;
        Ok(())
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn init_schema(&self) -> Result<()> {
        Ok(())
    }

    /// Insert or update a single AABB
    #[cfg(feature = "sqlite-index")]
    pub fn insert_aabb(&self, refno: RefU64, aabb: &Aabb, noun: Option<&str>) -> Result<()> {
        let conn = self.get_connection()?;
        let mut tx = conn.unchecked_transaction()?;

        // Insert/replace in items table
        tx.execute(
            "INSERT OR REPLACE INTO items (id, noun) VALUES (?1, ?2)",
            params![refno.0 as i64, noun.unwrap_or("")],
        )?;

        // Insert/replace in rtree
        tx.execute(
            "INSERT OR REPLACE INTO aabb_index (id, min_x, max_x, min_y, max_y, min_z, max_z) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                refno.0 as i64,
                aabb.mins.x as f64,
                aabb.maxs.x as f64,
                aabb.mins.y as f64,
                aabb.maxs.y as f64,
                aabb.mins.z as f64,
                aabb.maxs.z as f64
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn insert_aabb(&self, _refno: RefU64, _aabb: &Aabb, _noun: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// Batch insert/update AABBs
    #[cfg(feature = "sqlite-index")]
    pub fn insert_many<I>(&self, iter: I) -> Result<usize>
    where
        I: IntoIterator<Item = (RefU64, Aabb, Option<String>)>,
    {
        let conn = self.get_connection()?;
        let mut tx = conn.unchecked_transaction()?;
        let mut count = 0;

        for (refno, aabb, noun) in iter {
            let id = refno.0 as i64;

            tx.execute(
                "INSERT OR REPLACE INTO items (id, noun) VALUES (?1, ?2)",
                params![id, noun.as_deref().unwrap_or("")],
            )?;

            tx.execute(
                "INSERT OR REPLACE INTO aabb_index (id, min_x, max_x, min_y, max_y, min_z, max_z) 
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    aabb.mins.x as f64,
                    aabb.maxs.x as f64,
                    aabb.mins.y as f64,
                    aabb.maxs.y as f64,
                    aabb.mins.z as f64,
                    aabb.maxs.z as f64
                ],
            )?;

            count += 1;
        }

        tx.commit()?;
        Ok(count)
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn insert_many<I>(&self, _iter: I) -> Result<usize>
    where
        I: IntoIterator<Item = (RefU64, Aabb, Option<String>)>,
    {
        Ok(0)
    }

    /// Query intersecting AABBs
    #[cfg(feature = "sqlite-index")]
    pub fn query_intersect(&self, query: &Aabb) -> Result<Vec<RefU64>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare_cached(
            "SELECT id FROM aabb_index 
             WHERE max_x >= ?1 AND min_x <= ?2 
               AND max_y >= ?3 AND min_y <= ?4 
               AND max_z >= ?5 AND min_z <= ?6",
        )?;

        let ids = stmt
            .query_map(
                params![
                    query.mins.x as f64,
                    query.maxs.x as f64,
                    query.mins.y as f64,
                    query.maxs.y as f64,
                    query.mins.z as f64,
                    query.maxs.z as f64
                ],
                |row| Ok(RefU64(row.get::<_, i64>(0)? as u64)),
            )?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ids)
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn query_intersect(&self, _query: &Aabb) -> Result<Vec<RefU64>> {
        Ok(vec![])
    }

    // -------------------- 扩展查询能力（包含/高级相交/近邻/射线） --------------------
    /// 高级相交查询：支持类型过滤、容差、排序、限制等
    #[cfg(feature = "sqlite-index")]
    fn query_intersect_advanced(
        &self,
        query: &Aabb,
        opts: &QueryOptions,
    ) -> Result<Vec<SpatialHit>> {
        self.query_by_overlap(query, opts, false)
    }

    /// 包含查询：返回完全被 query AABB 包含的元素
    #[cfg(feature = "sqlite-index")]
    fn query_contains_advanced(
        &self,
        query: &Aabb,
        opts: &QueryOptions,
    ) -> Result<Vec<SpatialHit>> {
        self.query_by_overlap(query, opts, true)
    }

    /// 内部：统一构造 SQL 执行 overlap/contain 两类查询
    #[cfg(feature = "sqlite-index")]
    fn query_by_overlap(
        &self,
        query: &Aabb,
        opts: &QueryOptions,
        contains: bool,
    ) -> Result<Vec<SpatialHit>> {
        use rusqlite::{ToSql, params_from_iter};
        let conn = self.get_connection()?;

        let tol = opts.tolerance.max(0.0);
        let (minx, maxx) = ((query.mins.x - tol) as f64, (query.maxs.x + tol) as f64);
        let (miny, maxy) = ((query.mins.y - tol) as f64, (query.maxs.y + tol) as f64);
        let (minz, maxz) = ((query.mins.z - tol) as f64, (query.maxs.z + tol) as f64);

        let mut sql = String::from(
            "SELECT aabb_index.id, min_x, max_x, min_y, max_y, min_z, max_z FROM aabb_index ",
        );
        let use_type_filter = !opts.types.is_empty();
        if use_type_filter {
            sql.push_str("JOIN items ON items.id = aabb_index.id ");
        }
        sql.push_str("WHERE ");
        if contains {
            sql.push_str("min_x >= ? AND max_x <= ? AND min_y >= ? AND max_y <= ? AND min_z >= ? AND max_z <= ? ");
        } else {
            sql.push_str("max_x >= ? AND min_x <= ? AND max_y >= ? AND min_y <= ? AND max_z >= ? AND min_z <= ? ");
        }

        let mut params: Vec<Box<dyn ToSql>> = vec![
            Box::new(minx),
            Box::new(maxx),
            Box::new(miny),
            Box::new(maxy),
            Box::new(minz),
            Box::new(maxz),
        ];

        if use_type_filter {
            sql.push_str("AND items.noun IN (");
            for i in 0..opts.types.len() {
                if i > 0 {
                    sql.push_str(",");
                }
                sql.push_str("?");
            }
            sql.push_str(") ");
            for t in &opts.types {
                params.push(Box::new(t.clone()));
            }
        }

        if !opts.exclude.is_empty() {
            sql.push_str("AND aabb_index.id NOT IN (");
            for i in 0..opts.exclude.len() {
                if i > 0 {
                    sql.push_str(",");
                }
                sql.push_str("?");
            }
            sql.push_str(") ");
            for e in &opts.exclude {
                params.push(Box::new(e.0 as i64));
            }
        }

        if let Some(SortBy::Id(order)) = &opts.sort {
            sql.push_str("ORDER BY aabb_index.id ");
            match order {
                SortOrder::Asc => sql.push_str("ASC "),
                SortOrder::Desc => sql.push_str("DESC "),
            }
        }

        if let Some(limit) = opts.limit {
            sql.push_str("LIMIT ? ");
            params.push(Box::new(limit as i64));
        }

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params.iter().map(|p| &**p)), |row| {
            let id: i64 = row.get(0)?;
            let min_x: f64 = row.get(1)?;
            let max_x: f64 = row.get(2)?;
            let min_y: f64 = row.get(3)?;
            let max_y: f64 = row.get(4)?;
            let min_z: f64 = row.get(5)?;
            let max_z: f64 = row.get(6)?;
            Ok((id as u64, [min_x, min_y, min_z], [max_x, max_y, max_z]))
        })?;

        let mut hits = Vec::new();
        for r in rows {
            let (id, mins, maxs) = r?;
            let aabb = Aabb::new(
                [mins[0] as f32, mins[1] as f32, mins[2] as f32].into(),
                [maxs[0] as f32, maxs[1] as f32, maxs[2] as f32].into(),
            );
            hits.push(SpatialHit {
                refno: RefU64(id),
                bbox: if opts.include_bbox { Some(aabb) } else { None },
                distance: None,
            });
        }

        if let Some(SortBy::DistanceTo(p)) = &opts.sort {
            for h in hits.iter_mut() {
                let bb = if let Some(b) = &h.bbox {
                    b.clone()
                } else {
                    self.get_aabb(h.refno)?.unwrap_or_else(|| {
                        Aabb::new([0.0, 0.0, 0.0].into(), [0.0, 0.0, 0.0].into())
                    })
                };
                h.distance = Some(distance_point_aabb(*p, &bb));
            }
            hits.sort_by(|a, b| {
                a.distance
                    .partial_cmp(&b.distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(limit) = opts.limit {
                if hits.len() > limit {
                    hits.truncate(limit);
                }
            }
        }

        Ok(hits)
    }

    /// 近邻查询（点）
    #[cfg(feature = "sqlite-index")]
    fn query_knn_point(
        &self,
        point: Point3<f32>,
        k: usize,
        search_radius: Option<f32>,
        opts: &QueryOptions,
    ) -> Result<Vec<SpatialHit>> {
        let mut radius = search_radius.unwrap_or(1.0_f32);
        let mut best: Vec<SpatialHit> = Vec::new();
        for _ in 0..10 {
            let q = Aabb::new(
                (point - Vector3::new(radius, radius, radius)).into(),
                (point + Vector3::new(radius, radius, radius)).into(),
            );
            let mut local_opts = opts.clone();
            local_opts.include_bbox = true;
            local_opts.limit = Some((k as usize).saturating_mul(8));
            let mut hits = self.query_intersect_advanced(&q, &local_opts)?;
            hits.sort_by_key(|h| h.refno.0);
            hits.dedup_by_key(|h| h.refno.0);
            for h in hits.iter_mut() {
                if let Some(bb) = &h.bbox {
                    h.distance = Some(distance_point_aabb(point, bb));
                }
            }
            hits.sort_by(|a, b| {
                a.distance
                    .partial_cmp(&b.distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if hits.len() >= k {
                best = hits.into_iter().take(k).collect();
                break;
            }
            best = hits;
            radius *= 2.0;
        }
        if best.len() > k {
            best.truncate(k);
        }
        Ok(best)
    }

    /// 射线相交
    #[cfg(feature = "sqlite-index")]
    fn query_ray_internal(
        &self,
        origin: Point3<f32>,
        dir: Vector3<f32>,
        max_distance: f32,
        opts: &QueryOptions,
    ) -> Result<Vec<SpatialHit>> {
        let d = if dir.norm() > 0.0 {
            dir.normalize()
        } else {
            Vector3::new(1.0, 0.0, 0.0)
        };
        let end = origin + d * max_distance;
        let seg_bb = Aabb::new(origin.inf(&end), origin.sup(&end));
        let mut local_opts = opts.clone();
        local_opts.include_bbox = true;
        // 先不过滤 limit，避免丢失更近的对象
        local_opts.limit = None;
        let mut hits = self.query_intersect_advanced(&seg_bb, &local_opts)?;
        for h in hits.iter_mut() {
            if let Some(bb) = &h.bbox {
                h.distance = ray_aabb_toi(origin, d, bb, max_distance);
            }
        }
        hits.retain(|h| h.distance.is_some());
        hits.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(limit) = opts.limit {
            if hits.len() > limit {
                hits.truncate(limit);
            }
        }
        Ok(hits)
    }

    /// Get AABB by refno
    #[cfg(feature = "sqlite-index")]
    pub fn get_aabb(&self, refno: RefU64) -> Result<Option<Aabb>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare_cached(
            "SELECT min_x, max_x, min_y, max_y, min_z, max_z 
             FROM aabb_index WHERE id = ?1",
        )?;

        let result = stmt
            .query_row(params![refno.0 as i64], |row| {
                let min_x: f64 = row.get(0)?;
                let max_x: f64 = row.get(1)?;
                let min_y: f64 = row.get(2)?;
                let max_y: f64 = row.get(3)?;
                let min_z: f64 = row.get(4)?;
                let max_z: f64 = row.get(5)?;

                Ok(Aabb::new(
                    [min_x as f32, min_y as f32, min_z as f32].into(),
                    [max_x as f32, max_y as f32, max_z as f32].into(),
                ))
            })
            .optional()?;

        Ok(result)
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn get_aabb(&self, _refno: RefU64) -> Result<Option<Aabb>> {
        Ok(None)
    }

    /// Delete an AABB
    #[cfg(feature = "sqlite-index")]
    pub fn delete_aabb(&self, refno: RefU64) -> Result<()> {
        let conn = self.get_connection()?;
        let mut tx = conn.unchecked_transaction()?;

        tx.execute(
            "DELETE FROM aabb_index WHERE id = ?1",
            params![refno.0 as i64],
        )?;
        tx.execute("DELETE FROM items WHERE id = ?1", params![refno.0 as i64])?;

        tx.commit()?;
        Ok(())
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn delete_aabb(&self, _refno: RefU64) -> Result<()> {
        Ok(())
    }

    /// Clear all data
    #[cfg(feature = "sqlite-index")]
    pub fn clear(&self) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute_batch("DELETE FROM aabb_index; DELETE FROM items;")?;
        Ok(())
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn clear(&self) -> Result<()> {
        Ok(())
    }

    /// Get statistics about the index
    #[cfg(feature = "sqlite-index")]
    pub fn get_stats(&self) -> Result<IndexStats> {
        let conn = self.get_connection()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM aabb_index", [], |row| row.get(0))?;

        Ok(IndexStats {
            total_elements: count as usize,
            index_type: "SQLite R*-tree".to_string(),
        })
    }

    #[cfg(not(feature = "sqlite-index"))]
    pub fn get_stats(&self) -> Result<IndexStats> {
        Ok(IndexStats {
            total_elements: 0,
            index_type: "Disabled".to_string(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub total_elements: usize,
    pub index_type: String,
}

/// Thread-safe wrapper for SqliteSpatialIndex
#[derive(Clone)]
pub struct SharedSpatialIndex {
    inner: Arc<SqliteSpatialIndex>,
}

impl SharedSpatialIndex {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(SqliteSpatialIndex::new(path)?),
        })
    }

    pub fn with_default_path() -> Result<Self> {
        Ok(Self {
            inner: Arc::new(SqliteSpatialIndex::with_default_path()?),
        })
    }
}

impl std::ops::Deref for SharedSpatialIndex {
    type Target = SqliteSpatialIndex;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(feature = "sqlite-index")]
impl SpatialQueryBackend for SqliteSpatialIndex {
    fn query_intersect_hits(&self, query: &Aabb, opts: &QueryOptions) -> Result<Vec<SpatialHit>> {
        self.query_intersect_advanced(query, opts)
    }

    fn query_contains_hits(&self, query: &Aabb, opts: &QueryOptions) -> Result<Vec<SpatialHit>> {
        self.query_contains_advanced(query, opts)
    }

    fn query_nearest_to_point(
        &self,
        point: Point3<f32>,
        k: usize,
        search_radius: Option<f32>,
        opts: &QueryOptions,
    ) -> Result<Vec<SpatialHit>> {
        self.query_knn_point(point, k, search_radius, opts)
    }

    fn query_ray_hits(
        &self,
        origin: Point3<f32>,
        dir: Vector3<f32>,
        max_distance: f32,
        opts: &QueryOptions,
    ) -> Result<Vec<SpatialHit>> {
        self.query_ray_internal(origin, dir, max_distance, opts)
    }
}

/// 点到 AABB 的最短距离
fn distance_point_aabb(p: Point3<f32>, bb: &Aabb) -> f32 {
    let dx = if p.x < bb.mins.x {
        bb.mins.x - p.x
    } else if p.x > bb.maxs.x {
        p.x - bb.maxs.x
    } else {
        0.0
    };
    let dy = if p.y < bb.mins.y {
        bb.mins.y - p.y
    } else if p.y > bb.maxs.y {
        p.y - bb.maxs.y
    } else {
        0.0
    };
    let dz = if p.z < bb.mins.z {
        bb.mins.z - p.z
    } else if p.z > bb.maxs.z {
        p.z - bb.maxs.z
    } else {
        0.0
    };
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// 射线与 AABB 最近命中距离（0..=max_distance），无命中返回 None
fn ray_aabb_toi(
    origin: Point3<f32>,
    dir: Vector3<f32>,
    bb: &Aabb,
    max_distance: f32,
) -> Option<f32> {
    let mut tmin = f32::NEG_INFINITY;
    let mut tmax = f32::INFINITY;

    // X axis
    if dir.x != 0.0 {
        let inv = 1.0 / dir.x;
        let t1 = (bb.mins.x - origin.x) * inv;
        let t2 = (bb.maxs.x - origin.x) * inv;
        tmin = tmin.max(t1.min(t2));
        tmax = tmax.min(t1.max(t2));
    } else {
        // 平行于 X 轴，原点必须落在 slab 内
        if origin.x < bb.mins.x || origin.x > bb.maxs.x {
            return None;
        }
    }

    // Y axis
    if dir.y != 0.0 {
        let inv = 1.0 / dir.y;
        let t1 = (bb.mins.y - origin.y) * inv;
        let t2 = (bb.maxs.y - origin.y) * inv;
        tmin = tmin.max(t1.min(t2));
        tmax = tmax.min(t1.max(t2));
    } else {
        if origin.y < bb.mins.y || origin.y > bb.maxs.y {
            return None;
        }
    }

    // Z axis
    if dir.z != 0.0 {
        let inv = 1.0 / dir.z;
        let t1 = (bb.mins.z - origin.z) * inv;
        let t2 = (bb.maxs.z - origin.z) * inv;
        tmin = tmin.max(t1.min(t2));
        tmax = tmax.min(t1.max(t2));
    } else {
        if origin.z < bb.mins.z || origin.z > bb.maxs.z {
            return None;
        }
    }

    if tmax < 0.0 {
        return None;
    }
    let t = if tmin >= 0.0 { tmin } else { tmax };
    if t >= 0.0 && t <= max_distance {
        Some(t)
    } else {
        None
    }
}
