//! UKEY → UDA 定义 refno 的解析：`ATT_UDA` 行里 `u` 记录链接的唯一来源。
//!
//! 设计元素把 UDA 值存在以 `UKEY`（哈希）为键的显式属性区；消费者（rs-core
//! `query.rs`、`common.surql`、料表 surql）读的却是 `(ATT_UDA:{id}).udas` 里的
//! `u.UDNA / u.DYUDNA / u.UTYP`——`u` 必须是指向 `UDA:<refno>` 名词表行的记录链接，
//! 而那张 `UDA` 表来自 DICT 库的同步。所以写 `ATT_UDA` 之前要把每个 `hash_val`
//! 换成 `UDA` 表里对应定义的 refno；legacy 基线靠 `aios_core::get_uda_refno` 逐个
//! 异步点查（`UKEY = hash`，查不到再按 dehash 出的四字名 + 序号回退），本模块把
//! 同一套判据搬到内存：一次 `SELECT id, UKEY, UDNA FROM UDA` 建表，之后渲染器同步查。
//!
//! 查不到的键**不静默丢**（宪法 III）：记进 [`UdaKeyMap::take_misses`]，由调用方写进
//! 回执 `warnings`；那一个 UDA 值不写，元素的其它属性照常落库。legacy 路径是直接丢。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use aios_core::RefU64;
use aios_core::tool::db_tool::{db1_dehash, get_uda_index, is_uda};
use serde::Deserialize;

/// `UDA` 名词表里一条定义的最小投影。
#[derive(Debug, Clone, Deserialize)]
pub struct UdaDefinitionRow {
    pub id: RefU64,
    #[serde(rename = "UKEY", default)]
    pub ukey: Option<i64>,
    #[serde(rename = "UDNA", default)]
    pub udna: Option<String>,
}

/// 一次同步 / 一个窗口共用的解析表。
#[derive(Debug, Default)]
pub struct UdaKeyMap {
    by_key: HashMap<i32, RefU64>,
    /// 按 `UKEY` 升序排好的 `(UKEY, UDNA, id)`，给 legacy 的「四字名 + 序号」回退用。
    ordered: Vec<(i64, String, RefU64)>,
    misses: Mutex<BTreeMap<i32, BTreeSet<RefU64>>>,
}

impl Clone for UdaKeyMap {
    fn clone(&self) -> Self {
        Self {
            by_key: self.by_key.clone(),
            ordered: self.ordered.clone(),
            // Misses are per render attempt; copying old diagnostics would
            // report keys that the cloned operation never touched.
            misses: Mutex::default(),
        }
    }
}

impl UdaKeyMap {
    /// 空表：每个 UDA 键都会记成 miss。测试与「没有 DICT 库」的库用。
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: impl IntoIterator<Item = UdaDefinitionRow>) -> Self {
        let mut by_key = HashMap::new();
        let mut ordered = Vec::new();
        for row in rows {
            let Some(ukey) = row.ukey else {
                continue;
            };
            if let Ok(key) = i32::try_from(ukey) {
                // 同一 UKEY 多个定义时保留第一条，与 legacy `limit 1` 同义。
                by_key.entry(key).or_insert(row.id);
            }
            ordered.push((ukey, row.udna.unwrap_or_default(), row.id));
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            by_key,
            ordered,
            misses: Mutex::default(),
        }
    }

    /// Build the compatibility record-link map directly from e3d-io's ordered
    /// Dictionary catalog.  Read-through mode has no DICT ingestion phase, so
    /// consulting the database here would make actual UDA values disappear.
    pub fn from_catalog(catalog: &e3d_io::uda_catalog::UdaCatalog) -> Self {
        Self::from_rows(catalog.definitions().iter().flat_map(|definition| {
            let row = |key| UdaDefinitionRow {
                id: RefU64::from_two_nums(definition.refno.word0, definition.refno.word1),
                ukey: Some(i64::from(key as i32)),
                udna: Some(format!(":{}", definition.name)),
            };
            std::iter::once(row(definition.key)).chain(definition.old_key.map(row))
        }))
    }

    /// 从当前 SurrealDB 的 `UDA` 表建表。DICT 相先于 DESI 相同步，所以 DESI 基线 /
    /// 增量到这里时定义已经在库里；表为空不是错误（项目没有 UDA 或没配 DICT 库）。
    pub async fn load() -> anyhow::Result<Self> {
        Self::load_on(&aios_core::SUL_DB).await
    }

    /// 同 [`UdaKeyMap::load`]，目标库由调用方给（结构化 sink 在 backup lane 上渲 `ATT_UDA` 时用）。
    pub async fn load_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> anyhow::Result<Self> {
        let mut response = db
            .query("SELECT id, UKEY, UDNA FROM UDA;")
            .await
            .map_err(|e| anyhow::anyhow!("读取 UDA 定义表失败: {e}"))?;
        let rows: Vec<UdaDefinitionRow> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码 UDA 定义表失败: {e}"))?;
        Ok(Self::from_rows(rows))
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// 与 `aios_core::get_uda_refno` 同一判据：先 `UKEY == hash`，再按 dehash 出的
    /// 四字名在 `UDNA` 里做子串匹配、按 `UKEY` 升序取第 `index` 条。
    pub fn resolve(&self, hash: i32) -> Option<RefU64> {
        if !is_uda(hash) {
            return None;
        }
        if let Some(refno) = self.by_key.get(&hash) {
            return Some(*refno);
        }
        let dehashed = db1_dehash(hash as u32);
        let name = dehashed.get(1..)?;
        if name.is_empty() {
            return None;
        }
        let index = get_uda_index(hash as u32).unwrap_or_default() as usize;
        self.ordered
            .iter()
            .filter(|(_, udna, _)| udna.contains(name))
            .nth(index)
            .map(|(_, _, id)| *id)
    }

    /// 解析并在查不到时记账；渲染器用这一条。
    pub fn resolve_for(&self, element: RefU64, hash: i32) -> Option<RefU64> {
        let resolved = self.resolve(hash);
        if resolved.is_none() {
            if let Ok(mut misses) = self.misses.lock() {
                misses.entry(hash).or_default().insert(element);
            }
        }
        resolved
    }

    /// 取走到目前为止的全部 miss：`hash → 受影响元素`。
    pub fn take_misses(&self) -> BTreeMap<i32, BTreeSet<RefU64>> {
        self.misses
            .lock()
            .map(|mut misses| std::mem::take(&mut *misses))
            .unwrap_or_default()
    }

    /// 把 miss 写成一句回执告警；没有 miss 返回 `None`。
    pub fn misses_warning(&self, dbnum: u32) -> Option<String> {
        let misses = self.take_misses();
        if misses.is_empty() {
            return None;
        }
        let affected: usize = misses.values().map(BTreeSet::len).sum();
        let detail = misses
            .iter()
            .take(8)
            .map(|(hash, elements)| {
                let sample = elements
                    .iter()
                    .take(3)
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                format!(
                    "{}(0x{:x})×{}[{}]",
                    db1_dehash(*hash as u32),
                    *hash as u32,
                    elements.len(),
                    sample
                )
            })
            .collect::<Vec<_>>()
            .join("、");
        Some(format!(
            "dbnum={dbnum}: {} 个 UDA 键在 UDA 表里没有定义，{affected} 个元素的这些 UDA 值未写入 ATT_UDA：{detail}",
            misses.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64, ukey: i64, udna: &str) -> UdaDefinitionRow {
        UdaDefinitionRow {
            id: RefU64(id),
            ukey: Some(ukey),
            udna: Some(udna.to_string()),
        }
    }

    /// 真实形状的 UDA 键：`0x171FAD39 + 名字编码 + (序号 << 24)`。
    fn uda_hash(name4: &str, index: u32) -> i32 {
        let mut k: u32 = 0;
        for (i, ch) in name4.bytes().enumerate() {
            k += ((ch as u32) - 32) * 64u32.pow(i as u32);
        }
        (0x171F_AD39u32 + k + (index << 24)) as i32
    }

    #[test]
    fn exact_ukey_wins_and_non_uda_hashes_are_never_resolved() {
        let hash = uda_hash("ROOM", 0);
        let map = UdaKeyMap::from_rows([row(1, hash as i64, ":ROOM")]);
        assert_eq!(map.resolve(hash), Some(RefU64(1)));
        assert_eq!(map.resolve(0x0008_1F4B), None, "普通属性哈希不是 UDA 键");
        assert!(
            map.take_misses().is_empty(),
            "resolve 不记账，resolve_for 才记"
        );
    }

    #[test]
    fn fallback_matches_dehashed_name_by_ukey_order_and_index() {
        // 库里的 UKEY 与元素存的键不同（重新分配过），只能靠四字名 + 序号回退。
        let stored = uda_hash("SECT", 1);
        let map = UdaKeyMap::from_rows([
            row(30, 300, ":H-SECTCODE"),
            row(10, 100, ":SECTA"),
            row(20, 200, ":OTHER"),
        ]);
        // 含 "SECT" 的按 UKEY 升序是 [":SECTA"(100), ":H-SECTCODE"(300)]，序号 1 → 300。
        assert_eq!(map.resolve(stored), Some(RefU64(30)));
        assert_eq!(map.resolve(uda_hash("SECT", 0)), Some(RefU64(10)));
        assert_eq!(map.resolve(uda_hash("SECT", 2)), None);
    }

    #[test]
    fn misses_are_recorded_per_hash_with_the_elements_that_hit_them() {
        let map = UdaKeyMap::empty();
        let hash = uda_hash("ZZZZ", 0);
        assert_eq!(map.resolve_for(RefU64(7), hash), None);
        assert_eq!(map.resolve_for(RefU64(8), hash), None);
        let warning = map.misses_warning(8000).expect("有 miss 就有告警");
        assert!(warning.contains("dbnum=8000"), "{warning}");
        assert!(warning.contains("2 个元素"), "{warning}");
        assert!(map.misses_warning(8000).is_none(), "take 之后清零");
    }
}
