use std::path::{Path, PathBuf};

#[cfg(feature = "sqlite-index")]
use rusqlite::{params, Connection, Result};

// Minimal SQLite-based AABB index using the SQLite RTree virtual table.
// This module is feature-gated behind `sqlite-index` and can be integrated
// incrementally without impacting existing backends.
pub struct SqliteAabbIndex {
    path: PathBuf,
}

#[cfg(feature = "sqlite-index")]
impl SqliteAabbIndex {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let this = SqliteAabbIndex { path: path.as_ref().to_path_buf() };
        let conn = Connection::open(&this.path)?;
        Self::configure(&conn)?;
        drop(conn);
        Ok(this)
    }

    fn configure(conn: &Connection) -> Result<()> {
        // WAL for multi-reader concurrency; NORMAL sync for performance.
        conn.pragma_update(None, "journal_mode", &"WAL")?;
        conn.pragma_update(None, "synchronous", &"NORMAL")?;
        Ok(())
    }

    pub fn init_schema(&self) -> Result<()> {
        let conn = Connection::open(&self.path)?;
        Self::configure(&conn)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS items (
                id INTEGER PRIMARY KEY
            );
            -- 3D AABB RTree: id, [min_x, max_x], [min_y, max_y], [min_z, max_z]
            CREATE VIRTUAL TABLE IF NOT EXISTS aabb_index USING rtree(
                id, min_x, max_x, min_y, max_y, min_z, max_z
            );
            "#,
        )?;
        Ok(())
    }

    // Batch insert/replace AABBs: (id, min_x, max_x, min_y, max_y, min_z, max_z)
    pub fn insert_many<I>(&self, iter: I) -> Result<usize>
    where
        I: IntoIterator<Item = (i64, f64, f64, f64, f64, f64, f64)>,
    {
        let conn = Connection::open(&self.path)?;
        Self::configure(&conn)?;
        let tx = conn.transaction()?;
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO aabb_index \
             (id, min_x, max_x, min_y, max_y, min_z, max_z) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut count = 0usize;
        for (id, minx, maxx, miny, maxy, minz, maxz) in iter {
            stmt.execute(params![id, minx, maxx, miny, maxy, minz, maxz])?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    // AABB intersection query: returns matching ids.
    pub fn query_intersect(
        &self,
        minx: f64,
        maxx: f64,
        miny: f64,
        maxy: f64,
        minz: f64,
        maxz: f64,
    ) -> Result<Vec<i64>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT id FROM aabb_index \
             WHERE min_x <= ?2 AND max_x >= ?1 \
               AND min_y <= ?4 AND max_y >= ?3 \
               AND min_z <= ?6 AND max_z >= ?5",
        )?;
        let rows = stmt.query_map((minx, maxx, miny, maxy, minz, maxz), |row| row.get::<_, i64>(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    }

    // Optional: range query on X for scanning.
    pub fn query_range_x(&self, minx: f64, maxx: f64) -> Result<Vec<i64>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT id FROM aabb_index \
             WHERE min_x <= ?2 AND max_x >= ?1",
        )?;
        let rows = stmt.query_map((minx, maxx), |row| row.get::<_, i64>(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    }

    // Query all AABBs: returns all (id, min_x, max_x, min_y, max_y, min_z, max_z) tuples
    pub fn query_all_aabbs(&self) -> Result<Vec<(i64, f64, f64, f64, f64, f64, f64)>> {
        let conn = Connection::open(&self.path)?;
        let mut stmt = conn.prepare(
            "SELECT id, min_x, max_x, min_y, max_y, min_z, max_z FROM aabb_index"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
            ))
        })?;
        let mut aabbs = Vec::new();
        for r in rows {
            aabbs.push(r?);
        }
        Ok(aabbs)
    }
}

#[cfg(all(test, feature = "sqlite-index"))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn basic_intersect() {
        let path = "test_aabb.sqlite";
        let _ = fs::remove_file(path);
        let idx = SqliteAabbIndex::open(path).unwrap();
        idx.init_schema().unwrap();

        let data = vec![
            (1, 0.0, 10.0, 0.0, 5.0, -5.0, 5.0),
            (2, 5.0, 15.0, 2.0, 8.0, -2.0, 2.0),
            (3, 20.0, 30.0, -1.0, 1.0, 0.0, 1.0),
        ];
        idx.insert_many(data).unwrap();

        let ids = idx
            .query_intersect(4.0, 6.0, 1.0, 3.0, -1.0, 1.0)
            .unwrap();
        assert!(ids.contains(&1) && ids.contains(&2) && !ids.contains(&3));

        let _ = fs::remove_file(path);
    }
}
