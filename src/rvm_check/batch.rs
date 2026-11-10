//! `aios-database rvm-check`：给定 E3D 连接信息与库号，自动枚举根、导基准、生成、比对、汇总。
//!
//! 页面单根链路（[`super::execute`]）的批量版，一个命令跑完一个库乃至整个 MDB：
//!
//! 1. 根 = 该库 WORLD 的直接成员里 noun 为 `SITE` / `APPLDW` 的元素（`--site-only` 只取 SITE）；
//!    两类根的件集合互不重叠、合起来才是整库（`.kiro/specs/site-split-rvm-compare`）。
//!    `--unit zone` 时再往下划成比对单元（[`collect_units`]）：每个 ZONE 一个单元，APPLDW 下经
//!    APPLDA、APPDAR 两层才到 ZONE；不在任何 ZONE 里的子节点各成一个兜底单元。单元就是作业，
//!    各自导基准、各自生成即比对。
//! 2. 基准：每个库一次 E3D TTY 会话，把该库全部根逐个 `export`（[`super::export_macro::export_batch_body`]）；
//!    哪个根没落出文件就再单独按 refno / 名称补导一次（页面同一条退路）。`--baseline-dir` 给了
//!    就不碰 E3D，直接取目录里的 `<tag>_<w0>_<w1>.rvm`。
//! 3. 生成：缺省每个根起一个本程序的子进程（`--child`），父进程盯着可提交内存余量与耗时，
//!    越线就杀掉记 `commit-guard` / `timeout`——ams7333 一个根生成要 9 GB，爆在子进程里不连坐整批。
//!    `--in-process` 改回进程内（省一次库打开，但没有隔离）。
//! 4. 比对：进程内 [`super::compare_rvms`]（只读映射 + 并行，几百 MB 以内），判色与页面同一口径。
//!
//! 导出与生成是流水线：E3D 会话在自己的线程里逐库跑，生成工人（`--jobs`）不等它；一个根生成完、
//! 它的基准也到了才比对。生成即比对（`--fused-compare`，`--unit zone` 固定打开）反过来：
//! 先等这个根的基准就位，再边生成边比。产物按 `<out>/<tag>/<w0>_<w1>/` 落，每根一份 `run.json`，
//! 整批一份 `summary.json`；`--resume` 跳过已比对完的根。密码只进 E3D 登录串，不进任何落盘文件。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use aios_core::RefU64;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::{
    FusedCompare, RunInput, RunSpec, RvmCounts, RvmMismatch, RvmVerdict, driver_with_grace,
    evidence_text, export_e3d, export_macro, generate_ours_scoped, prepare_connection, redact,
    resolve_node, tail, verdict_message,
};
use crate::data_interface::direct_tree::DirectTreeService;

/// 跑哪几步。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// 导出 + 生成 + 比对。
    Full,
    /// 只导基准。
    Export,
    /// 只生成。
    Generate,
    /// 只比对（基准与生成物都已在产物目录里）。
    Compare,
}

impl Stage {
    fn exports(self) -> bool {
        matches!(self, Self::Full | Self::Export)
    }
    fn generates(self) -> bool {
        matches!(self, Self::Full | Self::Generate)
    }
    fn compares(self) -> bool {
        matches!(self, Self::Full | Self::Compare)
    }
}

/// 比对单元（`--unit`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    /// WORLD 下的 SITE / APPLDW（现状）。
    Root,
    /// 这些根下的每个 ZONE 各成一个单元（[`collect_units`]）。
    Zone,
}

impl Unit {
    fn is_root(&self) -> bool {
        *self == Self::Root
    }

    /// 控制台与报错里怎么称呼一个作业。
    fn word(self) -> &'static str {
        match self {
            Self::Root => "根",
            Self::Zone => "单元",
        }
    }
}

/// 命令行的 `--unit` / `--fused-compare` / `--emit-rvm` 收成实际的 `(fused_compare, emit_rvm)`：
/// `--unit zone` 固定生成即比对；`--emit-rvm` 不给时，融合就不写 `generated.rvm`，不融合照写。
pub fn fused_and_emit(unit: Unit, fused_compare: bool, emit_rvm: Option<bool>) -> (bool, bool) {
    let fused = fused_compare || unit == Unit::Zone;
    (fused, emit_rvm.unwrap_or(!fused))
}

/// `--scope` 的取值：在所选根里只比哪一块。
///
/// 与 `--only` 分工：`--only` 挑根（进批的作业），`--scope` 挑根里的一块（比对范围）。
/// 两种档都只从选中节点起算，所以落进 vendor 时 `RvmExportOptions.root` 要换成选中节点；
/// 而产物目录与基准文件仍按**所属根**算（[`RootJob::refno`] 不动）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeArg {
    /// 只比这个节点自己，不下探子级。
    Node(String),
    /// 比这个节点的整棵子树（含它自己）。
    Subtree(String),
}

impl ScopeArg {
    /// `node:w0/w1` / `subtree:w0/w1` → 归一成 `w0/w1` 的 [`ScopeArg`]。
    ///
    /// clap 里没有 `key:value` 的先例（`--stage` 是 `ValueEnum`），所以这里自己拆：
    /// 只按第一个 `:` 切，refno 里不会出现冒号。
    pub fn parse(raw: &str) -> Result<Self> {
        let (kind, refno) = raw.split_once(':').with_context(|| {
            format!("--scope 要写成 node:<refno> 或 subtree:<refno>，收到 {raw:?}")
        })?;
        let refno = normalize_ref(refno)
            .with_context(|| format!("--scope 里的 refno 无法解析：{refno}"))?;
        match kind.trim().to_ascii_lowercase().as_str() {
            "node" => Ok(Self::Node(refno)),
            "subtree" => Ok(Self::Subtree(refno)),
            other => bail!("--scope 的档位只认 node / subtree，收到 {other:?}"),
        }
    }

    /// 选中节点的 refno（`w0/w1`）。
    pub fn refno(&self) -> &str {
        match self {
            Self::Node(refno) | Self::Subtree(refno) => refno,
        }
    }

    /// 命令行原样形态，给子进程透传用。
    pub fn to_arg(&self) -> String {
        match self {
            Self::Node(refno) => format!("node:{refno}"),
            Self::Subtree(refno) => format!("subtree:{refno}"),
        }
    }
}

pub struct BatchOptions {
    pub connection: RunInput,
    pub dbnums: Vec<u32>,
    pub all_desi: bool,
    pub only: Vec<String>,
    /// `--scope`：只比所选根里的一块。`None` = 整个根（现状，零介入）。
    pub scope: Option<ScopeArg>,
    pub site_only: bool,
    /// 比对单元：`Root` 是现状；`Zone` 要求 `fused_compare` 打开（见 [`fused_and_emit`]）。
    pub unit: Unit,
    /// 只列出比对单元（写 `<out>/units.json`），不碰 E3D、不生成。
    pub list_units: bool,
    pub out: PathBuf,
    pub resume: bool,
    pub force: bool,
    pub baseline_dir: Option<PathBuf>,
    pub stage: Stage,
    pub in_process: bool,
    pub jobs: usize,
    pub min_commit_free_gb: f64,
    pub gen_timeout: Duration,
    pub e3d_timeout: Duration,
    /// 生成即比对：子进程把成员流直喂 `StreamingCompare`，compare 不再等
    /// `generated.rvm` 落盘。基准要先于生成在盘上，所以每个根先等交接板上它那一格
    /// （`--baseline-dir` 放好，或 E3D 导出完）再开工；缺基准就不生成。只在 `--stage full` 成立。
    pub fused_compare: bool,
    /// 子进程还写不写 `generated.rvm`。false 时产物只有 compare/judgement，
    /// 仅在 `fused_compare` 下成立（没有对拍就没有证据链）。
    pub emit_rvm: bool,
    /// 每个作业跑完就把它的结果与差异写进 `rvm_check_run` / `rvm_check_mismatch`
    /// （[`super::store::StoreWorker`]），不等整批结束；续跑时补上次没落进去的。
    /// 连的是 `DbOption.toml` 指的那台库；`store_mode` 是 mem 时没有可写的持久库，告警后照跑。
    pub store: bool,
    /// 比对内存预算（MB），按根算：只管比对，不含生成占的内存。0 = 不设预算。
    /// 现状：只映射到 vendor 的核对窗口（`FusedCompare.window_bytes`），也就是待判成员
    /// 攒满即并发判一批的那个窗口；按预算切批与超预算拒收都还没实现，要等 vendor 暴露
    /// 摘要大小与记账。所以 0 的当前行为是：核对窗口取 vendor 默认 256MB，负载全在内存。
    pub compare_memory_mb: usize,
    /// 池级元素缓存上限（条）：0 = vendor 默认 200_000。
    /// 大缓存换解析速度、多占驻留；小缓存省内存、满了要重解析。
    pub element_cache: usize,
    /// fused 模式是否落 `compare.json` 全量报告（大件上百 MB）。
    /// false = 结果只入库，盘上留 judgement.json 做进程间运输。
    pub emit_compare: bool,
    /// 生成子进程的 rayon 线程数，0 = 不设上限。
    pub threads: usize,
}

/// `--child`：只为给定的根做生成，产物落在与父进程同一套目录里，别的什么都不写。
pub struct ChildOptions {
    pub connection: RunInput,
    pub dbno: u32,
    pub refnos: Vec<String>,
    /// 父进程的 `--scope`：`refnos` 里只有一个是根，比对范围落在它下面。
    pub scope: Option<ScopeArg>,
    pub out: PathBuf,
    pub gen_timeout: Duration,
    /// 给了则生成即比对：流式对拍的基准 `.rvm`（父进程已放入 `job.dir` 的 `e3d.rvm`）。
    pub compare_baseline: Option<PathBuf>,
    /// 是否写 `generated.rvm`（false = 纯流式对拍模式）。
    pub emit_rvm: bool,
    /// 与父进程同一口径的比对内存预算（0 = 不设预算，vendor 默认核对窗口 256MB）。
    pub compare_memory_mb: usize,
    pub element_cache: usize,
    /// 是否落 `compare.json` 全量报告（false = 结果只入库）。
    pub emit_compare: bool,
    /// 生成子进程的 rayon 线程数，0 = 不设上限。
    pub threads: usize,
}

pub struct BatchOutcome {
    pub summary_path: PathBuf,
    pub clean: bool,
    pub records: Vec<RootRecord>,
    /// 这一批在库里的编号（`rvm_check_run.batch_id`）；没入库时也给，便于对照产物目录。
    pub batch_id: String,
    /// 入库结果：`Some(Ok((根数, 行数)))` / `Some(Err(原因))` / 没开入库时 `None`。
    pub stored: Option<Result<(usize, usize), String>>,
}

/// 库里的批次编号：产物目录最后两级路径（`rvm-check-batch/20261107-full`），续跑同一目录就是同一批。
pub fn batch_id_for(out_root: &Path) -> String {
    let mut parts: Vec<String> = out_root
        .components()
        .rev()
        .take(2)
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    parts.reverse();
    if parts.is_empty() {
        out_root.display().to_string()
    } else {
        parts.join("/")
    }
}

/// 根的终态。`pass` / `empty` 是干净的；`exported` / `generated` 只在对应单步模式下出现。
pub mod status {
    pub const PASS: &str = "pass";
    pub const EMPTY: &str = "empty";
    pub const KIND_DIFFER: &str = "kind_differ";
    pub const FAIL: &str = "fail";
    pub const NO_BASELINE: &str = "no-baseline";
    pub const GEN_FAILED: &str = "gen-failed";
    pub const COMPARE_FAILED: &str = "compare-failed";
    pub const COMMIT_GUARD: &str = "commit-guard";
    pub const TIMEOUT: &str = "timeout";
    pub const EXPORTED: &str = "exported";
    pub const GENERATED: &str = "generated";

    pub fn is_clean(status: &str) -> bool {
        matches!(status, PASS | EMPTY | EXPORTED | GENERATED)
    }

    /// 续跑时可以跳过的终态：比对已经出过结果。
    pub fn is_compared(status: &str) -> bool {
        matches!(status, PASS | EMPTY | KIND_DIFFER | FAIL)
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct StageRecord {
    /// 整数退出码，或 `commit-guard` / `timeout` / `reuse` / `batch` / `refno` / `name` 一类的说明。
    pub exit: serde_json::Value,
    pub seconds: f64,
    #[serde(default)]
    pub peak_private_mb: Option<u64>,
    #[serde(default)]
    pub min_commit_free_gb: Option<f64>,
    #[serde(default)]
    pub message: String,
    pub finished: String,
}

impl StageRecord {
    fn new(
        exit: impl Into<serde_json::Value>,
        started: Instant,
        message: impl Into<String>,
    ) -> Self {
        Self {
            exit: exit.into(),
            seconds: round1(started.elapsed().as_secs_f64()),
            peak_private_mb: None,
            min_commit_free_gb: None,
            message: message.into(),
            finished: chrono::Local::now().to_rfc3339(),
        }
    }
}

/// 每根跑完的内存分项。只填 gen-model 侧能测到的项：生成段峰值、比对段峰值、
/// 生成期间可提交内存余量、产物文件字节。
///
/// 待比窗口峰值、负载字节、元素缓存条数/清空次数、目录网格缓存条数都还没测——那些要等 vendor
/// 先在流式对拍与池级缓存上暴露计数，这里故意不留一堆恒为 `None` 的空字段。
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct MemoryBreakdown {
    /// 生成子进程整段累计峰值（`PeakPagefileUsage`），与 `gen.peak_private_mb` 同一口径。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gen_peak_private_mb: Option<u64>,
    /// 比对段峰值（按时刻采样的当前提交量取 max）。读文件比对、或阶段标记没落盘时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_peak_private_mb: Option<u64>,
    /// 生成期间可提交内存余量的最小值（GB）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_commit_free_gb: Option<f64>,
    /// 产物文件字节合计：`ours/generated.rvm`、`compare.json`、`judgement.json`、
    /// `ours/report.json`、`ours/elements.json`。一个都没落就是 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_bytes: Option<u64>,
}

impl MemoryBreakdown {
    /// 从已经落定的三段记录里拼内存分项。拿不到的项留 `None`，绝不拿父进程自己的内存冒充。
    fn collect(record: &RootRecord) -> Self {
        let dir = Path::new(&record.dir);
        let mut product_bytes: Option<u64> = None;
        for path in [
            dir.join("ours").join("generated.rvm"),
            dir.join("compare.json"),
            dir.join("judgement.json"),
            dir.join("ours").join("report.json"),
            dir.join("ours").join("elements.json"),
        ] {
            if let Ok(meta) = std::fs::metadata(&path) {
                *product_bytes.get_or_insert(0) += meta.len();
            }
        }
        Self {
            gen_peak_private_mb: record.generate.as_ref().and_then(|g| g.peak_private_mb),
            compare_peak_private_mb: record.compare.as_ref().and_then(|c| c.peak_private_mb),
            min_commit_free_gb: record.generate.as_ref().and_then(|g| g.min_commit_free_gb),
            product_bytes,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RootRecord {
    pub dbno: u32,
    pub tag: String,
    #[serde(rename = "ref")]
    pub refno: String,
    pub noun: String,
    pub name: String,
    /// 单元种类。`root`（现状）不落进 `run.json`。
    #[serde(default, skip_serializing_if = "UnitKind::is_root")]
    pub unit: UnitKind,
    /// `--unit zone` 时单元所在的上级根。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentRoot>,
    pub dir: String,
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub resumed: bool,
    #[serde(default)]
    pub export: Option<StageRecord>,
    #[serde(default, rename = "gen")]
    pub generate: Option<StageRecord>,
    #[serde(default)]
    pub compare: Option<StageRecord>,
    #[serde(default)]
    pub counts: Option<RvmCounts>,
    #[serde(default)]
    pub mismatches: Vec<RvmMismatch>,
    #[serde(default)]
    pub mismatches_truncated: bool,
    #[serde(default)]
    pub centreline_only_baseline: usize,
    /// 每根跑完的内存分项。现在只填 gen-model 侧能测到的项；
    /// 待比窗口峰值、负载字节、元素缓存条数/清空次数、目录网格缓存条数要等 vendor 先暴露。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryBreakdown>,
    /// 不截断的差异清单，只给入库用（[`super::store`]）；不进 `run.json`。
    #[serde(skip)]
    pub mismatches_all: Vec<RvmMismatch>,
    /// 只画中心线的基准成员名，同样只给入库用。
    #[serde(skip)]
    pub centreline_names: Vec<String>,
    /// 这次只核对了作业里的一块时，`--scope` 的原样写法（`node:w0/w1` / `subtree:w0/w1`）。
    /// 构件行按它决定重新核对后清哪一片陈旧行；续跑补入库也要它，所以进 `run.json`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// 构件级结果的条数（作业目录里 `element-checks.json` 那一份）；没有比对出结果时没有这一格。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_checks: Option<usize>,
    /// 构件级结果，只给入库用（落 `rvm_check_element`）；交给入库线程之后就放掉，不进 `run.json`。
    #[serde(skip)]
    pub elements: Option<Vec<super::element::ElementCheck>>,
}

/// 一个作业是哪种比对单元。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnitKind {
    /// WORLD 下的 SITE / APPLDW（`--unit root`）。
    #[default]
    Root,
    /// `--unit zone` 下的一个 ZONE。
    Zone,
    /// `--unit zone` 下不在任何 ZONE 里的子节点，各成一个兜底单元。
    Fallback,
}

impl UnitKind {
    fn is_root(&self) -> bool {
        *self == Self::Root
    }

    /// 库里 `rvm_check_run.unit` 的取值，与 `run.json` 里的写法相同。
    fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Zone => "zone",
            Self::Fallback => "fallback",
        }
    }
}

/// 单元所在的上级根（WORLD 下的 SITE / APPLDW）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ParentRoot {
    /// `=w0/w1`。
    #[serde(rename = "ref")]
    pub refno: String,
    pub noun: String,
    pub name: String,
}

/// 枚举出来的一个作业：`--unit root` 时是根，`--unit zone` 时是单元。
#[derive(Clone)]
struct RootJob {
    dbno: u32,
    tag: String,
    /// `w0/w1`。
    refno: String,
    noun: String,
    name: String,
    dir: PathBuf,
    unit_kind: UnitKind,
    /// `--unit zone` 时的上级根；根作业为 `None`。
    parent: Option<ParentRoot>,
}

impl RootJob {
    fn display_ref(&self) -> String {
        format!("={}", self.refno)
    }
    fn stem(&self) -> String {
        self.refno.replace('/', "_")
    }
}

#[derive(Clone)]
enum ExportOutcome {
    Ready(StageRecord),
    Missing(StageRecord),
}

impl ExportOutcome {
    fn record(&self) -> &StageRecord {
        match self {
            Self::Ready(record) | Self::Missing(record) => record,
        }
    }
}

/// 导出线程 → 生成工人的交接板：每个根一格，填上就叫醒等它的工人。
struct ExportBoard {
    slots: Mutex<Vec<Option<ExportOutcome>>>,
    ready: Condvar,
}

impl ExportBoard {
    fn new(len: usize) -> Self {
        Self {
            slots: Mutex::new(vec![None; len]),
            ready: Condvar::new(),
        }
    }

    fn set(&self, index: usize, outcome: ExportOutcome) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slots[index] = Some(outcome);
        self.ready.notify_all();
    }

    fn wait(&self, index: usize) -> ExportOutcome {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(outcome) = &slots[index] {
                return outcome.clone();
            }
            slots = self
                .ready
                .wait(slots)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// 导出线程退出时把没填的格全部填成「缺基准」，工人不会挂死。
    fn fill_missing(&self, message: &str) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for slot in slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(ExportOutcome::Missing(StageRecord::new(
                    "not-exported",
                    Instant::now(),
                    message,
                )));
            }
        }
        self.ready.notify_all();
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `=w0/w1` / `w0/w1` / `w0_w1` → `w0/w1`。
pub fn normalize_ref(raw: &str) -> Option<String> {
    let refno = RefU64::from_str(raw.trim()).ok()?;
    (refno.0 != 0).then(|| format!("{}/{}", refno.get_0(), refno.get_1()))
}

fn tag_of(e3d_project: &str, dbno: u32) -> String {
    format!("{}{}", e3d_project.to_ascii_lowercase(), dbno)
}

fn root_dir(out: &Path, tag: &str, refno: &str) -> PathBuf {
    out.join(tag).join(refno.replace('/', "_"))
}

pub(crate) fn ensure_db_option_present() -> Result<()> {
    let cwd = std::env::current_dir().context("取当前目录")?;
    let name = std::env::var("DB_OPTION_FILE").unwrap_or_else(|_| "DbOption".to_string());
    let candidates = [cwd.join(format!("{name}.toml")), cwd.join(&name)];
    ensure!(
        candidates.iter().any(|path| path.is_file()),
        "当前目录 {} 下没有 {name}.toml：rvm-check 和服务一样按工作目录读配置（项目目录、MDB、schema），请在部署目录或仓库根下运行",
        cwd.display()
    );
    Ok(())
}

/// 一个库的根：WORLD 直接成员里的 SITE（与 APPLDW）。
fn enumerate_roots(
    tree: &DirectTreeService,
    dbno: u32,
    site_only: bool,
    e3d_project: &str,
    out: &Path,
) -> Result<Vec<RootJob>> {
    let metadata = tree.metadata()?;
    let database = metadata
        .databases()
        .iter()
        .find(|db| db.dbnum == dbno)
        .with_context(|| format!("库 {dbno} 不在当前 MDB 里（或没有文件）"))?;
    ensure!(
        database.db_type.eq_ignore_ascii_case("DESI"),
        "库 {dbno} 是 {}，不是 DESI 设计库",
        database.db_type
    );
    let world = metadata
        .world_of(dbno)
        .with_context(|| format!("库 {dbno} 读不到 WORLD"))?;
    let tag = tag_of(e3d_project, dbno);
    let mut roots = Vec::new();
    for node in tree.children(world)? {
        let noun = node.noun.trim().to_ascii_uppercase();
        let keep = noun == "SITE" || (!site_only && noun == "APPLDW");
        if !keep {
            continue;
        }
        let refno = node.refno.refno();
        let refno = format!("{}/{}", refno.get_0(), refno.get_1());
        roots.push(RootJob {
            dbno,
            tag: tag.clone(),
            dir: root_dir(out, &tag, &refno),
            refno,
            noun,
            name: node.name.trim().to_string(),
            unit_kind: UnitKind::Root,
            parent: None,
        });
    }
    Ok(roots)
}

/// 划分比对单元时继续往下钻的纯容器：上级根本身（SITE / APPLDW），以及 APPLDW 到 ZONE 之间的两层。
/// 页面单根把构件行归到单元名下时也按它认单元（`super::locate_unit`）。
pub(super) const UNIT_CONTAINER_NOUNS: &[&str] = &["SITE", "APPLDW", "APPLDA", "APPDAR"];

/// 容器最多嵌几层。实际最深是 APPLDW → APPLDA → APPDAR 两层，再深就是数据不对。
const UNIT_CONTAINER_DEPTH: usize = 8;

/// 划分单元只要的三样：refno、大写名词、名字。
#[derive(Clone, Debug, PartialEq, Eq)]
struct TreeEntry {
    refno: RefU64,
    noun: String,
    name: String,
}

/// 从上级根往下划分比对单元：纯容器（[`UNIT_CONTAINER_NOUNS`]）继续下钻，其余每个子节点
/// 成一个单元——常态是 ZONE，别的名词各成一个兜底单元。按成员表原序深度优先，所以
/// 上级根下的每个非容器节点恰好落进一个单元。`children` 给出一个容器的直接成员。
fn collect_units(
    parent: RefU64,
    children: &mut impl FnMut(RefU64) -> Result<Vec<TreeEntry>>,
) -> Result<Vec<TreeEntry>> {
    fn walk(
        container: RefU64,
        depth: usize,
        children: &mut impl FnMut(RefU64) -> Result<Vec<TreeEntry>>,
        units: &mut Vec<TreeEntry>,
    ) -> Result<()> {
        ensure!(
            depth < UNIT_CONTAINER_DEPTH,
            "={container} 下的容器嵌套超过 {UNIT_CONTAINER_DEPTH} 层"
        );
        for entry in children(container)? {
            if UNIT_CONTAINER_NOUNS.contains(&entry.noun.as_str()) {
                walk(entry.refno, depth + 1, children, units)?;
            } else {
                units.push(entry);
            }
        }
        Ok(())
    }
    let mut units = Vec::new();
    walk(parent, 0, children, &mut units)?;
    Ok(units)
}

/// `--unit zone` 的作业：把每个上级根（[`enumerate_roots`] 的结果）按 [`collect_units`] 划成单元。
/// 单元的产物目录与基准文件按单元自己的 refno 走（[`root_dir`]），与根作业同一套命名。
fn units_of(tree: &DirectTreeService, roots: &[RootJob], out: &Path) -> Result<Vec<RootJob>> {
    let mut units = Vec::new();
    for root in roots {
        let start = RefU64::from_str(&root.refno)
            .map_err(|error| anyhow::anyhow!("refno 无法解析：{}（{error}）", root.refno))?;
        let parent = ParentRoot {
            refno: root.display_ref(),
            noun: root.noun.clone(),
            name: root.name.clone(),
        };
        let mut children = |container: RefU64| -> Result<Vec<TreeEntry>> {
            Ok(tree
                .children(container)?
                .into_iter()
                .map(|node| TreeEntry {
                    refno: node.refno.refno(),
                    noun: node.noun.trim().to_ascii_uppercase(),
                    name: node.name.trim().to_string(),
                })
                .collect())
        };
        for entry in collect_units(start, &mut children)? {
            let refno = format!("{}/{}", entry.refno.get_0(), entry.refno.get_1());
            units.push(RootJob {
                dbno: root.dbno,
                tag: root.tag.clone(),
                dir: root_dir(out, &root.tag, &refno),
                refno,
                unit_kind: if entry.noun == "ZONE" {
                    UnitKind::Zone
                } else {
                    UnitKind::Fallback
                },
                noun: entry.noun,
                name: entry.name,
                parent: Some(parent.clone()),
            });
        }
    }
    Ok(units)
}

/// `--only` 选中了这个作业：点名的是它自己，或（`--unit zone` 时）点名的是它的上级根。
/// `wanted` 已归一成 `w0/w1`。
fn only_keeps(job: &RootJob, wanted: &[String]) -> bool {
    wanted.iter().any(|refno| {
        *refno == job.refno
            || job
                .parent
                .as_ref()
                .is_some_and(|parent| parent.refno.trim_start_matches('=') == refno)
    })
}

/// 参数组合的硬约束，在碰库与 E3D 之前查完。
fn check_modes(opts: &BatchOptions) -> Result<()> {
    if opts.unit == Unit::Zone {
        ensure!(
            opts.fused_compare,
            "--unit zone 固定走生成即比对，调用方要打开 fused_compare（见 fused_and_emit）"
        );
        ensure!(
            matches!(opts.stage, Stage::Full | Stage::Export),
            "--unit zone 只支持 --stage full（导出 + 生成即比对）或 --stage export（只导基准）"
        );
    } else if opts.fused_compare {
        ensure!(
            opts.stage.generates() && opts.stage.compares(),
            "--fused-compare 只在 生成+比对 同跑的阶段成立"
        );
    }
    ensure!(
        opts.emit_rvm || opts.fused_compare,
        "--emit-rvm=false 需要 --fused-compare（没有对拍就没有证据链）"
    );
    Ok(())
}

/// 生成即比对的基准：交接板上这一格是 `Ready` 且文件在盘上才开工。拿不到就返回缺基准的原因，
/// 调用方记 `no-baseline`、不生成——不退回「先写 generated.rvm、再按文件比」。
fn fused_from(
    outcome: &ExportOutcome,
    job: &RootJob,
    opts: &BatchOptions,
) -> std::result::Result<FusedCompare, String> {
    match outcome {
        ExportOutcome::Missing(export) if export.message.is_empty() => {
            Err("没有基准，生成即比对不开工".to_string())
        }
        ExportOutcome::Missing(export) => Err(export.message.clone()),
        ExportOutcome::Ready(_) => {
            let baseline = job.dir.join("e3d.rvm");
            if !baseline.is_file() {
                return Err(format!("基准记为就位，但盘上没有 {}", baseline.display()));
            }
            Ok(FusedCompare {
                baseline,
                emit_rvm: opts.emit_rvm,
                window_bytes: opts.compare_memory_mb * 1024 * 1024,
                emit_compare: opts.emit_compare,
            })
        }
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path).with_context(|| format!("建 {}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.flush()?;
    Ok(())
}

fn read_record(path: &Path) -> Option<RootRecord> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 续跑跳过的作业补入库用的事实。`run.json` 里的差异清单最多 400 条，只画中心线的成员只记了个数；
/// 入库要全量，从作业目录里那份完整的判定取（[`full_judgement`]），取不到才退回 `run.json` 里的。
///
/// 构件级结果同样从作业目录取（`element-checks.json`）。加那份文件之前跑的老产物目录里没有它，这种
/// 作业补的只有运行行与差异行，构件行补不了（要重跑）。
fn resumed_facts(record: &RootRecord, batch_id: &str, spec: &RunSpec) -> super::store::RunFacts {
    let mut record = record.clone();
    if let Some(judgement) = full_judgement(&record) {
        record.mismatches_all = judgement.mismatches;
        record.centreline_names = judgement.centreline_only_baseline;
    }
    record.elements = match super::element::read_checks(Path::new(&record.dir)) {
        Ok(checks) => checks,
        Err(message) => {
            eprintln!(
                "警告：{} {} 的构件级结果读不回来，这次不补构件行：{message}",
                record.tag, record.refno
            );
            None
        }
    };
    record.take_run_facts(batch_id, spec)
}

/// 作业目录里不截断的那份判定。生成即比对留了 `judgement.json`，直接读；读文件比对的作业只有
/// `compare.json`，`run.json` 里的清单不全（截断过，或有只记了个数的中心线成员）时才重判一遍——
/// 大根的 `compare.json` 上百 MB，清单本来就全的不去读它。
fn full_judgement(record: &RootRecord) -> Option<super::report::Judgement> {
    let dir = Path::new(&record.dir);
    if let Ok(bytes) = std::fs::read(dir.join("judgement.json")) {
        return serde_json::from_slice(&bytes).ok();
    }
    if !record.mismatches_truncated && record.centreline_only_baseline == 0 {
        return None;
    }
    let bytes = std::fs::read(dir.join("compare.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(super::report::judge_compare_full(&value))
}

fn dir_has_entries(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

/// 绝对路径，不带 Windows `canonicalize` 加的 `\\?\` 前缀——那个前缀会原样进 E3D 宏与报告。
fn absolute_dir(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let text = absolute.display().to_string();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!(r"\\{rest}")),
        None => PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)),
    }
}

/// 子进程模式：生成给定的根，不写 run.json，不汇总。失败以非零退出码和 stderr 告知父进程。
pub fn run_child(opts: &ChildOptions) -> Result<()> {
    ensure_db_option_present()?;
    let mut spec = prepare_connection(&opts.connection, false).map_err(anyhow::Error::msg)?;
    spec.tools.timeout = opts.gen_timeout;
    let tree = DirectTreeService::open(&spec.project_name, &spec.e3d_mdb)?;
    let tag = tag_of(&spec.e3d_project, opts.dbno);
    for raw in &opts.refnos {
        let refno = normalize_ref(raw).with_context(|| format!("refno 无法解析：{raw}"))?;
        let dir = root_dir(&opts.out, &tag, &refno);
        let mut spec = spec.clone();
        spec.refno = refno.clone();
        let node = resolve_node(&tree, &spec).map_err(anyhow::Error::msg)?;
        ensure!(
            node.dbnum == opts.dbno,
            "={refno} 属于库 {}，不是 {}",
            node.dbnum,
            opts.dbno
        );
        let fused = opts.compare_baseline.as_ref().map(|baseline| FusedCompare {
            baseline: baseline.clone(),
            emit_rvm: opts.emit_rvm,
            window_bytes: opts.compare_memory_mb * 1024 * 1024,
            emit_compare: opts.emit_compare,
        });
        generate_ours_scoped(
            &spec,
            &node,
            &dir,
            fused.as_ref(),
            opts.element_cache,
            opts.scope.as_ref(),
        )
        .map_err(anyhow::Error::msg)?;
        println!("generated ={refno} -> {}", dir.join("ours").display());
    }
    Ok(())
}

/// `--scope` 的选中节点属于哪个根：沿 OWNER 链上溯，取第一个出现在 `roots` 里的节点。
///
/// 根是 WORLD 的直接成员（SITE / APPLDW），所以「第一个落在根表里的祖先」就是所属根；
/// 一个都找不到说明这个 refno 压根不在这批根的树里。
fn scope_root_of(
    tree: &DirectTreeService,
    scope_refno: &str,
    roots: &[RootJob],
) -> Result<Option<usize>> {
    let start = RefU64::from_str(scope_refno)
        .map_err(|error| anyhow::anyhow!("refno 无法解析：{scope_refno}（{error}）"))?;
    let chain = tree
        .ancestors(start)
        .with_context(|| format!("上溯 ={scope_refno} 的属主链失败"))?;
    for node in chain {
        let refno = node.refno();
        let text = format!("{}/{}", refno.get_0(), refno.get_1());
        if let Some(index) = roots.iter().position(|job| job.refno == text) {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

/// 整批。返回每根记录与 `summary.json` 的路径；`clean` 为真表示每个根都是 `pass` / `empty`。
pub fn run(opts: &BatchOptions) -> Result<BatchOutcome> {
    ensure_db_option_present()?;
    ensure!(opts.jobs >= 1, "--jobs 至少为 1");
    ensure!(
        opts.min_commit_free_gb > 0.0,
        "--min-commit-free-gb 要大于 0"
    );
    check_modes(opts)?;
    let need_e3d = !opts.list_units && opts.stage.exports() && opts.baseline_dir.is_none();
    let spec = prepare_connection(&opts.connection, need_e3d).map_err(anyhow::Error::msg)?;
    if need_e3d {
        ensure!(
            spec.tools.launcher_found,
            "找不到 E3D 启动脚本（试过 {}）；用 DbOption.toml 的 [rvm_check] e3d_launcher 指定",
            spec.tools.launcher.display()
        );
        ensure!(
            spec.install_dir.join("des.exe").is_file(),
            "E3D 安装目录里没有 des.exe：{}",
            spec.install_dir.display()
        );
    }
    if let Some(dir) = &opts.baseline_dir {
        ensure!(dir.is_dir(), "基准目录不存在：{}", dir.display());
    }

    let tree = DirectTreeService::open(&spec.project_name, &spec.e3d_mdb)
        .with_context(|| format!("打开 MDB {} 的模型树", spec.e3d_mdb))?;
    let mut dbnums: Vec<u32> = if opts.all_desi {
        tree.metadata()?
            .databases()
            .iter()
            .filter(|db| db.db_type.eq_ignore_ascii_case("DESI"))
            .map(|db| db.dbnum)
            .collect()
    } else {
        opts.dbnums.clone()
    };
    dbnums.dedup();
    ensure!(
        !dbnums.is_empty(),
        "没有要跑的库：给 --dbnum，或用 --all-desi"
    );

    // 产物目录一律绝对路径：E3D 宏里的 `export file` 与子进程都按它找文件。
    let out_root = absolute_dir(&opts.out);
    let mut roots = Vec::new();
    for &dbno in &dbnums {
        roots.extend(enumerate_roots(
            &tree,
            dbno,
            opts.site_only,
            &spec.e3d_project,
            &out_root,
        )?);
    }
    let mut jobs = match opts.unit {
        Unit::Root => roots.clone(),
        Unit::Zone => units_of(&tree, &roots, &out_root)?,
    };
    let mut wanted = Vec::new();
    if !opts.only.is_empty() {
        for raw in &opts.only {
            wanted.push(
                normalize_ref(raw).with_context(|| format!("--only 里的 refno 无法解析：{raw}"))?,
            );
        }
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|refno| {
                !jobs
                    .iter()
                    .any(|job| only_keeps(job, std::slice::from_ref(*refno)))
            })
            .collect();
        if !missing.is_empty() {
            eprintln!(
                "警告：--only 里的 {} 不是{}，忽略",
                missing
                    .iter()
                    .map(|refno| format!("={refno}"))
                    .collect::<Vec<_>>()
                    .join("、"),
                match opts.unit {
                    Unit::Root => "所选库 WORLD 下的 SITE / APPLDW",
                    Unit::Zone =>
                        "所选库的比对单元（ZONE / 兜底单元），也不是它们的上级 SITE / APPLDW",
                }
            );
        }
        jobs.retain(|job| only_keeps(job, &wanted));
    }
    if opts.list_units {
        return list_units(&tree, opts, &dbnums, &roots, &jobs, &wanted, &out_root);
    }
    ensure!(
        !jobs.is_empty(),
        "{}",
        match opts.unit {
            Unit::Root => "所选库的 WORLD 下没有 SITE / APPLDW 根",
            Unit::Zone => "所选库下没有比对单元（SITE / APPLDW 下没有 ZONE，也没有兜底单元）",
        }
    );

    let word = opts.unit.word();
    // `--scope` 与 `--only` 的冲突：范围指的节点必须落在本批已选的根里。
    // 不静默忽略、不自动把它的根加回作业列表（那会悄悄多跑一个根）。
    if let Some(scope) = &opts.scope {
        let Some(index) = scope_root_of(&tree, scope.refno(), &jobs)? else {
            let root_list: Vec<String> = jobs
                .iter()
                .map(|job| format!("{} {} {}", job.display_ref(), job.noun, job.name))
                .collect();
            let chain = tree
                .ancestors(RefU64::from_str(scope.refno()).map_err(|error| {
                    anyhow::anyhow!("refno 无法解析：{}（{error}）", scope.refno())
                })?)
                .map(|chain| {
                    chain
                        .into_iter()
                        .map(|node| node.refno().to_string())
                        .collect::<Vec<_>>()
                        .join(" ← ")
                })
                .unwrap_or_else(|error| format!("（读不到：{error}）"));
            bail!(
                "--scope 与 --only 冲突：选中节点 ={} 不属于本批的任何{word}。\n\
                 --scope 的 OWNER 链（自身 → 属主）：{chain}\n\
                 本批的{word}（{}）：{}",
                scope.refno(),
                match opts.unit {
                    Unit::Root => "--only 只认 WORLD 下的 SITE / APPLDW",
                    Unit::Zone => "--unit zone：ZONE / 兜底单元",
                },
                if root_list.is_empty() {
                    "（空）".to_string()
                } else {
                    root_list.join("、")
                }
            );
        };
        if jobs.len() > 1 {
            // 一份范围只对应一个根：多根并跑会让其余根按「整个根」比对，把范围外成员
            // 全记成 `only_baseline` 混进同一份汇总。这里直接拒，让调用方用 --only 收窄。
            let root = &jobs[index];
            bail!(
                "--scope {} 属于{word} {} {}，但本批有 {} 个{word}：加 --only {} 收窄到这一个{word}",
                scope.to_arg(),
                root.display_ref(),
                root.noun,
                jobs.len(),
                root.display_ref()
            );
        }
        let kept = jobs.remove(index);
        // 选中节点必须在根所在的那个库里：`export_rvm` 只把 `db_file` 那一只库当主库，
        // 跨库的选中节点在 DFS 里根本不会出现（不是「比空」，是「找不到起点」）。
        scope_same_db(&tree, scope, kept.dbno)?;
        jobs.clear();
        jobs.push(kept);
    }

    std::fs::create_dir_all(&out_root).with_context(|| format!("建 {}", out_root.display()))?;
    if !opts.resume && !opts.force && dir_has_entries(&out_root) {
        bail!(
            "产物目录 {} 非空：加 --resume 续跑（跳过已比对的根），或 --force 覆盖",
            out_root.display()
        );
    }

    // 续跑：已比对出结果的根原样收进汇总，不再跑。
    let mut resumed: Vec<Option<RootRecord>> = vec![None; jobs.len()];
    if opts.resume {
        for (index, job) in jobs.iter().enumerate() {
            if let Some(mut record) = read_record(&job.dir.join("run.json"))
                && status::is_compared(&record.status)
                && (job.dir.join("judgement.json").is_file()
                    || job.dir.join("compare.json").is_file())
            {
                record.resumed = true;
                resumed[index] = Some(record);
            }
        }
    }

    let baseline_source = match &opts.baseline_dir {
        Some(dir) => format!("目录 {}", dir.display()),
        None if opts.stage.exports() => format!(
            "E3D TTY（{} {} {}，每库一次会话）",
            spec.e3d_project, spec.e3d_mdb, spec.username
        ),
        None => "产物目录里已有的 e3d.rvm".to_string(),
    };
    println!(
        "rvm-check：{} 个库、{} 个{word}（{} 个续跑跳过），阶段 {:?}，生成 {}（jobs {}），基准 {}，产物 {}",
        dbnums.len(),
        jobs.len(),
        resumed.iter().filter(|record| record.is_some()).count(),
        opts.stage,
        if opts.in_process {
            "进程内"
        } else {
            "子进程"
        },
        opts.jobs,
        baseline_source,
        out_root.display()
    );
    // 比对内存台账：每根预算 × 并发根数（`--jobs`）= 总量。预算 0 时没有上限可乘，
    // 只说清当前行为（预算只映射到 vendor 的核对窗口，按预算切批与超预算拒收还没实现）。
    if opts.compare_memory_mb == 0 {
        println!(
            "比对内存：每根不设预算（核对窗口 256MB，负载全在内存），jobs {} → 总量不设上限",
            opts.jobs
        );
    } else {
        println!(
            "比对内存：每根 {} MB，jobs {} → 总量 {} MB",
            opts.compare_memory_mb,
            opts.jobs,
            opts.compare_memory_mb.saturating_mul(opts.jobs)
        );
    }
    for job in &jobs {
        println!(
            "  {} {} {} {}{}",
            job.tag,
            job.display_ref(),
            job.noun,
            job.name,
            job.parent
                .as_ref()
                .map(|parent| format!("  ← {} {}", parent.refno, parent.noun))
                .unwrap_or_default()
        );
    }

    let batch_id = batch_id_for(&out_root);
    // 入库线程：一个作业跑完就落一个，不等整批结束。连不上只告警——产物目录里的 JSON 仍是完整的一份。
    let store = opts.store.then(|| {
        let (worker, stored) = super::store::StoreWorker::start(&batch_id);
        match stored {
            Ok(stored) => {
                // 续跑跳过的作业：上次没落进库的（中途断了、当时库连不上）在这里补上，已经在库里的不动。
                let missing: Vec<&RootRecord> = resumed
                    .iter()
                    .flatten()
                    .filter(|record| !stored.contains(&record.run_id(&batch_id)))
                    .collect();
                if !missing.is_empty() {
                    println!(
                        "入库：续跑跳过的{word}里有 {} 个库里还没有，补进去",
                        missing.len()
                    );
                }
                for record in missing {
                    worker.submit(resumed_facts(record, &batch_id, &spec));
                }
            }
            Err(error) => eprintln!(
                "警告：结果入不了库（{error}）。这一批照跑，产物目录里的 JSON 仍完整；\
                 库通了以后加 --resume 重跑一遍即可补进去，或加 --no-store 关掉这一步"
            ),
        }
        worker
    });

    let board = ExportBoard::new(jobs.len());
    let results: Mutex<Vec<Option<RootRecord>>> = Mutex::new(resumed.clone());
    let next = AtomicUsize::new(0);
    let in_process_gate = Mutex::new(());
    let pending: Vec<usize> = (0..jobs.len())
        .filter(|index| resumed[*index].is_none())
        .collect();

    std::thread::scope(|scope| {
        // 基准这一路：导出线程，或从目录取，或看产物目录里有没有。
        if opts.stage.exports() && opts.baseline_dir.is_none() {
            let board = &board;
            let jobs = &jobs;
            let spec = &spec;
            let out_root = &out_root;
            let pending = &pending;
            scope.spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    export_all(jobs, pending, spec, out_root, opts.e3d_timeout, board)
                }));
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => board.fill_missing(&format!("导出线程中止：{error:#}")),
                    Err(panic) => board
                        .fill_missing(&format!("导出线程 panic：{}", super::panic_text(&*panic))),
                }
            });
        } else {
            for &index in &pending {
                let job = &jobs[index];
                let started = Instant::now();
                let outcome = match &opts.baseline_dir {
                    Some(dir) => place_baseline_from_dir(dir, job, started),
                    None => {
                        let rvm = job.dir.join("e3d.rvm");
                        if std::fs::metadata(&rvm).is_ok_and(|meta| meta.len() > 32) {
                            ExportOutcome::Ready(StageRecord::new("reuse", started, ""))
                        } else {
                            ExportOutcome::Missing(StageRecord::new(
                                "missing",
                                started,
                                format!("产物目录里没有 {}", rvm.display()),
                            ))
                        }
                    }
                };
                board.set(index, outcome);
            }
        }

        // 工人：生成（子进程 / 进程内）→ 等基准 → 比对 → run.json。
        let workers = if opts.stage == Stage::Export {
            1
        } else {
            opts.jobs.min(pending.len().max(1))
        };
        for _ in 0..workers {
            let board = &board;
            let jobs = &jobs;
            let spec = &spec;
            let tree = &tree;
            let out_root = &out_root;
            let results = &results;
            let next = &next;
            let pending = &pending;
            let in_process_gate = &in_process_gate;
            let store = store.as_ref();
            let batch_id = &batch_id;
            scope.spawn(move || {
                loop {
                    let slot = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&index) = pending.get(slot) else {
                        break;
                    };
                    let job = &jobs[index];
                    let mut record = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        process_root(
                            job,
                            index,
                            opts,
                            spec,
                            tree,
                            out_root,
                            board,
                            in_process_gate,
                        )
                    }))
                    .unwrap_or_else(|panic| {
                        let mut record = RootRecord::new(job);
                        record.status = status::COMPARE_FAILED.to_string();
                        record.message = format!("处理根时 panic：{}", super::panic_text(&*panic));
                        record
                    });
                    // 内存分项统一在这里拼：产物都已落盘，字节数才是这一轮的真实值。
                    record.memory = Some(MemoryBreakdown::collect(&record));
                    if let Err(error) = write_json(&job.dir.join("run.json"), &record) {
                        eprintln!("写 {} 失败：{error:#}", job.dir.join("run.json").display());
                    }
                    println!("{}", record.console_line());
                    if let Some(store) = store {
                        // 构件级结果整份搬给入库线程（不复制）。
                        store.submit(record.take_run_facts(batch_id, spec));
                    }
                    // 不截断的清单与构件级结果只为入库留着，交出去就放掉：按 ZONE 跑一批上千个单元，
                    // 不攒到整批结束（`--no-store` 时同样不留）。
                    record.mismatches_all = Vec::new();
                    record.centreline_names = Vec::new();
                    record.elements = None;
                    results
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())[index] = Some(record);
                }
            });
        }
    });

    let records: Vec<RootRecord> = results
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            record.unwrap_or_else(|| {
                let mut record = RootRecord::new(&jobs[index]);
                record.status = status::COMPARE_FAILED.to_string();
                record.message = "没有跑到这个根".to_string();
                if let Some(store) = &store {
                    store.submit(record.run_facts(&batch_id, &spec));
                }
                record
            })
        })
        .collect();

    let summary = Summary::build(
        opts,
        &spec,
        &dbnums,
        &baseline_source,
        &batch_id,
        records.clone(),
    );
    let summary_path = out_root.join("summary.json");
    write_json(&summary_path, &summary)?;
    let table = summary.table();
    std::fs::write(out_root.join("summary.txt"), &table)?;
    println!("\n{table}");
    println!("summary.json: {}", summary_path.display());

    // 入库线程收尾：等已经交出去的都写完，再报这一批的账。
    let stored = store.map(|worker| {
        let draining = Instant::now();
        let finished = worker.finish();
        let (elements, stale_elements) = (finished.elements, finished.stale_elements);
        if finished.elements_superseded > 0 {
            println!(
                "入库：{} 个{word}在库里已经有更晚的一次核对，这次只补了运行行与差异行，构件行没有动",
                finished.elements_superseded
            );
        }
        if finished.runs > 0 {
            // 入库线程与生成并行：写库合计不是整批多出来的墙钟，收尾等的那一段才是。
            println!(
                "入库用时：写库合计 {:.1} 秒（其中构件行 {:.1} 秒，{elements} 行）；全部{word}跑完之后又等了 {:.1} 秒写完",
                finished.millis as f64 / 1000.0,
                finished.element_millis as f64 / 1000.0,
                draining.elapsed().as_secs_f64()
            );
        }
        let outcome = finished.into_result();
        match &outcome {
            Ok((0, _)) => println!("入库：batch_id {batch_id} 这次没有新结果要写，库里已经是全的"),
            Ok((runs, rows)) => println!(
                "结果已入库 {}：batch_id {batch_id}，{runs} 个{word}、{rows} 行差异、{elements} 行构件\
                 （另清掉 {stale_elements} 行陈旧的构件行） → rvm_check_run / rvm_check_mismatch / rvm_check_element\
                 （查询：aios-database rvm-check-query rollup --batch-id {batch_id}；构件级：\
                 rvm-check-query elements --refno <构件 refno>、element-grades --batch-id {batch_id}；服务开着时还有 \
                 GET /api/v1/rvm-check/rollup?batch_id=…、/history、/mismatches、/reasons、/elements、/element-grades）",
                aios_core::get_db_option().get_version_db_conn_str()
            ),
            Err(error) => eprintln!(
                "结果没有全部入库（产物目录里的 JSON 仍完整；库通了以后加 --resume 重跑一遍可补进去，\
                 加 --no-store 可关掉这一步）：{error}"
            ),
        }
        outcome
    });
    Ok(BatchOutcome {
        summary_path,
        clean: summary.clean,
        records,
        batch_id,
        stored,
    })
}

/// `<out>/units.json`：`--list-units` 的普查结果。
#[derive(Serialize)]
struct UnitsListing {
    generated_at: String,
    unit: Unit,
    dbnums: Vec<u32>,
    parents: Vec<ParentListing>,
    units: Vec<UnitListing>,
}

/// 一个上级根下划出了多少单元。
#[derive(Serialize)]
struct ParentListing {
    dbno: u32,
    tag: String,
    #[serde(rename = "ref")]
    refno: String,
    noun: String,
    name: String,
    /// 上级根自己有没有名字：无名 SITE 的显示名要按整个 MDB 数序号。
    named: bool,
    units: usize,
    zones: usize,
    fallback: usize,
    /// 兜底单元按名词计数。
    fallback_nouns: BTreeMap<String, usize>,
}

#[derive(Serialize)]
struct UnitListing {
    dbno: u32,
    tag: String,
    #[serde(rename = "ref")]
    refno: String,
    noun: String,
    name: String,
    unit: UnitKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
}

/// `--list-units`：写 `<out>/units.json`，打印每个上级根划出多少单元后返回，不碰 E3D、不生成。
/// 给了 `--only` 时只列选中的作业，以及被点名、但一个单元也没划出来的上级根。
fn list_units(
    tree: &DirectTreeService,
    opts: &BatchOptions,
    dbnums: &[u32],
    roots: &[RootJob],
    jobs: &[RootJob],
    wanted: &[String],
    out_root: &Path,
) -> Result<BatchOutcome> {
    let mut parents = Vec::new();
    for root in roots {
        let parent_ref = root.display_ref();
        let mine: Vec<&RootJob> = jobs
            .iter()
            .filter(|job| match &job.parent {
                Some(parent) => parent.refno == parent_ref,
                None => job.refno == root.refno,
            })
            .collect();
        if !wanted.is_empty() && mine.is_empty() && !wanted.contains(&root.refno) {
            continue;
        }
        let mut fallback_nouns: BTreeMap<String, usize> = BTreeMap::new();
        for job in &mine {
            if job.unit_kind == UnitKind::Fallback {
                *fallback_nouns.entry(job.noun.clone()).or_default() += 1;
            }
        }
        let count = |kind: UnitKind| mine.iter().filter(|job| job.unit_kind == kind).count();
        parents.push(ParentListing {
            dbno: root.dbno,
            tag: root.tag.clone(),
            refno: parent_ref,
            noun: root.noun.clone(),
            name: root.name.clone(),
            named: RefU64::from_str(&root.refno)
                .ok()
                .and_then(|refno| tree.name_of(refno))
                .is_some(),
            units: mine.len(),
            zones: count(UnitKind::Zone),
            fallback: count(UnitKind::Fallback),
            fallback_nouns,
        });
    }
    let listing = UnitsListing {
        generated_at: chrono::Local::now().to_rfc3339(),
        unit: opts.unit,
        dbnums: dbnums.to_vec(),
        parents,
        units: jobs
            .iter()
            .map(|job| UnitListing {
                dbno: job.dbno,
                tag: job.tag.clone(),
                refno: job.display_ref(),
                noun: job.noun.clone(),
                name: job.name.clone(),
                unit: job.unit_kind,
                parent: job.parent.as_ref().map(|parent| parent.refno.clone()),
            })
            .collect(),
    };
    let path = out_root.join("units.json");
    write_json(&path, &listing)?;
    for parent in &listing.parents {
        println!(
            "{} {} {} {}{}：{} 个单元（ZONE {}，兜底 {}{}）",
            parent.tag,
            parent.refno,
            parent.noun,
            parent.name,
            if parent.named { "" } else { "（无名）" },
            parent.units,
            parent.zones,
            parent.fallback,
            if parent.fallback_nouns.is_empty() {
                String::new()
            } else {
                format!(
                    "：{}",
                    parent
                        .fallback_nouns
                        .iter()
                        .map(|(noun, count)| format!("{noun} {count}"))
                        .collect::<Vec<_>>()
                        .join("、")
                )
            }
        );
    }
    println!(
        "合计 {} 个上级根、{} 个单元 → {}",
        listing.parents.len(),
        listing.units.len(),
        path.display()
    );
    Ok(BatchOutcome {
        summary_path: path,
        clean: true,
        records: Vec::new(),
        batch_id: batch_id_for(out_root),
        stored: None,
    })
}

impl RootRecord {
    fn new(job: &RootJob) -> Self {
        Self {
            dbno: job.dbno,
            tag: job.tag.clone(),
            refno: job.display_ref(),
            noun: job.noun.clone(),
            name: job.name.clone(),
            unit: job.unit_kind,
            parent: job.parent.clone(),
            dir: job.dir.display().to_string(),
            status: String::new(),
            message: String::new(),
            resumed: false,
            export: None,
            generate: None,
            compare: None,
            counts: None,
            mismatches: Vec::new(),
            mismatches_truncated: false,
            centreline_only_baseline: 0,
            memory: None,
            mismatches_all: Vec::new(),
            centreline_names: Vec::new(),
            scope: None,
            element_checks: None,
            elements: None,
        }
    }

    /// 把构件级结果交出去（连同它们记在哪个单元名下），自己不再留：整根 8 万条不在汇总里多攒一份。
    /// 单元是这个作业自己（按 ZONE 跑是 ZONE / 兜底单元，整根跑是根）；`--scope` 选的就是整个作业时
    /// 按整个单元算。
    fn take_element_facts(&mut self) -> Option<super::store::ElementFacts> {
        let checks = self.elements.take()?;
        let unit_refno = self.refno.trim_start_matches('=').to_string();
        let root_refno = match &self.parent {
            Some(parent) => parent.refno.trim_start_matches('=').to_string(),
            None => unit_refno.clone(),
        };
        let scope = self
            .scope
            .as_deref()
            .and_then(|raw| ScopeArg::parse(raw).ok())
            .filter(|scope| !matches!(scope, ScopeArg::Subtree(refno) if *refno == unit_refno))
            .map(|scope| super::store::ElementScope {
                kind: match scope {
                    ScopeArg::Node(_) => "node",
                    ScopeArg::Subtree(_) => "subtree",
                }
                .to_string(),
                refno: scope.refno().to_string(),
            });
        Some(super::store::ElementFacts {
            checks,
            unit_refno,
            unit_kind: self.unit.as_str().to_string(),
            root_refno: Some(root_refno),
            scope,
        })
    }

    /// 库里这一行的主键：`<batch_id>/<tag>/<w0_w1>`。同一批重跑同一个作业覆盖同一行。
    fn run_id(&self, batch_id: &str) -> String {
        format!(
            "{batch_id}/{}/{}",
            self.tag,
            self.refno.trim_start_matches('=').replace('/', "_")
        )
    }

    /// 入库用的事实。`run_id` 见 [`Self::run_id`]；上级根对单元是它的 SITE / APPLDW，对根是它自己。
    /// 续跑从 `run.json` 读回来的记录只剩截断过的差异清单，走 [`resumed_facts`]，而且只补库里没有的。
    pub(crate) fn run_facts(&self, batch_id: &str, spec: &RunSpec) -> super::store::RunFacts {
        use super::store::{RunFacts, SOURCE_BATCH, StageSeconds};
        let refno = self.refno.trim_start_matches('=').to_string();
        let (root_refno, root_noun, root_name) = match &self.parent {
            Some(parent) => (
                parent.refno.trim_start_matches('=').to_string(),
                parent.noun.clone(),
                parent.name.clone(),
            ),
            None => (refno.clone(), self.noun.clone(), self.name.clone()),
        };
        let verdict = match self.status.as_str() {
            status::PASS | status::EMPTY | status::EXPORTED | status::GENERATED => {
                super::report::VERDICT_PASS
            }
            status::KIND_DIFFER => super::report::VERDICT_KIND,
            _ => super::report::VERDICT_FAIL,
        };
        let stage = match self.status.as_str() {
            status::NO_BASELINE => "e3d_export",
            status::GEN_FAILED | status::COMMIT_GUARD | status::TIMEOUT => "generate_ours",
            status::COMPARE_FAILED => "compare_failed",
            status::EXPORTED => "e3d_export",
            status::GENERATED => "generate_ours",
            _ => "compare",
        };
        let seconds = |stage: &Option<StageRecord>| stage.as_ref().map_or(0.0, |s| s.seconds);
        let finished_at = self
            .compare
            .as_ref()
            .or(self.generate.as_ref())
            .or(self.export.as_ref())
            .and_then(|stage| chrono::DateTime::parse_from_rfc3339(&stage.finished).ok())
            .map(|time| time.with_timezone(&chrono::Local))
            .unwrap_or_else(chrono::Local::now);
        let dir = PathBuf::from(&self.dir);
        let mut files = Vec::new();
        for (name, path) in [
            ("e3d.rvm", dir.join("e3d.rvm")),
            ("generated.rvm", dir.join("ours").join("generated.rvm")),
            ("compare.json", dir.join("compare.json")),
            ("run.json", dir.join("run.json")),
        ] {
            if path.is_file() {
                files.push(name.to_string());
            }
        }
        let message = if self.status == status::COMMIT_GUARD
            || self.status == status::TIMEOUT
            || self.status == status::GEN_FAILED
        {
            // 生成那一段的说明比终态行更具体（退出码、stderr 尾巴）。
            self.generate
                .as_ref()
                .map(|g| g.message.clone())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| self.message.clone())
        } else {
            self.message.clone()
        };
        RunFacts {
            run_id: self.run_id(batch_id),
            source: SOURCE_BATCH,
            batch_id: Some(batch_id.to_string()),
            project: spec.project_name.clone(),
            e3d_project: spec.e3d_project.clone(),
            e3d_mdb: spec.e3d_mdb.clone(),
            refno,
            dbnum: Some(self.dbno),
            db_type: Some("DESI".to_string()),
            sesno: None,
            noun: Some(self.noun.clone()),
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            unit: Some(self.unit.as_str().to_string()),
            root_refno: Some(root_refno),
            root_noun: Some(root_noun),
            root_name: (!root_name.is_empty()).then_some(root_name),
            verdict: verdict.to_string(),
            status: self.status.clone(),
            stage: stage.to_string(),
            message,
            counts: self.counts.clone().unwrap_or_default(),
            mismatches: if self.mismatches_all.is_empty() {
                self.mismatches.clone()
            } else {
                self.mismatches_all.clone()
            },
            centreline_only_baseline: self.centreline_names.clone(),
            e3d_export_by: self
                .export
                .as_ref()
                .and_then(|e| e.exit.as_str().map(str::to_string)),
            seconds: StageSeconds {
                resolve: 0.0,
                generate: seconds(&self.generate),
                e3d_export: seconds(&self.export),
                compare: seconds(&self.compare),
            },
            gen_peak_private_mb: self.generate.as_ref().and_then(|g| g.peak_private_mb),
            artifact_dir: Some(dir),
            files,
            started_at: None,
            finished_at,
            // 构件级结果不在这里抄一份：调用方用 [`Self::take_element_facts`] 整份搬过去。
            elements: None,
        }
    }

    /// [`Self::run_facts`] 加上构件级结果（搬走，不复制）：交给入库线程的那一份。
    fn take_run_facts(&mut self, batch_id: &str, spec: &RunSpec) -> super::store::RunFacts {
        let mut facts = self.run_facts(batch_id, spec);
        facts.elements = self.take_element_facts();
        facts
    }

    fn console_line(&self) -> String {
        let counts = self.counts.as_ref();
        let number =
            |pick: fn(&RvmCounts) -> u64| counts.map_or("-".to_string(), |c| pick(c).to_string());
        format!(
            "{:<8} {:<16} {:<6} {:<14} both {:>6} exact {:>6} off {:>5} only_b {:>5} only_g {:>5} kind_differ {:>4} | gen {:>6}s {:>6} MB | cmp {:>6} MB | export {:>6}s | compare {:>5}s{}",
            self.tag,
            self.refno,
            self.noun,
            self.status,
            number(|c| c.both),
            number(|c| c.exact),
            number(|c| c.off + c.container_off),
            number(|c| c.only_baseline + c.container_only_baseline),
            number(|c| c.only_generated + c.container_only_generated),
            number(|c| c.kind_differ),
            self.generate
                .as_ref()
                .map_or("-".to_string(), |g| format!("{:.1}", g.seconds)),
            self.generate
                .as_ref()
                .and_then(|g| g.peak_private_mb)
                .map_or("-".to_string(), |mb| mb.to_string()),
            self.compare
                .as_ref()
                .and_then(|c| c.peak_private_mb)
                .map_or("-".to_string(), |mb| mb.to_string()),
            self.export
                .as_ref()
                .map_or("-".to_string(), |e| format!("{:.1}", e.seconds)),
            self.compare
                .as_ref()
                .map_or("-".to_string(), |c| format!("{:.1}", c.seconds)),
            if self.message.is_empty() {
                String::new()
            } else {
                format!("  {}", tail(&self.message, 160).replace('\n', " "))
            }
        )
    }
}

/// 从基准目录取 `<tag>_<w0>_<w1>.rvm`：能硬链接就硬链接，不然复制。
fn place_baseline_from_dir(dir: &Path, job: &RootJob, started: Instant) -> ExportOutcome {
    let source = dir.join(format!("{}_{}.rvm", job.tag, job.stem()));
    if !source.is_file() {
        return ExportOutcome::Missing(StageRecord::new(
            "missing",
            started,
            format!("基准目录里没有 {}", source.display()),
        ));
    }
    let target = job.dir.join("e3d.rvm");
    if let Err(error) = std::fs::create_dir_all(&job.dir) {
        return ExportOutcome::Missing(StageRecord::new("io-error", started, error.to_string()));
    }
    let _ = std::fs::remove_file(&target);
    let linked =
        std::fs::hard_link(&source, &target).is_ok() || std::fs::copy(&source, &target).is_ok();
    if linked {
        ExportOutcome::Ready(StageRecord::new(
            "reuse",
            started,
            source.display().to_string(),
        ))
    } else {
        ExportOutcome::Missing(StageRecord::new(
            "io-error",
            started,
            format!("复制 {} 失败", source.display()),
        ))
    }
}

/// 导出线程：每个库一次 TTY 会话导全部根；没落出来的根再单独补导。
fn export_all(
    jobs: &[RootJob],
    pending: &[usize],
    spec: &RunSpec,
    out_root: &Path,
    e3d_timeout: Duration,
    board: &ExportBoard,
) -> Result<()> {
    // 按根在队列里首次出现的库序导出：工人按同一顺序生成，先生成的根先等到基准，流水线才不空转。
    let mut order: Vec<u32> = Vec::new();
    let mut by_db: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for &index in pending {
        let dbno = jobs[index].dbno;
        if !order.contains(&dbno) {
            order.push(dbno);
        }
        by_db.entry(dbno).or_default().push(index);
    }
    let mut session_spec = spec.clone();
    session_spec.tools.timeout = e3d_timeout;
    for dbno in order {
        let indices = by_db.remove(&dbno).unwrap_or_default();
        let tag = jobs[indices[0]].tag.clone();
        let word = if jobs[indices[0]].unit_kind == UnitKind::Root {
            "根"
        } else {
            "单元"
        };
        let started = Instant::now();
        let mut items = Vec::with_capacity(indices.len());
        for &index in &indices {
            let job = &jobs[index];
            std::fs::create_dir_all(&job.dir)?;
            let rvm = job.dir.join("e3d.rvm");
            let _ = std::fs::remove_file(&rvm);
            items.push((job.refno.clone(), rvm));
        }
        println!("E3D 导出 {tag}：{} 个{word}，一次会话 …", indices.len());
        let body = export_macro::export_batch_body(&items)?;
        // 导出文件在 L3-DONE 之前就写完了，QUIT 后只等 E3D 收尾 15 秒（环境变量仍可压过）。
        let done_grace = if std::env::var_os("E3D_MCP_DONE_GRACE_SECS").is_some() {
            crate::e3d_query::done_grace_from_env()
        } else {
            Duration::from_secs(15)
        };
        let driver = driver_with_grace(&session_spec, done_grace);
        let label = format!("rvm-check-export-{tag}");
        let session = driver.run_source_evidence(out_root, &label, &body);
        let session_seconds = started.elapsed().as_secs_f64();
        let (log, note) = match &session {
            Ok(evidence) => (
                evidence_text(evidence),
                format!(
                    "exit={:?} alive={} done={} timeout={}",
                    evidence.exit_status,
                    evidence.alive_seen,
                    evidence.done_seen,
                    evidence.timed_out
                ),
            ),
            Err(error) => (String::new(), format!("会话没有启动：{error:#}")),
        };
        let export_log = out_root.join(&tag).join("e3d-export.log");
        if let Some(parent) = export_log.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&export_log)
        {
            let _ = writeln!(
                file,
                "=== {} {tag} {} 个{word} {:.1}s {note}\n{}",
                chrono::Local::now().to_rfc3339(),
                indices.len(),
                session_seconds,
                redact(&log, &spec.password)
            );
        }
        println!("E3D 导出 {tag}：会话 {:.1}s（{note}）", session_seconds);
        let per_root_seconds = round1(session_seconds / indices.len() as f64);
        for &index in &indices {
            let job = &jobs[index];
            let rvm = job.dir.join("e3d.rvm");
            let size = std::fs::metadata(&rvm).map(|meta| meta.len()).unwrap_or(0);
            if export_macro::export_done_for(&log, &job.refno) && size > 32 {
                let mut record = StageRecord::new("batch", started, "");
                record.seconds = per_root_seconds;
                board.set(index, ExportOutcome::Ready(record));
                continue;
            }
            // 单独补导：页面那条 refno → 名称的退路。
            let retry_started = Instant::now();
            let mut root_spec = session_spec.clone();
            root_spec.refno = job.refno.clone();
            let _ = std::fs::remove_file(&rvm);
            let outcome = match export_e3d(&root_spec, &job.dir) {
                Ok(by) => {
                    let mut record = StageRecord::new(
                        by.as_str(),
                        retry_started,
                        "批量会话里没落出文件，单独补导成功",
                    );
                    record.seconds = round1(record.seconds + per_root_seconds);
                    ExportOutcome::Ready(record)
                }
                Err(message) => {
                    let mut record = StageRecord::new(
                        "missing",
                        retry_started,
                        redact(&message, &spec.password),
                    );
                    record.seconds = round1(record.seconds + per_root_seconds);
                    ExportOutcome::Missing(record)
                }
            };
            println!(
                "E3D 补导 {} {}：{}",
                job.tag,
                job.display_ref(),
                match &outcome {
                    ExportOutcome::Ready(record) => format!("成功（{}）", record.exit),
                    ExportOutcome::Missing(record) =>
                        format!("失败：{}", tail(&record.message, 200)),
                }
            );
            board.set(index, outcome);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_root(
    job: &RootJob,
    index: usize,
    opts: &BatchOptions,
    spec: &RunSpec,
    tree: &DirectTreeService,
    out_root: &Path,
    board: &ExportBoard,
    in_process_gate: &Mutex<()>,
) -> RootRecord {
    let mut record = RootRecord::new(job);
    record.scope = opts.scope.as_ref().map(ScopeArg::to_arg);
    // 比对段峰值（MB）：只有融合对拍且阶段标记落盘时才有值，后面写 compare 段记录时用。
    let mut compare_peak_mb: Option<u64> = None;
    if let Err(error) = std::fs::create_dir_all(&job.dir) {
        record.status = status::GEN_FAILED.to_string();
        record.message = format!("建目录失败：{error}");
        return record;
    }

    // 生成即比对在生成前就领了交接板上这一格，后面不再等第二次。
    let mut export_seen = false;
    if opts.stage.generates() {
        let generated = job.dir.join("ours").join("generated.rvm");
        let _ = std::fs::remove_file(&generated);
        // 融合对拍的产物一并清掉：不然上一轮的 compare/judgement 会被当成这一轮的结果。
        let _ = std::fs::remove_file(job.dir.join("compare.json"));
        let _ = std::fs::remove_file(job.dir.join("judgement.json"));
        // 构件级结果与成员出处同理：这一轮没比对出结果时，不能把上一轮的构件行再写一遍。
        let _ = std::fs::remove_file(job.dir.join(super::element::ELEMENTS_FILE));
        let _ = std::fs::remove_file(
            job.dir
                .join("ours")
                .join(super::element::SOURCES_FILE),
        );
        // 阶段标记也要清：上一轮留下的 compare_start 会让这一轮的采样一开始就归到比对段。
        let _ = std::fs::remove_file(job.dir.join(PHASE_COMPARE_START));
        let _ = std::fs::remove_file(job.dir.join(PHASE_COMPARE_END));
        // `--scope` 落的名字路径同理：不清就会拿上一轮的选中节点去裁这一轮的基准。
        let _ = std::fs::remove_file(job.dir.join("scope.json"));
        // 生成即比对：先等交接板上这一格——`--baseline-dir` 在工人开工前已同步放好，E3D 模式等导出
        // 线程填。基准在盘上才开工；缺基准记 `no-baseline` 收工，不退回「先写 generated.rvm」的老路。
        let fused = if opts.fused_compare {
            let outcome = board.wait(index);
            record.export = Some(outcome.record().clone());
            export_seen = true;
            match fused_from(&outcome, job, opts) {
                Ok(fused) => Some(fused),
                Err(message) => {
                    record.status = status::NO_BASELINE.to_string();
                    record.message = message;
                    return record;
                }
            }
        } else {
            None
        };
        let (generation, measured) = if opts.in_process {
            let _serial = in_process_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // 进程内跑没有独立子进程可采样，比对段峰值只能留空。
            (
                generate_in_process(
                    job,
                    spec,
                    tree,
                    opts.gen_timeout,
                    fused.as_ref(),
                    opts.element_cache,
                    opts.scope.as_ref(),
                ),
                None,
            )
        } else {
            generate_in_child(job, spec, out_root, opts, fused.as_ref())
        };
        compare_peak_mb = measured;
        // 不写 generated.rvm 时，生成成功的物证是子进程落下的 judgement.json
        // （compare.json 默认不落——结果入库，大件 JSON 只是可选下钻证据）。
        let produced = fused.as_ref().map_or_else(
            || generated.is_file(),
            |fused| {
                if fused.emit_rvm {
                    generated.is_file()
                } else {
                    job.dir.join("judgement.json").is_file()
                }
            },
        );
        let failed_status = match generation.exit.as_str() {
            Some(status::COMMIT_GUARD) => Some(status::COMMIT_GUARD),
            Some(status::TIMEOUT) => Some(status::TIMEOUT),
            _ if generation.exit != serde_json::Value::from(0) => Some(status::GEN_FAILED),
            _ if !produced => Some(status::GEN_FAILED),
            _ => None,
        };
        if let Some(failed) = failed_status {
            record.status = failed.to_string();
            record.message = if generation.message.is_empty() {
                "生成没有写出 generated.rvm".to_string()
            } else {
                generation.message.clone()
            };
        }
        record.generate = Some(generation);
    }

    if opts.stage != Stage::Generate && !export_seen {
        let outcome = board.wait(index);
        record.export = Some(outcome.record().clone());
        if let ExportOutcome::Missing(export) = &outcome
            && record.status.is_empty()
        {
            record.status = status::NO_BASELINE.to_string();
            record.message = export.message.clone();
        }
    }

    if !record.status.is_empty() {
        return record;
    }
    match opts.stage {
        Stage::Export => {
            record.status = status::EXPORTED.to_string();
            return record;
        }
        Stage::Generate => {
            record.status = status::GENERATED.to_string();
            return record;
        }
        Stage::Full | Stage::Compare => {}
    }

    let generated = job.dir.join("ours").join("generated.rvm");
    let judgement_path = job.dir.join("judgement.json");
    if !judgement_path.is_file() && !generated.is_file() {
        record.status = status::GEN_FAILED.to_string();
        record.message = format!("产物目录里没有 {}", generated.display());
        return record;
    }
    let started = Instant::now();
    // 子进程已做完生成即比对（judgement.json 在盘上）时直接读判定：
    // `judge_report_full` 在子进程内跑过，compare.json 也是它落的，判色口径一致。
    // 第二项是构件级结果：生成即比对由子进程落在 `element-checks.json`，读文件比对在这里当场摊出来
    // 并落同一个文件（续跑补入库读它）。拿不到时为 `None`，这次就不动 `rvm_check_element`。
    let judgement = if judgement_path.is_file() {
        std::fs::read(&judgement_path)
            .map_err(|error| format!("读 judgement.json 失败：{error}"))
            .and_then(|bytes| {
                serde_json::from_slice::<super::report::Judgement>(&bytes)
                    .map_err(|error| format!("解析 judgement.json 失败：{error}"))
            })
            .map(|judgement| {
                let elements = super::element::read_checks(&job.dir).unwrap_or_else(|message| {
                    eprintln!(
                        "警告：{} {} 的构件级结果读不回来，这次不写构件行：{message}",
                        job.tag,
                        job.display_ref()
                    );
                    None
                });
                (judgement, elements)
            })
    } else {
        // 比对范围从生成侧落的 `scope.json` 读回：名字路径只能在生成侧算（要 `DbSet` +
        // `UdetCatalog` 复现 E3D 显示名），父进程手里只有 `DirectTreeService`。
        if opts.scope.is_some() && !job.dir.join("scope.json").is_file() {
            // 只比对阶段没跑过生成：没有名字路径就裁不了基准，静默退回「整个根」会把
            // 范围外成员全记成 `only_baseline` 判红。明说缺什么。
            Err(format!(
                "--scope 要生成侧落的 {}（选中节点的名字路径），这个产物目录里没有：\
                 先跑生成（--stage full 或 generate）",
                job.dir.join("scope.json").display()
            ))
        } else {
            match super::read_scope_file(&job.dir) {
                Ok(scope) => compare_files(job, spec, tree, &scope),
                Err(message) => Err(message),
            }
        }
    };
    match judgement {
        Ok((judgement, elements)) => {
            record.element_checks = elements.as_ref().map(Vec::len);
            record.elements = elements;
            let verdict = judgement.verdict;
            record.message = verdict_message(verdict, &judgement.counts);
            record.status = match verdict {
                RvmVerdict::Pass if judgement.counts.both == 0 => status::EMPTY,
                RvmVerdict::Pass => status::PASS,
                RvmVerdict::KindDiffer => status::KIND_DIFFER,
                RvmVerdict::Fail => status::FAIL,
            }
            .to_string();
            let mut compare = StageRecord::new(
                if judgement_path.is_file() {
                    serde_json::Value::from("fused")
                } else {
                    serde_json::Value::from(0)
                },
                started,
                "",
            );
            compare.peak_private_mb = compare_peak_mb;
            if compare_peak_mb.is_none() {
                // 两种拿不到的情形分开写清，免得把「没测」看成「测出来是 0」。
                compare.message = if judgement_path.is_file() {
                    "比对跑在生成子进程内，但没有阶段标记文件（ours/phase.compare_start/.end），\
                     切不出比对段峰值"
                        .to_string()
                } else {
                    "读文件比对在父进程内跑，未单独采样；不拿父进程内存冒充".to_string()
                };
            }
            record.compare = Some(compare);
            let (page_mismatches, truncated) = judgement.page_mismatches();
            record.counts = Some(judgement.counts);
            record.mismatches = page_mismatches;
            record.mismatches_truncated = truncated;
            record.centreline_only_baseline = judgement.centreline_only_baseline.len();
            record.mismatches_all = judgement.mismatches;
            record.centreline_names = judgement.centreline_only_baseline;
        }
        Err(message) => {
            record.status = status::COMPARE_FAILED.to_string();
            record.message = redact(&message, &spec.password);
            let mut compare = StageRecord::new(1, started, record.message.clone());
            compare.peak_private_mb = compare_peak_mb;
            record.compare = Some(compare);
        }
    }
    record
}

/// 读文件比对（不走生成即比对的那条路）：判色之外把构件级结果也摊出来，落进作业目录。
///
/// 构件级结果要把基准侧的单侧件按名字解回 refno，那一步得知道这个作业的库文件与会话
/// （[`resolve_node`]）。节点解不出来时比对照做，只是这次没有构件级结果。
fn compare_files(
    job: &RootJob,
    spec: &RunSpec,
    tree: &DirectTreeService,
    scope: &e3d_model::rvm_compare::CompareScope,
) -> std::result::Result<
    (
        super::report::Judgement,
        Option<Vec<super::element::ElementCheck>>,
    ),
    String,
> {
    let mut spec = spec.clone();
    spec.refno = job.refno.clone();
    let node = match resolve_node(tree, &spec) {
        Ok(node) => node,
        Err(message) => {
            eprintln!(
                "警告：{} {} 解不出库文件与会话，这次不出构件级结果：{message}",
                job.tag,
                job.display_ref()
            );
            return super::compare_rvms_scoped(&job.dir, scope).map(|judgement| (judgement, None));
        }
    };
    let (judgement, checks) = super::compare_rvms_with_elements(&job.dir, scope, &spec, &node)?;
    if let Err(message) = super::element::write_checks(&job.dir, &checks) {
        eprintln!(
            "警告：{} {} 的构件级结果没有落盘（续跑补入库时补不了构件行）：{message}",
            job.tag,
            job.display_ref()
        );
    }
    Ok((judgement, Some(checks)))
}

fn generate_in_process(
    job: &RootJob,
    spec: &RunSpec,
    tree: &DirectTreeService,
    gen_timeout: Duration,
    fused: Option<&FusedCompare>,
    element_cache: usize,
    scope: Option<&ScopeArg>,
) -> StageRecord {
    let started = Instant::now();
    let mut spec = spec.clone();
    spec.refno = job.refno.clone();
    spec.tools.timeout = gen_timeout;
    let node = match resolve_node(tree, &spec) {
        Ok(node) => node,
        Err(message) => return StageRecord::new(1, started, message),
    };
    match super::generate_ours_scoped(&spec, &node, &job.dir, fused, element_cache, scope) {
        Ok(()) => StageRecord::new(0, started, ""),
        Err(message) => StageRecord::new(1, started, message),
    }
}

/// 比对段在产物目录里的阶段标记文件：子进程在 vendor 比对前后各写一个，
/// 父进程据此把「当前提交量」采样归到 gen 段还是 compare 段。
const PHASE_COMPARE_START: &str = "ours/phase.compare_start";
const PHASE_COMPARE_END: &str = "ours/phase.compare_end";

/// 生成段 / 收尾段的采样间隔。生成段是几分钟量级，密采样拿不到更多信息，只会白烧 CPU。
const GEN_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// 比对段的采样间隔。比对段实测只有 0.0–0.1 秒
/// （`output/rvm-check-batch/mem-breakdown/summary.txt` 的 `cmp_s` 列，ams1112 `=9304/2`），
/// 沿用 500ms 一轮的话大概率一个样本都落不进比对段，`compare_peak` 恒为 0、
/// 比对段峰值永远是 `None`——功能等于没做。20ms 一轮至少能在 0.1 秒的窗口里采到 4~5 个样本。
const COMPARE_SAMPLE_INTERVAL: Duration = Duration::from_millis(20);

/// 阶段标记状态机。标记文件不存在时永远停在 [`Phase::Gen`]——这是可接受的降级：
/// 融合对拍（生成完接着比对，同一子进程）切不出比对段峰值，compare 峰值留 `None`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// `compare_start` 还没出现：采样归生成段。
    Gen,
    /// `compare_start` 已出现、`compare_end` 未出现：采样归比对段。
    Compare,
    /// 两个标记都出现：比对已收尾，后续采样不再计入比对段（仍计入生成段累计峰值）。
    Done,
}

impl Phase {
    /// 按产物目录里标记文件的存在情况推进状态，只前进不回退。
    fn advance(self, dir: &Path) -> Self {
        match self {
            Self::Gen if dir.join(PHASE_COMPARE_START).exists() => Self::Compare,
            Self::Compare if dir.join(PHASE_COMPARE_END).exists() => Self::Done,
            other => other,
        }
    }

    /// 本阶段的采样间隔：只有比对段需要密采样，见 [`COMPARE_SAMPLE_INTERVAL`] 的说明。
    fn sample_interval(self) -> Duration {
        match self {
            Self::Compare => COMPARE_SAMPLE_INTERVAL,
            Self::Gen | Self::Done => GEN_SAMPLE_INTERVAL,
        }
    }
}

/// 选中节点与根是否同一个库。`export_rvm` 只把 `db_file` 那一只库当主库，
/// 跨库的选中节点在 DFS 里根本不会出现（不是「比空」，是「找不到起点」）。
fn scope_same_db(tree: &DirectTreeService, scope: &ScopeArg, dbno: u32) -> Result<()> {
    let refno = RefU64::from_str(scope.refno())
        .map_err(|error| anyhow::anyhow!("refno 无法解析：{error}"))?;
    match tree.dbnum_of(refno) {
        Some(found) if found == dbno => Ok(()),
        Some(found) => bail!(
            "--scope {} 在库 {found}，这个根在库 {dbno}：跨库的选中节点跑不了\
             （export_rvm 只把根所在库当主库）",
            scope.to_arg()
        ),
        None => bail!("--scope {} 不在当前 MDB 里", scope.to_arg()),
    }
}

/// 子进程生成：`aios-database rvm-check --child …`，父进程盯内存与耗时。
///
/// 返回 `(生成段记录, 比对段峰值 MB)`。第二项只有融合对拍且阶段标记落盘时才有值：
/// 比对跑在同一子进程里，`PeakPagefileUsage` 是整段累计峰值，切不出比对段，
/// 所以改成每轮读**当前**提交量、按阶段标记归段、段内取 max。
fn generate_in_child(
    job: &RootJob,
    spec: &RunSpec,
    out_root: &Path,
    opts: &BatchOptions,
    fused: Option<&FusedCompare>,
) -> (StageRecord, Option<u64>) {
    let started = Instant::now();
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            return (
                StageRecord::new(1, started, format!("取不到自身路径：{error}")),
                None,
            );
        }
    };
    let stdout = std::fs::File::create(job.dir.join("gen.stdout.log"));
    let stderr = std::fs::File::create(job.dir.join("gen.stderr.log"));
    let (Ok(stdout), Ok(stderr)) = (stdout, stderr) else {
        return (StageRecord::new(1, started, "建 gen.*.log 失败"), None);
    };
    let mut command = std::process::Command::new(exe);
    command
        .arg("rvm-check")
        .arg("--child")
        .arg("--dbnum")
        .arg(job.dbno.to_string())
        .arg("--only")
        .arg(job.display_ref())
        .arg("--out")
        .arg(out_root)
        .arg("--gen-timeout-secs")
        .arg(opts.gen_timeout.as_secs().to_string())
        .arg("--projects-dir")
        .arg(&spec.projects_dir)
        .arg("--project-name")
        .arg(&spec.project_name)
        .arg("--e3d-project")
        .arg(&spec.e3d_project)
        .arg("--mdb")
        .arg(&spec.e3d_mdb);
    // `--scope` 必须透传：子进程才是真正算名字路径、决定 DFS 起点的那一侧。
    if let Some(scope) = &opts.scope {
        command.arg("--scope").arg(scope.to_arg());
    }
    if let Some(fused) = fused {
        command
            .arg("--compare-baseline")
            .arg(&fused.baseline)
            .arg("--emit-rvm")
            .arg(fused.emit_rvm.to_string());
    }
    command
        .arg("--compare-memory-mb")
        .arg(opts.compare_memory_mb.to_string())
        .arg("--element-cache")
        .arg(opts.element_cache.to_string())
        .arg("--emit-compare")
        .arg(opts.emit_compare.to_string())
        .arg("--threads")
        .arg(opts.threads.to_string());
    command
        .env("RUST_BACKTRACE", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(stdout))
        .stderr(std::process::Stdio::from(stderr));
    // 双保险：子进程里若有池在 `build_global` 之前就读环境，也能按同一线程数起来。
    if opts.threads > 0 {
        command.env("RAYON_NUM_THREADS", opts.threads.to_string());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return (
                StageRecord::new(1, started, format!("起子进程失败：{error}")),
                None,
            );
        }
    };
    let mut peak: u64 = 0;
    let mut compare_peak: u64 = 0;
    let mut min_free: Option<f64> = None;
    let mut killed: Option<&'static str> = None;
    let mut phase = Phase::Gen;
    let deadline = started + opts.gen_timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => {
                return (
                    StageRecord::new(1, started, format!("等子进程失败：{error}")),
                    None,
                );
            }
        }
        // 整段累计峰值：现有语义不动（`gen.peak_private_mb` 到处都是）。
        // `PeakPagefileUsage` 是内核维护的累计峰值，读它只取值、不改值，所以比对段密采样
        // 只是把同一个数读得更勤，不会让峰值变大或失真——因此这里不降频，每轮都读。
        if let Some(bytes) = memory::process_peak_private_bytes(&child) {
            peak = peak.max(bytes);
        }
        // 当前提交量：按阶段标记归段，比对段取 max。标记文件没落盘时永远停在 Gen，
        // compare_peak 保持 0，比对段峰值留 None。
        phase = phase.advance(&job.dir);
        if let Some(bytes) = memory::process_private_bytes(&child) {
            if phase == Phase::Compare {
                compare_peak = compare_peak.max(bytes);
            }
        }
        // 提交量余量取整段 min：比对段密采样只是把采样点变密，min 只会更准，不会变松。
        if let Some(free) = memory::commit_free_gb() {
            min_free = Some(min_free.map_or(free, |current: f64| current.min(free)));
            if free < opts.min_commit_free_gb {
                killed = Some(status::COMMIT_GUARD);
            }
        }
        if killed.is_none() && Instant::now() >= deadline {
            killed = Some(status::TIMEOUT);
        }
        if killed.is_some() {
            let _ = child.kill();
            break;
        }
        // 间隔按阶段自适应：比对段几十毫秒量级必须密采样，生成段与收尾段 500ms 足够。
        // 顺带的效果是超时判定在比对段也更密，TIMEOUT 只会更及时，不会更松。
        std::thread::sleep(phase.sample_interval());
    }
    let exit = child.wait();
    // 退出后再读一次峰值。循环里是「先看退没退、再读峰值、再睡一个间隔」：不足一个间隔就跑完的
    // 单元（release 构建下的小 ZONE 不到 0.5 秒）只在刚起进程时读到过一次，记下来的是 1 MB；
    // 跑得久的也漏掉最后一个间隔里涨的那一段。句柄在 `child` 释放前一直有效，内核累计峰值照样读得到。
    if let Some(bytes) = memory::process_peak_private_bytes(&child) {
        peak = peak.max(bytes);
    }
    let stderr_tail = std::fs::read_to_string(job.dir.join("gen.stderr.log"))
        .map(|text| tail(&text, 600))
        .unwrap_or_default();
    let mut record = match killed {
        Some(status::COMMIT_GUARD) => StageRecord::new(
            status::COMMIT_GUARD,
            started,
            format!(
                "可提交内存余量低于 {:.1} GB，已杀掉生成子进程",
                opts.min_commit_free_gb
            ),
        ),
        Some(_) => StageRecord::new(
            status::TIMEOUT,
            started,
            format!("生成超过 {} 秒，已杀掉子进程", opts.gen_timeout.as_secs()),
        ),
        None => match exit {
            Ok(status) if status.success() => StageRecord::new(0, started, ""),
            Ok(status) => StageRecord::new(
                status.code().unwrap_or(-1),
                started,
                if stderr_tail.trim().is_empty() {
                    format!("生成子进程退出码 {status}")
                } else {
                    stderr_tail
                },
            ),
            Err(error) => StageRecord::new(1, started, format!("等子进程失败：{error}")),
        },
    };
    record.peak_private_mb = (peak > 0).then_some(peak / (1024 * 1024));
    record.min_commit_free_gb = min_free.map(round2);
    // 比对段峰值：标记文件没落盘时 compare_peak 恒 0，这里给 None 而不是编一个数。
    let compare_peak_mb = (compare_peak > 0).then_some(compare_peak / (1024 * 1024));
    (record, compare_peak_mb)
}

/// 内存护栏要读的三个数：系统可提交内存余量、子进程峰值私有内存、子进程当前提交量。
mod memory {
    #[cfg(windows)]
    pub fn commit_free_gb() -> Option<f64> {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        // 安全性：结构体已按 Win32 要求填好长度，指针在调用期间有效。
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        (ok != 0).then(|| status.ullAvailPageFile as f64 / (1024.0 * 1024.0 * 1024.0))
    }

    #[cfg(windows)]
    pub fn process_peak_private_bytes(child: &std::process::Child) -> Option<u64> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;
        // 安全性：句柄来自 std 的 Child，进程存活期间有效；结构体长度已填。
        let ok =
            unsafe { K32GetProcessMemoryInfo(child.as_raw_handle() as _, &mut counters, size) };
        (ok != 0).then(|| counters.PeakPagefileUsage as u64)
    }

    /// 子进程**当前**提交量（字节）：`PagefileUsage`，不是 `PeakPagefileUsage`。
    /// 峰值切不出「比对段」这种同一进程内的阶段，只能按时刻采样当前值再归段取 max。
    #[cfg(windows)]
    pub fn process_private_bytes(child: &std::process::Child) -> Option<u64> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;
        // 安全性：句柄来自 std 的 Child，进程存活期间有效；结构体长度已填。
        let ok =
            unsafe { K32GetProcessMemoryInfo(child.as_raw_handle() as _, &mut counters, size) };
        (ok != 0).then(|| counters.PagefileUsage as u64)
    }

    #[cfg(not(windows))]
    pub fn commit_free_gb() -> Option<f64> {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        Some(system.available_memory() as f64 / (1024.0 * 1024.0 * 1024.0))
    }

    #[cfg(not(windows))]
    pub fn process_peak_private_bytes(child: &std::process::Child) -> Option<u64> {
        use sysinfo::{Pid, ProcessesToUpdate, System};
        let pid = Pid::from_u32(child.id());
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        system.process(pid).map(|process| process.memory())
    }

    /// 非 Windows：`sysinfo` 只给当前驻留，没有「当前提交量」这一项，
    /// 也没有跨平台的历史峰值可读——与现有峰值回退同口径，直接返回 None，
    /// 让比对段峰值在非 Windows 上保持 `None`（不拿驻留冒充提交量）。
    #[cfg(not(windows))]
    pub fn process_private_bytes(_child: &std::process::Child) -> Option<u64> {
        None
    }
}

#[derive(Serialize)]
struct DbSummary {
    dbno: u32,
    tag: String,
    roots: usize,
    clean_roots: usize,
    status_counts: BTreeMap<String, usize>,
    counts: RvmCounts,
    gen_seconds: f64,
    gen_peak_private_mb: u64,
    /// 本库各根比对段峰值的最大者（MB）。没有一个根测到时为 0——`summary.txt` 打印成 `-`。
    compare_peak_private_mb: u64,
    export_seconds: f64,
    compare_seconds: f64,
}

#[derive(Serialize)]
struct Summary {
    generated_at: String,
    /// 库里 `rvm_check_run.batch_id` 的取值（[`batch_id_for`]）。
    batch_id: String,
    stage: Stage,
    out: String,
    dbnums: Vec<u32>,
    site_only: bool,
    /// 比对单元；`root`（现状）不落进 summary.json。
    #[serde(skip_serializing_if = "Unit::is_root")]
    unit: Unit,
    jobs: usize,
    in_process: bool,
    baseline: String,
    e3d_project: String,
    mdb: String,
    clean: bool,
    status_counts: BTreeMap<String, usize>,
    totals: RvmCounts,
    gen_seconds: f64,
    export_seconds: f64,
    compare_seconds: f64,
    dbs: Vec<DbSummary>,
    roots: Vec<RootRecord>,
}

impl Summary {
    fn build(
        opts: &BatchOptions,
        spec: &RunSpec,
        dbnums: &[u32],
        baseline: &str,
        batch_id: &str,
        roots: Vec<RootRecord>,
    ) -> Self {
        let mut status_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut totals = RvmCounts::default();
        let mut dbs: BTreeMap<u32, DbSummary> = BTreeMap::new();
        let (mut gen_seconds, mut export_seconds, mut compare_seconds) = (0.0, 0.0, 0.0);
        for record in &roots {
            *status_counts.entry(record.status.clone()).or_default() += 1;
            let db = dbs.entry(record.dbno).or_insert_with(|| DbSummary {
                dbno: record.dbno,
                tag: record.tag.clone(),
                roots: 0,
                clean_roots: 0,
                status_counts: BTreeMap::new(),
                counts: RvmCounts::default(),
                gen_seconds: 0.0,
                gen_peak_private_mb: 0,
                compare_peak_private_mb: 0,
                export_seconds: 0.0,
                compare_seconds: 0.0,
            });
            db.roots += 1;
            if status::is_clean(&record.status) {
                db.clean_roots += 1;
            }
            *db.status_counts.entry(record.status.clone()).or_default() += 1;
            if let Some(counts) = &record.counts {
                totals.add(counts);
                db.counts.add(counts);
            }
            if let Some(generation) = &record.generate {
                gen_seconds += generation.seconds;
                db.gen_seconds += generation.seconds;
                db.gen_peak_private_mb = db
                    .gen_peak_private_mb
                    .max(generation.peak_private_mb.unwrap_or(0));
            }
            if let Some(export) = &record.export {
                export_seconds += export.seconds;
                db.export_seconds += export.seconds;
            }
            if let Some(compare) = &record.compare {
                compare_seconds += compare.seconds;
                db.compare_seconds += compare.seconds;
                db.compare_peak_private_mb = db
                    .compare_peak_private_mb
                    .max(compare.peak_private_mb.unwrap_or(0));
            }
        }
        for db in dbs.values_mut() {
            db.gen_seconds = round1(db.gen_seconds);
            db.export_seconds = round1(db.export_seconds);
            db.compare_seconds = round1(db.compare_seconds);
        }
        Self {
            generated_at: chrono::Local::now().to_rfc3339(),
            batch_id: batch_id.to_string(),
            stage: opts.stage,
            out: opts.out.display().to_string(),
            dbnums: dbnums.to_vec(),
            site_only: opts.site_only,
            unit: opts.unit,
            jobs: opts.jobs,
            in_process: opts.in_process,
            baseline: baseline.to_string(),
            e3d_project: spec.e3d_project.clone(),
            mdb: spec.e3d_mdb.clone(),
            clean: roots.iter().all(|record| status::is_clean(&record.status)),
            status_counts,
            totals,
            gen_seconds: round1(gen_seconds),
            export_seconds: round1(export_seconds),
            compare_seconds: round1(compare_seconds),
            dbs: dbs.into_values().collect(),
            roots,
        }
    }

    fn table(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{:<8} {:<16} {:<6} {:<14} {:>7} {:>7} {:>6} {:>7} {:>7} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}\n",
            "tag", "ref", "noun", "status", "both", "exact", "off", "only_b", "only_g", "kdiff",
            "gen_s", "gen_MB", "cmp_MB", "export_s", "cmp_s"
        ));
        for record in &self.roots {
            let counts = record.counts.clone().unwrap_or_default();
            let has_counts = record.counts.is_some();
            let cell = |value: u64| {
                if has_counts {
                    value.to_string()
                } else {
                    "-".to_string()
                }
            };
            out.push_str(&format!(
                "{:<8} {:<16} {:<6} {:<14} {:>7} {:>7} {:>6} {:>7} {:>7} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}\n",
                record.tag,
                record.refno,
                record.noun,
                record.status,
                cell(counts.both),
                cell(counts.exact),
                cell(counts.off + counts.container_off),
                cell(counts.only_baseline + counts.container_only_baseline),
                cell(counts.only_generated + counts.container_only_generated),
                cell(counts.kind_differ),
                record.generate.as_ref().map_or("-".to_string(), |g| format!("{:.1}", g.seconds)),
                record
                    .generate
                    .as_ref()
                    .and_then(|g| g.peak_private_mb)
                    .map_or("-".to_string(), |mb| mb.to_string()),
                record
                    .compare
                    .as_ref()
                    .and_then(|c| c.peak_private_mb)
                    .map_or("-".to_string(), |mb| mb.to_string()),
                record.export.as_ref().map_or("-".to_string(), |e| format!("{:.1}", e.seconds)),
                record.compare.as_ref().map_or("-".to_string(), |c| format!("{:.1}", c.seconds)),
            ));
        }
        out.push('\n');
        for db in &self.dbs {
            // 一个根都没测到比对段峰值时打 `-`：0 会被读成「测出来是 0」。
            let compare_peak = if db.compare_peak_private_mb > 0 {
                format!("{} MB", db.compare_peak_private_mb)
            } else {
                "-".to_string()
            };
            out.push_str(&format!(
                "{:<8} roots {:>3} clean {:>3}  both {:>7} exact {:>7} off {:>5} only_b {:>5} only_g {:>5} kind_differ {:>4}  gen {:>7.1}s peak {:>6} MB  cmp peak {:>8}  export {:>7.1}s  compare {:>6.1}s  {}\n",
                db.tag,
                db.roots,
                db.clean_roots,
                db.counts.both,
                db.counts.exact,
                db.counts.off + db.counts.container_off,
                db.counts.only_baseline + db.counts.container_only_baseline,
                db.counts.only_generated + db.counts.container_only_generated,
                db.counts.kind_differ,
                db.gen_seconds,
                db.gen_peak_private_mb,
                compare_peak,
                db.export_seconds,
                db.compare_seconds,
                db.status_counts
                    .iter()
                    .map(|(status, count)| format!("{status} {count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str(&format!(
            "\n合计 {} 个{}：{}；both {} exact {} chord {} reference_artifact {} film {} off {} only_baseline {} only_generated {} unkeyable {}/{} kind_differ {}；生成 {:.1}s 导出 {:.1}s 比对 {:.1}s → {}\n",
            self.roots.len(),
            self.unit.word(),
            self.status_counts
                .iter()
                .map(|(status, count)| format!("{status} {count}"))
                .collect::<Vec<_>>()
                .join(", "),
            self.totals.both,
            self.totals.exact,
            self.totals.chord,
            self.totals.reference_artifact,
            self.totals.film,
            self.totals.off + self.totals.container_off,
            self.totals.only_baseline + self.totals.container_only_baseline,
            self.totals.only_generated + self.totals.container_only_generated,
            self.totals.unkeyable_baseline,
            self.totals.unkeyable_generated,
            self.totals.kind_differ,
            self.gen_seconds,
            self.export_seconds,
            self.compare_seconds,
            if self.clean { "CLEAN" } else { "NOT CLEAN" }
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refnos_normalize_to_slash_form() {
        assert_eq!(
            normalize_ref("=24383/66456").as_deref(),
            Some("24383/66456")
        );
        assert_eq!(normalize_ref("24383/66456").as_deref(), Some("24383/66456"));
        assert_eq!(normalize_ref("24383_66456").as_deref(), Some("24383/66456"));
        assert_eq!(
            normalize_ref("  =24383/66456 ").as_deref(),
            Some("24383/66456")
        );
        assert_eq!(normalize_ref("nonsense"), None);
    }

    #[test]
    fn root_dirs_follow_tag_and_stem() {
        assert_eq!(tag_of("AMS", 7999), "ams7999");
        assert_eq!(
            root_dir(Path::new("out"), "ams7999", "24383/66456"),
            Path::new("out").join("ams7999").join("24383_66456")
        );
    }

    #[test]
    fn export_board_hands_outcomes_to_waiting_workers_and_fills_the_rest() {
        let board = ExportBoard::new(3);
        board.set(
            1,
            ExportOutcome::Ready(StageRecord::new("batch", Instant::now(), "")),
        );
        assert!(matches!(board.wait(1), ExportOutcome::Ready(_)));
        board.fill_missing("导出线程中止");
        assert!(
            matches!(board.wait(0), ExportOutcome::Missing(record) if record.message == "导出线程中止")
        );
        assert!(
            matches!(board.wait(1), ExportOutcome::Ready(_)),
            "已填的格不被覆盖"
        );
    }

    #[test]
    fn clean_statuses_are_pass_and_empty_only_in_full_mode() {
        assert!(status::is_clean(status::PASS));
        assert!(status::is_clean(status::EMPTY));
        assert!(!status::is_clean(status::KIND_DIFFER));
        assert!(!status::is_clean(status::FAIL));
        assert!(!status::is_clean(status::NO_BASELINE));
        assert!(!status::is_clean(status::COMMIT_GUARD));
        assert!(status::is_compared(status::FAIL));
        assert!(!status::is_compared(status::GEN_FAILED));
    }

    fn root_job(dbno: u32, refno: &str) -> RootJob {
        RootJob {
            dbno,
            tag: tag_of("AMS", dbno),
            refno: refno.to_string(),
            noun: "SITE".to_string(),
            name: String::new(),
            dir: root_dir(Path::new("out"), &tag_of("AMS", dbno), refno),
            unit_kind: UnitKind::Root,
            parent: None,
        }
    }

    fn zone_job(refno: &str, parent: &str) -> RootJob {
        RootJob {
            noun: "ZONE".to_string(),
            unit_kind: UnitKind::Zone,
            parent: Some(ParentRoot {
                refno: format!("={parent}"),
                noun: "SITE".to_string(),
                name: "/S".to_string(),
            }),
            ..root_job(1112, refno)
        }
    }

    fn options() -> BatchOptions {
        BatchOptions {
            connection: RunInput {
                install_dir: String::new(),
                projects_dir: String::new(),
                project_name: String::new(),
                e3d_project: String::new(),
                username: String::new(),
                password: String::new(),
                e3d_mdb: String::new(),
                refno: String::new(),
            },
            dbnums: vec![7999],
            all_desi: false,
            only: Vec::new(),
            scope: None,
            site_only: false,
            unit: Unit::Root,
            list_units: false,
            out: PathBuf::from("out"),
            resume: false,
            force: false,
            baseline_dir: None,
            stage: Stage::Full,
            in_process: false,
            jobs: 1,
            min_commit_free_gb: 1.5,
            gen_timeout: Duration::from_secs(1),
            e3d_timeout: Duration::from_secs(1),
            fused_compare: false,
            emit_rvm: true,
            store: false,
            compare_memory_mb: 0,
            element_cache: 0,
            emit_compare: false,
            threads: 0,
        }
    }

    fn run_spec() -> RunSpec {
        RunSpec {
            install_dir: PathBuf::new(),
            projects_dir: PathBuf::new(),
            project_name: "AvevaMarineSample".to_string(),
            project_dir: PathBuf::new(),
            project_evar: PathBuf::new(),
            e3d_project: "AMS".to_string(),
            username: String::new(),
            password: String::new(),
            e3d_mdb: "/ALL".to_string(),
            refno: String::new(),
            tools: super::super::resolve_tools(),
        }
    }

    #[test]
    fn summary_sums_counts_and_decides_clean_per_root_status() {
        let job = root_job;
        let mut pass = RootRecord::new(&job(7999, "1/2"));
        pass.status = status::PASS.to_string();
        pass.counts = Some(RvmCounts {
            both: 10,
            exact: 10,
            kind_match: 10,
            ..RvmCounts::default()
        });
        pass.generate = Some(StageRecord {
            exit: 0.into(),
            seconds: 2.5,
            peak_private_mb: Some(900),
            ..StageRecord::default()
        });
        let mut fail = RootRecord::new(&job(7999, "1/3"));
        fail.status = status::FAIL.to_string();
        fail.counts = Some(RvmCounts {
            both: 4,
            exact: 3,
            off: 1,
            kind_match: 4,
            ..RvmCounts::default()
        });
        let opts = options();
        let spec = run_spec();
        let summary = Summary::build(
            &opts,
            &spec,
            &[7999],
            "test",
            "rvm-check-batch/t",
            vec![pass.clone(), fail.clone()],
        );
        assert!(!summary.clean);
        assert_eq!(summary.batch_id, "rvm-check-batch/t");
        assert_eq!(summary.totals.both, 14);
        assert_eq!(summary.totals.off, 1);
        assert_eq!(summary.status_counts.get("pass"), Some(&1));
        assert_eq!(summary.status_counts.get("fail"), Some(&1));
        assert_eq!(summary.dbs.len(), 1);
        assert_eq!(summary.dbs[0].clean_roots, 1);
        assert_eq!(summary.dbs[0].gen_peak_private_mb, 900);
        let table = summary.table();
        assert!(table.contains("NOT CLEAN"));
        assert!(table.contains("=1/3"));

        // 入库事实：run_id 按批次 / tag / 根拼，refno 去掉 `=`，终态映射到三色 + 阶段。
        let facts = pass.run_facts("rvm-check-batch/t", &spec);
        assert_eq!(facts.run_id, "rvm-check-batch/t/ams7999/1_2");
        assert_eq!(facts.refno, "1/2");
        assert_eq!(facts.verdict, "pass");
        assert_eq!(facts.project, "AvevaMarineSample");
        assert_eq!(facts.gen_peak_private_mb, Some(900));
        assert_eq!(facts.seconds.generate, 2.5);
        let mut guarded = RootRecord::new(&job(7999, "1/4"));
        guarded.status = status::COMMIT_GUARD.to_string();
        guarded.message = "x".to_string();
        guarded.generate = Some(StageRecord {
            exit: status::COMMIT_GUARD.into(),
            message: "可提交内存余量低于 1.5 GB，已杀掉生成子进程".to_string(),
            ..StageRecord::default()
        });
        let facts = guarded.run_facts("b", &spec);
        assert_eq!(facts.verdict, "fail");
        assert_eq!(facts.stage, "generate_ours");
        assert!(facts.message.contains("可提交内存"));
        let (row, rows) = super::super::store::build_rows(&facts);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reason_kind, "generate_commit_guard");
        assert_eq!(row.mismatch_by_kind["generate_commit_guard"], 1);
    }

    #[test]
    fn batch_ids_are_the_last_two_path_components() {
        assert_eq!(
            batch_id_for(Path::new(r"D:\x\output\rvm-check-batch\20261107-full")),
            "rvm-check-batch/20261107-full"
        );
        assert_eq!(batch_id_for(Path::new("single")), "single");
    }

    /// 阶段状态机：标记文件缺席时永远停在 Gen（compare 峰值留 None），出现后只前进不回退。
    #[test]
    fn phase_markers_advance_one_way_and_never_without_files() {
        let dir = std::env::temp_dir().join(format!("rvm-phase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ours")).unwrap();
        assert_eq!(Phase::Gen.advance(&dir), Phase::Gen, "没有标记就停在 Gen");
        std::fs::write(dir.join(PHASE_COMPARE_START), b"").unwrap();
        assert_eq!(Phase::Gen.advance(&dir), Phase::Compare);
        assert_eq!(
            Phase::Compare.advance(&dir),
            Phase::Compare,
            "只到 start 不进 Done"
        );
        std::fs::write(dir.join(PHASE_COMPARE_END), b"").unwrap();
        assert_eq!(Phase::Compare.advance(&dir), Phase::Done);
        assert_eq!(Phase::Done.advance(&dir), Phase::Done, "不回退");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 子进程退出之后还读得到它的累计峰值：`generate_in_child` 靠这一点在退出后补读一次，
    /// 不足一个采样间隔就跑完的单元才不会只留下刚起进程时读到的 1 MB。
    #[cfg(windows)]
    #[test]
    fn peak_private_bytes_are_still_readable_after_the_child_exits() {
        let mut child = std::process::Command::new("cmd")
            .args(["/d", "/c", "exit 0"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("起 cmd");
        child.wait().expect("等 cmd 退出");
        let peak = memory::process_peak_private_bytes(&child).expect("退出后读峰值");
        assert!(
            peak >= 256 * 1024,
            "cmd 退出后的累计峰值只有 {peak} 字节，不像是整段的峰值"
        );
    }

    /// `--scope` 的解析：两种档、refno 三种写法归一，写错的形态必须报错而不是猜。
    #[test]
    fn scope_arg_parses_both_kinds_and_normalizes_refno() {
        assert_eq!(
            ScopeArg::parse("node:=9304/2").unwrap(),
            ScopeArg::Node("9304/2".to_string())
        );
        assert_eq!(
            ScopeArg::parse("subtree:9304_2").unwrap(),
            ScopeArg::Subtree("9304/2".to_string())
        );
        assert_eq!(
            ScopeArg::parse("Subtree:9304/2").unwrap().to_arg(),
            "subtree:9304/2"
        );
        // 没有冒号 / 档位写错 / refno 解析不出来，三种都当场报错。
        assert!(ScopeArg::parse("9304/2").is_err());
        assert!(ScopeArg::parse("leaf:9304/2").is_err());
        assert!(ScopeArg::parse("node:").is_err());
        assert_eq!(ScopeArg::Node("1/2".to_string()).refno(), "1/2");
    }

    fn r(text: &str) -> RefU64 {
        RefU64::from_str(text).unwrap()
    }

    /// 单元划分：SITE 下按成员原序一个 ZONE 一个单元，ZONE 下不再往下拆；APPLDW 经 APPLDA、
    /// APPDAR 两层才到 ZONE；容器下的非 ZONE 子节点各成一个兜底单元。
    #[test]
    fn units_split_at_zones_and_descend_through_application_containers() {
        let entry = |refno: &str, noun: &str, name: &str| TreeEntry {
            refno: r(refno),
            noun: noun.to_string(),
            name: name.to_string(),
        };
        let tree: std::collections::HashMap<RefU64, Vec<TreeEntry>> =
            std::collections::HashMap::from([
                (
                    r("1/1"),
                    vec![
                        entry("1/2", "ZONE", "/Z1"),
                        entry("1/3", "STRU", "/LOOSE"),
                        entry("1/4", "ZONE", "/Z2"),
                    ],
                ),
                (r("1/2"), vec![entry("1/20", "PIPE", "/P")]),
                (r("2/1"), vec![entry("2/2", "APPLDA", "/A")]),
                (r("2/2"), vec![entry("2/3", "APPDAR", "/D")]),
                (r("2/3"), vec![entry("2/4", "ZONE", "/Z3")]),
            ]);
        let mut children = |refno: RefU64| -> Result<Vec<TreeEntry>> {
            Ok(tree.get(&refno).cloned().unwrap_or_default())
        };
        let site = collect_units(r("1/1"), &mut children).unwrap();
        assert_eq!(
            site.iter()
                .map(|unit| (unit.noun.as_str(), unit.name.as_str()))
                .collect::<Vec<_>>(),
            vec![("ZONE", "/Z1"), ("STRU", "/LOOSE"), ("ZONE", "/Z2")]
        );
        assert_eq!(
            collect_units(r("2/1"), &mut children).unwrap(),
            vec![entry("2/4", "ZONE", "/Z3")]
        );
        assert!(collect_units(r("3/1"), &mut children).unwrap().is_empty());

        // 容器套自己：报错，不死循环。
        let mut looped =
            |_: RefU64| -> Result<Vec<TreeEntry>> { Ok(vec![entry("9/1", "APPLDA", "")]) };
        assert!(collect_units(r("9/1"), &mut looped).is_err());
    }

    /// `--only`：根作业只认自己的 refno；单元作业还认它的上级根（点名 SITE 就是它下面的全部单元）。
    #[test]
    fn only_picks_units_by_their_own_or_their_parent_refno() {
        let zone = zone_job("17496/8518", "9304/2");
        assert!(only_keeps(&zone, &["17496/8518".to_string()]));
        assert!(only_keeps(&zone, &["9304/2".to_string()]));
        assert!(!only_keeps(&zone, &["9304/3".to_string()]));
        let root = root_job(1112, "9304/2");
        assert!(only_keeps(&root, &["9304/2".to_string()]));
        assert!(!only_keeps(&root, &["17496/8518".to_string()]));
    }

    /// `--unit zone` 固定生成即比对；`--emit-rvm` 不给时融合不写 generated.rvm、不融合照写；
    /// 显式给了就照办。
    #[test]
    fn zone_mode_forces_fused_and_fused_drops_the_rvm_by_default() {
        assert_eq!(fused_and_emit(Unit::Root, false, None), (false, true));
        assert_eq!(fused_and_emit(Unit::Root, true, None), (true, false));
        assert_eq!(fused_and_emit(Unit::Root, true, Some(true)), (true, true));
        assert_eq!(fused_and_emit(Unit::Zone, false, None), (true, false));
        assert_eq!(fused_and_emit(Unit::Zone, false, Some(true)), (true, true));
    }

    /// 参数组合：ZONE 模式只有 full / export 两档且必须融合；融合不再要求 `--baseline-dir`；
    /// 不融合时不能关掉 generated.rvm。
    #[test]
    fn mode_checks_reject_only_the_combinations_that_cannot_work() {
        let with = |edit: fn(&mut BatchOptions)| {
            let mut opts = options();
            edit(&mut opts);
            check_modes(&opts)
        };
        assert!(with(|_| {}).is_ok());
        assert!(with(|opts| opts.fused_compare = true).is_ok());
        assert!(
            with(|opts| {
                opts.fused_compare = true;
                opts.stage = Stage::Generate;
            })
            .is_err()
        );
        assert!(with(|opts| opts.emit_rvm = false).is_err());
        assert!(
            with(|opts| {
                opts.unit = Unit::Zone;
                opts.fused_compare = true;
                opts.emit_rvm = false;
            })
            .is_ok()
        );
        assert!(
            with(|opts| {
                opts.unit = Unit::Zone;
                opts.fused_compare = true;
                opts.stage = Stage::Export;
            })
            .is_ok()
        );
        assert!(
            with(|opts| {
                opts.unit = Unit::Zone;
                opts.fused_compare = true;
                opts.stage = Stage::Compare;
            })
            .is_err()
        );
        assert!(with(|opts| opts.unit = Unit::Zone).is_err());
    }

    /// 生成即比对先领基准：缺基准、或记为就位但盘上没有，都不开工；在盘上才组装融合参数。
    #[test]
    fn fused_generation_starts_only_with_a_baseline_on_disk() {
        let dir = std::env::temp_dir().join(format!("rvm-fused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let job = RootJob {
            dir: dir.clone(),
            ..zone_job("17496/8518", "9304/2")
        };
        let mut opts = options();
        opts.unit = Unit::Zone;
        opts.fused_compare = true;
        opts.emit_rvm = false;
        opts.compare_memory_mb = 64;

        let missing = ExportOutcome::Missing(StageRecord::new(
            "missing",
            Instant::now(),
            "基准目录里没有 x",
        ));
        assert_eq!(
            fused_from(&missing, &job, &opts).err().as_deref(),
            Some("基准目录里没有 x")
        );
        let ready = ExportOutcome::Ready(StageRecord::new("batch", Instant::now(), ""));
        assert!(fused_from(&ready, &job, &opts).is_err());

        std::fs::write(dir.join("e3d.rvm"), b"baseline").unwrap();
        let fused = fused_from(&ready, &job, &opts).unwrap();
        assert_eq!(fused.baseline, dir.join("e3d.rvm"));
        assert!(!fused.emit_rvm);
        assert_eq!(fused.window_bytes, 64 * 1024 * 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `run.json`：根作业不多出单元字段（老产物不变），单元作业带上种类与上级根；
    /// 没有这两个字段的老 `run.json` 照样读得回（续跑）。
    #[test]
    fn run_json_carries_unit_fields_only_for_units_and_reads_old_records() {
        let root = serde_json::to_value(RootRecord::new(&root_job(7999, "1/2"))).unwrap();
        assert!(root.get("unit").is_none());
        assert!(root.get("parent").is_none());

        let mut zone =
            serde_json::to_value(RootRecord::new(&zone_job("17496/8518", "9304/2"))).unwrap();
        assert_eq!(zone["unit"], "zone");
        assert_eq!(zone["parent"]["ref"], "=9304/2");

        let object = zone.as_object_mut().unwrap();
        object.remove("unit");
        object.remove("parent");
        let old: RootRecord = serde_json::from_value(zone).unwrap();
        assert_eq!(old.unit, UnitKind::Root);
        assert!(old.parent.is_none());
    }

    fn off(name: &str) -> RvmMismatch {
        RvmMismatch {
            category: "off".to_string(),
            name: name.to_string(),
            noun: "PIPE".to_string(),
            max_axis_delta_mm: Some(3.0),
            baseline_kinds: Vec::new(),
            generated_kinds: Vec::new(),
            baseline_aabb: None,
            generated_aabb: None,
            reason_kind: String::new(),
            reason: String::new(),
            severity: String::new(),
        }
    }

    /// 入库事实带单元种类与上级根：根作业的上级根是它自己，单元作业是它的 SITE / APPLDW；
    /// 差异行跟着带上级根，按 SITE 查差异不必先查运行行。
    #[test]
    fn stored_facts_name_the_unit_kind_and_its_root() {
        let spec = run_spec();
        let root = RootRecord::new(&RootJob {
            name: "/1RS-CIVI".to_string(),
            ..root_job(1112, "9304/2")
        });
        let facts = root.run_facts("b", &spec);
        assert_eq!(facts.unit.as_deref(), Some("root"));
        assert_eq!(facts.root_refno.as_deref(), Some("9304/2"));
        assert_eq!(facts.root_noun.as_deref(), Some("SITE"));
        assert_eq!(facts.root_name.as_deref(), Some("/1RS-CIVI"));

        let mut zone = RootRecord::new(&zone_job("17496/8518", "9304/2"));
        zone.status = status::FAIL.to_string();
        zone.mismatches_all = vec![off("/P1")];
        let facts = zone.run_facts("b", &spec);
        assert_eq!(facts.run_id, "b/ams1112/17496_8518");
        assert_eq!(facts.refno, "17496/8518");
        assert_eq!(facts.unit.as_deref(), Some("zone"));
        assert_eq!(facts.root_refno.as_deref(), Some("9304/2"));
        assert_eq!(facts.root_noun.as_deref(), Some("SITE"));
        assert_eq!(facts.root_name.as_deref(), Some("/S"));
        let (row, rows) = super::super::store::build_rows(&facts);
        assert_eq!(row.unit.as_deref(), Some("zone"));
        assert_eq!(row.root_refno.as_deref(), Some("9304/2"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].refno, "17496/8518");
        assert_eq!(rows[0].root_refno.as_deref(), Some("9304/2"));
    }

    /// 续跑补入库要全量的差异清单：`run.json` 里那份截断过时，读文件比对的作业从 `compare.json` 重判，
    /// 生成即比对的作业读它留下的 `judgement.json`（两个都在时用后者）；清单本来就全的不去读大文件。
    #[test]
    fn resumed_facts_take_the_full_list_from_the_job_directory() {
        let dir = std::env::temp_dir().join(format!("rvm-resumed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut record = RootRecord::new(&RootJob {
            dir: dir.clone(),
            ..zone_job("17496/8518", "9304/2")
        });
        record.status = status::FAIL.to_string();
        record.mismatches = vec![off("/A")];
        let spec = run_spec();
        assert_eq!(resumed_facts(&record, "b", &spec).mismatches.len(), 1);

        // 读文件比对：只有 compare.json。清单没截断时不重判，截断过才从它拿全量。
        std::fs::write(
            dir.join("compare.json"),
            serde_json::json!({
                "baseline_members": 4,
                "by_noun": { "PIPE": { "both": 1, "exact": 1, "kind_match": 1, "only_baseline": 3 } },
                "pairs": [],
                "only_baseline": ["/A", "/B", "/C"],
                "only_generated": [],
                "centreline_only_baseline": ["SNOD 1 of SCTN /S", "SNOD 2 of SCTN /S"]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(resumed_facts(&record, "b", &spec).mismatches.len(), 1);
        record.mismatches_truncated = true;
        let facts = resumed_facts(&record, "b", &spec);
        assert_eq!(facts.mismatches.len(), 3);
        assert_eq!(facts.centreline_only_baseline.len(), 2);

        let judgement = super::super::report::Judgement {
            verdict: RvmVerdict::Fail,
            counts: RvmCounts::default(),
            mismatches: vec![off("/A"), off("/B")],
            centreline_only_baseline: vec!["SNOD 1 of SCTN /S".to_string()],
        };
        std::fs::write(
            dir.join("judgement.json"),
            serde_json::to_vec(&judgement).unwrap(),
        )
        .unwrap();
        let facts = resumed_facts(&record, "b", &spec);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(facts.run_id, record.run_id("b"));
        assert_eq!(facts.mismatches.len(), 2);
        assert_eq!(facts.centreline_only_baseline.len(), 1);
    }

    fn element_check(refno: &str) -> super::super::element::ElementCheck {
        serde_json::from_value(serde_json::json!({
            "refno": refno, "noun": "ELBO", "name": "/A", "grade": "exact", "severity": "ok"
        }))
        .unwrap()
    }

    /// 构件级结果记在作业自己这个单元名下（按 ZONE 跑是那个 ZONE，上级根是它的 SITE；整根跑单元就是根）；
    /// `--scope` 只比了一块时带上范围，选中的就是整个作业时按整个单元算。交给入库线程是整份搬走，
    /// 记录里不再留；`run.json` 只记范围与条数。
    #[test]
    fn element_facts_are_filed_under_the_job_and_handed_over_whole() {
        let spec = run_spec();
        let mut zone = RootRecord::new(&zone_job("17496/8518", "9304/2"));
        zone.status = status::PASS.to_string();
        zone.elements = Some(vec![element_check("1/1"), element_check("1/2")]);
        zone.element_checks = Some(2);
        assert!(
            zone.run_facts("b", &spec).elements.is_none(),
            "不带构件级结果的那一份不复制它们"
        );
        let elements = zone
            .take_run_facts("b", &spec)
            .elements
            .expect("比对出了结果就有构件级结果");
        assert_eq!(elements.checks.len(), 2);
        assert_eq!(elements.unit_refno, "17496/8518");
        assert_eq!(elements.unit_kind, "zone");
        assert_eq!(elements.root_refno.as_deref(), Some("9304/2"));
        assert_eq!(elements.scope, None);
        assert!(zone.elements.is_none(), "交出去就不留");
        assert!(zone.take_run_facts("b", &spec).elements.is_none());
        let json = serde_json::to_value(&zone).unwrap();
        assert_eq!(json["element_checks"], 2);
        assert!(json.get("elements").is_none() && json.get("scope").is_none());

        let scoped = |scope: &str| {
            let mut root = RootRecord::new(&root_job(1112, "9304/2"));
            root.elements = Some(vec![element_check("1/1")]);
            root.scope = Some(scope.to_string());
            let json = serde_json::to_value(&root).unwrap();
            assert_eq!(json["scope"], scope);
            // 续跑读回来的记录照样带着范围。
            let mut back: RootRecord = serde_json::from_value(json).unwrap();
            back.elements = root.elements.take();
            back.take_run_facts("b", &spec).elements.unwrap()
        };
        let elements = scoped("subtree:17496/9000");
        assert_eq!(elements.unit_refno, "9304/2");
        assert_eq!(elements.unit_kind, "root");
        assert_eq!(elements.root_refno.as_deref(), Some("9304/2"), "根的上级根是它自己");
        assert_eq!(
            elements.scope,
            Some(super::super::store::ElementScope {
                kind: "subtree".to_string(),
                refno: "17496/9000".to_string()
            })
        );
        assert_eq!(scoped("node:17496/9000").scope.unwrap().kind, "node");
        assert_eq!(scoped("subtree:9304/2").scope, None, "选的就是整个作业");
        assert!(
            scoped("node:9304/2").scope.is_some(),
            "只比根节点自己不等于整个根"
        );
    }

    /// 续跑补入库：构件级结果从作业目录的 `element-checks.json` 取；没有那份文件的老产物目录补不了构件行。
    #[test]
    fn resumed_facts_take_element_checks_from_the_job_directory() {
        let dir = std::env::temp_dir().join(format!("rvm-resumed-elements-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut record = RootRecord::new(&RootJob {
            dir: dir.clone(),
            ..zone_job("17496/8518", "9304/2")
        });
        record.status = status::PASS.to_string();
        let spec = run_spec();
        assert!(
            resumed_facts(&record, "b", &spec).elements.is_none(),
            "老产物目录：没有构件级结果"
        );

        let checks = vec![element_check("1/1"), element_check("1/2"), element_check("1/3")];
        super::super::element::write_checks(&dir, &checks).unwrap();
        let facts = resumed_facts(&record, "b", &spec);
        let _ = std::fs::remove_dir_all(&dir);
        let elements = facts.elements.expect("从作业目录读回来");
        assert_eq!(elements.checks, checks);
        assert_eq!(elements.unit_refno, "17496/8518");
        assert_eq!(elements.root_refno.as_deref(), Some("9304/2"));
    }

    /// 真库上对一遍：页面单根把构件行记到哪个单元名下（`page_element_facts`），与这里划分出来的单元是不是
    /// 同一个答案。属主链的读法（节点自身在前、链头是 WORLD、名词的写法）只有真树验得了。
    /// 每个上级根查它自己、它下面的容器层，再取头、中、尾三个单元，各自往下钻到叶子。
    #[test]
    #[ignore = "requires local AMS source files"]
    fn ams_page_nodes_are_filed_under_the_units_enumerated_here() {
        let tree = DirectTreeService::open("AvevaMarineSample", "ALL").unwrap();
        let out = Path::new("unused");
        let filed = |refno: &str| {
            let facts = super::super::page_element_facts(&tree, refno, Vec::new());
            (
                facts.unit_refno,
                facts.unit_kind,
                facts.root_refno,
                facts.scope.map(|scope| (scope.kind, scope.refno)),
            )
        };
        let expect = |unit: &str, kind: &str, root: &str, scope: Option<&str>| {
            (
                unit.to_string(),
                kind.to_string(),
                Some(root.to_string()),
                scope.map(|refno| ("subtree".to_string(), refno.to_string())),
            )
        };
        let slash = |refno: RefU64| format!("{}/{}", refno.get_0(), refno.get_1());
        let (mut roots_seen, mut containers_seen, mut units_seen, mut below_seen) = (0, 0, 0, 0);
        for dbno in [1112u32, 7333, 7999, 8000] {
            let roots = enumerate_roots(&tree, dbno, false, "AMS", out).unwrap();
            assert!(!roots.is_empty(), "库 {dbno} 没有根");
            for root in &roots {
                assert_eq!(
                    filed(&root.refno),
                    expect(&root.refno, "root", &root.refno, None),
                    "根 ={}",
                    root.refno
                );
                roots_seen += 1;
                // 根与单元之间的容器层（APPLDW 下的 APPLDA）：记在根名下，只算核对了一块。
                let start = RefU64::from_str(&root.refno).unwrap();
                for child in tree.children(start).unwrap() {
                    let noun = child.noun.trim().to_ascii_uppercase();
                    if UNIT_CONTAINER_NOUNS.contains(&noun.as_str()) {
                        let refno = slash(child.refno.refno());
                        assert_eq!(
                            filed(&refno),
                            expect(&root.refno, "root", &root.refno, Some(&refno)),
                            "{noun} ={refno}"
                        );
                        containers_seen += 1;
                    }
                }
                let units = units_of(&tree, std::slice::from_ref(root), out).unwrap();
                let picks = [0, units.len() / 2, units.len().saturating_sub(1)];
                for (at, unit) in units.iter().enumerate() {
                    if !picks.contains(&at) {
                        continue;
                    }
                    let kind = unit.unit_kind.as_str();
                    assert_eq!(
                        filed(&unit.refno),
                        expect(&unit.refno, kind, &root.refno, None),
                        "单元 ={}",
                        unit.refno
                    );
                    units_seen += 1;
                    let mut cursor = RefU64::from_str(&unit.refno).unwrap();
                    for _ in 0..8 {
                        let Some(child) = tree.children(cursor).unwrap().into_iter().next() else {
                            break;
                        };
                        cursor = child.refno.refno();
                        let refno = slash(cursor);
                        assert_eq!(
                            filed(&refno),
                            expect(&unit.refno, kind, &root.refno, Some(&refno)),
                            "{} ={refno}，单元 ={}",
                            child.noun,
                            unit.refno
                        );
                        below_seen += 1;
                    }
                }
            }
        }
        eprintln!(
            "根 {roots_seen}，根下的容器层 {containers_seen}，单元 {units_seen}，单元之下的节点 {below_seen}"
        );
        assert!(containers_seen > 0 && units_seen > 0 && below_seen > 0);
    }
}
