/// CATA 依赖闭包按需入库：每一批 DESI 写回都把它依赖的目录元素一起写进库（plan 2026-09-06 §3 D5–D7）。
pub mod cata_dependency;
pub mod db_model;
pub mod dbnum_stats;
/// 元素数据行的批量渲染器：先删后插、每表一条 `INSERT IGNORE`（plan 2026-09-06 §3 D1/D2）。
/// rocksdb 形态下基线与增量唯一的写法出口。
pub mod element_batch;
/// 数据面事务切块上限（plan 2026-09-06 §3 D3）：2 MiB / 5000 行 / 1000 元素，可配。
pub mod tx_limits;
// pub mod spatial_model;
pub mod interface;
pub mod structs;
pub mod tidb_manager;

pub mod mesh_manager;

pub mod db_manager;

pub mod project_paths;

pub mod extract_family;

pub mod debug_scope;

pub mod watch_scope;

/// 模型增量类型限定（`model_incremental_types`）：只圈模型消费，数据增量不受影响。
pub mod model_type_scope;

pub mod increment_manager;

pub mod sweep_log;

pub mod increment_pipeline;
#[cfg(test)]
mod issue10_direct_add_node;

pub mod window_repair;

pub mod model_refresh;

pub mod model_impact;

pub mod sesno_range;

/// 数据面的元素记录 `EleData`（自 old-parse-pdms-db `parse::EleData` 原样搬入，e3d-io 收集器的产物）。
pub mod ele_data;

/// 数据面操作类型与 SurrealQL 渲染（`EleOperationData` 家族，自 old-pdms-io `io.rs` 原样搬入）。
pub mod ele_ops;

/// UKEY → UDA 定义 refno（`ATT_UDA` 行 `u` 链接的来源；替代 `aios_core::get_uda_refno` 的逐个点查）。
pub mod uda_keys;

/// 库文件头 / 会话页读法（e3d-io 版，替代 `DbPageBasicInfo` / `PdmsIO`）。
pub mod db_file_header;

/// 冻结源：收集时冻住的文件世代与目标会话（替代 `SnapshotToken` / `DabaconSnapshot`）。
pub mod frozen_source;

/// e3d-io 净窗口收集器：数据面增量窗口的默认底座（替代 old-pdms-io `net_window`）。
pub mod e3d_collector;

/// 数据面读透：读透形态（`data_face = read-through`）下 `pe` / `ATT_*` / `pe_owner` 行的唯一
/// 来源——生成一根顺手写透子树、`element/ensure` 按需读透；行形状与基线 / 增量同一份。
pub mod read_through;

/// Complete file-side element attributes (system, explicit and Dictionary UDA)
/// plus the bounded read-through backup queue used by the HTTP API.
pub mod element_attributes;

/// 文件观察登记：监控目录、文件名→全路径、文件头缓存（替代 old-pdms-io `PdmsWatcher` 的状态部分）。
pub mod db_file_registry;

/// 库文件分发：`.cba` 归档压缩与远端增量 clone（自 old-pdms-io `sync` 搬入）。
pub mod file_sync;

pub mod dbnum_state;

/// 每库数据水位与模型水位的唯一三态判决（ADR-060 D9/M8）。
pub mod dbnum_consistency;

/// 每库模型取数源的唯一判据：rocksdb 还是进程内投影（plan 2026-09-06 §8 R1，共识 d-29）。
pub mod model_read_route;

/// Persistent project/namespace/data-face ownership marker for data-plane rows.
pub mod data_face_signature;

pub mod fast_delete;

pub mod manual_update;

pub mod mdb_membership;

/// 本 MDB 的元素骨架（noun + owner + ref0 归属），进程级常驻，启动时从 e3d-io 预热。
/// 属性不在里面——那一档实测 1.29 GiB，见模块头。
pub mod mdb_memory_store;
pub mod mirror_spool;
pub mod model_publish_queue;

pub mod update_scope;

pub mod on_demand_model;

pub(crate) mod on_demand_db;

pub mod sync_publisher;

pub mod side_effect_pending;

pub mod helper;

pub mod cata_closure;

/// ADR-053 direct 模式的取数底座：按 dbnum 池化 e3d-io 引擎、钉在各库
/// `applied_sesno` 上，直接从 db 文件取元素属性，不经 SurrealDB。
pub mod direct_store;
pub mod direct_tree;
pub mod model_source;

/// e3d-io 的 `ElementExtraction` → 生成链消费的 `NamedAttrMap`（ADR-053 Q4）。
pub mod direct_attmap;

/// ADR-053 D3：type / name / backref 三个派生索引，一次全树扫描预建，
/// 磁盘缓存 + 指纹失效。
pub mod direct_index;

pub mod parse_error;

pub mod geom_error;

pub mod embedded_surql;

pub mod generation_root;

/// Core3D 粒度/去重规则的可执行参考模型。**不在生产路径上**——它是给
/// `generation_root` / `model_update_plan` 当契约的，见模块头。
pub mod core3d_reference;

pub mod model_update_plan;

/// 稳态增量窗口 S→T 的模型面选根：`roots_S ∪ roots_T` 与 `touches_roots`（ADR-056 P2-1 / D9 / N7）。
pub mod window_root_plan;

/// 结构连接正向级联（core `ADSTCH` 同形，联动对齐计划 C2）：窗口里的 `SJOI` / `FIXING` / `SCTN` …
/// 正向读 `CREF` / `JOIS` / `JOIE` / `CFRA`、核对回指，把对端的生成根并进 `touched`。开关默认关。
pub mod structural_connection;

/// 电缆端点 / 路由反向级联（core 规则 F `ELCXRF` / E `CLIST` 同形，联动对齐计划 C8 同库版）：窗口里的
/// `EQUI` / `ELCONN` / `CWBRAN` 到 T 端文件自带的 `STAREF` / `ENDREF` 反向表点查引用它的电缆，
/// 把电缆的生成根并进 `touched`。只查本库；开关默认关。
pub mod cable_endpoint;

/// CATA 窗口 S→T 的变动目录件集合 + T 端反向引用表清单——目录反向级联的触发源
/// （ADR-058 D2；2026-09-06 计划 P2）。文件侧纯函数，不碰库。
pub mod cata_cascade_window;

/// 反向引用查询（「联动查询」）：目录件变了 → 谁引用它 → 上卷到生成根。照 core
/// `DB_MDB::findElements` 逐库点查文件自带的反向引用表，不读 `ref_rev`
/// （2026-09-06 计划 P3；共识 d-635 / d-665）。文件侧纯函数，不碰库。
pub mod reverse_reference;

pub mod model_update_pending;

pub mod model_concurrency;

pub mod model_rebuild;

pub mod batch_queue;

pub mod batch_scheduler;

pub mod initialization_phase;

pub mod batch_worker;

pub mod queue_stall_diagnostics;

pub mod batch_failure_log;

pub mod task_registry;

/// 持久层逐表对拍与「mem:// 实例 + 生产 schema」的中性载体（与暂存无关，P3 不删）。
pub mod table_parity;

/// 直写版崩溃重放对拍（ADR-056 实施约束 3 / spec 035 T171）：窗口语句批中途中止 →
/// 同一固定区间重放 → 与一次成功逐表一致。
#[cfg(test)]
mod direct_window_replay_parity;

pub mod staging;

/// old-pdms-io 退出护栏（`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §0；
/// 2026-09-04 从「只许在 legacy 门后」收紧为「一处都不许有」）：
///
/// 1. 根 `Cargo.toml` 与 `python/Cargo.toml` 不得声明 `pdms_io` 依赖、`[patch]` 或任何
///    `legacy_pdms_io` / `legacy_session_replay` / `debug_parse` feature；
/// 2. `src/`、`tests/`、`python/src/` 的**代码行**不得出现 `pdms_io::` / `use pdms_io`，
///    也不得再挂 `feature = "legacy_pdms_io"` / `"legacy_session_replay"` 门。
///
/// 依赖图那一条（`cargo tree --workspace -i pdms_io` 必须匹配不到任何包）由
/// `scripts/Check-LegacyPdmsIo.ps1` 判。对拍的判据是 E3D TTY 录下的会话链 / 宏，不是第二套解析器。
#[cfg(test)]
mod legacy_pdms_io_guard {
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// 只认代码：注释行、以及字符串字面量里的 `"pdms_io::"`（本护栏自己的禁词表）不算。
    fn code_mentions(line: &str, needle: &str) -> bool {
        let code = line.trim_start();
        !code.starts_with("//")
            && code
                .match_indices(needle)
                .any(|(at, _)| !code[..at].ends_with('"'))
    }

    fn manifest_lines_mentioning(manifest: &str, needles: &[&str]) -> Vec<String> {
        manifest
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .filter(|line| needles.iter().any(|needle| line.contains(needle)))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn no_manifest_declares_old_pdms_io_or_its_feature_gates() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for manifest in ["Cargo.toml", "python/Cargo.toml"] {
            let text = std::fs::read_to_string(root.join(manifest)).expect(manifest);
            let hits = manifest_lines_mentioning(
                &text,
                &[
                    "pdms_io",
                    "old-pdms-io",
                    "legacy_pdms_io",
                    "legacy_session_replay",
                    "debug_parse",
                ],
            );
            assert!(
                hits.is_empty(),
                "{manifest} 仍声明 old-pdms-io 或它的 feature 门（2026-09-04 起一处都不许有）：{hits:#?}"
            );
        }
    }

    #[test]
    fn no_source_mentions_old_pdms_io_even_behind_a_feature_gate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["src", "tests", "python/src"] {
            rust_files(&root.join(dir), &mut files);
        }
        assert!(
            files.len() > 50,
            "源码枚举失败，只找到 {} 个 .rs",
            files.len()
        );

        let needles = [
            "pdms_io::",
            "use pdms_io",
            "feature = \"legacy_pdms_io\"",
            "feature = \"legacy_session_replay\"",
        ];
        let mut leaks = Vec::new();
        for file in files {
            let source = std::fs::read_to_string(&file).expect("read source");
            let hit = source
                .lines()
                .enumerate()
                .find(|(_, line)| needles.iter().any(|needle| code_mentions(line, needle)));
            if let Some((index, line)) = hit {
                let relative = file
                    .strip_prefix(root)
                    .unwrap_or(&file)
                    .display()
                    .to_string()
                    .replace('\\', "/");
                leaks.push(format!("{relative}:{}: {}", index + 1, line.trim()));
            }
        }
        assert!(
            leaks.is_empty(),
            "这些文件仍引用 old-pdms-io 或它的 feature 门（数据面只许走 e3d-io / ele_ops；对拍以 E3D TTY 为准）：{leaks:#?}"
        );
    }
}

/// old-parse-pdms-db 退出护栏（`docs/plans/2026-09-04-parse-pdms-db-retirement-inventory.md` B9；
/// 2026-09-04 从「只许在 legacy 门后」收紧为「一处都不许有」）：
///
/// 1. 根 `Cargo.toml` 与 `python/Cargo.toml` 不得声明 `parse_pdms_db` 依赖、`[patch]` 或
///    `legacy_parse_pdms_db` feature；
/// 2. `src/`、`tests/`、`python/src/` 的**代码行**不得出现 `parse_pdms_db::` / `use parse_pdms_db`，
///    也不得再挂 `feature = "legacy_parse_pdms_db"` 门；
/// 3. 基线编排全文不含 legacy 解析器的中间态。
///
/// 依赖图那一条（`cargo tree -e normal -i parse_pdms_db` 必须匹配不到任何包）由
/// `scripts/Check-LegacyParsePdmsDb.ps1` 判。基线全量解析已收敛成 e3d-io 退化窗口
/// （`e3d_collector::stream_baseline` + `versioned_db::baseline`），`member_prune` 整个文件删除；
/// 夹具的验证闸走 `db_file_header::snapshot_presence`，差分 oracle 走 `e3d_collector::read_ele_data`。
/// 对拍以 E3D TTY 为准（共识 d-6），所以 legacy 解析器连「参照臂」这个身份也不再有。
#[cfg(test)]
mod legacy_parse_pdms_db_guard {
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    fn code_mentions_any(line: &str, needles: &[&str]) -> bool {
        let code = line.trim_start();
        !code.starts_with("//")
            && needles.iter().any(|needle| {
                code.match_indices(needle)
                    .any(|(at, _)| !code[..at].ends_with('"'))
            })
    }

    fn relative(root: &Path, file: &Path) -> String {
        file.strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string()
            .replace('\\', "/")
    }

    fn manifest_lines_mentioning(manifest: &str, needles: &[&str]) -> Vec<String> {
        manifest
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .filter(|line| needles.iter().any(|needle| line.contains(needle)))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn no_manifest_declares_old_parse_pdms_db_or_its_feature_gate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for manifest in ["Cargo.toml", "python/Cargo.toml"] {
            let text = std::fs::read_to_string(root.join(manifest)).expect(manifest);
            let hits = manifest_lines_mentioning(
                &text,
                &["parse_pdms_db", "old-parse-pdms-db", "legacy_parse_pdms_db"],
            );
            assert!(
                hits.is_empty(),
                "{manifest} 仍声明 old-parse-pdms-db 或它的 feature 门（2026-09-04 起一处都不许有）：{hits:#?}"
            );
        }
    }

    #[test]
    fn no_source_mentions_old_parse_pdms_db_even_behind_a_feature_gate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["src", "tests", "python/src"] {
            rust_files(&root.join(dir), &mut files);
        }
        assert!(
            files.len() > 50,
            "源码枚举失败，只找到 {} 个 .rs",
            files.len()
        );

        const NEEDLES: [&str; 3] = [
            "parse_pdms_db::",
            "use parse_pdms_db",
            "feature = \"legacy_parse_pdms_db\"",
        ];
        let mut leaks = Vec::new();
        for file in files {
            let source = std::fs::read_to_string(&file).expect("read source");
            let hit = source
                .lines()
                .enumerate()
                .find(|(_, line)| code_mentions_any(line, &NEEDLES));
            if let Some((index, line)) = hit {
                leaks.push(format!(
                    "{}:{}: {}",
                    relative(root, &file),
                    index + 1,
                    line.trim()
                ));
            }
        }
        assert!(
            leaks.is_empty(),
            "这些文件仍引用 old-parse-pdms-db 或它的 feature 门（文件头走 db_file_header::DbBasicInfo、\
             元素记录走 data_interface::ele_data::EleData、noun 字典走 noun_dict、索引 / 属性走 e3d-io；\
             对拍以 E3D TTY 为准）：{leaks:#?}"
        );
    }

    /// 基线编排不许再出现 legacy 解析器的中间态（退役收口方案 B8.3 第 7 条）。
    ///
    /// `DbBasicData` 是 `parse_db_basic_data` 的产物，`member_prune` 是补链轮的事后裁决——
    /// 两样都只有旧基线有。e3d-io 退化窗口一个都不经过（收集走 `e3d_collector::stream_baseline`，
    /// 行形状走 `versioned_db::baseline::render_baseline_batches`），任何一个写回来都意味着
    /// 又开了第二条收集路，而那正是宪法 II 不许的。2026-09-07 起基线编排在
    /// `versioned_db/baseline_ingest.rs`（`sync_total_async_threaded` 壳退役），`database.rs`
    /// 只剩 SYS 引导与统计修复——两处都扫。
    #[test]
    fn the_baseline_orchestration_has_no_legacy_parser_intermediates() {
        const INTERMEDIATES: [&str; 2] = ["DbBasicData", "member_prune"];
        const FILES: [&str; 2] = [
            "src/versioned_db/baseline_ingest.rs",
            "src/versioned_db/database.rs",
        ];
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut leaks = Vec::new();
        for file in FILES {
            let source = std::fs::read_to_string(root.join(file)).expect("read baseline source");
            leaks.extend(
                source
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| code_mentions_any(line, &INTERMEDIATES))
                    .map(|(index, line)| format!("{file}:{}: {}", index + 1, line.trim())),
            );
        }
        assert!(
            leaks.is_empty(),
            "基线编排退回了 legacy 解析器的中间态：{leaks:#?}"
        );
    }
}
