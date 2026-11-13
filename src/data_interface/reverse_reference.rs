//! 反向引用查询（「联动查询」）：目录件变了 → 谁引用它 → 上卷到生成根。
//!
//! 照 core 的 `DB_MDB::findElements(dbType, refAttr, target)`（core.dll `0x59f4720`）：**逐库**、
//! 按目标 refno 在文件自带的反向引用表里**点查**（`db_start_ref_table_search` / e3d-io
//! `ReadOnlyEngine::referrers_of`），跨库并集。不扫库、不建内存表、不读 `ref_rev`
//! （共识 d-635：`ref_rev` 降级为 UI 便利，选根代码禁读）。
//!
//! 两段式（`docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md` P3；
//! 表→noun 的对应关系是 783 库语料实测，见 `docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md`）：
//!
//! 1. **CATA 侧闭包**：种子 = 变动目录件 **∪ 它们在目录里的祖先**（一个 `SDTE` 变了，真正被引用的是
//!    它属主 `DTSE`；`SBOX` 变了，被引用的是 `GMSE`）。对 MDB 里每个目录库的每张反向表点查种子，
//!    得到目录中间件（`SCOM`/`SFIT` ← `CATR` 表的 `SPCO`、`GMSE` ← `GMRE` 表的 `SCOM`…），再点查它们，
//!    直到集合不增长（共识 d-665：不设深度上限）。中间件**只走过、不成根**。
//! 2. **DESI 侧一跳**：对 MDB 里每个设计库的每张反向表点查闭包里的全部目标
//!    （`SPRE`/`HSTU`/`LSTU` 指 `SPCO`、`ISPE` 指 `SPEC`、`CATR` 指 `TABITE`），得到设计引用者，
//!    再经 [`crate::data_interface::generation_root::resolve_element_generation_root`] 上卷到根。
//!
//! **退路 (ii)（共识 d-665）**：某个目录库**一张反向表都没有**（`ams7351_0001` 那种）时，它里面
//! 属「设计侧可直接引用」层（[`DESIGN_REFERABLE_NOUNS`]）的种子照常走第 2 段——DESI 侧的表在**设计库**
//! 文件里，与目录库有没有表无关；属目录内部层（`SCOM`/`GMSE`/`DTSE`…及一切拿不准的 noun）的种子
//! 算不出「谁在目录里引用它」，该目录库整体退化（[`ReverseCascade::degraded`]），由调用方把它的
//! 全部依赖根判过期并点名 `catalogue_cascade_degraded`。**宁多算不漏算。**
//!
//! 纯文件侧：输入是 MDB 成员的 `(dbnum, db_type, file, sesno)` 与种子，不碰 SurrealDB；上卷到根
//! 要读设计库的 owner 链，由调用方注入 `resolve_root`（生产用 `E3dModelService::build_set`，
//! 测试可以直接给 `DbSet`），本模块不认识配置。阻塞 I/O。
//!
//! # 两个入口
//!
//! | 入口 | 种子 | 走哪些库 | 落在哪 |
//! |---|---|---|---|
//! | [`expand_reverse_cascade`]（P3） | 变动**目录**件 | CATA 侧闭包 + 全部 DESI 库一跳 | 启动目录复核 `reconcile_catalogue_versions` |
//! | [`DesignReferenceCascade`]（P4） | 一窗的变更**设计**元素 | MDB 每个 DESI 库一跳（逐库并集） | 稳态窗口规划器 `WindowRootSources` |
//!
//! 两者共用同一份「哪些表是反向引用表」的判据（[`reference_tables_of`]）与同一条点查
//! （`ReadOnlyEngine::referrers_of`）；分开的是种子从哪来、结果排给谁。
//!
//! # 引用者读的是被引用件身上的格（GitHub #143）
//!
//! 多数引用边的引用者只关心被引用件「在哪 / 在不在」。[`DESIGN_CASCADE_TARGET_READS`] 上的不一样：引用者的几何读的是
//! 被引用件**身上的某几格**——支撑件经 `COMPRE` 找到被支撑的构件，取它的 `LSTU` / `HSTU` 走到管子的目录件要外径。
//! 两个入口各补一步：P4 只在那几格窗口两端取值不同时才让引用者重算（同库的上卷成根，别的库里的点名交给
//! [`crate::data_interface::cross_db_handoff`]）；P3 的设计侧多走一跳——经那几张表命中的设计件，再问一次谁经
//! 名单上的属性指着它。P4 超点查预算的窗（一窗动了上千个元素）这一类不跟着整窗放弃：名单上的反向表整张走一遍
//! （成本只跟表的大小有关），别的库里的引用者照常点名——本库的根有整窗重算兜底，别的库里的没有。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_core::RefnoEnum;
use anyhow::Context;
use e3d_attlib::db1_dehash;
use e3d_io::record::element::ParsedElement;
use e3d_io::{DbSet, ReadOnlyEngine, RefNo};
use e3d_model::increment::UpdatePlan;
use serde::{Deserialize, Serialize};

use crate::data_interface::generation_root::{
    GenerationNode, GenerationRoot, refno_from_e3d, refno_to_e3d, resolve_element_generation_root,
};
use crate::data_interface::structural_connection::roots_on_owner_chain;

/// 设计侧反向表**直接**指向的目录 noun（783 库语料：`SPRE`/`HSTU`/`LSTU` → `SPCO`，`ISPE` → `SPEC`，
/// 设计库里的 `CATR` → `TABITE`）。退路 (ii) 只放行这一层——它们不需要目录内部闭包也能在设计库
/// 的表里被点查到。不在名单里的 noun 一律当目录内部层处理（拿不准就退化，不是放过）。
pub const DESIGN_REFERABLE_NOUNS: &[&str] = &["SPCO", "SPEC", "TABITE"];

/// 目录祖先链最多走多少层——目录树很浅（`SDTE` → `DTSE` → `DTAB`/`SECT` → `CATA` → `WORL`），
/// 超过就是 owner 链成环或坏数据，停下来记账而不是转圈。
const MAX_ANCESTOR_DEPTH: usize = 32;

/// MDB 的一个成员库：反查在哪些文件里做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDb {
    pub dbnum: u32,
    /// `DESI` / `CATA` / …；只有这两类参与反查，其它类型跳过。
    pub db_type: String,
    pub file: PathBuf,
    /// 钉住的会话；`None` = 文件最新。目录库一般钉最新（窗口的 T 端），设计库钉当前 pin。
    pub sesno: Option<u32>,
}

impl SourceDb {
    fn is_design(&self) -> bool {
        self.db_type.eq_ignore_ascii_case("DESI")
    }

    fn is_catalogue(&self) -> bool {
        self.db_type.eq_ignore_ascii_case("CATA")
    }
}

/// 一个变动目录件（P2 的输出；`noun` 读不出来也收，闭包会再读一次记录头）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueSeed {
    pub refno: RefNo,
    pub noun: Option<String>,
}

/// 一次反查的全部产物与账目。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseCascade {
    /// 种子（变动目录件）。
    pub seeds: Vec<String>,
    /// 目录侧闭包：种子 ∪ 种子祖先 ∪ 经目录反向表逐跳找到的中间件。**不成根**。
    pub catalogue_closure: BTreeSet<String>,
    /// 设计引用者，按设计库分。
    pub design_referrers: BTreeMap<u32, BTreeSet<String>>,
    /// 上卷到的生成根，按设计库分、按 refno 去重。
    pub roots: BTreeMap<u32, Vec<CascadeRoot>>,
    /// 上卷不到任何生成根的设计引用者（连粗层级都不是——多半是 `WORL`/`SITE` 直属或非模型件），
    /// 按设计库分。它们不排工作，但要看得见。
    pub unrooted: BTreeMap<u32, Vec<String>>,
    /// 退化的目录库 → 原因（退路 (ii)：零表 + 有目录内部层的种子）。
    pub degraded: BTreeMap<u32, String>,
    /// 每个库用到的反向表名（升序）——`表在就查`，这里记的是真被点查过的。
    pub tables_used: BTreeMap<u32, Vec<String>>,
    /// 点查总次数（成本账）。
    pub point_queries: usize,
    /// 读不出记录头的目录件（祖先链就此中断，其余照常）。
    pub unreadable: Vec<String>,
    /// 再一跳找到的引用者，按设计库分（GitHub #143；它们也在 `design_referrers` 里、一样上卷）：经
    /// [`DESIGN_CASCADE_TARGET_READS`] 里被读的那几张表（`LSTU` / `HSTU`）命中的设计件，各自又被谁经名单上的
    /// 属性（`COMPRE`）指着——支撑件不直接引用管子的目录件，第 2 段那一跳点不到它。
    #[serde(default)]
    pub dependent_referrers: BTreeMap<u32, BTreeSet<String>>,
}

/// 一个被反查命中的生成根。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CascadeRoot {
    pub refno: String,
    pub noun: String,
    pub name: String,
}

impl ReverseCascade {
    pub fn root_count(&self) -> usize {
        self.roots.values().map(Vec::len).sum()
    }

    pub fn referrer_count(&self) -> usize {
        self.design_referrers.values().map(BTreeSet::len).sum()
    }

    /// 横幅一行。
    pub fn summary_line(&self) -> String {
        let roots = self
            .roots
            .iter()
            .map(|(dbnum, roots)| format!("{dbnum}:{}", roots.len()))
            .collect::<Vec<_>>()
            .join(" ");
        let degraded = if self.degraded.is_empty() {
            String::new()
        } else {
            format!(
                "；退化目录库 {:?}（该库依赖根全部过期）",
                self.degraded.keys().collect::<Vec<_>>()
            )
        };
        let dependent_count: usize = self.dependent_referrers.values().map(BTreeSet::len).sum();
        let dependents = if dependent_count == 0 {
            String::new()
        } else {
            format!("（其中 {dependent_count} 个是再一跳找到的）")
        };
        format!(
            "反查：种子 {} → 目录闭包 {} → 设计引用者 {}{dependents} → 生成根 {}{}（点查 {} 次{}{}）{degraded}",
            self.seeds.len(),
            self.catalogue_closure.len(),
            self.referrer_count(),
            self.root_count(),
            if roots.is_empty() {
                String::new()
            } else {
                format!(" [{roots}]")
            },
            self.point_queries,
            if self.unrooted.values().any(|v| !v.is_empty()) {
                format!(
                    "，{} 个引用者上卷不到根",
                    self.unrooted.values().map(Vec::len).sum::<usize>()
                )
            } else {
                String::new()
            },
            if self.unreadable.is_empty() {
                String::new()
            } else {
                format!("，{} 个目录件记录头读不出", self.unreadable.len())
            },
        )
    }
}

/// 一次启动目录反查的回执 / `/health` 形状：只有数与库号，不带逐件清单
/// （清单在日志里；`/health` 与 [`crate::data_interface::cata_cascade_window::CatalogueWindowReport`]
/// 同一纪律）。前半是 [`ReverseCascade`] 的账，后半是调用方据此排了什么。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueCascadeReport {
    /// 进反查的种子数：过期目录库窗口里的变动目录件，退化库的不算。
    pub seeds: usize,
    /// 目录侧闭包大小（种子 ∪ 祖先 ∪ 中间件）。
    pub catalogue_closure: usize,
    /// 设计引用者总数。
    pub design_referrers: usize,
    /// 上卷到的生成根数，按设计库。
    pub roots: BTreeMap<u32, usize>,
    /// 上卷不到根的设计引用者数。
    pub unrooted: usize,
    /// 退化的目录库 → 原因。窗口收不了 / 导入级窗口 / 变动件数超阈 / 零反向表且有目录内部层
    /// 种子 / 所在目录库不在 MDB 里，都在这里；这些库的**依赖根全部**按过期重排。
    pub degraded: BTreeMap<u32, String>,
    /// 每库真被点查过的反向表名。
    pub tables_used: BTreeMap<u32, Vec<String>>,
    /// 点查总次数（成本账）。
    pub point_queries: usize,
    /// 记录头读不出的目录件数。
    pub unreadable: usize,
    /// 反查命中、且所在设计库的**库级**目录落款仍在旧目录上（ADR-079）、被排进 `RegenRoot` 的根数，按设计库。
    pub enqueued: BTreeMap<u32, usize>,
    /// 退化目录库波及的设计库被整库排进 `RegenRoot` 的根数，按设计库（与上一栏不重复计）。
    pub enqueued_degraded: BTreeMap<u32, usize>,
    /// 反查命中、但所在设计库的落款已在新目录上（重启前 / 上一次核对已经重算过）的根数——不重排。
    pub already_current: usize,
    /// 落款仍旧、却一根都没被命中的设计库：只排一条盖落款的空名单 ModelOnly（ADR-079），升序。
    #[serde(default)]
    pub stamp_only: Vec<u32>,
    /// 2026-09-15 之前：反查命中、却没有 `gen_root` 行的根数（从未生成过，懒生成）。逐根行随 ADR-079
    /// 退役后这一格分不出来了，恒 0；字段留着不动 `/health` 的形状。
    pub lazy_roots: usize,
    /// 命中却因监听限定域 / 发布队列阻断而跳过的根数。
    pub skipped_out_of_scope: usize,
    /// 落款仍旧、却被监听限定（`watch_dbnums`）挡在排工之外的设计库，升序（GitHub #67）。核对已先按限定域
    /// 分过一次（`model_coverage::split_libraries_by_watch_scope`），这里本该恒空；非空就是有谁把域外的落款送了
    /// 进来——说出来，不静默。
    #[serde(default)]
    pub libraries_out_of_scope: Vec<u32>,
    /// 反查整体没跑成的原因（MDB 成员读不到、某个库文件打不开……）。此时上面全 0、
    /// 什么都没排，受影响的根只剩按需 `ensure` 的目录指纹失配兜底——是**未知**，不是零命中。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CatalogueCascadeReport {
    /// 反查的账先落进来；排工那几格由调用方随后填。
    pub fn from_cascade(cascade: &ReverseCascade) -> Self {
        Self {
            seeds: cascade.seeds.len(),
            catalogue_closure: cascade.catalogue_closure.len(),
            design_referrers: cascade.referrer_count(),
            roots: cascade
                .roots
                .iter()
                .map(|(dbnum, roots)| (*dbnum, roots.len()))
                .collect(),
            unrooted: cascade.unrooted.values().map(Vec::len).sum(),
            degraded: cascade.degraded.clone(),
            tables_used: cascade.tables_used.clone(),
            point_queries: cascade.point_queries,
            unreadable: cascade.unreadable.len(),
            ..Self::default()
        }
    }

    pub fn failed(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::default()
        }
    }

    /// 按目录文件分组核对（GitHub #47）的几次反查并成一份回执：数相加，按库的表并起来，
    /// 退化原因与错误都留着（同一个目录库两组都退化时留先到的那句）。
    pub fn absorb(&mut self, other: CatalogueCascadeReport) {
        let add = |mine: &mut BTreeMap<u32, usize>, theirs: BTreeMap<u32, usize>| {
            for (dbnum, count) in theirs {
                *mine.entry(dbnum).or_default() += count;
            }
        };
        self.seeds += other.seeds;
        self.catalogue_closure += other.catalogue_closure;
        self.design_referrers += other.design_referrers;
        add(&mut self.roots, other.roots);
        self.unrooted += other.unrooted;
        for (dbnum, reason) in other.degraded {
            self.degraded.entry(dbnum).or_insert(reason);
        }
        for (dbnum, tables) in other.tables_used {
            let kept = self.tables_used.entry(dbnum).or_default();
            kept.extend(tables);
            kept.sort();
            kept.dedup();
        }
        self.point_queries += other.point_queries;
        self.unreadable += other.unreadable;
        add(&mut self.enqueued, other.enqueued);
        add(&mut self.enqueued_degraded, other.enqueued_degraded);
        self.already_current += other.already_current;
        self.stamp_only.extend(other.stamp_only);
        self.stamp_only.sort_unstable();
        self.stamp_only.dedup();
        self.lazy_roots += other.lazy_roots;
        self.skipped_out_of_scope += other.skipped_out_of_scope;
        self.libraries_out_of_scope
            .extend(other.libraries_out_of_scope);
        self.libraries_out_of_scope.sort_unstable();
        self.libraries_out_of_scope.dedup();
        self.error = match (self.error.take(), other.error) {
            (Some(mine), Some(theirs)) => Some(format!("{mine}；{theirs}")),
            (mine, theirs) => mine.or(theirs),
        };
    }

    pub fn enqueued_total(&self) -> usize {
        self.enqueued.values().sum::<usize>() + self.enqueued_degraded.values().sum::<usize>()
    }

    /// 有没有哪个目录库这次没能精确回答（退化）或整个反查没跑成。
    pub fn is_degraded(&self) -> bool {
        self.error.is_some() || !self.degraded.is_empty()
    }

    /// 横幅一行。
    pub fn banner_line(&self) -> String {
        if let Some(error) = &self.error {
            return format!(
                "目录反查没跑成（受影响的根**未知**，本次一根不排，只剩按需 ensure 的目录指纹失配兜底）：{error}"
            );
        }
        let per_db = |map: &BTreeMap<u32, usize>| {
            map.iter()
                .map(|(dbnum, n)| format!("{dbnum}:{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut line = format!(
            "目录反查：种子 {} → 目录闭包 {} → 设计引用者 {} → 生成根 {}（点查 {} 次）；排 RegenRoot {} 根",
            self.seeds,
            self.catalogue_closure,
            self.design_referrers,
            self.roots.values().sum::<usize>(),
            self.point_queries,
            self.enqueued_total(),
        );
        if !self.enqueued.is_empty() {
            line.push_str(&format!(" [命中 {}]", per_db(&self.enqueued)));
        }
        if !self.enqueued_degraded.is_empty() {
            line.push_str(&format!(
                " [退化库依赖 {}]",
                per_db(&self.enqueued_degraded)
            ));
        }
        if self.already_current > 0 {
            line.push_str(&format!(
                "，{} 根所在库落款已在新目录上不重排",
                self.already_current
            ));
        }
        if !self.stamp_only.is_empty() {
            line.push_str(&format!(
                "，{} 个库没被波及、只排盖落款的空任务 {:?}",
                self.stamp_only.len(),
                self.stamp_only
            ));
        }
        if self.lazy_roots > 0 {
            line.push_str(&format!(
                "，{} 根从未生成过（懒生成，不排）",
                self.lazy_roots
            ));
        }
        if self.skipped_out_of_scope > 0 {
            line.push_str(&format!(
                "，{} 根在限定域外 / 被阻断跳过",
                self.skipped_out_of_scope
            ));
        }
        if !self.libraries_out_of_scope.is_empty() {
            line.push_str(&format!(
                "，{} 个库落款仍旧却在 watch_dbnums 限定域外、没排 {:?}",
                self.libraries_out_of_scope.len(),
                self.libraries_out_of_scope
            ));
        }
        if self.unrooted > 0 {
            line.push_str(&format!("，{} 个引用者上卷不到根", self.unrooted));
        }
        if self.unreadable > 0 {
            line.push_str(&format!("，{} 个目录件记录头读不出", self.unreadable));
        }
        if !self.degraded.is_empty() {
            line.push_str(&format!(
                "；退化目录库 {:?}（依赖根全部过期）",
                self.degraded.keys().collect::<Vec<_>>()
            ));
        }
        line
    }
}

/// 一个打开的库：引擎 + 通过判据的反向表清单。
struct OpenDb {
    dbnum: u32,
    engine: ReadOnlyEngine,
    /// `(表名, name_hash)`，升序。空 = 该库没有任何反向表。
    tables: Vec<(String, u32)>,
}

impl OpenDb {
    /// 打开并读出通过判据的反向表清单。
    fn open(source: &SourceDb) -> anyhow::Result<Self> {
        let mut engine = match source.sesno {
            Some(sesno) => ReadOnlyEngine::open_at(&source.file, sesno),
            None => ReadOnlyEngine::open(&source.file),
        }
        .with_context(|| {
            format!(
                "open dbnum {} ({}) {} at {}",
                source.dbnum,
                source.db_type,
                source.file.display(),
                source
                    .sesno
                    .map_or_else(|| "latest".to_string(), |s| s.to_string())
            )
        })?;
        let tables = reference_tables_of(&mut engine);
        Ok(Self {
            dbnum: source.dbnum,
            engine,
            tables,
        })
    }

    /// 这个库里谁经任何一张表引用 `target`。
    fn referrers_of(&mut self, target: RefNo, queries: &mut usize) -> anyhow::Result<Vec<RefNo>> {
        let mut out = Vec::new();
        for (name, hash) in &self.tables {
            *queries += 1;
            let found = self
                .engine
                .referrers_of(*hash, target)
                .with_context(|| format!("dbnum {} table {name} target {target}", self.dbnum))?;
            out.extend(found);
        }
        Ok(out)
    }

    /// 同 [`Self::referrers_of`]，按表分开给：`(表名, 引用者)`，没有引用者的表不出。
    fn referrers_by_table(
        &mut self,
        target: RefNo,
        queries: &mut usize,
    ) -> anyhow::Result<Vec<(String, Vec<RefNo>)>> {
        let mut out = Vec::new();
        for (name, hash) in &self.tables {
            *queries += 1;
            let found: Vec<RefNo> = self
                .engine
                .referrers_of(*hash, target)
                .with_context(|| format!("dbnum {} table {name} target {target}", self.dbnum))?
                .into_iter()
                .collect();
            if !found.is_empty() {
                out.push((name.clone(), found));
            }
        }
        Ok(out)
    }

    /// 记录头：`(noun, owner)`；不在本库 / 读不出 → `None`。
    fn header(&mut self, refno: RefNo) -> Option<(String, RefNo)> {
        let view = self.engine.find_element(refno).ok().flatten()?;
        let parsed = ParsedElement::parse(&view.raw_bytes).ok()?;
        Some((
            db1_dehash(parsed.noun_hash).unwrap_or_else(|| format!("#{}", parsed.noun_hash)),
            parsed.owner,
        ))
    }
}

/// 判「是不是反向引用表」时最多看多少条（与 P1 / P2 同一判据：引用者全是本库 refno）。
const REFERENCE_TABLE_SAMPLE: usize = 8;

/// 一个库通过判据的反向表 `(名, hash)`，升序。
fn reference_tables_of(engine: &mut ReadOnlyEngine) -> Vec<(String, u32)> {
    let own_dbno = engine.descriptor().db_mark;
    let roots: Vec<_> = engine.tables().to_vec();
    let mut out = Vec::new();
    for table in roots {
        let Some(mut reference) = engine.reference_table(table.name_hash) else {
            continue;
        };
        let mut local = true;
        let mut seen = 0usize;
        for entry in reference.iter().take(REFERENCE_TABLE_SAMPLE) {
            match entry {
                Ok(entry) if entry.referrer.dbno() == own_dbno => seen += 1,
                _ => {
                    local = false;
                    break;
                }
            }
        }
        if local && seen > 0 {
            out.push((
                db1_dehash(table.name_hash).unwrap_or_else(|| format!("#{}", table.name_hash)),
                table.name_hash,
            ));
        }
    }
    out.sort();
    out
}

fn text(refno: RefNo) -> String {
    format!("{}/{}", refno.dbno(), refno.word1)
}

/// `RefNo` 不实现 `Ord`；集合用原样的 `(word0, word1)` 当键（保留 extent 位，可逆）。
type RefKey = (u32, u32);

fn key(refno: RefNo) -> RefKey {
    (refno.word0, refno.word1)
}

fn unkey(key: RefKey) -> RefNo {
    RefNo::new(key.0, key.1)
}

/// 目录件变了 → 谁引用它 → 上卷到根。见模块文档。
///
/// `resolve_root(dbnum, referrer)` 把一个设计引用者上卷到它的生成根（`None` = 上卷不到）。
pub fn expand_reverse_cascade(
    sources: &[SourceDb],
    seeds: &[CatalogueSeed],
    mut resolve_root: impl FnMut(u32, RefNo) -> anyhow::Result<Option<GenerationRoot>>,
) -> anyhow::Result<ReverseCascade> {
    let mut cascade = ReverseCascade {
        seeds: seeds.iter().map(|seed| text(seed.refno)).collect(),
        ..ReverseCascade::default()
    };
    if seeds.is_empty() {
        return Ok(cascade);
    }

    let mut catalogues: BTreeMap<u32, OpenDb> = BTreeMap::new();
    for source in sources.iter().filter(|source| source.is_catalogue()) {
        catalogues.insert(source.dbnum, OpenDb::open(source)?);
    }

    // ---- 第 1 段：目录侧闭包 ----
    let mut closure: BTreeSet<RefKey> = BTreeSet::new();
    let mut frontier: VecDeque<RefNo> = VecDeque::new();
    let mut degraded_reasons: BTreeMap<u32, Vec<String>> = BTreeMap::new();

    for seed in seeds {
        // 种子 ∪ 祖先。祖先链只在种子自己所在的目录库里读。
        let mut chain = vec![seed.refno];
        let mut noun_of_seed = seed.noun.clone();
        if let Some(db) = catalogues.get_mut(&seed.refno.dbno()) {
            let mut current = seed.refno;
            for depth in 0..MAX_ANCESTOR_DEPTH {
                let Some((noun, owner)) = db.header(current) else {
                    if depth == 0 {
                        cascade.unreadable.push(text(current));
                    }
                    break;
                };
                if depth == 0 && noun_of_seed.is_none() {
                    noun_of_seed = Some(noun.clone());
                }
                if noun == "WORL" || owner.word0 == 0 && owner.word1 == 0 || owner == current {
                    break;
                }
                chain.push(owner);
                current = owner;
            }
            // 退路 (ii)：这个目录库没有任何反向表，而种子不属设计侧可直接引用的层。
            if db.tables.is_empty() {
                let layer_ok = noun_of_seed
                    .as_deref()
                    .is_some_and(|noun| DESIGN_REFERABLE_NOUNS.contains(&noun));
                if !layer_ok {
                    degraded_reasons.entry(db.dbnum).or_default().push(format!(
                        "{}（{}）",
                        text(seed.refno),
                        noun_of_seed.as_deref().unwrap_or("noun 未知")
                    ));
                }
            }
        } else {
            // 种子所在的目录库不在 MDB 里：算不出它在目录里的引用者，按退化记。
            degraded_reasons
                .entry(seed.refno.dbno())
                .or_default()
                .push(format!("{}（所在目录库不在 MDB 里）", text(seed.refno)));
        }
        for refno in chain {
            if closure.insert(key(refno)) {
                frontier.push_back(refno);
            }
        }
    }

    while let Some(target) = frontier.pop_front() {
        for db in catalogues.values_mut() {
            if db.tables.is_empty() {
                continue;
            }
            for referrer in db.referrers_of(target, &mut cascade.point_queries)? {
                if closure.insert(key(referrer)) {
                    frontier.push_back(referrer);
                }
            }
        }
    }

    for (dbnum, items) in degraded_reasons {
        cascade.degraded.insert(
            dbnum,
            format!(
                "目录库 {dbnum} 没有任何反向引用表，算不出这些目录内部层变动的引用者：{}",
                items.join("、")
            ),
        );
    }
    for (dbnum, db) in &catalogues {
        if !db.tables.is_empty() {
            cascade.tables_used.insert(
                *dbnum,
                db.tables.iter().map(|(name, _)| name.clone()).collect(),
            );
        }
    }
    drop(catalogues);
    cascade.catalogue_closure = closure.iter().map(|k| text(unkey(*k))).collect();

    // ---- 第 2 段：设计侧一跳 ----
    let targets: Vec<RefNo> = closure.into_iter().map(unkey).collect();
    let mut referrers_by_db: BTreeMap<u32, BTreeSet<RefKey>> = BTreeMap::new();
    // 经「引用者还要到被引用件身上去读的格」那几张表（`LSTU` / `HSTU`，[`DESIGN_CASCADE_TARGET_READS`]）命中的设计件。
    let mut read_through: BTreeSet<RefKey> = BTreeSet::new();
    // 带名单上那几个属性的反向表（`COMPRE`）的设计库：开着留给再一跳，别的查完就关。
    let mut carriers: Vec<OpenDb> = Vec::new();
    for source in sources.iter().filter(|source| source.is_design()) {
        let mut db = OpenDb::open(source)?;
        if db.tables.is_empty() {
            continue;
        }
        let mut referrers: BTreeSet<RefKey> = BTreeSet::new();
        for target in &targets {
            for (table, found) in db.referrers_by_table(*target, &mut cascade.point_queries)? {
                if is_target_read(&table) {
                    read_through.extend(found.iter().copied().map(key));
                }
                referrers.extend(found.into_iter().map(key));
            }
        }
        cascade.tables_used.insert(
            source.dbnum,
            db.tables.iter().map(|(name, _)| name.clone()).collect(),
        );
        if !referrers.is_empty() {
            referrers_by_db.insert(source.dbnum, referrers);
        }
        if db
            .tables
            .iter()
            .any(|(name, _)| target_reads_of(name).is_some())
        {
            carriers.push(db);
        }
    }

    // ---- 第 2 段之二：再一跳（GitHub #143）----
    // 上面经 `LSTU` / `HSTU` 命中的设计件（弯头、支管……）自己会重算；经 `COMPRE` 指着它们的支撑件读的是同一个
    // 目录件（管外径），却不直接引用它，上面那一跳点不到。到带 `COMPRE` 表的设计库里按这些设计件再点查一次，查到的
    // 支撑件并进它所在库的引用者，与别的引用者一样上卷、一样排工。没有设计件经那几张表命中，或没有哪个库带这张表
    // （AMS 样例工程就是），这一段一次点查都不发。
    if !read_through.is_empty() {
        for db in &mut carriers {
            let tables: Vec<(String, u32)> = db
                .tables
                .iter()
                .filter(|(name, _)| target_reads_of(name).is_some())
                .cloned()
                .collect();
            for component in read_through.iter().copied().map(unkey) {
                for (name, hash) in &tables {
                    cascade.point_queries += 1;
                    let found = db.engine.referrers_of(*hash, component).with_context(|| {
                        format!("dbnum {} table {name} target {component}", db.dbnum)
                    })?;
                    for referrer in found.into_iter().filter(RefNo::is_valid) {
                        if referrers_by_db
                            .entry(db.dbnum)
                            .or_default()
                            .insert(key(referrer))
                        {
                            cascade
                                .dependent_referrers
                                .entry(db.dbnum)
                                .or_default()
                                .insert(text(referrer));
                        }
                    }
                }
            }
        }
    }
    drop(carriers);

    // ---- 上卷 ----
    for (dbnum, referrers) in referrers_by_db {
        let mut roots: BTreeMap<String, CascadeRoot> = BTreeMap::new();
        let mut unrooted = Vec::new();
        for referrer in referrers.iter().copied().map(unkey) {
            match resolve_root(dbnum, referrer)? {
                Some(root) => {
                    roots.entry(root.root.to_pdms_str()).or_insert(CascadeRoot {
                        refno: root.root.to_pdms_str(),
                        noun: root.noun,
                        name: root.name,
                    });
                }
                None => unrooted.push(text(referrer)),
            }
        }
        cascade
            .design_referrers
            .insert(dbnum, referrers.iter().map(|k| text(unkey(*k))).collect());
        if !roots.is_empty() {
            cascade.roots.insert(dbnum, roots.into_values().collect());
        }
        if !unrooted.is_empty() {
            cascade.unrooted.insert(dbnum, unrooted);
        }
    }
    Ok(cascade)
}

/// 用一个钉住会话的 `DbSet` 做 `resolve_root`：owner 链从文件读，判据是
/// [`resolve_element_generation_root`] 那一份（与稳态窗口选根同源）。
pub fn resolve_root_from_set(
    set: &Arc<DbSet>,
    unit_types: &[String],
    referrer: RefNo,
) -> Option<GenerationRoot> {
    resolve_element_generation_root(refno_from_e3d(referrer), unit_types, |candidate| {
        let element = set.element(refno_to_e3d(candidate));
        let noun = element.element_type().ok()?;
        let owner = element
            .owner()
            .ok()
            .flatten()
            .map(|owner| refno_from_e3d(owner.refno()));
        let name = element.stored_name().ok().flatten().unwrap_or_default();
        Some(GenerationNode { owner, noun, name })
    })
}

/// 枚举一个目录下的库文件为 [`SourceDb`]（测试与探针用；生产从 MDB 成员来）。
pub fn sources_in_dir(dir: &Path, db_types: &[&str]) -> anyhow::Result<Vec<SourceDb>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Ok(engine) = ReadOnlyEngine::open(&path) else {
            continue;
        };
        let descriptor = engine.descriptor();
        let Some(db_type) = descriptor.db_type_name() else {
            continue;
        };
        if !db_types
            .iter()
            .any(|wanted| db_type.eq_ignore_ascii_case(wanted))
        {
            continue;
        }
        out.push(SourceDb {
            dbnum: descriptor.db_mark,
            db_type,
            file: path,
            sesno: None,
        });
    }
    out.sort_by_key(|source| source.dbnum);
    Ok(out)
}

// ---------------------------------------------------------------------------
// P4：设计侧通用一跳反查（稳态窗口规划器用）
// ---------------------------------------------------------------------------

/// 引用者要并进 `touched` 的属性——**这一条才是行为**。
///
/// 783 库差表（`docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md` §1.3）里「设计→设计、
/// 跨单元、e3d-model 几何真读」的引用边只有 `POSREF` 一条：电缆桥架成员的位置参照，
/// `vendor/e3d-model/src/rpath.rs` 沿它逐级折算落到 `RPATH` 上的点。被参照的那件动了，参照它的
/// 桥架件几何就变，而它多半不在同一个交付单元下——owner 链上卷盖不住，只有反查能答。
///
/// 其余设计侧反向表（`GOBREF` `HTPREF` `HDBREF` `GRDREF` `TMRREF` `OWNREF` `CONNRE`…）今天的几何
/// 一个都不读，[`DesignReferenceCascade`] 查了**只记账**（`edges_by_attribute`）。几何覆盖扩到读
/// 它们那天，把名字加进这里就是全部改动——入口、护栏、账都已经在原位。`COMPRE` 是例外：它的引用者读的是被引用件
/// 身上的格，走带条件的那一张（[`DESIGN_CASCADE_TARGET_READS`]）。
pub const DESIGN_CASCADE_ATTRIBUTES: [&str; 1] = ["POSREF"];

/// 带条件的行为名单（GitHub #143）：`(引用属性, 引用者到被引用件身上去读的那几格)`。
///
/// [`DESIGN_CASCADE_ATTRIBUTES`] 上的引用者关心的是被引用件「在哪 / 在不在」，被引用件一进账本它就上卷。这一张上的
/// 引用者读的是被引用件**身上的某几格**：只有这几格在窗口两端取值不同，引用者才重算；被引用件只是挪了位置、改了名，
/// 引用者不动——不为保险多算。
///
/// `COMPRE`：支撑件（`ANCI` 一族）的管外径。e3d-io 目录求值的 `ATTRIB POD` 在设计件自己没有 `LSTU` 时经它的
/// `COMPRE` 找到被支撑的构件，取那只构件的 `LSTU`（弯头、三通一类）或 `HSTU`（`BRAN`）走到管子的目录件要 `PARA[2]`
/// （e3d-io `db_param_env.rs` 的 `catalogue_component_trace`）；外径决定支撑件整件沿 z 的平移。支撑件自己的 `COMPRE`
/// 改指别处归元数据门管（GitHub #132，`model_impact::E3D_IO_DESIGN_SIDE_READS`）；这里管的是反方向——被指的那件换了
/// 规格。名单与 e3d-io 的读点由测试 `the_conditional_list_matches_what_e3d_io_reads_on_the_referenced_component`
/// 钉在一起。
///
/// 另一处不同：这一类的引用者多数在**别的设计库**里（APS 样例工程 234 条 `COMPRE` 边有 224 条，支撑库指着管道库），
/// 所以跨库的不只计数，点名交给跨库交接（[`DesignReferenceCascade::handoff_referrers`]）。
pub const DESIGN_CASCADE_TARGET_READS: [(&str, &[&str]); 1] = [("COMPRE", &["LSTU", "HSTU"])];

/// `attribute` 在不在带条件的名单上：在就给引用者经它到被引用件身上去读的那几格。
fn target_reads_of(attribute: &str) -> Option<&'static [&'static str]> {
    DESIGN_CASCADE_TARGET_READS
        .iter()
        .find(|(name, _)| *name == attribute)
        .map(|(_, reads)| *reads)
}

/// `table` 是不是带条件的名单上某个属性要到被引用件身上去读的格（`LSTU` / `HSTU`）。
fn is_target_read(table: &str) -> bool {
    DESIGN_CASCADE_TARGET_READS
        .iter()
        .any(|(_, reads)| reads.contains(&table))
}

/// 一格引用属性在窗口一端读出来的样子（带条件的名单两端比对用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceRead {
    /// 元素不在这一端 / 它的 noun 没有这一格 / 这一格是空引用——对引用者是同一回事：经它走不到任何东西。
    Absent,
    /// 指着这个元素。
    Target(RefNo),
    /// 元素在、这一格也有，却读不出来。**不是**「没有」。
    Unreadable,
}

/// 纯函数：同一个元素的同一组引用属性在窗口两端各读一遍（逐格对齐），有没有哪一格不同。
///
/// 任一端读不出来按「变了」算：读不出来时「没变」与「不知道」长得一样，宁多算不漏算。
pub fn reference_reads_differ(base: &[ReferenceRead], target: &[ReferenceRead]) -> bool {
    base.len() != target.len()
        || base
            .iter()
            .zip(target)
            .any(|(before, after)| before != after || matches!(before, ReferenceRead::Unreadable))
}

/// I/O：`element` 的 `attribute` 在这一端的 `DbSet` 上读出来的样子。
pub fn read_reference(set: &Arc<DbSet>, element: RefNo, attribute: &str) -> ReferenceRead {
    let element = set.element(element);
    match element.exists() {
        Ok(true) => {}
        Ok(false) => return ReferenceRead::Absent,
        Err(_) => return ReferenceRead::Unreadable,
    }
    match element.has_attribute(attribute) {
        Ok(true) => {}
        Ok(false) => return ReferenceRead::Absent,
        Err(_) => return ReferenceRead::Unreadable,
    }
    match element.get_ref(attribute) {
        Ok(Some(target)) => ReferenceRead::Target(target),
        Ok(None) => ReferenceRead::Absent,
        Err(_) => ReferenceRead::Unreadable,
    }
}

/// 一窗最多发多少次点查（`变更元素数 × 要开的库里反向表总数`）。
///
/// 钉在**点查次数**而不是变更元素数上：真正的成本是这个乘积，而 MDB 逐库并集把它乘上了成员库
/// 的个数——同样 2 000 个变更元素，单库 5 张表是一万次，三十个成员库就是三十万次。
/// 预算在开完库、读完表清单之后、发第一次点查之前算，超了**一次都不发**，判不了
/// （[`DesignReferenceCascade::degraded`]），按计划 §2 原则 3 退到「该库全部根过期」。
///
/// 取值由实测定（2026-09-07，语料 31 个带 `POSREF` 表的设计库 / 93 窗）：最大的一窗
/// `aps250110_0001 29→30` 是 7 139 个变更元素 × 10 张表 = **71 390 次点查**，加进去之后整趟
/// 93 窗对拍反而从 214 s 降到 200 s——点查是 O(树高)，成本淹没在索引扫描里，量不出来。
/// 20 万挡住的是逐库并集乘出来的那一档：同一窗放进一个三十成员的 MDB 就是一百多万次。
///
/// 这个数原本钉在变更元素数上（2 000），上面那一窗因此判不了、`regen` 从 251 涨到 495——
/// 白重算 244 个根。换成点查次数之后那一窗答得出来了，93 窗两态**逐字相同**。
pub const DESIGN_CASCADE_QUERY_BUDGET: usize = 200_000;

/// 一条点查出来的反向边。`attribute` 就是表名（写侧 `db_put_reference` 把表名取成 attId）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignReferrerEdge {
    /// 被查的键：这一窗的某个变更元素。
    pub target: RefNo,
    pub attribute: String,
    /// `referrer.attribute == target` 的那个元素。总在它自己那个库里。
    pub referrer: RefNo,
}

/// 逐库点查的产物与账——I/O 段的全部输出，喂给纯函数 [`DesignReferenceCascade::from_edges`]。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesignReferrerEdges {
    pub edges: Vec<DesignReferrerEdge>,
    /// 拿去点查的变更元素数。
    pub sources: usize,
    /// 每个成员库真被点查过的反向表名（升序）；没有反向表的库不进这张表。
    pub tables_used: BTreeMap<u32, Vec<String>>,
    /// 点查次数（键 × 表）。
    pub queries: usize,
    /// 表在、读挂了的点查次数。
    pub query_failures: usize,
    /// 打不开的成员库 → 原因。**不是**「这个库没有引用者」，是未知。
    pub unreadable_dbs: BTreeMap<u32, String>,
    /// 本该发的点查次数超了 [`DESIGN_CASCADE_QUERY_BUDGET`]：一次都没发，`edges` 里没有点查出来的边但
    /// **不是**「没有引用者」。库开过了、表清单读过了，所以 `tables_used` 仍是实的。带条件那一类的表这时整张
    /// 走一遍（见下面两格），`edges` 里只有它们的边。
    pub over_budget: Option<usize>,
    /// 超预算时补的那一手（GitHub #143，`walk_target_read_tables`）：带条件那一类
    /// （[`DESIGN_CASCADE_TARGET_READS`]）的反向表整张走了几张。没超预算时是 0——那时这几张表与别的表一样点查。
    pub target_read_tables_walked: usize,
    /// 同上：一共走过多少条表项（留下来成边的只是被引用件在本窗变更元素里的那些）。
    pub target_read_entries_walked: usize,
}

/// 一窗里要拿去反查的变更元素：账本里的 refno，去重，不分变更类型。
///
/// 三种变更都要查：`Modified` 改了被参照件的位置，`Deleted` 让参照它的件失去锚点，`Created`
/// 唤醒此前解不开的参照。
pub fn changed_elements(plan: &UpdatePlan) -> Vec<RefNo> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for entry in plan.ledger.entries() {
        if seen.insert(key(entry.refno)) {
            out.push(entry.refno);
        }
    }
    out
}

/// I/O：对每个变更元素，在每个设计库的每张反向表里点查一次。
///
/// 分两步：先把库全开了、表清单读出来（这一步的成本只跟**库数**有关），据此算出这一窗要发多少
/// 次点查；超 `budget` 就一次都不发（`over_budget`）。开库便宜，而它算出来的乘积是唯一诚实的
/// 预算依据——变更元素数本身说明不了成本，逐库并集把它乘上了成员库个数。
///
/// 一个库打不开 / 一次点查读挂都只记账不中断——中断会把「这一窗没有引用者」与「没查成」
/// 混成同一个结果，而这两件事的处置正相反（见 [`DesignReferenceCascade::degraded`]）。
pub fn read_design_referrer_edges(
    sources: &[RefNo],
    design_dbs: &[SourceDb],
    budget: usize,
) -> DesignReferrerEdges {
    let mut out = DesignReferrerEdges {
        sources: sources.len(),
        ..DesignReferrerEdges::default()
    };
    if sources.is_empty() {
        return out;
    }
    let mut open: Vec<OpenDb> = Vec::new();
    for source in design_dbs.iter().filter(|source| source.is_design()) {
        match OpenDb::open(source) {
            Ok(db) if db.tables.is_empty() => {}
            Ok(db) => {
                out.tables_used.insert(
                    source.dbnum,
                    db.tables.iter().map(|(name, _)| name.clone()).collect(),
                );
                open.push(db);
            }
            Err(error) => {
                out.unreadable_dbs
                    .insert(source.dbnum, format!("{error:#}"));
            }
        }
    }
    let planned = sources.len() * open.iter().map(|db| db.tables.len()).sum::<usize>();
    if planned > budget {
        out.over_budget = Some(planned);
        walk_target_read_tables(&mut open, sources, &mut out);
        return out;
    }
    for db in &mut open {
        let tables = db.tables.clone();
        for target in sources {
            for (name, hash) in &tables {
                out.queries += 1;
                match db.engine.referrers_of(*hash, *target) {
                    Ok(referrers) => {
                        out.edges
                            .extend(
                                referrers
                                    .into_iter()
                                    .filter(RefNo::is_valid)
                                    .map(|referrer| DesignReferrerEdge {
                                        target: *target,
                                        attribute: name.clone(),
                                        referrer,
                                    }),
                            )
                    }
                    Err(_) => out.query_failures += 1,
                }
            }
        }
    }
    out
}

/// 超预算时补的一手（GitHub #143）：带条件那一类（[`DESIGN_CASCADE_TARGET_READS`]，今天只有 `COMPRE`）的反向表
/// **整张走一遍**，留下被引用件是本窗变更元素的边；别的表照旧一次都不查。
///
/// 为什么不能跟着整窗一起放弃：超预算的窗，本库已发布的根全部重算（[`DesignReferenceCascade::degraded`]），本库里的
/// 引用者由此盖住；可这一类的引用者多数在**别的设计库**里，那里没有谁替它们兜底——而「一窗动了上千个元素」正是
/// 成批换规格的样子。整张走一遍的成本只跟表的大小有关、与变更元素数无关（APS 样例工程四张 `COMPRE` 表共 234 条），
/// 所以不占点查预算。表打不开 / 走到一半读挂记一笔 `query_failures`（这一窗本来就判不了，账上多一个原因）。
fn walk_target_read_tables(open: &mut [OpenDb], sources: &[RefNo], out: &mut DesignReferrerEdges) {
    let changed: BTreeSet<RefKey> = sources.iter().copied().map(key).collect();
    for db in open {
        let tables: Vec<(String, u32)> = db
            .tables
            .iter()
            .filter(|(name, _)| target_reads_of(name).is_some())
            .cloned()
            .collect();
        for (name, hash) in tables {
            let Some(mut table) = db.engine.reference_table(hash) else {
                out.query_failures += 1;
                continue;
            };
            out.target_read_tables_walked += 1;
            for entry in table.iter() {
                let Ok(entry) = entry else {
                    out.query_failures += 1;
                    break;
                };
                out.target_read_entries_walked += 1;
                if entry.referrer.is_valid() && changed.contains(&key(entry.target)) {
                    out.edges.push(DesignReferrerEdge {
                        target: entry.target,
                        attribute: name.clone(),
                        referrer: entry.referrer,
                    });
                }
            }
        }
    }
}

/// 一窗的设计侧通用一跳反查：账 + 要并进 `touched` 的引用者根。挂在 `WindowRootSources` 上
/// （与 C2 `structural_connection`、C8 `cable_endpoint` 同一形状）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesignReferenceCascade {
    /// 开关态。`false` = 一个成员库都没开、一次表都没查，[`Self::expand_touched`] 不动 `touched`。
    pub enabled: bool,
    pub sources: usize,
    pub tables_used: BTreeMap<u32, Vec<String>>,
    pub queries: usize,
    pub query_failures: usize,
    pub unreadable_dbs: BTreeMap<u32, String>,
    /// 命中的引用者边数，按属性分。**观测**：哪些设计侧属性真的答出了引用者。
    pub edges_by_attribute: BTreeMap<String, usize>,
    /// [`DESIGN_CASCADE_ATTRIBUTES`] 命中、且引用者在本库的边，上卷到的生成根。
    pub referrer_roots: BTreeSet<RefnoEnum>,
    /// 引用者在 T 端 owner 链上找不到任何生成根（悬空表项 / 无根族）的边数。
    pub referrer_without_root: usize,
    /// [`DESIGN_CASCADE_ATTRIBUTES`] 命中、但引用者在**别的**设计库里的边数，按那个库分。
    ///
    /// 本窗口只排本库的工作（`roots_T` 就是本库的），排不了它们——这是在案的跨库缺口，
    /// 与 C8 「只查本库」留下的那 196 条同性质。记出来，不静默。
    pub foreign_referrers: BTreeMap<u32, usize>,
    /// [`DESIGN_CASCADE_TARGET_READS`] 命中、且被引用件那几格在窗口两端取值不同的边数，按属性分（GitHub #143）。
    /// 它们的引用者要重算：本库的上卷进 `referrer_roots`，别的库里的进 `handoff_referrers`。
    pub target_reads_triggered: BTreeMap<String, usize>,
    /// 同一张名单命中、但那几格两端取值相同的边数，按属性分——被引用件只改了引用者不读的东西，引用者不动。
    pub target_reads_unchanged: BTreeMap<String, usize>,
    /// 触发了的边里引用者在**别的**设计库的那些（去重，按出现序）：本窗的计划装不下它们的根，点名交给跨库交接
    /// （[`crate::data_interface::cross_db_handoff`]，与 C2 的跨库对端、C8 的跨库引用者同一条路）。反查超预算
    /// （`degraded`）的窗这一格照样是实的：那时这一类的表整张走一遍（[`DesignReferrerEdges::over_budget`]），
    /// 本库的根靠整窗重算盖住，别的库里的只有这一份名单。
    pub handoff_referrers: Vec<RefNo>,
    /// 判不了的原因：点查次数超预算 / 成员库打不开 / 表读挂。
    ///
    /// 调用方（[`crate::data_interface::window_root_plan::WindowImpact`]）据此**放弃凭证前移**
    /// ——计划 §2 原则 3：反查答不上来时空集与「真的没有引用者」长得一模一样，静默取前者就是
    /// 漏算，所以退到今天的行为（已持久化的根全部重算）。
    pub degraded: Option<String>,
}

impl DesignReferenceCascade {
    /// 开关关：零 I/O、零改动。
    pub fn off() -> Self {
        Self::default()
    }

    /// 纯函数：把点查出的边折成根集与账。`root_chain(引用者)` 给它 T 端 owner 链上的根
    /// （生产传 [`roots_on_owner_chain`]，测试可以手搓）。
    ///
    /// 没有窗口两端可比的调用方用这一个（探针、只看 `POSREF` 的测试）：[`DESIGN_CASCADE_TARGET_READS`] 上的属性
    /// 一条都不触发、只记账。生产走 [`Self::collect`] → [`Self::from_edges_with`]。
    pub fn from_edges(
        edges: &DesignReferrerEdges,
        dbnum: u32,
        root_chain: impl FnMut(RefNo) -> Vec<RefNo>,
    ) -> Self {
        Self::from_edges_with(edges, dbnum, root_chain, |_, _| false)
    }

    /// [`Self::from_edges`]，外加带条件那一类的判词：`target_reads_changed(被引用件, 那几格)` 回这几格在窗口两端
    /// 取值是否不同（生产传 [`read_reference`] + [`reference_reads_differ`]）。同一个被引用件只问一次。
    pub fn from_edges_with(
        edges: &DesignReferrerEdges,
        dbnum: u32,
        mut root_chain: impl FnMut(RefNo) -> Vec<RefNo>,
        mut target_reads_changed: impl FnMut(RefNo, &[&str]) -> bool,
    ) -> Self {
        let mut out = Self {
            enabled: true,
            sources: edges.sources,
            tables_used: edges.tables_used.clone(),
            queries: edges.queries,
            query_failures: edges.query_failures,
            unreadable_dbs: edges.unreadable_dbs.clone(),
            ..Self::default()
        };
        // 带条件那一类的判词按（被引用件, 属性）记一次：一只构件被几只支撑件指着，两端只读一遍。
        let mut verdicts: BTreeMap<(RefKey, &str), bool> = BTreeMap::new();
        let mut handed_off: BTreeSet<RefKey> = BTreeSet::new();
        for edge in &edges.edges {
            let attribute = edge.attribute.as_str();
            *out.edges_by_attribute
                .entry(edge.attribute.clone())
                .or_default() += 1;
            let conditional = target_reads_of(attribute);
            if let Some(reads) = conditional {
                let changed = *verdicts
                    .entry((key(edge.target), attribute))
                    .or_insert_with(|| target_reads_changed(edge.target, reads));
                let bucket = if changed {
                    &mut out.target_reads_triggered
                } else {
                    &mut out.target_reads_unchanged
                };
                *bucket.entry(edge.attribute.clone()).or_default() += 1;
                if !changed {
                    continue;
                }
            } else if !DESIGN_CASCADE_ATTRIBUTES.contains(&attribute) {
                continue;
            }
            if edge.referrer.dbno() != dbnum {
                if conditional.is_some() {
                    // 带条件那一类的引用者多数在别的设计库里：点名交给那个库的任务，不只计数。
                    if handed_off.insert(key(edge.referrer)) {
                        out.handoff_referrers.push(edge.referrer);
                    }
                } else {
                    *out.foreign_referrers
                        .entry(edge.referrer.dbno())
                        .or_default() += 1;
                }
                continue;
            }
            let roots = root_chain(edge.referrer);
            if roots.is_empty() {
                out.referrer_without_root += 1;
            }
            out.referrer_roots
                .extend(roots.into_iter().map(refno_from_e3d));
        }
        let mut reasons = Vec::new();
        if let Some(planned) = edges.over_budget {
            let mut reason = format!(
                "这一窗要发 {planned} 次点查（{} 个变更元素 × {} 个成员库的反向表），超过预算 \
                 {DESIGN_CASCADE_QUERY_BUDGET}，一次都没发",
                edges.sources,
                out.tables_used.len()
            );
            if edges.target_read_tables_walked > 0 {
                reason.push_str(&format!(
                    "（带条件那一类的 {} 张表整张走了一遍、{} 条表项：别的库里的引用者照常交接）",
                    edges.target_read_tables_walked, edges.target_read_entries_walked
                ));
            }
            reasons.push(reason);
        }
        if !out.unreadable_dbs.is_empty() {
            reasons.push(format!(
                "MDB 设计成员 {:?} 打不开（它们的引用者未知）",
                out.unreadable_dbs.keys().collect::<Vec<_>>()
            ));
        }
        if out.query_failures > 0 {
            reasons.push(format!("{} 次点查读挂", out.query_failures));
        }
        out.degraded = (!reasons.is_empty()).then(|| reasons.join("；"));
        out
    }

    /// I/O 版：从账本粗筛变更元素，逐库点查，再按 T 端 owner 索引与 `roots_T` 映射成根。
    ///
    /// `design_dbs` 是 MDB 的设计成员（`E3dModelService::design_sources()`）；本库无论在不在
    /// 里面都会被查，且**钉 T 端**——引用者的记录与表都在引用方那边，要问的就是 T 那一刻的表。
    /// `base` / `target` 是本窗两端的 `DbSet`：带条件的名单（[`DESIGN_CASCADE_TARGET_READS`]）要在两端各读一遍
    /// 被引用件——它是本窗的变更元素，在本库里，两端都钉着。
    #[allow(clippy::too_many_arguments)]
    pub fn collect(
        plan: &UpdatePlan,
        dbnum: u32,
        file: &Path,
        target_sesno: u32,
        design_dbs: &[SourceDb],
        owners: &BTreeMap<(u32, u32), RefNo>,
        roots_t: &[GenerationRoot],
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
    ) -> Self {
        let sources = changed_elements(plan);
        if sources.is_empty() {
            return Self {
                enabled: true,
                ..Self::default()
            };
        }
        let dbs = design_dbs_for_window(dbnum, file, target_sesno, design_dbs);
        let edges = read_design_referrer_edges(&sources, &dbs, DESIGN_CASCADE_QUERY_BUDGET);
        let root_keys: BTreeSet<(u32, u32)> = roots_t
            .iter()
            .map(|root| {
                let refno = refno_to_e3d(root.root);
                (refno.word0, refno.word1)
            })
            .collect();
        Self::from_edges_with(
            &edges,
            dbnum,
            |referrer| roots_on_owner_chain(owners, &root_keys, referrer),
            |element, attributes| {
                let read = |set: &Arc<DbSet>| -> Vec<ReferenceRead> {
                    attributes
                        .iter()
                        .map(|attribute| read_reference(set, element, attribute))
                        .collect()
                };
                reference_reads_differ(&read(base), &read(target))
            },
        )
    }

    /// 把引用者根并进 `touched`，回新增的个数。开关关 = 不动。
    pub fn expand_touched(&self, touched: &mut BTreeSet<RefnoEnum>) -> usize {
        if !self.enabled {
            return 0;
        }
        self.referrer_roots
            .iter()
            .filter(|root| touched.insert(**root))
            .count()
    }

    /// 要交给跨库交接的引用者（[`crate::data_interface::cross_db_handoff::elements_by_dbnum`] 的输入）：带条件那一类
    /// 触发了、引用者却在别的设计库里的。开关关着是空的。
    pub fn handoff_referrer_elements(&self) -> impl Iterator<Item = RefNo> + '_ {
        self.handoff_referrers.iter().copied()
    }

    /// 回执里的一行观测。
    pub fn summary_line(&self, dbnum: u32) -> String {
        if !self.enabled {
            return format!(
                "dbnum={dbnum}: design-reference cascade off — touched unchanged \
                 (model_design_reference_cascade=false)"
            );
        }
        let edges = self
            .edges_by_attribute
            .iter()
            .map(|(attribute, count)| format!("{attribute}:{count}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut line = format!(
            "dbnum={dbnum}: design-reference cascade on — sources={} dbs_queried={} queries={} \
             query_failures={} edges=[{edges}] behavioural={:?} referrer_roots={} \
             referrer_without_root={}",
            self.sources,
            self.tables_used.len(),
            self.queries,
            self.query_failures,
            DESIGN_CASCADE_ATTRIBUTES,
            self.referrer_roots.len(),
            self.referrer_without_root,
        );
        let target_reads = DESIGN_CASCADE_TARGET_READS
            .iter()
            .filter_map(|(attribute, reads)| {
                let changed = self
                    .target_reads_triggered
                    .get(*attribute)
                    .copied()
                    .unwrap_or(0);
                let unchanged = self
                    .target_reads_unchanged
                    .get(*attribute)
                    .copied()
                    .unwrap_or(0);
                (changed + unchanged > 0).then(|| {
                    format!(
                        "{attribute}→{}:{changed} changed/{unchanged} unchanged",
                        reads.join("|")
                    )
                })
            })
            .collect::<Vec<_>>();
        if !target_reads.is_empty() {
            line.push_str(&format!(" target_reads=[{}]", target_reads.join(" ")));
        }
        if !self.handoff_referrers.is_empty() {
            let mut by_db: BTreeMap<u32, usize> = BTreeMap::new();
            for referrer in &self.handoff_referrers {
                *by_db.entry(referrer.dbno()).or_default() += 1;
            }
            line.push_str(&format!(
                "，{} 个引用者在别的设计库里（{by_db:?}），交给跨库交接",
                self.handoff_referrers.len()
            ));
        }
        if !self.foreign_referrers.is_empty() {
            line.push_str(&format!(
                "，{:?} 的引用者在别的设计库里、本窗口排不了（在案跨库缺口）",
                self.foreign_referrers
            ));
        }
        if let Some(reason) = &self.degraded {
            line.push_str(&format!("；判不了：{reason}（放弃凭证前移）"));
        }
        line
    }
}

/// 本窗口要开哪些设计库：MDB 的设计成员，本库换成钉住 T 端的那一份（不在名单里就补进去）。
fn design_dbs_for_window(
    dbnum: u32,
    file: &Path,
    target_sesno: u32,
    design_dbs: &[SourceDb],
) -> Vec<SourceDb> {
    let own = SourceDb {
        dbnum,
        db_type: "DESI".to_string(),
        file: file.to_path_buf(),
        sesno: Some(target_sesno),
    };
    let mut out: Vec<SourceDb> = design_dbs
        .iter()
        .filter(|source| source.is_design() && source.dbnum != dbnum)
        .cloned()
        .collect();
    out.push(own);
    out.sort_by_key(|source| source.dbnum);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMS_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";
    const AMS5052: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5052_0001";
    const AMS7351: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7351_0001";

    fn fixtures_present() -> bool {
        let present = Path::new(AMS_DIR).is_dir() && Path::new(AMS5052).is_file();
        if !present {
            println!("skip: E3D fixtures are not installed");
        }
        present
    }

    /// 没有 owner 链可查时的兜底 resolver：引用者自己就当根（只用来数命中）。
    fn self_as_root(_dbnum: u32, referrer: RefNo) -> anyhow::Result<Option<GenerationRoot>> {
        Ok(Some(GenerationRoot {
            root: refno_from_e3d(referrer),
            noun: "?".into(),
            name: String::new(),
            kind: crate::data_interface::generation_root::GenerationRootKind::DeliveryUnit,
        }))
    }

    /// P3 主门的前半：`ams5052_0001 184→185` 那个 `SDTE` 的闭包必须走到 DTSE→SCOM/SFIT→SPCO
    /// （目录侧三跳），再在语料的全部 DESI 库里点查——并回答 09-04 §2.10 留的问题
    /// 「`ams5052` 有没有被本语料的 DESI 库引用」。
    #[test]
    fn the_ams5052_change_closes_over_the_catalogue_and_is_looked_up_in_every_design_db() {
        if !fixtures_present() {
            return;
        }
        let window = crate::data_interface::cata_cascade_window::collect_catalogue_window(
            5052,
            Path::new(AMS5052),
            184,
            185,
        )
        .expect("collect the P2 window");
        assert_eq!(window.changes.len(), 1);
        let change = &window.changes[0];
        let seed = CatalogueSeed {
            refno: change.refno(),
            noun: change.noun.clone(),
        };
        assert_eq!(seed.refno.dbno(), 5052);

        let mut sources = sources_in_dir(Path::new(AMS_DIR), &["DESI", "CATA"]).expect("sources");
        assert!(sources.iter().any(|s| s.dbnum == 5052 && s.is_catalogue()));
        // 只钉目录库的 T 端；设计库取最新。
        for source in &mut sources {
            if source.dbnum == 5052 {
                source.sesno = Some(185);
            }
        }
        let cascade =
            expand_reverse_cascade(&sources, &[seed.clone()], self_as_root).expect("cascade");
        println!("{}", cascade.summary_line());
        println!("closure = {:?}", cascade.catalogue_closure);
        println!("referrers by db = {:?}", cascade.design_referrers);

        assert_eq!(cascade.seeds, vec![change.refno.clone()]);
        assert!(
            cascade.catalogue_closure.len() > 1,
            "the SDTE's owner chain alone should put more than the seed in the closure"
        );
        assert!(
            cascade.degraded.is_empty(),
            "ams5052 keeps its tables: {:?}",
            cascade.degraded
        );
        assert!(
            cascade
                .tables_used
                .get(&5052)
                .is_some_and(|tables| tables.iter().any(|t| t == "CATR")),
            "the catalogue side must have queried CATR: {:?}",
            cascade.tables_used
        );
        assert!(cascade.point_queries > 0);
        // 这一句就是 09-04 §2.10 的前置问题的答案，打印出来给 P4 的主门选库用。
        println!(
            "ams5052 referenced by {} design elements in {} DESI dbs",
            cascade.referrer_count(),
            cascade.design_referrers.len()
        );
    }

    const AMS7000: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7000_0001";

    /// H1 的 live 门（2026-09-08）：`ams7000_0001` 是 DESI 门钉住的 `PROJECT_CATALOGUE`，
    /// 09-06 计划 P3 说主门该改到它、却一直没跑过。沿它的会话链往回找最近一个「变了目录件、
    /// 且反查得到设计引用者」的窗口 S→T，走 P2 收窗 → P3 反查（与运行期 `reconcile_catalogue_versions`
    /// 同一对纯函数），再按 (c) 的凭证判据核：落款停在 S 的根判过期、落款到 T 的判命中——
    /// 这正是 external 形态下目录改了、设计没改时 `ensure` 从前答错的那一问。
    ///
    /// 语料里没有「改一个目录件就 SAVEWORK」的自然窗口对，所以先沿链往回找；本机语料的
    /// `ams7000_0001` 链上只留了 5 个会话（1 / 237 / 238 / 262 / 263，MERGE CHANGES 折过），
    /// 262→263 零变更、其余都是几万件的导入级窗——找不到精确窗就退到 C8 / P4 live 门的同一手法：
    /// 从某个 DESI 的 `SPRE` 反向表里拿一条指向 7000 的边，把那个 SPCO 当**假定的变更源**，
    /// 反查回来的引用者必须包含那条边的 referrer。DB 那一半（`libraries_behind_catalogue` / 排工）
    /// 由 mem:// 单测钉，这里只走文件半边。
    #[test]
    #[ignore = "manual live: walks the ams7000_0001 session chain and opens every DESI in ams000"]
    fn live_ams7000_catalogue_window_regenerates_referrers_and_fails_stale_stamps() {
        use crate::data_interface::model_coverage::{CatalogueStamp, catalogue_stamps_current};
        if !fixtures_present() || !Path::new(AMS7000).is_file() {
            println!("skip: ams7000_0001 is not installed");
            return;
        }
        // 与 `model_coverage::CATALOGUE_CASCADE_SEED_CAP` 同值：更大的窗在运行期本来就退化。
        const SEED_CAP: usize = 50;
        let file = Path::new(AMS7000);
        let sessions: Vec<u32> = ReadOnlyEngine::open(file)
            .expect("open ams7000_0001")
            .sessions()
            .expect("session chain")
            .iter()
            .map(|session| session.session_id)
            .collect();
        let target = *sessions
            .first()
            .expect("a catalogue file has at least one session");
        let mut sources = sources_in_dir(Path::new(AMS_DIR), &["DESI", "CATA"]).expect("sources");
        assert!(sources.iter().any(|s| s.dbnum == 7000 && s.is_catalogue()));
        for source in &mut sources {
            if source.dbnum == 7000 {
                source.sesno = Some(target);
            }
        }
        // (c) 的判据先钉死：落款停在旧会话的根不算当前，到 T 的才算。与下面走哪条路无关。
        let latest = |dbnum: u32| (dbnum == 7000).then_some(target);
        let stamped_at = |session: u32| CatalogueStamp {
            dbnum: Some(7000),
            session: Some(session),
        };
        assert!(catalogue_stamps_current(&[stamped_at(target)], latest));
        for &base in sessions.iter().skip(1) {
            assert!(
                !catalogue_stamps_current(&[stamped_at(base)], latest),
                "a root still stamped on catalogue session {base} must not count as current at {target}"
            );
        }

        // 退路：假定源。语料里 ams7000 没有精确窗时，反查那一半照样要验——拿一条真实存在的
        // DESI→7000 `SPRE` 边，把它的目标当变了的目录件。
        let assumed_seed = || -> Option<(CatalogueSeed, RefNo, u32)> {
            let spre = e3d_attlib::db1_hash("SPRE");
            for source in sources.iter().filter(|source| source.is_design()) {
                let Ok(mut engine) = ReadOnlyEngine::open(&source.file) else {
                    continue;
                };
                let Some(mut table) = engine.reference_table(spre) else {
                    continue;
                };
                let edge = table
                    .iter()
                    .filter_map(Result::ok)
                    .find(|entry| entry.target.dbno() == 7000)
                    .map(|entry| (entry.target, entry.referrer));
                if let Some((seed, referrer)) = edge {
                    return Some((
                        CatalogueSeed {
                            refno: seed,
                            noun: Some("SPCO".into()),
                        },
                        referrer,
                        source.dbnum,
                    ));
                }
            }
            None
        };

        let mut found = None;
        for &base in sessions.iter().skip(1).take(24) {
            let window = match crate::data_interface::cata_cascade_window::collect_catalogue_window(
                7000, file, base, target,
            ) {
                Ok(window) => window,
                Err(error) => {
                    println!("skip window {base}→{target}: {error:#}");
                    continue;
                }
            };
            if window.changes.is_empty() || window.changes.len() > SEED_CAP {
                println!(
                    "window {base}→{target}: {} changes, not a precise-cascade shape",
                    window.changes.len()
                );
                continue;
            }
            let seeds: Vec<CatalogueSeed> = window
                .changes
                .iter()
                .map(|change| CatalogueSeed {
                    refno: change.refno(),
                    noun: change.noun.clone(),
                })
                .collect();
            let cascade = expand_reverse_cascade(&sources, &seeds, self_as_root).expect("cascade");
            println!("window {base}→{target}: {}", cascade.summary_line());
            if cascade.referrer_count() > 0 {
                found = Some((base, window, cascade));
                break;
            }
        }
        let cascade = match found {
            Some((base, window, cascade)) => {
                println!(
                    "ams7000 {base}→{target}: {} changed catalogue elements → {} design referrers in {} DESI dbs → {} roots",
                    window.changes.len(),
                    cascade.referrer_count(),
                    cascade.design_referrers.len(),
                    cascade.roots.values().map(Vec::len).sum::<usize>()
                );
                cascade
            }
            None => {
                println!(
                    "no precise window on the ams7000_0001 chain ({} sessions retained); \
                     falling back to an assumed changed SPCO taken from a DESI SPRE table",
                    sessions.len()
                );
                let Some((seed, referrer, referrer_db)) = assumed_seed() else {
                    println!("skip: no DESI in ams000 carries an SPRE edge into 7000");
                    return;
                };
                println!(
                    "assumed source: {} (SPCO in 7000) referenced by {referrer} in DESI {referrer_db}",
                    seed.refno
                );
                let cascade = expand_reverse_cascade(&sources, &[seed], self_as_root)
                    .expect("cascade from the assumed source");
                println!("{}", cascade.summary_line());
                assert!(
                    cascade
                        .design_referrers
                        .get(&referrer_db)
                        .is_some_and(|referrers| referrers.contains(&text(referrer))),
                    "the SPRE edge's own referrer {} must come back from the reverse lookup: {:?}",
                    text(referrer),
                    cascade.design_referrers
                );
                cascade
            }
        };
        assert!(
            cascade.degraded.is_empty(),
            "the project catalogue keeps its reference tables: {:?}",
            cascade.degraded
        );
        assert!(
            cascade.point_queries > 0 && cascade.referrer_count() > 0,
            "the reverse lookup must have queried and found design referrers"
        );
        assert!(
            cascade.roots.values().map(Vec::len).sum::<usize>() > 0,
            "every design referrer rolls up to a generation root"
        );
    }

    /// 退路 (ii)：零表目录库上，SPCO 层的种子照常进设计侧点查（不退化），SCOM 层的种子让该库退化并点名。
    #[test]
    fn a_tableless_catalogue_degrades_only_for_catalogue_internal_seeds() {
        if !fixtures_present() || !Path::new(AMS7351).is_file() {
            return;
        }
        let sources = vec![SourceDb {
            dbnum: 7351,
            db_type: "CATA".into(),
            file: PathBuf::from(AMS7351),
            sesno: None,
        }];
        let spco = CatalogueSeed {
            refno: RefNo::new(encode_dbno(7351), 1),
            noun: Some("SPCO".into()),
        };
        let scom = CatalogueSeed {
            refno: RefNo::new(encode_dbno(7351), 2),
            noun: Some("SCOM".into()),
        };
        let unknown = CatalogueSeed {
            refno: RefNo::new(encode_dbno(7351), 3),
            noun: None,
        };

        let only_spco = expand_reverse_cascade(&sources, &[spco.clone()], self_as_root).unwrap();
        assert!(only_spco.degraded.is_empty(), "{:?}", only_spco.degraded);

        let with_scom =
            expand_reverse_cascade(&sources, &[spco, scom, unknown], self_as_root).unwrap();
        let reason = with_scom.degraded.get(&7351).expect("7351 degrades");
        assert!(reason.contains("7351/2（SCOM）"), "{reason}");
        assert!(reason.contains("7351/3（noun 未知）"), "{reason}");
        assert!(
            !reason.contains("7351/1"),
            "the SPCO seed is not a reason: {reason}"
        );
        assert!(with_scom.summary_line().contains("退化目录库"));
    }

    /// 种子所在的目录库不在 MDB 里 → 退化并说明，不是静默零命中。
    #[test]
    fn a_seed_from_a_catalogue_outside_the_mdb_degrades() {
        let cascade = expand_reverse_cascade(
            &[],
            &[CatalogueSeed {
                refno: RefNo::new(encode_dbno(4242), 7),
                noun: Some("SCOM".into()),
            }],
            self_as_root,
        )
        .unwrap();
        assert!(
            cascade
                .degraded
                .get(&4242)
                .is_some_and(|r| r.contains("不在 MDB 里"))
        );
        assert_eq!(
            cascade.catalogue_closure.len(),
            1,
            "the seed itself is still in the closure"
        );
    }

    #[test]
    fn no_seeds_means_an_empty_cascade_without_opening_anything() {
        let cascade = expand_reverse_cascade(
            &[SourceDb {
                dbnum: 1,
                db_type: "DESI".into(),
                file: PathBuf::from(r"Z:\does\not\exist"),
                sesno: None,
            }],
            &[],
            self_as_root,
        )
        .expect("no seeds, no I/O");
        assert_eq!(cascade, ReverseCascade::default());
    }

    /// `RefNo::word0` 里 dbno 的编码（`RefNo::dbno` 的逆）：低 13 位 + 高位段左移 13。
    fn encode_dbno(dbno: u32) -> u32 {
        (dbno & 0x1FFF) | ((dbno & 0x3E000) << 13)
    }

    /// 回执只带数与库号：账从 [`ReverseCascade`] 折出来，排工那几格由调用方填；横幅把每一格
    /// 都说出来，退化库点名，`is_degraded` 只看退化与失败。
    #[test]
    fn the_cascade_report_carries_counts_not_lists_and_names_every_bucket() {
        let mut cascade = ReverseCascade {
            seeds: vec!["5052/7".into(), "5052/8".into()],
            catalogue_closure: ["5052/7", "5052/8", "5052/1", "5052/2", "5052/3"]
                .into_iter()
                .map(String::from)
                .collect(),
            point_queries: 42,
            unreadable: vec!["5052/8".into()],
            ..ReverseCascade::default()
        };
        cascade.design_referrers.insert(
            8000,
            ["8000/1", "8000/2", "8000/3"].map(String::from).into(),
        );
        cascade.roots.insert(
            8000,
            vec![
                CascadeRoot {
                    refno: "8000/10".into(),
                    noun: "BRAN".into(),
                    name: "/B1".into(),
                },
                CascadeRoot {
                    refno: "8000/11".into(),
                    noun: "EQUI".into(),
                    name: "/E1".into(),
                },
            ],
        );
        cascade.unrooted.insert(8000, vec!["8000/3".into()]);
        cascade
            .tables_used
            .insert(5052, vec!["CATR".into(), "DTRE".into()]);
        cascade.tables_used.insert(8000, vec!["SPRE".into()]);

        let mut report = CatalogueCascadeReport::from_cascade(&cascade);
        assert_eq!(report.seeds, 2);
        assert_eq!(report.catalogue_closure, 5);
        assert_eq!(report.design_referrers, 3);
        assert_eq!(report.roots, BTreeMap::from([(8000, 2)]));
        assert_eq!(report.unrooted, 1);
        assert_eq!(report.unreadable, 1);
        assert_eq!(report.point_queries, 42);
        assert!(!report.is_degraded());
        assert_eq!(report.enqueued_total(), 0);

        report.enqueued.insert(8000, 1);
        report.already_current = 1;
        report.lazy_roots = 0;
        let line = report.banner_line();
        assert!(
            line.contains("种子 2 → 目录闭包 5 → 设计引用者 3 → 生成根 2（点查 42 次）"),
            "{line}"
        );
        assert!(line.contains("排 RegenRoot 1 根 [命中 8000:1]"), "{line}");
        assert!(line.contains("1 根所在库落款已在新目录上不重排"), "{line}");
        assert!(line.contains("1 个引用者上卷不到根"), "{line}");
        assert!(line.contains("1 个目录件记录头读不出"), "{line}");
        assert!(!line.contains("懒生成"), "{line}");
        assert!(!line.contains("盖落款的空任务"), "{line}");
        assert!(!line.contains("退化"), "{line}");

        // ADR-079：落款仍旧却没被波及的库只排盖落款的空任务，横幅点名它。
        report.stamp_only = vec![8002];
        let line = report.banner_line();
        assert!(
            line.contains("1 个库没被波及、只排盖落款的空任务 [8002]"),
            "{line}"
        );

        // GitHub #67：被监听限定挡在排工外的库不许静默——横幅点名是 `watch_dbnums` 划的。
        report.libraries_out_of_scope = vec![7997];
        let line = report.banner_line();
        assert!(
            line.contains("1 个库落款仍旧却在 watch_dbnums 限定域外、没排 [7997]"),
            "{line}"
        );
        report.libraries_out_of_scope.clear();

        report.degraded.insert(7351, "零表".into());
        report.enqueued_degraded.insert(8001, 40);
        assert!(report.is_degraded());
        assert_eq!(report.enqueued_total(), 41);
        let line = report.banner_line();
        assert!(line.contains("[退化库依赖 8001:40]"), "{line}");
        assert!(
            line.contains("退化目录库 [7351]（依赖根全部过期）"),
            "{line}"
        );

        let json = serde_json::to_value(&report).expect("serialises");
        assert!(json.get("error").is_none(), "{json}");
        assert!(json.get("roots").is_some());

        let failed = CatalogueCascadeReport::failed("open ams7000_0001: locked".into());
        assert!(failed.is_degraded());
        let line = failed.banner_line();
        assert!(line.contains("没跑成"), "{line}");
        assert!(line.contains("未知"), "{line}");
        assert!(line.contains("locked"), "{line}");
        assert!(
            serde_json::to_value(&failed)
                .expect("serialises")
                .get("error")
                .is_some()
        );
    }

    #[test]
    fn encode_dbno_inverts_refno_dbno() {
        for dbno in [1u32, 5052, 7351, 8000, 24384, 0x3FFFF] {
            assert_eq!(RefNo::new(encode_dbno(dbno), 0).dbno(), dbno, "{dbno}");
        }
    }

    // ---- P4：设计侧通用一跳（`DesignReferenceCascade`）----

    const OWN_DB: u32 = 7200;
    const PEER_DB: u32 = 7205;

    fn design_refno(dbno: u32, id: u32) -> RefNo {
        RefNo::new(encode_dbno(dbno), id)
    }

    fn own(id: u32) -> RefnoEnum {
        refno_from_e3d(design_refno(OWN_DB, id))
    }

    fn design_root(id: u32, noun: &str) -> GenerationRoot {
        GenerationRoot {
            root: own(id),
            noun: noun.into(),
            name: format!("/N{id}"),
            kind: crate::data_interface::generation_root::GenerationRootKind::Normal,
        }
    }

    /// 一次点查的产物：本库两张表（`GOBREF` 不在行为名单里、`POSREF` 在），键是变更元素 1。
    fn queried(edges: &[(&str, RefNo)]) -> DesignReferrerEdges {
        DesignReferrerEdges {
            edges: edges
                .iter()
                .map(|(attribute, referrer)| DesignReferrerEdge {
                    target: design_refno(OWN_DB, 1),
                    attribute: (*attribute).to_string(),
                    referrer: *referrer,
                })
                .collect(),
            sources: 1,
            tables_used: BTreeMap::from([(
                OWN_DB,
                vec!["GOBREF".to_string(), "POSREF".to_string()],
            )]),
            queries: 2,
            query_failures: 0,
            unreadable_dbs: BTreeMap::new(),
            over_budget: None,
            target_read_tables_walked: 0,
            target_read_entries_walked: 0,
        }
    }

    /// 引用者 → T 端 owner 链上的根：11 是桥架件、归到根 10；12 悬空无根。
    fn design_root_chain(referrer: RefNo) -> Vec<RefNo> {
        match referrer.word1 {
            10 | 11 => vec![design_refno(OWN_DB, 10)],
            _ => vec![],
        }
    }

    /// 改元素 1（在根 20 下）：闭包只碰到根 20。把 P4 的产物并进 `touched`、把它的判不了原因
    /// 交给 `WindowImpact`，再走**生产同一段** `plan_window_roots`——契约要看的是四桶分流，
    /// 不是中间那个集合。
    fn design_plan_with(
        cascade: &DesignReferenceCascade,
    ) -> crate::data_interface::window_root_plan::WindowRootPlan {
        use crate::data_interface::window_root_plan::{
            PersistedRoot, WindowImpact, plan_window_roots,
        };
        let mut touched: BTreeSet<RefnoEnum> = [own(20)].into_iter().collect();
        cascade.expand_touched(&mut touched);
        let impact = WindowImpact {
            touched,
            closure_complete: true,
            cascade_degraded: cascade.degraded.clone(),
            unresolved: 0,
            candidates: 1,
            cache_stamp: 0,
            index_keys: 1000,
        };
        let roots_t = vec![design_root(10, "CWBRAN"), design_root(20, "PIPE")];
        let roots_s = vec![
            PersistedRoot {
                refno: own(10),
                noun: "CWBRAN".into(),
            },
            PersistedRoot {
                refno: own(20),
                noun: "PIPE".into(),
            },
        ];
        plan_window_roots(&roots_t, &roots_s, &impact)
    }

    fn regen_ids(
        plan: &crate::data_interface::window_root_plan::WindowRootPlan,
    ) -> Vec<crate::data_interface::window_root_plan::PersistedRoot> {
        plan.regen
            .iter()
            .map(
                |root| crate::data_interface::window_root_plan::PersistedRoot {
                    refno: root.root,
                    noun: root.noun.clone(),
                },
            )
            .collect()
    }

    /// 契约第一态：`POSREF` 在表里有引用者 → 引用它的桥架件那个根进 `regen`，而它本来只会
    /// `advance`（这一窗的闭包根本没碰到它）。
    #[test]
    fn a_posref_referrer_pulls_its_own_root_into_regen() {
        let cascade = DesignReferenceCascade::from_edges(
            &queried(&[("POSREF", design_refno(OWN_DB, 11))]),
            OWN_DB,
            design_root_chain,
        );
        assert_eq!(cascade.referrer_roots, BTreeSet::from([own(10)]));
        assert_eq!(
            cascade.edges_by_attribute,
            BTreeMap::from([("POSREF".to_string(), 1)])
        );
        assert!(cascade.degraded.is_none());

        let plan = design_plan_with(&cascade);
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![own(10), own(20)]
        );
        assert!(plan.advance.is_empty());
    }

    /// 契约第二态：不在行为名单里的属性**只记账**。`GOBREF` 有引用者也不动 `touched`——
    /// 今天的几何一个都不读它们，多算就是白重算一批根。
    #[test]
    fn a_non_behavioural_attribute_is_counted_and_changes_nothing() {
        let cascade = DesignReferenceCascade::from_edges(
            &queried(&[("GOBREF", design_refno(OWN_DB, 11))]),
            OWN_DB,
            design_root_chain,
        );
        assert_eq!(
            cascade.edges_by_attribute,
            BTreeMap::from([("GOBREF".to_string(), 1)])
        );
        assert!(cascade.referrer_roots.is_empty());
        assert_eq!(cascade.queries, 2);

        let plan = design_plan_with(&cascade);
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![own(20)]
        );
        assert_eq!(plan.advance, vec![own(10)]);
    }

    /// 契约第三态：开关关 → 一个成员库都没开，`touched` 一字不改，四桶与引入前逐字相同。
    #[test]
    fn the_switch_off_leaves_touched_untouched() {
        let cascade = DesignReferenceCascade::off();
        assert!(!cascade.enabled);
        let mut touched: BTreeSet<RefnoEnum> = [own(20)].into_iter().collect();
        assert_eq!(cascade.expand_touched(&mut touched), 0);
        assert_eq!(touched, BTreeSet::from([own(20)]));

        let plan = design_plan_with(&cascade);
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![own(20)]
        );
        assert_eq!(plan.advance, vec![own(10)]);
        assert!(cascade.summary_line(OWN_DB).contains("cascade off"));
    }

    /// 引用者在**别的**设计库里：本窗口排不了它（`roots_T` 是本库的），所以记成在案的跨库
    /// 缺口——但**不判退化**，否则每有一条跨库边就把全库根打翻一遍。
    #[test]
    fn a_referrer_in_another_design_db_is_recorded_not_dropped() {
        let cascade = DesignReferenceCascade::from_edges(
            &queried(&[("POSREF", design_refno(PEER_DB, 11))]),
            OWN_DB,
            |_| panic!("别的库的引用者不该拿本库的 owner 索引上卷"),
        );
        assert_eq!(cascade.foreign_referrers, BTreeMap::from([(PEER_DB, 1)]));
        assert!(cascade.referrer_roots.is_empty());
        assert!(cascade.degraded.is_none());
        assert!(
            cascade.summary_line(OWN_DB).contains("在案跨库缺口"),
            "{}",
            cascade.summary_line(OWN_DB)
        );
    }

    /// 引用者上卷不到根（悬空表项）：记账，不加根。
    #[test]
    fn a_referrer_without_a_root_is_counted() {
        let cascade = DesignReferenceCascade::from_edges(
            &queried(&[("POSREF", design_refno(OWN_DB, 12))]),
            OWN_DB,
            design_root_chain,
        );
        assert_eq!(cascade.referrer_without_root, 1);
        assert!(cascade.referrer_roots.is_empty());
        assert!(cascade.degraded.is_none());
    }

    /// **判不了 ≠ 没有引用者。** 成员库打不开 / 表读挂时空集与「真的没人引用」长得一模一样，
    /// 所以退到今天的行为：放弃凭证前移，已持久化的根全部重算，原因在 `degraded_reason` 里
    /// 说得出是反查没答上来。
    #[test]
    fn a_member_db_that_will_not_open_degrades_instead_of_answering_empty() {
        let mut edges = queried(&[]);
        edges
            .unreadable_dbs
            .insert(PEER_DB, "open dbnum 7205: locked".into());
        let cascade = DesignReferenceCascade::from_edges(&edges, OWN_DB, design_root_chain);
        let reason = cascade.degraded.clone().expect("打不开就是判不了");
        assert!(reason.contains("7205"), "{reason}");

        let plan = design_plan_with(&cascade);
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![own(10), own(20)],
            "判不了就把已持久化的根全部重算"
        );
        assert!(plan.advance.is_empty());
        let degraded = plan.degraded.expect("退化原因要落进计划");
        assert!(degraded.contains("design-reference cascade"), "{degraded}");

        let mut read_failure = queried(&[]);
        read_failure.query_failures = 3;
        let cascade = DesignReferenceCascade::from_edges(&read_failure, OWN_DB, design_root_chain);
        assert!(
            cascade
                .degraded
                .is_some_and(|reason| reason.contains("3 次点查读挂"))
        );
    }

    /// 预算钉在**点查次数**上，而不是变更元素数：同样的变更量，逐库并集把成本乘上了成员库
    /// 个数。超预算时一次点查都不发，而且照样判不了——`tables_used` 仍是实的（库开过、表清单
    /// 读过），这样回执里说得出「本该发多少次」。
    #[test]
    fn the_budget_counts_point_queries_and_spends_none_when_it_blows() {
        let edges = read_design_referrer_edges(
            &[design_refno(OWN_DB, 1), design_refno(OWN_DB, 2)],
            &[SourceDb {
                dbnum: PEER_DB,
                db_type: "DESI".into(),
                file: PathBuf::from(r"Z:\does\not\exist"),
                sesno: None,
            }],
            0,
        );
        // 库都打不开 → 没有表 → 预算是 0 次，不该判成超预算。
        assert_eq!(edges.over_budget, None);

        let mut blown = queried(&[]);
        blown.sources = 7_139;
        blown.over_budget = Some(35_695);
        blown.queries = 0;
        let cascade = DesignReferenceCascade::from_edges(&blown, OWN_DB, design_root_chain);
        assert!(cascade.enabled);
        assert_eq!(cascade.queries, 0);
        assert!(cascade.referrer_roots.is_empty());
        let reason = cascade.degraded.clone().expect("超预算就是判不了");
        assert!(
            reason.contains("35695") && reason.contains("一次都没发"),
            "{reason}"
        );
        assert!(design_plan_with(&cascade).advance.is_empty());
    }

    /// 本库无论在不在 MDB 名单里都要查，而且**钉 T 端**；名单里那一份不能再开一次（重复点查）。
    #[test]
    fn the_window_pins_its_own_db_at_the_target_session() {
        let peer = SourceDb {
            dbnum: PEER_DB,
            db_type: "DESI".into(),
            file: PathBuf::from(r"Z:\peer"),
            sesno: Some(9),
        };
        let stale_own = SourceDb {
            dbnum: OWN_DB,
            db_type: "DESI".into(),
            file: PathBuf::from(r"Z:\own"),
            sesno: Some(3),
        };
        let dbs =
            design_dbs_for_window(OWN_DB, Path::new(r"Z:\own"), 17, &[peer.clone(), stale_own]);
        assert_eq!(
            dbs,
            vec![
                SourceDb {
                    dbnum: OWN_DB,
                    db_type: "DESI".into(),
                    file: PathBuf::from(r"Z:\own"),
                    sesno: Some(17),
                },
                peer.clone(),
            ]
        );
        // 名单为空 = 只查本库。
        assert_eq!(
            design_dbs_for_window(OWN_DB, Path::new(r"Z:\own"), 17, &[])
                .into_iter()
                .map(|source| source.dbnum)
                .collect::<Vec<_>>(),
            vec![OWN_DB]
        );
    }

    /// 行为名单就一条，多一条都不行——每加一个名字都是「这一窗要多重算一批根」，
    /// 依据只能是 P0 差表里「e3d-model 几何真读它」。
    #[test]
    fn the_behavioural_attribute_list_is_exactly_posref() {
        assert_eq!(DESIGN_CASCADE_ATTRIBUTES, ["POSREF"]);
        for attribute in ["GOBREF", "HTPREF", "HDBREF", "GRDREF", "STAREF", "SPRE"] {
            assert!(
                !DESIGN_CASCADE_ATTRIBUTES.contains(&attribute),
                "{attribute} 今天的几何不读，不该进 touched"
            );
        }
    }

    /// 没有变更元素 = 不开任何库、不查任何表，但**是开着的**（与开关关那一态区分得开）。
    #[test]
    fn no_changed_elements_means_no_io() {
        let edges = read_design_referrer_edges(
            &[],
            &[SourceDb {
                dbnum: PEER_DB,
                db_type: "DESI".into(),
                file: PathBuf::from(r"Z:\does\not\exist"),
                sesno: None,
            }],
            DESIGN_CASCADE_QUERY_BUDGET,
        );
        assert_eq!(edges, DesignReferrerEdges::default());
        let cascade = DesignReferenceCascade::from_edges(&edges, OWN_DB, design_root_chain);
        assert!(cascade.enabled);
        assert!(cascade.degraded.is_none());
        assert_eq!(cascade.queries, 0);
    }

    /// 库打不开进的是 `unreadable_dbs`（未知），不是「零引用者」。
    #[test]
    fn an_unopenable_db_lands_in_the_unreadable_bucket() {
        let edges = read_design_referrer_edges(
            &[design_refno(OWN_DB, 1)],
            &[SourceDb {
                dbnum: PEER_DB,
                db_type: "DESI".into(),
                file: PathBuf::from(r"Z:\does\not\exist"),
                sesno: None,
            }],
            DESIGN_CASCADE_QUERY_BUDGET,
        );
        assert_eq!(edges.sources, 1);
        assert_eq!(edges.queries, 0);
        assert!(edges.unreadable_dbs.contains_key(&PEER_DB));
        assert!(edges.tables_used.is_empty());
    }

    #[test]
    fn the_summary_line_tells_every_bucket() {
        let cascade = DesignReferenceCascade::from_edges(
            &queried(&[
                ("POSREF", design_refno(OWN_DB, 11)),
                ("GOBREF", design_refno(OWN_DB, 11)),
            ]),
            OWN_DB,
            design_root_chain,
        );
        let line = cascade.summary_line(OWN_DB);
        assert!(line.contains("cascade on"), "{line}");
        assert!(line.contains("edges=[GOBREF:1 POSREF:1]"), "{line}");
        assert!(line.contains("referrer_roots=1"), "{line}");
        assert!(line.contains("dbs_queried=1"), "{line}");

        let mut blown = queried(&[]);
        blown.over_budget = Some(9_999_999);
        let capped = DesignReferenceCascade::from_edges(&blown, OWN_DB, design_root_chain)
            .summary_line(OWN_DB);
        assert!(
            capped.contains("判不了") && capped.contains("放弃凭证前移"),
            "{capped}"
        );
    }

    // ---- GitHub #143：引用者读的是被引用件身上的格（`COMPRE` → `LSTU` / `HSTU`）----

    /// 被支撑的管件换了规格（`LSTU` / `HSTU` 在窗口两端取值不同）→ 经 `COMPRE` 指着它的支撑件那个根进 `regen`，
    /// 而它本来只会 `advance`（这一窗的闭包根本没碰到它）。问两端的就是名单上那两格。
    #[test]
    fn a_support_is_regenerated_when_the_component_it_points_at_changed_its_tube_reference() {
        let mut asked: Vec<(RefNo, Vec<String>)> = Vec::new();
        let cascade = DesignReferenceCascade::from_edges_with(
            &queried(&[("COMPRE", design_refno(OWN_DB, 11))]),
            OWN_DB,
            design_root_chain,
            |target, reads| {
                asked.push((target, reads.iter().map(|read| read.to_string()).collect()));
                true
            },
        );
        assert_eq!(
            asked,
            vec![(
                design_refno(OWN_DB, 1),
                vec!["LSTU".to_string(), "HSTU".to_string()]
            )],
            "问的是被引用件、名单上那两格，问一次"
        );
        assert_eq!(cascade.referrer_roots, BTreeSet::from([own(10)]));
        assert_eq!(
            cascade.target_reads_triggered,
            BTreeMap::from([("COMPRE".to_string(), 1)])
        );
        assert!(cascade.target_reads_unchanged.is_empty());
        assert!(cascade.handoff_referrers.is_empty());
        assert!(cascade.degraded.is_none());
        let line = cascade.summary_line(OWN_DB);
        assert!(
            line.contains("target_reads=[COMPRE→LSTU|HSTU:1 changed/0 unchanged]"),
            "{line}"
        );
        assert!(line.contains("referrer_roots=1"), "{line}");

        let plan = design_plan_with(&cascade);
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![own(10), own(20)]
        );
        assert!(plan.advance.is_empty());
    }

    /// 不相关的改动不把支撑件拉进来：被支撑的管件只是挪了位置 / 改了名（那两格两端相同），边查得到、记了账，
    /// 支撑件的根留在 `advance`——不为保险多算。
    #[test]
    fn a_component_that_changed_nothing_its_support_reads_leaves_the_support_alone() {
        let cascade = DesignReferenceCascade::from_edges_with(
            &queried(&[("COMPRE", design_refno(OWN_DB, 11))]),
            OWN_DB,
            |_| panic!("没触发的边不该去上卷引用者"),
            |_, _| false,
        );
        assert_eq!(
            cascade.edges_by_attribute,
            BTreeMap::from([("COMPRE".to_string(), 1)])
        );
        assert_eq!(
            cascade.target_reads_unchanged,
            BTreeMap::from([("COMPRE".to_string(), 1)])
        );
        assert!(cascade.target_reads_triggered.is_empty());
        assert!(cascade.referrer_roots.is_empty());
        assert!(cascade.handoff_referrers.is_empty());
        assert!(cascade.degraded.is_none());
        let line = cascade.summary_line(OWN_DB);
        assert!(
            line.contains("target_reads=[COMPRE→LSTU|HSTU:0 changed/1 unchanged]"),
            "{line}"
        );

        let plan = design_plan_with(&cascade);
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![own(20)]
        );
        assert_eq!(plan.advance, vec![own(10)]);
    }

    /// 跨库：支撑件在别的设计库里（APS 样例工程 234 条 `COMPRE` 边里的 224 条）。本窗的计划装不下它的根
    /// （`roots_T` 是本库的），所以不上卷、也不只是计数——点名交给跨库交接，按它所在的库分组。
    #[test]
    fn a_support_in_another_design_db_is_handed_off_to_that_db() {
        let support = design_refno(PEER_DB, 11);
        let other = design_refno(PEER_DB, 12);
        let cascade = DesignReferenceCascade::from_edges_with(
            &queried(&[("COMPRE", support), ("COMPRE", other), ("COMPRE", support)]),
            OWN_DB,
            |_| panic!("别的库的引用者不该拿本库的 owner 索引上卷"),
            |_, _| true,
        );
        assert_eq!(
            cascade.handoff_referrers,
            vec![support, other],
            "去重、按出现序"
        );
        assert!(
            cascade.foreign_referrers.is_empty(),
            "交接了的不记进「排不了」那一格"
        );
        assert!(cascade.referrer_roots.is_empty());
        assert!(cascade.degraded.is_none());
        assert_eq!(
            crate::data_interface::cross_db_handoff::elements_by_dbnum(
                cascade.handoff_referrer_elements()
            ),
            BTreeMap::from([(PEER_DB, vec![support.to_string(), other.to_string()])])
        );
        let line = cascade.summary_line(OWN_DB);
        assert!(line.contains("交给跨库交接"), "{line}");
        assert!(!line.contains("在案跨库缺口"), "{line}");

        // 没触发就不交接。
        let quiet = DesignReferenceCascade::from_edges_with(
            &queried(&[("COMPRE", support)]),
            OWN_DB,
            |_| panic!("别的库的引用者不该拿本库的 owner 索引上卷"),
            |_, _| false,
        );
        assert!(quiet.handoff_referrers.is_empty());
        assert!(quiet.foreign_referrers.is_empty());
    }

    /// 一只构件被几只支撑件指着：两端只读一遍（判词按被引用件记），每条边都按这一个判词走——同库的上卷，
    /// 别的库里的交接。
    #[test]
    fn the_two_ends_are_read_once_per_referenced_element() {
        let mut calls = 0usize;
        let cascade = DesignReferenceCascade::from_edges_with(
            &queried(&[
                ("COMPRE", design_refno(OWN_DB, 11)),
                ("COMPRE", design_refno(OWN_DB, 10)),
                ("COMPRE", design_refno(PEER_DB, 5)),
            ]),
            OWN_DB,
            design_root_chain,
            |_, _| {
                calls += 1;
                true
            },
        );
        assert_eq!(calls, 1);
        assert_eq!(
            cascade.target_reads_triggered,
            BTreeMap::from([("COMPRE".to_string(), 3)])
        );
        assert_eq!(cascade.referrer_roots, BTreeSet::from([own(10)]));
        assert_eq!(cascade.handoff_referrers, vec![design_refno(PEER_DB, 5)]);
    }

    /// 反查超预算的窗（一窗动了上千个元素——成批换规格就是这个样子）：点查一次都不发，整窗判不了、本库已发布的根
    /// 全部重算；带条件那一类的表整张走出来的边照常过判词，别的库里的支撑件照常点名交接——那个库没有谁替它们兜底。
    #[test]
    fn an_over_budget_window_still_hands_off_the_supports_in_other_design_dbs() {
        let support = design_refno(PEER_DB, 11);
        let mut walked = queried(&[("COMPRE", support), ("COMPRE", design_refno(OWN_DB, 11))]);
        walked.sources = 7_139;
        walked.queries = 0;
        walked.over_budget = Some(1_934_669);
        walked.target_read_tables_walked = 4;
        walked.target_read_entries_walked = 234;
        let cascade =
            DesignReferenceCascade::from_edges_with(&walked, OWN_DB, design_root_chain, |_, _| {
                true
            });
        assert_eq!(cascade.handoff_referrers, vec![support]);
        assert_eq!(cascade.referrer_roots, BTreeSet::from([own(10)]));
        let reason = cascade.degraded.clone().expect("超预算仍然是判不了");
        assert!(
            reason.contains("一次都没发") && reason.contains("4 张表整张走了一遍、234 条表项"),
            "{reason}"
        );
        let line = cascade.summary_line(OWN_DB);
        assert!(
            line.contains("交给跨库交接") && line.contains("判不了"),
            "{line}"
        );
        // 本库：判不了 → 已发布的根全部重算，没有一个只前移。
        let plan = design_plan_with(&cascade);
        assert!(plan.advance.is_empty());
        assert_eq!(plan.regen.len(), 2);

        // 被引用件那两格没变：超预算也不交接（不为保险多算）。
        let quiet =
            DesignReferenceCascade::from_edges_with(&walked, OWN_DB, |_| Vec::new(), |_, _| false);
        assert!(quiet.handoff_referrers.is_empty());
        assert!(quiet.degraded.is_some());

        // 没走过表（库里没有这一类的表）时原因不提这一句。
        let mut bare = queried(&[]);
        bare.over_budget = Some(35_695);
        let reason = DesignReferenceCascade::from_edges(&bare, OWN_DB, design_root_chain)
            .degraded
            .expect("超预算就是判不了");
        assert!(!reason.contains("整张走了一遍"), "{reason}");
    }

    /// 没有窗口两端可比的调用方（`from_edges`：探针与只看 `POSREF` 的测试）：带条件那一类一条都不触发、只记账；
    /// `POSREF` 不看这个判词，照旧无条件上卷。
    #[test]
    fn without_the_two_ends_the_conditional_attributes_only_count() {
        let cascade = DesignReferenceCascade::from_edges(
            &queried(&[
                ("COMPRE", design_refno(OWN_DB, 12)),
                ("POSREF", design_refno(OWN_DB, 11)),
            ]),
            OWN_DB,
            design_root_chain,
        );
        assert_eq!(
            cascade.target_reads_unchanged,
            BTreeMap::from([("COMPRE".to_string(), 1)])
        );
        assert!(cascade.target_reads_triggered.is_empty());
        assert_eq!(
            cascade.referrer_roots,
            BTreeSet::from([own(10)]),
            "POSREF 照旧无条件上卷"
        );
        assert_eq!(cascade.referrer_without_root, 0, "没触发的边不去上卷");

        let posref_only = DesignReferenceCascade::from_edges_with(
            &queried(&[("POSREF", design_refno(OWN_DB, 11))]),
            OWN_DB,
            design_root_chain,
            |_, _| panic!("POSREF 不在带条件的名单上，不该问两端"),
        );
        assert_eq!(posref_only.referrer_roots, BTreeSet::from([own(10)]));
        assert!(posref_only.target_reads_triggered.is_empty());
        assert!(posref_only.target_reads_unchanged.is_empty());
    }

    /// 两端比对的口径：逐格比；读不出来的一格按「变了」算（「没变」与「不知道」长得一样时宁多算不漏算）；
    /// 两端都没有这一格（桥架 `CTRAY` 既没有 `LSTU` 也没有 `HSTU`）不算变。
    #[test]
    fn the_two_end_comparison_treats_an_unreadable_read_as_changed() {
        use ReferenceRead::{Absent, Target, Unreadable};
        let spec = |id: u32| Target(design_refno(7000, id));
        assert!(!reference_reads_differ(
            &[spec(1), Absent],
            &[spec(1), Absent]
        ));
        assert!(!reference_reads_differ(
            &[Absent, Absent],
            &[Absent, Absent]
        ));
        assert!(
            reference_reads_differ(&[spec(1), Absent], &[spec(2), Absent]),
            "换规格"
        );
        assert!(
            reference_reads_differ(&[Absent, spec(1)], &[Absent, spec(2)]),
            "支管换管头规格"
        );
        assert!(
            reference_reads_differ(&[spec(1), Absent], &[Absent, Absent]),
            "被删 / 清空"
        );
        assert!(
            reference_reads_differ(&[Absent, Absent], &[spec(1), Absent]),
            "新建"
        );
        assert!(reference_reads_differ(
            &[Unreadable, Absent],
            &[Unreadable, Absent]
        ));
        assert!(reference_reads_differ(
            &[spec(1), Absent],
            &[Unreadable, Absent]
        ));
    }

    /// 名单与 e3d-io 的读点钉在一起：`ATTRIB POD` 经 `COMPRE` 走到被支撑的构件之后，在那只构件上读的入口属性
    /// 恰好是名单上那两格，也只在这一处读。e3d-io 多读一格（或换了读法）这条就红——回来把名单补上，否则那一格
    /// 变了支撑件不会跟着重算。
    #[test]
    fn the_conditional_list_matches_what_e3d_io_reads_on_the_referenced_component() {
        assert_eq!(
            DESIGN_CASCADE_TARGET_READS,
            [("COMPRE", &["LSTU", "HSTU"][..])]
        );
        assert!(
            !DESIGN_CASCADE_ATTRIBUTES.contains(&"COMPRE"),
            "COMPRE 只走带条件的那一张：被支撑的构件挪位置不该让支撑件重算"
        );
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/shared/e3d-io/src/db_param_env.rs"),
        )
        .expect("e3d-io src/db_param_env.rs 可读")
        .replace("\r\n", "\n");
        let production = match source.find("#[cfg(test)]") {
            Some(cut) => &source[..cut],
            None => source.as_str(),
        };
        assert!(
            production.contains("const COMPONENT_REFERENCE: &str = \"COMPRE\";"),
            "e3d-io 不再把 COMPRE 记在 COMPONENT_REFERENCE 里"
        );
        assert!(
            production.contains("let target = match design.get_element(COMPONENT_REFERENCE) {"),
            "被支撑的构件不再是经设计件的 COMPRE 取到的"
        );
        // 形态 A / B 都在 `for attribute in [...]` 里问一遍，属性名取自这张数组。
        let marker = "for attribute in [";
        let reads: Vec<&str> = production
            .match_indices(marker)
            .map(|(at, matched)| {
                let rest = &production[at + matched.len()..];
                &rest[..rest.find(']').expect("closing bracket")]
            })
            .collect();
        assert_eq!(
            reads,
            [r#""LSTU", "HSTU""#],
            "e3d-io 在 COMPRE 的目标上读的入口属性变了"
        );
        // 每一格都经 `spec_reference(&target, attribute)` 取，不另开读法。
        assert_eq!(
            production.matches("spec_reference(&target, attribute)").count(),
            reads.len(),
            "被支撑的构件多了一处别的读法：核一遍读的是哪一格"
        );
    }

    const APS_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000";

    /// 一张反向表的全部表项 `(被引用件, 引用者)`；库打不开 / 没有这张表给空。
    fn reference_table_entries(file: &Path, table: &str) -> Vec<(RefNo, RefNo)> {
        let hash = e3d_attlib::db1_hash(table);
        let Ok(mut engine) = ReadOnlyEngine::open(file) else {
            return Vec::new();
        };
        if !engine.tables().iter().any(|entry| entry.name_hash == hash) {
            return Vec::new();
        }
        let entries = match engine.reference_table(hash) {
            Some(mut reference) => reference
                .iter()
                .filter_map(Result::ok)
                .map(|entry| (entry.target, entry.referrer))
                .collect(),
            None => Vec::new(),
        };
        entries
    }

    /// P3 的再一跳（GitHub #143）：管子的目录件变了 → 引用它的管件（`LSTU` / `HSTU` 表）自己被点到；经 `COMPRE` 指着
    /// 管件的支撑件读的是同一个目录件、却不直接引用它——要靠再一跳才点得到，而且落在它自己那个库的引用者里。
    ///
    /// 语料里没有「只改管子目录件」的自然窗口，手法与 `live_ams7000_…` 的退路相同：从文件里现成的一条链
    /// （支撑件 —`COMPRE`→ 管件 —`LSTU` / `HSTU`→ 目录件）取那个目录件当假定的变更源。只读反向表，不要模板目录；
    /// APS 样例工程不在本机就跳过。
    #[test]
    fn a_catalogue_change_reaches_the_supports_that_point_at_the_components_using_it() {
        let dir = Path::new(APS_DIR);
        if !dir.is_dir() {
            println!("skip: the APS sample project is not installed");
            return;
        }
        let sources = sources_in_dir(dir, &["DESI", "CATA"]).expect("sources");
        let design_file = |dbnum: u32| {
            sources
                .iter()
                .find(|source| source.is_design() && source.dbnum == dbnum)
                .map(|source| source.file.clone())
        };
        let mut chain: Option<(RefNo, RefNo, RefNo, &str)> = None;
        'search: for source in sources.iter().filter(|source| source.is_design()) {
            for (component, support) in reference_table_entries(&source.file, "COMPRE") {
                let Some(component_file) = design_file(component.dbno()) else {
                    continue;
                };
                for read in ["LSTU", "HSTU"] {
                    let found = reference_table_entries(&component_file, read)
                        .into_iter()
                        .find(|(_, referrer)| *referrer == component);
                    if let Some((catalogue_item, _)) = found {
                        chain = Some((support, component, catalogue_item, read));
                        break 'search;
                    }
                }
            }
        }
        let Some((support, component, catalogue_item, read)) = chain else {
            println!(
                "skip: no support in the corpus points at a component that carries LSTU / HSTU"
            );
            return;
        };
        println!(
            "chain: support {} -COMPRE-> component {} -{read}-> catalogue item {}",
            text(support),
            text(component),
            text(catalogue_item)
        );

        let seed = CatalogueSeed {
            refno: catalogue_item,
            noun: None,
        };
        let cascade = expand_reverse_cascade(&sources, &[seed], self_as_root).expect("cascade");
        println!("{}", cascade.summary_line());
        assert!(
            cascade
                .design_referrers
                .get(&component.dbno())
                .is_some_and(|referrers| referrers.contains(&text(component))),
            "管件自己经 {read} 表被点到"
        );
        assert!(
            cascade
                .dependent_referrers
                .get(&support.dbno())
                .is_some_and(|referrers| referrers.contains(&text(support))),
            "支撑件不直接引用目录件，只有再一跳点得到：{:?}",
            cascade.dependent_referrers
        );
        assert!(
            cascade
                .design_referrers
                .get(&support.dbno())
                .is_some_and(|referrers| referrers.contains(&text(support))),
            "再一跳找到的支撑件并进它所在库的引用者"
        );
        let support_root = refno_from_e3d(support).to_pdms_str();
        assert!(
            cascade
                .roots
                .get(&support.dbno())
                .is_some_and(|roots| roots.iter().any(|root| root.refno == support_root)),
            "…并且一样上卷成根（这里的 resolver 把引用者自己当根）"
        );
    }

    /// 超预算时整张走一遍，与点查答的是同一件事（GitHub #143）：APS 样例工程里每张 `COMPRE` 表各取头一条边，把
    /// 被引用件当变更源——预算给 0（必超）走出来的边，与预算放开点查出来的 `COMPRE` 边逐条相同；点查一次都不发，
    /// 别的表一条边都不出。APS 样例工程不在本机就跳过。
    #[test]
    fn walking_the_conditional_tables_finds_the_same_edges_as_point_queries() {
        let dir = Path::new(APS_DIR);
        if !dir.is_dir() {
            println!("skip: the APS sample project is not installed");
            return;
        }
        let sources = sources_in_dir(dir, &["DESI"]).expect("sources");
        let mut targets: Vec<RefNo> = sources
            .iter()
            .filter_map(|source| {
                reference_table_entries(&source.file, "COMPRE")
                    .into_iter()
                    .next()
                    .map(|(component, _)| component)
            })
            .collect();
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            println!("skip: no design database in the corpus carries a COMPRE table");
            return;
        }

        let walked = read_design_referrer_edges(&targets, &sources, 0);
        assert!(walked.over_budget.is_some(), "预算 0：必超");
        assert_eq!(walked.queries, 0, "超预算：点查一次都不发");
        assert!(walked.target_read_tables_walked > 0);
        assert!(walked.target_read_entries_walked >= walked.edges.len());
        assert!(
            walked.edges.iter().all(|edge| edge.attribute == "COMPRE"),
            "只走带条件那一类的表"
        );
        assert_eq!(walked.query_failures, 0);

        let queried = read_design_referrer_edges(&targets, &sources, usize::MAX);
        assert!(queried.over_budget.is_none());
        assert_eq!(queried.target_read_tables_walked, 0, "没超预算不走整表");
        let pairs = |edges: &DesignReferrerEdges| -> BTreeSet<(RefKey, RefKey)> {
            edges
                .edges
                .iter()
                .filter(|edge| edge.attribute == "COMPRE")
                .map(|edge| (key(edge.target), key(edge.referrer)))
                .collect()
        };
        println!(
            "{} 个被引用件：整表走 {} 张 / {} 条表项，留下 {} 条边；点查 {} 次，COMPRE 边 {} 条",
            targets.len(),
            walked.target_read_tables_walked,
            walked.target_read_entries_walked,
            walked.edges.len(),
            queried.queries,
            pairs(&queried).len()
        );
        assert!(!pairs(&walked).is_empty());
        assert_eq!(pairs(&walked), pairs(&queried));
        assert_eq!(walked.edges.len(), pairs(&walked).len(), "没有重复的边");

        // 走出来的边照常过判词：别的库里的支撑件被点名。
        let own_db = targets[0].dbno();
        let cascade =
            DesignReferenceCascade::from_edges_with(&walked, own_db, |_| Vec::new(), |_, _| true);
        assert!(cascade.degraded.is_some());
        let foreign = walked
            .edges
            .iter()
            .filter(|edge| edge.referrer.dbno() != own_db)
            .map(|edge| key(edge.referrer))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            cascade
                .handoff_referrers
                .iter()
                .copied()
                .map(key)
                .collect::<BTreeSet<_>>(),
            foreign
        );
    }

    // ---- 真库（手动）：POSREF 反查与两态对拍 ----

    /// 语料里有 `POSREF` 表的设计库（P0 差表：DESI 31 库 / 7 109 条，目标 0 跨库）。
    /// 门做两件事：一是**反查真的答得出**（拿表里现成的一条边当变更源，引用者的根必须被拉进
    /// `regen`，而它在闭包里本来不在）；二是**现有窗口两态逐字相同**（今天的自然窗口里
    /// 没有「只改被参照件、参照它的桥架件不动」的形状，所以开关不该改变任何一窗的结果）。
    #[cfg(test)]
    mod live {
        use std::path::PathBuf;
        use std::sync::Arc;

        use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
        use e3d_io::session::SessionSelector;

        use super::*;
        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::{
            enumerate_generation_roots, resolve_delivery_unit_types,
        };
        use crate::data_interface::window_root_plan::{
            PersistedRoot, WindowRootSources, plan_window_roots,
        };
        use crate::fast_model::e3d_model_service::scan_index;

        /// 本机语料的项目数据目录。P0 差表扫的就是这几个（783 库）；`AIOS_POSREF_DIRS`
        /// （`;` 分隔）可整体替换。
        const CORPUS_DIRS: [&str; 5] = [
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000",
            r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000",
            r"D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000",
        ];

        fn corpus_dirs() -> Vec<PathBuf> {
            match std::env::var("AIOS_POSREF_DIRS") {
                Ok(value) => value.split(';').map(PathBuf::from).collect(),
                Err(_) => CORPUS_DIRS.iter().map(PathBuf::from).collect(),
            }
        }

        /// 语料里**全部**带 `POSREF` 表的设计库，按目录分组。写死一个不行：P0 差表说 783 库里
        /// 只有 31 个有这张表，哪些有得现问；只挑第一个也不行——一个库答对了不代表这条路对。
        fn posref_design_dbs() -> Vec<(PathBuf, Vec<SourceDb>)> {
            let hash = e3d_attlib::db1_hash("POSREF");
            let mut out = Vec::new();
            for dir in corpus_dirs() {
                if !dir.is_dir() {
                    continue;
                }
                let Ok(sources) = sources_in_dir(&dir, &["DESI"]) else {
                    continue;
                };
                let carriers: Vec<SourceDb> = sources
                    .into_iter()
                    .filter(|source| {
                        let Ok(mut engine) = ReadOnlyEngine::open(&source.file) else {
                            return false;
                        };
                        engine.tables().iter().any(|table| table.name_hash == hash)
                            && engine.reference_table(hash).is_some()
                    })
                    .collect();
                if !carriers.is_empty() {
                    out.push((dir, carriers));
                }
            }
            if out.is_empty() {
                println!("skip: no DESI database in the corpus carries a POSREF table");
            }
            out
        }

        fn posref_hash() -> u32 {
            e3d_attlib::db1_hash("POSREF")
        }

        fn open(schema: &DirectSchema, file: &Path, sesno: u32) -> Arc<DbSet> {
            let set = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            set.add_db(DbFilePin {
                file: file.to_path_buf(),
                template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
                db_type: Some("DESI".into()),
                sesno: Some(sesno),
            })
            .expect("pin DESI");
            set
        }

        /// **每一个**带 `POSREF` 表的设计库都要答得出：拿它表里现成的一条边当假定的变更源
        /// （语料里没有「只改被参照件」的自然窗口，与 C8 的 `live_zdj7200_…` 同一手法），
        /// 反查回来的根必须与正向沿 owner 链算出的期望逐字相等。
        ///
        /// 同时**真的走一次 MDB 逐库并集**：peers 取同目录下其余带 `POSREF` 表的库。
        /// P0 差表说 `POSREF` 目标 0 跨库，所以 `foreign_referrers` 该恒为空——非空即是新事实，
        /// 这条门就是那个报警器。
        #[test]
        #[ignore = "manual live: needs DESI databases that carry a POSREF table and the E3D template directory"]
        fn live_every_posref_table_answers_its_own_referrers() {
            let corpus = posref_design_dbs();
            if corpus.is_empty() {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let (mut checked, mut skipped_empty) = (0usize, 0usize);
            for (dir, carriers) in &corpus {
                println!(
                    "=== {} : {} 个库带 POSREF 表 ===",
                    dir.display(),
                    carriers.len()
                );
                for carrier in carriers {
                    let file = &carrier.file;
                    let mut engine = ReadOnlyEngine::open(file).expect("open the design file");
                    let sesno = engine.session().sesno;
                    let dbnum = engine.descriptor().db_mark;
                    let edge = engine
                        .reference_table(posref_hash())
                        .expect("POSREF 表打得开")
                        .iter()
                        .filter_map(Result::ok)
                        .map(|entry| (entry.target, entry.referrer))
                        .next();
                    drop(engine);
                    let Some((target, referrer)) = edge else {
                        // 表在但一条边都没有：是「这个库不留引用者」，不是「没查成」。
                        skipped_empty += 1;
                        continue;
                    };

                    let index = scan_index(file, Some(sesno)).expect("scan index");
                    let set = open(&schema, file, sesno);
                    let roots_t =
                        enumerate_generation_roots(&set, &index.roots, &unit_types).expect("roots");
                    let root_keys: BTreeSet<(u32, u32)> = roots_t
                        .iter()
                        .map(|root| {
                            let refno = refno_to_e3d(root.root);
                            (refno.word0, refno.word1)
                        })
                        .collect();
                    // 逐库并集：本库钉 T 端，peers = 同目录其余带 POSREF 表的库。
                    let dbs = design_dbs_for_window(dbnum, file, sesno, carriers);
                    let edges =
                        read_design_referrer_edges(&[target], &dbs, DESIGN_CASCADE_QUERY_BUDGET);
                    let cascade = DesignReferenceCascade::from_edges(&edges, dbnum, |element| {
                        roots_on_owner_chain(&index.owners, &root_keys, element)
                    });
                    println!("  {}", cascade.summary_line(dbnum));

                    assert!(cascade.degraded.is_none(), "{:?}", cascade.degraded);
                    assert!(
                        cascade
                            .edges_by_attribute
                            .get("POSREF")
                            .is_some_and(|count| *count >= 1),
                        "{}: 点查必须找回表里那条边：{:?}",
                        file.display(),
                        cascade.edges_by_attribute
                    );
                    assert!(
                        cascade.foreign_referrers.is_empty(),
                        "{}: P0 差表说 POSREF 目标 0 跨库，这里冒出来了：{:?}",
                        file.display(),
                        cascade.foreign_referrers
                    );
                    let expected: BTreeSet<RefnoEnum> =
                        roots_on_owner_chain(&index.owners, &root_keys, referrer)
                            .into_iter()
                            .map(refno_from_e3d)
                            .collect();
                    assert_eq!(
                        cascade.referrer_roots,
                        expected,
                        "{}: 引用者 {referrer} 的根必须一个不多一个不少",
                        file.display()
                    );
                    checked += 1;
                }
            }
            println!("查过 {checked} 个库；{skipped_empty} 个库表在但零边");
            assert!(checked > 0, "语料里至少要有一个库的 POSREF 表非空");
        }

        /// 现有窗口逐步、**全部** `POSREF` 库：开关只能**多算不能少算**，而且今天的自然窗口里
        /// 应当逐字相同。
        ///
        /// 不变量分两层，弱的那层才是真契约：
        /// 1. `regen_off ⊆ regen_on` ——级联只会往 `touched` 里加，永远不会拿走；
        /// 2. 没有引用者根、也没判退化时，两态**逐字相同**（无事可加就不该有任何差别）。
        ///
        /// 真出现「加了根导致两态不同」的窗口就打印出来——那正是语料里第一次出现
        /// 「只改被参照件」形状的信号，不该被一条 `assert_eq!` 掩盖成失败。
        #[test]
        #[ignore = "manual live: needs DESI databases that carry a POSREF table and the E3D template directory"]
        fn live_existing_windows_never_lose_a_root_with_the_cascade_on() {
            let corpus = posref_design_dbs();
            if corpus.is_empty() {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let (mut windows_checked, mut differed) = (0usize, Vec::new());
            for (_, carriers) in &corpus {
                for carrier in carriers {
                    let file = &carrier.file;
                    let mut engine = ReadOnlyEngine::open(file).expect("open the design file");
                    let mut sesnos: Vec<u32> = engine
                        .sessions()
                        .expect("sessions")
                        .iter()
                        .map(|session| session.session_id)
                        .collect();
                    sesnos.sort_unstable();
                    sesnos.dedup();
                    let steps = engine
                        .diff_steps(SessionSelector::Exact(sesnos[0]), SessionSelector::Latest)
                        .expect("diff steps");
                    // 每个库只跑最后三步：这条门要证的是不变量，不是把整条会话链再走一遍。
                    let windows: Vec<(u32, u32)> = steps
                        .iter()
                        .rev()
                        .take(3)
                        .rev()
                        .map(|step| (step.from.sesno, step.to.sesno))
                        .collect();
                    drop(engine);

                    for (base_sesno, target_sesno) in windows {
                        let (base, target) = (
                            open(&schema, file, base_sesno),
                            open(&schema, file, target_sesno),
                        );
                        let base_index =
                            scan_index(file, Some(base_sesno)).expect("scan base index");
                        let roots_s: Vec<PersistedRoot> =
                            enumerate_generation_roots(&base, &base_index.roots, &unit_types)
                                .expect("enumerate base roots")
                                .into_iter()
                                .map(|root| PersistedRoot {
                                    refno: root.root,
                                    noun: root.noun,
                                })
                                .collect();
                        let run = |on: bool| {
                            let _switch = crate::options::DesignReferenceCascadeOverride::set(on);
                            let sources = WindowRootSources::collect(
                                file,
                                base_sesno,
                                target_sesno,
                                &base,
                                &target,
                                &unit_types,
                                &[],
                            )
                            .expect("collect");
                            let impact = sources.impact(&roots_s);
                            let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
                            let regen: BTreeSet<RefnoEnum> =
                                plan.regen.iter().map(|root| root.root).collect();
                            (sources.design_reference, regen)
                        };
                        let ((on, regen_on), (off, regen_off)) = (run(true), run(false));
                        assert!(on.enabled && !off.enabled);
                        let label = format!(
                            "{} {base_sesno}→{target_sesno}",
                            file.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        );
                        assert!(
                            regen_off.is_subset(&regen_on),
                            "{label}: 级联只该往 touched 里加；off\\on = {:?}",
                            regen_off.difference(&regen_on).collect::<Vec<_>>()
                        );
                        if regen_on != regen_off {
                            println!("  ★ {label}: {}", on.summary_line(0));
                            println!(
                                "     regen_on={} regen_off={} 多出 {:?}",
                                regen_on.len(),
                                regen_off.len(),
                                regen_on.difference(&regen_off).collect::<Vec<_>>()
                            );
                            differed.push(label.clone());
                        } else if on.referrer_roots.is_empty() && on.degraded.is_none() {
                            // 无事可加、也没判不了——这一窗本就该逐字相同。
                        }
                        if on.queries > 0 {
                            println!("  {label}: {}", on.summary_line(0));
                        }
                        windows_checked += 1;
                    }
                }
            }
            println!(
                "对拍 {windows_checked} 窗；两态不同的 {} 窗：{differed:?}",
                differed.len()
            );
            assert!(windows_checked > 0);
        }

        /// 真库假定源（GitHub #143，手动；#132 时写成探针）：支撑件（`ANCI` 一族）经 `COMPRE` 指着被支撑的构件，管外径取自
        /// 那只构件的 `LSTU` / `HSTU`（e3d-io 目录求值的 `ATTRIB POD`）。被指的构件换了规格，支撑件得跟着重算。
        ///
        /// 量三件事（只读，数是语料的性质，不下数值断言）：
        /// 1. 哪些设计库带 `COMPRE` 反向表、表里多少条边、多少条的两端不在同一个库；
        /// 2. 被指的构件是哪种形态：自己有 `LSTU`（形态 A）、没有 `LSTU` 但有 `HSTU`（形态 B，BRAN）、两样都没有；
        /// 3. 假定每个被指的构件都换了规格，本模块的设计侧一跳（P4）逐库点查答出什么、做了什么。
        ///
        /// 钉的是 [`DESIGN_CASCADE_TARGET_READS`] 接上之后的形状：以被指构件所在的库为本窗的库，同库支撑件的根一个不多
        /// 一个不少地上卷，别的库里的支撑件点名交给跨库交接、在它自己那个库折得出根；被指的是桥架（两格都没有）不触发；
        /// 同一个会话读两遍（「只挪了位置」）一条都不触发。
        ///
        /// 只读文件：`AIOS_APS_PROJECT_DIR`（缺省本机 APS 样例工程）+ `AIOS_E3D_TEMPLATE_DIR`。
        #[test]
        #[ignore = "manual live probe: walks every DESI database of the APS sample project; needs the E3D template directory"]
        fn live_probe_component_reference_edges_and_what_the_design_cascade_does_with_them() {
            let dir =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            if !dir.is_dir() {
                println!("skip: {} 不在本机", dir.display());
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let sources = sources_in_dir(&dir, &["DESI"]).expect("DESI sources");
            let hash = e3d_attlib::db1_hash("COMPRE");

            // 1. 反向表在**引用者**那个库的文件里，键是被引用的元素。
            let mut edges: Vec<(RefNo, RefNo)> = Vec::new();
            let mut carriers = 0usize;
            for source in &sources {
                let Ok(mut engine) = ReadOnlyEngine::open(&source.file) else {
                    continue;
                };
                if !engine.tables().iter().any(|table| table.name_hash == hash) {
                    continue;
                }
                let entries: Vec<(RefNo, RefNo)> = match engine.reference_table(hash) {
                    Some(mut table) => table
                        .iter()
                        .filter_map(Result::ok)
                        .map(|entry| (entry.target, entry.referrer))
                        .collect(),
                    None => continue,
                };
                carriers += 1;
                let cross = entries
                    .iter()
                    .filter(|(target, referrer)| target.dbno() != referrer.dbno())
                    .count();
                println!(
                    "[#132] dbnum {} 的 COMPRE 表：{} 条边，其中 {cross} 条指到别的库",
                    source.dbnum,
                    entries.len()
                );
                edges.extend(entries);
            }
            let cross_db = edges
                .iter()
                .filter(|(target, referrer)| target.dbno() != referrer.dbno())
                .count();
            println!(
                "[#132] {} 个 DESI 库里 {carriers} 个带 COMPRE 反向表；共 {} 条边，跨库 {cross_db} 条、同库 {} 条",
                sources.len(),
                edges.len(),
                edges.len() - cross_db
            );
            if edges.is_empty() {
                println!("[#132] 语料里没有 COMPRE 边（或没有这张反向表）");
                return;
            }

            // 2. 被指的构件的形态。全部 DESI 库注册进同一份 DbSet（与生产 `build_set` 同形）才读得到跨库的那一端。
            let all = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            for source in &sources {
                let _ = all.add_db(DbFilePin {
                    file: source.file.clone(),
                    template: template_file_for(schema.template_dir(), "DESI")
                        .expect("DESI template"),
                    db_type: Some("DESI".into()),
                    sesno: None,
                });
            }
            let mut shapes: BTreeMap<String, usize> = BTreeMap::new();
            let mut forward_ok = 0usize;
            for (target, referrer) in &edges {
                let referrer_element = all.element(*referrer);
                let referrer_noun = referrer_element
                    .element_type()
                    .unwrap_or_else(|_| "?".into());
                if referrer_element.get_ref("COMPRE").ok().flatten() == Some(*target) {
                    forward_ok += 1;
                }
                let target_element = all.element(*target);
                let target_noun = target_element.element_type().unwrap_or_else(|_| "?".into());
                let points_somewhere = |name: &str| {
                    target_element.has_attribute(name).unwrap_or(false)
                        && target_element
                            .get_ref(name)
                            .ok()
                            .flatten()
                            .is_some_and(|refno| refno.is_valid())
                };
                let form = if points_somewhere("LSTU") {
                    "形态 A（自己有 LSTU）"
                } else if points_somewhere("HSTU") {
                    "形态 B（HSTU）"
                } else {
                    "LSTU / HSTU 都没有"
                };
                *shapes
                    .entry(format!("{referrer_noun}.COMPRE -> {target_noun}：{form}"))
                    .or_default() += 1;
            }
            println!(
                "[#132] 正向核对（引用者的 COMPRE 读出来就是表里那个键）：{forward_ok}/{}",
                edges.len()
            );
            for (shape, count) in &shapes {
                println!("[#132]   {shape} × {count}");
            }

            // 3. 假定被指的构件此前没有 `LSTU` / `HSTU`、现在是文件里的样子（等于每只都换了规格），以它所在的库为本窗
            //    的库：带条件那一类（[`DESIGN_CASCADE_TARGET_READS`]）该把同库的支撑件上卷成根、把别的库里的支撑件点名
            //    交给跨库交接；被指的是桥架（两格都没有）不触发。预算放开——这里量的是答不答得出，不是成本。
            let mut targets: Vec<RefNo> = edges.iter().map(|(target, _)| *target).collect();
            targets.sort();
            targets.dedup();
            let everything = read_design_referrer_edges(&targets, &sources, usize::MAX);
            let seen = everything
                .edges
                .iter()
                .filter(|edge| edge.attribute == "COMPRE")
                .count();
            println!(
                "[#143] {} 个被指的构件：点查 {} 次，查回 COMPRE 引用者边 {seen} 条（表里 {} 条）",
                targets.len(),
                everything.queries,
                edges.len()
            );
            assert_eq!(seen, edges.len(), "点查该找回表里的每一条边");
            assert!(
                DESIGN_CASCADE_TARGET_READS
                    .iter()
                    .any(|(attribute, _)| *attribute == "COMPRE"),
                "COMPRE 不在带条件的名单上：这条测试的结论要重写"
            );

            // 每个涉及到的库（被指构件所在的库、支撑件所在的库）在文件最新会话上的属主索引与根集。
            let unit_types = resolve_delivery_unit_types(&[]);
            struct Lib {
                owners: BTreeMap<(u32, u32), RefNo>,
                roots_t: Vec<GenerationRoot>,
                root_keys: BTreeSet<(u32, u32)>,
            }
            let lib_of = |dbnum: u32| -> Lib {
                let source = sources
                    .iter()
                    .find(|source| source.dbnum == dbnum)
                    .expect("库在语料里");
                let sesno = ReadOnlyEngine::open(&source.file)
                    .expect("open the design file")
                    .session()
                    .sesno;
                let index = scan_index(&source.file, Some(sesno)).expect("scan index");
                let set = open(&schema, &source.file, sesno);
                let roots_t =
                    enumerate_generation_roots(&set, &index.roots, &unit_types).expect("roots");
                let root_keys = roots_t
                    .iter()
                    .map(|root| {
                        let refno = refno_to_e3d(root.root);
                        (refno.word0, refno.word1)
                    })
                    .collect();
                Lib {
                    owners: index
                        .owners
                        .iter()
                        .map(|(key, owner)| (*key, *owner))
                        .collect(),
                    roots_t,
                    root_keys,
                }
            };
            // 「换了规格」的判词：S 端当它两格都没有，T 端读文件。
            let changed = |element: RefNo, reads: &[&str]| {
                let after: Vec<ReferenceRead> = reads
                    .iter()
                    .map(|read| read_reference(&all, element, read))
                    .collect();
                reference_reads_differ(&vec![ReferenceRead::Absent; after.len()], &after)
            };

            let mut by_db: BTreeMap<u32, Vec<RefNo>> = BTreeMap::new();
            for target in &targets {
                by_db.entry(target.dbno()).or_default().push(*target);
            }
            let mut libs: BTreeMap<u32, Lib> = BTreeMap::new();
            let (mut triggered, mut untouched, mut same_db_roots, mut handed_off, mut queued_roots) =
                (0usize, 0usize, 0usize, 0usize, 0usize);
            for (own, own_targets) in &by_db {
                let queried = read_design_referrer_edges(own_targets, &sources, usize::MAX);
                if !libs.contains_key(own) {
                    libs.insert(*own, lib_of(*own));
                }
                let lib = &libs[own];
                let cascade = DesignReferenceCascade::from_edges_with(
                    &queried,
                    *own,
                    |referrer| roots_on_owner_chain(&lib.owners, &lib.root_keys, referrer),
                    |element, reads| changed(element, reads),
                );
                println!("[#143] {}", cascade.summary_line(*own));
                assert!(cascade.degraded.is_none(), "{:?}", cascade.degraded);

                // 期望逐条边正向算：一个不多一个不少。
                let mut expected_roots: BTreeSet<RefnoEnum> = BTreeSet::new();
                let mut expected_handoff: BTreeSet<RefNo> = BTreeSet::new();
                for edge in &queried.edges {
                    let behavioural = match edge.attribute.as_str() {
                        "POSREF" => true,
                        "COMPRE" => changed(edge.target, &["LSTU", "HSTU"]),
                        _ => false,
                    };
                    if !behavioural {
                        continue;
                    }
                    if edge.referrer.dbno() == *own {
                        expected_roots.extend(
                            roots_on_owner_chain(&lib.owners, &lib.root_keys, edge.referrer)
                                .into_iter()
                                .map(refno_from_e3d),
                        );
                    } else if edge.attribute == "COMPRE" {
                        expected_handoff.insert(edge.referrer);
                    }
                }
                assert_eq!(
                    cascade.referrer_roots, expected_roots,
                    "dbnum {own}: 同库支撑件的根"
                );
                assert_eq!(
                    cascade
                        .handoff_referrers
                        .iter()
                        .copied()
                        .collect::<BTreeSet<_>>(),
                    expected_handoff,
                    "dbnum {own}: 交给跨库交接的支撑件"
                );
                triggered += cascade
                    .target_reads_triggered
                    .get("COMPRE")
                    .copied()
                    .unwrap_or(0);
                untouched += cascade
                    .target_reads_unchanged
                    .get("COMPRE")
                    .copied()
                    .unwrap_or(0);
                same_db_roots += cascade.referrer_roots.len();
                handed_off += cascade.handoff_referrers.len();

                // 「只挪了位置」：同一个会话读两遍，那两格相同 → 一条都不触发，一只支撑件都不拉。
                let quiet = DesignReferenceCascade::from_edges_with(
                    &queried,
                    *own,
                    |_| Vec::new(),
                    |element, reads| {
                        let read: Vec<ReferenceRead> = reads
                            .iter()
                            .map(|name| read_reference(&all, element, name))
                            .collect();
                        reference_reads_differ(&read, &read)
                    },
                );
                assert!(
                    quiet.target_reads_triggered.is_empty(),
                    "dbnum {own}: 同一个会话读两遍不该有哪一格不同：{:?}",
                    quiet.target_reads_triggered
                );
                assert!(quiet.handoff_referrers.is_empty());

                // 跨库的那一半：在支撑件所在的库把它们折成根——跨库交接排的就是这一份名单。
                let handed = crate::data_interface::cross_db_handoff::elements_by_dbnum(
                    cascade.handoff_referrer_elements(),
                );
                for (peer, refnos) in handed {
                    if !libs.contains_key(&peer) {
                        libs.insert(peer, lib_of(peer));
                    }
                    let peer_lib = &libs[&peer];
                    let elements: Vec<RefNo> = refnos
                        .iter()
                        .filter_map(|refno| {
                            crate::data_interface::cross_db_handoff::parse_refno(refno)
                        })
                        .collect();
                    assert_eq!(
                        elements.len(),
                        refnos.len(),
                        "交接名单里的 refno 都拼得回来"
                    );
                    let folded = crate::data_interface::cross_db_handoff::fold_elements_to_roots(
                        &peer_lib.owners,
                        &peer_lib.roots_t,
                        &elements,
                    );
                    let mut nouns: BTreeMap<String, usize> = BTreeMap::new();
                    for root in &folded.roots {
                        *nouns.entry(root.noun.clone()).or_default() += 1;
                    }
                    println!(
                        "[#143] dbnum {own} → dbnum {peer}：{} 只支撑件折成 {} 个根 {nouns:?}，{} 只折不出根",
                        elements.len(),
                        folded.roots.len(),
                        folded.unresolved.len()
                    );
                    assert!(
                        !folded.roots.is_empty(),
                        "dbnum {peer}: 交接过去的支撑件至少折得出一个根"
                    );
                    queued_roots += folded.roots.len();
                }
            }
            println!(
                "[#143] 合计：COMPRE 边触发 {triggered} 条、不触发 {untouched} 条（被指的构件两格都没有）；同库上卷 \
                 {same_db_roots} 个根；跨库点名 {handed_off} 只支撑件、折成 {queued_roots} 个根"
            );
            assert_eq!(triggered + untouched, edges.len(), "每条边都有判词");
            assert!(
                triggered > 0,
                "语料里至少有一只支撑件的被支撑构件带 LSTU / HSTU"
            );
        }

        /// 量法（GitHub #143，手动）：接上之后真实会话里多算多少。
        ///
        /// 走遍「被支撑的构件」所在库的整条会话链（`AIOS_143_LAST_STEPS=n` 只走每个库最后 n 步），每一窗跑生产同一段
        /// `WindowRootSources::collect`（设计成员 = 工程目录下全部 DESI 库，别的库按文件最新），数：
        /// - 变更元素里被 `COMPRE` 指着的有多少条边，其中 `LSTU` / `HSTU` 真变了（触发）的、没变（不触发——收窄省下的）的；
        /// - 触发的边给本窗多拉了几个同库的根（`regen` 的差）、点名了几只别的库里的支撑件、它们在那个库折成几个根；
        /// - 反查判不了（超点查预算 / 成员库打不开）的窗有几个。
        ///
        /// 只打印，不下数值断言（数是语料的性质）；钉的只有不变量：这条级联只会往 `regen` 里加。
        #[test]
        #[ignore = "manual live measurement: walks whole session chains of the APS design databases that hold supported components; needs the E3D template directory"]
        fn live_measure_what_the_component_reference_cascade_adds_on_natural_windows() {
            let started = std::time::Instant::now();
            let dir =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            if !dir.is_dir() {
                println!("skip: {} 不在本机", dir.display());
                return;
            }
            let last_steps: usize = std::env::var("AIOS_143_LAST_STEPS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let sources = sources_in_dir(&dir, &["DESI"]).expect("DESI sources");
            let unit_types = resolve_delivery_unit_types(&[]);
            let hash = e3d_attlib::db1_hash("COMPRE");

            // 被支撑的构件在哪些库里（反向表在支撑件那个库的文件里，键是被支撑的构件）。
            let mut target_dbs: BTreeSet<u32> = BTreeSet::new();
            for source in &sources {
                let Ok(mut engine) = ReadOnlyEngine::open(&source.file) else {
                    continue;
                };
                if !engine.tables().iter().any(|table| table.name_hash == hash) {
                    continue;
                }
                if let Some(mut table) = engine.reference_table(hash) {
                    target_dbs.extend(
                        table
                            .iter()
                            .filter_map(Result::ok)
                            .map(|entry| entry.target.dbno()),
                    );
                }
            }
            println!("[#143] 被支撑的构件所在的库：{target_dbs:?}");

            #[derive(Default)]
            struct Tally {
                windows: usize,
                degraded: usize,
                with_edges: usize,
                triggered: usize,
                unchanged: usize,
                added_roots: usize,
                mixed: usize,
                handed: usize,
                queued_roots: usize,
                regen: usize,
            }
            let mut total = Tally::default();
            // 支撑件所在的库：文件最新会话上的属主索引与根集，折根用。
            let mut peer_libs: BTreeMap<u32, (BTreeMap<(u32, u32), RefNo>, Vec<GenerationRoot>)> =
                BTreeMap::new();
            for dbnum in &target_dbs {
                let Some(source) = sources.iter().find(|source| source.dbnum == *dbnum) else {
                    println!("[#143] dbnum {dbnum} 不在工程目录里，跳过");
                    continue;
                };
                let file = &source.file;
                let label = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut engine = ReadOnlyEngine::open(file).expect("open the design file");
                let mut sesnos: Vec<u32> = engine
                    .sessions()
                    .expect("sessions")
                    .iter()
                    .map(|session| session.session_id)
                    .collect();
                sesnos.sort_unstable();
                sesnos.dedup();
                let steps = engine
                    .diff_steps(SessionSelector::Exact(sesnos[0]), SessionSelector::Latest)
                    .expect("diff steps");
                let mut windows: Vec<(u32, u32)> = steps
                    .iter()
                    .map(|step| (step.from.sesno, step.to.sesno))
                    .collect();
                if last_steps > 0 && windows.len() > last_steps {
                    windows = windows.split_off(windows.len() - last_steps);
                }
                drop(engine);

                let mut tally = Tally::default();
                for (base_sesno, target_sesno) in windows {
                    let (base, target) = (
                        open(&schema, file, base_sesno),
                        open(&schema, file, target_sesno),
                    );
                    let base_index = scan_index(file, Some(base_sesno)).expect("scan base index");
                    let roots_s: Vec<PersistedRoot> =
                        enumerate_generation_roots(&base, &base_index.roots, &unit_types)
                            .expect("enumerate base roots")
                            .into_iter()
                            .map(|root| PersistedRoot {
                                refno: root.root,
                                noun: root.noun,
                            })
                            .collect();
                    let mut window = WindowRootSources::collect(
                        file,
                        base_sesno,
                        target_sesno,
                        &base,
                        &target,
                        &unit_types,
                        &sources,
                    )
                    .expect("collect");
                    let regen_of = |window: &WindowRootSources| -> BTreeSet<RefnoEnum> {
                        let impact = window.impact(&roots_s);
                        plan_window_roots(&window.roots_t, &roots_s, &impact)
                            .regen
                            .iter()
                            .map(|root| root.root)
                            .collect()
                    };
                    let cascade = window.design_reference.clone();
                    let regen_on = regen_of(&window);
                    tally.windows += 1;
                    tally.regen += regen_on.len();
                    if cascade.degraded.is_some() {
                        tally.degraded += 1;
                    }
                    let triggered = cascade
                        .target_reads_triggered
                        .get("COMPRE")
                        .copied()
                        .unwrap_or(0);
                    let unchanged = cascade
                        .target_reads_unchanged
                        .get("COMPRE")
                        .copied()
                        .unwrap_or(0);
                    if triggered + unchanged == 0 {
                        continue;
                    }
                    tally.with_edges += 1;
                    tally.triggered += triggered;
                    tally.unchanged += unchanged;

                    // 对照：同一窗、引用者一个都不上卷。本窗没有 `POSREF` 边时它就是「不带 COMPRE」；有的话拆不开，
                    // 记一笔、差值按上界读。
                    let mut added = 0usize;
                    if triggered > 0 && !cascade.referrer_roots.is_empty() {
                        if cascade.edges_by_attribute.contains_key("POSREF") {
                            tally.mixed += 1;
                        }
                        window.design_reference.referrer_roots.clear();
                        let regen_off = regen_of(&window);
                        assert!(
                            regen_off.is_subset(&regen_on),
                            "{label} {base_sesno}→{target_sesno}: 级联只该往 regen 里加"
                        );
                        added = regen_on.difference(&regen_off).count();
                    }
                    tally.added_roots += added;

                    // 跨库的那一半在支撑件所在的库折成根。
                    let handed = crate::data_interface::cross_db_handoff::elements_by_dbnum(
                        cascade.handoff_referrer_elements(),
                    );
                    let mut queued = 0usize;
                    for (peer, refnos) in &handed {
                        if !peer_libs.contains_key(peer) {
                            let Some(peer_source) =
                                sources.iter().find(|source| source.dbnum == *peer)
                            else {
                                continue;
                            };
                            let sesno = ReadOnlyEngine::open(&peer_source.file)
                                .expect("open the peer design file")
                                .session()
                                .sesno;
                            let index =
                                scan_index(&peer_source.file, Some(sesno)).expect("scan index");
                            let set = open(&schema, &peer_source.file, sesno);
                            let roots = enumerate_generation_roots(&set, &index.roots, &unit_types)
                                .expect("roots");
                            peer_libs.insert(
                                *peer,
                                (
                                    index
                                        .owners
                                        .iter()
                                        .map(|(key, owner)| (*key, *owner))
                                        .collect(),
                                    roots,
                                ),
                            );
                        }
                        let (owners, roots) = &peer_libs[peer];
                        let elements: Vec<RefNo> = refnos
                            .iter()
                            .filter_map(|refno| {
                                crate::data_interface::cross_db_handoff::parse_refno(refno)
                            })
                            .collect();
                        queued += crate::data_interface::cross_db_handoff::fold_elements_to_roots(
                            owners, roots, &elements,
                        )
                        .roots
                        .len();
                    }
                    tally.handed += cascade.handoff_referrers.len();
                    tally.queued_roots += queued;
                    println!(
                        "[#143]   {label} {base_sesno}→{target_sesno}: regen={} COMPRE 触发 {triggered} / 不触发 \
                         {unchanged}；同库多拉 {added} 根；跨库点名 {} 只 → {queued} 个根 {:?}{}",
                        regen_on.len(),
                        cascade.handoff_referrers.len(),
                        handed.keys().collect::<Vec<_>>(),
                        if cascade.degraded.is_some() {
                            "；反查判不了"
                        } else {
                            ""
                        }
                    );
                }
                println!(
                    "[#143] {label}: {} 窗（反查判不了 {}），regen 合计 {}；变更元素被 COMPRE 指着的 {} 窗：触发 {} 条 / \
                     不触发 {} 条；同库多拉 {} 根（其中与 POSREF 同窗拆不开的 {} 窗）；跨库点名 {} 只 → {} 个根",
                    tally.windows,
                    tally.degraded,
                    tally.regen,
                    tally.with_edges,
                    tally.triggered,
                    tally.unchanged,
                    tally.added_roots,
                    tally.mixed,
                    tally.handed,
                    tally.queued_roots
                );
                total.windows += tally.windows;
                total.degraded += tally.degraded;
                total.with_edges += tally.with_edges;
                total.triggered += tally.triggered;
                total.unchanged += tally.unchanged;
                total.added_roots += tally.added_roots;
                total.mixed += tally.mixed;
                total.handed += tally.handed;
                total.queued_roots += tally.queued_roots;
                total.regen += tally.regen;
            }
            println!(
                "[#143] 合计 {} 个库 {} 窗（反查判不了 {}），regen 合计 {}；变更元素被 COMPRE 指着的 {} 窗：触发 {} 条 / \
                 不触发 {} 条；同库多拉 {} 根（与 POSREF 同窗拆不开 {} 窗）；跨库点名 {} 只 → {} 个根；用时 {:.0} 秒",
                target_dbs.len(),
                total.windows,
                total.degraded,
                total.regen,
                total.with_edges,
                total.triggered,
                total.unchanged,
                total.added_roots,
                total.mixed,
                total.handed,
                total.queued_roots,
                started.elapsed().as_secs_f64()
            );
        }

        /// 自然窗口（GitHub #143 的三条验收落在同一窗真实会话上，不是假定源）：`aps250132_0001` 36→37 这一窗动了二十几只
        /// 被支撑件指着的管件，其中只有一只真换了规格（`LSTU` / `HSTU` 两端取值不同）。走生产同一段
        /// `WindowRootSources::collect`（设计成员 = 工程目录下全部 DESI 库）：
        /// - 换了规格的那一只：指着它的支撑件被点名——它在别的设计库里，进计划的跨库名单，在那个库折得出根；
        /// - 其余的（挪了位置 / 改了别的属性）：指着它们的支撑件一只都不点名；
        /// - 逐条边拿两端的直读核对，一只不多一只不少。
        ///
        /// 数是语料的性质，只钉形状：两种边都在、名单与逐条核对的结果相同。APS 样例工程不在本机就跳过。
        #[test]
        fn live_aps_a_natural_window_names_only_the_supports_of_the_respecified_component() {
            let dir =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            let file = dir.join("aps250132_0001");
            if !file.is_file() {
                println!("skip: {} 不在本机", file.display());
                return;
            }
            let (base_sesno, target_sesno) = (36u32, 37u32);
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let sources = sources_in_dir(&dir, &["DESI"]).expect("DESI sources");
            let unit_types = resolve_delivery_unit_types(&[]);
            let dbnum = ReadOnlyEngine::open(&file)
                .expect("open the design file")
                .descriptor()
                .db_mark;
            let (base, target) = (
                open(&schema, &file, base_sesno),
                open(&schema, &file, target_sesno),
            );
            let window = WindowRootSources::collect(
                &file,
                base_sesno,
                target_sesno,
                &base,
                &target,
                &unit_types,
                &sources,
            )
            .expect("collect");
            let cascade = &window.design_reference;
            println!("[#143] {}", cascade.summary_line(dbnum));
            assert!(cascade.enabled, "设计引用级联默认开");
            assert!(cascade.degraded.is_none(), "{:?}", cascade.degraded);

            // 逐条边直读两端：被引用件的那两格变没变。
            let changed = changed_elements(&window.plan);
            let dbs = design_dbs_for_window(dbnum, &file, target_sesno, &sources);
            let edges = read_design_referrer_edges(&changed, &dbs, DESIGN_CASCADE_QUERY_BUDGET);
            let read = |set: &Arc<DbSet>, element: RefNo| -> Vec<ReferenceRead> {
                ["LSTU", "HSTU"]
                    .iter()
                    .map(|name| read_reference(set, element, name))
                    .collect()
            };
            let mut respecified: BTreeSet<RefNo> = BTreeSet::new();
            let mut untouched: BTreeSet<RefNo> = BTreeSet::new();
            let mut expected_handoff: BTreeSet<RefNo> = BTreeSet::new();
            let mut left_alone: BTreeSet<RefNo> = BTreeSet::new();
            for edge in edges.edges.iter().filter(|edge| edge.attribute == "COMPRE") {
                if reference_reads_differ(&read(&base, edge.target), &read(&target, edge.target)) {
                    respecified.insert(edge.target);
                    if edge.referrer.dbno() != dbnum {
                        expected_handoff.insert(edge.referrer);
                    }
                } else {
                    untouched.insert(edge.target);
                    left_alone.insert(edge.referrer);
                }
            }
            println!(
                "[#143] aps250132_0001 {base_sesno}→{target_sesno}: 变更元素 {} 个；被支撑件指着的管件里换了规格的 {} 只、\
                 没换的 {} 只；该点名的支撑件 {} 只、不该点名的 {} 只",
                changed.len(),
                respecified.len(),
                untouched.len(),
                expected_handoff.len(),
                left_alone.len()
            );
            assert!(
                !respecified.is_empty() && !untouched.is_empty(),
                "这一窗该两种都有（换了规格的 {} 只、没换的 {} 只）；语料变了就换一窗——量法 \
                 live_measure_what_the_component_reference_cascade_adds_on_natural_windows 会逐窗列出来",
                respecified.len(),
                untouched.len()
            );
            assert!(
                !expected_handoff.is_empty(),
                "换了规格的管件有别的设计库里的支撑件指着"
            );
            let named: BTreeSet<RefNo> = cascade.handoff_referrers.iter().copied().collect();
            assert_eq!(named, expected_handoff, "点名的支撑件一只不多一只不少");
            assert!(
                named.is_disjoint(&left_alone),
                "被支撑的管件没换规格，指着它的支撑件不该被点名"
            );

            // 进计划的那一份名单（规划器原样放进 `cross_db_elements`）。
            let planned = crate::data_interface::cross_db_handoff::elements_by_dbnum(
                window.cross_db_cascade_elements(),
            );
            for support in &expected_handoff {
                assert!(
                    planned
                        .get(&support.dbno())
                        .is_some_and(|list| list.contains(&support.to_string())),
                    "支撑件 {support} 在计划的跨库名单里：{planned:?}"
                );
            }
            for support in &left_alone {
                assert!(
                    !planned
                        .get(&support.dbno())
                        .is_some_and(|list| list.contains(&support.to_string())),
                    "支撑件 {support} 不该在计划的跨库名单里"
                );
            }

            // 跨库交接在支撑件所在的库（文件最新会话）把它们折成根——排工排的就是这一份。
            let handed = crate::data_interface::cross_db_handoff::elements_by_dbnum(
                cascade.handoff_referrer_elements(),
            );
            for (peer, refnos) in &handed {
                assert_ne!(*peer, dbnum, "交接的都在别的设计库里");
                let peer_source = sources
                    .iter()
                    .find(|source| source.dbnum == *peer)
                    .expect("支撑件所在的库在工程目录里");
                let sesno = ReadOnlyEngine::open(&peer_source.file)
                    .expect("open the peer design file")
                    .session()
                    .sesno;
                let index = scan_index(&peer_source.file, Some(sesno)).expect("scan index");
                let set = open(&schema, &peer_source.file, sesno);
                let roots =
                    enumerate_generation_roots(&set, &index.roots, &unit_types).expect("roots");
                let owners: BTreeMap<(u32, u32), RefNo> = index
                    .owners
                    .iter()
                    .map(|(key, owner)| (*key, *owner))
                    .collect();
                let elements: Vec<RefNo> = refnos
                    .iter()
                    .filter_map(|refno| crate::data_interface::cross_db_handoff::parse_refno(refno))
                    .collect();
                assert_eq!(elements.len(), refnos.len(), "名单里的 refno 都拼得回来");
                let folded = crate::data_interface::cross_db_handoff::fold_elements_to_roots(
                    &owners, &roots, &elements,
                );
                println!(
                    "[#143] dbnum {dbnum} → dbnum {peer}：{} 只支撑件折成 {} 个根，{} 只折不出根",
                    elements.len(),
                    folded.roots.len(),
                    folded.unresolved.len()
                );
                assert!(
                    !folded.roots.is_empty(),
                    "dbnum {peer}: 交接过去的支撑件折得出根"
                );
                assert!(
                    folded.unresolved.is_empty(),
                    "dbnum {peer}: {:?}",
                    folded.unresolved
                );
            }
        }
    }
}
