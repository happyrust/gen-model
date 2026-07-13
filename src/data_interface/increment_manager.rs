// 标准库导入
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

// AIOS核心模块导入
use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::version::{backup_data, backup_owner_relate};
use aios_core::{RefU64Vec, RefnoEnum, get_db_option};
use aios_core::{clear_all_caches, get_default_name, get_pe};

// 异步和工具库导入
use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use notify::{RecursiveMode, Watcher};

// PDMS相关模块导入
use parse_pdms_db::parse::parse_db_basic_info;
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::{EleOperationData, EleOperationDetail, PdmsIO};
use pdms_io::watch::PdmsWatcher;

// 其他依赖导入
use petgraph::visit::Walker;
use tokio::fs::create_dir_all;
use walkdir::WalkDir;

// 本地模块导入
use crate::api::element::gen_pdms_element_insert_sql;
use crate::data_interface::helper::delete_inst_relate_cascade;
use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::*;
use parse_pdms_db::parse::DbBasicInfo;
use tracing_subscriber::fmt::format;

use crate::consts::PDMS_ELEMENTS_TABLE;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_should_exclude_file() {
        // 创建一个简单的测试实例，只需要测试 should_exclude_file 方法
        struct TestManager;

        impl TestManager {
            fn should_exclude_file(&self, file_path: &std::path::Path) -> bool {
                // 复制 should_exclude_file 的逻辑用于测试
                if let Some(extension) = file_path.extension() {
                    if let Some(ext_str) = extension.to_str() {
                        let ext_lower = ext_str.to_lowercase();

                        let excluded_extensions = [
                            "com", "exe", "dll", "sys", "tmp", "temp", "log", "bak", "backup",
                            "old", "cache", "lock", "pid", "swp", "swo", "~",
                        ];

                        if excluded_extensions.contains(&ext_lower.as_str()) {
                            return true;
                        }
                    }
                }

                if let Some(file_name) = file_path.file_name() {
                    if let Some(name_str) = file_name.to_str() {
                        let name_lower = name_str.to_lowercase();

                        let excluded_patterns = ["thumbs.db", "desktop.ini", ".ds_store"];

                        for pattern in &excluded_patterns {
                            if name_lower == *pattern {
                                return true;
                            }
                        }

                        if name_str.starts_with("~$") {
                            return true;
                        }

                        if name_str.starts_with('.') && name_str.len() > 1 {
                            return true;
                        }
                    }
                }

                false
            }
        }

        let manager = TestManager;

        // 测试应该被排除的文件扩展名
        assert!(manager.should_exclude_file(Path::new("test.com")));
        assert!(manager.should_exclude_file(Path::new("program.exe")));
        assert!(manager.should_exclude_file(Path::new("library.dll")));
        assert!(manager.should_exclude_file(Path::new("system.sys")));
        assert!(manager.should_exclude_file(Path::new("temp.tmp")));
        assert!(manager.should_exclude_file(Path::new("backup.bak")));
        assert!(manager.should_exclude_file(Path::new("debug.log")));
        assert!(manager.should_exclude_file(Path::new("cache.cache")));

        // 测试应该被排除的系统文件
        assert!(manager.should_exclude_file(Path::new("thumbs.db")));
        assert!(manager.should_exclude_file(Path::new("desktop.ini")));
        assert!(manager.should_exclude_file(Path::new(".ds_store")));
        assert!(manager.should_exclude_file(Path::new("~$document.docx")));

        // 测试隐藏文件
        assert!(manager.should_exclude_file(Path::new(".hidden")));
        assert!(manager.should_exclude_file(Path::new(".gitignore")));

        // 测试不应该被排除的文件
        assert!(!manager.should_exclude_file(Path::new("data.db")));
        assert!(!manager.should_exclude_file(Path::new("config.txt")));
        assert!(!manager.should_exclude_file(Path::new("ams7334_0001")));
        assert!(!manager.should_exclude_file(Path::new("zdj7015_0001")));
        assert!(!manager.should_exclude_file(Path::new("document.pdf")));

        // 测试大小写不敏感
        assert!(manager.should_exclude_file(Path::new("TEST.COM")));
        assert!(manager.should_exclude_file(Path::new("Program.EXE")));
        assert!(manager.should_exclude_file(Path::new("THUMBS.DB")));
    }
}

/// 增量更新信息结构体
///
/// 用于存储和跟踪数据库中元素的增量变化信息
#[derive(Debug, Default, Clone)]
pub struct IncrementInfo {
    /// 元素的引用编号
    pub refno: RefU64,
    /// 数据库编号
    pub db_no: i32,
    /// 元素的属性映射
    pub attr: NamedAttrMap,
    /// 子元素的引用编号列表
    pub children: RefU64Vec,
    /// 元素的操作类型(增加/修改/删除)
    pub operation: EleOperation,
}

impl IncrementInfo {
    /// 检查元素是否被修改
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果元素被修改返回true，否则返回false
    #[inline]
    pub fn is_modified(&self) -> bool {
        matches!(self.operation, EleOperation::Modified)
    }

    /// 检查元素是否被删除
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果元素被删除返回true，否则返回false
    #[inline]
    pub fn is_deleted(&self) -> bool {
        matches!(self.operation, EleOperation::Deleted)
    }

    /// 检查元素是否为新增
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果元素是新增的返回true，否则返回false
    #[inline]
    pub fn is_added(&self) -> bool {
        matches!(self.operation, EleOperation::Add)
    }
}

/// JSON数据分块处理的大小常量
const JSON_CHUNK_COUNT: usize = 200;

/// MySQL批量处理的大小常量
const BATCH_SIZE: usize = 100;

/// 更新world transform的批量大小（较小，因为涉及复杂计算）
const TRANSFORM_BATCH_SIZE: usize = 50;

/// 查询inst_relate数据的批量大小（最小，避免查询超时）
const QUERY_BATCH_SIZE: usize = 20;

/// 需要检查的数据库类型列表
/// 包含目录(CATA)、设计(DESI)、字典(DICT)、系统(SYST)、全局(GLB/GLOB)等类型
pub const CHECK_DB_TYPES: [&'static str; 6] = ["CATA", "DESI", "DICT", "SYST", "GLB", "GLOB"];

impl AiosDBManager {
    /// 简化的MySQL pdms_element表更新方法
    ///
    /// 这是一个简化版本的方法，只需要传入range_eles参数即可完成MySQL数据库的更新。
    /// 该方法会自动处理数据库连接、元素分类和批量更新操作。
    ///
    /// # 参数
    ///
    /// * `range_eles` - 从collect_increment_eles方法获取的增量元素数据
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<()>` - 成功返回Ok(())，失败返回错误信息
    ///
    /// # 使用示例
    ///
    /// ```rust
    /// // 在增量更新流程中使用
    /// let range_eles = io.collect_increment_eles(Some(sesno_range))?;
    /// aios_db_manager.update_mysql_pdms_elements_simple(&range_eles).await?;
    /// ```
    #[cfg(feature = "sql")]
    pub async fn update_mysql_pdms_elements_simple(
        &self,
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> anyhow::Result<()> {
        self.update_mysql_pdms_elements(range_eles).await
    }
    /// 检查文件是否应该被排除在监控之外
    ///
    /// 根据文件扩展名和文件名模式检查文件是否应该被排除在监控范围之外。
    ///
    /// # 参数
    ///
    /// * `file_path` - 文件路径
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果文件应该被排除返回true，否则返回false
    ///
    /// # 排除规则
    ///
    /// 1. 检查文件扩展名是否在排除列表中
    /// 2. 检查文件名是否匹配排除模式
    /// 3. 检查是否为系统文件或临时文件
    fn should_exclude_file(&self, file_path: &std::path::Path) -> bool {
        // 获取文件扩展名
        if let Some(extension) = file_path.extension() {
            if let Some(ext_str) = extension.to_str() {
                let ext_lower = ext_str.to_lowercase();

                // 排除的文件扩展名列表
                let excluded_extensions = [
                    "com",    // COM可执行文件
                    "exe",    // Windows可执行文件
                    "dll",    // 动态链接库
                    "sys",    // 系统文件
                    "tmp",    // 临时文件
                    "temp",   // 临时文件
                    "log",    // 日志文件
                    "bak",    // 备份文件
                    "backup", // 备份文件
                    "old",    // 旧文件
                    "cache",  // 缓存文件
                    "lock",   // 锁文件
                    "pid",    // 进程ID文件
                    "swp",    // Vim交换文件
                    "swo",    // Vim交换文件
                    "~",      // 临时备份文件
                ];

                if excluded_extensions.contains(&ext_lower.as_str()) {
                    println!("排除文件（扩展名）: {:?}", file_path);
                    return true;
                }
            }
        }

        // 获取文件名
        if let Some(file_name) = file_path.file_name() {
            if let Some(name_str) = file_name.to_str() {
                let name_lower = name_str.to_lowercase();

                // 排除的文件名模式
                let excluded_patterns = [
                    "thumbs.db",   // Windows缩略图缓存
                    "desktop.ini", // Windows桌面配置
                    ".ds_store",   // macOS文件夹配置
                    "~$",          // Office临时文件前缀
                ];

                // 检查是否匹配排除模式
                for pattern in &excluded_patterns {
                    if pattern.starts_with("~$") && name_lower.starts_with("~$") {
                        println!("排除文件（临时文件）: {:?}", file_path);
                        return true;
                    } else if name_lower == *pattern {
                        println!("排除文件（系统文件）: {:?}", file_path);
                        return true;
                    }
                }

                // 排除以点开头的隐藏文件（Unix风格）
                if name_str.starts_with('.') && name_str.len() > 1 {
                    println!("排除文件（隐藏文件）: {:?}", file_path);
                    return true;
                }
            }
        }

        false
    }

    /// 检查数据库文件是否应该被处理
    ///
    /// 根据配置的过滤规则检查数据库文件是否应该被包含在增量更新处理中。
    ///
    /// # 参数
    ///
    /// * `db_type` - 数据库类型字符串
    /// * `db_num` - 数据库编号 (u32类型，与DbBasicInfo保持一致)
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果文件应该被处理返回true，否则返回false
    ///
    /// # 过滤规则
    ///
    /// 1. 检查数据库类型是否在支持列表中
    /// 2. `only_sync_sys` 时仅允许 SYS meta（与全量同步路径一致）
    /// 3. 检查是否在手动指定的数据库编号列表中（如果配置了）
    /// 4. 检查是否在排除的数据库编号列表中
    ///
    /// init_watcher / async_watch 共用此门控；SesnoRangeResolver 的 `skip_cata`
    /// 两侧均传 `false`，避免双路径对 CATA 分叉。
    fn should_process_database(&self, db_type: &str, db_num: u32) -> bool {
        // 检查数据库类型是否支持
        if !CHECK_DB_TYPES.contains(&db_type) {
            return false;
        }

        let db_option = get_db_option();

        // 与 versioned_db 全量 only_sync_sys 对齐：跳过 DESI/CATA
        if db_option.only_sync_sys
            && !crate::data_interface::sesno_range::COLD_START_DB_TYPES.contains(&db_type)
        {
            return false;
        }

        let manual_dbnums = db_option.manual_db_nums.clone().unwrap_or_default();
        let exclude_dbnums = db_option.exclude_db_nums.clone().unwrap_or_default();

        // 检查是否在手动指定的数据库编号列表中
        if !manual_dbnums.is_empty() && !manual_dbnums.contains(&db_num) {
            return false;
        }

        // 检查是否在排除的数据库编号列表中
        if !exclude_dbnums.is_empty() && exclude_dbnums.contains(&db_num) {
            return false;
        }

        true
    }
    /// 执行增量更新操作
    ///
    /// 编排：IncrementPipeline → enqueue side-effects → 可选 MySQL → SYST 派生 → ModelRefresh。
    /// 返回 [`IncrResult`] 供调用方（如 SyncPublisher）继续处理。
    ///
    /// 模型刷新 / SYST 派生同步失败不会使整体返回 Err：持久化与水位已完成，
    /// 错误只记入 `IncrResult::warnings`，并以 [`SideEffectCompensator`] 落库待补偿，
    /// 以保证调用方仍能对成功文件执行异地同步发布。
    ///
    /// Map value: `(basic_info, sesno_range, db_type)`.
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>, String)>,
    ) -> anyhow::Result<crate::data_interface::increment_pipeline::IncrResult> {
        use crate::data_interface::increment_pipeline::IncrementPipeline;
        use crate::data_interface::side_effect_pending::{
            SideEffectCompensator, SideEffectKind,
        };

        if increment_ranges_map.is_empty() {
            return Ok(Default::default());
        }

        let mut incr = IncrementPipeline::new()
            .apply(increment_ranges_map)
            .await;

        for err in &incr.errors {
            println!("增量文件失败: {:?} — {}", err.path, err.error);
        }
        for w in &incr.warnings {
            println!("增量警告: {}", w);
        }

        if let Err(e) = SideEffectCompensator::enqueue_from_incr(self, &incr).await {
            let warn = format!("副作用任务入队失败（不影响水位）: {e:?}");
            println!("{warn}");
            incr.warnings.push(warn);
        }

        #[cfg(feature = "sql")]
        {
            for success in &incr.successes {
                match self.update_mysql_pdms_elements(&success.range_eles).await {
                    Ok(_) => println!(
                        "MySQL pdms_element 更新成功: dbnum={}",
                        success.dbnum
                    ),
                    Err(e) => println!(
                        "MySQL pdms_element 更新失败 dbnum={}: {}",
                        success.dbnum, e
                    ),
                }
            }
        }

        // SYST 增量落 PE 后，刷新 TEAM 等派生表（与全量 only_sync_sys 路径一致；失败不回滚水位）
        if incr.has_db_type("SYST") {
            match (async {
                let aios_mgr =
                    aios_core::aios_db_mgr::aios_mgr::AiosDBMgr::init_from_db_option().await?;
                crate::team_data::sync_team_data(&aios_mgr).await
            })
            .await
            {
                Ok(_) => {
                    println!("SYST 增量后派生同步 sync_team_data 完成");
                    if let Err(e) = SideEffectCompensator::complete_syst_jobs(&incr).await {
                        println!("标记 syst_derived done 失败: {e:?}");
                    }
                }
                Err(e) => {
                    let warn = format!("SYST 派生同步失败（持久化与水位不受影响）: {e:?}");
                    println!("{warn}");
                    incr.warnings.push(warn);
                    let end_sesno = incr
                        .successes
                        .iter()
                        .filter(|s| s.db_type == "SYST")
                        .map(|s| s.end_sesno)
                        .max()
                        .unwrap_or(0);
                    let dbnum = incr
                        .successes
                        .iter()
                        .find(|s| s.db_type == "SYST")
                        .map(|s| s.dbnum)
                        .unwrap_or(0);
                    let _ = SideEffectCompensator::mark_failed(
                        SideEffectKind::SystDerived,
                        dbnum,
                        end_sesno,
                        &format!("{e:?}"),
                    )
                    .await;
                }
            }
        }

        if let Err(e) =
            crate::data_interface::model_refresh::ModelRefreshPolicy::refresh(self, &incr).await
        {
            let warn = format!("模型刷新失败（持久化与水位不受影响）: {e:?}");
            println!("{warn}");
            incr.warnings.push(warn);
            for success in &incr.successes {
                if crate::data_interface::increment_pipeline::SYS_META_DB_TYPES
                    .contains(&success.db_type.as_str())
                    || success.changed_refnos.is_empty()
                {
                    continue;
                }
                let _ = SideEffectCompensator::mark_failed(
                    SideEffectKind::ModelRefresh,
                    success.dbnum,
                    success.end_sesno,
                    &format!("{e:?}"),
                )
                .await;
            }
        } else if let Err(e) = SideEffectCompensator::complete_model_jobs(&incr).await {
            println!("标记 model_refresh done 失败: {e:?}");
        }

        Ok(incr)
    }

    /// 初始化文件监控器
    ///
    /// 在系统启动时扫描监控目录中的所有数据库文件，检查是否需要进行增量更新。
    /// 该方法会比较文件中的最新会话号与数据库中记录的会话号，如果文件更新则执行增量更新。
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<()>` - 成功返回Ok(())，失败返回错误信息
    ///
    /// # 处理流程
    ///
    /// 1. 创建必要的目录结构
    /// 2. 遍历所有监控目录中的文件
    /// 3. 解析数据库基本信息
    /// 4. 检查是否需要增量更新
    /// 5. 生成压缩包(如果启用MQTT功能)
    /// 6. 执行增量更新操作
    /// 7. 成功后由 SyncPublisher 发布异地同步（与 async_watch 对齐）
    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        // 创建存档与压缩临时目录（SyncPublisher 依赖 assets/temp）
        fs::create_dir_all("assets/archives")?;
        fs::create_dir_all("assets/temp")?;
        let mut time = Instant::now();
        dbg!(&self.watcher.watch_dirs);

        // 先补偿上次未完成的副作用（水位已推过的 mesh / SYST 派生）
        match crate::data_interface::side_effect_pending::SideEffectCompensator::drain(self)
            .await
        {
            Ok(n) if n > 0 => println!("启动补偿完成 {n} 个副作用任务"),
            Ok(_) => {}
            Err(e) => println!("启动副作用补偿失败（继续增量扫描）: {e:?}"),
        }

        // 获取数据库配置选项
        let db_option = get_db_option();
        let manual_dbnums = db_option.manual_db_nums.clone().unwrap_or_default();
        let exclude_dbnums = db_option.exclude_db_nums.clone().unwrap_or_default();

        // 遍历所有监控目录
        for watch_dir in &self.watcher.watch_dirs {
            // 按文件大小降序排列，优先处理大文件
            for entry in WalkDir::new(watch_dir).sort_by(|a, b| {
                let a_len = a.path().metadata().map(|m| m.len()).unwrap_or_default();
                let b_len = b.path().metadata().map(|m| m.len()).unwrap_or_default();
                b_len.cmp(&a_len)
            }) {
                let dir_entry = entry.map_err(|e| anyhow::anyhow!("获取目录条目失败: {}", e))?;
                let path = dir_entry.path();

                // 获取文件名(不含扩展名)
                let file_name = path
                    .file_stem()
                    .ok_or_else(|| anyhow::anyhow!("无法从路径获取文件名: {}", path.display()))?
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("文件名转换为字符串失败: {}", path.display()))?;

                // 跳过目录
                if path.is_dir() {
                    continue;
                }

                // 检查文件是否应该被排除
                if self.should_exclude_file(path) {
                    continue;
                }

                // 解析数据库基本信息
                let DbBasicInfo {
                    db_type,
                    ses_pgno,
                    db_no,
                } = parse_db_basic_info(path.to_path_buf());

                // 使用统一的过滤方法检查是否应该处理此数据库
                if !self.should_process_database(&db_type, db_no) {
                    continue;
                }

                // 获取项目名称和文件最新会话号
                let project = get_db_option().project_name.clone();
                let file_latest_sesno = PdmsIO::new(&project, path.to_path_buf(), true)
                    .get_latest_sesno()
                    .unwrap_or_default();
                dbg!(&file_latest_sesno);

                // 建立文件名到完整路径的映射
                self.watcher
                    .file_name_full_path_map
                    .insert(file_name.to_owned(), path.to_path_buf());

                // 只有开启MQTT功能时，才需要初始化压缩数据包用于异地同步
                #[cfg(feature = "mqtt")]
                {
                    use crate::data_interface::sync_publisher::SyncPublisher;
                    if let Err(e) =
                        SyncPublisher::ensure_archive(&path.to_path_buf()).await
                    {
                        eprintln!("初始化存档失败 {:?}: {}", file_name, e);
                    }
                }

                // 统一 sesno 范围解析（水位=dbnum，nearest 跳跃）
                {
                    use crate::data_interface::sesno_range::SesnoRangeResolver;
                    let resolver = SesnoRangeResolver::new();
                    match resolver
                        .resolve(
                            path,
                            &project,
                            db_no,
                            file_latest_sesno as i32,
                            false, // CATA 仅由 should_process_database 门控（与 watch 对齐）
                            &db_type,
                        )
                        .await
                    {
                        Ok(Some(plan)) => {
                            if plan.cold_start {
                                println!(
                                    "发现需要冷启动的 SYS meta 文件: {:?}, db_type={}, 水位=0, 文件最新sesno: {}, range={:?}",
                                    &file_name, plan.db_type, plan.file_latest_sesno, plan.range
                                );
                            } else {
                                println!(
                                    "发现需要增量更新的文件: {:?}, 当前数据库最大sesno: {}, \
                                    文件最新sesno: {}",
                                    &file_name, plan.db_latest_sesno, plan.file_latest_sesno
                                );
                            }
                            params.insert(plan.path, (plan.basic_info, plan.range, plan.db_type));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            println!("sesno 范围解析失败 {:?}: {:?}", file_name, e);
                        }
                    }
                }
            }
        }

        // 等所有文件检查完毕后，执行批量增量更新
        if !params.is_empty() {
            dbg!(params.len());
        }

        // 执行增量更新；成功后与 async_watch 一致，走 SyncPublisher 异地同步
        match self.execute_incr_update(params).await {
            Ok(incr) if incr.had_work() => {
                println!(
                    "启动时自动增量更新执行完成。成功 {} 个文件，失败 {} 个",
                    incr.successes.len(),
                    incr.errors.len()
                );
                let publisher =
                    crate::data_interface::sync_publisher::SyncPublisher::new(
                        self.mqtt_client.clone(),
                    );
                let outcome = publisher.publish(&incr).await;
                for e in &outcome.errors {
                    println!("SyncPublisher 错误: {}", e);
                }
                println!(
                    "SyncPublisher(init): published={}, skipped={}",
                    outcome.published.len(),
                    outcome.skipped.len()
                );
            }
            Ok(_) => {
                println!("没有发现需要增量更新的内容。")
            }
            Err(e) => {
                println!("执行增量更新时发生错误: {:?}", e);
            }
        }

        println!("初始化增量更新总耗时: {} 秒", time.elapsed().as_secs_f32());

        anyhow::Ok(())
    }

    /// 开始异步监控数据文件夹
    ///
    /// 启动文件系统监控器，实时监测数据库文件的变化并执行增量更新。
    /// 当检测到文件修改时，会自动触发增量更新流程。
    ///
    /// # 返回值
    ///
    /// * `notify::Result<()>` - 成功返回Ok(())，失败返回监控错误
    ///
    /// # 监控流程
    ///
    /// 1. 创建异步文件监控器
    /// 2. 监控指定目录中的文件变化
    /// 3. 过滤只关心数据内容变化的事件
    /// 4. 扫描变化文件的头部信息
    /// 5. 比较会话号确定是否需要增量更新
    /// 6. 执行增量更新和模型同步
    /// 7. 启动时与每次数据变更事件前 drain 副作用补偿队列
    pub async fn async_watch(&self) -> notify::Result<()> {
        // 创建异步文件监控器
        let (mut watcher, mut rx) = PdmsWatcher::async_watcher()?;
        dbg!(&self.watcher.watch_dirs);

        // 为每个监控目录设置文件监控
        self.watcher.watch_dirs.iter().for_each(|x| {
            watcher
                .watch(x.as_path(), RecursiveMode::NonRecursive)
                .expect("文件监控设置失败");
        });

        // 创建必要的目录结构
        create_dir_all("assets/archives")
            .await
            .map_err(|e| notify::Error::io(e))?;
        create_dir_all("assets/temp")
            .await
            .map_err(|e| notify::Error::io(e))?;

        // 启动时先补偿一次（与 init_watcher 对齐；仅 watch 场景也会覆盖）
        match crate::data_interface::side_effect_pending::SideEffectCompensator::drain(self)
            .await
        {
            Ok(n) if n > 0 => println!("watch 启动补偿完成 {n} 个副作用任务"),
            Ok(_) => {}
            Err(e) => println!("watch 启动副作用补偿失败（继续监听）: {e:?}"),
        }

        // 持续监听文件变化事件
        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    // 过滤事件类型，只处理数据内容变化的事件
                    // 跳过仅元数据变动的情况
                    let data_changed = matches!(
                        event.kind,
                        notify::EventKind::Modify(notify::event::ModifyKind::Data(_))
                            | notify::EventKind::Modify(notify::event::ModifyKind::Any)
                            | notify::EventKind::Create(notify::event::CreateKind::File)
                            | notify::EventKind::Remove(notify::event::RemoveKind::File)
                    );
                    if !data_changed {
                        continue;
                    }

                    // 每次有数据变更时顺带 drain：覆盖「水位已推、刷新失败」的积压
                    if let Err(e) = crate::data_interface::side_effect_pending::SideEffectCompensator::drain(
                        self,
                    )
                    .await
                    {
                        println!("watch 周期副作用补偿失败: {e:?}");
                    }

                    // 记录文件变化事件
                    println!("检测到文件变化: {:?}", &event);

                    // 预过滤：检查事件中的文件路径，排除不需要监控的文件
                    let filtered_paths: Vec<_> = event
                        .paths
                        .iter()
                        .filter(|path| !self.should_exclude_file(path))
                        .cloned()
                        .collect();

                    if filtered_paths.is_empty() {
                        println!("所有变化的文件都被排除规则过滤，跳过处理");
                        continue;
                    }

                    println!("开始扫描数据库头部信息，过滤后路径: {:?}", &filtered_paths);

                    // 扫描变化文件的数据库头部信息（使用过滤后的路径）
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(&filtered_paths) {
                        println!("成功扫描到 {} 个数据库头部", new_headers.len());
                        let mut params = IndexMap::new();

                        // 处理每个扫描到的数据库头部信息
                        for (path, new_header) in &new_headers {
                            println!("正在处理路径: {:?}", path);

                            // 检查文件是否应该被排除
                            if self.should_exclude_file(path) {
                                println!("文件被排除规则过滤: {:?}", path);
                                continue;
                            }

                            // 获取文件名用于后续处理
                            let file_name = match path.file_stem().and_then(|s| s.to_str()) {
                                Some(name) => name,
                                None => {
                                    println!("无法获取文件名: {:?}", path);
                                    continue;
                                }
                            };

                            // 解析数据库基本信息以获取数据库类型
                            let db_basic_info = parse_db_basic_info(path.to_path_buf());
                            let db_type = &db_basic_info.db_type;
                            let db_num = db_basic_info.db_no;

                            // 使用统一的过滤方法检查是否应该处理此数据库
                            if !self.should_process_database(db_type, db_num) {
                                println!(
                                    "根据过滤规则跳过数据库: 类型={}, 编号={}",
                                    db_type, db_num
                                );
                                continue;
                            }

                            println!(
                                "检查文件: {}, 数据库编号: {}, 文件会话号: {}",
                                file_name, db_num, new_header.latest_ses_data.sesno
                            );

                            use crate::data_interface::sesno_range::SesnoRangeResolver;
                            let project = get_db_option().project_name.clone();
                            let resolver = SesnoRangeResolver::new();
                            match resolver
                                .resolve_with_header(
                                    path,
                                    &project,
                                    new_header.clone(),
                                    false, // CATA 仅由 should_process_database 门控（与 init 对齐）
                                    db_type,
                                )
                                .await
                            {
                                Ok(Some(plan)) => {
                                    if plan.cold_start {
                                        println!(
                                            "发现需要冷启动的 SYS meta 文件: {}, db_type={}, 水位=0, 文件会话号: {}, range={:?}",
                                            file_name, plan.db_type, plan.file_latest_sesno, plan.range
                                        );
                                    } else {
                                        println!(
                                            "发现需要增量更新的文件: {}, 数据库会话号: {}, 文件会话号: {}",
                                            file_name, plan.db_latest_sesno, plan.file_latest_sesno
                                        );
                                    }
                                    params.insert(
                                        plan.path,
                                        (plan.basic_info, plan.range, plan.db_type),
                                    );
                                }
                                Ok(None) => {
                                    println!(
                                        "文件 {} 无需更新（水位已覆盖或不可解析）",
                                        file_name
                                    );
                                }
                                Err(e) => {
                                    println!("sesno 范围解析失败 {}: {:?}", file_name, e);
                                }
                            }
                        }
                        // 如果没有需要更新的参数，跳过后续处理
                        if params.is_empty() {
                            continue;
                        }

                        // 执行增量更新，成功后由 SyncPublisher 负责异地同步
                        match self.execute_incr_update(params).await {
                            Ok(incr) if incr.had_work() => {
                                let publisher =
                                    crate::data_interface::sync_publisher::SyncPublisher::new(
                                        self.mqtt_client.clone(),
                                    );
                                let outcome = publisher.publish(&incr).await;
                                for e in &outcome.errors {
                                    println!("SyncPublisher 错误: {}", e);
                                }
                                println!(
                                    "SyncPublisher: published={}, skipped={}",
                                    outcome.published.len(),
                                    outcome.skipped.len()
                                );
                            }
                            Ok(_) => {
                                println!("文件 {:?} 发生修改，但未触发增量更新。", &event.paths);
                                continue;
                            }
                            Err(e) => {
                                println!("执行增量更新时发生错误: {:?}", e);
                            }
                        }
                    } else {
                        println!("扫描数据库头部信息失败，路径: {:?}", &event.paths);
                    }
                }
                Err(e) => println!("文件监控错误: {:?}", e),
            }
        }

        Ok(())
    }

    /// 将元素更新到数据库中
    ///
    /// 该方法负责将收集到的增量元素数据批量更新到数据库中。
    /// 目前的实现中，实际的数据库更新操作已经在PdmsIO中完成，
    /// 这里保留接口用于未来可能的扩展。
    ///
    /// # 参数
    ///
    /// * `io` - PDMS IO 对象，用于数据库操作
    /// * `range_eles` - 元素操作数据的映射
    ///   - key: 会话号(sesno)
    ///   - value: 该会话号下的元素操作数据列表
    /// * `dbnum` - 数据库编号
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<()>` - 成功返回Ok(())，失败返回错误信息
    ///
    /// # 实现说明
    ///
    /// 当前实现为空，因为实际的数据库更新已经在PdmsIO.update_elements_to_database()中完成。
    /// 保留此方法是为了：
    /// 1. 保持接口的完整性
    /// 2. 未来可能需要在这里添加额外的处理逻辑
    /// 3. 支持不同的数据库更新策略
    ///
    /// # 历史实现（已注释）
    ///
    /// 之前的实现包括：
    /// - 收集所有SQL语句
    /// - 按JSON_CHUNK_COUNT分批执行
    /// - 使用SurrealDB进行批量更新
    pub async fn update_elements_to_database(
        &self,
        io: &PdmsIO,
        range_eles: HashMap<u32, Vec<EleOperationData>>,
        dbnum: i32,
    ) -> anyhow::Result<()> {
        // 当前实现为空，实际更新已在PdmsIO中完成
        //
        // 历史实现（已注释）:
        // 分类元素并收集所有SQL语句
        // let mut sqls = Vec::with_capacity(range_eles.len());
        // for (sesno, ele_vec) in &range_eles {
        //     for ele in ele_vec {
        //         let sql = ele.detail.to_surql(&sesno.to_string());
        //         sqls.push(sql);
        //     }
        // }
        //
        // // 批量执行SQL，按JSON_CHUNK_COUNT分块处理
        // for chunk in sqls.chunks(JSON_CHUNK_COUNT) {
        //     let batch_sql = chunk.join(";");
        //     if !batch_sql.is_empty() {
        //         SUL_DB.query(batch_sql).await?;
        //     }
        // }

        Ok(())
    }

    /// 更新MySQL pdms_element表数据
    ///
    /// 该方法专门用于将增量元素数据更新到MySQL的pdms_element表中。
    /// 根据元素操作类型（新增、修改、删除）执行相应的数据库操作。
    ///
    /// # 参数
    ///
    /// * `range_eles` - 元素操作数据的映射
    ///   - key: 会话号(sesno)
    ///   - value: 该会话号下的元素操作数据列表
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<()>` - 成功返回Ok(())，失败返回错误信息
    ///
    /// # 处理逻辑
    ///
    /// 1. **新增操作**: 插入新的元素记录到pdms_element表
    /// 2. **修改操作**: 更新现有元素的相关字段
    /// 3. **删除操作**: 将IS_DEL字段设置为1，标记为已删除
    ///
    /// # 性能优化
    ///
    /// - 使用批量SQL操作减少数据库连接开销
    /// - 按操作类型分组处理，提高执行效率
    /// - 分批处理避免SQL语句过长
    #[cfg(feature = "sql")]
    pub async fn update_mysql_pdms_elements(
        &self,
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> anyhow::Result<()> {
        // 获取数据库连接配置
        let db_option = get_db_option();
        let project_name = &db_option.project_name;
        // 获取MySQL连接池
        let connection_str =
            crate::data_interface::tidb_manager::AiosDBManager::get_default_conn_str(&db_option);
        let pool = crate::data_interface::tidb_manager::AiosDBManager::get_db_pool(
            &connection_str,
            project_name,
        )
        .await?;
        // 分类收集不同操作类型的元素
        let mut insert_elements = Vec::new(); // 新增元素
        let mut update_elements = Vec::new(); // 修改元素
        let mut delete_elements = Vec::new(); // 删除元素
        // 遍历所有会话号下的元素操作数据
        for (sesno, ele_vec) in range_eles {
            for ele_data in ele_vec {
                match &ele_data.detail {
                    EleOperationDetail::Add(add_data) => {
                        insert_elements.push((ele_data.refno, *sesno, add_data));
                    }
                    EleOperationDetail::Modified(modify_data) => {
                        update_elements.push((ele_data.refno, *sesno, modify_data));
                    }
                    EleOperationDetail::Deleted => {
                        delete_elements.push((ele_data.refno, *sesno));
                    }
                    EleOperationDetail::None => {
                        // 跳过无操作类型
                        continue;
                    }
                }
            }
        }
        // 处理新增元素
        if !insert_elements.is_empty() {
            self.process_mysql_insert_elements(&pool, &insert_elements)
                .await?;
        }
        // 处理修改元素
        if !update_elements.is_empty() {
            self.process_mysql_update_elements(&pool, &update_elements)
                .await?;
        }
        // 处理删除元素
        if !delete_elements.is_empty() {
            self.process_mysql_delete_elements(&pool, &delete_elements)
                .await?;
        }
        println!("MySQL pdms_element表更新完成");
        Ok(())
    }

    /// 处理MySQL新增元素操作
    ///
    /// 批量插入新增的元素到pdms_element表中
    ///
    /// # 参数
    ///
    /// * `pool` - MySQL连接池
    /// * `insert_elements` - 新增元素列表，包含(refno, sesno, add_data)
    #[cfg(feature = "sql")]
    async fn process_mysql_insert_elements(
        &self,
        pool: &sqlx::Pool<sqlx::MySql>,
        insert_elements: &[(RefU64, u32, &parse_pdms_db::parse::EleData)],
    ) -> anyhow::Result<()> {
        if insert_elements.is_empty() {
            return Ok(());
        }
        println!("开始处理{}个新增元素", insert_elements.len());
        // 分批处理，避免SQL语句过长
        for chunk in insert_elements.chunks(BATCH_SIZE) {
            let mut insert_sql = String::new();
            let mut has_valid_elements = false;

            for (refno, _sesno, add_data) in chunk {
                // 使用EleData中的属性信息
                let attr_map = add_data.whole_attmap.att_map();
                // 构建children_map（使用EleData中的children信息）
                let mut children_map = HashMap::new();
                if !add_data.children.is_empty() {
                    // 将RefU64Vec转换为Vec<RefU64>
                    let children_vec: Vec<RefU64> = add_data.children.iter().cloned().collect();
                    children_map.insert(*refno, children_vec);
                }
                // 从属性映射中获取数据库编号，如果没有则使用默认值0
                let dbnum = attr_map.get_i32("DBNO").unwrap_or(0);
                // 生成插入SQL片段
                let sql_fragment = gen_pdms_element_insert_sql(attr_map, dbnum, &children_map);
                if !sql_fragment.is_empty() {
                    insert_sql.push_str(&sql_fragment);
                    has_valid_elements = true;
                }
            }
            // 执行批量插入
            if has_valid_elements {
                // 构建完整的INSERT语句
                let mut full_sql = format!(
                    "INSERT IGNORE INTO {} (ID, REFNO, TYPE, OWNER, NAME, NUMBDB, ORDER_NUM, CHILDREN_COUNT, IS_DEL) VALUES {}",
                    PDMS_ELEMENTS_TABLE, insert_sql
                );
                // 移除最后的逗号
                if full_sql.ends_with(",") {
                    full_sql.truncate(full_sql.len() - 1);
                }
                // 执行SQL
                match sqlx::query(&full_sql).execute(pool).await {
                    Ok(result) => {
                        println!("成功插入{}行记录", result.rows_affected());
                    }
                    Err(e) => {
                        println!("插入元素失败: {}", e);
                        println!("SQL: {}", full_sql);
                        return Err(anyhow::anyhow!("插入元素失败: {}", e));
                    }
                }
            }
        }

        println!("新增元素处理完成");
        Ok(())
    }
    /// 处理MySQL修改元素操作
    ///
    /// 更新已存在元素的相关字段
    ///
    /// # 参数
    ///
    /// * `pool` - MySQL连接池
    /// * `update_elements` - 修改元素列表，包含(refno, sesno, modify_data)
    #[cfg(feature = "sql")]
    async fn process_mysql_update_elements(
        &self,
        pool: &sqlx::Pool<sqlx::MySql>,
        update_elements: &[(RefU64, u32, &pdms_io::io::ModifiedElement)],
    ) -> anyhow::Result<()> {
        use crate::consts::PDMS_ELEMENTS_TABLE;

        if update_elements.is_empty() {
            return Ok(());
        }
        println!("开始处理{}个修改元素", update_elements.len());
        // 分批处理
        for chunk in update_elements.chunks(BATCH_SIZE) {
            let mut update_sqls = Vec::new();
            for (refno, _sesno, modify_data) in chunk {
                // todo 暂时通过查询surreal来获取最终得值
                if let Some(pe) = get_pe((*refno).into()).await? {
                    let name = if !pe.name.is_empty() {
                        pe.name
                    } else {
                        get_default_name((*refno).into())
                            .await?
                            .unwrap_or("".to_string())
                    };
                    // 构建UPDATE语句
                    let update_sql = format!(
                        "UPDATE {} SET OWNER={}, NAME='{}' WHERE ID={}",
                        PDMS_ELEMENTS_TABLE,
                        pe.owner.refno().0,
                        name,
                        pe.refno.refno().0
                    );
                    update_sqls.push(update_sql);
                }
            }
            // 批量执行UPDATE语句
            for sql in update_sqls {
                match sqlx::query(&sql).execute(pool).await {
                    Ok(result) => {
                        if result.rows_affected() == 0 {
                            println!("警告: MySQL 更新元素时未找到对应记录: {}", sql);
                        }
                    }
                    Err(e) => {
                        println!("更新元素失败: {}", e);
                        println!("SQL: {}", sql);
                        return Err(anyhow::anyhow!("更新元素失败: {}", e));
                    }
                }
            }
        }
        println!("修改元素处理完成");
        Ok(())
    }

    /// 处理MySQL删除元素操作
    ///
    /// 将删除的元素标记为已删除（IS_DEL=1）
    ///
    /// # 参数
    ///
    /// * `pool` - MySQL连接池
    /// * `delete_elements` - 删除元素列表，包含(refno, sesno)
    #[cfg(feature = "sql")]
    async fn process_mysql_delete_elements(
        &self,
        pool: &sqlx::Pool<sqlx::MySql>,
        delete_elements: &[(RefU64, u32)],
    ) -> anyhow::Result<()> {
        if delete_elements.is_empty() {
            return Ok(());
        }
        println!("开始处理{}个删除元素", delete_elements.len());
        // 分批处理
        for chunk in delete_elements.chunks(BATCH_SIZE) {
            // 构建批量UPDATE语句，将IS_DEL设置为1
            let refno_list: Vec<String> = chunk
                .iter()
                .map(|(refno, _sesno)| refno.0.to_string())
                .collect();

            let delete_sql = format!(
                "UPDATE {} SET IS_DEL=1 WHERE ID IN ({})",
                PDMS_ELEMENTS_TABLE,
                refno_list.join(",")
            );
            match sqlx::query(&delete_sql).execute(pool).await {
                Ok(result) => {
                    println!("成功标记{}行记录为已删除", result.rows_affected());
                }
                Err(e) => {
                    println!("删除元素失败: {}", e);
                    println!("SQL: {}", delete_sql);
                    return Err(anyhow::anyhow!("删除元素失败: {}", e));
                }
            }
        }
        println!("删除元素处理完成");
        Ok(())
    }

    /// 处理模型更新
    ///
    /// 这是增量更新系统中的核心模型处理方法。根据增量元素数据的变化类型，
    /// 智能判断是否需要进行几何体更新或变换更新，并执行相应的模型生成操作。
    ///
    /// # 参数
    ///
    /// * `range_eles` - 按会话号分组的元素操作数据映射
    ///   - key: 会话号(sesno)
    ///   - value: 该会话号下的元素操作数据列表
    /// * `db_num` - 数据库编号，用于标识处理的数据库
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<()>` - 成功返回Ok(())，失败返回错误信息
    ///
    /// # 处理逻辑
    ///
    /// ## 几何体更新处理:
    /// - **新增操作**: 直接生成新的模型数据
    /// - **修改操作**: 先删除旧的inst_relate数据，再重新生成模型
    /// - **删除操作**: PE已添加删除标记，无需额外处理
    ///
    /// ## 变换更新处理:
    /// - 更新该元素及其子树中所有有inst_relate的节点的world transform
    /// - 使用递归查询获取所有受影响的子节点
    ///
    /// # 性能优化
    ///
    /// - 使用HashSet收集需要处理的refnos，避免重复处理
    /// - 分批处理数据库操作，提高执行效率
    /// - 智能判断更新类型，只处理必要的操作
    pub(crate) async fn process_model_updates(
        &self,
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        db_num: i32,
    ) -> anyhow::Result<()> {
        use crate::SUL_DB;
        use crate::fast_model::occ_generate::process_meshes_update_db_deep;
        use crate::get_db_option;
        use std::collections::HashSet;

        // 收集需要生成模型的参考号集合
        let mut refnos_to_generate: HashSet<RefnoEnum> = HashSet::new();
        // 收集需要删除inst_relate数据的参考号集合
        let mut refnos_to_delete_inst_relate: HashSet<RefnoEnum> = HashSet::new();
        // 收集需要更新world transform的参考号集合
        let mut refnos_to_update_transform: HashSet<RefnoEnum> = HashSet::new();

        println!("开始处理模型更新，数据库编号: {}", db_num);

        // 遍历所有会话号下的元素操作数据
        for (sesno, ele_vec) in range_eles {
            println!("处理会话号: {}, 元素数量: {}", sesno, ele_vec.len());

            for ele_data in ele_vec {
                let refno = RefnoEnum::from(ele_data.refno);

                // 判断是否为几何体相关的更新
                if ele_data.is_geometry_update() {
                    match &ele_data.detail {
                        EleOperationDetail::Deleted => {
                            // 删除操作：PE已添加删除标记，无需额外的模型处理
                            println!("元素 {} 被删除，PE已加删除标记，无需额外处理", refno);
                        }
                        EleOperationDetail::Modified(_) => {
                            // 修改操作：需要先清理旧数据，再重新生成模型
                            println!("元素 {} 被修改，需要重新生成模型", refno);
                            refnos_to_delete_inst_relate.insert(refno);
                            refnos_to_generate.insert(refno);
                        }
                        EleOperationDetail::Add(_) => {
                            // 新增操作：直接生成新的模型数据
                            println!("元素 {} 新增，需要生成模型", refno);
                            refnos_to_generate.insert(refno);
                        }
                        EleOperationDetail::None => {
                            // 无操作类型，跳过处理
                            continue;
                        }
                    }
                }
                // 判断是否为变换相关的更新
                else if ele_data.detail.is_transform_change() {
                    println!("元素 {} 发生变换变化，需要更新子树的world transform", refno);
                    refnos_to_update_transform.insert(refno);
                }
            }
        }

        // 记录各类任务的状态，避免在移动数据后无法访问
        let has_delete_tasks = !refnos_to_delete_inst_relate.is_empty();
        let has_generate_tasks = !refnos_to_generate.is_empty();
        let has_transform_tasks = !refnos_to_update_transform.is_empty();

        // 处理几何体相关的更新任务
        if has_delete_tasks || has_generate_tasks {
            println!(
                "需要删除inst_relate的元素数量: {}",
                refnos_to_delete_inst_relate.len()
            );
            println!("需要生成模型的元素数量: {}", refnos_to_generate.len());

            // 第一步：删除修改元素的旧inst_relate数据
            // 这是必要的清理步骤，确保不会有残留的旧数据影响新模型
            if has_delete_tasks {
                let refnos_vec: Vec<RefnoEnum> = refnos_to_delete_inst_relate.into_iter().collect();
                delete_inst_relate_cascade(&refnos_vec, 1000).await?;
            }

            // 第二步：批量生成新的模型数据
            // 使用深度模型生成算法，确保几何数据的完整性和准确性
            if has_generate_tasks {
                let refnos_vec: Vec<RefnoEnum> = refnos_to_generate.into_iter().collect();
                let db_option = get_db_option();

                println!("开始批量生成模型数据...");
                process_meshes_update_db_deep(&db_option, &refnos_vec).await?;
                println!("模型数据生成完成");
            }
        }

        // 处理变换相关的更新任务
        // 当元素的位置、旋转或缩放发生变化时，需要更新其世界变换矩阵
        if has_transform_tasks {
            println!(
                "需要更新world transform的元素数量: {}",
                refnos_to_update_transform.len()
            );
            self.update_world_transforms(&refnos_to_update_transform)
                .await?;
        }

        // 如果没有任何更新任务，记录日志信息
        if !has_delete_tasks && !has_generate_tasks && !has_transform_tasks {
            println!("本次增量更新中没有需要处理的模型或变换更新");
        }

        Ok(())
    }

    /// 更新指定参考号及其子树的世界变换矩阵
    ///
    /// 当元素的变换属性（位置、旋转、缩放）发生变化时，需要更新该元素及其所有子节点中
    /// 有inst_relate数据的世界变换矩阵。这确保了3D模型在场景中的正确显示。
    ///
    /// # 算法优化
    ///
    /// 采用三步优化策略：
    /// 1. **智能筛选**: 直接获取子树中所有有inst_relate的几何节点，避免无效计算
    /// 2. **批量计算**: 批量获取世界变换矩阵，减少函数调用开销
    /// 3. **批量更新**: 批量执行数据库更新操作，提高IO效率
    ///
    /// # 参数
    ///
    /// * `refnos` - 发生变换变化的根节点参考号集合
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<()>` - 成功返回Ok(())，失败返回错误信息
    ///
    /// # 性能特点
    ///
    /// - **高效查询**: 使用递归SQL查询一次性获取所有相关节点
    /// - **内存友好**: 分批处理避免大量数据同时加载到内存
    /// - **错误容忍**: 单个节点计算失败不影响其他节点的更新
    async fn update_world_transforms(&self, refnos: &HashSet<RefnoEnum>) -> anyhow::Result<()> {
        use crate::SUL_DB;
        use aios_core::get_world_transform;

        // 如果没有需要更新的节点，直接返回
        if refnos.is_empty() {
            return Ok(());
        }

        println!("开始更新 {} 个元素及其子树的world transform", refnos.len());

        // 第一步：智能筛选 - 获取子树中所有有inst_relate的几何节点
        let refnos_with_inst_relate = self.get_inst_relate_nodes_in_subtree(refnos).await?;

        if refnos_with_inst_relate.is_empty() {
            println!("子树中没有节点有inst_relate数据，无需更新world transform");
            return Ok(());
        }

        println!(
            "子树中有inst_relate数据的节点数量: {}",
            refnos_with_inst_relate.len()
        );

        // 第二步：分批处理 - 避免单次处理过多数据
        let refnos_vec: Vec<RefnoEnum> = refnos_with_inst_relate.into_iter().collect();

        for chunk in refnos_vec.chunks(TRANSFORM_BATCH_SIZE) {
            let mut update_sqls = Vec::new();

            // 第三步：批量计算和更新
            for &refno in chunk {
                // 重新计算该节点的世界变换矩阵
                if let Some(world_transform) = get_world_transform(refno).await? {
                    let update_sql = format!(
                        "UPDATE {} SET world_trans = {};",
                        refno.to_inst_relate_key(),
                        serde_json::to_string(&world_transform)
                            .map_err(|e| anyhow::anyhow!("序列化Transform失败: {}", e))?
                    );
                    update_sqls.push(update_sql);
                } else {
                    // 记录警告但不中断处理流程
                    println!("警告: 无法计算元素 {} 的world transform", refno);
                }
            }

            // 批量执行数据库更新
            if !update_sqls.is_empty() {
                let batch_sql = update_sqls.join("");
                println!("执行world transform更新SQL，批次大小: {}", chunk.len());
                SUL_DB
                    .query(batch_sql)
                    .await
                    .map_err(|e| anyhow::anyhow!("更新world transform失败: {}", e))?;
            }
        }

        println!("world transform更新完成");
        Ok(())
    }

    /// 获取指定参考号及其子树中所有有inst_relate数据的几何节点
    ///
    /// 这是一个高性能的树遍历查询方法，用于在复杂的层次结构中快速定位需要更新的几何节点。
    /// 该方法采用单次递归SQL查询替代传统的多次查询方式，显著提升了查询效率。
    ///
    /// # 算法优势
    ///
    /// - **一次查询**: 使用递归SQL一次性获取整个子树的相关节点
    /// - **智能过滤**: 只返回有inst_relate数据的节点，避免无效处理
    /// - **深度遍历**: 支持最多10层的递归查询，覆盖复杂的层次结构
    /// - **容错机制**: 提供回退策略，确保在复杂查询失败时仍能正常工作
    ///
    /// # 参数
    ///
    /// * `refnos` - 根节点的参考号集合，作为遍历的起始点
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<HashSet<RefnoEnum>>` - 子树中有inst_relate数据的参考号集合
    ///
    /// # SQL查询说明
    ///
    /// 查询使用SurrealDB的递归语法，通过pe_owner关系向下遍历：
    /// - 检查根节点本身是否有inst_relate
    /// - 递归检查所有子节点（最多10层）
    /// - 过滤掉已删除的节点
    /// - 返回去重后的结果集
    async fn get_inst_relate_nodes_in_subtree(
        &self,
        refnos: &HashSet<RefnoEnum>,
    ) -> anyhow::Result<HashSet<RefnoEnum>> {
        use crate::SUL_DB;

        // 如果输入为空，直接返回空结果
        if refnos.is_empty() {
            return Ok(HashSet::new());
        }

        let mut result = HashSet::new();

        // 分批处理，避免SQL语句过长导致性能问题
        let refnos_vec: Vec<&RefnoEnum> = refnos.iter().collect();

        for chunk in refnos_vec.chunks(QUERY_BATCH_SIZE) {
            // 构建PE键列表，用于SQL查询
            let pe_keys: String = chunk
                .iter()
                .map(|refno| refno.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");

            // 构建递归SQL查询
            // 该查询实现了以下逻辑：
            // 1. 从指定的根节点开始遍历
            // 2. 递归获取所有子节点（支持最多10层深度）
            // 3. 只返回那些有inst_relate记录的节点
            // 4. 过滤掉已删除的节点
            let sql = format!(
                r#"
                array::distinct(array::flatten(
                    select value [
                        if record::exists(type::thing('inst_relate', record::id(id))) {{ [id] }} else {{ [] }},
                        array::flatten(
                            select value if record::exists(type::thing('inst_relate', record::id(in))) {{ [in] }} else {{ [] }}
                            from [{}]<-pe_owner<-(? as p1)<-pe_owner<-(? as p2)<-pe_owner<-(? as p3)
                            <-pe_owner<-(? as p4)<-pe_owner<-(? as p5)<-pe_owner<-(? as p6)<-pe_owner<-(? as p7)
                            <-pe_owner<-(? as p8)<-pe_owner<-(? as p9)<-pe_owner<-(? as p10)
                            where record::exists(in.id) and !in.deleted
                        )
                    ] from [{}]
                ))
                "#,
                pe_keys, pe_keys
            );

            println!("执行inst_relate节点查询，批次大小: {}", chunk.len());

            // 执行查询并处理结果
            match SUL_DB.query(sql).await {
                Ok(mut response) => {
                    if let Ok(refnos_result) = response.take::<Vec<RefnoEnum>>(0) {
                        println!("找到 {} 个有inst_relate的节点", refnos_result.len());
                        result.extend(refnos_result);
                    }
                }
                Err(e) => {
                    println!("批量查询inst_relate节点失败: {}", e);
                    // 容错机制：如果复杂查询失败，回退到逐个检查的方式
                    // 这确保了系统的健壮性，即使在极端情况下也能正常工作
                    for &refno in chunk {
                        if self
                            .check_single_inst_relate_exists(refno)
                            .await
                            .unwrap_or(false)
                        {
                            result.insert(*refno);
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// 检查单个参考号是否存在inst_relate记录
    ///
    /// 这是一个轻量级的检查方法，用作复杂查询失败时的回退机制。
    /// 通过简单的计数查询来确定指定的参考号是否有对应的inst_relate数据。
    ///
    /// # 参数
    ///
    /// * `refno` - 需要检查的参考号
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<bool>` - 如果存在inst_relate记录返回true，否则返回false
    ///
    /// # 使用场景
    ///
    /// 该方法主要用于以下情况：
    /// - 作为复杂递归查询的回退方案
    /// - 单个节点的快速检查
    /// - 调试和验证用途
    ///
    /// # 性能考虑
    ///
    /// - 使用COUNT查询而非SELECT *，减少数据传输
    /// - 包含record::exists检查，确保记录真实存在
    /// - 错误容忍设计，查询失败时返回false而非抛出异常
    async fn check_single_inst_relate_exists(&self, refno: &RefnoEnum) -> anyhow::Result<bool> {
        use crate::SUL_DB;

        // 构建inst_relate表的键名
        let inst_relate_key = refno.to_inst_relate_key();

        // 使用COUNT查询检查记录是否存在
        // record::exists确保记录真实存在而非仅仅是表结构
        let sql = format!(
            "SELECT count() FROM {} WHERE record::exists(id)",
            inst_relate_key
        );

        match SUL_DB.query(sql).await {
            Ok(mut response) => {
                // 尝试获取计数结果
                if let Ok(count) = response.take::<Option<i64>>(0) {
                    Ok(count.unwrap_or(0) > 0)
                } else {
                    // 如果无法解析结果，认为记录不存在
                    Ok(false)
                }
            }
            Err(_) => {
                // 如果查询失败，可能的原因：
                // 1. 表不存在
                // 2. 网络问题
                // 3. 权限问题
                // 为了保持系统稳定性，返回false而不是抛出异常
                Ok(false)
            }
        }
    }
}
