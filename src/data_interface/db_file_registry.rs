//! 文件观察登记（替代 old-pdms-io `watch::PdmsWatcher` 在本仓被用到的那部分）。
//!
//! 出处：`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §F（T413）。
//!
//! 清单 §F 把 `PdmsWatcher` 当成「唯一的目录监听实现」，那是看名字下的结论。核对本仓的实际
//! 用法（`rg PdmsWatcher src/`）：`init_local_watcher` / `async_watcher` / `scan_db_headers`
//! **一处都没有调用**——真正盯目录的是 `increment_manager::async_watch` 自己的
//! `notify::PollWatcher`（`project_paths::mount` 挂目录）。`PdmsWatcher` 在这里只是一个状态壳：
//!
//! | 字段 | 谁在用 |
//! |---|---|
//! | `watch_dirs` | `AiosDBManager::watch_dirs()` 的启动快照、五个夹具测试 |
//! | `file_name_full_path_map` | `init_watcher` 扫描时登记；MQTT clone 按文件名反查路径 |
//! | `headers` / `get_dbno` | 只有 MQTT clone 读；从未被写过（`init_local_watcher` 不被调用） |
//!
//! 所以这里不需要在 e3d-io 侧新写 watcher，也没有事件序列要对拍：把这个状态壳搬进本仓，
//! 文件头缓存的类型换成 e3d-io 版的 [`DbFileHeader`]，`get_dbno` 的语义（有缓存才回答）不变。
//! 按 `CONTEXT.md`「文件观察」的分层，读头是分析层的事——`headers` 由调用方在需要时用
//! [`DbFileRegistry::record_header`] 填，登记本身不读文件。

use std::path::{Path, PathBuf};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use super::db_file_header::DbFileHeader;

/// 监控目录清单 + 文件名→全路径 + 文件头缓存。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DbFileRegistry {
    /// 监控目录（启动时解析的快照；摄入侧要读 `AiosDBManager::watch_dirs()`）。
    pub watch_dirs: Vec<PathBuf>,
    /// 库文件头缓存：路径 → 头信息。由分析层按需填入，登记本身不读文件。
    #[serde(skip)]
    pub headers: DashMap<PathBuf, DbFileHeader>,
    /// 文件名（不含扩展名）→ 全路径。
    #[serde(skip)]
    pub file_name_full_path_map: DashMap<String, PathBuf>,
}

impl DbFileRegistry {
    pub fn new<P: AsRef<Path>>(dirs: Vec<P>) -> Self {
        Self {
            watch_dirs: dirs
                .into_iter()
                .map(|dir| dir.as_ref().to_path_buf())
                .collect(),
            headers: Default::default(),
            file_name_full_path_map: Default::default(),
        }
    }

    /// 登记一份已读出的文件头（分析层读头，这里只存）。
    pub fn record_header(&self, path: &Path, header: DbFileHeader) {
        self.headers.insert(path.to_path_buf(), header);
    }

    /// 缓存里这个路径的库号；没读过头就是 `None`（与 `PdmsWatcher::get_dbno` 同语义）。
    pub fn get_dbno(&self, path: &PathBuf) -> Option<u32> {
        self.headers.get(path).map(|header| header.dbnum as u32)
    }

    /// 存成 JSON（`PdmsWatcher::save` 的替身；缓存字段不落盘）。
    pub fn save(&self, path: Option<&str>) -> anyhow::Result<()> {
        std::fs::write(
            path.unwrap_or("watcher.json"),
            serde_json::to_string(self)?.as_bytes(),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbno_is_only_known_after_a_header_was_recorded() {
        let registry = DbFileRegistry::new(vec![PathBuf::from("D:/proj/ams000")]);
        let file = PathBuf::from("D:/proj/ams000/ams8000_0001");
        assert_eq!(registry.get_dbno(&file), None);
        registry.record_header(
            &file,
            DbFileHeader {
                dbnum: 8000,
                ..DbFileHeader::default()
            },
        );
        assert_eq!(registry.get_dbno(&file), Some(8000));
        assert_eq!(registry.watch_dirs, vec![PathBuf::from("D:/proj/ams000")]);
    }
}
