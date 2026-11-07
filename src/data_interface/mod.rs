/// CATA 依赖闭包按需入库：每一批 DESI 写回都把它依赖的目录元素一起写进库（plan 2026-09-06 §3 D5–D7）。
pub mod cata_dependency;
pub mod db_model;
/// 一个 dbnum 初始化的编排（ADR-077）：解析 ∥ 生成 ∥ 两条写入 lane → 核对 → 一笔收尾。
pub mod dbnum_initialize;
/// 元素数据行的批量渲染器：先删后插、每表一条 `INSERT IGNORE`（plan 2026-09-06 §3 D1/D2）。
/// rocksdb 形态下基线与增量唯一的写法出口。
pub mod element_batch;
pub mod mdb_read_store;
/// PE 写事务的包装（`BEGIN / COMMIT`、上限、切块、执行；原 `dbnum_stats`，ADR-071 D6 / S7 搬出）。
pub mod pe_batch;
/// 落盘写者（ADR-077）：按 lane 的有界通道 + 后台单写者，属性 / 模型各一条 lane，不同 lane 并发、同 lane 串行。
pub mod persist_writer;
/// 数据面事务切块上限（plan 2026-09-06 §3 D3）：2 MiB / 5000 行 / 1000 元素，可配。
pub mod tx_limits;
pub mod uda_catalog_provider;
// pub mod spatial_model;
pub mod interface;
pub mod structs;
pub mod tidb_manager;

pub mod mesh_manager;

pub mod db_manager;

pub mod project_paths;

/// 项目定位器（ADR-078 D2）：项目码 → 库目录，只在人指定的搜索根里按 `<code>000` 找。
pub mod project_locator;

/// MDB 闭包（ADR-078 D1）：要开哪些项目由主项目 SYS 里该 MDB 的 CURD 声明按项目码派生，
/// 每个码交给 `project_locator` 定位；`included_projects` 不再参与。
pub mod mdb_closure;

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

/// memory-routed 库的整库即时生成：冻结根快照、任务 single-flight 与 ready-roots 对账。
pub mod model_dbnum_ensure;

pub mod model_impact;

pub mod sesno_range;

/// 数据面的元素记录 `EleData`（自 old-parse-pdms-db `parse::EleData` 原样搬入，e3d-io 收集器的产物）。
pub mod ele_data;

pub mod direct_pipe_catalog;
pub mod direct_spec_catalog;
/// 数据面操作类型与 SurrealQL 渲染（`EleOperationData` 家族，自 old-pdms-io `io.rs` 原样搬入）。
pub mod ele_ops;
pub mod pipe_model_adapter;

/// UKEY → UDA 定义 refno（`ATT_UDA` 行 `u` 链接的来源；替代 `aios_core::get_uda_refno` 的逐个点查）。
pub mod uda_keys;

/// 库文件头 / 会话页读法（e3d-io 版，替代 `DbPageBasicInfo` / `PdmsIO`）。
pub mod db_file_header;

/// 按 dbnum 的文件世代账本：`(FileIdentity, latest_sesno)` 任一变化 +1，区分会话前进 / 回滚 / 换文件
/// （母计划不变量 7；阶段 4/5 切片接线，本模块纯内存）。
pub mod db_generation;

/// 项目内全部数据库的 DAB session tag 启动聚合，以及 `/health` 的有界只读快照。
pub mod dabacon_tag_summary;

/// 冻结源：收集时冻住的文件世代与目标会话（替代 `SnapshotToken` / `DabaconSnapshot`）。
pub mod frozen_source;

/// dbnum 任务收尾的一笔控制面事务：数据 / 模型 / 房间水位 + 写入标记，事实全来自任务内存
/// （ADR-071 D3）。
pub(crate) mod task_end;

/// 一个 dbnum 任务这一版攒下的副作用集合（空间刷新 / 删除、房间清边）——内存里的那本，
/// 替掉「文件轮次」那一行的写进去又读回来（ADR-071 D1）。
pub(crate) mod task_effects;

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

/// 机器级写者锁：按端点 + namespace + database 一台库只许一个写者（2026-09-10 数据窗口计划 §2）。
pub mod database_writer_lock;

/// 交付删除记录（`datacenter_delete`）的只读读者：`GET /api/v1/datacenter/deletes` 的数据面（spec §4.14）。
pub mod datacenter_delete_reader;
pub mod fast_delete;

pub mod manual_update;

pub mod mdb_membership;

/// SHA-256 摘要（原 `model_publish_queue::digest`，ADR-071 S7 搬出）。
pub mod digest;
/// 本 MDB 的元素骨架（noun + owner + ref0 归属），进程级常驻，启动时从 e3d-io 预热。
/// 属性不在里面——那一档实测 1.29 GiB，见模块头。
pub mod mdb_memory_store;
pub mod mirror_spool;
/// 模型覆盖的只读取数（模型门 / 目录落款核对与反向级联排工；原 `model_update_pending`，
/// ADR-071 S7 搬出）。`gen_root` 表退役后（ADR-079）它只读水位行与进程内账本。
pub mod model_coverage;
/// 进程内的生成根账本（ADR-079）：逐根成败只活在内存，库里模型面唯一的持久事实是
/// `dbnum_watermark.model_sesno`；进程终止账本消失，重启由两枚水位与写入标记决定欠什么。
pub mod model_root_ledger;
/// 强制范围重生成（先删后生成）的未完成留痕（issue #59）：水位不跟着那一删退，进程死在中途时
/// 只有它记得哪片范围还没补回来；启动就绪后以 `force = false` 续跑。
pub mod model_scope_regen;
/// 数据窗口的内存产物 + 同步落库口（原 `model_publish_queue/{structured_data_sink,
/// increment_window_artifact}.rs`，ADR-071 S7 搬出）。
pub mod window_landing;

pub mod update_scope;

pub mod on_demand_model;

/// 投影删除的入口：根锁 → 库级生成锁 → 结算受影响的根 → 墓碑 → 删除提交，锁序只在这里定。
pub(crate) mod projection_removal;

pub(crate) mod on_demand_db;

pub mod sync_publisher;

pub mod helper;

/// 清理模型「一并删网格文件」那一格：一片范围引用过的内容寻址几何（`inst_geo` 行 + `.mesh`）
/// 的收集与删除。只给 `on_demand_model::clear_model_scope` / `fast_delete::wipe_dbnum_model_face` 用，
/// HTTP 层只传 `ClearGeometry` 开关。
pub(crate) mod mesh_purge;

pub mod cata_closure;
/// 依赖闭包的进程级解析缓存（plan 2026-09-15）：目录元素解析一次、跨调用复用；RocksDB 仍是唯一事实。
pub(crate) mod cata_closure_cache;

/// dbnum → 文件的路由目录，从 SYS/CURD 成员清单造，不含逐元素表（母计划 §5.1 / 切片 W2）。
pub mod mdb_route_directory;
/// RefNo → dbnum 的唯一解法：位解码（母计划 2026-09-12 refno 原生路由，不变量 1）。
pub mod refno_route;

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

pub(crate) mod generation_flight;
pub mod generation_root;
pub(crate) mod room_read_through;
/// 面板自身的变化 → 本库整库房间归属重算（`RoomOnly`，d-819 / ADR-073 D13）：变更集里的房间定义族
/// 与开工前对齐换代两条触发，任务在本批根刷盘之后、收尾之前用现有几何重写两张边。
pub(crate) mod room_recompute;
/// 反向传播（ADR-073 D13 反查）：面板变了 → 拿新盒查全局树 / 管身（谁进来）+ 库上名下的成员边（谁离开）→ 折成本库受影响的根，整库重算降为兜底。
pub(crate) mod room_reverse;

/// Core3D 粒度/去重规则的可执行参考模型。**不在生产路径上**——它是给
/// `generation_root` / `model_update_plan` 当契约的，见模块头。
pub mod core3d_reference;

pub mod model_update_plan;

/// 稳态增量窗口 S→T 的模型面选根：`roots_S ∪ roots_T` 与 `touches_roots`（ADR-056 P2-1 / D9 / N7）。
pub mod window_root_plan;

/// 刚体前移（ADR-066）的纯函数层：只动 `POS / ORI` 的锚点名下、已安定的根不整根重算，投影记录
/// 左乘 `Δ = W_T·W_S⁻¹` 再按 regen 同款发布。开关 `model_transform_cheap_path` 默认关。
pub mod rigid_motion;

/// 结构连接正向级联（core `ADSTCH` 同形，联动对齐计划 C2）：窗口里的 `SJOI` / `FIXING` / `SCTN` …
/// 正向读 `CREF` / `JOIS` / `JOIE` / `CFRA`、核对回指，把对端的生成根并进 `touched`。开关默认关。
pub mod structural_connection;

/// 电缆端点 / 路由反向级联（core 规则 F `ELCXRF` / E `CLIST` 同形，联动对齐计划 C8）：窗口里的
/// `EQUI` / `ELCONN` / `CWBRAN` 到 T 端文件自带的 `STAREF` / `ENDREF` 反向表点查引用它的电缆，
/// 把电缆的生成根并进 `touched`；MDB 其余设计成员的同名表也查，跨库引用者交给 [`cross_db_handoff`]。开关默认关。
pub mod cable_endpoint;

/// 跨库交接（GitHub #34 前置 2 / #35）：级联命中、却落在别的设计库里的元素，窗口落地之后在那个库的文件最新会话上
/// 折成根，排一条只带名单的 `ModelOnly`（与目录级联同一条路）。
pub mod cross_db_handoff;

/// CATA 窗口 S→T 的变动目录件集合 + T 端反向引用表清单——目录反向级联的触发源
/// （ADR-058 D2；2026-09-06 计划 P2）。文件侧纯函数，不碰库。
pub mod cata_cascade_window;

/// 反向引用查询（「联动查询」）：目录件变了 → 谁引用它 → 上卷到生成根。照 core
/// `DB_MDB::findElements` 逐库点查文件自带的反向引用表，不读 `ref_rev`
/// （2026-09-06 计划 P3；共识 d-635 / d-665）。文件侧纯函数，不碰库。
pub mod reverse_reference;

/// 数据窗口的收口尾事务（ADR-071 D4：数据行 + `applied_sesno` 那一笔原子提交）。崩溃恢复
/// 记录 `increment_update_attempt` 随 ADR-072 D1 退役，位置让给 [`write_state`]。
pub mod increment_tail;

/// 写入标记 `dbnum_write_state:{dbnum}` 与「回退到水位」（ADR-072 D1 / D2 / D3）：任务第一笔
/// 写库前置 `writing{S,T}`，收尾那一笔置 `finished`；见到 `writing` 就删掉 `sesno > S` 的痕迹、
/// 三水位退回 S、排 S→T 重跑。
pub mod write_state;

// `model_watermark`（ADR-060 D6 / D7 的启动对账：探针读 `gen_root` 凭证算「有证据支撑的模型水位」、
// 比行上记的高就下调）随 `gen_root` 表退役（ADR-079）：库里没有逐根的证据可拿来对水位，
// `model_sesno` 就是承诺本身，上调只由任务收尾一笔做，回退由写入标记 / 回退到水位做。

/// 启动期模型覆盖复核：模型水位 ↔ 数据水位对齐，落后的库排 `model_sesno → 文件最新` 的
/// ModelOnly（ADR-058 D4 / ADR-079）。「这个库欠到哪」只有两枚水位一个说法。
pub mod startup_reconcile;

pub mod model_concurrency;

/// 生成器身份核对（GitHub #73）：水位行的库级落款 `model_generator` 与本进程指纹不一致就整库排
/// `ModelOnly` 起点 0（启动复核与 `update/execute` 两个入口）。
pub mod generator_reconcile;
pub mod model_rebuild;

/// 数据库架构版本核对：库里 `storage_version:current.version` 与本进程
/// `aios_core::model_storage::MODEL_STORAGE_VERSION` 不一致时只点名、不自动升级。
pub mod storage_version_reconcile;

/// 数据库架构版本一键升级：逐库排 `reinitialize`，全部成功后写 `storage_version:current`。
pub mod storage_upgrade;

pub mod batch_queue;

pub mod batch_scheduler;

pub mod initialization_phase;

pub mod batch_worker;

pub mod queue_stall_diagnostics;

pub mod batch_failure_log;

pub mod task_registry;

/// 终态任务的落盘账本 + 进程内归档（`/api/v1/tasks` 重启后还翻得到历史，计划 2026-10-24 W5）。
pub mod task_history_log;

/// 交付单元开工前 / 收尾后的房间归属快照（单元回执的 `rooms_before` / `rooms_after`，计划 2026-10-24 W1）。
pub mod unit_room_snapshot;

/// 一批的元素级变化清单（`GET /tasks/{id}/changes`，计划 2026-10-24 W3）。
pub mod task_change_list;
/// 交付单元开工前 / 收尾后的几何快照（单元回执的 `geometry_before` / `geometry_after` / `geometry_diff`，计划 2026-10-24 W4）。
pub mod unit_geometry_snapshot;

/// 持久层逐表对拍与「mem:// 实例 + 生产 schema」的中性载体（与暂存无关，P3 不删）。
pub mod table_parity;

/// 直写版崩溃重放对拍（ADR-056 实施约束 3 / spec 035 T171）：窗口语句批中途中止 →
/// 同一固定区间重放 → 与一次成功逐表一致。
#[cfg(test)]
mod direct_window_replay_parity;

/// 删除投影的现行契约：统一删除入口接手前后都要成立。
#[cfg(test)]
mod projection_removal_contract;

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
    /// 只剩建表与统计修复——两处都扫。
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

    /// SYS 元数据不许再长出第二条专用引导（2026-09-07 `bin/sync_sys_only` 退役）。
    ///
    /// SYST/COMM/MISC/GLB/GLOB 与 DICT 是初始化的一部分，不是它前面的一道准备工序：服务侧由
    /// 首轮重扫当普通库入队首载（`needs_initial_load` 对 SYS meta 同样成立）、watcher 按会话增量追；
    /// 不起服务的规格工具走 `AiosDBManager::initialize_project_sys_metadata`——与首载**同一个**
    /// `initialize_dbnum_baseline`。再开一条只灌 SYS 的路，就会有两套「什么时候算已初始化」的判据，
    /// 而那正是 `bootstrap_sys_metadata` 当年的形状：它谁的水位都不看、调用即整库重灌。
    ///
    /// 持久层只有 SurrealDB 一个（宪法 I）：`sqlx` 不许再进 manifest。
    ///
    /// 2026-09-07 退役的 MySQL 那条路（`feature = "sql"`）之所以能烂很久没人发现，正是因为
    /// 它是个**从来没人开的 feature**：默认集里没有、CI 的 `--features` 里没有、脚本里也没有，
    /// 于是门后的代码跟着 `aios_core` 的接口一起腐烂（`AiosDBMgr::get_global_pool` 早就没了），
    /// 一开就是七个编译错。
    ///
    /// 钉在 manifest 上而不是源码上是有意的：仓里还躺着一批**没进模块树的孤儿文件**
    /// （`ssc.rs` / `mdb.rs` / `metadata.rs` / `pcf/*` / `plug_in/*` / `api/*` …，`lib.rs` 里
    /// 那几行 `// pub mod` 就是它们的现状），它们满篇 `sqlx` 却一行都不参与编译。
    /// 只要 manifest 里没有这个依赖，任何一个想复活的文件都会当场编不过——这才是那道真闸。
    #[test]
    fn no_manifest_declares_sqlx_or_the_sql_feature() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for manifest in ["Cargo.toml", "python/Cargo.toml"] {
            let path = root.join(manifest);
            if !path.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect(manifest);
            let hits = manifest_lines_mentioning(&text, &["sqlx", "sql = ["]);
            assert!(
                hits.is_empty(),
                "{manifest} 仍声明 sqlx / `sql` feature（2026-09-07 退役，持久层只有 SurrealDB）：{hits:#?}"
            );
        }
    }

    /// 钉两件事：那个 bin 的文件不许回来，`initialize_project_sys_metadata` 的库前提（只补没有水位的）
    /// 不许被摘掉。
    #[test]
    fn sys_metadata_has_no_bootstrap_of_its_own() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(
            !root.join("src/bin/sync_sys_only.rs").exists(),
            "bin/sync_sys_only 已退役（ADR-058 D8 实施注记四）：SYS 元数据由初始化与增量两条常规路负责，\
             不许再开一条只灌 SYS 的引导"
        );

        let manual_update =
            std::fs::read_to_string(root.join("src/data_interface/manual_update.rs"))
                .expect("read manual_update");
        let body = manual_update
            .split_once("pub async fn initialize_project_sys_metadata(")
            .expect("SYS 元数据入口不在了：它是规格工具唯一的那条路")
            .1
            .split_once("\n    pub(crate) async fn initialize_dbnum_baseline(")
            .expect("baseline entry follows")
            .0;
        assert!(
            body.contains("DbnumState::applied_sesno") && body.contains("applied > 0"),
            "SYS 元数据入口必须只补**还没有水位**的库——已有水位的后续会话归 watcher 增量，\
             在这里重灌就是第二套初始化判据"
        );
    }
}
