//! 本 MDB 的元素骨架，进程级常驻，启动时从 e3d-io 预热。
//!
//! 装三样，**不装属性**：`ref0 → dbnum` 归属、`refno → 记录头自报的属主`、
//! `refno → noun`。属性照旧按需走 [`super::direct_store::DirectStore`]（ADR-053），
//! 池级缓存归它管（ADR-059）。
//!
//! # 为什么是这三样，为什么到此为止
//!
//! 2026-09-05 用 `bin/mdb_memory_probe` 对 `AvevaMarineSample /ALL`（28 个 DESI、
//! 611,345 个元素、文件合计 301.4 MiB）量过三档粒度，release、串行：
//!
//! | 档 | 装什么 | 墙钟 | 常驻 | 每元素 |
//! |---|---|---|---|---|
//! | index | refno 清单 + ref0 归属 | 0.46s | 5.3 MiB | 9 B |
//! | **header（本模块）** | **加上 noun + owner** | **0.72s** | **19.8 MiB** | **34 B** |
//! | extract | 每元素整张属性表 | 8.02s | **1291 MiB** | 2215 B |
//!
//! 全属性常驻是 1.29 GiB——而那 2215 B/元素里大半是 `HashMap<String, NamedAttrValue>`
//! 的表示开销（同一个 noun 的几十个属性名在库里重复了十万遍），不是数据本身。
//! 换紧凑布局之前不谈全属性常驻，所以这里停在 header 档。
//!
//! # 属主取自记录头，不是 `OWNER` 属性
//!
//! 头扫描（`ScanTier::Header`）一次页读同时拿到 noun 与 owner，不解析整条记录。
//! 它与属性表里那个 `OWNER` 必须是同一个值——`bin/mdb_memory_probe --tier owner-parity`
//! 就是逐元素比这两者的，全库零分歧才算数。[`super::direct_tree::DirectTreeService`]
//! 里那条「成员表 OWNER 与元素记录 OWNER 必须相等」的断言在运行期继续守着同一件事。
//!
//! # 时点
//!
//! 每个库按**装载那一刻文件自报的最新会话**钉死，与 `DirectStore` 对同一批库的
//! `DbPin { sesno: None }` 同一口径。库在跑的时候被写了，骨架不会自己跟着变——
//! 它跟 `DirectStore` 冻住的那个会话是同一个，两边一起旧，不会一半新一半旧。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use aios_core::RefU64;
use anyhow::Context;
use e3d_io::refno::RefNo;
use e3d_io::uda_catalog::UdaCatalog;
use e3d_io::{ReadOnlyEngine, ScanTier};

use super::mdb_membership::MdbMembership;

const MAX_OWNER_DEPTH: usize = 256;

/// 一个元素在骨架里的全部内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementRow {
    pub noun_hash: u32,
    /// 记录头自报的属主。无效（全零）与自指在装载时就归一成 `None`——
    /// 「没有属主」是链的终点，让它以两种形状存在等于让每个读者各判一次。
    pub owner: Option<RefNo>,
}

/// 骨架里的一个库，以及它被钉在哪个会话上。
#[derive(Debug, Clone)]
pub struct LoadedDb {
    pub dbnum: u32,
    pub db_type: String,
    pub file: PathBuf,
    pub sesno: u32,
    pub elements: usize,
}

#[derive(Debug, Clone, Default)]
pub struct LoadStats {
    pub databases: usize,
    pub elements: usize,
    /// 索引指到、但记录读不出来的那些。不是 0 就该有人看一眼。
    pub unreadable_records: usize,
    pub load_ms: u128,
}

/// 一个 MDB 的元素骨架。
pub struct MdbMemoryStore {
    epoch: u64,
    project: String,
    mdb: String,
    membership: Arc<MdbMembership>,
    uda_catalog: Arc<UdaCatalog>,
    databases: Vec<LoadedDb>,
    /// `ref0 → dbnum`。**不是** `RefU64::get_0()`——那只是 ref0，不是库号。
    affiliations: HashMap<u32, u32>,
    elements: HashMap<RefNo, ElementRow>,
    /// `(dbnum, noun_hash) → refnos`，refno 升序。
    by_noun: HashMap<(u32, u32), Vec<RefNo>>,
    stats: LoadStats,
    /// 装载时发现、但不足以拒绝整个骨架的事。空着是常态，不空要有人看。
    problems: Vec<String>,
}

impl MdbMemoryStore {
    /// 扫这个 MDB 声明的每一个盘上有文件的 DESI，建骨架。
    ///
    /// 声明了却不在盘上的库**不静默跳过**：它进 `problems`，因为它对下游的表现是
    /// 「这个 refno 不存在」，跟「这个元素真的被删了」长得一模一样。
    pub fn load(project: &str, mdb: &str, membership: &MdbMembership) -> anyhow::Result<Self> {
        let started = Instant::now();
        let schema = super::direct_store::DirectSchema::open_from_env()?;
        let uda_catalog = super::element_attributes::load_uda_catalog(membership, &schema)?;
        let mut store = Self {
            epoch: 0,
            project: project.to_string(),
            mdb: format!("/{}", mdb.trim_start_matches('/')),
            membership: Arc::new(membership.clone()),
            uda_catalog: Arc::new(uda_catalog),
            databases: Vec::new(),
            affiliations: HashMap::new(),
            elements: HashMap::new(),
            by_noun: HashMap::new(),
            stats: LoadStats::default(),
            problems: Vec::new(),
        };

        for db in membership.element_databases() {
            let Some(path) = db.path.clone() else {
                store.problems.push(format!(
                    "DESI {} ({}) 被 MDB 声明、配置的项目目录里没有文件：它的元素在骨架里\
                     一个都没有，下游读起来跟「元素不存在」没有区别",
                    db.dbnum, db.name
                ));
                continue;
            };
            let db_type = if db.styp == super::mdb_membership::ISOD_STYP {
                "ISOD"
            } else {
                "DESI"
            };
            store.load_one(db.dbnum, db_type, &path)?;
        }

        anyhow::ensure!(
            !store.elements.is_empty(),
            "MDB {} 的 DESI 声明里一个元素都没扫出来（声明 {} 个库）",
            store.mdb,
            membership.element_databases().count()
        );
        // refno 升序：`RefU64` 是 `(word0 << 32) + word1`，按这两个字排就是按它排，
        // 与 `DbIndexes::refnos_of_noun` 交出来的键序同一个顺序。
        for refnos in store.by_noun.values_mut() {
            refnos.sort_unstable_by_key(|refno| (refno.word0, refno.word1));
        }
        store.stats.databases = store.databases.len();
        store.stats.elements = store.elements.len();
        store.stats.load_ms = started.elapsed().as_millis();
        Ok(store)
    }

    fn load_one(
        &mut self,
        declared: u32,
        db_type: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let header = super::db_file_header::DbFileHeader::read(path)?;
        // 名字挑出来的文件与它自报的库号不符，就没有哪一个值可以拿来建归属：
        // 按声明记，跨库 owner 上溯会去错库；按自报记，MDB 的成员名单就对不上了。
        if header.dbnum != declared as i32 {
            self.problems.push(format!(
                "DESI {declared} 选中的文件 {} 自报库号 {}，两者不符，本库不进骨架",
                path.display(),
                header.dbnum
            ));
            return Ok(());
        }
        let sesno = header.latest_sesno();
        let mut engine = ReadOnlyEngine::open(path)
            .with_context(|| format!("e3d-io 打开 DESI {declared} ({})", path.display()))?;

        let mut count = 0usize;
        for scanned in engine
            .scan_elements(ScanTier::Header)
            .with_context(|| format!("头扫描 DESI {declared} ({})", path.display()))?
        {
            let Ok(scanned) = scanned else {
                self.stats.unreadable_records += 1;
                continue;
            };
            let ref0 = scanned.refno.word0;
            if let Some(previous) = self.affiliations.insert(ref0, declared)
                && previous != declared
            {
                // 一个 ref0 落在两个库里，之后每一次跨库 owner 上溯与属性直读都得
                // 猜库。猜出来的那一份读得出来、看着正常，只是不是你要的元素。
                anyhow::bail!(
                    "MDB {} 的 ref0 {ref0} 同时属于 DESI {previous} 和 {declared}",
                    self.mdb
                );
            }
            let owner = (scanned.owner.is_valid() && scanned.owner != scanned.refno)
                .then_some(scanned.owner);
            self.elements.insert(
                scanned.refno,
                ElementRow {
                    noun_hash: scanned.noun_hash,
                    owner,
                },
            );
            self.by_noun
                .entry((declared, scanned.noun_hash))
                .or_default()
                .push(scanned.refno);
            count += 1;
        }

        self.databases.push(LoadedDb {
            dbnum: declared,
            db_type: db_type.to_string(),
            file: path.to_path_buf(),
            sesno,
            elements: count,
        });
        Ok(())
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn membership(&self) -> Arc<MdbMembership> {
        self.membership.clone()
    }

    pub fn uda_catalog(&self) -> Arc<UdaCatalog> {
        self.uda_catalog.clone()
    }

    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    pub fn databases(&self) -> &[LoadedDb] {
        &self.databases
    }

    pub fn session_fingerprint(&self) -> Vec<(u32, PathBuf, u32)> {
        self.databases
            .iter()
            .map(|db| (db.dbnum, db.file.clone(), db.sesno))
            .collect()
    }

    pub fn is_current(&self) -> anyhow::Result<bool> {
        for db in &self.databases {
            let header = super::db_file_header::DbFileHeader::read(&db.file)?;
            if header.latest_sesno() != db.sesno {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn stats(&self) -> &LoadStats {
        &self.stats
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// `ref0 → dbnum`。跨库 owner 上溯与属性直读都从这里定库。
    pub fn dbnum_of_ref0(&self, ref0: u32) -> Option<u32> {
        self.affiliations.get(&ref0).copied()
    }

    pub fn dbnum_of(&self, refno: RefU64) -> Option<u32> {
        self.dbnum_of_ref0(refno.get_0())
    }

    /// 某个库拥有的全部 Ref0，升序。`dbnum → Ref0` 是一对多（`CONTEXT.md`「Ref0 库归属」），
    /// 这是那张归属表的反向视图，给 `/dbnums` 每行的 `ref0s` 用；不在骨架里的库回空。
    pub fn ref0s_of(&self, dbnum: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .affiliations
            .iter()
            .filter(|(_, owner)| **owner == dbnum)
            .map(|(ref0, _)| *ref0)
            .collect();
        out.sort_unstable();
        out
    }

    pub fn row(&self, refno: RefU64) -> Option<ElementRow> {
        self.elements.get(&to_e3d(refno)).copied()
    }

    pub fn contains(&self, refno: RefU64) -> bool {
        self.elements.contains_key(&to_e3d(refno))
    }

    /// 属主。`None` 既可能是链的终点，也可能是这个元素不在骨架里——两者用
    /// [`Self::contains`] 分开，别拿 `None` 当「到顶了」。
    pub fn owner_of(&self, refno: RefU64) -> Option<RefU64> {
        self.row(refno)?.owner.map(from_e3d)
    }

    pub fn noun_hash_of(&self, refno: RefU64) -> Option<u32> {
        self.row(refno).map(|row| row.noun_hash)
    }

    /// 某个库里这个 noun 的全部 refno，refno 升序。
    ///
    /// 与 [`super::direct_index::DbIndexes::refnos_of_noun`] 的差别只有一处：那边可以
    /// 按「成员表非空」过滤，头扫描看不到成员表，所以这里过滤不了。**调用方接着就去
    /// 读成员表的话，两者结果相同**——成员表空的那个贡献零个成员。
    pub fn refnos_of_noun(&self, dbnum: u32, noun: &str) -> Vec<RefU64> {
        self.refnos_of_noun_hash(dbnum, e3d_attlib::db1_hash(noun))
    }

    pub fn refnos_of_noun_hash(&self, dbnum: u32, noun_hash: u32) -> Vec<RefU64> {
        self.by_noun
            .get(&(dbnum, noun_hash))
            .map(|refnos| refnos.iter().copied().map(from_e3d).collect())
            .unwrap_or_default()
    }

    /// 自身 → OWNER → …… 的整条链，与 Plant UI 既有契约同序。
    ///
    /// 全程零解析：此前这条链每上一级要抽一次整条记录再转一张属性表，只为读一个
    /// `OWNER`。链外的属主（指到本 MDB 声明之外的库）照旧响亮失败，不悄悄截断——
    /// 截断出来的祖先链会让选根落在一个更细的根上，而那不报错。
    pub fn ancestors(&self, start: RefU64) -> anyhow::Result<Vec<RefU64>> {
        anyhow::ensure!(
            self.contains(start),
            "{start} 不在 MDB {} 的 DESI 骨架里（库没被声明，或元素在钉住的会话上不存在）",
            self.mdb
        );
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut current = start;
        for _ in 0..MAX_OWNER_DEPTH {
            anyhow::ensure!(seen.insert(current), "OWNER 链成环于 {current}");
            out.push(current);
            let Some(owner) = self.owner_of(current) else {
                return Ok(out);
            };
            anyhow::ensure!(
                self.contains(owner),
                "{current} 的属主 {owner} 不在 MDB {} 的 DESI 骨架里",
                self.mdb
            );
            current = owner;
        }
        anyhow::bail!("OWNER 链超过 {MAX_OWNER_DEPTH} 层，起点 {start}")
    }

    /// `InMemoryCataLocator::from_parts` 的两个入参。
    ///
    /// 骨架已经把每个 ref0 扫过一遍了，定位器不必再扫第二遍——那正是这次预热
    /// 想省掉的东西（此前是第一次树请求时逐库 `indexed_refnos()`）。
    pub fn locator_parts(&self) -> (HashMap<u32, u32>, HashMap<u32, (String, String, PathBuf)>) {
        let files = self
            .databases
            .iter()
            .map(|db| {
                (
                    db.dbnum,
                    ("DESI".to_string(), self.project.clone(), db.file.clone()),
                )
            })
            .collect();
        (self.affiliations.clone(), files)
    }

    /// 一行装载自述，给启动横幅用。
    pub fn describe(&self) -> String {
        format!(
            "MDB {} / {}：DESI 骨架 {} 个库 / {} 个元素，装载 {} ms（noun + owner + ref0 归属；\
             属性不在骨架里，按需走 direct_store）",
            self.mdb, self.project, self.stats.databases, self.stats.elements, self.stats.load_ms
        )
    }
}

fn to_e3d(refno: RefU64) -> RefNo {
    RefNo::new(refno.get_0(), refno.get_1())
}

fn from_e3d(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}

type Registry = Vec<(String, String, Arc<MdbMemoryStore>)>;

fn registry() -> &'static RwLock<Registry> {
    static REGISTRY: std::sync::OnceLock<RwLock<Registry>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

fn stale_registry() -> &'static RwLock<HashMap<(String, String), String>> {
    static STALE: std::sync::OnceLock<RwLock<HashMap<(String, String), String>>> =
        std::sync::OnceLock::new();
    STALE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn stale_reason(project: &str, mdb: &str) -> Option<String> {
    let key = (
        project.to_owned(),
        format!("/{}", mdb.trim_start_matches('/')),
    );
    stale_registry().read().ok()?.get(&key).cloned()
}

/// 登记一份骨架，替换同一 `(project, mdb)` 的旧的。
pub fn install(mut store: MdbMemoryStore) -> Arc<MdbMemoryStore> {
    if let Ok(mut cache) = registry().write() {
        let key = (store.project.clone(), store.mdb.clone());
        store.epoch = cache
            .iter()
            .find(|(project, mdb, _)| *project == key.0 && *mdb == key.1)
            .map(|(_, _, current)| current.epoch.saturating_add(1))
            .unwrap_or(1);
        let shared = Arc::new(store);
        cache.retain(|(project, mdb, _)| (project.clone(), mdb.clone()) != key);
        cache.push((key.0, key.1, shared.clone()));
        return shared;
    }
    Arc::new(store)
}

/// 已预热的骨架，没预热过就是 `None`。
///
/// 和 [`super::mdb_membership::get`] 一样**故意不惰性重建**：扫一遍全部 DESI 不是
/// 一个看着人畜无害的 getter 该干的事。要它自己建的调用方走 [`ensure`]。
pub fn get(project: &str, mdb: &str) -> Option<Arc<MdbMemoryStore>> {
    let wanted = format!("/{}", mdb.trim_start_matches('/'));
    let cache = registry().read().ok()?;
    cache
        .iter()
        .find(|(known_project, known_mdb, _)| known_project == project && *known_mdb == wanted)
        .map(|(_, _, store)| store.clone())
}

/// 预热过就拿现成的，没预热过就现建一份并登记。
///
/// 启动预热失败时这条兜住：树接口照旧能用，只是把那 0.7 秒挪回第一次请求上。
pub fn ensure(project: &str, mdb: &str) -> anyhow::Result<Arc<MdbMemoryStore>> {
    if let Some(hit) = get(project, mdb) {
        return Ok(hit);
    }
    let option = aios_core::get_db_option();
    let membership = super::mdb_membership::get(project, mdb)
        .map(Ok)
        .unwrap_or_else(|| super::mdb_membership::resolve(option, project, mdb).map(Arc::new))?;
    Ok(install(MdbMemoryStore::load(project, mdb, &membership)?))
}

/// Rebuild outside the registry lock and replace the complete MDB snapshot in
/// one write.  Readers therefore see either the old session set or the new
/// one, never a partly rebuilt set.
pub fn refresh_if_stale(project: &str, mdb: &str) -> anyhow::Result<bool> {
    let key = (
        project.to_owned(),
        format!("/{}", mdb.trim_start_matches('/')),
    );
    if get(project, mdb).is_some_and(|store| store.is_current().unwrap_or(false)) {
        if let Ok(mut stale) = stale_registry().write() {
            stale.remove(&key);
        }
        return Ok(false);
    }
    let option = aios_core::get_db_option();
    let rebuilt = super::mdb_membership::resolve(option, project, mdb)
        .and_then(|membership| MdbMemoryStore::load(project, mdb, &membership));
    match rebuilt {
        Ok(store) => {
            install(store);
            if let Ok(mut stale) = stale_registry().write() {
                stale.remove(&key);
            }
            Ok(true)
        }
        Err(error) => {
            if let Ok(mut stale) = stale_registry().write() {
                stale.insert(key, format!("{error:#}"));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refno(word0: u32, word1: u32) -> RefNo {
        RefNo::new(word0, word1)
    }

    /// `/ALL` 的 SITE 在设计库、WORL 在另一个库，跨库上溯是常态而不是边角，
    /// 所以骨架必须是 MDB 级的，不能一库一份。
    fn store() -> MdbMemoryStore {
        let mut store = MdbMemoryStore {
            epoch: 1,
            project: "P".into(),
            mdb: "/ALL".into(),
            membership: Arc::new(MdbMembership::default()),
            uda_catalog: Arc::new(UdaCatalog::default()),
            databases: Vec::new(),
            affiliations: HashMap::from([(1, 7000), (2, 8000)]),
            elements: HashMap::from([
                // 7000 里的 WORL，没有属主。
                (
                    refno(1, 1),
                    ElementRow {
                        noun_hash: e3d_attlib::db1_hash("WORL"),
                        owner: None,
                    },
                ),
                // 8000 里的 SITE，属主在 7000。
                (
                    refno(2, 1),
                    ElementRow {
                        noun_hash: e3d_attlib::db1_hash("SITE"),
                        owner: Some(refno(1, 1)),
                    },
                ),
                (
                    refno(2, 2),
                    ElementRow {
                        noun_hash: e3d_attlib::db1_hash("ZONE"),
                        owner: Some(refno(2, 1)),
                    },
                ),
            ]),
            by_noun: HashMap::from([
                ((7000, e3d_attlib::db1_hash("WORL")), vec![refno(1, 1)]),
                ((8000, e3d_attlib::db1_hash("SITE")), vec![refno(2, 1)]),
                ((8000, e3d_attlib::db1_hash("ZONE")), vec![refno(2, 2)]),
            ]),
            stats: LoadStats::default(),
            problems: Vec::new(),
        };
        store.stats.elements = store.elements.len();
        store
    }

    #[test]
    fn the_owner_chain_crosses_databases() {
        let chain = store()
            .ancestors(from_e3d(refno(2, 2)))
            .expect("跨库上溯要走得通");
        assert_eq!(
            chain,
            vec![
                from_e3d(refno(2, 2)),
                from_e3d(refno(2, 1)),
                from_e3d(refno(1, 1))
            ],
            "自身在前，属主在后，跨库那一跳不许断"
        );
    }

    /// 属主指到骨架外面时必须响亮失败。悄悄截断的祖先链会让选根落在一个更细的
    /// 根上——模型照样出得来，只是范围小了一圈，而那不报错。
    #[test]
    fn an_owner_outside_the_skeleton_is_loud_not_truncated() {
        let mut store = store();
        store.elements.insert(
            refno(2, 3),
            ElementRow {
                noun_hash: e3d_attlib::db1_hash("EQUI"),
                owner: Some(refno(9, 9)),
            },
        );
        let error = store
            .ancestors(from_e3d(refno(2, 3)))
            .expect_err("属主不在骨架里要报错");
        assert!(
            format!("{error:#}").contains("不在 MDB"),
            "错误里要点名是谁的属主够不着：{error:#}"
        );
    }

    #[test]
    fn a_cycle_in_the_owner_chain_is_refused() {
        let mut store = store();
        store.elements.get_mut(&refno(1, 1)).expect("WORL 在").owner = Some(refno(2, 2));
        let error = store
            .ancestors(from_e3d(refno(2, 2)))
            .expect_err("成环要报错");
        assert!(format!("{error:#}").contains("成环"), "{error:#}");
    }

    #[test]
    fn nouns_are_looked_up_per_database() {
        let store = store();
        assert_eq!(
            store.refnos_of_noun(7000, "WORL"),
            vec![from_e3d(refno(1, 1))]
        );
        assert!(
            store.refnos_of_noun(8000, "WORL").is_empty(),
            "8000 里没有 WORL，别把 7000 的那个交出去"
        );
        assert_eq!(
            store.refnos_of_noun(8000, "SITE"),
            vec![from_e3d(refno(2, 1))]
        );
    }

    /// ref0 不是 dbnum。`RefU64::get_0()` 顶不了归属反查——这条在 `direct_store`
    /// 的模块头上已经写过一次，骨架是同一个坑的第二个入口。
    #[test]
    fn affiliation_is_a_lookup_not_the_ref0_itself() {
        let store = store();
        assert_eq!(store.dbnum_of(from_e3d(refno(2, 1))), Some(8000));
        assert_eq!(store.dbnum_of_ref0(1), Some(7000));
        assert_eq!(store.dbnum_of_ref0(3), None);
    }

    /// 骨架只装 noun + owner + 归属。属性一旦悄悄进来，19.8 MiB 会变成 1.29 GiB，
    /// 而涨的那一天没有任何东西会说话（实测见模块头那张表）。
    #[test]
    fn the_skeleton_holds_no_attributes() {
        let row = std::mem::size_of::<ElementRow>();
        assert!(
            row <= 16,
            "ElementRow 涨到 {row} 字节了：骨架是 noun + owner，属性走 direct_store"
        );
    }
}
