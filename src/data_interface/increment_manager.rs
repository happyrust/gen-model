// 标准库导入
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

// AIOS核心模块导入
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::version::{backup_data, backup_owner_relate};
use aios_core::{RefU64Vec, RefnoEnum};
use aios_core::{get_default_name, get_pe};

// 异步和工具库导入
use futures::{SinkExt, StreamExt};
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use notify::{Config, PollWatcher, RecursiveMode, Watcher};

// PDMS相关模块导入
use parse_pdms_db::parse::{DbBasicInfo, parse_file_basic_info};
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::{EleOperationData, EleOperationDetail, PdmsIO};
use pdms_io::watch::PdmsWatcher;

// 其他依赖导入
use petgraph::visit::Walker;
use tokio::fs::create_dir_all;
use walkdir::WalkDir;

// 本地模块导入
use crate::api::element::gen_pdms_element_insert_sql;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::project_paths::{MountState, path_starts_with};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_interface::update_scope::UpdateScope;
use crate::fast_model::*;
use tracing_subscriber::fmt::format;

use crate::consts::PDMS_ELEMENTS_TABLE;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn duplicate_dbnums_are_detected_across_separate_paths() {
        let duplicates = duplicate_dbnums([
            ("AMS".to_string(), 7997, PathBuf::from("first")),
            ("AMS".to_string(), 8000, PathBuf::from("only")),
            ("AMS".to_string(), 7997, PathBuf::from("second")),
        ]);
        assert_eq!(duplicates, HashSet::from([("AMS".to_string(), 7997)]));
    }

    /// 不同项目各自的 sys 库天然共用 dbnum=8191（amssys / acpsys / zdjsys）。
    /// 只按 dbnum 判重会把三个正常的库一起阻断——实测就是这么发生的。
    #[test]
    fn same_dbnum_in_different_projects_is_not_a_duplicate() {
        let duplicates = duplicate_dbnums([
            (
                "AvevaMarineSample".to_string(),
                8191,
                PathBuf::from("ams000/amssys"),
            ),
            (
                "AvevaCatalogue".to_string(),
                8191,
                PathBuf::from("acp000/acpsys"),
            ),
            ("ZDJ".to_string(), 8191, PathBuf::from("ZDJ000/zdjsys")),
        ]);
        assert!(duplicates.is_empty(), "跨项目同号不是重复: {duplicates:?}");
    }

    /// 同一个项目里的人手副本仍然要被拦住——这才是 F6 本来要防的东西。
    #[test]
    fn a_copy_inside_one_project_is_still_blocked() {
        let duplicates = duplicate_dbnums([
            (
                "AMS".to_string(),
                1112,
                PathBuf::from("ams000/ams1112_0001"),
            ),
            (
                "AMS".to_string(),
                1112,
                PathBuf::from("ams000/ams1112_0001 copy"),
            ),
        ]);
        assert_eq!(duplicates, HashSet::from([("AMS".to_string(), 1112)]));
    }

    /// 扫描裁决到自动路径处置的映射是策略红线（ADR-021）：回退转重建、其余阻断
    /// 类异常跳过、无异常与良性搬家放行。逐类点名（不留 `_ =>` 兜底）：新增异常
    /// 种类时这里编译不过，作者必须显式选边。
    #[test]
    fn the_scan_gate_maps_every_verdict_shape() {
        use crate::data_interface::dbnum_state::{FileAnomaly, ScanVerdict};

        let gate = |anomaly: Option<FileAnomaly>| {
            scan_gate_for(&ScanVerdict {
                prior: None,
                anomaly,
            })
        };
        assert_eq!(gate(None), ScanGate::Proceed);
        assert_eq!(
            gate(Some(FileAnomaly::PathMigrated {
                old_path: "/old".into(),
                new_path: "/new".into(),
            })),
            ScanGate::Proceed,
            "良性搬家照常放行"
        );
        assert_eq!(
            gate(Some(FileAnomaly::Rollback {
                file_latest_sesno: 114,
                applied_sesno: 120,
                file_latest_sesno_time: None,
                applied_sesno_time: None,
            })),
            ScanGate::Reinit,
            "回退不阻断，转整库重建入队"
        );
        assert_eq!(
            gate(Some(FileAnomaly::TypeChanged {
                stored_db_type: "DESI".into(),
                observed_db_type: "SYST".into(),
            })),
            ScanGate::Blocked
        );
        assert_eq!(
            gate(Some(FileAnomaly::Duplicate {
                paths: vec!["/a".into(), "/b".into()],
            })),
            ScanGate::Blocked
        );
        assert_eq!(
            gate(Some(FileAnomaly::Missing {
                path: "/gone".into()
            })),
            ScanGate::Blocked
        );
        assert_eq!(
            gate(Some(FileAnomaly::ForeignProject {
                stored_project: "AMS".into(),
                observed_project: "ZDJ".into(),
            })),
            ScanGate::Blocked,
            "身份歧义类异常照旧阻断，绝不自动清库"
        );
    }

    /// B3（2026-07-26 审计）：`DbnumState::record_scan` 按 dbnum UPSERT 文件身份字段
    /// （file_name / file_path / file_size / file_latest_sesno）。同一 dbnum 的第二个文件
    /// 若先走 `scan_and_check_file`，就会把首见文件的身份覆盖掉——此后即便阻断了该
    /// dbnum，回退 / 迁移检测的基准也已经被污染。故 `sweep_watch_dirs`（启动重扫与
    /// 范围刷新重扫共用）与 `async_watch` 两条自动路径都必须先做重复 dbnum 阻断、
    /// 再落库观察。
    ///
    /// 这两步嵌在依赖实库的大函数里，没法用纯函数钉住，所以直接在源码上钉顺序。
    /// marker 用 `concat!` 拼接，避免本测试自己的字符串字面量先于真函数被命中。
    #[test]
    fn duplicate_dbnum_guard_precedes_scan_record_on_both_auto_paths() {
        let src = include_str!("increment_manager.rs");
        for (name, marker) in [
            (
                "sweep_watch_dirs",
                concat!("async fn ", "sweep_watch_dirs("),
            ),
            ("async_watch", concat!("pub async fn ", "async_watch(")),
        ] {
            let body = src
                .split_once(marker)
                .unwrap_or_else(|| panic!("{name} 未找到"))
                .1;
            let guard_at = body
                .find("seen_dbnums.insert(")
                .unwrap_or_else(|| panic!("{name}: 缺少重复 dbnum 阻断"));
            let scan_at = body
                .find(".scan_and_check_file(")
                .unwrap_or_else(|| panic!("{name}: 缺少 scan_and_check_file 调用"));
            assert!(
                guard_at < scan_at,
                "{name}: 重复 dbnum 阻断必须先于 scan_and_check_file，\
                 否则重复文件会覆盖 dbnum_watermark 里的文件身份字段"
            );
        }
    }

    /// 手动与自动喂的是同一个队列，入队口径只能有一份。自动路径过去只过类型白名单
    /// 与手写的 dbnum 名单，于是 MDB 外的设计库照样入队——预览说它不在本期执行
    /// 范围里，队列里却有它的任务行。
    ///
    /// 手法同上：这道门嵌在依赖实库的大函数里，没法用纯函数钉住，只能钉源码。
    #[test]
    fn both_auto_paths_gate_on_the_shared_scope_predicate() {
        let src = include_str!("increment_manager.rs");
        for (name, marker) in [
            (
                "sweep_watch_dirs",
                concat!("async fn ", "sweep_watch_dirs("),
            ),
            ("async_watch", concat!("pub async fn ", "async_watch(")),
        ] {
            let body = src
                .split_once(marker)
                .unwrap_or_else(|| panic!("{name} 未找到"))
                .1;
            let gate_at = body
                .find(".in_scope(")
                .unwrap_or_else(|| panic!("{name}: 缺少本期执行范围门控"));
            let discover_at = body
                .find(".discover_batch(")
                .unwrap_or_else(|| panic!("{name}: 缺少 discover_batch 调用"));
            assert!(
                gate_at < discover_at,
                "{name}: 入队前必须先过 in_scope，否则 MDB 外的库会被自动路径带进队列，\
                 而手动预览还一口咬定它不在范围里"
            );
        }
    }

    /// CATA 永远不进执行范围（update_scope 决策 A），但按需 CATA 闭包的定位器
    /// 从 `dbnum_watermark` 读目录文件身份（`cata_closure` 的登记契约）。全新库上
    /// 若范围门直接跳过 CATA，登记行永远不出现，闭包对分支组件解不出任何依赖
    /// （parsed=0），BRAN/HANG 的目录几何整体生成不出来——生产老库靠范围纪律
    /// 之前的遗留登记行掩盖了这个坑（2026-08-13 testbed 首次暴露）。
    ///
    /// 钉两件事：范围外 CATA 的登记豁免分支在 `scan_and_check_file` 之前；
    /// scan 之后、`discover_batch` 之前有一道 `in_scope` 收尾门（只登记不入队）。
    #[test]
    fn out_of_scope_cata_is_recorded_but_never_enqueued() {
        let src = include_str!("increment_manager.rs");
        let body = src
            .split_once(concat!("async fn ", "sweep_dirs("))
            .expect("sweep_dirs 未找到")
            .1;
        let cata_exempt_at = body
            .find(concat!("eq_ignore_ascii_case(\"", "CATA\")"))
            .expect("sweep_dirs: 缺少范围外 CATA 的登记豁免分支");
        let scan_at = body
            .find(".scan_and_check_file(")
            .expect("sweep_dirs: 缺少 scan_and_check_file 调用");
        let enqueue_gate_at = body[scan_at..]
            .find("if !in_scope")
            .expect("sweep_dirs: scan 之后缺少范围收尾门（范围外 CATA 只登记不入队）")
            + scan_at;
        let discover_at = body
            .find(".discover_batch(")
            .expect("sweep_dirs: 缺少 discover_batch 调用");
        assert!(
            cata_exempt_at < scan_at,
            "范围外 CATA 的豁免判定必须在观察落库之前，否则登记行仍然写不进去"
        );
        assert!(
            enqueue_gate_at < discover_at,
            "范围收尾门必须在 discover_batch 之前，否则范围外 CATA 会被带进队列"
        );
    }

    /// 两条自动路径给批次定的「归属项目」必须来自文件所在的监控目录，不能是配置里
    /// 的主项目名。
    ///
    /// 后者是 SurrealDB 的库名，拿它当归属的后果实测过：`acp000\acp7006_0001` 被记成
    /// `AvevaMarineSample`，执行侧于是去 `ams000` 里找它，`initialize_project_dbnum_baseline`
    /// 报「项目 AvevaMarineSample 未找到 dbnum=7006」，批次每一轮都 failed 一次，
    /// 而日志里只有一句「状态 failed」，看不出是归属记错了。
    ///
    /// 这一步嵌在依赖实库的大函数里，没法用纯函数钉住，所以直接钉源码。
    #[test]
    fn both_auto_paths_take_the_owning_project_from_the_watch_dir() {
        let src = include_str!("increment_manager.rs");
        for (name, marker) in [
            (
                "sweep_watch_dirs",
                concat!("async fn ", "sweep_watch_dirs("),
            ),
            ("async_watch", concat!("pub async fn ", "async_watch(")),
        ] {
            let body = src
                .split_once(marker)
                .unwrap_or_else(|| panic!("{name} 未找到"))
                .1;
            let discover_at = body
                .find(".discover_batch(")
                .unwrap_or_else(|| panic!("{name}: 缺少 discover_batch 调用"));
            let owning_at = body
                .find(concat!("self.owning_", "project("))
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: 归属项目必须取自监控目录（owning_project），不能用 project_name"
                    )
                });
            assert!(
                owning_at < discover_at,
                "{name}: 必须先定归属项目再 discover_batch，否则批次带着错的项目入队"
            );
            assert!(
                !body[..discover_at].contains(concat!("db_option.project_", "name.clone()")),
                "{name}: 发现阶段不得用配置里的主项目名当文件归属"
            );
        }
    }

    /// 手动路径的候选目录必须与自动 watcher 的监控目录是同一份，否则手动执行能把
    /// 监听不到的库排进队列（B4：数据落了库、此后永不更新，看起来却很新鲜）。
    /// 监控目录按项目收集（每个项目取它的 `*000` 库目录），按前缀过滤就还原出本项目那几个。
    #[test]
    fn ingestible_dirs_are_the_watch_dirs_under_this_project() {
        let watch = vec![
            PathBuf::from("/proj/AvevaMarineSample/dabacon000"),
            PathBuf::from("/proj/Another/dabacon000"),
        ];
        assert_eq!(
            dirs_under(&watch, Path::new("/proj/AvevaMarineSample")),
            vec![PathBuf::from("/proj/AvevaMarineSample/dabacon000")]
        );
        // 前缀比的是路径分量不是字符串，`Aveva` 匹配不上 `AvevaMarineSample`。
        assert!(dirs_under(&watch, Path::new("/proj/Aveva")).is_empty());
        // 一个都没有时调用方要说话（手动侧回执里那句告警），不能静悄悄地报「没有候选」。
        assert!(dirs_under(&[], Path::new("/proj/AvevaMarineSample")).is_empty());
    }

    #[test]
    fn unreadable_files_are_not_treated_as_e3d_databases() {
        assert!(try_parse_db_basic_info(Path::new("missing-e3d-db")).is_none());
    }

    /// 范围只由 MDB 定：`manual_db_nums` 这类手写名单再也不参与增量判定。
    ///
    /// 它们在这道门上待了太久，代价是 issue #10——7999 被 `manual_db_nums` 挡在外面，
    /// watcher 每 30 秒发现一次增量、每次跳过，日志上却与「MDB 里没这个库」一模一样。
    /// 现在配置里怎么写都不影响：MDB 说了算。
    #[test]
    fn handwritten_dbnum_lists_no_longer_narrow_the_increment_scope() {
        let mut option = DbOption::default();
        option.project_name = "Main".to_string();
        option.manual_db_nums = Some(vec![1001]);
        option.exclude_db_nums = Some(vec![7997]);
        option.only_sync_sys = true;
        let scope = UpdateScope::for_tests("/ALL", &[1001, 7997]);

        for dbnum in [1001, 7997] {
            assert!(
                in_scope_with(&option, &scope, "Main", "DESI", dbnum),
                "MDB 声明了 {dbnum}，配置里的手写名单不该有否决权"
            );
        }
        assert!(
            !in_scope_with(&option, &scope, "Main", "DESI", 8000),
            "MDB 没声明 8000，它就不进范围"
        );
        assert!(
            in_scope_with(&option, &scope, "Main", "SYST", 8191),
            "SYS meta 始终解析：MDB 的成员名单本身就存在它里面"
        );
    }

    /// 归属不符的观察**一个字都不许写**。
    ///
    /// 阻断路径本来就只写观察值（file_size / file_latest_sesno / scanned_at），
    /// 但那恰恰是被写脏的那几个：`dbnum_watermark:8191` 挂着 amssys 的身份，
    /// `file_latest_sesno` 却是 zdjsys 的 52 —— 面板上看到的是一个不存在的文件状态。
    /// 这两步嵌在依赖实库的函数里，只能钉源码。
    #[test]
    fn a_foreign_project_observation_is_not_persisted_at_all() {
        let src = include_str!("dbnum_state.rs");
        let body = src
            .split_once(concat!("pub async fn ", "record_observation("))
            .expect("record_observation 未找到")
            .1;
        let guard_at = body
            .find(concat!("is_foreign_", "project"))
            .expect("record_observation 必须先挡掉归属不符的观察");
        let blocked_at = body
            .find(concat!("record_blocked_", "observation("))
            .expect("record_observation 应当有阻断落库分支");
        assert!(
            guard_at < blocked_at,
            "归属校验必须先于阻断落库：阻断分支写的正是被跨项目写脏的那几个观察字段"
        );

        let classify = src
            .split_once(concat!("pub async fn ", "classify_scan("))
            .expect("classify_scan 未找到")
            .1;
        let owner_at = classify
            .find("owner_project")
            .expect("classify_scan 必须先比对登记行的归属项目");
        let check_at = classify
            .find(concat!("check_file_against_", "state("))
            .expect("classify_scan 应当调用 check_file_against_state");
        assert!(
            owner_at < check_at,
            "归属校验要先于回退/类型判据：两个项目的文件放一起比，判据本身就没有意义"
        );
    }

    /// 一个 Surreal 库只服务一个主项目：别的项目的运行态系统库（SYST/GLB/GLOB）
    /// 压根不该进摄入范围。
    ///
    /// dbnum 在 AVEVA 里只在**项目内**唯一 —— 三个项目的 sys 库都是 8191，而本库的
    /// 状态层（`dbnum_watermark` 的记录 id、`dbnum_info_table`、`pe.dbnum` 聚合）
    /// 全部按裸 dbnum 做键。放进来的实测后果：`dbnum_watermark:8191` 记着 amssys
    /// 的身份，`file_latest_sesno` 却被 zdjsys 的值写脏。
    ///
    /// DICT 目录库不在此列：它是被主项目依赖的数据，dbnum 也不冲突，照旧摄入。
    #[test]
    fn foreign_project_runtime_sys_databases_are_out_of_scope() {
        let mut option = DbOption::default();
        option.project_name = "AvevaMarineSample".to_string();
        let scope = UpdateScope::for_tests("/ALL", &[8000]);

        for db_type in PROJECT_RUNTIME_SYS_TYPES {
            assert!(
                in_scope_with(&option, &scope, "AvevaMarineSample", db_type, 8191),
                "主项目自己的 {db_type} 必须摄入"
            );
            assert!(
                !in_scope_with(&option, &scope, "AvevaCatalogue", db_type, 8191),
                "别的项目的 {db_type} 不该进范围"
            );
        }

        assert!(
            in_scope_with(&option, &scope, "AvevaCatalogue", "DICT", 7006),
            "目录库是主项目依赖的数据，跨项目照旧摄入"
        );
    }

    /// 库文件白名单。正反例全部取自 `D:/AVEVA/Projects/E3D3.1` 下真实躺着的文件
    /// ——副本的头部与正本一字不差，只能靠名字分辨；认错的代价是 dbnum 1112 拿到
    /// 五个候选、整个库被判「同号重复」而阻断。
    #[test]
    fn only_aveva_named_files_count_as_databases() {
        for name in [
            "ams1112_0001",
            "acp250705_0001", // 六位库号
            "TES1000_0001",   // TEST 项目整套是大写
            "ams3001",        // 无序号的老形态，登记表里是个正经 DESI
            "zdj7209",
            "TES001",
            "amssys", // SYST 库 8191：MDB / CURD 就存在它里面
            "amscom",
            "amsmis",
            "TESsys",
        ] {
            assert!(is_pdms_db_file_name(name), "{name} 应当算库文件");
        }

        for name in [
            "ams1112_0001 copy",                             // 人手复制，带空格
            "ams1112_0001 copy 3",                           //
            "ams1112_0001_old",                              // 后缀不是四位数字
            "ams1112_0001-new",                              // 旧规则唯一挡得住的那种
            "ams1112_0001.zip",                              // 带扩展名
            "ams7997_0001.codex-before-d03-delete-20260727", // 日期后缀备份
            "amscom.codex-before-d03-relaunch-20260727",
            "TES1001_0001 - 副本",
            "ams000.7z",
            "DBOutput.txt",
            "_0001",       // 没有前缀
            "ab12_0001",   // 前缀不足三位字母
            "ams1112_001", // 序号不足四位
            "ams",         // 只有前缀
            "amssys2",     // sys/com/mis 后面不许再挂东西
        ] {
            assert!(!is_pdms_db_file_name(name), "{name} 不该算库文件");
        }
    }

    #[tokio::test]
    #[ignore = "manual live: copies one configured E3D header into a throwaway duplicate directory"]
    async fn live_watch_directory_blocks_duplicate_dbnum_files() {
        let mut manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let source = manager
            .watcher
            .watch_dirs
            .iter()
            .flat_map(|dir| {
                WalkDir::new(dir)
                    .max_depth(1)
                    .into_iter()
                    .filter_map(Result::ok)
            })
            .find_map(|entry| {
                // 判重不过范围门，所以随便一个能读出头的候选文件都能当夹具源。
                let path = entry.file_type().is_file().then(|| entry.into_path())?;
                is_candidate_db_file(&path).then_some(())?;
                Some((path.clone(), try_parse_db_basic_info(&path)?))
            })
            .expect("configured watch dirs contain an E3D database");
        let mut source_file = fs::File::open(&source.0).expect("open source E3D header");
        let mut header = [0u8; 60];
        source_file
            .read_exact(&mut header)
            .expect("read source E3D header");
        let fixture =
            std::env::temp_dir().join(format!("aios-duplicate-dbnum-{}", std::process::id()));
        fs::create_dir_all(&fixture).expect("create duplicate directory");
        // 副本必须取合 AVEVA 形态的名字：候选门先过 `is_pdms_db_file_name` 白名单，
        // 叫 first/second 根本进不了判重（本用例曾因此腐化——写于白名单落地之前，
        // 判重集合恒为空。dbnum 来自文件头，两个不同序号映射到同一个库号）。
        fs::write(fixture.join("ams9990_0001"), header).expect("write first header");
        fs::write(fixture.join("ams9990_0002"), header).expect("write second header");
        manager.watcher = Arc::new(PdmsWatcher::new(vec![fixture.clone()]));

        // 判重键是 (归属项目, dbnum)；夹具目录不在任何监控目录的归属登记里，
        // `owning_project` 按约定退回配置里的主项目名。
        assert_eq!(
            manager.duplicate_dbnums_across_watch_dirs(),
            HashSet::from([(manager.db_option.project_name.clone(), source.1.db_no)])
        );

        fs::remove_dir_all(&fixture).expect("remove duplicate directory");
    }

    /// 黑名单那一道门。直接调被生产路径用的那个函数——这里过去是一份手抄的副本，
    /// 抄件永远绿着，改坏真函数也发现不了。
    #[test]
    fn test_should_exclude_file() {
        // 测试应该被排除的文件扩展名
        assert!(should_exclude_file(Path::new("test.com")));
        assert!(should_exclude_file(Path::new("program.exe")));
        assert!(should_exclude_file(Path::new("library.dll")));
        assert!(should_exclude_file(Path::new("system.sys")));
        assert!(should_exclude_file(Path::new("temp.tmp")));
        assert!(should_exclude_file(Path::new("backup.bak")));
        assert!(should_exclude_file(Path::new("debug.log")));
        assert!(should_exclude_file(Path::new("cache.cache")));

        // 测试应该被排除的系统文件
        assert!(should_exclude_file(Path::new("thumbs.db")));
        assert!(should_exclude_file(Path::new("desktop.ini")));
        assert!(should_exclude_file(Path::new(".ds_store")));
        assert!(should_exclude_file(Path::new("~$document.docx")));

        // 测试隐藏文件
        assert!(should_exclude_file(Path::new(".hidden")));
        assert!(should_exclude_file(Path::new(".gitignore")));

        // 测试不应该被排除的文件
        assert!(!should_exclude_file(Path::new("data.db")));
        assert!(!should_exclude_file(Path::new("config.txt")));
        assert!(!should_exclude_file(Path::new("ams7334_0001")));
        assert!(!should_exclude_file(Path::new("zdj7015_0001")));
        assert!(!should_exclude_file(Path::new("document.pdf")));

        // 测试大小写不敏感
        assert!(should_exclude_file(Path::new("TEST.COM")));
        assert!(should_exclude_file(Path::new("Program.EXE")));
        assert!(should_exclude_file(Path::new("THUMBS.DB")));

        // 黑名单**挡不住**人手副本与带日期后缀的备份——它们没有扩展名、或者
        // 扩展名压根不在清单里。这就是白名单必须存在、且三条自动路径都得过它的
        // 全部理由（见 `is_candidate_db_file`）。
        assert!(!should_exclude_file(Path::new("ams1112_0001 copy")));
        assert!(!should_exclude_file(Path::new(
            "ams7997_0001.codex-before-d03-delete-20260727"
        )));
    }

    /// 候选库文件 = 黑名单 ∩ 白名单。这是三条自动路径与手动扫描共用的那道门，
    /// 也是「一个人手副本冻住整个库」这条事故链的唯一出口。
    ///
    /// 反例里那几个副本形态全部来自 `D:/AVEVA/Projects/E3D3.1` 下真实躺着的文件：
    /// 它们没有扩展名、头部与正本一字不差，黑名单一个都挡不住，所以这条测试
    /// 必须由白名单来兜。
    #[test]
    fn only_real_database_files_are_candidates() {
        for name in [
            "ams1112_0001",
            "acp250705_0001",
            "TES1000_0001",
            "ams3001",
            "zdj7209",
            // sys / com / mis 三个项目库。注意 `amscom` 与黑名单里的 `com`
            // 扩展名同名却毫不相干——它没有扩展名，误伤它等于丢掉 SYST 8191
            // 之外的整套项目库。
            "amssys",
            "amscom",
            "amsmis",
            "TESsys",
        ] {
            assert!(
                is_candidate_db_file(Path::new(name)),
                "{name} 应当算候选库文件"
            );
        }

        for name in [
            // 人手复制的副本——黑名单挡不住，只有白名单能拦。
            "ams1112_0001 copy",
            "ams1112_0001 copy 3",
            "ams1112_0001_old",
            "TES1001_0001 - 副本",
            "ams7997_0001.codex-before-d03-delete-20260727",
            "amscom.codex-before-d03-relaunch-20260727",
            // 黑名单本来就该挡住的。
            "debug.log",
            "thumbs.db",
            ".gitignore",
            "ams000.7z",
            // 压根不是库文件。
            "DBOutput.txt",
            "_0001",
            "ab12_0001",
        ] {
            assert!(
                !is_candidate_db_file(Path::new(name)),
                "{name} 不该算候选库文件"
            );
        }
    }

    /// 三条自动路径必须都过同一道候选门，且都在读文件头之前过。
    ///
    /// 漏掉任何一处的代价不是「多解析几个杂项文件」：`duplicate_dbnums_across_watch_dirs`
    /// 漏掉，一个 `ams1112_0001 copy` 就让 dbnum 1112 拿到两个候选、被判同号重复而
    /// **整库停更**；而手动预览（它过了白名单）看到的是唯一候选、报告一切正常。
    ///
    /// 这三道门嵌在依赖实库的大函数里，没法用纯函数钉住，所以直接钉源码。
    /// marker 用 `concat!` 拼接，避免本测试自己的字符串字面量先于真函数被命中。
    #[test]
    fn every_auto_path_gates_on_the_shared_candidate_predicate() {
        let src = include_str!("increment_manager.rs");
        for (name, marker) in [
            (
                "sweep_watch_dirs",
                concat!("async fn ", "sweep_watch_dirs("),
            ),
            ("async_watch", concat!("pub async fn ", "async_watch(")),
            (
                "duplicate_dbnums_across_watch_dirs",
                concat!("fn ", "duplicate_dbnums_across_watch_dirs("),
            ),
        ] {
            let body = src
                .split_once(marker)
                .unwrap_or_else(|| panic!("{name} 未找到"))
                .1;
            let gate_at = body
                .find("is_candidate_db_file(")
                .unwrap_or_else(|| panic!("{name}: 缺少候选库文件门控"));
            let header_at = body
                .find("try_parse_db_basic_info(")
                .unwrap_or_else(|| panic!("{name}: 缺少 try_parse_db_basic_info 调用"));
            assert!(
                gate_at < header_at,
                "{name}: 候选门控必须先于读文件头，否则人手复制的副本会被当成同一个 \
                 dbnum 的第二个候选，把整个库判成「同号重复」而阻断"
            );
        }
    }

    /// 阻断裁决只能有一个权威，自动路径不许自己再列一份异常清单。
    ///
    /// 这里过去是 `match` 只列 `Rollback` / `PathMigrated`，其余走 `_ => true` 放行，
    /// 于是 `TypeChanged` 在自动路径上被静默放过，而手动预览把它标成阻断。
    #[test]
    fn the_auto_path_blocks_by_the_shared_anomaly_verdict() {
        let src = include_str!("increment_manager.rs");
        let body = src
            .split_once(concat!("pub(crate) async fn ", "scan_and_check_file("))
            .expect("scan_and_check_file 未找到")
            .1;
        // 用「下一个函数定义」而不是缩进花括号来收边：这个文件是 CRLF 的，
        // 按 "\n    }\n" 找永远找不到。
        let body = body
            .split_once(concat!("pub async fn ", "init_watcher("))
            .expect("scan_and_check_file 之后应当是 init_watcher")
            .0;

        assert!(
            body.contains("classify_scan(") && body.contains("record_observation("),
            "分类与落库都必须走共用裁决，自动路径不得自己拼 check_file_against_state"
        );
        assert!(
            !body.contains("_ => true"),
            "不许有放行式兜底：新增一种异常时必须显式决定它阻不阻断"
        );

        let classify_at = body.find("classify_scan(").expect("已在上面断言过存在");
        let record_at = body
            .find("record_observation(")
            .expect("已在上面断言过存在");
        assert!(
            classify_at < record_at,
            "必须先裁决再落库：落库会按 dbnum 覆盖 db_type/file_path，\
             而它们正是 check_file_against_state 的判据"
        );
    }

    /// 读不出最新会话号的文件必须跳过本轮，不得吞成 sesno=0（2026-08-13 审计 P1）。
    ///
    /// 0 会对 applied > 0 的库伪造「文件回退」：把假观察值（file_latest_sesno=0）
    /// 写进登记行，控制台还播报一次实际不会发生的整库重建（reinit 形状 1..=0
    /// 过不了入队的 covers 守卫）。三条扫描路径必须同口径：手动 warn+跳过、
    /// watch 头部扫描失败跳过、sweep 曾是唯一吞错的那条。嵌在依赖实库的大函数里，
    /// 钉源码（marker 用 `concat!` 拼接，避免本测试自己的字面量先被命中）。
    #[test]
    fn a_failed_sesno_read_is_skipped_not_zeroed_on_the_sweep_path() {
        let src = include_str!("increment_manager.rs");
        let body = src
            .split_once(concat!("async fn ", "sweep_dirs("))
            .expect("sweep_dirs 未找到")
            .1
            .split_once(concat!("fn ", "reinit_batch("))
            .expect("sweep_dirs 之后是 reinit_batch")
            .0;
        let read_at = body
            .find(".get_latest_sesno()")
            .expect("sweep_dirs 必须读文件最新会话号");
        let rest = &body[read_at..];
        // 600 字节窗口，向后走到字符边界（周边是中文注释，硬切会劈开多字节字符）。
        let mut end = rest.len().min(600);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let window = &rest[..end];
        assert!(
            !window.contains("unwrap_or_default"),
            "读失败不得吞成 0（伪造回退 + 假观察值）: {window}"
        );
        assert!(
            window.contains("continue"),
            "读失败必须跳过本轮该文件（与手动路径同口径）: {window}"
        );
    }

    /// 回退播报不许声称一次可能不会发生的入队（2026-08-13 审计 P1 附带项）。
    ///
    /// reinit 形状（1..=file_latest）仍要过 `batch_queue::enqueue` 的 covers 守卫
    /// 与合并判定，实际落点由入队日志（`enqueue_discovered` 的 outcome 行）报告。
    /// 这句话曾写死「已按整库重建入队」，在空文件（file_latest=0）等边界下与
    /// 事实不符——日志说了一件没有发生的事。
    #[test]
    fn the_rollback_line_reports_disposition_not_a_presumed_enqueue() {
        let src = include_str!("increment_manager.rs");
        let body = src
            .split_once(concat!("pub(crate) async fn ", "scan_and_check_file("))
            .expect("scan_and_check_file 未找到")
            .1
            .split_once(concat!("pub async fn ", "init_watcher("))
            .expect("scan_and_check_file 之后应当是 init_watcher")
            .0;
        assert!(
            !body.contains(concat!("已按整库重建", "入队")),
            "播报不得预设入队结果，实际落点归入队日志"
        );
        assert!(
            body.contains("转整库重建"),
            "回退处置（转整库重建）必须仍然喊出来"
        );
    }

    /// MySQL 镜像（feature=sql）：NAME 必须参数绑定、DBNO 缺失必须出声
    /// （2026-08-13 审计 P2）。
    ///
    /// 元素名可含引号/反斜杠，拼进单引号字面量会让该条 UPDATE 失败且只留
    /// warning；DBNO 缺失静默取 0 则在镜像表里留下一个看着像真的库号。
    /// 函数在 `#[cfg(feature = "sql")]` 门后，纯函数测不到，钉源码
    /// （include_str 不受 feature 影响）。
    #[test]
    fn the_mysql_mirror_binds_name_and_reports_missing_dbno() {
        let src = include_str!("increment_manager.rs");
        let update = src
            .split_once(concat!("async fn ", "process_mysql_update_elements("))
            .expect("process_mysql_update_elements 未找到")
            .1
            .split_once(concat!("async fn ", "process_mysql_delete_elements("))
            .expect("其后应当是 process_mysql_delete_elements")
            .0;
        assert!(
            update.contains(".bind("),
            "OWNER/NAME/ID 必须走 sqlx 参数绑定: {update}"
        );
        assert!(
            !update.contains(concat!("NAME='", "{}'")),
            "不得把元素名拼进单引号字面量"
        );
        let insert = src
            .split_once(concat!("async fn ", "process_mysql_insert_elements("))
            .expect("process_mysql_insert_elements 未找到")
            .1
            .split_once(concat!("async fn ", "process_mysql_update_elements("))
            .expect("其后应当是 process_mysql_update_elements")
            .0;
        assert!(
            insert.contains("缺 DBNO"),
            "DBNO 缺失必须告警而不是静默取 0: {insert}"
        );
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

/// 这个文件名是不是一个 AVEVA 库文件。三位项目前缀打头，后面只认两种形态：
///
/// - `<前缀><库号>` 带或不带 `_<四位序号>`——`ams1112_0001`、`acp250705_0001`、
///   `TES1000_0001`，以及没有序号的老形态 `ams3001`、`zdj7209`、`TES001`；
/// - `<前缀>sys` / `com` / `mis`——项目的系统、通信与杂项库，每个项目各一个。
///   **`amssys` 就是 SYST 库 8191**，MDB 与 CURD 都存在它里面，认不出它等于
///   让本期执行范围再也刷新不了。
///
/// 大小写不敏感：TEST 项目整套用的是大写 `TES…`，按小写认会把它 85 个库全吞掉。
///
/// 这是**白名单**，与 [`should_exclude_file`] 那份扩展名黑名单
/// 相反。黑名单只挡得住它列举过的东西，挡不住 `ams1112_0001 copy`、
/// `ams1112_0001 copy 3`、`ams1112_0001_old` 这类人手复制的副本——它们没有扩展名、
/// 头部与正本一字不差，于是 dbnum 1112 一口气拿到五个候选文件，整个库被判成
/// 「同号重复」而阻断。原先那条「名字含 `-` 就跳过」接不住带空格的副本，也接不住
/// `ams7997_0001.codex-before-d03-delete-20260727` 这种带日期后缀的备份。
///
/// 规则拿真实项目校过：`D:/AVEVA/Projects/E3D3.1` 下 8 个项目的库目录，999 个
/// 编号库 + 每项目 3 个 sys/com/mis + 一批无序号老库全部命中，杂项一个不漏地弃掉。
pub fn is_pdms_db_file_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() <= 3 || !bytes[..3].iter().all(u8::is_ascii_alphabetic) {
        return false;
    }
    let rest = &name[3..];
    if matches!(rest.to_ascii_lowercase().as_str(), "sys" | "com" | "mis") {
        return true;
    }
    // `<库号>` 或 `<库号>_<四位序号>`，别的一律不是。
    let (dbnum, seq) = match rest.split_once('_') {
        Some((dbnum, seq)) => (dbnum, Some(seq)),
        None => (rest, None),
    };
    if dbnum.is_empty() || !dbnum.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    seq.is_none_or(|seq| seq.len() == 4 && seq.bytes().all(|b| b.is_ascii_digit()))
}

/// 这个文件该不该被排除在监控之外（扩展名 / 系统文件 / 隐藏文件黑名单）。
///
/// 只挡它列举过的东西——挡不住 `ams1112_0001 copy` 这类没有扩展名的人手副本，
/// 那是 [`is_pdms_db_file_name`] 白名单的职责。两道门合起来才是
/// [`is_candidate_db_file`]，调用方一律用后者，不要单独用这一道。
fn should_exclude_file(file_path: &std::path::Path) -> bool {
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
                log::debug!("排除文件（扩展名）: {:?}", file_path);
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
                    log::debug!("排除文件（临时文件）: {:?}", file_path);
                    return true;
                } else if name_lower == *pattern {
                    log::debug!("排除文件（系统文件）: {:?}", file_path);
                    return true;
                }
            }

            // 排除以点开头的隐藏文件（Unix风格）
            if name_str.starts_with('.') && name_str.len() > 1 {
                log::debug!("排除文件（隐藏文件）: {:?}", file_path);
                return true;
            }
        }
    }

    false
}

/// 这个路径是不是一个**候选库文件**：两道门都过才算。
///
/// 1. [`should_exclude_file`]——扩展名 / 系统文件黑名单；
/// 2. [`is_pdms_db_file_name`]——AVEVA 库命名白名单。
///
/// 三条自动路径（启动重扫 `sweep_watch_dirs`、文件事件 `async_watch`、重复 dbnum
/// 复查 `duplicate_dbnums_across_watch_dirs`）与手动候选扫描
/// (`manual_update::scan_project_candidates`) 必须共用这一个谓词。
///
/// 自动侧曾经只过黑名单。少的那道白名单不是「多解析几个杂项文件」的效率问题，
/// 而是**整个库停更**：`ams1112_0001 copy` 这类人手复制的副本没有扩展名、
/// 头部与正本一字不差，黑名单挡不住，于是 dbnum 1112 拿到两个候选、被 F6 判成
/// 「同号重复」而阻断，此后一条批次都不入队。现场只有一行 println，而手动预览
/// （它过了白名单）看到的是唯一候选、报告「无异常」——面板上永远查不出
/// 水位为什么不动。
///
/// 不在这里做 `is_file()`：那要多一次 stat，而三个调用点各自都已排除目录。
pub fn is_candidate_db_file(path: &std::path::Path) -> bool {
    if should_exclude_file(path) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        log::debug!("跳过文件名无法解析为 UTF-8 的条目: {}", path.display());
        return false;
    };
    if !is_pdms_db_file_name(name) {
        log::debug!("跳过不合 AVEVA 库命名的文件（副本 / 备份 / 杂项）: {name}");
        return false;
    }
    true
}

/// 「项目运行态」系统库：MDB / DB / CURD 这些**项目自身**的结构数据。
///
/// 与 DICT 目录库的区别是数据域，不是类型高低：DICT 是被主项目依赖的数据，
/// 跨项目引用是正常业务；SYST/GLB/GLOB 描述的是「那个项目自己怎么组织」，
/// 对本库毫无意义。而且 dbnum 在 AVEVA 里只在**项目内**唯一 —— 三个项目的
/// sys 库都是 8191，本库的状态层（`dbnum_watermark` 记录 id、`dbnum_info_table`、
/// `pe.dbnum` 聚合）却全部按裸 dbnum 做键，放进来就是三份数据抢同一行。
pub const PROJECT_RUNTIME_SYS_TYPES: [&str; 3] = ["SYST", "GLB", "GLOB"];

/// 这个库是不是「别的项目的运行态系统库」。
pub(crate) fn is_foreign_runtime_sys(db_option: &DbOption, project: &str, db_type: &str) -> bool {
    PROJECT_RUNTIME_SYS_TYPES.contains(&db_type)
        && !project
            .trim()
            .eq_ignore_ascii_case(db_option.project_name.trim())
}

/// 增量摄入的唯一判定：**本期 MDB 声明的 DESI**。
///
/// 只剩两条判据：
///
/// 1. 别的项目的运行态系统库（SYST/GLB/GLOB）永远不进——这不是可配的口味，
///    dbnum 在 AVEVA 里只在项目内唯一，三个项目的 sys 库都是 8191，而本库的状态层
///    全部按裸 dbnum 做键，放进来就是三份数据抢同一行；
/// 2. [`UpdateScope::admits`]——SYS meta（SYST/DICT/GLB/GLOB）放行，因为 MDB 的成员
///    名单本身就存在这些库里；其余只认本 MDB 声明过的 DESI。
///
/// 从前这里还串着一道 `should_process_database`：类型白名单 + `only_sync_sys` +
/// `exclude_db_nums` + `manual_db_nums`。它们制造的是同一句「不在本期执行范围」下的
/// 两种成因——「MDB 里没有这个库」与「有人在配置里把它划掉了」在现场长得一模一样。
/// issue #10 卡的正是这个：7999 被 `manual_db_nums` 挡着，watcher 每 30 秒发现一次
/// 增量、每次都跳过，而模型树看起来只是「不更新」。范围现在只由 MDB 定，手写名单
/// 不再参与增量判定（`manual_db_nums` / `exclude_db_nums` 仍供全量模型生成与按需
/// 基线解析使用，见 `fast_model::gen_model` 与 `manual_update::baseline_sync_options`）。
pub(crate) fn in_scope_with(
    db_option: &DbOption,
    scope: &UpdateScope,
    project: &str,
    db_type: &str,
    db_num: u32,
) -> bool {
    // 一个 Surreal 库只服务一个主项目。别的项目的运行态系统库不属于本库的数据域
    // ——这不是「异常阻断」，是压根不在摄入范围内，两者不能混为一谈。
    if is_foreign_runtime_sys(db_option, project, db_type) {
        log::debug!(
            "忽略非主项目的运行态系统库: project={project} db_type={db_type} dbnum={db_num}"
        );
        return false;
    }
    scope.admits(db_type, db_num)
}

/// 「这个库为什么被跳过」——说给盯着控制台的人听。
///
/// 光说「不在本期执行范围」是句同义反复：人接着要问的一定是「哪个 MDB、它到底
/// 声明了什么」。范围只由 MDB 定之后这个答案是确定的，那就把它直接写进日志。
pub(crate) fn out_of_scope_reason(scope: &UpdateScope, db_type: &str, db_num: u32) -> String {
    let declared = scope.declared_desi().count();
    if db_type != "DESI" {
        return format!(
            "不在本期执行范围，跳过数据库: 类型={db_type}, 编号={db_num}\
             （只有 DESI 与 SYS meta 参与增量，{db_type} 不参与）"
        );
    }
    format!(
        "不在本期执行范围，跳过数据库: 类型={db_type}, 编号={db_num}\
         （MDB {} 的 CURD 里没有它；本期声明了 {declared} 个 DESI）",
        scope.mdb()
    )
}

/// 同一句范围告警只说一次。
///
/// 文件事件是 30 秒一轮的轮询，范围没解出来的话每一轮都会重算出同一句话；
/// 每轮都打等于把它埋进自己的噪声里。换了一句（比如 MDB 从「一条都没有」变成
/// 「有但 CURD 是空的」）就该重新说。
fn warn_scope_once(warning: &str) {
    use std::sync::Mutex;
    static LAST: Mutex<String> = Mutex::new(String::new());

    let mut last = match LAST.lock() {
        Ok(last) => last,
        Err(poisoned) => poisoned.into_inner(),
    };
    if *last == warning {
        return;
    }
    last.clear();
    last.push_str(warning);
    println!("{warning}");
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

/// MySQL批量处理的大小常量
const BATCH_SIZE: usize = 100;

/// 更新world transform的批量大小（较小，因为涉及复杂计算）
const TRANSFORM_BATCH_SIZE: usize = 50;

/// 查询inst_relate数据的批量大小（最小，避免查询超时）
const QUERY_BATCH_SIZE: usize = 20;

/// 只认监控目录的**直属**文件。
///
/// 「能被摄入的文件集」必须等于「能被监听到的文件集」：`async_watch` 注册的是
/// `RecursiveMode::NonRecursive`，子目录里的库文件收不到任何变更事件。摄入它们
/// 只会在库里留下一份此后永不更新、看起来却很新鲜的数据（B4）。需要覆盖某个
/// 子目录时，把它配成监控目录，而不是把遍历放深。
pub(crate) const INGEST_MAX_DEPTH: usize = 1;

/// 同一个**项目**里出现两次的 dbnum。
///
/// 键必须带归属项目：不同项目各自的 sys 库（amssys / acpsys / zdjsys）天然共用
/// dbnum=8191，只按 dbnum 判重会把三个正常的库一起阻断。防人手副本
/// （`ams1112_0001 copy`）靠的是「同项目内同号」，不受影响。
fn duplicate_dbnums(
    entries: impl IntoIterator<Item = (String, u32, PathBuf)>,
) -> HashSet<(String, u32)> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter_map(|(project, dbnum, _)| {
            let key = (project, dbnum);
            (!seen.insert(key.clone())).then_some(key)
        })
        .collect()
}

/// 监控目录里落在 `project_dir` 下的那些。
///
/// 比较走 [`path_starts_with`] 而不是 `Path::starts_with`：后者逐段区分大小写，
/// 而 Windows 上 `D:/AVEVA/...ZDJ` 与 `d:\aveva\...zdj` 是同一个目录。判错的后果
/// 是手动侧「一个候选都没有」，自动侧却在监听同一个目录——两条路径就此分家。
fn dirs_under(watch_dirs: &[PathBuf], project_dir: &std::path::Path) -> Vec<PathBuf> {
    watch_dirs
        .iter()
        .filter(|dir| path_starts_with(dir, project_dir))
        .cloned()
        .collect()
}

fn try_parse_db_basic_info(path: &std::path::Path) -> Option<DbBasicInfo> {
    let mut file = fs::File::open(path).ok()?;
    let mut header = [0u8; 60];
    file.read_exact(&mut header).ok()?;
    Some(parse_file_basic_info(&header))
}

/// 一次 F6 扫描裁决在自动路径上的处置（ADR-021）。
///
/// 过去 `scan_and_check_file` 返回 `bool`（放行 / 阻断），回退默认整库重建后
/// 处置变成三种：回退既不放行（增量窗口不许接手）也不阻断（不等人），而是
/// 由调用方按首次导入形状入队一条重建批次，清库归 worker 执行体的冻结点复核。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanGate {
    /// 无异常（或良性路径迁移）：照常走水位比对与增量发现。
    Proceed,
    /// 阻断类异常（类型变更 / 同号多文件 / 缺失 / 归属不符）或读状态失败：
    /// 本轮跳过，水位不动，等人处理。
    Blocked,
    /// 回退：按整库重建入队（applied=0 形状，窗口 1..file_latest），
    /// 扫描路径不删任何数据。
    Reinit,
}

/// 裁决 → 处置的唯一映射（纯函数，好测）。逐类点名、不留 `_ =>` 兜底：新增
/// 一种异常时这里编译不过，作者必须显式决定它放行、阻断还是重建。
fn scan_gate_for(verdict: &crate::data_interface::dbnum_state::ScanVerdict) -> ScanGate {
    use crate::data_interface::dbnum_state::FileAnomaly;

    match &verdict.anomaly {
        None | Some(FileAnomaly::PathMigrated { .. }) => ScanGate::Proceed,
        Some(FileAnomaly::Rollback { .. }) => ScanGate::Reinit,
        Some(
            FileAnomaly::TypeChanged { .. }
            | FileAnomaly::Duplicate { .. }
            | FileAnomaly::Missing { .. }
            | FileAnomaly::ForeignProject { .. },
        ) => ScanGate::Blocked,
    }
}

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
    /// 这个项目里「能被摄入」的库目录，即监控目录中落在 `project_dir` 下的那些。
    ///
    /// 手动与自动两条触发路径喂的是同一个队列，因此必须共用同一份目录集合与同一个
    /// [`INGEST_MAX_DEPTH`]。手动侧过去递归整个项目目录，于是它能把监听不到的子目录
    /// 里的库排进队列——正好制造出自动侧专门在防的 B4。
    ///
    /// 监控目录是按项目收集的（每个项目取其 `*000` 库目录），所以按前缀过滤就能还原
    /// 「本项目的那几个」。
    pub(crate) fn ingestible_dirs(&self, project_dir: &std::path::Path) -> Vec<PathBuf> {
        dirs_under(&self.watch_dirs(), project_dir)
    }

    /// 当前真正生效的监控目录：启动时解析出的那批，并上共享盘恢复后才补挂进来的那批。
    ///
    /// 摄入侧必须读这一份而不是 `self.watcher.watch_dirs`——后者是启动快照，
    /// 补挂进来的目录只会被轮询、不会被摄入，正好制造出「监听得到但永不入队」。
    pub(crate) fn watch_dirs(&self) -> Vec<PathBuf> {
        use crate::data_interface::project_paths::{discovered_watch_dirs, path_identity};

        let mut dirs = self.watcher.watch_dirs.clone();
        let mut seen: HashSet<String> = dirs.iter().map(|dir| path_identity(dir)).collect();
        for dir in discovered_watch_dirs() {
            if seen.insert(path_identity(&dir)) {
                dirs.push(dir);
            }
        }
        dirs
    }

    /// 文件的归属项目：看它落在哪个监控目录下。
    ///
    /// 解析不出来时退回配置里的主项目名并告警——那是「监控目录归属没登记」的信号，
    /// 而不是一个可以静默接受的默认值：归属记错会让执行侧去错误的项目目录里找文件，
    /// F6 判重键也随之退化。告警走 `warn_unattributed_once`（stderr + 按目录去重），
    /// 只有 log::warn 的话现场根本看不见退化已经发生。
    pub(crate) fn owning_project(&self, path: &std::path::Path) -> String {
        crate::data_interface::project_paths::project_of_path(path).unwrap_or_else(|| {
            let fallback = self.db_option.project_name.clone();
            crate::data_interface::project_paths::warn_unattributed_once(path, &fallback);
            fallback
        })
    }

    /// F6：同一 dbnum 在监控目录里出现了多个文件。
    ///
    /// **不过范围门**：判重看的是磁盘上有几个候选文件，与这个库这一期跑不跑无关。
    /// 范围收窄时若连判重也跟着收窄，一个躺在目录里的 `ams1112_0001 copy` 会在范围
    /// 放开的那一天才第一次被发现，而那时它已经污染过一轮文件身份了。
    fn duplicate_dbnums_across_watch_dirs(&self) -> HashSet<(String, u32)> {
        duplicate_dbnums(self.watch_dirs().into_iter().flat_map(|watch_dir| {
            let project = self.owning_project(&watch_dir);
            WalkDir::new(watch_dir)
                .max_depth(INGEST_MAX_DEPTH)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .filter(|entry| is_candidate_db_file(entry.path()))
                .filter_map(move |entry| {
                    let info = try_parse_db_basic_info(entry.path())?;
                    Some((project.clone(), info.db_no, entry.path().to_path_buf()))
                })
        }))
    }

    /// F6：自动 watcher 的「文件观察落库 + 异常检测」。
    ///
    /// 分类与落库都交给 [`DbnumState::classify_scan`] / [`DbnumState::record_observation`]
    /// ——手动预览、手动入队、worker 执行体走的是同两个函数，四条路径不可能再分叉。
    /// 这里只剩下自动路径独有的那部分：把裁决翻成日志，把处置翻成 [`ScanGate`]。
    ///
    /// 处置只由 [`scan_gate_for`] 说了算（它按 [`FileAnomaly`] 逐类点名）：回退
    /// 返回 [`ScanGate::Reinit`]，调用方按首次导入形状入队重建批次（ADR-021，
    /// 清库归 worker 执行体的冻结点复核，扫描路径不删任何数据）；其余阻断类
    /// 异常返回 [`ScanGate::Blocked`]，调用方跳过（水位不回退）。
    pub(crate) async fn scan_and_check_file(
        &self,
        project: &str,
        path: &std::path::Path,
        file_name: &str,
        db_type: &str,
        db_num: u32,
        file_latest_sesno: i32,
    ) -> ScanGate {
        use crate::data_interface::dbnum_state::{DbnumState, FileAnomaly, FileObservation};

        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let obs = FileObservation {
            dbnum: db_num,
            project: project.to_string(),
            db_type: db_type.to_string(),
            file_name: file_name.to_string(),
            file_path: path.display().to_string(),
            file_size,
            file_latest_sesno,
            file_modified_at: None,
        };
        // 读状态失败按阻断处理：水位读不出来就不知道有没有回退，宁可这一轮不入队，
        // 也不能拿一个默认的 0 当水位去跑（那会把老库判成首次导入）。
        let verdict = match DbnumState::classify_scan(&obs).await {
            Ok(verdict) => verdict,
            Err(e) => {
                println!("F6 读取 DBNUM 状态失败，本轮跳过 dbnum={db_num}: {e:#}");
                return ScanGate::Blocked;
            }
        };
        if let Err(e) = DbnumState::record_observation(&obs, &verdict).await {
            println!("F6 记录扫描观察失败 dbnum={db_num}: {e}");
        }

        // 逐个变体点名，不留 `_ =>` 兜底：将来新增一种异常时这里编译不过，
        // 作者必须显式决定它放行、阻断还是重建、怎么说。
        match &verdict.anomaly {
            None => {}
            Some(FileAnomaly::Rollback {
                file_latest_sesno: f,
                applied_sesno: a,
                ..
            }) => println!(
                "F6 文件回退/替换 dbnum={db_num}（file_latest={f} < applied={a}），\
                 转整库重建：按首次导入形状交由入队，实际落点见入队日志\
                 （worker 冻结点复核仍判回退才清空该库数据并重新解析，ADR-021）"
            ),
            Some(FileAnomaly::TypeChanged {
                stored_db_type,
                observed_db_type,
            }) => println!(
                "F6 库类型变更，阻断 dbnum={db_num}（登记 {stored_db_type} → 现场 {observed_db_type}）；\
                 登记身份保持不变，否则下一轮就检不出同一个异常"
            ),
            Some(FileAnomaly::Duplicate { paths }) => {
                println!("F6 同 dbnum 多文件，阻断 dbnum={db_num}: {paths:?}")
            }
            Some(FileAnomaly::Missing { path }) => {
                println!("F6 登记文件缺失，阻断 dbnum={db_num}: {path}")
            }
            Some(FileAnomaly::PathMigrated { old_path, new_path }) => println!(
                "F6 文件路径迁移 dbnum={db_num}: {old_path} -> {new_path}（已更新登记路径）"
            ),
            Some(FileAnomaly::ForeignProject {
                stored_project,
                observed_project,
            }) => println!(
                "F6 dbnum={db_num} 的登记行属于项目 {stored_project}，现场文件来自 \
                 {observed_project}，已阻断且不写任何观察值（dbnum 只在项目内唯一）"
            ),
        }

        scan_gate_for(&verdict)
    }

    // `execute_incr_update`（发现即执行的旧自动编排）随 ADR-011 合流退役：
    // 执行只发生在 `batch_worker` 的消费循环里（一条队列、一个消费者）。它独有的
    // 三步都有了新家——MySQL 可选同步进了 `execute_one_dbnum` 的成功分支，SYST
    // 派生由 worker 经 `SideEffectCompensator::enqueue_syst` 走持久补偿队列，
    // `notify_incr_applied` 摘要随之删除（plant-ui 只订 tasks 主题，它从未有过消费者）。

    /// 初始化文件监控器
    ///
    /// 在系统启动时扫描监控目录中的所有数据库文件，检查是否有待应用的会话；
    /// 有则**入队**（发现即入队，ADR-011 §2）。执行归数据批次 worker——重启后
    /// 的队列正是靠这次重扫从水位重建出来的（ADR-011 §4）。
    ///
    /// 只负责启动那一次性的目录准备，重扫本身见 [`Self::sweep_watch_dirs`]。
    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        // 创建存档与压缩临时目录（SyncPublisher 依赖 assets/temp）；
        // 还要 assets/meshes——worker 消费本次入队的批次时就可能开始写网格，
        // 而调用方 `lib.rs` 建这个目录的那行排在 `init_watcher()` 之后。
        fs::create_dir_all("assets/archives")?;
        fs::create_dir_all("assets/temp")?;
        fs::create_dir_all("assets/meshes")?;
        self.sweep_watch_dirs("init", true).await
    }

    /// SYS meta 落库之后再重扫一次监控目录。
    ///
    /// 本期执行范围由 MDB 定，而 MDB 与 CURD 就存在 SYS meta 库里。全新项目的第一轮
    /// 只解析得出 SYS meta（那时范围还是空的），有人往 MDB 里加一个库也是同样的形状
    /// ——那些**刚刚进入范围**的设计库自己并没有文件变更事件，不重扫就得等下次重启
    /// 才会被发现。
    pub async fn resweep_for_scope_change(&self) -> anyhow::Result<()> {
        self.sweep_watch_dirs("scope-refresh", false).await
    }

    /// 重扫监控目录并入队：解范围 → 遍历 → F6 → 发现 → 入队。
    ///
    /// `ensure_archives` 只有启动那一次为真：`SyncPublisher::ensure_archive` 会把整个
    /// 库文件重压一遍，重扫时再压一遍纯属白烧 CPU。
    async fn sweep_watch_dirs(&self, origin: &str, ensure_archives: bool) -> anyhow::Result<()> {
        let watch_dirs = self.watch_dirs();
        // 目录集合为空时这轮重扫会「成功地」扫出 0 个候选，与「确实没有新会话」
        // 长得一模一样。它同时也是手动摄入的候选面，所以必须自己喊出来。
        // 这个判定只属于整面重扫：子集补扫（share-remount）的列表由调用方保证非空。
        if watch_dirs.is_empty() {
            let msg = format!(
                "[{origin}] 监控目录列表为空：没有解析出任何 *000 库目录，本轮不会发现任何库；\
                 检查 DbOption.toml 的 project_path / included_projects / project_dirs，\
                 启动日志「监控目录解析」一段列出了逐项目原因"
            );
            log::error!("{msg}");
            eprintln!("{msg}");
        }
        self.sweep_dirs(origin, ensure_archives, watch_dirs).await
    }

    /// 对给定目录子集做一轮发现与入队（[`Self::sweep_watch_dirs`] 的执行体）。
    ///
    /// 目录集合参数化是给共享盘重挂轮用的：补挂成功后只补扫刚恢复的那几个目录。
    /// 整面重扫在网络盘上可能分钟级，而那次 await 就睡在 `async_watch` 的事件
    /// select 循环里——扫多久，文件事件就积压多久。刚恢复的目录本来就必须扫
    /// （PollWatcher 不补发停机期间的事件），其余目录没有理由陪跑。
    async fn sweep_dirs(
        &self,
        origin: &str,
        ensure_archives: bool,
        watch_dirs: Vec<PathBuf>,
    ) -> anyhow::Result<()> {
        // 本期执行范围与手动路径走同一个谓词（`in_scope`）。自动路径过去只过类型
        // 白名单，于是 MDB 外的设计库照样入队——预览说它不在范围里、队列里却有它的
        // 任务行，而两条路径喂的是同一个 worker。
        //
        // 解不出范围就一个库都不入队，与 `enqueue_manual_update` 同一条纪律：宁可这轮
        // 不跑，也不能退回「扫全项目」。全新项目不会撞上这条——库里一条 MDB 都没有时
        // `resolve` 给的是 bootstrap 范围（只放行 SYS meta），不是错误。
        let scope = match self.update_scope(None).await {
            Ok(scope) => scope,
            Err(error) => {
                let msg = format!("[{origin}] 无法确定本期执行范围，未入队任何批次: {error:#}");
                log::error!("{msg}");
                eprintln!("{msg}");
                return Ok(());
            }
        };
        if let Some(warning) = scope.warning() {
            println!("[{origin}] {warning}");
        }

        let mut params: IndexMap<PathBuf, crate::data_interface::batch_scheduler::DiscoveredBatch> =
            IndexMap::new();
        // F6：同 dbnum 多文件检测（阻断不挑选，与手动路径一致）。按「归属项目 + dbnum」
        // 判重，跨项目同号的 sys 库不算重复。
        let mut seen_dbnums: HashMap<(String, u32), PathBuf> = HashMap::new();
        let mut blocked_dupes: HashSet<(String, u32)> = HashSet::new();
        // 范围外的库：聚合成一句，别让 258 行「跳过」把重扫日志淹掉。
        let mut out_of_scope: Vec<String> = Vec::new();
        let time = Instant::now();
        log::debug!("[{origin}] 监控目录: {watch_dirs:?}");

        // 遍历所有监控目录（深度见 [`INGEST_MAX_DEPTH`]）。
        for watch_dir in &watch_dirs {
            // 按文件大小降序排列，优先处理大文件
            for entry in WalkDir::new(watch_dir)
                .max_depth(INGEST_MAX_DEPTH)
                .sort_by(|a, b| {
                    let a_len = a.path().metadata().map(|m| m.len()).unwrap_or_default();
                    let b_len = b.path().metadata().map(|m| m.len()).unwrap_or_default();
                    b_len.cmp(&a_len)
                })
            {
                // 单个条目读不动就跳过它。过去这里是 `?`：共享盘在遍历途中抖一下，
                // 整轮重扫就此中止，而启动路径上 `init_watcher()` 的 `?` 会把这个错误
                // 一路抛到 `run_cli`，**整个服务起不来**。同一条纪律见下面的文件名分支。
                let dir_entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        log::warn!("[{origin}] 跳过读不动的目录条目（{watch_dir:?}）: {error}");
                        eprintln!("[{origin}] 跳过读不动的目录条目（{watch_dir:?}）: {error}");
                        continue;
                    }
                };
                let path = dir_entry.path();

                // 跳过目录
                if path.is_dir() {
                    continue;
                }

                // 黑名单 + AVEVA 库命名白名单，与 async_watch、重复 dbnum 复查、
                // 手动候选扫描共用同一个谓词（见 `is_candidate_db_file`）。
                if !is_candidate_db_file(path) {
                    continue;
                }

                // 取文件名（不含扩展名）。取不到就跳过这一个条目——过去这里是 `?`，
                // 一个非 UTF-8 的文件名能把整轮重扫连同 `init_watcher` 一起打掉，
                // 而 `async_watch` 遇到同样的情况只是 continue。两条自动路径同口径。
                let Some(file_name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    println!("[{origin}] 跳过无法解析文件名的条目: {}", path.display());
                    continue;
                };

                // 解析数据库基本信息
                let Some(DbBasicInfo {
                    db_type,
                    ses_pgno,
                    db_no,
                }) = try_parse_db_basic_info(path)
                else {
                    println!("跳过无法读取 E3D 数据库头的文件: {}", path.display());
                    continue;
                };

                // 归属项目取自「文件落在哪个监控目录下」，不是配置里的主项目名。
                // 后者是 SurrealDB 的库名，拿它当归属会让 acp000 / ZDJ000 下的库被送去
                // 主项目目录里找，必然找不到、批次必然 failed（见 `project_of_path`）。
                // 必须先于范围门：范围门要用它判「是不是别的项目的运行态系统库」。
                let project = self.owning_project(path);

                // 本期 MDB 声明的设计库（SYS meta 例外），与手动路径共用（`in_scope`）。
                let in_scope = self.in_scope(&scope, &project, &db_type, db_no);
                if !in_scope {
                    if is_foreign_runtime_sys(&self.db_option, &project, &db_type) {
                        println!(
                            "[{origin}] 忽略非主项目的运行态系统库: project={project} \
                             db_type={db_type} dbnum={db_no} file={file_name}\
                             （dbnum 只在项目内唯一，本库只承载主项目 {} 的系统库）",
                            self.db_option.project_name
                        );
                        continue;
                    }
                    // CATA 永远不进执行范围（update_scope 决策 A），但**登记不能跟着跳**：
                    // 按需 CATA 闭包的定位器从 dbnum_watermark 读目录文件身份
                    // （`cata_closure::load_dbnum_files_from_watermark` 的契约），全新
                    // 库上没有这些行时，闭包对分支组件解不出任何依赖（parsed=0），
                    // BRAN/HANG 的目录几何整体生成不出来。生产老库靠范围纪律之前的
                    // 遗留登记行掩盖了这一点。所以 CATA 走完 F6 判重与观察落库，
                    // 只是不入队；其余范围外类型维持原跳过语义。
                    if !db_type.eq_ignore_ascii_case("CATA") {
                        // 逐个打印会在整面重扫时刷屏（AvevaMarineSample 目录里躺着
                        // 287 个 DESI，MDB 只声明 29 个），聚合成一句收在循环外。
                        out_of_scope.push(format!("{db_type}:{db_no}"));
                        continue;
                    }
                }
                // 读不出最新会话号就跳过本轮该文件，与手动路径同口径
                // （`manual_update::scan_project_candidates` 的 warn+跳过）。老写法
                // `.unwrap_or_default()` 把读失败吞成 0：对 applied > 0 的库伪造出
                // 「文件回退」，把假观察值（file_latest_sesno = 0）写进登记行，还让
                // 控制台播报一次实际不会发生的整库重建——reinit 形状 1..=0 过不了
                // 入队的 covers 守卫，日志与事实不符（2026-08-13 审计 P1）。
                let file_latest_sesno =
                    match PdmsIO::new(&project, path.to_path_buf(), true).get_latest_sesno() {
                        Ok(sesno) => sesno,
                        Err(error) => {
                            let msg = format!(
                                "[{origin}] 跳过无法读取最新会话的数据库文件 {}: {error}",
                                path.display()
                            );
                            log::warn!("{msg}");
                            eprintln!("{msg}");
                            continue;
                        }
                    };
                log::debug!("扫描 {path:?}: file_latest_sesno={file_latest_sesno}");

                // 建立文件名到完整路径的映射
                self.watcher
                    .file_name_full_path_map
                    .insert(file_name.to_owned(), path.to_path_buf());

                // F6：同一 dbnum 出现多个文件 → 阻断该 dbnum（不自动挑选）。
                //
                // 必须先于 scan_and_check_file（2026-07-26 审计 B3）：record_scan 按 dbnum
                // UPSERT 身份字段（file_name/file_path/file_size/file_latest_sesno），重复
                // 文件一旦先落库，就会把首见文件的身份覆盖掉，此后即使阻断该 dbnum，
                // dbnum_watermark 里记的也已经是「后来那个」文件，回退/迁移检测的基准被污染。
                //
                // 键是 (归属项目, dbnum) 而不是单独的 dbnum：不同项目各自的 sys 库
                // （amssys / acpsys / zdjsys）天然共用 dbnum=8191，只按 dbnum 判重会把
                // 三个正常的库一起阻断。同项目内的人手副本仍然照旧被拦住。
                if let Some(prev) = seen_dbnums.insert((project.clone(), db_no), path.to_path_buf())
                {
                    blocked_dupes.insert((project.clone(), db_no));
                    println!(
                        "F6 发现项目 {} 内同 dbnum={} 的多个文件，阻断该 dbnum：{:?} / {:?}",
                        project, db_no, prev, path
                    );
                    continue;
                }
                // F6：文件观察落库 + 回退/迁移检测；回退按整库重建入队（ADR-021），
                // 其余阻断类异常跳过（水位不回退）。
                let gate = self
                    .scan_and_check_file(
                        &project,
                        path,
                        file_name,
                        &db_type,
                        db_no,
                        file_latest_sesno as i32,
                    )
                    .await;
                if gate == ScanGate::Blocked {
                    continue;
                }

                // 范围外的 CATA 到此为止：身份已登记（供按需闭包定位），不入队
                // ——回退重建也一样（CATA 永远不进执行范围，重建批次没有 worker
                // 会认领它）。
                if !in_scope {
                    continue;
                }

                // 只有开启MQTT功能时，才需要初始化压缩数据包用于异地同步
                #[cfg(feature = "mqtt")]
                if ensure_archives {
                    use crate::data_interface::sync_publisher::SyncPublisher;
                    if let Err(e) = SyncPublisher::ensure_archive(&path.to_path_buf()).await {
                        eprintln!("初始化存档失败 {:?}: {}", file_name, e);
                    }
                }

                // 回退：按首次导入形状入队重建批次，绕过 discover_batch——那道门
                // 的「水位已覆盖」早退（file_latest <= applied）对回退恒成立，而
                // 这里的依据是 F6 裁决，不是水位比对。
                if gate == ScanGate::Reinit {
                    params.insert(
                        path.to_path_buf(),
                        self.reinit_batch(
                            &project,
                            path,
                            &db_type,
                            db_no,
                            file_latest_sesno as i32,
                        ),
                    );
                    continue;
                }

                // 需不需要更新只看「文件会话号 vs 水位」；从未解析（水位 0）的库
                // 同样入队，worker 执行体的 `needs_initial_load` 会把基线接管过去
                // （与手动路径同口径——两条路径合流后只剩这一份判定）。
                match self
                    .discover_batch(&project, path, &db_type, db_no, file_latest_sesno as i32)
                    .await
                {
                    Some(found) => {
                        params.insert(path.to_path_buf(), found);
                    }
                    None => {}
                }
            }
        }

        // 范围外的库一条条打会刷屏，一条不打又会让「我明明改了这个库」无处对账
        // ——按 MDB 口径报一次总数与样本，人一眼能判断是不是自己要的那个库落在外面。
        if !out_of_scope.is_empty() {
            let sample = out_of_scope.iter().take(12).join("、");
            println!(
                "[{origin}] {} 个库不在 MDB {} 的声明名单里，本轮不入队（本期声明 {} 个 DESI）：{sample}{}",
                out_of_scope.len(),
                scope.mdb(),
                scope.declared_desi().count(),
                if out_of_scope.len() > 12 { " …" } else { "" }
            );
        }

        // F6：移除被判为「同 dbnum 多文件」的文件（阻断不挑选，阻断的库不入队）。
        if !blocked_dupes.is_empty() {
            params
                .retain(|_p, found| !blocked_dupes.contains(&(found.project.clone(), found.dbnum)));
        }

        // 等所有文件检查完毕后，逐条入队；执行与发布归数据批次 worker。
        if !params.is_empty() {
            log::info!("[{origin}] 重扫待入队批次数: {}", params.len());
        }
        self.enqueue_discovered(origin, Self::sweep_holds_rows(), params);

        println!(
            "[{origin}] 重扫（重建队列）总耗时: {} 秒",
            time.elapsed().as_secs_f32()
        );

        anyhow::Ok(())
    }

    /// 回退重建批次的入队形状（ADR-021）：按首次导入（applied=0，窗口
    /// 1..file_latest）入队，数据一行不动——清库归 worker 执行体的冻结点复核
    /// （`execute_one_dbnum` 复核仍判回退才调 `wipe_dbnum_for_reinit`）。
    ///
    /// 与 [`Self::discover_batch`] 刻意分开：那道门按「文件会话号 vs 水位」判
    /// 有没有活，对回退恒判「已覆盖」；这里的依据是 F6 裁决。窗口时刻按
    /// `1..file_latest` 取——重建就是把整个文件当作待应用窗口。
    fn reinit_batch(
        &self,
        project: &str,
        path: &std::path::Path,
        db_type: &str,
        db_num: u32,
        file_latest_sesno: i32,
    ) -> crate::data_interface::batch_scheduler::DiscoveredBatch {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let (first_pending_sesno_time, file_latest_sesno_time) =
            crate::data_interface::manual_update::window_times_rfc3339(
                project,
                path,
                1,
                file_latest_sesno,
            );
        crate::data_interface::batch_scheduler::DiscoveredBatch {
            project: project.to_string(),
            dbnum: db_num,
            db_type: db_type.to_string(),
            path: path.to_path_buf(),
            file_name,
            applied_sesno: 0,
            file_latest_sesno,
            first_pending_sesno_time,
            file_latest_sesno_time,
        }
    }

    /// 一次发现的公共判定：读水位、比会话号，需要更新则给出待入队批次。
    ///
    /// 基线（水位 0、文件有会话）与增量窗口在这里不分家——都只是「有活要干」，
    /// 具体怎么干由 worker 冻结点重扫时的 `execute_one_dbnum` 决定。
    /// 返回 `None` 表示水位已覆盖或读取失败（失败已打印）。
    ///
    /// `file_name` 从 `path` 现取（完整文件名）：init 重扫与 watch 事件两个调用方
    /// 手里各是一种口径（一个全名一个去了扩展名的 stem），在这里统一。
    async fn discover_batch(
        &self,
        project: &str,
        path: &std::path::Path,
        db_type: &str,
        db_num: u32,
        file_latest_sesno: i32,
    ) -> Option<crate::data_interface::batch_scheduler::DiscoveredBatch> {
        use crate::data_interface::dbnum_state::DbnumState;

        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let applied = match DbnumState::applied_sesno(db_num).await {
            Ok(applied) => applied,
            Err(error) => {
                eprintln!("dbnum={db_num} 读取水位失败，本轮跳过: {error:#}");
                return None;
            }
        };
        if file_latest_sesno <= applied {
            return None;
        }
        if applied == 0 {
            println!(
                "发现从未解析过的文件: {file_name}, db_type={db_type}, 文件最新sesno: {file_latest_sesno}（入队后由基线接管）"
            );
        } else {
            println!(
                "发现需要增量更新的文件: {file_name}, 当前数据库最大sesno: {applied}, 文件最新sesno: {file_latest_sesno}"
            );
        }
        // 队列「保存窗口」列显示的是两端保存的写入时刻（plant-ui ADR-0019），
        // 在这里一次开文件读两页。放在上面那两个早退之后：只有真的有活要干的库
        // 才付这个 IO，水位已覆盖的一律不读。
        let (first_pending_sesno_time, file_latest_sesno_time) =
            crate::data_interface::manual_update::window_times_rfc3339(
                project,
                path,
                applied + 1,
                file_latest_sesno,
            );
        Some(crate::data_interface::batch_scheduler::DiscoveredBatch {
            project: project.to_string(),
            dbnum: db_num,
            db_type: db_type.to_string(),
            path: path.to_path_buf(),
            file_name: file_name.to_string(),
            applied_sesno: applied,
            file_latest_sesno,
            first_pending_sesno_time,
            file_latest_sesno_time,
        })
    }

    /// 把一批发现逐条入队并打日志（init 重扫与 watch 事件两条路径共用）。
    ///
    /// `hold` 分开重扫与真实触发：重扫看到的是**已经躺在那儿**的会话，谁都没要求
    /// 现在处理；文件事件与人工执行才是「有人正在这个库上干活」。判据用参数传而
    /// 不是在这里认 `origin` 字符串——重扫的来源有三个（init / scope-refresh /
    /// share-remount），漏认一个就是一次意外的自动开工。
    fn enqueue_discovered(
        &self,
        origin: &str,
        hold: bool,
        params: IndexMap<PathBuf, crate::data_interface::batch_scheduler::DiscoveredBatch>,
    ) {
        use crate::data_interface::batch_queue::Enqueued;
        use crate::data_interface::batch_scheduler::BatchScheduler;
        use crate::data_interface::task_registry::TaskRegistry;

        let scheduler = BatchScheduler::global();
        let registry = TaskRegistry::global();
        for (_path, found) in params {
            let outcome = scheduler.enqueue(registry, &found, hold);
            let verb = match outcome.outcome {
                Enqueued::New => "新排",
                Enqueued::Merged => "并入会话",
                Enqueued::AlreadyCovered => "已覆盖",
                Enqueued::BehindRunning => "接在运行批次之后",
            };
            let posture = if hold { "，挂起待增量触发" } else { "" };
            println!(
                "[{origin}] dbnum={} {verb}：sesno {}..={}（task {}，排在第 {} 位{posture}）",
                found.dbnum,
                outcome.info.start_sesno,
                outcome.info.end_sesno,
                outcome.info.task_id,
                outcome.info.position
            );
        }
    }

    /// 重扫排出来的行要不要挂起：`startup_autorun` 开着就是历史行为（不挂起）。
    fn sweep_holds_rows() -> bool {
        !crate::options::startup_autorun()
    }

    /// 重挂轮：重新解析配置、补挂缺席的目录，挂上了就补一次重扫。
    ///
    /// 重新解析是必须的而不是「顺手」：共享盘在启动那一刻不可达时，
    /// `plan_watch_dirs` 连它的 `*000` 子目录都列不出来，那些目录压根没进过启动列表，
    /// 只重试老列表永远等不到它们。新解析出来的目录同时登记进
    /// `project_paths::record_discovered_watch_dirs`，摄入侧才看得见它们。
    ///
    /// 补挂之后要重扫：PollWatcher 只报「挂载之后」的变化，停机 / 掉线期间攒下的
    /// 会话不会有任何事件，不重扫就要等下一次有人动那个库才被发现。
    async fn remount_watch_dirs(&self, watcher: &mut PollWatcher, mounted: &mut MountState) {
        use crate::data_interface::project_paths::{
            path_identity, plan_watch_dirs, record_discovered_watch_dirs, record_watch_dir_owners,
        };

        let known: HashSet<String> = self
            .watcher
            .watch_dirs
            .iter()
            .map(|dir| path_identity(dir))
            .collect();
        // 解析要走阻塞线程：对着一台掉线的共享机 `read_dir` 会卡住整个 runtime。
        let db_option = self.db_option.clone();
        let plan = match tokio::task::spawn_blocking(move || plan_watch_dirs(&db_option)).await {
            Ok(plan) => plan,
            Err(error) => {
                log::warn!("重挂轮解析监控目录失败: {error}");
                return;
            }
        };
        record_watch_dir_owners(&plan);
        let discovered = record_discovered_watch_dirs(plan.dirs(), &known);
        if !discovered.is_empty() {
            println!("重挂轮发现新的监控目录: {discovered:?}");
        }

        // 先复查已挂目录还在不在：「挂上过」不等于「还在被监听」，中途掉线的目录
        // 不降级的话永远不会被这一轮回头看。恢复时必须先 unwatch 再重挂——直接重挂
        // 会让 PollWatcher 把同一个目录列两遍，F6 立刻整库阻断。
        let dirs = self.watch_dirs();
        let newly_lost = mounted.refresh_health(&dirs);
        if !newly_lost.is_empty() {
            log::warn!("监控目录失联，降级等待恢复: {newly_lost:?}");
            eprintln!("监控目录失联，降级等待恢复: {newly_lost:?}");
        }
        let released = mounted.unwatch_lost(watcher);
        if released > 0 {
            println!("重挂轮解除了 {released} 个失联目录的旧监听，准备重挂");
        }

        let missing = mounted.missing(dirs);
        if missing.is_empty() {
            return;
        }

        let before = mounted.len();
        let failures = mounted.mount(watcher, &missing);
        let added = mounted.len() - before;
        if added == 0 {
            log::warn!(
                "重挂轮：{} 个目录仍不可达（{}）",
                missing.len(),
                failures.join("；")
            );
            return;
        }

        // 只补扫这次真正挂上的目录，不做整面重扫：这段 await 睡在 async_watch 的
        // 事件 select 循环里，扫多久事件就积压多久；网络盘整面扫可能分钟级，而
        // 其余目录本轮什么都没发生，没有理由陪跑。
        let added_dirs: Vec<PathBuf> = missing
            .iter()
            .filter(|dir| mounted.contains(dir))
            .cloned()
            .collect();
        println!("重挂轮补挂了 {added} 个监控目录，开始补扫停机期间的会话: {added_dirs:?}");
        if let Err(error) = self.sweep_dirs("share-remount", false, added_dirs).await {
            log::error!("补挂后的重扫失败: {error:#}");
            eprintln!("补挂后的重扫失败: {error:#}");
        }
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
        // 远程共享目录(SMB/CIFS/NFS)上 OS 原生事件(Windows ReadDirectoryChangesW /
        // Linux inotify)对「其他主机」的写入不可靠、甚至完全收不到，会导致增量漏检。
        // 因此这里改用 notify 的 PollWatcher：定时 stat 整棵被监控目录树，按 mtime /
        // 新增 / 删除对比得出变化，跨网络共享稳定可靠。轮询间隔默认 30s，可用环境变量
        // AIOS_WATCH_POLL_SECS 覆盖（单位秒；非法或 <=0 时回退默认）。
        let poll_secs = std::env::var("AIOS_WATCH_POLL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&s| s > 0)
            .unwrap_or(30);

        // 创建定时轮询文件监控器（PollWatcher）
        let (mut tx, mut rx) = futures::channel::mpsc::channel(1);
        let mut watcher = PollWatcher::new(
            move |res| {
                futures::executor::block_on(async {
                    let _ = tx.send(res).await;
                });
            },
            Config::default().with_poll_interval(std::time::Duration::from_secs(poll_secs)),
        )?;
        println!(
            "async_watch 使用 PollWatcher 定时轮询（间隔 {poll_secs}s），适配远程共享目录的增量发现"
        );

        // 共享盘不会挑着服务启动的那一秒在线：晚上线、维护重启、网络抖动都是常态。
        // 因此挂载不是一次性动作——挂不上的目录进重挂轮，每 AIOS_WATCH_REMOUNT_SECS
        // 秒重试一次（含重新解析：盘不在时连它的 `*000` 目录都列不出来），挂上就补一次
        // 重扫把停机期间的会话追回来。设为 0 关闭重挂，退回「一个都挂不上就报错退出」。
        let remount_secs = std::env::var("AIOS_WATCH_REMOUNT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        let mut mounted = MountState::new();
        let watch_dirs = self.watch_dirs();
        log::debug!("async_watch 监控目录: {watch_dirs:?}");

        // 单个目录不可达（共享盘掉线、路径写错）不再 panic 掉整个看门狗（T903）：
        // 逐目录告警并继续挂载其余目录。一个都没挂上时也不能装作在工作——那等同于
        // 「看门狗在跑却什么都不监控」，是比 panic 更难发现的静默失效，所以要么进
        // 重挂轮并持续告警，要么（关掉重挂时）带着逐目录原因报错退出。
        let failures = mounted.mount(&mut watcher, &watch_dirs);
        if mounted.is_empty() {
            // 目录列表本身为空是另一种病（配置解析阶段就没解析出目录，见
            // `project_paths::plan_watch_dirs`），此处一个 for 都不会进，过去它与
            // 「三个共享盘全掉线」报的是同一句话。
            let detail = if watch_dirs.is_empty() {
                "监控目录列表为空：配置里没有解析出任何 *000 库目录。检查 DbOption.toml 的 \
                 project_path / included_projects / project_dirs（共享目录可在 project_dirs \
                 里对该项目单独写绝对路径或 UNC），启动日志「监控目录解析」一段列出了逐项目原因"
                    .to_string()
            } else {
                format!(
                    "没有任何监控目录挂载成功；逐目录原因: {}",
                    failures.join("；")
                )
            };
            if remount_secs == 0 {
                return Err(notify::Error::generic(&format!(
                    "{detail}（AIOS_WATCH_REMOUNT_SECS=0，重挂已关闭，看门狗退出）"
                )));
            }
            log::error!("{detail}；每 {remount_secs}s 重试一次，恢复即自动接管");
            eprintln!("{detail}；每 {remount_secs}s 重试一次，恢复即自动接管");
        } else {
            println!(
                "已挂载 {}/{} 个监控目录: {:?}",
                mounted.len(),
                watch_dirs.len(),
                watch_dirs
            );
        }

        // 创建必要的目录结构
        create_dir_all("assets/archives")
            .await
            .map_err(|e| notify::Error::io(e))?;
        create_dir_all("assets/temp")
            .await
            .map_err(|e| notify::Error::io(e))?;
        create_dir_all("assets/meshes")
            .await
            .map_err(|e| notify::Error::io(e))?;

        // 积压补偿（副作用 / 模型待重试）不再由 watcher 顺带做：合流之后
        // 那是数据批次 worker 空闲轮的职责——watcher 只负责「发现即入队」，
        // 事件回调再也不会被一轮增量执行堵住（ADR-011 §2 治的正是这个）。

        // 持续监听文件变化事件；与之并行的是共享盘重挂轮（见 `remount_secs`）。
        let mut remount_tick =
            tokio::time::interval(std::time::Duration::from_secs(remount_secs.max(1)));
        remount_tick.tick().await; // interval 的第一拍是立即触发的，丢掉

        // 周期对账重扫：PollWatcher 的事件只发一次，处理途中失败（SUL_DB 连接抖动、
        // 服务器重启——2026-08-06 现场）事件就永久丢了，E3D 不再保存的话那次变更
        // 谁也追不回来。这里按固定间隔把「文件最新会话号 vs applied 水位」整面重比
        // 一遍，一切来源的漏事件都在一个周期内被追回。入队按水位判定天然幂等，
        // 与启动重扫 / 重挂补扫共用同一条 `sweep_watch_dirs` 路径。
        // `AIOS_WATCH_RECONCILE_SECS` 覆盖间隔，0 = 关闭；整面重扫在网络盘上可能
        // 较慢且与事件处理共用本循环，间隔别设太小。
        let reconcile_secs = std::env::var("AIOS_WATCH_RECONCILE_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300);
        let mut reconcile_tick =
            tokio::time::interval(std::time::Duration::from_secs(reconcile_secs.max(1)));
        reconcile_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        reconcile_tick.tick().await; // 第一拍立即触发，而启动重扫刚扫过，丢掉
        loop {
            let res = tokio::select! {
                incoming = rx.next() => match incoming {
                    Some(res) => res,
                    None => break,
                },
                _ = remount_tick.tick(), if remount_secs > 0 => {
                    self.remount_watch_dirs(&mut watcher, &mut mounted).await;
                    continue;
                }
                _ = reconcile_tick.tick(), if reconcile_secs > 0 => {
                    if let Err(error) = self.sweep_watch_dirs("reconcile", false).await {
                        // 失败不退避不计数：下一拍就是重试。
                        let msg = format!("[reconcile] 周期对账重扫失败，等下一拍重试: {error:#}");
                        log::warn!("{msg}");
                        eprintln!("{msg}");
                    }
                    continue;
                }
            };
            match res {
                Ok(event) => {
                    // 过滤事件类型，只处理增/改/删这类内容相关事件。
                    // PollWatcher 通过 mtime 变化发出 Modify(Metadata(WriteTime))、
                    // 新增发出 Create(Any)、删除发出 Remove(Any)，因此这里放宽为任意
                    // Create/Modify/Remove（仅排除 Access 等纯访问事件）。最终是否真有
                    // 增量仍由后续 sesno 水位复核兜底，误报只多一次廉价头部扫描。
                    let data_changed = matches!(
                        event.kind,
                        notify::EventKind::Create(_)
                            | notify::EventKind::Modify(_)
                            | notify::EventKind::Remove(_)
                    );
                    if !data_changed {
                        continue;
                    }

                    // 记录文件变化事件
                    println!("检测到文件变化: {:?}", &event);

                    // 预过滤：只留候选库文件（黑名单 + AVEVA 库命名白名单，
                    // 与启动重扫、重复 dbnum 复查、手动扫描共用 `is_candidate_db_file`）。
                    let filtered_paths: Vec<_> = event
                        .paths
                        .iter()
                        .filter(|path| is_candidate_db_file(path))
                        .cloned()
                        .collect();

                    if filtered_paths.is_empty() {
                        println!("所有变化的文件都被排除规则过滤，跳过处理");
                        continue;
                    }

                    println!("开始扫描数据库头部信息，过滤后路径: {:?}", &filtered_paths);

                    // 本期执行范围走缓存解析：名单只在 SYS meta 批次落库时才变（那一刻
                    // 缓存被显式失效），SUL_DB 瞬时不可用时暖缓存放行——一次连接抖动
                    // 不该把整批事件丢掉（2026-08-06 现场）。解不出来（冷缓存 + 故障，
                    // 或配置错误）仍整批不入队，绝不退回「扫全项目」；丢掉的事件由
                    // 周期对账重扫兜底追回（见本循环的 reconcile_tick）。
                    let scope = match self.update_scope_cached().await {
                        Ok(scope) => scope,
                        Err(error) => {
                            eprintln!("无法确定本期执行范围，本批文件事件不入队: {error:#}");
                            continue;
                        }
                    };
                    // 范围解不出名单时那句解释必须出现在**这条**路径上。少了它，
                    // 「MDB 还没解析出来」在现场只表现为每 30 秒重复一遍的
                    // 「不在本期执行范围」，没有任何线索指向 SYS meta 库。
                    // 事件是轮询来的，同一句话每轮都会重算，故按内容去重。
                    if let Some(warning) = scope.warning() {
                        warn_scope_once(warning);
                    }

                    // 扫描变化文件的数据库头部信息（使用过滤后的路径）
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(&filtered_paths) {
                        println!("成功扫描到 {} 个数据库头部", new_headers.len());
                        let mut params = IndexMap::new();
                        // F6：本批次内同 dbnum 多文件检测（阻断不挑选），键含归属项目。
                        let mut seen_dbnums: HashMap<(String, u32), PathBuf> = HashMap::new();
                        let mut blocked_dupes: HashSet<(String, u32)> = HashSet::new();

                        // 处理每个扫描到的数据库头部信息
                        for (path, new_header) in &new_headers {
                            println!("正在处理路径: {:?}", path);

                            // 复核一遍候选判定：`scan_db_headers` 的返回集合来自上面的
                            // filtered_paths，但这道门便宜、且它是本函数体内出现在
                            // try_parse_db_basic_info 之前的那一道，别让它退化掉。
                            if !is_candidate_db_file(path) {
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
                            let Some(db_basic_info) = try_parse_db_basic_info(path) else {
                                println!("跳过无法读取 E3D 数据库头的文件: {}", path.display());
                                continue;
                            };
                            let db_type = &db_basic_info.db_type;
                            let db_num = db_basic_info.db_no;

                            // 归属项目取自文件所在的监控目录，与启动重扫同口径；
                            // 必须先于范围门（范围门要用它判别的项目的运行态系统库）。
                            let project = self.owning_project(path);

                            // 本期 MDB 声明的设计库（SYS meta 例外），与启动重扫、
                            // 手动触发共用同一个谓词。
                            if !self.in_scope(&scope, &project, db_type, db_num) {
                                if is_foreign_runtime_sys(&self.db_option, &project, db_type) {
                                    println!(
                                        "忽略非主项目的运行态系统库: project={project} \
                                         db_type={db_type} dbnum={db_num}"
                                    );
                                } else {
                                    println!("{}", out_of_scope_reason(&scope, db_type, db_num));
                                }
                                continue;
                            }

                            // F6：同一 dbnum 出现多个文件 → 阻断该 dbnum（不自动挑选）。
                            // 与 init_watcher 同口径，必须先于 scan_and_check_file（审计 B3）。
                            // 键含归属项目：跨项目同号的 sys 库不是重复。
                            if let Some(prev) =
                                seen_dbnums.insert((project.clone(), db_num), path.to_path_buf())
                            {
                                blocked_dupes.insert((project.clone(), db_num));
                                println!(
                                    "F6 发现项目 {} 内同 dbnum={} 的多个文件，阻断该 dbnum：{:?} / {:?}",
                                    project, db_num, prev, path
                                );
                                continue;
                            }
                            // F6：文件观察落库 + 回退/迁移检测；回退按整库重建
                            // 入队（ADR-021），其余阻断类异常跳过（水位不回退）。
                            match self
                                .scan_and_check_file(
                                    &project,
                                    path,
                                    file_name,
                                    db_type,
                                    db_num,
                                    new_header.latest_ses_data.sesno as i32,
                                )
                                .await
                            {
                                ScanGate::Blocked => continue,
                                ScanGate::Reinit => {
                                    params.insert(
                                        path.to_path_buf(),
                                        self.reinit_batch(
                                            &project,
                                            path,
                                            db_type,
                                            db_num,
                                            new_header.latest_ses_data.sesno as i32,
                                        ),
                                    );
                                    continue;
                                }
                                ScanGate::Proceed => {}
                            }

                            println!(
                                "检查文件: {}, 数据库编号: {}, 文件会话号: {}",
                                file_name, db_num, new_header.latest_ses_data.sesno
                            );

                            // 水位对比即发现；基线与增量窗口都只是「有活要干」，
                            // 由 worker 冻结点重扫时决定怎么干（与 init 同口径）。
                            if let Some(found) = self
                                .discover_batch(
                                    &project,
                                    path,
                                    db_type,
                                    db_num,
                                    new_header.latest_ses_data.sesno as i32,
                                )
                                .await
                            {
                                params.insert(path.to_path_buf(), found);
                            } else {
                                println!("文件 {} 无需更新（水位已覆盖）", file_name);
                            }
                        }
                        // Event batches are not a global file snapshot. Recheck all
                        // non-recursively watched files so duplicates arriving in
                        // separate poll events cannot bypass the dbnum guard.
                        blocked_dupes.extend(self.duplicate_dbnums_across_watch_dirs());

                        // F6：移除被判为「同 dbnum 多文件」的文件（阻断不挑选，不入队）。
                        if !blocked_dupes.is_empty() {
                            println!("F6 阻断重复 dbnum: {blocked_dupes:?}");
                            params.retain(|_p, found| {
                                !blocked_dupes.contains(&(found.project.clone(), found.dbnum))
                            });
                        }

                        // 如果没有需要入队的批次，跳过后续处理
                        if params.is_empty() {
                            continue;
                        }

                        // 发现即入队（ADR-011 §2）：执行、发布与补偿都归数据批次
                        // worker，事件回调从此不再被一轮增量执行堵住。
                        // 文件真的变了 = 有人在这个库上干活：不挂起，并放行该
                        // dbnum 启动时挂起的积压，两段合成一条一起跑。
                        self.enqueue_discovered("watch", false, params);
                    } else {
                        println!("扫描数据库头部信息失败，路径: {:?}", &event.paths);
                    }
                }
                Err(e) => println!("文件监控错误: {:?}", e),
            }
        }

        // 走到这里说明事件流已经关闭（PollWatcher 被丢弃 / 发送端全部消失）。
        // 对一个本该长驻的看门狗而言这不是正常终止：过去这里返回 Ok(())，调用方
        // 一路 .unwrap() 也是 Ok，看门狗就此静默死亡、增量再也不会被发现（T903）。
        log::error!("async_watch 事件流意外关闭，增量看门狗已停止监听");
        Err(notify::Error::generic(
            "async_watch 事件流意外关闭，增量看门狗已停止监听",
        ))
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
        let project_name = &self.db_option.project_name;
        // 获取MySQL连接池
        let connection_str =
            crate::data_interface::tidb_manager::AiosDBManager::get_default_conn_str(
                &self.db_option,
            );
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
                // 从属性映射中获取数据库编号；缺失以 0（未解析）写入并出声——
                // 0 是「拿不到真值」的显式记号，不是可静默默认的正常值
                // （2026-08-13 审计：镜像表里一个看着像真的库号比缺值更难排查）。
                let dbnum = attr_map.get_i32("DBNO").unwrap_or_else(|| {
                    log::warn!(
                        "MySQL 镜像: 元素 {refno:?} 缺 DBNO 属性，NUMBDB 以 0（未解析）写入"
                    );
                    0
                });
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
            let mut updates = Vec::new();
            for (refno, _sesno, _modify_data) in chunk {
                // todo 暂时通过查询surreal来获取最终得值
                if let Some(pe) = get_pe((*refno).into()).await? {
                    let name = if !pe.name.is_empty() {
                        pe.name
                    } else {
                        get_default_name((*refno).into())
                            .await?
                            .unwrap_or("".to_string())
                    };
                    updates.push((pe.owner.refno().0, name, pe.refno.refno().0));
                }
            }
            // 逐条参数绑定执行：NAME 是外部字符串（元素名可含引号/反斜杠），拼进
            // 单引号字面量会破坏语句、让该条 UPDATE 失败且只留 warning
            // （2026-08-13 审计 P2）。OWNER/ID 顺带一起绑定，语句文本从此恒定。
            let update_sql = format!("UPDATE {PDMS_ELEMENTS_TABLE} SET OWNER=?, NAME=? WHERE ID=?");
            for (owner, name, id) in updates {
                match sqlx::query(&update_sql)
                    .bind(owner)
                    .bind(&name)
                    .bind(id)
                    .execute(pool)
                    .await
                {
                    Ok(result) => {
                        if result.rows_affected() == 0 {
                            println!("警告: MySQL 更新元素时未找到对应记录: ID={id} NAME={name}");
                        }
                    }
                    Err(e) => {
                        println!("更新元素失败: {}", e);
                        println!("SQL: {update_sql} (ID={id}, NAME={name})");
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
    /// - **错误可见**: 任一模型节点无法计算时返回错误，由 pending 任务重试
    pub(crate) async fn update_world_transforms(
        &self,
        refnos: &HashSet<RefnoEnum>,
    ) -> anyhow::Result<()> {
        // 如果没有需要更新的节点，直接返回
        if refnos.is_empty() {
            return Ok(());
        }

        println!("开始更新 {} 个元素及其子树的world transform", refnos.len());

        // 第一步：智能筛选 - 获取子树中所有有inst_relate的几何节点。子树按窗口前
        // 持久态解析，与锁域解析同一纪律（mutation_roots_resolve_against_the_
        // pre_window_persistent_state）。
        let refnos_with_inst_relate = self.get_inst_relate_nodes_in_subtree(refnos).await?;

        if refnos_with_inst_relate.is_empty() {
            println!("子树中没有节点有inst_relate数据，无需更新world transform");
            return Ok(());
        }

        println!(
            "子树中有inst_relate数据的节点数量: {}",
            refnos_with_inst_relate.len()
        );

        // 第二步：变换产物刷新走可路由的后半（写入全部经 execute_model_write）。
        let refnos_vec: Vec<RefnoEnum> = refnos_with_inst_relate.into_iter().collect();
        refresh_world_transform_products(&self.db_option, &refnos_vec).await
    }

    /// 获取指定参考号及其子树中所有有inst_relate数据的几何节点
    ///
    /// 这是一个高性能的树遍历查询方法，用于在复杂的层次结构中快速定位需要更新的几何节点。
    /// 该方法复用无深度上限、循环安全的 `pe_owner` 子树遍历，再批量筛选模型节点。
    ///
    /// # 算法优势
    ///
    /// - **智能过滤**: 只返回有inst_relate数据的节点，避免无效处理
    /// - **深度遍历**: 不设固定层数上限
    /// - **错误可见**: 查询或 record id 解码失败会保留待重试任务
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
    /// 查询通过 pe_owner 关系向下遍历：
    /// - 检查根节点本身是否有inst_relate
    /// - 循环安全地检查所有后代
    /// - 返回去重后的结果集
    async fn get_inst_relate_nodes_in_subtree(
        &self,
        refnos: &HashSet<RefnoEnum>,
    ) -> anyhow::Result<HashSet<RefnoEnum>> {
        use crate::SUL_DB;
        use crate::data_interface::helper::{collect_pe_subtree_refnos, pe_thing_to_refno};
        use surrealdb::sql::Thing;

        if refnos.is_empty() {
            return Ok(HashSet::new());
        }

        let mut result = HashSet::new();
        let roots = refnos.iter().copied().collect::<Vec<_>>();
        let subtree = collect_pe_subtree_refnos(&roots).await?;
        let subtree = subtree.into_iter().collect::<Vec<_>>();

        for chunk in subtree.chunks(QUERY_BATCH_SIZE) {
            let pe_keys: String = chunk
                .iter()
                .map(|refno| refno.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            // 隐含直管段的行（out 指向共享单位几何 inst_info:⟨1⟩/⟨2⟩，挂在 BRAN/HANG
            // 名下）必须排除：它的 world_trans 是「单位圆柱 → 世界管段」的缩放矩阵，
            // 由生成层按分支成员的 arrive/leave 点现场推导；拿元素本身的世界变换去覆盖
            // 它，管段会被画成分支原点处的一个单位圆柱。
            //
            // 排除曾经的代价是「挪一个管件，管件动了而管段停在旧位置」（issue #5）。
            // 那条口子堵在计划层，管段的正确变换只有生成层算得出来，所以计划层的办法
            // 一律是「让拥有这段管的那个单元整根重生成」
            // （`model_update_plan::reroute_derived_geometry_units`）：
            //
            // - 位姿目标落在 BRAN/HANG 里（挪管件、挪整条分支）→ 目标本身改判重生成，
            //   根本不会排 Transform 工作项，走不到这里；
            // - 位姿目标在 BRAN/HANG 之上（挪 PIPE/STRU/ZONE/SITE）→ 它保留 Transform
            //   刷子树，同时子树里的每个 BRAN/HANG 各排一条 RegenRoot。
            //
            // 第二种情形下这道排除仍然会跳过子树里的管段行——那是对的：那些行由随后的
            // 重生成重写，这里拿元素的世界变换覆盖它们只会画出一堆单位圆柱。
            let sql = format!(
                "array::flatten(SELECT VALUE IF record::exists(type::thing('inst_relate', record::id(id))) \
                 AND type::thing('inst_relate', record::id(id)).out != inst_info:⟨1⟩ \
                 AND type::thing('inst_relate', record::id(id)).out != inst_info:⟨2⟩ \
                 {{ [id] }} ELSE {{ [] }} FROM [{pe_keys}]);"
            );
            let mut response = SUL_DB.query(&sql).await?.check()?;
            for value in response.take::<Vec<Thing>>(0)? {
                result.insert(pe_thing_to_refno(value)?);
            }
        }

        Ok(result)
    }
}

/// 子树展开之后的变换产物刷新（`update_world_transforms` 的后半，独立成函数以便
/// 在暂存窗口内单测）：重算世界变换 → trans 记录落库 → world_trans 指针改指 →
/// AABB 刷新 → 房间触发。
///
/// 写入必须全部经 `execute_model_write` 路由：暂存窗口内进暂存库 + journal
/// （ADR-017 I1 窗口计算期间持久层零写入），直写模式带写冲突重试。此前指针
/// UPDATE 直打持久层，是 2026-08-07 审核的 P0：窗口执行中途持久层出现指向
/// 暂存专属 trans 记录的悬空指针（窗口阻断则永久悬空、元素从一切
/// `world_trans.d != none` 读者里消失，D9 形态）；同时暂存里的旧指针让窗口内
/// AABB 刷新拿旧位置算包围盒，提交后模型画在新位置而空间树 / 房间归属停在
/// 旧位置（D1 复活）。
pub(crate) async fn refresh_world_transform_products(
    db_option: &DbOption,
    refnos_vec: &[RefnoEnum],
) -> anyhow::Result<()> {
    use aios_core::get_world_transform;

    for chunk in refnos_vec.chunks(TRANSFORM_BATCH_SIZE) {
        let mut update_sqls = Vec::new();
        let mut transform_map: HashMap<u64, String> = HashMap::new();

        // 批量计算和更新
        for &refno in chunk {
            // 重新计算该节点的世界变换矩阵
            if let Some(world_transform) = get_world_transform(refno).await? {
                // 必须写成 `trans:⟨hash⟩` 记录链接，不能直接塞裸对象：全部读者取的都是
                // `world_trans.d`（`query_insts`、`update_inst_relate_aabbs_by_refnos`…），
                // 而 inst_relate 是 schemaless 表，裸对象会被静默接受、`.d` 变成 none，
                // 于是该元素在几何查询里 world_trans 为空，在包围盒刷新的
                // `where world_trans.d != none` 处被整条过滤掉（ADR-010 D9）。
                let json = serde_json::to_string(&world_transform)
                    .map_err(|e| anyhow::anyhow!("序列化Transform失败: {}", e))?;
                let transform_hash = aios_core::gen_bytes_hash::<_, 64>(&world_transform);
                // world_trans_d 与指针同语句原子写（P4 写时物化）：值在内存渲染
                // 纯字面量，journal 维持纯数据；停留在旧值的行内副本会让读者
                // 拿到旧位置的几何（与指针不同步 = 静默错值，比缺值更糟）。
                update_sqls.push(format!(
                    "UPDATE {} SET world_trans = trans:⟨{}⟩, world_trans_d = {};",
                    refno.to_inst_relate_key(),
                    transform_hash,
                    json
                ));
                transform_map.entry(transform_hash).or_insert(json);
            } else {
                anyhow::bail!("无法计算已有模型节点 {refno} 的 world transform");
            }
        }

        // trans 记录要先落库，否则 world_trans 会指向不存在的记录，`.d` 一样取不到。
        crate::fast_model::utils::save_transforms_to_surreal(&transform_map).await?;

        // 指针批量改指：与 trans 记录、AABB 指针同一条写路由——暂存窗口内进
        // 暂存库 + journal，直写模式经 execute_surreal_checked 获得写冲突重试
        // （此前的裸 query 连冲突重试都没有）。
        if !update_sqls.is_empty() {
            let batch_sql = update_sqls.join("");
            println!("执行world transform更新SQL，批次大小: {}", chunk.len());
            crate::surreal_retry::execute_model_write(&batch_sql, "更新 world_trans 指针").await?;
        }
    }

    // world_trans 一变，inst_relate.aabb（由 world_trans * geo.trans 变换 geo.aabb 合并
    // 而来）立即失效。这条便宜路径不重生成几何，永远进不了 process_meshes_update_db_deep，
    // 不在这里显式刷新的话，包围盒与空间树会永久停在旧位置，房间归属随之算错（ADR-010 D1）。
    //
    // replace_exist 必须传 true：默认的 replace_mesh=false 会给 SQL 追加 `and aabb=none`，
    // 而这条路径上的元素全都已经有包围盒，会被整批跳过。
    // 纯 POS/ORI 移动正是「设备从 A 房挪到 B 房」。显式增量入口只为包围盒确实
    // 变化的目标建任务：直写时 AABB 指针、room pending、spatial epoch 同事务，
    // 暂存时把变化寄存进窗口并由尾事务收口，调用方不再做有崩溃窗口的后置入队。
    update_inst_relate_aabbs_by_refnos_incremental(refnos_vec, true).await?;

    println!("world transform更新完成");
    Ok(())
}

#[cfg(test)]
mod transform_subtree_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "manual live: requires the configured AvevaMarineSample Surreal database"]
    async fn live_transform_branch_includes_known_model_child() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let branch = RefnoEnum::from("24381/100817");
        let damp = RefnoEnum::from("24381/100819");
        let nodes = manager
            .get_inst_relate_nodes_in_subtree(&HashSet::from([branch]))
            .await
            .expect("collect model nodes in BRAN subtree");
        assert!(nodes.contains(&damp));
        manager
            .update_world_transforms(&HashSet::from([branch]))
            .await
            .expect("refresh BRAN subtree transforms");
    }
}

/// 2026-08-07 审核 P0 的回归：暂存窗口内纯位姿 Transform 的写路由。
///
/// 此前 `run_staged_non_regen_work` → `update_world_transforms` 的 world_trans
/// 指针批量 UPDATE 用 `SUL_DB.query` 直写持久层——窗口计算期间持久层被写入
/// 指向暂存专属 trans 记录的悬空指针，且暂存里的旧指针让窗口内 AABB 刷新拿
/// 旧位置算包围盒、房间变更判定失灵。该路径此前没有任何测试覆盖。
#[cfg(test)]
mod staged_transform_write_routing_tests {
    use super::*;
    use crate::data_interface::staging::ResourceThresholds;
    use crate::data_interface::staging::lifecycle::create_window_on;
    use surrealdb::engine::any::connect;

    /// 与仓内其它夹具同一保留段（4000000001），序号避开 issue10 与 room_fixture
    /// 的 1..30 段——`GLOBAL_AABB_TREE` 是进程级共享，撞号会污染「树上首次见到」
    /// 的判定基线。
    fn refu(n: u64) -> RefU64 {
        RefU64((4000000001u64 << 32) | n)
    }

    /// 暂存 Transform 的三条断言（修复前本用例必红）：
    ///
    /// 1. **journal**：trans 记录 INSERT 与 world_trans 指针 UPDATE 都必须进
    ///    journal（`ExecMode::Both`），暂存行改指新 trans 记录且新指针可解引用
    ///    （D9 不悬空）；
    /// 2. **零落盘**：本用例刻意不连接 `SUL_DB`（与
    ///    `staging_context_routes_reads_and_never_touches_sul_db` 同一负向对照）——
    ///    修复前指针 UPDATE 直打未初始化的全局句柄、当场报错；函数成功本身就是
    ///    「窗口计算期间持久层零写入」的证明；
    /// 3. **房间触发**：位姿位移导致包围盒变化时，房间重算意图必须寄存进窗口
    ///    （D1 不复活）。
    #[tokio::test(flavor = "multi_thread")]
    async fn staged_transform_routes_pointer_updates_through_the_journal() {
        let root = RefnoEnum::from(refu(777001));
        let equi = RefnoEnum::from(refu(777002));
        let root_pe = root.to_pe_key();
        let equi_pe = equi.to_pe_key();
        let equi_inst = equi.to_inst_relate_key();
        let root_id = root_pe.trim_start_matches("pe:");
        let equi_id = equi_pe.trim_start_matches("pe:");

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7986, 2, 2, ResourceThresholds::default())
            .await
            .expect("create window");

        // 窗口内已解析的暂存世界：EQUI 的名词表行带新 POS（解析写入后的形态），
        // inst_relate 仍指旧 trans 记录——Transform 工作项拿到的正是这个状态。
        // 几何侧一条 geo_relate → inst_geo（带 aabb 与 trans），让 AABB 刷新
        // 有东西可算。
        window
            .staging_db()
            .query(format!(
                "UPSERT {root_pe} CONTENT {{ noun: 'SITE', deleted: false, refno: SITE:⟨{root_id}⟩ }};\
                 UPSERT SITE:⟨{root_id}⟩ CONTENT {{ TYPE: 'SITE', NAME: '/ZZTR-ROOT' }};\
                 UPSERT {equi_pe} CONTENT {{ noun: 'EQUI', deleted: false, owner: {root_pe}, refno: EQUI:⟨{equi_id}⟩ }};\
                 UPSERT EQUI:⟨{equi_id}⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/ZZTR-EQUI', POS: [1000.0, 0.0, 0.0] }};\
                 CREATE trans:zztr_old SET d = {{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }};\
                 CREATE aabb:zztr_geo SET d = {{ mins: [0.0, 0.0, 0.0], maxs: [100.0, 100.0, 100.0] }};\
                 CREATE inst_info:zztr_geo;\
                 CREATE inst_geo:zztr_geo SET meshed = true, visible = true, aabb = aabb:zztr_geo;\
                 RELATE inst_info:zztr_geo->geo_relate->inst_geo:zztr_geo \
                     SET trans = trans:zztr_old, geo_type = 'Pos', visible = true;\
                 RELATE {equi_pe}->{equi_inst}->inst_info:zztr_geo \
                     SET world_trans = trans:zztr_old, aabb = aabb:zztr_geo, solid = true, generic = 'EQUI';"
            ))
            .await
            .expect("plant staged fixture")
            .check()
            .expect("staged fixture applied");

        let db_option = DbOption::default();

        window
            .scope(refresh_world_transform_products(&db_option, &[equi]))
            .await
            .expect("暂存 Transform 必须全程只写暂存与 journal（SUL_DB 未连接，直写即错）");

        // 1. journal：两笔写都在场。
        let journal = window.journal().await;
        let journal_sqls = journal.iter().map(|entry| &entry.sql).collect::<Vec<_>>();
        assert!(
            journal_sqls
                .iter()
                .any(|sql| sql.contains("INSERT IGNORE INTO trans")),
            "trans 记录必须随 journal 写回: {journal_sqls:#?}"
        );
        let pointer_marker = format!("UPDATE {equi_inst} SET world_trans = trans:");
        assert!(
            journal_sqls.iter().any(|sql| sql.contains(&pointer_marker)),
            "world_trans 指针 UPDATE 必须进 journal（修复前它直写持久层、journal 缺位）: \
             {journal_sqls:#?}"
        );

        // 2. 暂存行改指新 trans 记录，且新指针在暂存世界可解引用。
        let mut response = window
            .staging_db()
            .query(format!(
                "RETURN record::id({equi_inst}.world_trans);\
                 RETURN {equi_inst}.world_trans.d != NONE;"
            ))
            .await
            .expect("read staged pointer")
            .check()
            .expect("valid staged pointer query");
        let trans_id: Option<String> = response.take(0).expect("take trans id");
        let resolvable: Option<bool> = response.take(1).expect("take resolvable");
        let trans_id = trans_id.expect("staged inst_relate 必须有 world_trans 指针");
        assert_ne!(trans_id, "zztr_old", "指针必须改指重算出的新 trans 记录");
        assert_eq!(
            resolvable,
            Some(true),
            "新指针必须指向暂存里存在的 trans 记录（D9 不悬空）"
        );

        // 3. 位姿位移 → 包围盒变化 → 房间重算意图寄存进窗口。
        let spatial = window.deferred_spatial().await;
        assert_eq!(
            spatial.room_changes.get(&equi),
            Some(&"EQUI".to_string()),
            "包围盒确实变了的位姿目标必须寄存房间变更: {:?}",
            spatial.room_changes
        );

        window.drop_database().await.expect("cleanup");
    }

    /// 「回退即红」源码钉：变换产物刷新的函数体内不得出现 SUL_DB 直连，指针
    /// 批量 UPDATE 必须经 execute_model_write 路由。函数嵌着窗口设施与实库
    /// 查询、无法用纯函数钉住，与本文件其余源码钉同一手法（marker 用 `concat!`
    /// 拼接，避免本测试自己的字面量先于真函数被命中）。
    #[test]
    fn transform_pointer_updates_route_through_execute_model_write() {
        let src = include_str!("increment_manager.rs");
        let body = src
            .split_once(concat!(
                "pub(crate) async fn ",
                "refresh_world_transform_products("
            ))
            .expect("refresh_world_transform_products 必须存在")
            .1
            .split_once(concat!("mod ", "transform_subtree_tests"))
            .expect("函数体到下一个测试模块为止")
            .0;
        assert!(
            !body.contains(concat!("SUL", "_DB")),
            "变换产物刷新不得出现任何 SUL_DB 直连（2026-08-07 P0）"
        );
        assert!(
            body.contains("execute_model_write"),
            "world_trans 指针 UPDATE 必须经 execute_model_write 路由"
        );
    }
}
