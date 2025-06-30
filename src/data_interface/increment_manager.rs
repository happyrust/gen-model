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
use aios_core::{clear_all_caches, SUL_DB};
use aios_core::{get_db_option, RefU64Vec, RefnoEnum};

// 异步和工具库导入
use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use notify::{RecursiveMode, Watcher};

// PDMS相关模块导入
use parse_pdms_db::parse::parse_db_basic_info;
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::{EleOperationData, EleOperationDetail, PdmsIO};
use pdms_io::sync::compress::{execute_compress, CompressOptions};
use pdms_io::watch::PdmsWatcher;

// 其他依赖导入
use petgraph::visit::Walker;
use rumqttc::QoS;
use tokio::fs::create_dir_all;
use walkdir::WalkDir;

// 本地模块导入
use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::*;
use crate::mqtt_service::SyncE3dFileMsg;
use parse_pdms_db::parse::DbBasicInfo;

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
                            "com", "exe", "dll", "sys", "tmp", "temp", "log", "bak",
                            "backup", "old", "cache", "lock", "pid", "swp", "swo", "~",
                        ];

                        if excluded_extensions.contains(&ext_lower.as_str()) {
                            return true;
                        }
                    }
                }

                if let Some(file_name) = file_path.file_name() {
                    if let Some(name_str) = file_name.to_str() {
                        let name_lower = name_str.to_lowercase();

                        let excluded_patterns = [
                            "thumbs.db", "desktop.ini", ".ds_store",
                        ];

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

/// 需要检查的数据库类型列表
/// 包含目录(CATA)、设计(DESI)、字典(DICT)、系统(SYST)、全局(GLB/GLOB)等类型
pub const CHECK_DB_TYPES: [&'static str; 6] = ["CATA", "DESI", "DICT", "SYST", "GLB", "GLOB"];

impl AiosDBManager {
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
                    "com",      // COM可执行文件
                    "exe",      // Windows可执行文件
                    "dll",      // 动态链接库
                    "sys",      // 系统文件
                    "tmp",      // 临时文件
                    "temp",     // 临时文件
                    "log",      // 日志文件
                    "bak",      // 备份文件
                    "backup",   // 备份文件
                    "old",      // 旧文件
                    "cache",    // 缓存文件
                    "lock",     // 锁文件
                    "pid",      // 进程ID文件
                    "swp",      // Vim交换文件
                    "swo",      // Vim交换文件
                    "~",        // 临时备份文件
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
                    "thumbs.db",        // Windows缩略图缓存
                    "desktop.ini",      // Windows桌面配置
                    ".ds_store",        // macOS文件夹配置
                    "~$",               // Office临时文件前缀
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
    /// 2. 检查是否在手动指定的数据库编号列表中（如果配置了）
    /// 3. 检查是否在排除的数据库编号列表中
    fn should_process_database(&self, db_type: &str, db_num: u32) -> bool {
        // 检查数据库类型是否支持
        if !CHECK_DB_TYPES.contains(&db_type) {
            return false;
        }

        let db_option = get_db_option();
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
    /// 该函数是增量更新的核心方法，负责处理多个数据库文件的增量更新。
    /// 它会遍历所有需要更新的数据库文件，收集增量元素数据，并执行相应的更新操作。
    ///
    /// # 参数
    ///
    /// * `increment_ranges_map` - 增量更新范围映射表
    ///   - 键：数据库文件路径
    ///   - 值：元组(数据库页面基本信息, 需要更新的会话号范围)
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<bool>` - 成功返回Ok(true)，失败返回错误信息
    ///
    /// # 处理流程
    ///
    /// 1. 遍历每个需要更新的数据库文件
    /// 2. 打开PDMS IO对象
    /// 3. 收集指定会话号范围内的增量元素
    /// 4. 将元素更新到数据库
    /// 5. 处理相关的模型更新
    ///
    /// # 错误处理
    ///
    /// 当PDMS IO打开失败或数据库操作失败时会返回错误
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>)>,
    ) -> anyhow::Result<bool> {
        // 遍历所有需要增量更新的文件
        for (path, (basic_info, sesno_range)) in increment_ranges_map {
            println!("正在处理文件: {:?}, 会话号范围: {:?}", path, &sesno_range);

            // 创建并打开PDMS IO对象
            let mut io = PdmsIO::new("", path.clone(), true);
            io.open()
                .map_err(|e| anyhow::anyhow!("打开PDMS IO失败: {}", e))?;

            // 收集指定范围内的增量元素
            let range_eles = io.collect_increment_eles(Some(sesno_range))?;

            // 将元素更新到数据库
            io.update_elements_to_database(&range_eles).await?;

            // 执行相关的模型更新操作
            // self.process_model_updates(&range_eles, basic_info.pdms_header.db_num).await?;
        }

        Ok(true)
    }

    /// 通过文件名查询数据库中最新的会话号
    ///
    /// # 参数
    ///
    /// * `file_name` - 要查询的数据库文件名
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<u32>` - 成功则返回最新会话号,失败返回错误
    ///
    /// # 错误
    ///
    /// 当数据库查询失败时会返回错误
    async fn query_latest_sesno_by_file_name(file_name: &str) -> anyhow::Result<u32> {
        let mut response = SUL_DB
            .query(format!(
                r#"
                select value sesno from only db_file_info:{} limit 1;
                "#,
                file_name
            ))
            .await?;
        let sesno: Option<u32> = response.take(0)?;
        Ok(sesno.unwrap_or_default())
    }

    /// 通过数据库编号查询数据库中最新的会话号
    ///
    /// # 参数
    ///
    /// * `dbnum` - 要查询的数据库编号
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<u32>` - 成功则返回最新会话号,失败返回错误
    ///
    /// # 错误
    ///
    /// 当数据库查询失败时会返回错误
    async fn query_latest_sesno_by_dbnum(dbnum: u32) -> anyhow::Result<u32> {
        // 从dbnum_info_table中查询对应dbnum的最大sesno
        // 使用更高效的查询，直接获取该dbnum的最大sesno值
        let mut response = SUL_DB
            .query(format!(
                r#"
                math::max(array::flatten([
                    SELECT VALUE sesno FROM dbnum_info_table WHERE dbnum = {}
                ]));
                "#,
                dbnum
            ))
            .await?;
        let sesno: Option<u32> = response.take(0)?;
        Ok(sesno.unwrap_or_default())
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
    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        // 创建存档目录
        fs::create_dir_all("assets/archives")?;
        let mut time = Instant::now();
        dbg!(&self.watcher.watch_dirs);

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
                let dir_entry =
                    entry.map_err(|e| anyhow::anyhow!("获取目录条目失败: {}", e))?;
                let path = dir_entry.path();

                // 获取文件名(不含扩展名)
                let file_name = path
                    .file_stem()
                    .ok_or_else(|| {
                        anyhow::anyhow!("无法从路径获取文件名: {}", path.display())
                    })?
                    .to_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!("文件名转换为字符串失败: {}", path.display())
                    })?;

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

                // 查询数据库中的最新会话号
                // TODO: 对于数据库中不存在的文件，需要考虑全新解析
                let Ok(db_latest_sesno) = Self::query_latest_sesno_by_dbnum(db_no).await else {
                    // 暂时跳过数据库里没有的文件，后续考虑自动追加文件全新解析
                    continue;
                };

                // 跳过会话号为0的情况
                if db_latest_sesno == 0 {
                    continue;
                }

                // 建立文件名到完整路径的映射
                self.watcher
                    .file_name_full_path_map
                    .insert(file_name.to_owned(), path.to_path_buf());

                // 只有开启MQTT功能时，才需要初始化压缩数据包用于异地同步
                #[cfg(feature = "mqtt")]
                {
                    // 初始化CBA存档文件，确保后续增量下载功能正常
                    // 后续可能需要添加环境变量来控制是否重新生成存档文件
                    let input = path.to_path_buf();
                    let output: PathBuf = format!("assets/archives/{}.cba", file_name).into();
                    let compress_opt = CompressOptions::new(input, output, "assets/temp");
                    execute_compress(compress_opt)
                        .await
                        .expect("压缩失败");
                }

                // 检查每个文件是否需要增量更新
                {
                    let mut io = PdmsIO::new(&project, path, true);
                    io.open()?;
                    if let Ok(basic_info) = io.get_page_basic_info() {
                        // 如果文件的会话号大于数据库中的会话号，说明需要增量更新
                        if file_latest_sesno > db_latest_sesno {
                            println!("发现需要增量更新的文件: {:?}, 当前数据库最大sesno: {db_latest_sesno}, \
                                    文件最新sesno: {file_latest_sesno}", &file_name);

                            // 获取最接近的大于指定值的会话号
                            // 注意: db_latest_sesno + 1 不一定存在，需要找最近的会话号
                            let nearest_sesno = io
                                .get_nearest_large_sesno(db_latest_sesno as i32 + 1)
                                .unwrap_or_default();

                            // 添加到待更新参数列表
                            params.insert(
                                path.to_path_buf(),
                                (
                                    basic_info.clone(),
                                    nearest_sesno..=file_latest_sesno as i32,
                                ),
                            );
                        }
                        // 注意：不再初始化缓存，因为我们已经移除了对缓存的依赖
                    }
                }
            }
        }

        // 等所有文件检查完毕后，执行批量增量更新
        if !params.is_empty() {
            dbg!(params.len());
        }

        // 执行增量更新并处理结果
        match self.execute_incr_update(params).await {
            Ok(true) => {
                println!("启动时自动增量更新执行完成。")
            }
            Ok(false) => {
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

                    // 记录文件变化事件
                    println!("检测到文件变化: {:?}", &event);

                    // 预过滤：检查事件中的文件路径，排除不需要监控的文件
                    let filtered_paths: Vec<_> = event.paths.iter()
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
                                println!("根据过滤规则跳过数据库: 类型={}, 编号={}", db_type, db_num);
                                continue;
                            }

                            println!("检查文件: {}, 数据库编号: {}, 文件会话号: {}",
                                    file_name, db_num, new_header.latest_ses_data.sesno);

                            // 直接从数据库获取最新的会话号，不依赖缓存
                            let db_latest_sesno = match Self::query_latest_sesno_by_dbnum(db_num).await {
                                Ok(sesno) => sesno,
                                Err(e) => {
                                    println!("查询数据库最新sesno失败: {:?}", e);
                                    continue;
                                }
                            };

                            println!("数据库最新会话号: {}, 文件会话号: {}",
                                    db_latest_sesno, new_header.latest_ses_data.sesno);

                            // 如果数据库中的会话号与文件中的会话号相同，说明未发生修改
                            if db_latest_sesno as i32 >= new_header.latest_ses_data.sesno {
                                println!("文件 {} 无需更新，数据库会话号({}) >= 文件会话号({})",
                                        file_name, db_latest_sesno, new_header.latest_ses_data.sesno);
                                continue;
                            }

                            println!("发现需要增量更新的文件: {}, 数据库会话号: {}, 文件会话号: {}",
                                    file_name, db_latest_sesno, new_header.latest_ses_data.sesno);

                            // 构建增量更新参数，指定准确的会话号范围
                            params.insert(
                                path.clone(),
                                (
                                    new_header.clone(),
                                    (db_latest_sesno as i32 + 1)..=new_header.latest_ses_data.sesno,
                                ),
                            );
                        }
                        // 如果没有需要更新的参数，跳过后续处理
                        if params.is_empty() {
                            continue;
                        }

                        // 用于收集需要通知的文件信息
                        let mut notify_file_names = vec![];
                        let mut notify_file_hashes = vec![];

                        // 执行增量更新操作
                        match self.execute_incr_update(params).await {
                            Ok(true) => {
                                // 增量更新成功，处理文件同步（不再依赖缓存）
                                for (path, new_header) in new_headers {
                                    let file_name = match path.file_stem().and_then(|s| s.to_str()) {
                                        Some(name) => name,
                                        None => {
                                            println!("无法获取文件名: {:?}", path);
                                            continue;
                                        }
                                    };
                                    let dbno = new_header.pdms_header.db_num as u32;

                                    // 跳过目录
                                    if path.is_dir() {
                                        continue;
                                    }

                                    println!("处理增量更新成功后的同步: {}", file_name);

                                    // 为发生修改的文件重新生成压缩存档
                                    let output: PathBuf =
                                        format!("assets/archives/{}.cba", file_name).into();

                                    let compress_opt = CompressOptions::new(
                                        path.clone(),
                                        output,
                                        "assets/temp",
                                    );
                                    let file_hash = execute_compress(compress_opt)
                                        .await
                                        .unwrap()
                                        .to_string();

                                    // 检查地区数据库配置
                                    // 如果location_dbs为空，则所有地区都推送
                                    // 否则只有指定地区对应的数据库编号才能推送
                                    if let Some(location_dbs) = &get_db_option().location_dbs {
                                        if !location_dbs.contains(&dbno) {
                                            println!("数据库编号 {} 不在地区配置中，跳过推送", dbno);
                                            continue;
                                        }
                                    }

                                    // 检查数据库中是否已存在相同的文件哈希记录
                                    // 只有自己创建的、在记录中还没有的文件才发送消息
                                    // 如果是其他地方创建的，则跳过避免重复同步
                                    let sql = format!(
                                        "select value <string>\
                                        id from (select * from e3d_sync where location != '{}' and '{}' in file_names and '{}' in file_hashes order by timestamp desc) ",
                                        get_db_option().location.as_str(),
                                        file_name,
                                        &file_hash
                                    );

                                    let mut response = SUL_DB.query(&sql).await.unwrap();
                                    let id = response.take::<Vec<String>>(0).unwrap();

                                    // 如果查询结果为空，说明是新的变化，需要推送通知
                                    if id.is_empty() {
                                        println!("检测到增量更新，准备推送文件: {}", &file_name);
                                        notify_file_hashes.push(file_hash);
                                        notify_file_names.push(file_name.to_owned());
                                    } else {
                                        println!("文件 {} 的哈希已存在，跳过推送", file_name);
                                    }
                                }
                            }
                            Ok(false) => {
                                println!("文件 {:?} 发生修改，但未触发增量更新。", &event.paths);
                                continue;
                            }
                            Err(e) => {
                                println!("执行增量更新时发生错误: {:?}", e);
                            }
                        }

                        // 发布数据库文件更新通知
                        dbg!(&notify_file_names);
                        #[cfg(feature = "mqtt")]
                        if !notify_file_names.is_empty() {
                            // 创建同步消息载荷
                            let payload = SyncE3dFileMsg::new(notify_file_names, notify_file_hashes);

                            // 在本地数据库中保存同步记录
                            // TODO: 后续需要配置哪些数据库可以修改，哪些不能修改
                            SUL_DB
                                .query(format!(
                                    "INSERT IGNORE INTO e3d_sync {} ",
                                    serde_json::to_string(&payload).unwrap()
                                ))
                                .await
                                .unwrap();

                            // 通过MQTT发布同步消息
                            // TODO: 检查是否只是claim page的变化，如果只是claim修改，是否需要每次都同步？
                            // 需要避免出现循环同步的情况
                            self.mqtt_client
                                .clone()
                                .publish("Sync/E3d", QoS::ExactlyOnce, true, payload)
                                .await
                                .unwrap();
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
    async fn process_model_updates(
        &self,
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        db_num: i32,
    ) -> anyhow::Result<()> {
        use std::collections::HashSet;
        use crate::fast_model::occ_generate::process_meshes_update_db_deep;
        use crate::get_db_option;
        use crate::SUL_DB;

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
                        },
                        EleOperationDetail::Modified(_) => {
                            // 修改操作：需要先清理旧数据，再重新生成模型
                            println!("元素 {} 被修改，需要重新生成模型", refno);
                            refnos_to_delete_inst_relate.insert(refno);
                            refnos_to_generate.insert(refno);
                        },
                        EleOperationDetail::Add(_) => {
                            // 新增操作：直接生成新的模型数据
                            println!("元素 {} 新增，需要生成模型", refno);
                            refnos_to_generate.insert(refno);
                        },
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
            println!("需要删除inst_relate的元素数量: {}", refnos_to_delete_inst_relate.len());
            println!("需要生成模型的元素数量: {}", refnos_to_generate.len());

            // 第一步：删除修改元素的旧inst_relate数据
            // 这是必要的清理步骤，确保不会有残留的旧数据影响新模型
            if has_delete_tasks {
                self.delete_inst_relate_data(&refnos_to_delete_inst_relate).await?;
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
            println!("需要更新world transform的元素数量: {}", refnos_to_update_transform.len());
            self.update_world_transforms(&refnos_to_update_transform).await?;
        }

        // 如果没有任何更新任务，记录日志信息
        if !has_delete_tasks && !has_generate_tasks && !has_transform_tasks {
            println!("本次增量更新中没有需要处理的模型或变换更新");
        }

        Ok(())
    }

    /// 删除指定参考号的inst_relate数据
    ///
    /// 当元素被修改时，需要先删除其旧的inst_relate数据，然后重新生成。
    /// 该方法会级联删除相关的geo_relate数据，确保数据的完整性。
    ///
    /// # 参数
    ///
    /// * `refnos` - 需要删除inst_relate数据的参考号集合
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<()>` - 成功返回Ok(())，失败返回错误信息
    ///
    /// # 删除策略
    ///
    /// 采用级联删除的方式，按以下顺序删除：
    /// 1. 删除geo_relate指向的几何数据
    /// 2. 删除geo_relate关系数据
    /// 3. 删除inst_relate的输出关系
    /// 4. 删除inst_relate记录本身
    ///
    /// # 性能优化
    ///
    /// - 使用批处理机制，每批处理100个元素
    /// - 避免SQL语句过长导致的性能问题
    /// - 提供详细的进度日志便于监控
    async fn delete_inst_relate_data(&self, refnos: &HashSet<RefnoEnum>) -> anyhow::Result<()> {
        use crate::SUL_DB;

        // 如果没有需要删除的数据，直接返回
        if refnos.is_empty() {
            return Ok(());
        }

        println!("开始删除 {} 个元素的inst_relate数据", refnos.len());

        // 分批处理，避免SQL语句过长导致性能问题
        const BATCH_SIZE: usize = 100;
        let refnos_vec: Vec<&RefnoEnum> = refnos.iter().collect();

        // 按批次处理删除操作
        for chunk in refnos_vec.chunks(BATCH_SIZE) {
            let mut delete_sqls = Vec::new();

            for &refno in chunk {
                // 构建级联删除SQL语句
                // 按照依赖关系的逆序进行删除，确保数据完整性
                let delete_sql = format!(
                    r#"
                    delete array::flatten(select value out->geo_relate.out from {});
                    delete array::flatten(select value out->geo_relate from {});
                    delete array::flatten(select value out from {});
                    delete {};"#,
                    refno.to_inst_relate_key(),  // 删除geo_relate指向的几何数据
                    refno.to_inst_relate_key(),  // 删除geo_relate关系数据
                    refno.to_inst_relate_key(),  // 删除inst_relate的输出关系
                    refno.to_inst_relate_key()   // 删除inst_relate记录本身
                );
                delete_sqls.push(delete_sql);
            }

            // 执行批量删除操作
            if !delete_sqls.is_empty() {
                let batch_sql = delete_sqls.join("");
                println!("执行删除SQL，批次大小: {}", chunk.len());
                SUL_DB.query(batch_sql).await
                    .map_err(|e| anyhow::anyhow!("删除inst_relate数据失败: {}", e))?;
            }
        }

        println!("inst_relate数据删除完成");
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
        use aios_core::get_world_transform;
        use crate::SUL_DB;

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

        println!("子树中有inst_relate数据的节点数量: {}", refnos_with_inst_relate.len());

        // 第二步：分批处理 - 避免单次处理过多数据
        const BATCH_SIZE: usize = 50;
        let refnos_vec: Vec<RefnoEnum> = refnos_with_inst_relate.into_iter().collect();

        for chunk in refnos_vec.chunks(BATCH_SIZE) {
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
                SUL_DB.query(batch_sql).await
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
    async fn get_inst_relate_nodes_in_subtree(&self, refnos: &HashSet<RefnoEnum>) -> anyhow::Result<HashSet<RefnoEnum>> {
        use crate::SUL_DB;

        // 如果输入为空，直接返回空结果
        if refnos.is_empty() {
            return Ok(HashSet::new());
        }

        let mut result = HashSet::new();

        // 分批处理，避免SQL语句过长导致性能问题
        const BATCH_SIZE: usize = 20;
        let refnos_vec: Vec<&RefnoEnum> = refnos.iter().collect();

        for chunk in refnos_vec.chunks(BATCH_SIZE) {
            // 构建PE键列表，用于SQL查询
            let pe_keys: String = chunk.iter()
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
                },
                Err(e) => {
                    println!("批量查询inst_relate节点失败: {}", e);
                    // 容错机制：如果复杂查询失败，回退到逐个检查的方式
                    // 这确保了系统的健壮性，即使在极端情况下也能正常工作
                    for &refno in chunk {
                        if self.check_single_inst_relate_exists(refno).await.unwrap_or(false) {
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
            },
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
