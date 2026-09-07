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
        format!(
            "反查：种子 {} → 目录闭包 {} → 设计引用者 {} → 生成根 {}{}（点查 {} 次{}{}）{degraded}",
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
    /// 反查命中、且 `gen_root` 落款仍在旧目录上、被排进 `RegenRoot` 的根数，按设计库。
    pub enqueued: BTreeMap<u32, usize>,
    /// 退化目录库的依赖根被排进 `RegenRoot` 的数，按设计库（与上一栏不重复计）。
    pub enqueued_degraded: BTreeMap<u32, usize>,
    /// 反查命中、但 `gen_root` 落款已在新目录上（重启前已经重算过）的根数——不重排。
    pub already_current: usize,
    /// 反查命中、却没有 `gen_root` 行的根数：从未生成过，按需懒生成（ADR-058 D6），启动不排。
    pub lazy_roots: usize,
    /// 命中却因监听限定域 / 发布队列阻断而跳过的根数。
    pub skipped_out_of_scope: usize,
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
                "，{} 根落款已在新目录上不重排",
                self.already_current
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

    // ---- 第 2 段：设计侧一跳 + 上卷 ----
    let targets: Vec<RefNo> = closure.into_iter().map(unkey).collect();
    for source in sources.iter().filter(|source| source.is_design()) {
        let mut db = OpenDb::open(source)?;
        if db.tables.is_empty() {
            continue;
        }
        let mut referrers: BTreeSet<RefKey> = BTreeSet::new();
        for target in &targets {
            referrers.extend(
                db.referrers_of(*target, &mut cascade.point_queries)?
                    .into_iter()
                    .map(key),
            );
        }
        cascade.tables_used.insert(
            source.dbnum,
            db.tables.iter().map(|(name, _)| name.clone()).collect(),
        );
        drop(db);
        if referrers.is_empty() {
            continue;
        }
        let mut roots: BTreeMap<String, CascadeRoot> = BTreeMap::new();
        let mut unrooted = Vec::new();
        for referrer in referrers.iter().copied().map(unkey) {
            match resolve_root(source.dbnum, referrer)? {
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
        cascade.design_referrers.insert(
            source.dbnum,
            referrers.iter().map(|k| text(unkey(*k))).collect(),
        );
        if !roots.is_empty() {
            cascade
                .roots
                .insert(source.dbnum, roots.into_values().collect());
        }
        if !unrooted.is_empty() {
            cascade.unrooted.insert(source.dbnum, unrooted);
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
/// 它们那天，把名字加进这里就是全部改动——入口、护栏、账都已经在原位。
pub const DESIGN_CASCADE_ATTRIBUTES: [&str; 1] = ["POSREF"];

/// 一窗最多拿多少个变更元素去点查。
///
/// 超过就判不了（[`DesignReferenceCascade::degraded`]）：导入级窗口逐件点查每个成员库的每张表
/// 不是这条路该付的钱，而计划 §2 原则 3 要求「判不了 → 该库全部根过期」，宁多算不漏算。
pub const DESIGN_CASCADE_SOURCE_CAP: usize = 2_000;

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
/// 一个库打不开 / 一次点查读挂都只记账不中断——中断会把「这一窗没有引用者」与「没查成」
/// 混成同一个结果，而这两件事的处置正相反（见 [`DesignReferenceCascade::degraded`]）。
pub fn read_design_referrer_edges(
    sources: &[RefNo],
    design_dbs: &[SourceDb],
) -> DesignReferrerEdges {
    let mut out = DesignReferrerEdges {
        sources: sources.len(),
        ..DesignReferrerEdges::default()
    };
    if sources.is_empty() {
        return out;
    }
    for source in design_dbs.iter().filter(|source| source.is_design()) {
        let mut db = match OpenDb::open(source) {
            Ok(db) => db,
            Err(error) => {
                out.unreadable_dbs
                    .insert(source.dbnum, format!("{error:#}"));
                continue;
            }
        };
        let tables = db.tables.clone();
        if tables.is_empty() {
            continue;
        }
        out.tables_used.insert(
            source.dbnum,
            tables.iter().map(|(name, _)| name.clone()).collect(),
        );
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
    /// 判不了的原因：变更件数超上限 / 成员库打不开 / 表读挂。
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

    /// 变更件数超 [`DESIGN_CASCADE_SOURCE_CAP`]：一次点查都不发，直接判不了。
    pub fn over_cap(sources: usize) -> Self {
        Self {
            enabled: true,
            sources,
            degraded: Some(format!(
                "变更元素 {sources} 个超过反查上限 {DESIGN_CASCADE_SOURCE_CAP}，没有点查任何反向表"
            )),
            ..Self::default()
        }
    }

    /// 纯函数：把点查出的边折成根集与账。`root_chain(引用者)` 给它 T 端 owner 链上的根
    /// （生产传 [`roots_on_owner_chain`]，测试可以手搓）。
    pub fn from_edges(
        edges: &DesignReferrerEdges,
        dbnum: u32,
        mut root_chain: impl FnMut(RefNo) -> Vec<RefNo>,
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
        for edge in &edges.edges {
            *out.edges_by_attribute
                .entry(edge.attribute.clone())
                .or_default() += 1;
            if !DESIGN_CASCADE_ATTRIBUTES.contains(&edge.attribute.as_str()) {
                continue;
            }
            if edge.referrer.dbno() != dbnum {
                *out.foreign_referrers
                    .entry(edge.referrer.dbno())
                    .or_default() += 1;
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
    pub fn collect(
        plan: &UpdatePlan,
        dbnum: u32,
        file: &Path,
        target_sesno: u32,
        design_dbs: &[SourceDb],
        owners: &BTreeMap<(u32, u32), RefNo>,
        roots_t: &[GenerationRoot],
    ) -> Self {
        let sources = changed_elements(plan);
        if sources.is_empty() {
            return Self {
                enabled: true,
                ..Self::default()
            };
        }
        if sources.len() > DESIGN_CASCADE_SOURCE_CAP {
            return Self::over_cap(sources.len());
        }
        let dbs = design_dbs_for_window(dbnum, file, target_sesno, design_dbs);
        let edges = read_design_referrer_edges(&sources, &dbs);
        let root_keys: BTreeSet<(u32, u32)> = roots_t
            .iter()
            .map(|root| {
                let refno = refno_to_e3d(root.root);
                (refno.word0, refno.word1)
            })
            .collect();
        Self::from_edges(&edges, dbnum, |referrer| {
            roots_on_owner_chain(owners, &root_keys, referrer)
        })
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
        assert!(line.contains("1 根落款已在新目录上不重排"), "{line}");
        assert!(line.contains("1 个引用者上卷不到根"), "{line}");
        assert!(line.contains("1 个目录件记录头读不出"), "{line}");
        assert!(!line.contains("懒生成"), "{line}");
        assert!(!line.contains("退化"), "{line}");

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

    /// 超上限：一次点查都不发（导入级窗口不该逐件问每个库的每张表），而且照样判不了。
    #[test]
    fn too_many_changed_elements_degrade_without_a_single_point_query() {
        let cascade = DesignReferenceCascade::over_cap(DESIGN_CASCADE_SOURCE_CAP + 1);
        assert!(cascade.enabled);
        assert_eq!(cascade.queries, 0);
        assert!(cascade.tables_used.is_empty());
        assert!(
            cascade
                .degraded
                .as_deref()
                .is_some_and(|reason| reason.contains("超过反查上限"))
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

        let capped = DesignReferenceCascade::over_cap(9_999).summary_line(OWN_DB);
        assert!(
            capped.contains("判不了") && capped.contains("放弃凭证前移"),
            "{capped}"
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

        /// 在一个项目目录里找出**第一个带 `POSREF` 表的设计库**。写死路径不行：P0 差表说
        /// 语料 783 库里只有 31 个库有这张表，哪一个有得现问。
        fn posref_design_db() -> Option<(SourceDb, u32)> {
            let dir =
                PathBuf::from(std::env::var("AIOS_POSREF_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000".into()
                }));
            let hash = e3d_attlib::db1_hash("POSREF");
            for source in sources_in_dir(&dir, &["DESI"]).ok()? {
                let Ok(mut engine) = ReadOnlyEngine::open(&source.file) else {
                    continue;
                };
                let listed = engine.tables().iter().any(|table| table.name_hash == hash);
                if listed && engine.reference_table(hash).is_some() {
                    return Some((source, hash));
                }
            }
            println!(
                "skip: no DESI database under {} carries a POSREF table",
                dir.display()
            );
            None
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

        /// 从 `POSREF` 表里拿一条现成的边当假定的变更源——语料里没有「只改被参照件」的自然
        /// 窗口，所以验收只能这么搭（与 C8 的 `live_zdj7200_…` 同一手法）。
        #[test]
        #[ignore = "manual live: needs a DESI database that carries a POSREF table and the E3D template directory"]
        fn live_a_posref_target_pulls_its_referrer_root_in() {
            let Some((source, posref)) = posref_design_db() else {
                return;
            };
            let file = source.file.clone();
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let mut engine = ReadOnlyEngine::open(&file).expect("open the design file");
            let sesno = engine.session().sesno;
            let dbnum = engine.descriptor().db_mark;
            let tables = reference_tables_of(&mut engine);
            println!(
                "{} @{sesno} dbnum={dbnum} tables={tables:?}",
                file.display()
            );
            let (target, referrer) = engine
                .reference_table(posref)
                .expect("POSREF 表打得开")
                .iter()
                .filter_map(Result::ok)
                .map(|entry| (entry.target, entry.referrer))
                .next()
                .expect("POSREF 表里要有边");
            drop(engine);

            let index = scan_index(&file, Some(sesno)).expect("scan index");
            let set = open(&schema, &file, sesno);
            let roots_t =
                enumerate_generation_roots(&set, &index.roots, &unit_types).expect("roots");
            let root_keys: BTreeSet<(u32, u32)> = roots_t
                .iter()
                .map(|root| {
                    let refno = refno_to_e3d(root.root);
                    (refno.word0, refno.word1)
                })
                .collect();
            let edges = read_design_referrer_edges(
                &[target],
                &[SourceDb {
                    dbnum,
                    db_type: "DESI".into(),
                    file: file.clone(),
                    sesno: Some(sesno),
                }],
            );
            let cascade = DesignReferenceCascade::from_edges(&edges, dbnum, |element| {
                roots_on_owner_chain(&index.owners, &root_keys, element)
            });
            println!("{}", cascade.summary_line(dbnum));
            assert!(cascade.degraded.is_none(), "{:?}", cascade.degraded);
            assert!(
                cascade
                    .edges_by_attribute
                    .get("POSREF")
                    .is_some_and(|count| *count >= 1),
                "点查必须找回表里那条边：{:?}",
                cascade.edges_by_attribute
            );
            assert!(
                cascade.foreign_referrers.is_empty(),
                "P0 差表说 POSREF 目标 0 跨库：{:?}",
                cascade.foreign_referrers
            );
            let expected: BTreeSet<RefnoEnum> =
                roots_on_owner_chain(&index.owners, &root_keys, referrer)
                    .into_iter()
                    .map(refno_from_e3d)
                    .collect();
            assert_eq!(
                cascade.referrer_roots, expected,
                "引用者 {referrer} 的根必须一个不多一个不少"
            );
        }

        /// 现有窗口逐步：开关两态的 `regen` 必须逐字相同。
        #[test]
        #[ignore = "manual live: needs a DESI database that carries a POSREF table and the E3D template directory"]
        fn live_existing_windows_are_identical_with_the_cascade_on_and_off() {
            let Some((source, _)) = posref_design_db() else {
                return;
            };
            let file = source.file.clone();
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let mut engine = ReadOnlyEngine::open(&file).expect("open the design file");
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
            // 只跑最后几步：这条门要证的是「开关不改结果」，不是把整条会话链再走一遍。
            let windows: Vec<(u32, u32)> = steps
                .iter()
                .rev()
                .take(6)
                .rev()
                .map(|step| (step.from.sesno, step.to.sesno))
                .collect();
            drop(engine);
            assert!(!windows.is_empty(), "这个库要有不止一个会话");

            for (base_sesno, target_sesno) in windows {
                let (base, target) = (
                    open(&schema, &file, base_sesno),
                    open(&schema, &file, target_sesno),
                );
                let base_index = scan_index(&file, Some(base_sesno)).expect("scan base index");
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
                        &file,
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
                    let regen: Vec<RefnoEnum> = plan.regen.iter().map(|root| root.root).collect();
                    (sources.design_reference, regen)
                };
                let ((on, regen_on), (off, regen_off)) = (run(true), run(false));
                println!(
                    "{base_sesno}→{target_sesno}: {} | regen_on={} regen_off={}",
                    on.summary_line(0),
                    regen_on.len(),
                    regen_off.len()
                );
                assert!(on.enabled && !off.enabled);
                assert_eq!(
                    regen_on, regen_off,
                    "{base_sesno}→{target_sesno}: 两态 regen 必须逐字相同"
                );
            }
        }
    }
}
