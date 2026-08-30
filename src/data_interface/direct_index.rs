//! ADR-053 D3 的三个派生索引：type / name / backref，从一次全树扫描预建。
//!
//! dabacon 文件里只有 refno 一把主键（索引篇计划
//! `docs/plans/2026-08-30-e3d-io-index-capability-gaps.md` G3）。direct 模式要的
//! 三类查找，文件侧没有现成结构，全部由 `e3d-io::scan_elements(Full)` 的同一次
//! 遍历产出——429 库语料上这次遍历本身 767 ms（P2 门数字），构建成本的大头在
//! 每元素的描述符抽取。
//!
//! 三个索引的口径，各自对齐它要替掉的 DB 查询：
//!
//! * **type**：`query_type_refnos_by_dbnum(&["SITE"], dbnum, has_children, _)` 的
//!   Surreal 语义是「noun 表里 `REFNO.dbnum={dbnum}` 的行」，`has_children` 过滤
//!   `pe_owner` 入边非空。文件侧等价：记录头词 3 的 noun_hash 分组；`has_children`
//!   = 记录自带的成员表非空。
//! * **name**：`NAME` 显式属性原文（含前导 `/`）→ refno。重名不折叠——Surreal 按
//!   name 查回多行就是多行，索引也保留多值，唯一性是查询方的裁决。
//! * **backref**：出边反转。出边四路：记录头的 owner、成员表、隐式区 ref 型属性
//!   （SPRE/CATR/LSTU/PSPE…，按描述符抽取）、显式流 ref 型属性。文件里没有反向边，
//!   这是 direct 唯一必须「预建」的东西（D3 原文）。
//!
//! **失效**：索引钉在 `(dbnum, pinned_sesno, 文件身份)` 上。文件被换、时点被换，
//! 指纹不中就重建——宁可重扫一遍，不读一个「看着像」的旧索引。
//! 磁盘缓存默认在系统临时目录下，[`INDEX_DIR_ENV`] 可改；读不出、版本不合、指纹
//! 不中都静默走重建，坏缓存文件不该挡住生成。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use aios_core::RefU64;
use e3d_io::record::template::TemplateProvider;
use e3d_io::refno::RefNo;
use e3d_io::{ReadOnlyEngine, ScanTier};
use serde::{Deserialize, Serialize};

/// 磁盘缓存目录的环境变量；未设时在 `%TEMP%/aios-direct-index` 下。
pub const INDEX_DIR_ENV: &str = "AIOS_DIRECT_INDEX_DIR";

/// 序列化格式版本。索引结构一变就递增，旧缓存整个作废，没有迁移。
const FORMAT_VERSION: u32 = 1;

/// 一条入边是从哪种出边反转来的。
///
/// 对拍与消费方常常只要其中一类（结构边 vs 属性边），所以分类保留在索引里，
/// 过滤是查询期的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackRefVia {
    /// 记录头词 4/5：source 的 owner 是 target。
    Owner,
    /// source 的成员表里有 target。
    Member,
    /// source 的某个 ref 型属性指向 target；值是属性 hash（db1 哈希）。
    Attr(u32),
}

/// 一条入边：`source` 经 `via` 指向被查的元素。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackRef {
    pub source: u64,
    pub via: BackRefVia,
}

/// 指纹：这份索引是对哪个文件、哪个时点建的。
///
/// 与 `DirectStore` 的文件身份口径一致（长度 + 修改时间），再加会话号与格式
/// 版本。四项有一项不合，缓存就是别人的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFingerprint {
    pub format_version: u32,
    pub dbnum: i32,
    pub pinned_sesno: u32,
    pub file_len: u64,
    /// 修改时间距 UNIX_EPOCH 的纳秒；拿不到修改时间就是 `None`，此时磁盘缓存
    /// 永不命中（宁可重建）。
    pub file_mtime_ns: Option<u128>,
}

impl IndexFingerprint {
    pub fn of(
        file: &Path,
        dbnum: i32,
        pinned_sesno: u32,
    ) -> std::io::Result<Self> {
        let meta = std::fs::metadata(file)?;
        Ok(Self {
            format_version: FORMAT_VERSION,
            dbnum,
            pinned_sesno,
            file_len: meta.len(),
            file_mtime_ns: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()),
        })
    }

    fn cacheable(&self) -> bool {
        self.file_mtime_ns.is_some()
    }
}

/// 构建过程的自述，随索引一起存，供「构建耗时与体积入档」（D3 验收）直接取数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildStats {
    pub elements: u64,
    pub named: u64,
    pub inbound_edges: u64,
    /// 模板缺失/不权威、退成「只收显式 ref」的元素数。这不是错误：DICT 一类库
    /// 的部分 noun 没有模板，它们的隐式区 ref 收不到，显式与结构边照收。
    pub template_fallbacks: u64,
    pub build_ms: u64,
}

/// 一个库在一个时点上的三个派生索引。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbIndexes {
    pub fingerprint: IndexFingerprint,
    pub stats: BuildStats,
    /// noun_hash → `(refno, 成员表非空)`，键序。
    by_type: HashMap<u32, Vec<(u64, bool)>>,
    /// NAME 原文 → refnos。重名保留多值。
    by_name: HashMap<String, Vec<u64>>,
    /// target → 入边。
    inbound: HashMap<u64, Vec<BackRef>>,
}

impl DbIndexes {
    /// 某 noun（按 db1 哈希）的全部 refno，`has_children` 语义与
    /// `query_type_refnos_by_dbnum` 对齐：`Some(true)` 只要成员表非空的。
    pub fn refnos_of_noun_hash(&self, noun_hash: u32, has_children: Option<bool>) -> Vec<RefU64> {
        let Some(rows) = self.by_type.get(&noun_hash) else {
            return Vec::new();
        };
        rows.iter()
            .filter(|(_, childful)| has_children.is_none_or(|want| *childful == want))
            .map(|(raw, _)| RefU64(*raw))
            .collect()
    }

    /// 按 noun 名（"SITE"）查，名字在这里换成哈希，别处不得再有第二份换算。
    pub fn refnos_of_noun(&self, noun: &str, has_children: Option<bool>) -> Vec<RefU64> {
        self.refnos_of_noun_hash(e3d_attlib::db1_hash(noun), has_children)
    }

    /// 这个库里出现过的全部 noun_hash 及元素数。
    pub fn noun_census(&self) -> Vec<(u32, usize)> {
        let mut out: Vec<(u32, usize)> = self
            .by_type
            .iter()
            .map(|(hash, rows)| (*hash, rows.len()))
            .collect();
        out.sort_unstable();
        out
    }

    /// 按存储的 NAME 原文查。查不到就是没有——名字规范化（大小写、前导 `/`）
    /// 是调用方的口径，索引存的是文件里的原文。
    pub fn find_named(&self, name: &str) -> Vec<RefU64> {
        self.by_name
            .get(name)
            .map(|rows| rows.iter().map(|raw| RefU64(*raw)).collect())
            .unwrap_or_default()
    }

    pub fn named_count(&self) -> usize {
        self.by_name.len()
    }

    /// 指向 `target` 的全部入边。
    pub fn inbound_of(&self, target: RefU64) -> &[BackRef] {
        self.inbound
            .get(&target.0)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// 从打开的引擎构建。锁与池化是 `DirectStore` 的事，这里只管扫和装。
    pub fn build(
        engine: &mut ReadOnlyEngine,
        provider: &mut TemplateProvider,
        attlib: &e3d_attlib::AttlibData,
        fingerprint: IndexFingerprint,
    ) -> anyhow::Result<Self> {
        let started = Instant::now();
        let owner_hash = e3d_attlib::db1_hash("OWNER");

        let scanned: Vec<e3d_io::ScannedElement> = engine
            .scan_elements(ScanTier::Full)?
            .collect::<Result<Vec<_>, _>>()?;

        let mut by_type: HashMap<u32, Vec<(u64, bool)>> = HashMap::new();
        let mut by_name: HashMap<String, Vec<u64>> = HashMap::new();
        let mut inbound: HashMap<u64, Vec<BackRef>> = HashMap::new();
        let mut stats = BuildStats {
            elements: scanned.len() as u64,
            ..Default::default()
        };

        for element in &scanned {
            let parsed = element
                .parsed
                .as_ref()
                .expect("ScanTier::Full carries the parsed record");
            let source = raw(element.refno);

            by_type
                .entry(element.noun_hash)
                .or_default()
                .push((source, !parsed.members.is_empty()));

            if let Some(name) = &element.name {
                stats.named += 1;
                by_name.entry(name.clone()).or_default().push(source);
            }

            let mut push = |target: RefNo, via: BackRefVia| {
                let target = raw(target);
                if target == source || target == 0 {
                    return;
                }
                inbound
                    .entry(target)
                    .or_default()
                    .push(BackRef { source, via });
                stats.inbound_edges += 1;
            };

            push(element.owner, BackRefVia::Owner);
            for member in &parsed.members {
                push(*member, BackRefVia::Member);
            }

            // 显式流的 ref 型属性。
            for attribute in &parsed.explicit_attributes {
                use e3d_io::record::explicit::ExplicitValue;
                match &attribute.value {
                    ExplicitValue::RefNo(target) => push(*target, BackRefVia::Attr(attribute.hash)),
                    ExplicitValue::RefNoArray(targets) => {
                        for target in targets {
                            push(*target, BackRefVia::Attr(attribute.hash));
                        }
                    }
                    _ => {}
                }
            }

            // 隐式区的 ref 型属性，按描述符抽取。模板缺失/不权威不是整库失败：
            // 那个元素退成上面两路，计数留痕。
            let template = match provider.template_for(element.noun_hash) {
                Ok(Some(template)) => template,
                _ => {
                    stats.template_fallbacks += 1;
                    continue;
                }
            };
            let extraction = match engine.extract_parsed_element_with_descriptors(
                element.refno,
                parsed,
                attlib,
                template,
            ) {
                Ok(extraction) => extraction,
                Err(_) => {
                    stats.template_fallbacks += 1;
                    continue;
                }
            };
            for attribute in &extraction.attributes {
                use e3d_io::record::descriptor::{AttributeExtractionStatus, DescriptorValue};
                // 只收文件里真存了值的（含公式覆盖）；默认值不是出边——它不指向
                // 任何真实存储的引用，反转出来的入边在 Surreal 侧也不存在。
                if !matches!(
                    attribute.status,
                    AttributeExtractionStatus::Decoded | AttributeExtractionStatus::DecodedExplicit
                ) {
                    continue;
                }
                // 头字段那条 Owner 边已收，模板里的 OWNER 描述符不再计一次。
                if attribute.hash == owner_hash {
                    continue;
                }
                match &attribute.value {
                    Some(DescriptorValue::RefNo(target)) => {
                        push(*target, BackRefVia::Attr(attribute.hash));
                    }
                    Some(DescriptorValue::RefNoArray(targets)) => {
                        for target in targets {
                            push(*target, BackRefVia::Attr(attribute.hash));
                        }
                    }
                    _ => {}
                }
            }
        }

        stats.build_ms = started.elapsed().as_millis() as u64;
        Ok(Self {
            fingerprint,
            stats,
            by_type,
            by_name,
            inbound,
        })
    }

    /// 指纹命中读缓存，不中就构建并写回。
    ///
    /// 缓存 I/O 的任何失败都走重建：索引可以再算，读进一份坏的没法再对。
    pub fn load_or_build(
        engine: &mut ReadOnlyEngine,
        provider: &mut TemplateProvider,
        attlib: &e3d_attlib::AttlibData,
        fingerprint: IndexFingerprint,
    ) -> anyhow::Result<Self> {
        let cache = cache_path(&fingerprint);
        if fingerprint.cacheable() {
            if let Some(hit) = read_cache(&cache, &fingerprint) {
                return Ok(hit);
            }
        }
        let built = Self::build(engine, provider, attlib, fingerprint)?;
        if built.fingerprint.cacheable() {
            // 写失败只意味着下次重建，不值得让本次构建跟着失败。
            let _ = write_cache(&cache, &built);
        }
        Ok(built)
    }
}

fn raw(refno: RefNo) -> u64 {
    RefU64::from_two_nums(refno.word0, refno.word1).0
}

/// 缓存文件路径：目录来自环境或系统临时目录，文件名带 dbnum 与会话号。
fn cache_path(fingerprint: &IndexFingerprint) -> PathBuf {
    let dir = std::env::var_os(INDEX_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("aios-direct-index"));
    dir.join(format!(
        "idx-v{}-db{}-s{}.bin",
        FORMAT_VERSION, fingerprint.dbnum, fingerprint.pinned_sesno
    ))
}

fn read_cache(path: &Path, expected: &IndexFingerprint) -> Option<DbIndexes> {
    let bytes = std::fs::read(path).ok()?;
    let indexes: DbIndexes = bincode::deserialize(&bytes).ok()?;
    (&indexes.fingerprint == expected).then_some(indexes)
}

fn write_cache(path: &Path, indexes: &DbIndexes) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = bincode::serialize(indexes)?;
    // 先写旁文件再改名，读端永远看不到半个文件。
    let tmp = path.with_extension("bin.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_without_a_mtime_never_hits_the_cache() {
        let fingerprint = IndexFingerprint {
            format_version: FORMAT_VERSION,
            dbnum: 1,
            pinned_sesno: 2,
            file_len: 3,
            file_mtime_ns: None,
        };
        assert!(!fingerprint.cacheable());
    }

    #[test]
    fn queries_on_an_empty_index_answer_empty_not_panic() {
        let indexes = DbIndexes {
            fingerprint: IndexFingerprint {
                format_version: FORMAT_VERSION,
                dbnum: 1,
                pinned_sesno: 1,
                file_len: 0,
                file_mtime_ns: None,
            },
            stats: BuildStats::default(),
            by_type: HashMap::new(),
            by_name: HashMap::new(),
            inbound: HashMap::new(),
        };
        assert!(indexes.refnos_of_noun("SITE", None).is_empty());
        assert!(indexes.find_named("/NOWHERE").is_empty());
        assert!(indexes.inbound_of(RefU64(7)).is_empty());
    }
}
