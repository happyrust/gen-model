pub mod db_model;
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

/// 文件观察登记：监控目录、文件名→全路径、文件头缓存（替代 old-pdms-io `PdmsWatcher` 的状态部分）。
pub mod db_file_registry;

/// 库文件分发：`.cba` 归档压缩与远端增量 clone（自 old-pdms-io `sync` 搬入）。
pub mod file_sync;

pub mod dbnum_state;

pub mod fast_delete;

pub mod manual_update;

pub mod mdb_membership;

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
