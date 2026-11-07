//! 库文件清单。
//!
//! 这个模块曾经是 legacy 全量同步的家（`sync_pdms` → `sync_total_async_threaded` →
//! `bootstrap_sys_metadata`），三样在 2026-09-07 依次退役；同日最后清掉的是 MySQL 那一摊
//! （`MySqlMethods` / `create_project_database` / `create_info_database` / `execute_sql`，
//! 全在 `feature = "sql"` 后面）。它们不只是没人调：`AiosDBMgr::get_global_pool` /
//! `get_project_pools` 早已不在 `aios_core` 里，`cargo check --features sql` 就停在这几行上
//! ——留着的是一段**编不过**的死码，比没有更坏（它让人以为还有一条 MySQL 落库路）。
//! 持久层只有 SurrealDB 一个（宪法 I）。

use aios_core::SUL_DB;
use std::path::{Path, PathBuf};

/// 列出项目 `*000` 目录下需要解析的库文件。
///
/// 只保留普通文件：目录名不含 `.` 时会混进来，而 Windows 上 `File::open` 打开目录
/// 返回 PermissionDenied，会让整个解析任务 panic。抽取家族归并后只保留叶子
/// （主库被遮蔽）；同号兄弟抽取拒绝静默挑选。
/// `explicit_files`：调用方点名要解析的文件名（`included_db_files` 口径）。抽取家族
/// 归并会把被叶子 shadow 的主库从清单里删掉；但父层补缺（ADR-028 第 6 条）恰恰要
/// 点名解析主库——被点名的 shadow 文件必须回到清单，否则补缺同步静默空转。
pub(crate) fn collect_project_db_files(
    project_dir: &Path,
    explicit_files: Option<&[String]>,
) -> anyhow::Result<Vec<PathBuf>> {
    let target_dir = std::fs::read_dir(project_dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("000"))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("项目目录下没有 *000 数据库目录: {}", project_dir.display())
        })?;

    let mut numbered = Vec::new();
    let mut passthrough = Vec::new();
    for path in std::fs::read_dir(target_dir)?.filter_map(|entry| entry.ok().map(|e| e.path())) {
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.contains('.') {
            continue;
        }
        match crate::data_interface::extract_family::parse_extract_file_name(file_name) {
            Some(parsed) => numbered.push((".".to_string(), parsed.dbnum, path)),
            None => passthrough.push(path),
        }
    }
    let collapsed = crate::data_interface::extract_family::collapse_extract_families(numbered);
    if !collapsed.duplicate_keys.is_empty() {
        anyhow::bail!(
            "项目目录存在同号兄弟抽取，拒绝静默选一份: {:?}",
            collapsed.duplicate_keys
        );
    }
    let mut files: Vec<PathBuf> = collapsed
        .selected
        .into_iter()
        .map(|family| family.leaf_path)
        .collect();
    if let Some(explicit) = explicit_files {
        for parent in collapsed.shadowed_parents {
            let named = parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| explicit.iter().any(|want| want == name));
            if named {
                files.push(parent);
            }
        }
    }
    files.extend(passthrough);
    Ok(files)
}

// `preserve_unparsed_pe_metadata`（属性解不出来就只留 `pe` 拓扑行）2026-09-04 随 legacy 基线
// 一起删除：e3d-io 基线与增量同一条失败闸——白名单外的解码失败让这一库的基线失败、清库、
// 不推水位（退役收口方案 Q3 甲），不再有「拓扑在、属性残」的中间态。

/// Remove the retired statistics trigger (`update_dbnum_event`) that old databases may still carry.
pub async fn retire_dbnum_event() -> anyhow::Result<()> {
    crate::data_interface::pe_batch::retire_event_on(&SUL_DB).await
}

#[cfg(test)]
fn make_temp_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("gen-model-{tag}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn collect_project_db_files_skips_directories_and_prefers_extract_copy() {
    let root = make_temp_dir("dbfiles");
    let db_dir = root.join("TES000");
    std::fs::create_dir_all(&db_dir).unwrap();
    // 目录名不含 `.` 时会被当成待解析文件，Windows 上 File::open 打开目录返回
    // PermissionDenied，整个解析任务会 panic。
    std::fs::create_dir_all(db_dir.join("新建文件夹")).unwrap();
    for name in ["tes1002", "tes1002_0001", "tes1008", "tes1008.bak"] {
        std::fs::write(db_dir.join(name), b"stub").unwrap();
    }

    let mut files = collect_project_db_files(&root, None).unwrap();
    files.sort();
    let names = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["tes1002_0001", "tes1008"]);
    assert!(files.iter().all(|path| path.is_file()));

    std::fs::remove_dir_all(&root).unwrap();
}

/// ADR-028 父层补缺回归：被叶子 shadow 的主库，若被 `included_db_files` 点名，
/// 必须回到解析清单。回退到「无条件丢 shadow」的旧写法时这里会红——那正是
/// 补缺同步静默空转（一个文件都不解析却返回 Ok）的根因。
#[test]
fn collect_project_db_files_keeps_explicitly_named_shadowed_master() {
    let root = make_temp_dir("dbfiles-parent");
    let db_dir = root.join("TES000");
    std::fs::create_dir_all(&db_dir).unwrap();
    for name in ["tes1002", "tes1002_0001"] {
        std::fs::write(db_dir.join(name), b"stub").unwrap();
    }

    let explicit = vec!["tes1002".to_string()];
    let mut files = collect_project_db_files(&root, Some(&explicit)).unwrap();
    files.sort();
    let names = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["tes1002", "tes1002_0001"]);

    // 没点名时维持 shadow 语义，不回灌主库。
    let unnamed = collect_project_db_files(&root, Some(&["tes9999".to_string()])).unwrap();
    assert_eq!(unnamed.len(), 1);
    assert!(unnamed[0].ends_with("tes1002_0001"));

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn collect_project_db_files_rejects_sibling_extracts() {
    let root = make_temp_dir("dbfiles-sib");
    let db_dir = root.join("TES000");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::write(db_dir.join("tes9990_0001"), b"stub").unwrap();
    std::fs::write(db_dir.join("tes9990_0002"), b"stub").unwrap();
    let error =
        collect_project_db_files(&root, None).expect_err("sibling extracts must not be picked");
    assert!(error.to_string().contains("兄弟抽取"), "{error}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// legacy 全量解析入口与它的四个开关一起退役（ADR-058 D8）。原来这里钉的是
/// 「Meta → Catalogue → Design 三道全局屏障的先后」，那是 `sync_pdms` 自己的编排；
/// direct 成为默认形态后它一次也没被走到过（仓里每份配置都没打开这四个键）。
/// 同日晚些时候它的底座 `sync_total_async_threaded`（多类型 / 多文件 / `included_db_files`
/// 过滤的 legacy 壳）也退役了：基线摄入只剩 `versioned_db::baseline_ingest::ingest_file`
/// 一个按**单个文件**的入口，由 `manual_update` 的首载 / 重建路径调用，屏障由数据批次队列
/// 负责。最后一截 `bootstrap_sys_metadata`（空库 SYS 引导，`bin/sync_sys_only` 的全部内容）
/// 同日一并退役：SYS 元数据不是一道单独的引导，而是初始化的一部分——服务首轮重扫把它们
/// 当普通库入队首载（`needs_initial_load` 明说「SYS meta 也算」），规格工具走
/// `AiosDBManager::initialize_project_sys_metadata`，两条路同一个 `initialize_dbnum_baseline`。
///
/// 现在钉的是「别再长回来」：本文件不许重新出现那三个入口，也不许再读那四个开关，
/// 也不许再有第二个持久层（同日清掉的 MySQL 那一摊）。
#[test]
fn the_legacy_full_sync_entry_point_stays_retired() {
    let source = include_str!("database.rs");
    for retired in [
        "sync_pdms",
        "sync_total_async_threaded",
        "bootstrap_sys_metadata",
    ] {
        let entry = format!("pub async fn {retired}");
        assert!(
            !source.contains(&entry),
            "legacy 全量解析入口 {retired} 已退役（ADR-058 D8 / 2026-09-07 基线摄入方案），不许复活"
        );
    }
    for switch in [
        "total_sync",
        "incr_sync",
        "only_sync_sys",
        "is_sync_history",
    ] {
        assert!(
            !source.contains(&format!("db_option.{switch}")),
            "legacy 全量开关 {switch} 已退役，本文件不许再读它"
        );
    }
    // MySQL 落库路 2026-09-07 清掉时已经编不过（`AiosDBMgr::get_global_pool` /
    // `get_project_pools` 不在 aios_core 里了）——`feature = "sql"` 从来没人开，
    // 所以那段死码烂了很久也没人看见。持久层只有 SurrealDB 一个（宪法 I）。
    // 只看代码行：模块注释里点名了这几样，那是它们的墓志铭，不是复活。
    let code = source
        .split_once("fn the_legacy_full_sync_entry_point_stays_retired")
        .expect("this very test")
        .0
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>();
    for forbidden in ["sqlx", "MySql", "feature = \"sql\"", "dbg!"] {
        let hit = code.iter().find(|line| line.contains(forbidden));
        assert!(
            hit.is_none(),
            "本文件不许再出现 `{forbidden}`（MySQL 那条路 2026-09-07 退役，dbg! 不是日志）：{:?}",
            hit
        );
    }
}
