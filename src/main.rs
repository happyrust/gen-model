#![feature(let_chains)]
#![feature(duration_constructors)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

extern crate strum;

use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use std::fs;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use aios_core::SUL_DB;
use aios_core::material::save_all_material_data;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::ssc_setting::{
    set_pbs_fixed_node, set_pbs_node, set_pbs_room_major_node, set_pbs_room_node,
    set_pdms_major_code,
};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::{build_cate_relate, get_db_option};
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
use aios_database::versioned_db::database::*;
use aios_database::{run_app, run_cli};
use bevy_reflect::List;
use chrono::{Datelike, Local, Timelike};
use futures::StreamExt;
use itertools::Itertools;
use log::{LevelFilter, error};
use simplelog::*;
use surrealdb::opt::auth::Root;

/// 增量批次把 `execute_frozen_batch_body` 连同两层 `task_local` scope 一起内联
/// await（`batch_worker.rs` 的 `window.scope(...)`）。debug 版不做 async 状态机
/// 布局优化，这条复合 future 加上各层 poll 帧超过 std 默认的 2MB 线程栈，表现为
/// `thread 'tokio-rt-worker' has overflowed its stack`——服务能起、/health 全绿，
/// 直到真有增量要应用才当场死。release 压得下，所以 `#[tokio::main]` 一直没出事。
///
/// 取值对齐 `fast_model/mesh_generate.rs` 给 `gensec-spro-regression` 线程的 64MB：
/// 栈是保留地址空间、按页提交，多留不花实际内存。
const RUNTIME_STACK_SIZE: usize = 64 * 1024 * 1024;

/// 增量服务的命令行面。
///
/// **无子命令 = 起服务**。仓内所有脚本、`l3_suite` 夹具与部署包都是裸调这个
/// 二进制的，这条默认行为是硬约束，不是便利。
#[derive(clap::Parser)]
#[command(
    name = "aios-database",
    version = aios_database::build_info::display_version(),
    about = "AVEVA E3D 增量解析与几何生成服务"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// 起增量服务（不带任何子命令时的默认行为）。
    Serve(ServeArgs),
    /// 打印可用于发布与二进制补丁追踪的完整构建身份。
    Version(VersionArgs),
    /// 取本机服务的增量链路追踪。纯客户端，走 HTTP，不拿实例锁。
    ///
    /// 不拿锁是关键：`run_app` 一上来就 `acquire_process_instance_lock`，若这个
    /// 子命令也走那条路，服务跑着时它根本执行不了——而那正是唯一想用它的时候。
    Trace(TraceArgs),
    /// 批量 RVM 对拍：给库号与 E3D 连接信息，自动枚举 SITE / APPLDW 根（`--unit zone` 时再按 ZONE
    /// 划成比对单元），E3D TTY 导基准、生成、比对、汇总。与服务一样按工作目录读 `DbOption.toml`；
    /// 不拿实例锁，服务跑着也能用。
    ///
    /// 全部根都是 pass / empty 时退出码 0，否则 1；明细在 `<out>/summary.json`。
    RvmCheck(RvmCheckArgs),
    /// 查库里的 RVM 对拍结果（`rvm_check_run` / `rvm_check_mismatch` / `rvm_check_element`）。直接连
    /// `DbOption.toml` 指的那台库，只读，服务不必开着；应答与 `GET /api/v1/rvm-check/…` 同一份 JSON。
    RvmCheckQuery(RvmCheckQueryArgs),
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum RvmCheckQueryKind {
    /// 按上级根（SITE / APPLDW）汇总：每批每个根一行，带单元数、各终态个数、计数合计、没过的单元。
    Rollup,
    /// 运行记录：一个根 / 单元一行，最新在前。
    Runs,
    /// 差异明细：一条差异一行。
    Mismatches,
    /// 差异按原因种类计数，多的在前。
    Reasons,
    /// 构件级结果（`rvm_check_element`）：一个带几何的构件一行，通过的也有。`--refno` 给构件自己的
    /// refno，取它那一行加记在它名下的派生件（隐式管身）。
    Elements,
    /// 构件级结果按判档计数，按运行与单元分组；每组的 counts 与运行记录里的 counts 同一个形状。
    ElementGrades,
}

#[derive(clap::Args)]
struct RvmCheckQueryArgs {
    /// 查什么。
    #[arg(value_enum)]
    kind: RvmCheckQueryKind,
    /// 批次编号：跑完时打印的 batch_id，即产物目录的最后两级。
    #[arg(long, value_name = "ID")]
    batch_id: Option<String>,
    /// 上级根（SITE / APPLDW）的 refno：它自己那一行，加上它下面的全部单元。
    #[arg(long, value_name = "REF")]
    root: Option<String>,
    /// 根 / 单元自己的 refno；elements、element-grades 时是构件自己的 refno。
    #[arg(long, value_name = "REF")]
    refno: Option<String>,
    /// 构件所在的核对单元（ZONE / 兜底单元；整根跑的是 SITE / APPLDW）的 refno（elements、element-grades）。
    #[arg(long, value_name = "REF")]
    unit_refno: Option<String>,
    /// 判档：exact / chord / reference_artifact / film / off / only_baseline / only_generated /
    /// unkeyable_baseline / unkeyable_generated / centreline_only / container_*（elements、element-grades）。
    #[arg(long)]
    grade: Option<String>,
    /// 只看通过的 / 没通过的（elements、element-grades）。
    #[arg(long, action = clap::ArgAction::Set, value_name = "BOOL")]
    passed: Option<bool>,
    /// 只看图元种类一致的 / 不一致的（elements、element-grades）。
    #[arg(long, action = clap::ArgAction::Set, value_name = "BOOL")]
    kind_match: Option<bool>,
    /// 只看派生件（没有自己 refno 的隐式管身一类）/ 只看有自己 refno 的（elements、element-grades）。
    #[arg(long, action = clap::ArgAction::Set, value_name = "BOOL")]
    derived: Option<bool>,
    /// 只看 refno 解出来的 / 没解出来的（elements、element-grades）。
    #[arg(long, action = clap::ArgAction::Set, value_name = "BOOL")]
    refno_resolved: Option<bool>,
    /// 库号。
    #[arg(long, value_name = "N")]
    dbnum: Option<u32>,
    /// 单元种类：root / zone / fallback（rollup、runs）。
    #[arg(long, value_name = "KIND")]
    unit: Option<String>,
    /// 终态：pass / empty / kind_differ / fail / no-baseline / gen-failed …（rollup、runs）。
    #[arg(long)]
    status: Option<String>,
    /// 三色：pass / kind_differ / fail（rollup、runs）。
    #[arg(long)]
    verdict: Option<String>,
    /// 某一次运行的 run_id（mismatches、reasons、elements、element-grades）。
    #[arg(long, value_name = "ID")]
    run_id: Option<String>,
    /// 构件的 noun（mismatches、reasons、elements、element-grades）。
    #[arg(long)]
    noun: Option<String>,
    /// 差异分类：off / kind_differ / only_baseline / only_generated …（mismatches、reasons）。
    #[arg(long)]
    category: Option<String>,
    /// 原因种类：aabb_shifted / missing_implied_tube …（mismatches、reasons）。
    #[arg(long, value_name = "KIND")]
    reason_kind: Option<String>,
    /// error / warning / info（mismatches、reasons）；elements、element-grades 另有 ok。
    #[arg(long)]
    severity: Option<String>,
    /// 最多返回多少行：runs 缺省 50，mismatches、elements 缺省 500，上限 5000。
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    /// 跳过前多少行（mismatches、elements）。
    #[arg(long, value_name = "N")]
    offset: Option<usize>,
    /// 把 JSON 写进这个文件；不给就打到标准输出（标准输出只有这一份 JSON，可以直接接管道）。
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum RvmCheckStage {
    /// 导出 + 生成 + 比对。
    Full,
    /// 只用 E3D 导基准。
    Export,
    /// 只生成。
    Generate,
    /// 只比对产物目录里已有的基准与生成物。
    Compare,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum RvmCheckUnit {
    /// WORLD 下的 SITE / APPLDW 各成一个比对单元（现状）。
    Root,
    /// 这些根下的每个 ZONE 各成一个比对单元：各自导基准、各自生成即比对。
    Zone,
}

#[derive(clap::Args)]
struct RvmCheckArgs {
    /// 要跑的 DESI 库号，可重复或逗号分隔（`--dbnum 7999,7326`）。
    #[arg(long, value_delimiter = ',', value_name = "N")]
    dbnum: Vec<u32>,
    /// 跑当前 MDB 里全部 DESI 库。
    #[arg(long)]
    all_desi: bool,
    /// 只跑这些根（`=w0/w1`、`w0/w1` 或 `w0_w1`），可重复或逗号分隔。
    #[arg(long, value_delimiter = ',', value_name = "REF")]
    only: Vec<String>,
    /// 比对范围：`node:<refno>` 只比这个节点自己（不下探）；`subtree:<refno>` 比它整棵子树；
    /// 不给 = 整个根（现状）。选中的 refno 必须落在 `--only` 选出的根里，否则报错退出。
    ///
    /// 与 `--only` 分工不同：`--only` 选「哪些根进这批」，`--scope` 选「这些根里比哪一块」。
    #[arg(long, value_name = "node:REF|subtree:REF")]
    scope: Option<String>,
    /// 只取 SITE 类型的根（缺省 SITE + APPLDW，两类合起来才是整库）。
    #[arg(long)]
    site_only: bool,
    /// 比对单元。`zone`：SITE / APPLDW 下的每个 ZONE 各成一个单元（APPLDW 下经 APPLDA、APPDAR
    /// 两层才到 ZONE；不在任何 ZONE 里的子节点各成一个兜底单元），每个单元单独用 E3D 导基准、
    /// 单独生成即比对、不写 generated.rvm。只支持 --stage full / export。
    /// `--only` 可以点名单元，也可以点名上级 SITE / APPLDW（选中它下面的全部单元）。
    #[arg(long, value_enum, default_value_t = RvmCheckUnit::Root)]
    unit: RvmCheckUnit,
    /// 只列出比对单元（写 `<out>/units.json`，打印每个上级根划出多少单元），不碰 E3D、不生成。
    #[arg(long)]
    list_units: bool,
    /// 产物目录（缺省 `output/rvm-check-batch/<时间戳>`）。
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,
    /// 在已有产物目录上续跑：已比对出结果的根跳过。
    #[arg(long)]
    resume: bool,
    /// 允许写进非空的产物目录（每个根都重跑）。
    #[arg(long)]
    force: bool,
    /// 不碰 E3D，从这个目录取基准 `<tag>_<w0>_<w1>.rvm`（如 ams7999_24383_66456.rvm）。
    #[arg(long, value_name = "DIR")]
    baseline_dir: Option<PathBuf>,
    /// 跑哪几步。
    #[arg(long, value_enum, default_value_t = RvmCheckStage::Full)]
    stage: RvmCheckStage,
    /// 生成在本进程里跑。缺省每个根起一个子进程，便于内存护栏与失败隔离。
    #[arg(long)]
    in_process: bool,
    /// 同时生成的根数。
    #[arg(long, default_value_t = 1)]
    jobs: usize,
    /// 可提交内存余量低于此值（GB）时杀掉正在生成的子进程，该根记 commit-guard。
    #[arg(long, default_value_t = 1.5)]
    min_commit_free_gb: f64,
    /// 单个根生成的超时（秒）。
    #[arg(long, default_value_t = 1200)]
    gen_timeout_secs: u64,
    /// 一次 E3D 导出会话的超时（秒）。
    #[arg(long, default_value_t = 3600)]
    e3d_timeout_secs: u64,
    /// E3D 安装目录（`des.exe` 所在，要能跑 `des.exe -tty`）。导基准时必填。
    #[arg(long, value_name = "DIR")]
    install_dir: Option<PathBuf>,
    /// 项目父目录（缺省 DbOption.toml 的 project_path）。
    ///
    /// **它只管一部分**：预检的路径检查、项目的 evars 文件、E3D 导出基准的会话，以及生成时按库号找被引用的库
    /// （`<项目>/<码>000`）。枚举根（WORLD 下的 SITE / APPLDW）、模型树、根所在的设计库文件由配置的
    /// `project_path`（加 `project_search_paths`）决定，**不看这个参数**。
    ///
    /// 给的目录与配置的 `project_path` 不是同一个目录时（只比字面：斜杠方向、大小写、末尾分隔符不算差别），
    /// 启动时在 stderr 告警，`summary.json` / `summary.txt` / 每根的 `run.json` 与入库的运行行里记下两个目录。
    /// 两处若是内容不同的两份项目数据，根的设计库来自一份、基准和被引用的库来自另一份，比出来的差异不可信。
    #[arg(long, value_name = "DIR")]
    projects_dir: Option<String>,
    /// 项目目录名（缺省 DbOption.toml 的 project_name）。
    #[arg(long, value_name = "NAME")]
    project_name: Option<String>,
    /// E3D 项目码（缺省按项目目录里的 `<码>000` 扫出的第一个，如 AMS）。
    #[arg(long, value_name = "CODE")]
    e3d_project: Option<String>,
    /// MDB（缺省 DbOption.toml 的 mdb_name）。
    #[arg(long, value_name = "NAME")]
    mdb: Option<String>,
    /// E3D 用户名。
    #[arg(long, default_value = "SYSTEM")]
    username: String,
    /// E3D 密码；不给时读环境变量 RVM_CHECK_PASSWORD。只进登录串，不落盘。
    #[arg(long)]
    password: Option<String>,
    /// 不把结果写进库（缺省跑完写 `rvm_check_run` / `rvm_check_mismatch`，连 DbOption.toml 指的那台
    /// SurrealDB；`store_mode` 为 mem 时自动跳过）。
    #[arg(long)]
    no_store: bool,
    /// 生成即比对：成员流直喂流式对拍，`generated.rvm` 不必为对拍而写。
    /// 每个根先等它的基准就位（--baseline-dir 放好，或 E3D 导出完）再生成，缺基准就不生成。
    /// 只在 --stage full 成立；--unit zone 固定打开。
    #[arg(long)]
    fused_compare: bool,
    /// 子进程是否写 `generated.rvm`。不给：生成即比对时不写，其余照写。
    /// --emit-rvm false 仅在生成即比对下成立。
    #[arg(long, action = clap::ArgAction::Set, value_name = "BOOL")]
    emit_rvm: Option<bool>,
    /// 比对内存预算（MB），按根算：只管比对，不含生成占的内存。0 = 不设预算。
    ///
    /// 现状：这个值目前**只映射到 vendor 的核对窗口**（`RvmExportOptions.compare_window_bytes`），
    /// 也就是待判成员攒满即并发判一批的那个窗口；预算切分（按预算切批）与超预算拒收都还没实现，
    /// 要等 vendor 暴露摘要大小与记账。所以 0 的当前行为是：核对窗口取 256MB，负载全在内存。
    /// 旧名 `--compare-window-mb` 保留作 alias，脚本不必立刻改。
    #[arg(
        long = "compare-memory-mb",
        alias = "compare-window-mb",
        default_value_t = 0,
        value_name = "MB"
    )]
    compare_memory_mb: usize,
    /// 池级元素缓存上限（条）：跨元素复用解析好的库记录，0 = 默认 200_000。
    /// 大缓存换解析速度、多占驻留；小缓存省内存、满了要重解析。
    #[arg(long, default_value_t = 0, value_name = "N")]
    element_cache: usize,
    /// fused 模式是否落 `compare.json` 全量报告（大件上百 MB）。
    /// false = 结果只入库（rvm_check_run / rvm_check_mismatch），盘上留 judgement.json。
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    emit_compare: bool,
    /// 生成子进程的 rayon 线程数：0 = 逻辑核数（不设上限）。
    /// 批量层原样传给每个子进程，由子进程建全局池时使用。
    #[arg(long, default_value_t = 0, value_name = "N")]
    threads: usize,
    /// 内部用：子进程模式，只生成 --only 指定的根。
    #[arg(long, hide = true)]
    child: bool,
    /// 内部用（--child）：生成即比对的基准 .rvm 路径。
    #[arg(long, hide = true, value_name = "PATH")]
    compare_baseline: Option<PathBuf>,
}

#[derive(clap::Args, Default)]
struct ServeArgs {
    /// 自动初始化新发现的 dbnum；默认仅维护已初始化库，新库从前端手动初始化。
    #[arg(long)]
    auto_initialize_new_dbnums: bool,

    /// **调试用**：把本轮数据批次的检查圈到这些 dbnum，并全程追踪它们。
    ///
    /// 逗号分隔（`7998,8000`）。SYS meta 不受限——MDB 的成员名单存在那些库里。
    /// 这不是运行策略，所以它只能从命令行来，进不了配置文件。开启后 `/health`、
    /// 每一份 preview / execute 回执都会明说本进程是跛的。
    #[arg(long, value_name = "N[,N...]")]
    debug_dbnum: Option<String>,

    /// **调试用**：把增量摄入的数据批次圈到这些 dbnum，不带追踪。
    ///
    /// 逗号分隔（`7998,8000`）。SYS meta 不受限——MDB 的成员名单存在那些库里。
    /// 压过 `DbOption.toml` 的 `watch_dbnums`；与 `--debug-dbnum` 各管各的，两个
    /// 都给就两道门都要过。开启后启动横幅、`/health`、每一份 preview / execute
    /// 回执都会明说本进程的监听范围被收窄了。
    #[arg(long, value_name = "N[,N...]")]
    watch_dbnum: Option<String>,
}

#[derive(clap::Args, Default)]
struct VersionArgs {
    /// 输出机器可读 JSON；补丁生成脚本使用这一形态。
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct TraceArgs {
    /// 只看这一个 dbnum；不给就全都要。
    #[arg(long)]
    dbnum: Option<u32>,
    /// 最多取多少条（取最新的）；0 表示不限。
    #[arg(long, default_value_t = 0)]
    limit: usize,
    /// 服务地址。缺省取配置里的 `server_release_ip`。
    #[arg(long)]
    url: Option<String>,
}

fn main() -> anyhow::Result<()> {
    aios_database::process_diagnostics::install_panic_hook();
    aios_database::process_diagnostics::install_crash_handlers();
    use clap::Parser;

    let cli = Cli::parse();
    // 全局 rayon 池（`room_model` / `db_model` 的 `par_iter`）与几何私有池同样给 64MB 栈（GitHub #78）：
    // 默认 2MB，而发布包不像仓内脚本那样设 `RUST_MIN_STACK`。必须在第一次 `par_iter` 之前建。
    //
    // 建池挪到 `Cli::parse()` 之后：`--threads` 要能决定池大小，而全局池只能建一次
    // （`build_global` 重复调用返回 Err，下面照旧用 `let _ =` 吞掉）。只有 `rvm-check`
    // 有这个旋钮，其它子命令按 0 走默认线程数。
    let threads = match &cli.command {
        Some(Command::RvmCheck(args)) => args.threads,
        _ => 0,
    };
    let mut pool = rayon::ThreadPoolBuilder::new().stack_size(RUNTIME_STACK_SIZE);
    if threads > 0 {
        pool = pool.num_threads(threads);
    }
    let _ = pool.build_global();
    let serve = match cli.command {
        Some(Command::Version(args)) => return print_version(&args),
        Some(Command::Trace(args)) => return fetch_trace(&args),
        Some(Command::RvmCheck(args)) => return run_rvm_check(args),
        Some(Command::RvmCheckQuery(args)) => return run_rvm_check_query(args),
        Some(Command::Serve(args)) => args,
        None => ServeArgs::default(),
    };
    if serve.auto_initialize_new_dbnums {
        aios_database::options::enable_auto_initialize_new_dbnums();
    }
    if let Some(raw) = serve.debug_dbnum.as_deref() {
        // 拼错的取值直接失败，不回落。悄悄吞成「全范围」看起来像参数没生效，吞成
        // 「空集」看起来像什么都没跑，两种都要人再花一轮才发现是自己手误。
        let dbnums = aios_database::data_interface::debug_scope::parse_dbnums(raw)
            .map_err(|message| anyhow::anyhow!(message))?;
        println!(
            "*** 调试限定模式：数据批次只处理 dbnum {dbnums:?}，SYS meta 不受限。\
             这不是正常运行状态。***"
        );
        aios_database::data_interface::debug_scope::set_dbnums(dbnums);
    }
    if let Some(raw) = serve.watch_dbnum.as_deref() {
        // 同样不回落（理由同上）。横幅不在这里打：配置里写的 `watch_dbnums` 也要
        // 有同一句声明，两个来源共用 `run_cli` 那一处出口才不会漏掉配置那条路。
        let dbnums = aios_database::data_interface::watch_scope::parse_dbnums(raw)
            .map_err(|message| anyhow::anyhow!(message))?;
        aios_database::data_interface::watch_scope::set_cli_dbnums(dbnums);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(RUNTIME_STACK_SIZE)
        .build()?
        .block_on(run_app(None))
}

/// 批量对拍。同步跑完整批（E3D、生成子进程、比对都是阻塞调用），不进 tokio、不拿实例锁。
fn run_rvm_check(args: RvmCheckArgs) -> anyhow::Result<()> {
    use aios_database::rvm_check::RunInput;
    use aios_database::rvm_check::batch::{self, BatchOptions, ChildOptions, Stage, Unit};

    let stage = match args.stage {
        RvmCheckStage::Full => Stage::Full,
        RvmCheckStage::Export => Stage::Export,
        RvmCheckStage::Generate => Stage::Generate,
        RvmCheckStage::Compare => Stage::Compare,
    };
    let unit = match args.unit {
        RvmCheckUnit::Root => Unit::Root,
        RvmCheckUnit::Zone => Unit::Zone,
    };
    let (fused_compare, emit_rvm) = batch::fused_and_emit(unit, args.fused_compare, args.emit_rvm);
    let password = args
        .password
        .clone()
        .or_else(|| std::env::var("RVM_CHECK_PASSWORD").ok())
        .unwrap_or_default();
    let connection = RunInput {
        install_dir: args
            .install_dir
            .as_ref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default(),
        projects_dir: args.projects_dir.clone().unwrap_or_default(),
        project_name: args.project_name.clone().unwrap_or_default(),
        e3d_project: args.e3d_project.clone().unwrap_or_default(),
        username: args.username.clone(),
        password,
        e3d_mdb: args.mdb.clone().unwrap_or_default(),
        refno: String::new(),
        baseline_dir: args.baseline_dir.clone(),
    };
    let out = args.out.clone().unwrap_or_else(|| {
        PathBuf::from("output")
            .join("rvm-check-batch")
            .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string())
    });

    let scope = args
        .scope
        .as_deref()
        .map(batch::ScopeArg::parse)
        .transpose()?;

    if args.child {
        anyhow::ensure!(args.dbnum.len() == 1, "--child 只接受一个 --dbnum");
        anyhow::ensure!(!args.only.is_empty(), "--child 需要 --only");
        return batch::run_child(&ChildOptions {
            connection,
            dbno: args.dbnum[0],
            refnos: args.only.clone(),
            scope,
            out,
            gen_timeout: std::time::Duration::from_secs(args.gen_timeout_secs),
            compare_baseline: args.compare_baseline.clone(),
            emit_rvm: args.emit_rvm.unwrap_or(args.compare_baseline.is_none()),
            compare_memory_mb: args.compare_memory_mb,
            element_cache: args.element_cache,
            emit_compare: args.emit_compare,
            threads: args.threads,
        });
    }

    let outcome = batch::run(&BatchOptions {
        connection,
        dbnums: args.dbnum.clone(),
        all_desi: args.all_desi,
        only: args.only.clone(),
        scope,
        site_only: args.site_only,
        unit,
        list_units: args.list_units,
        out,
        resume: args.resume,
        force: args.force,
        baseline_dir: args.baseline_dir.clone(),
        stage,
        in_process: args.in_process,
        jobs: args.jobs,
        min_commit_free_gb: args.min_commit_free_gb,
        gen_timeout: std::time::Duration::from_secs(args.gen_timeout_secs),
        e3d_timeout: std::time::Duration::from_secs(args.e3d_timeout_secs),
        fused_compare,
        emit_rvm,
        compare_memory_mb: args.compare_memory_mb,
        element_cache: args.element_cache,
        emit_compare: args.emit_compare,
        threads: args.threads,
        store: !args.no_store,
    })?;
    if !outcome.clean {
        std::process::exit(1);
    }
    Ok(())
}

/// 查库里的对拍结果。同步跑完：自己连一次库、只读，不进 tokio、不拿实例锁。
fn run_rvm_check_query(args: RvmCheckQueryArgs) -> anyhow::Result<()> {
    use aios_database::rvm_check::store::{
        ElementFilter, MismatchFilter, Query, RunFilter, query_blocking,
    };

    let runs = || RunFilter {
        refno: args.refno.clone(),
        root_refno: args.root.clone(),
        unit: args.unit.clone(),
        dbnum: args.dbnum,
        verdict: args.verdict.clone(),
        status: args.status.clone(),
        batch_id: args.batch_id.clone(),
        limit: args.limit,
        ..RunFilter::default()
    };
    let mismatches = || MismatchFilter {
        run_id: args.run_id.clone(),
        refno: args.refno.clone(),
        root_refno: args.root.clone(),
        dbnum: args.dbnum,
        batch_id: args.batch_id.clone(),
        category: args.category.clone(),
        reason_kind: args.reason_kind.clone(),
        noun: args.noun.clone(),
        severity: args.severity.clone(),
        limit: args.limit,
        offset: args.offset,
        ..MismatchFilter::default()
    };
    let elements = || ElementFilter {
        refno: args.refno.clone(),
        unit_refno: args.unit_refno.clone(),
        root_refno: args.root.clone(),
        dbnum: args.dbnum,
        batch_id: args.batch_id.clone(),
        run_id: args.run_id.clone(),
        noun: args.noun.clone(),
        grade: args.grade.clone(),
        severity: args.severity.clone(),
        passed: args.passed,
        kind_match: args.kind_match,
        derived: args.derived,
        refno_resolved: args.refno_resolved,
        limit: args.limit,
        offset: args.offset,
        ..ElementFilter::default()
    };
    let query = match args.kind {
        RvmCheckQueryKind::Rollup => Query::Rollup(runs()),
        RvmCheckQueryKind::Runs => Query::Runs(runs()),
        RvmCheckQueryKind::Mismatches => Query::Mismatches(mismatches()),
        RvmCheckQueryKind::Reasons => Query::Reasons(mismatches()),
        RvmCheckQueryKind::Elements => Query::Elements(elements()),
        RvmCheckQueryKind::ElementGrades => Query::ElementGrades(elements()),
    };
    let answer = query_blocking(&query)?;
    let text = serde_json::to_string_pretty(&answer)?;
    match &args.out {
        Some(path) => {
            std::fs::write(path, &text)?;
            println!("{} 行 → {}", answer["count"], path.display());
        }
        None => println!("{text}"),
    }
    Ok(())
}

fn print_version(args: &VersionArgs) -> anyhow::Result<()> {
    let info = aios_database::build_info::current();
    if args.json {
        println!("{}", serde_json::to_string(&info)?);
    } else {
        println!(
            "{} {} (release {}, built {}, git {}{}, target {}, profile {})",
            info.product,
            info.build_id,
            info.version,
            info.build_time,
            info.git_commit,
            if info.git_dirty { ", dirty" } else { "" },
            info.target,
            info.profile
        );
    }
    Ok(())
}

/// 走 curl 而不是引一个 HTTP 客户端依赖：仓内已有先例
/// （`l3_suite` 的健康等待、`fixture.rs` 的服务探活都用它），为一个诊断子命令
/// 往服务二进制里塞一整套 TLS 栈不划算。
///
/// Windows 上必须点名 `curl.exe`（PowerShell 的 `curl` 是 Invoke-WebRequest
/// 别名，虽然 CreateProcess 不走别名，点名后缀是仓内既有惯例）；部署目标
/// CentOS 7 上没有 `.exe`，写死会让这个子命令在服务器上必挂（2026-08-18 审核 P3）。
#[cfg(windows)]
const CURL: &str = "curl.exe";
#[cfg(not(windows))]
const CURL: &str = "curl";

fn fetch_trace(args: &TraceArgs) -> anyhow::Result<()> {
    let base = args.url.clone().unwrap_or_else(|| {
        let endpoint = get_db_option().server_release_ip.clone();
        if endpoint.starts_with("http") {
            endpoint
        } else {
            format!("http://{endpoint}")
        }
    });
    let mut url = format!(
        "{}/api/v1/trace?limit={}",
        base.trim_end_matches('/'),
        args.limit
    );
    if let Some(dbnum) = args.dbnum {
        url.push_str(&format!("&dbnum={dbnum}"));
    }
    let output = std::process::Command::new(CURL)
        .args(["--silent", "--show-error", "--fail", &url])
        .output()
        .map_err(|e| anyhow::anyhow!("调用 {CURL} 失败: {e}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "取追踪失败（{}）：{}\n服务没起、或它不是带 http_api 的构建时会走到这里。",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    std::io::stdout().write_all(&output.stdout)?;
    println!();
    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "DB";
    let hash = db1_hash(noun);
    dbg!(hash);
    let hashes = [0xE567E, 640481, 919399];
    for hash in hashes {
        let str = db1_dehash(hash);
        dbg!(&hash);
        dbg!(str);
    }
}

#[test]
fn test_time() {
    use chrono::prelude::*;
    let local: DateTime<Local> = Local::now();
    println!(
        "year:{} , month: {} , day: {}, week_day:{},hour:{} , min: {} , sec:{}",
        local.year(),
        local.month(),
        local.day(),
        local.weekday(),
        local.hour(),
        local.minute(),
        local.second()
    );
}

/// 将 all_attr_info.bin 文件转成 json
#[test]
#[ignore = "manual converter: requires all_attr_info.bin in the working directory"]
fn test_turn_bin_into_json() {
    let mut file = File::open("all_attr_info.bin").unwrap();
    let mut data = vec![];
    file.read_to_end(&mut data).unwrap();
    let map = bincode::deserialize::<PdmsDatabaseInfo>(&data).unwrap();
    let json = serde_json::to_string(&map).unwrap();
    let mut new_file = File::create("all_attr_info_1.json").unwrap();
    new_file.write_all(&json.into_bytes()).unwrap();
}

#[cfg(test)]
mod tests {
    use aios_core::geometry::ShapeInstancesData;
    use aios_core::options::DbOption;
    use aios_core::pdms_types::{CataHashRefnoKV, RefnoEnum};
    use aios_core::pe::SPdmsElement;
    use aios_database::fast_model::cata_model::gen_cata_geos_with_tracing;
    use dashmap::DashMap;
    use flume::unbounded;
    use glam::Vec3;
    use std::sync::Arc;

    #[tokio::test]
    #[cfg(feature = "profile")]
    async fn test_gen_cata_geos_with_tracing() {
        println!("Starting gen_cata_geos tracing test with profile feature enabled");

        // Initialize sample data for testing
        let db_option = Arc::new(DbOption::default());
        let target_cata_map = Arc::new(DashMap::new());

        // Add some sample data to test with
        // This is just an example - you'll need to replace with real data for your test
        target_cata_map.insert(
            "sample_hash".to_string(),
            CataHashRefnoKV {
                cata_hash: "sample_hash".to_string(),
                group_refnos: vec![RefnoEnum::default()],
                exist_inst: false,
                ptset: None,
            },
        );

        let branch_map = Arc::new(DashMap::new());
        let sjus_map_arc = Arc::new(DashMap::new());

        // Create a channel to receive shape instances data
        let (sender, receiver) = unbounded();

        // Run gen_cata_geos with tracing enabled
        let result = gen_cata_geos_with_tracing(
            db_option,
            target_cata_map,
            branch_map,
            sjus_map_arc,
            sender,
        )
        .await;

        println!("gen_cata_geos result: {:?}", result);

        // For testing purposes, drain the receiver
        while let Ok(_) = receiver.try_recv() {}

        println!("Trace file generated at chrome_trace_cata_model.json");
        println!("You can open this file in Chrome at chrome://tracing");
    }

    #[tokio::test]
    #[cfg(not(feature = "profile"))]
    async fn test_gen_cata_geos_with_tracing() {
        println!("Starting gen_cata_geos tracing test without profile feature");
        println!("Note: For full tracing functionality, enable the 'profile' feature");

        // Initialize minimal test data
        let db_option = Arc::new(DbOption::default());
        let target_cata_map = Arc::new(DashMap::new());
        let branch_map = Arc::new(DashMap::new());
        let sjus_map_arc = Arc::new(DashMap::new());
        let (sender, _) = unbounded();

        // Run gen_cata_geos with tracing disabled
        let result = gen_cata_geos_with_tracing(
            db_option,
            target_cata_map,
            branch_map,
            sjus_map_arc,
            sender,
        )
        .await;

        println!("gen_cata_geos result: {:?}", result);
    }
}
