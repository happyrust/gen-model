use std::path::{Path, PathBuf};

use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::types::RefU64;
use aios_core::{RefnoEnum, RefnoSesno};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
use pdms_io::io::PdmsIO;

use config as cfg;
use rusqlite::{Connection, Result as SqlResult, Row, Statement, params};

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct StoredAabb {
    mins: [f32; 3],
    maxs: [f32; 3],
}

// 版本化的存储结构
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct VersionedStoredAabb {
    refno_value: u64,
    session: u32,
    mins: [f32; 3],
    maxs: [f32; 3],
    created_at: u64,
    updated_at: u64,
}

// 时间数据存储结构
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct RefnoTimeData {
    pub refno_value: u64,
    pub session: u32,
    pub dbnum: u32,
    pub created_at: u64,             // 创建时间戳
    pub updated_at: u64,             // 更新时间戳
    pub sesno_timestamp: u64,        // sesno 对应的实际时间
    pub author: Option<String>,      // 创建者
    pub description: Option<String>, // 变更描述
}

// 时间映射结构
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SesnoTimeMapping {
    pub dbnum: u32,
    pub sesno: u32,
    pub timestamp: u64,
    pub description: Option<String>,
}

// 缓存统计信息
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub ref_bbox_count: u64,
    pub versioned_count: u64,
    pub time_data_count: u64,
    pub sesno_mapping_count: u64,
}

impl From<Aabb> for StoredAabb {
    fn from(a: Aabb) -> Self {
        Self {
            mins: [a.mins.x, a.mins.y, a.mins.z],
            maxs: [a.maxs.x, a.maxs.y, a.maxs.z],
        }
    }
}

impl From<&StoredAabb> for Aabb {
    fn from(v: &StoredAabb) -> Self {
        Aabb::new(v.mins.into(), v.maxs.into())
    }
}

// PDMS 时间数据提取器
pub struct PdmsTimeExtractor {
    pdms_io: PdmsIO,
    dbnum: u32,
}

impl PdmsTimeExtractor {
    /// 创建新的时间数据提取器
    pub fn new(pdms_io: PdmsIO, dbnum: u32) -> Self {
        Self { pdms_io, dbnum }
    }

    /// 提取指定 sesno 的时间信息
    pub fn extract_time_data(&self, sesno: u32) -> Option<u64> {
        // 这里需要根据实际的 PDMS 数据格式来提取时间信息
        // 暂时返回一个基于 sesno 的模拟时间戳
        let base_timestamp = 1609459200; // 2021-01-01 00:00:00 UTC
        Some(base_timestamp + (sesno as u64 * 86400)) // 每个 sesno 间隔一天
    }

    /// 批量提取多个 sesno 的时间信息
    pub fn extract_batch(&self, sesnos: &[u32]) -> Vec<(u32, u64)> {
        sesnos
            .iter()
            .filter_map(|&sesno| self.extract_time_data(sesno).map(|ts| (sesno, ts)))
            .collect()
    }

    /// 获取数据库中的最大 sesno
    pub fn get_max_sesno(&self) -> Option<u32> {
        // 实际实现需要查询 PDMS 数据
        None
    }
}

// A dependency (dependent) refno for a given geo_hash.
pub type Dep = RefU64;

// Map type: geo_hash -> List of refnos who depend on that geo_hash.
pub type DepsForGeo = Vec<Dep>;

// Map type: refno -> List of geo_hashes that this refno needs.
pub type GeosForRef = Vec<String>;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct StoredRStarBBox {
    aabb: StoredAabb,
    refno: u64,
    noun: String,
}

impl From<&RStarBoundingBox> for StoredRStarBBox {
    fn from(v: &RStarBoundingBox) -> Self {
        Self {
            aabb: v.aabb.into(),
            refno: v.refno.0,
            noun: v.noun.clone(),
        }
    }
}

impl From<&StoredRStarBBox> for RStarBoundingBox {
    fn from(v: &StoredRStarBBox) -> Self {
        RStarBoundingBox::new((&v.aabb).into(), RefU64(v.refno).into(), v.noun.clone())
    }
}

pub struct AabbCache {
    db_path: PathBuf,
}

impl AabbCache {
    pub fn open_with_path<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(dir) = db_path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
            }
        }

        // Initialize the database schema
        let cache = Self { db_path };
        cache.init_schema()?;
        Ok(cache)
    }

    pub fn open_default() -> anyhow::Result<Self> {
        let db_path = Path::new("assets").join("aabb_cache.sqlite");
        Self::open_with_path(db_path)
    }

    fn get_connection(&self) -> SqlResult<Connection> {
        let conn = Connection::open(&self.db_path)?;
        Self::configure_connection(&conn)?;
        Ok(conn)
    }

    fn configure_connection(conn: &Connection) -> SqlResult<()> {
        conn.pragma_update(None, "journal_mode", &"WAL")?;
        conn.pragma_update(None, "synchronous", &"NORMAL")?;
        conn.pragma_update(None, "cache_size", &10000)?;
        Ok(())
    }

    fn init_schema(&self) -> anyhow::Result<()> {
        let conn = self.get_connection()?;

        // Create main tables
        conn.execute_batch(
            r#"
            -- Main AABB storage
            CREATE TABLE IF NOT EXISTS ref_bbox (
                refno INTEGER PRIMARY KEY,
                data BLOB NOT NULL
            );

            -- Geometry AABB storage
            CREATE TABLE IF NOT EXISTS geo_aabb (
                geo_hash TEXT PRIMARY KEY,
                data BLOB NOT NULL
            );

            -- Dependencies by reference
            CREATE TABLE IF NOT EXISTS deps_by_ref (
                refno INTEGER PRIMARY KEY,
                data BLOB NOT NULL
            );

            -- References by geometry
            CREATE TABLE IF NOT EXISTS refs_by_geo (
                geo_hash TEXT PRIMARY KEY,
                data BLOB NOT NULL
            );

            -- Versioned AABB storage
            CREATE TABLE IF NOT EXISTS versioned_ref_bbox (
                refno_key TEXT NOT NULL,
                session INTEGER NOT NULL,
                data BLOB NOT NULL,
                PRIMARY KEY (refno_key, session)
            );

            -- Time data storage
            CREATE TABLE IF NOT EXISTS refno_time_data (
                refno_key TEXT NOT NULL,
                session INTEGER NOT NULL,
                data BLOB NOT NULL,
                PRIMARY KEY (refno_key, session)
            );

            -- Session time mapping
            CREATE TABLE IF NOT EXISTS sesno_time_mapping (
                dbnum INTEGER NOT NULL,
                sesno INTEGER NOT NULL,
                timestamp INTEGER NOT NULL,
                description TEXT,
                PRIMARY KEY (dbnum, sesno)
            );

            -- 3D AABB RTree for spatial indexing
            CREATE VIRTUAL TABLE IF NOT EXISTS aabb_index USING rtree(
                id, min_x, max_x, min_y, max_y, min_z, max_z
            );

            -- Create indexes for better performance
            CREATE INDEX IF NOT EXISTS idx_versioned_refno ON versioned_ref_bbox(refno_key);
            CREATE INDEX IF NOT EXISTS idx_time_data_refno ON refno_time_data(refno_key);
            "#,
        )?;

        Ok(())
    }

    // ---------- SQLite RTree integration ----------
    pub fn sqlite_is_enabled() -> bool {
        // Read from DbOption.toml if present; default false
        let mut s = cfg::Config::builder();
        if Path::new("DbOption.toml").exists() {
            s = s.add_source(cfg::File::with_name("DbOption"));
        }
        let s = s.build();
        if let Ok(conf) = s {
            conf.get_bool("sqlite_index_enabled").unwrap_or(false)
        } else {
            false
        }
    }

    fn sqlite_default_path() -> PathBuf {
        // Allow override via DbOption.toml key: sqlite_index_path
        if let Ok(built) = cfg::Config::builder()
            .add_source(cfg::File::with_name("DbOption"))
            .build()
        {
            if let Ok(path) = built.get_string("sqlite_index_path") {
                if !path.is_empty() {
                    return PathBuf::from(path);
                }
            }
        }
        Path::new("assets").join("aabb_index.sqlite")
    }

    /// Rebuild SQLite RTree from internal storage
    pub fn sqlite_rebuild_from_internal(&self) -> anyhow::Result<usize> {
        let conn = self.get_connection()?;

        // Clear existing RTree data
        conn.execute("DELETE FROM aabb_index", [])?;

        // Load all ref_bbox data and insert into RTree
        let mut stmt = conn.prepare("SELECT refno, data FROM ref_bbox")?;
        let mut insert_stmt = conn.prepare(
            "INSERT INTO aabb_index (id, min_x, max_x, min_y, max_y, min_z, max_z) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        )?;

        let mut count = 0;
        let rows = stmt.query_map([], |row: &Row| {
            let refno: i64 = row.get(0)?;
            let data: Vec<u8> = row.get(1)?;
            Ok((refno, data))
        })?;

        for row in rows {
            let (refno, data) = row?;
            if let Ok(stored) = bincode::deserialize::<StoredRStarBBox>(&data) {
                let aabb = &stored.aabb;
                insert_stmt.execute(params![
                    refno,
                    aabb.mins[0] as f64,
                    aabb.maxs[0] as f64,
                    aabb.mins[1] as f64,
                    aabb.maxs[1] as f64,
                    aabb.mins[2] as f64,
                    aabb.maxs[2] as f64,
                ])?;
                count += 1;
            }
        }

        Ok(count)
    }

    /// Intersect query against SQLite RTree; returns refnos
    pub fn sqlite_query_intersect(&self, query: &Aabb) -> anyhow::Result<Vec<RefU64>> {
        let conn = self.get_connection()?;

        let minx = query.mins.x as f64;
        let maxx = query.maxs.x as f64;
        let miny = query.mins.y as f64;
        let maxy = query.maxs.y as f64;
        let minz = query.mins.z as f64;
        let maxz = query.maxs.z as f64;

        let mut stmt = conn.prepare(
            "SELECT id FROM aabb_index WHERE max_x >= ?1 AND min_x <= ?2 AND max_y >= ?3 AND min_y <= ?4 AND max_z >= ?5 AND min_z <= ?6"
        )?;

        let ids = stmt
            .query_map(params![minx, maxx, miny, maxy, minz, maxz], |row: &Row| {
                let id: i64 = row.get(0)?;
                Ok(RefU64(id as u64))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ids)
    }

    /// Get AABB by refno from SQLite RTree row
    pub fn sqlite_get_aabb(&self, refno: RefU64) -> anyhow::Result<Option<Aabb>> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT min_x, max_x, min_y, max_y, min_z, max_z FROM aabb_index WHERE id=?1",
        )?;

        let result = stmt.query_row(params![refno.0 as i64], |row: &Row| {
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
        });

        match result {
            Ok(aabb) => Ok(Some(aabb)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ---------- Basic CRUD operations ----------

    pub fn get_geo_aabb(&self, geo_hash: &str) -> Option<Aabb> {
        let conn: Connection = self.get_connection().ok()?;
        let mut stmt: Statement = conn
            .prepare("SELECT data FROM geo_aabb WHERE geo_hash = ?1")
            .ok()?;
        let data: Vec<u8> = stmt
            .query_row(params![geo_hash], |row: &Row| row.get(0))
            .ok()?;
        let stored: StoredAabb = bincode::deserialize(&data).ok()?;
        Some((&stored).into())
    }

    pub fn put_geo_aabb(&self, geo_hash: &str, aabb: &Aabb) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        let stored: StoredAabb = aabb.clone().into();
        let bytes = bincode::serialize(&stored)?;
        conn.execute(
            "INSERT OR REPLACE INTO geo_aabb (geo_hash, data) VALUES (?1, ?2)",
            params![geo_hash, bytes],
        )?;
        Ok(())
    }

    pub fn get_deps_for_geo(&self, geo_hash: &str) -> Option<DepsForGeo> {
        let conn: Connection = self.get_connection().ok()?;
        let mut stmt: Statement = conn
            .prepare("SELECT data FROM refs_by_geo WHERE geo_hash = ?1")
            .ok()?;
        let data: Vec<u8> = stmt
            .query_row(params![geo_hash], |row: &Row| row.get(0))
            .ok()?;
        bincode::deserialize(&data).ok()
    }

    pub fn put_deps_for_geo(&self, geo_hash: &str, deps: &DepsForGeo) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        let bytes = bincode::serialize(deps)?;
        conn.execute(
            "INSERT OR REPLACE INTO refs_by_geo (geo_hash, data) VALUES (?1, ?2)",
            params![geo_hash, bytes],
        )?;
        Ok(())
    }

    pub fn get_geos_for_ref(&self, refno: RefU64) -> Option<GeosForRef> {
        let conn: Connection = self.get_connection().ok()?;
        let mut stmt: Statement = conn
            .prepare("SELECT data FROM deps_by_ref WHERE refno = ?1")
            .ok()?;
        let data: Vec<u8> = stmt
            .query_row(params![refno.0], |row: &Row| row.get(0))
            .ok()?;
        bincode::deserialize(&data).ok()
    }

    pub fn put_geos_for_ref(&self, refno: RefU64, geos: &GeosForRef) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        let bytes = bincode::serialize(geos)?;
        conn.execute(
            "INSERT OR REPLACE INTO deps_by_ref (refno, data) VALUES (?1, ?2)",
            params![refno.0, bytes],
        )?;
        Ok(())
    }

    pub fn get_ref_bbox(&self, refno: RefU64) -> Option<RStarBoundingBox> {
        let conn: Connection = self.get_connection().ok()?;
        let mut stmt: Statement = conn
            .prepare("SELECT data FROM ref_bbox WHERE refno = ?1")
            .ok()?;
        let data: Vec<u8> = stmt
            .query_row(params![refno.0], |row: &Row| row.get(0))
            .ok()?;
        let stored: StoredRStarBBox = bincode::deserialize(&data).ok()?;
        Some((&stored).into())
    }

    pub fn put_ref_bbox(&self, bbox: &RStarBoundingBox) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        let stored: StoredRStarBBox = bbox.into();
        let bytes = bincode::serialize(&stored)?;

        // Store in main table
        conn.execute(
            "INSERT OR REPLACE INTO ref_bbox (refno, data) VALUES (?1, ?2)",
            params![stored.refno, bytes],
        )?;

        // Also update RTree index
        let aabb = &stored.aabb;
        conn.execute(
            "INSERT OR REPLACE INTO aabb_index (id, min_x, max_x, min_y, max_y, min_z, max_z) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                stored.refno as i64,
                aabb.mins[0] as f64,
                aabb.maxs[0] as f64,
                aabb.mins[1] as f64,
                aabb.maxs[1] as f64,
                aabb.mins[2] as f64,
                aabb.maxs[2] as f64,
            ],
        )?;

        Ok(())
    }

    pub fn remove_ref_bbox(&self, refno: RefU64) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM ref_bbox WHERE refno = ?1", params![refno.0])?;
        conn.execute(
            "DELETE FROM aabb_index WHERE id = ?1",
            params![refno.0 as i64],
        )?;
        Ok(())
    }

    // ---------- Versioned methods ----------

    pub fn put_ref_bbox_versioned(
        &self,
        bbox: &RStarBoundingBox,
        session: u32,
    ) -> anyhow::Result<()> {
        let refno_enum = if session == 0 {
            RefnoEnum::Refno(bbox.refno)
        } else {
            RefnoEnum::from(RefnoSesno::new(bbox.refno, session))
        };

        let refno_key = refno_enum.to_string();
        let versioned = VersionedStoredAabb {
            refno_value: bbox.refno.0,
            session,
            mins: [bbox.aabb.mins.x, bbox.aabb.mins.y, bbox.aabb.mins.z],
            maxs: [bbox.aabb.maxs.x, bbox.aabb.maxs.y, bbox.aabb.maxs.z],
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        let bytes = bincode::serialize(&versioned)?;
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO versioned_ref_bbox (refno_key, session, data) VALUES (?1, ?2, ?3)",
            params![refno_key, session, bytes],
        )?;

        Ok(())
    }

    pub fn get_ref_bbox_at_session(&self, refno: RefU64, session: u32) -> Option<RStarBoundingBox> {
        let refno_enum = if session == 0 {
            RefnoEnum::Refno(refno)
        } else {
            RefnoEnum::from(RefnoSesno::new(refno, session))
        };

        let refno_key = refno_enum.to_string();
        let conn: Connection = self.get_connection().ok()?;
        let mut stmt: Statement = conn
            .prepare("SELECT data FROM versioned_ref_bbox WHERE refno_key = ?1 AND session = ?2")
            .ok()?;

        let data: Vec<u8> = stmt
            .query_row(params![refno_key, session], |row: &Row| row.get(0))
            .ok()?;
        let versioned: VersionedStoredAabb = bincode::deserialize(&data).ok()?;

        let aabb = Aabb::new(versioned.mins.into(), versioned.maxs.into());
        Some(RStarBoundingBox::new(aabb, refno_enum, String::new()))
    }

    pub fn get_ref_bbox_history(&self, refno: RefU64) -> Vec<(u32, RStarBoundingBox)> {
        let mut results = Vec::new();

        if let Ok(conn) = self.get_connection() {
            // Query for all sessions of this refno
            let query = "SELECT session, data FROM versioned_ref_bbox WHERE refno_key LIKE ?1 ORDER BY session";
            if let Ok(mut stmt) = conn.prepare(query) {
                let prefix = format!("{}%", refno.0);
                if let Ok(rows) = stmt.query_map(params![prefix], |row: &Row| {
                    let session: u32 = row.get(0)?;
                    let data: Vec<u8> = row.get(1)?;
                    Ok((session, data))
                }) {
                    for row in rows {
                        if let Ok((session, data)) = row {
                            if let Ok(versioned) =
                                bincode::deserialize::<VersionedStoredAabb>(&data)
                            {
                                let aabb = Aabb::new(versioned.mins.into(), versioned.maxs.into());
                                let refno_enum = if session == 0 {
                                    RefnoEnum::Refno(refno)
                                } else {
                                    RefnoEnum::from(RefnoSesno::new(refno, session))
                                };
                                results.push((
                                    session,
                                    RStarBoundingBox::new(aabb, refno_enum, String::new()),
                                ));
                            }
                        }
                    }
                }
            }
        }

        results
    }

    pub fn get_ref_bbox_latest(&self, refno: RefU64) -> Option<RStarBoundingBox> {
        // First try to get current version (session = 0)
        if let Some(bbox) = self.get_ref_bbox_at_session(refno, 0) {
            return Some(bbox);
        }

        // Otherwise get the highest session version
        let history = self.get_ref_bbox_history(refno);
        history
            .into_iter()
            .max_by_key(|(session, _)| *session)
            .map(|(_, bbox)| bbox)
    }

    // ---------- Time data methods ----------

    pub fn put_refno_time_data(&self, time_data: &RefnoTimeData) -> anyhow::Result<()> {
        let refno_enum = if time_data.session == 0 {
            RefnoEnum::Refno(RefU64(time_data.refno_value))
        } else {
            RefnoEnum::from(RefnoSesno::new(
                RefU64(time_data.refno_value),
                time_data.session,
            ))
        };

        let refno_key = refno_enum.to_string();
        let bytes = bincode::serialize(time_data)?;
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO refno_time_data (refno_key, session, data) VALUES (?1, ?2, ?3)",
            params![refno_key, time_data.session, bytes],
        )?;

        Ok(())
    }

    pub fn get_refno_time_data(&self, refno: RefU64, session: u32) -> Option<RefnoTimeData> {
        let refno_enum = if session == 0 {
            RefnoEnum::Refno(refno)
        } else {
            RefnoEnum::from(RefnoSesno::new(refno, session))
        };

        let refno_key = refno_enum.to_string();
        let conn: Connection = self.get_connection().ok()?;
        let mut stmt: Statement = conn
            .prepare("SELECT data FROM refno_time_data WHERE refno_key = ?1 AND session = ?2")
            .ok()?;

        let data: Vec<u8> = stmt
            .query_row(params![refno_key, session], |row: &Row| row.get(0))
            .ok()?;
        bincode::deserialize(&data).ok()
    }

    pub fn put_sesno_time_mapping(&self, mapping: &SesnoTimeMapping) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO sesno_time_mapping (dbnum, sesno, timestamp, description) VALUES (?1, ?2, ?3, ?4)",
            params![mapping.dbnum, mapping.sesno, mapping.timestamp, mapping.description],
        )?;
        Ok(())
    }

    pub fn get_sesno_time_mapping(&self, dbnum: u32, sesno: u32) -> Option<SesnoTimeMapping> {
        let conn: Connection = self.get_connection().ok()?;
        let mut stmt: Statement = conn.prepare(
            "SELECT timestamp, description FROM sesno_time_mapping WHERE dbnum = ?1 AND sesno = ?2"
        ).ok()?;

        let result: SesnoTimeMapping = stmt
            .query_row(params![dbnum, sesno], |row: &Row| {
                Ok(SesnoTimeMapping {
                    dbnum,
                    sesno,
                    timestamp: row.get(0)?,
                    description: row.get(1)?,
                })
            })
            .ok()?;

        Some(result)
    }

    // ---------- Utility methods ----------

    pub fn get_stats(&self) -> anyhow::Result<CacheStats> {
        let conn = self.get_connection()?;

        let ref_bbox_count: u64 =
            conn.query_row("SELECT COUNT(*) FROM ref_bbox", [], |row: &Row| row.get(0))?;

        let versioned_count: u64 = conn.query_row(
            "SELECT COUNT(*) FROM versioned_ref_bbox",
            [],
            |row: &Row| row.get(0),
        )?;

        let time_data_count: u64 =
            conn.query_row("SELECT COUNT(*) FROM refno_time_data", [], |row: &Row| {
                row.get(0)
            })?;

        let sesno_mapping_count: u64 = conn.query_row(
            "SELECT COUNT(*) FROM sesno_time_mapping",
            [],
            |row: &Row| row.get(0),
        )?;

        Ok(CacheStats {
            ref_bbox_count,
            versioned_count,
            time_data_count,
            sesno_mapping_count,
        })
    }

    pub fn clear_all(&self) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        conn.execute_batch(
            r#"
            DELETE FROM ref_bbox;
            DELETE FROM geo_aabb;
            DELETE FROM deps_by_ref;
            DELETE FROM refs_by_geo;
            DELETE FROM versioned_ref_bbox;
            DELETE FROM refno_time_data;
            DELETE FROM sesno_time_mapping;
            DELETE FROM aabb_index;
            "#,
        )?;
        Ok(())
    }

    pub fn load_all_ref_bboxes(&self) -> anyhow::Result<Vec<RStarBoundingBox>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare("SELECT data FROM ref_bbox")?;
        let rows = stmt.query_map([], |row: &Row| {
            let data: Vec<u8> = row.get(0)?;
            Ok(data)
        })?;

        let mut out = Vec::new();
        for row in rows {
            let data = row?;
            if let Ok(stored) = bincode::deserialize::<StoredRStarBBox>(&data) {
                out.push((&stored).into());
            }
        }

        Ok(out)
    }

    pub fn warm_from_pdms_time(&mut self, extractor: &PdmsTimeExtractor) -> anyhow::Result<()> {
        // Extract time data for all known sessions
        if let Some(max_sesno) = extractor.get_max_sesno() {
            for sesno in 1..=max_sesno {
                if let Some(timestamp) = extractor.extract_time_data(sesno) {
                    let mapping = SesnoTimeMapping {
                        dbnum: extractor.dbnum,
                        sesno,
                        timestamp,
                        description: Some(format!(
                            "Auto-extracted from PDMS DB {}",
                            extractor.dbnum
                        )),
                    };
                    self.put_sesno_time_mapping(&mapping)?;
                }
            }
        }
        Ok(())
    }

    pub fn get_all_geo_hashes(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare("SELECT geo_hash FROM geo_aabb")?;
        let rows = stmt.query_map([], |row: &Row| {
            let hash: String = row.get(0)?;
            Ok(hash)
        })?;

        let mut hashes = Vec::new();
        for row in rows {
            hashes.push(row?);
        }

        Ok(hashes)
    }

    pub fn exists_ref_bbox(&self, refno: RefU64) -> bool {
        if let Ok(conn) = self.get_connection() {
            if let Ok(mut stmt) = conn.prepare("SELECT 1 FROM ref_bbox WHERE refno = ?1 LIMIT 1") {
                return stmt.exists(params![refno.0]).unwrap_or(false);
            }
        }
        false
    }

    pub fn count_ref_bboxes(&self) -> usize {
        if let Ok(conn) = self.get_connection() {
            if let Ok(count) = conn.query_row("SELECT COUNT(*) FROM ref_bbox", [], |row: &Row| {
                let c: i64 = row.get(0)?;
                Ok(c as usize)
            }) {
                return count;
            }
        }
        0
    }

    // Compatibility method for existing code
    pub fn sqlite_rebuild_from_redb(&self) -> anyhow::Result<usize> {
        self.sqlite_rebuild_from_internal()
    }
}

// DEPRECATED: Global AABB_CACHE is replaced by SqliteSpatialIndex
// pub static AABB_CACHE: Lazy<AabbCache> = Lazy::new(|| {
//     AabbCache::open_default().expect("Failed to open AABB cache")
// });

// DEPRECATED: These tests are for the old AABB_CACHE implementation
// #[cfg(test)]
// #[ignore]
// mod tests {
//     use super::*;
//     use glam::Vec3;
//
// //     #[test]
// //     fn test_aabb_cache_basic_operations() {
// //         let temp_dir = tempfile::tempdir().expect("create temp dir");
// //         let cache_path = temp_dir.path().join("test_cache.sqlite");
// //         let cache = AabbCache::open_with_path(&cache_path).expect("open cache");
//
//         // Test ref_bbox operations
//         let refno = RefU64(12345);
//         let aabb = Aabb::new(
//             Vec3::new(0.0, 0.0, 0.0).into(),
//             Vec3::new(10.0, 10.0, 10.0).into()
//         );
//         let bbox = RStarBoundingBox::new(aabb, refno.into(), "test_element".to_string());
//
//         cache.put_ref_bbox(&bbox).expect("put ref bbox");
//         let retrieved = cache.get_ref_bbox(refno).expect("get ref bbox");
//         assert_eq!(retrieved.refno, bbox.refno);
//
//         // Test geo_aabb operations
//         let geo_hash = "test_geo_hash";
//         cache.put_geo_aabb(geo_hash, &aabb).expect("put geo aabb");
//         let retrieved_aabb = cache.get_geo_aabb(geo_hash).expect("get geo aabb");
//         assert_eq!(retrieved_aabb.mins, aabb.mins);
//         assert_eq!(retrieved_aabb.maxs, aabb.maxs);
//     }
//
//     #[test]
//     fn test_versioned_operations() {
// //         let temp_dir = tempfile::tempdir().expect("create temp dir");
// //         let cache_path = temp_dir.path().join("test_versioned.sqlite");
// //         let cache = AabbCache::open_with_path(&cache_path).expect("open cache");
// //
// //         let refno = RefU64(54321);
//
//         // Store multiple versions
//         for session in 0..3 {
//             let aabb = Aabb::new(
//                 Vec3::new(session as f32, 0.0, 0.0).into(),
//                 Vec3::new(session as f32 + 10.0, 10.0, 10.0).into()
//             );
//             let bbox = RStarBoundingBox::new(aabb, refno.into(), format!("version_{}", session));
//             cache.put_ref_bbox_versioned(&bbox, session).expect("put versioned");
//         }
//
//         // Retrieve specific version
//         let version_1 = cache.get_ref_bbox_at_session(refno, 1).expect("get session 1");
//         assert_eq!(version_1.aabb.mins.x, 1.0);
//
//         // Get history
//         let history = cache.get_ref_bbox_history(refno);
//         assert!(history.len() >= 3);
//     }
//
//     #[test]
//     fn test_spatial_query() {
// //         let temp_dir = tempfile::tempdir().expect("create temp dir");
// //         let cache_path = temp_dir.path().join("test_spatial.sqlite");
// //         let cache = AabbCache::open_with_path(&cache_path).expect("open cache");
// //
// //         // Insert multiple elements
//         for i in 0..5 {
//             let refno = RefU64(1000 + i);
//             let aabb = Aabb::new(
//                 Vec3::new(i as f32 * 10.0, 0.0, 0.0).into(),
//                 Vec3::new(i as f32 * 10.0 + 5.0, 5.0, 5.0).into()
//             );
//             let bbox = RStarBoundingBox::new(aabb, refno.into(), format!("element_{}", i));
//             cache.put_ref_bbox(&bbox).expect("put ref bbox");
//         }
//
//         // Query intersecting
//         let query_aabb = Aabb::new(
//             Vec3::new(12.0, 0.0, 0.0).into(),
//             Vec3::new(18.0, 5.0, 5.0).into()
//         );
//
//         let results = cache.sqlite_query_intersect(&query_aabb).expect("query intersect");
//         assert!(!results.is_empty());
//     }
// }
