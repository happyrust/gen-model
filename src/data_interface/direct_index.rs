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
//! 第四张表 **attr_text**（G3 分组规则求值加）：字符串显式属性原文 → refno，键是
//! `(属性 hash, 原文)`。SCOSEL 规则的 `402` 是全串相等，所以它要的是「原文精确
//! 命中」而不是 name 那种「按名字查」——两者键不同，不能共用 `by_name`。
//! 它在**同一遍**扫描里建，不增加 `scan_elements` 次数。
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
///
/// 2：新增 `by_attr_text`（字符串属性倒排表，G3 的 SCOSEL 求值要用）。提到 2 之后
/// 缓存文件名（`idx-v2-*.bin`）与指纹里的 `format_version` 两处都变，旧的
/// `idx-v1-*.bin` 既不会被打开、就算改名也过不了 `read_cache` 的相等比较——
/// 少任何一道，bincode 都会把旧结构反序列化成 `by_attr_text` 为空的新结构，
/// 不报错，只是规则组永远查空。
const FORMAT_VERSION: u32 = 2;

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
    pub fn of(file: &Path, dbnum: i32, pinned_sesno: u32) -> std::io::Result<Self> {
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

/// 一个库在一个时点上的派生索引：type / name / backref / attr_text。
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
    /// 字符串属性倒排表：属性 hash → 原文 → refnos。G3 的 SCOSEL 求值靠它。
    ///
    /// 只收 `ExplicitValue::Text`。hunk（`RawWords`，DESC 超过 120 字符时走 type-22）
    /// 与 `TextList` 都不进——当前语料里规则匹配的 `DESC` 全是 inline 文本
    /// （`library/scosel-semantics.md` §6：全库 18482 个元素有显式 DESC，形态
    /// `type_code = 10240` / `ExplicitValue::Text`），hunk 值既不进表也不报错，
    /// 求值器把它记成 `unsupported` 而不是静默。
    ///
    /// 重名保留多值：两个元素可以有同一个 DESC 原文（APS 实测 `AREA01PIPE`
    /// 在两个库各有一个 ZONE），求值域由调用方按 MDB 收窄，索引不折叠。
    by_attr_text: HashMap<u32, HashMap<String, Vec<u64>>>,
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

    /// 字符串属性原文 → refnos：`(attr_hash, 原文)` 全串相等。
    ///
    /// 这是 SCOSEL 求值的两次查表之一（另一次是 noun 限定，走
    /// [`Self::refnos_of_noun_hash`]）。口径是**全串相等**、**区分大小写**，
    /// 与真实字节一致：`402` 是全串相等不是子串（`AREA01` 子串命中 20、全串 0），
    /// 语料里 DESC 全大写、无大小写混合样本，首版取保守口径（契约 VAL-EVAL-009
    /// 钉住这个决定）。
    ///
    /// 查不到就是没有——空索引、未知属性 hash、原文不在表里，三种都回空 `Vec`，
    /// 不 panic 不报错。求值器把空集当「没命中」，一条规则不该把请求打挂。
    pub fn refnos_of_attr_text(&self, attr_hash: u32, text: &str) -> Vec<RefU64> {
        self.by_attr_text
            .get(&attr_hash)
            .and_then(|by_text| by_text.get(text))
            .map(|rows| rows.iter().map(|raw| RefU64(*raw)).collect())
            .unwrap_or_default()
    }

    /// Case-insensitive NAME substring search over the frozen index.
    pub fn search_names(&self, query: &str) -> Vec<(String, RefU64)> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut out = self
            .by_name
            .iter()
            .filter(|(name, _)| name.to_ascii_lowercase().contains(&needle))
            .flat_map(|(name, rows)| rows.iter().map(move |raw| (name.clone(), RefU64(*raw))))
            .collect::<Vec<_>>();
        out.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.0.cmp(&right.1.0)));
        out
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
        let mut by_attr_text: HashMap<u32, HashMap<String, Vec<u64>>> = HashMap::new();
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

            // 显式流的两件事都在这一遍里做完：ref 型属性反转入边，文本型属性进
            // 字符串倒排表。不另开 `scan_elements`——APS 全扫实测 0.8s / 499839
            // 元素，翻倍是不可接受的（VAL-INDEX-005 钉 `scan_calls == 1`）。
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
                // `indexed_text` 只对 inline `Text` 给 `Some`；hunk（`RawWords`）与
                // `TextList` 都是 `None`，于是「hunk 不进表」由这一个函数决定，
                // 可以被单测直接钉住（VAL-INDEX-002）。
                if let Some(text) = indexed_text(&attribute.value) {
                    index_text_attribute(&mut by_attr_text, attribute.hash, text, source);
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
            by_attr_text,
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

/// 字符串属性倒排表的一格：`by_attr_text[属性 hash][原文] = [refno...]`。
///
/// 在 `build()` 的**同一遍** `scan_elements(ScanTier::Full)` 里被调，不另开扫描
/// （APS 全扫实测 0.8s / 499839 元素，翻倍是不可接受的；VAL-INDEX-005 用
/// `read_stats.scan_calls == 1` 钉住这件事）。
///
/// 独立函数而不是内联在 match 臂里：`build()` 那个循环已经背着 `push` 闭包，
/// 再加一段建表逻辑会把「边反转」和「文本倒排」两件事糊在一起。
fn index_text_attribute(
    by_attr_text: &mut HashMap<u32, HashMap<String, Vec<u64>>>,
    attr_hash: u32,
    text: &str,
    source: u64,
) {
    by_attr_text
        .entry(attr_hash)
        .or_default()
        .entry(text.to_string())
        .or_default()
        .push(source);
}

/// 一个显式属性的值是否该进字符串倒排表——`Some(原文)` 才进。
///
/// **只收 inline 的 `ExplicitValue::Text`**，其余一律 `None`：
///
/// * **hunk（`RawWords`）**：`DESC` 超过 120 字符时走 type-22 hunk，在
///   `explicit_attributes` 里是五字地址 `[1,page,packed,count,words]`，原文在别处。
///   这里是「记录原样解析」的那一遍，没有 pages 可读（`resolve_hunks` 是另一个
///   入口），所以原文根本拿不到——跳过它，并且**不报错**：求值侧会把命不中记成
///   `unsupported`，不静默。当前语料里规则匹配的 DESC 全是 inline 文本
///   （`library/scosel-semantics.md` §6；APS 全库实测 0 个 hunk 形态的 DESC）。
/// * `TextList`（type 19，字符串数组）也不进：`402` 比的是单个标量原文。
///
/// 抽成纯函数是为了让「hunk 不进表」成为**可判定**的断言（VAL-INDEX-002）：
/// `build()` 只能通过它把值放进表，所以 `indexed_text(RawWords) == None` 就等价于
/// 「hunk 永远进不了表」。
fn indexed_text(value: &e3d_io::record::explicit::ExplicitValue) -> Option<&str> {
    use e3d_io::record::explicit::ExplicitValue;
    match value {
        ExplicitValue::Text(text) => Some(text.as_str()),
        _ => None,
    }
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

    fn empty_indexes() -> DbIndexes {
        DbIndexes {
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
            by_attr_text: HashMap::new(),
        }
    }

    #[test]
    fn queries_on_an_empty_index_answer_empty_not_panic() {
        let indexes = empty_indexes();
        assert!(indexes.refnos_of_noun("SITE", None).is_empty());
        assert!(indexes.find_named("/NOWHERE").is_empty());
        assert!(indexes.inbound_of(RefU64(7)).is_empty());
    }

    /// VAL-INDEX-003：空索引查空不 panic——包括新加的字符串属性倒排表。
    ///
    /// 两个维度都要空：属性 hash 不认识（表里没有这个外层键）、属性 hash 认识但
    /// 原文不在（内层表里没有这个字符串）。求值器会把这两种情况当成「没命中」，
    /// 一旦这里 panic 或报错，一条规则就能把整个请求打挂。
    #[test]
    fn an_empty_attr_text_index_answers_empty_without_panicking() {
        let indexes = empty_indexes();
        assert!(indexes.refnos_of_attr_text(999_999, "不存在").is_empty());
        assert!(
            indexes
                .refnos_of_attr_text(e3d_attlib::db1_hash("DESC"), "不存在")
                .is_empty()
        );
        assert!(indexes.refnos_of_attr_text(0, "").is_empty());
    }

    /// VAL-INDEX-004：`FORMAT_VERSION` 提到 2，缓存文件名与指纹都跟着变。
    ///
    /// 双闸的意义：文件名变了，旧的 `idx-v1-db1-s2.bin` 根本不会被打开；指纹里的
    /// `format_version` 变了，就算有人手工把旧文件改成新名字，`read_cache` 的相等
    /// 比较也会把它判为「别人的索引」而重建。少任何一道，bincode 都会把旧结构
    /// （没有 `by_attr_text`）反序列化成一个「字段全默认」的新结构——不报错，只是
    /// 字符串属性索引永远空，规则组永远空转。
    #[test]
    fn the_format_version_bump_invalidates_old_cache_names() {
        assert_eq!(FORMAT_VERSION, 2, "FORMAT_VERSION 必须是 2");
        let fingerprint = IndexFingerprint {
            format_version: FORMAT_VERSION,
            dbnum: 7,
            pinned_sesno: 11,
            file_len: 1,
            file_mtime_ns: Some(1),
        };
        let path = cache_path(&fingerprint);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert_eq!(name, "idx-v2-db7-s11.bin", "缓存文件名要带 v2");
        // 指纹里的 format_version 由 `IndexFingerprint::of` 从 FORMAT_VERSION 填；
        // 这里对一个真实文件走一遍，确认落盘的那一份也带 2。
        let probe = std::env::temp_dir().join("aios-direct-index-fingerprint-probe.bin");
        std::fs::write(&probe, b"probe").expect("probe file writes");
        let real = IndexFingerprint::of(&probe, 7, 11).expect("probe file stats");
        assert_eq!(real.format_version, 2, "指纹里的 format_version 也要是 2");
        let _ = std::fs::remove_file(&probe);
        // 旧版本号在指纹里必须判不相等——这是「旧文件改成新名字也读不进来」那一道闸。
        let stale = IndexFingerprint {
            format_version: 1,
            ..fingerprint.clone()
        };
        assert_ne!(stale, fingerprint);
    }

    /// VAL-INDEX-001 / VAL-INDEX-003：`by_attr_text` 按 `(属性 hash, 原文)` 反查。
    ///
    /// 这里用字面量构造钉住访问器语义（`build()` 真的填表由 APS 真库的
    /// `aps_text_attributes_land_in_the_inverted_index` 证明）：同名属性下不同原文
    /// 各自成组，同名不同属性互不串味，查回来的是 `RefU64` 而不是裸 u64。
    #[test]
    fn text_attributes_are_indexed_by_hash_and_value() {
        let desc = e3d_attlib::db1_hash("DESC");
        let name = e3d_attlib::db1_hash("NAME");
        let mut by_attr_text: HashMap<u32, HashMap<String, Vec<u64>>> = HashMap::new();
        by_attr_text
            .entry(desc)
            .or_default()
            .insert("AREA01PIPE".to_string(), vec![1, 2]);
        by_attr_text
            .entry(desc)
            .or_default()
            .insert("AREA01GRID".to_string(), vec![3]);
        by_attr_text
            .entry(name)
            .or_default()
            .insert("AREA01PIPE".to_string(), vec![9]);
        let indexes = DbIndexes {
            by_attr_text,
            ..empty_indexes()
        };

        assert_eq!(
            indexes.refnos_of_attr_text(desc, "AREA01PIPE"),
            vec![RefU64(1), RefU64(2)],
            "DESC=AREA01PIPE 的两个元素都要查回来"
        );
        assert_eq!(
            indexes.refnos_of_attr_text(desc, "AREA01GRID"),
            vec![RefU64(3)]
        );
        assert_eq!(
            indexes.refnos_of_attr_text(name, "AREA01PIPE"),
            vec![RefU64(9)],
            "同原文不同属性互不串味"
        );
        assert!(indexes.refnos_of_attr_text(desc, "AREAGRID").is_empty());
        assert!(indexes.refnos_of_attr_text(desc, "area01pipe").is_empty(), "首版区分大小写");
    }

    /// VAL-INDEX-002：hunk 型属性值被跳过，且不是「悄悄跳过」——是**由类型判定**
    /// 决定的跳过。
    ///
    /// `build()` 只通过 [`indexed_text`] 把值放进 `by_attr_text`，所以把四种形状喂
    /// 进去就等于把整条建表路径钉住了：inline `Text` 进表；hunk 的五字地址
    /// `RawWords([1,page,packed,count,words])`、字符串数组 `TextList`、整型数组
    /// `IntArray` 全部不进。`DESC` 超过 120 字符时就是 `RawWords` 形态
    /// （`library/scosel-semantics.md` §6），这类值既不进表也不报错，求值侧把它记成
    /// `unsupported`。
    #[test]
    fn hunk_values_are_skipped_without_failing_the_build() {
        use e3d_io::record::explicit::ExplicitValue;

        assert_eq!(
            indexed_text(&ExplicitValue::Text("AREA01PIPE".into())),
            Some("AREA01PIPE")
        );
        assert_eq!(
            indexed_text(&ExplicitValue::RawWords(vec![1, 4211, 0, 3, 60])),
            None,
            "hunk 的五字地址不是原文，不得进表"
        );
        assert_eq!(
            indexed_text(&ExplicitValue::TextList(vec!["A".into(), "B".into()])),
            None,
            "字符串数组不是 SCOSEL 的 402 要比较的标量"
        );
        assert_eq!(indexed_text(&ExplicitValue::IntArray(vec![1, 2])), None);
        assert_eq!(indexed_text(&ExplicitValue::Integer(7)), None);

        // hunk 值喂给建表入口不会 panic、也不会留下任何桶——`build()` 根本不会调它。
        let mut table: HashMap<u32, HashMap<String, Vec<u64>>> = HashMap::new();
        if let Some(text) = indexed_text(&ExplicitValue::RawWords(vec![1, 0, 0, 3, 60])) {
            index_text_attribute(&mut table, 604_480, text, 42);
        }
        assert!(table.is_empty(), "hunk 值不该建出任何桶");
    }

    /// APS 真库里 `/ZONE-PIPING-AREA01` 所在的库（dbno 250132）与模板目录。
    const APS_LIBRARY: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000\aps250132_0001";
    const APS_LIBRARY_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000";
    const APS_TEMPLATE_DIR: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

    /// 扫一个库，分出「DESC 是 inline 文本」与「DESC 是 hunk」的元素。
    fn desc_shapes(library: &Path) -> (Vec<(String, RefU64)>, Vec<RefU64>) {
        use e3d_io::record::explicit::ExplicitValue;
        let desc_hash = e3d_attlib::db1_hash("DESC");
        let mut engine = ReadOnlyEngine::open(library).expect("library opens");
        let mut inline: Vec<(String, RefU64)> = Vec::new();
        let mut hunks: Vec<RefU64> = Vec::new();
        for element in engine.scan_elements(ScanTier::Full).expect("scans") {
            let element = element.expect("every element reads");
            let parsed = element.parsed.as_ref().expect("full tier");
            let refno = RefU64::from_two_nums(element.refno.word0, element.refno.word1);
            for attribute in &parsed.explicit_attributes {
                if attribute.hash != desc_hash {
                    continue;
                }
                match &attribute.value {
                    ExplicitValue::Text(text) => inline.push((text.clone(), refno)),
                    ExplicitValue::RawWords(_) => hunks.push(refno),
                    _ => {}
                }
            }
        }
        (inline, hunks)
    }

    /// VAL-INDEX-001 / VAL-INDEX-002 / VAL-INDEX-005：真库上建表，与独立扫描逐条对齐。
    ///
    /// 一次扫描收集「文件说的」（哪些元素的 DESC 是 inline 文本、哪些是 hunk），
    /// 然后 `reset()` 计数、调 `build()`、再断言：
    ///
    /// * `scan_calls == 1`——`by_attr_text` 复用既有那一遍 `ScanTier::Full`，
    ///   没有为它再扫一次（APS 全扫 0.8s，翻倍是不可接受的）。
    /// * 每个 inline `DESC` 文本桶与独立扫描逐条相等（VAL-INDEX-001）。
    /// * hunk 形态的 `DESC`（`RawWords`）**不进表**且 build 不 panic（VAL-INDEX-002）。
    /// * 查空不 panic（VAL-INDEX-003 的真库侧）。
    ///
    /// `read_stats` 是进程全局计数器，这条要与其它真库测试分开单跑：
    /// `cargo test --lib aps_text_attributes -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore = "requires local APS source files"]
    fn aps_text_attributes_land_in_the_inverted_index() {
        let library = Path::new(APS_LIBRARY);
        let templates = Path::new(APS_TEMPLATE_DIR);
        if !library.is_file() || !templates.join("attlib.dat").is_file() {
            eprintln!("SKIP: APS fixtures not present ({library:?}, {templates:?})");
            return;
        }
        let desc_hash = e3d_attlib::db1_hash("DESC");
        assert_eq!(desc_hash, 604_480, "DESC hash 变了，下面的断言要重定");

        // ── 1. 独立扫描：文件说的 inline 文本各是哪些元素 ──
        let (inline, _hunks) = desc_shapes(library);
        assert!(
            !inline.is_empty(),
            "APS 的 aps250132 里没有 inline DESC，这条门证明不了任何事"
        );

        // ── 2. 同一遍扫描建表（计数从 0 起，断言只扫了一次）──
        let attlib = e3d_attlib::AttlibData::parse_file(&templates.join("attlib.dat"))
            .expect("attlib parses");
        let mut provider = TemplateProvider::open(
            templates.join("desvir.dat"),
            Some(e3d_attlib::db1_hash("DESI")),
        )
        .expect("DESI template opens");
        let mut engine = ReadOnlyEngine::open(library).expect("APS library opens");
        let sesno = engine.session().sesno;
        let fingerprint = IndexFingerprint::of(library, 250_132, sesno).expect("fingerprint");
        e3d_io::read_stats::reset();
        let indexes = DbIndexes::build(&mut engine, &mut provider, &attlib, fingerprint)
            .expect("indexes build");
        let stats = e3d_io::read_stats::snapshot();
        assert_eq!(
            stats.scan_calls, 1,
            "by_attr_text 必须复用既有那一遍扫描，实际扫了 {} 次: {}",
            stats.scan_calls,
            stats.summary()
        );

        // ── 3. 每个 inline 文本桶与独立扫描逐条相等（VAL-INDEX-001 / VAL-INDEX-002）──
        //
        // 用「文件说的」建一张期望表：文本 → refno 集合。索引必须与它逐桶相等。
        // 这条同时排掉两类错误：漏收（某条 inline 文本没进表）与多收（hunk 元素的
        // refno 混进了某个文本桶——它没有 inline 文本，期望表里不该有它）。
        let mut expected: std::collections::BTreeMap<&str, Vec<RefU64>> =
            std::collections::BTreeMap::new();
        for (text, refno) in &inline {
            expected.entry(text.as_str()).or_default().push(*refno);
        }
        for (text, mut want) in expected {
            want.sort();
            want.dedup();
            let mut got = indexes.refnos_of_attr_text(desc_hash, text);
            got.sort();
            got.dedup();
            assert_eq!(got, want, "DESC={text:?} 的索引答案与独立扫描不一致");
        }
        // 索引里 DESC 下的桶数不得超过扫描看到的 distinct 原文数——多出来的桶只可能
        // 来自不该进表的值（hunk / TextList）。
        let distinct_texts = inline
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let indexed_desc_texts = indexes
            .by_attr_text
            .get(&desc_hash)
            .map(HashMap::len)
            .unwrap_or_default();
        assert_eq!(
            indexed_desc_texts, distinct_texts,
            "DESC 的文本桶数与 inline 原文数不一致（hunk/TextList 混进来了？）"
        );

        // ── 4. 查空不 panic（VAL-INDEX-003 的真库侧）──
        assert!(indexes.refnos_of_attr_text(desc_hash, "不可能存在的 DESC 原文").is_empty());
        assert!(indexes.refnos_of_attr_text(999_999, "任意").is_empty());

        // ── 5. 已知基准：`/ZONE-PIPING-AREA01` 的 DESC 是 AREA01PIPE ──
        let known = RefU64::from_two_nums(2_013_286_676, 490);
        let zone_hits = indexes.refnos_of_attr_text(desc_hash, "AREA01PIPE");
        eprintln!(
            "DESC=AREA01PIPE -> {zone_hits:?}；inline DESC {} 个 / DESC 文本桶 {} 个 / {}",
            inline.len(),
            indexed_desc_texts,
            stats.summary()
        );
        assert!(
            zone_hits.contains(&known),
            "库内应含 /ZONE-PIPING-AREA01（{known:?}），实际 {zone_hits:?}"
        );
        let zones = indexes.refnos_of_noun_hash(e3d_attlib::db1_hash("ZONE"), None);
        assert!(
            zones.contains(&known),
            "ZONE 的 noun 索引里也应有 {known:?}"
        );

        // ── 6. hunk 形态的 DESC 不进表（VAL-INDEX-002 的真库侧）──
        //
        // APS 语料的实测事实：`DESC` 全是 inline 文本（`library/scosel-semantics.md`
        // §6：全库 18482 个元素有显式 DESC，形态 `type_code = 10240` /
        // `ExplicitValue::Text`；`scosel_eval_probe` 的 desc_shape 统计也全是
        // `Text(inline)`）。所以真库侧**没有** hunk 观测对象，这里探 4 个库确认这
        // 一点（探全部 98 个库要 30 秒，不值得），找不到就明说这条只由单测覆盖——
        // 不让门静默通过。
        let mut scanned_libraries = 0usize;
        let mut hunk_case = None;
        for entry in std::fs::read_dir(APS_LIBRARY_DIR).expect("APS library dir reads") {
            if scanned_libraries >= 4 {
                break;
            }
            let path = entry.expect("entry").path();
            if !path.is_file() {
                continue;
            }
            scanned_libraries += 1;
            let (lib_inline, lib_hunks) = desc_shapes(&path);
            if !lib_hunks.is_empty() {
                hunk_case = Some((path, lib_inline, lib_hunks));
                break;
            }
        }
        let Some((hunk_library, hunk_inline, hunk_refnos)) = hunk_case else {
            eprintln!(
                "APS 语料里探了 {scanned_libraries} 个库都没有 hunk 形态的 DESC；\
                 VAL-INDEX-002 的真库侧无可观测对象，只由单测 \
                 hunk_values_are_skipped_without_failing_the_build 覆盖"
            );
            return;
        };
        let dbnum = hunk_library
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut hunk_engine = ReadOnlyEngine::open(&hunk_library).expect("hunk library opens");
        let hunk_sesno = hunk_engine.session().sesno;
        let mut hunk_provider = TemplateProvider::open(
            templates.join("desvir.dat"),
            Some(e3d_attlib::db1_hash("DESI")),
        )
        .expect("DESI template opens");
        let hunk_fingerprint =
            IndexFingerprint::of(&hunk_library, 0, hunk_sesno).expect("fingerprint");
        let hunk_indexes =
            DbIndexes::build(&mut hunk_engine, &mut hunk_provider, &attlib, hunk_fingerprint)
                .expect("hunk library indexes build");
        // 逐桶检查：hunk 元素没有 inline DESC 原文，所以它不该出现在 DESC 下的**任何**
        // 文本桶里。遍历全部桶（而不是只查 inline 集合里的原文）才能覆盖「hunk 元素的
        // refno 被别的属性混进来」这种情况。
        let desc_buckets = hunk_indexes
            .by_attr_text
            .get(&desc_hash)
            .map(|by_text| by_text.len())
            .unwrap_or_default();
        for refno in &hunk_refnos {
            for (text, rows) in hunk_indexes
                .by_attr_text
                .get(&desc_hash)
                .map(|by_text| by_text.iter().collect::<Vec<_>>())
                .unwrap_or_default()
            {
                assert!(
                    !rows.contains(&refno.0),
                    "hunk 形态的 DESC 元素 {refno:?}（{dbnum}）不该出现在 DESC={text:?} 的桶里"
                );
            }
        }
        assert!(
            !hunk_refnos.is_empty(),
            "{dbnum} 里找到了 hunk 形态的 DESC 元素清单，不该为空"
        );
        eprintln!(
            "hunk DESC：{dbnum} 里 {} 个元素走 hunk，未进表；同库 inline DESC {} 个 / DESC 桶 {} 个",
            hunk_refnos.len(),
            hunk_inline.len(),
            desc_buckets
        );
    }
}
