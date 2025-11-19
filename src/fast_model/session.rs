use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use pdms_io::io::PdmsIO;
use rusqlite::{Connection, Error, Result as SqlResult, Row, params};

// 仅管理与"会话/版本"相关的数据结构与逻辑

pub struct SessionStore {
    db_path: PathBuf,
}

impl SessionStore {
    pub fn open_with_path<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(dir) = db_path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
            }
        }

        let store = Self { db_path };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_default() -> anyhow::Result<Self> {
        // 复用 aabb 缓存文件，避免重复文件
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
        Ok(())
    }

    fn init_schema(&self) -> anyhow::Result<()> {
        let conn = self.get_connection()?;

        // Create session time mapping table if not exists
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sesno_time_mapping (
                dbnum INTEGER NOT NULL,
                sesno INTEGER NOT NULL,
                timestamp INTEGER NOT NULL,
                description TEXT,
                PRIMARY KEY (dbnum, sesno)
            )",
            [],
        )?;

        // Create index for better performance
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sesno_dbnum ON sesno_time_mapping(dbnum)",
            [],
        )?;

        Ok(())
    }

    /// 读取某个 dbnum 在本地缓存中记录到的最大 sesno
    pub fn get_max_sesno_for_dbnum(&self, dbnum: u32) -> Option<u32> {
        let conn: Connection = self.get_connection().ok()?;
        let result: Result<u32, _> = conn.query_row(
            "SELECT MAX(sesno) FROM sesno_time_mapping WHERE dbnum = ?1",
            params![dbnum],
            |row: &Row| row.get(0),
        );
        result.ok()
    }

    /// 存储 sesno 时间映射
    pub fn put_sesno_time_mapping(
        &self,
        dbnum: u32,
        sesno: u32,
        timestamp: u64,
    ) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO sesno_time_mapping (dbnum, sesno, timestamp) VALUES (?1, ?2, ?3)",
            params![dbnum, sesno, timestamp],
        )?;
        Ok(())
    }

    /// 获取指定 sesno 的时间映射
    pub fn get_sesno_time_mapping(&self, dbnum: u32, sesno: u32) -> Option<u64> {
        let conn: Connection = self.get_connection().ok()?;
        let result: Result<u64, _> = conn.query_row(
            "SELECT timestamp FROM sesno_time_mapping WHERE dbnum = ?1 AND sesno = ?2",
            params![dbnum, sesno],
            |row: &Row| row.get(0),
        );
        result.ok()
    }

    /// 获取某个 dbnum 的所有 sesno 时间映射
    pub fn get_all_sesno_mappings(&self, dbnum: u32) -> Vec<(u32, u64)> {
        if let Ok(conn) = self.get_connection() {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT sesno, timestamp FROM sesno_time_mapping WHERE dbnum = ?1 ORDER BY sesno",
            ) {
                if let Ok(rows) =
                    stmt.query_map(params![dbnum], |row: &Row| -> Result<(u32, u64), Error> {
                        let sesno: u32 = row.get(0)?;
                        let timestamp: u64 = row.get(1)?;
                        Ok((sesno, timestamp))
                    })
                {
                    return rows
                        .filter_map(|r: Result<(u32, u64), Error>| r.ok())
                        .collect();
                }
            }
        }
        Vec::new()
    }

    /// 清除指定 dbnum 的所有时间映射
    pub fn clear_dbnum_mappings(&self, dbnum: u32) -> anyhow::Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "DELETE FROM sesno_time_mapping WHERE dbnum = ?1",
            params![dbnum],
        )?;
        Ok(())
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

pub static SESSION_STORE: Lazy<SessionStore> =
    Lazy::new(|| SessionStore::open_default().expect("Failed to open session store"));

// DEPRECATED: These tests need to be updated
// #[cfg(test)]
// #[ignore]
// mod tests {
//     use super::*;
//
// //     #[test]
// //     fn test_session_store_operations() {
// //         let temp_dir = tempfile::tempdir().expect("create temp dir");
// //         let store_path = temp_dir.path().join("test_session.sqlite");
// //         let store = SessionStore::open_with_path(&store_path).expect("open store");
//
//         // Test put and get
//         let dbnum = 1;
//         let sesno = 100;
//         let timestamp = 1640995200; // 2022-01-01
//
//         store.put_sesno_time_mapping(dbnum, sesno, timestamp).expect("put mapping");
//         let retrieved = store.get_sesno_time_mapping(dbnum, sesno).expect("get mapping");
//         assert_eq!(retrieved, timestamp);
//
//         // Test max sesno
//         store.put_sesno_time_mapping(dbnum, 200, timestamp + 86400).expect("put mapping 2");
//         let max_sesno = store.get_max_sesno_for_dbnum(dbnum).expect("get max sesno");
//         assert_eq!(max_sesno, 200);
//
//         // Test get all mappings
//         let all_mappings = store.get_all_sesno_mappings(dbnum);
//         assert_eq!(all_mappings.len(), 2);
//     }
// }
