//! 元素数据行的**批量渲染器**：先删后插，每表一条 `INSERT IGNORE`。
//!
//! 出处：`docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md` §3 D1/D2（共识 d-29）。
//!
//! rocksdb 形态下数据面（`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`）只有这一种写法，基线与增量共用：
//!
//! ```text
//! WriteMode::Fresh    —— 库里还没有这一批（首次基线、父层补缺）：不删，只 INSERT IGNORE（已有行赢）
//! WriteMode::Replace  —— 增量 / 依赖行版本前移：先删整组表示，再 INSERT IGNORE
//! ```
//!
//! 一块的语句顺序（Replace）：
//!
//! ```sql
//! DELETE pe_owner:[pe:A, NONE]..=[pe:A, ..];      -- 每个元素：它名下的子槽位（id 区间删，O(children)，无表扫）
//! LET $uda_ids  = [ATT_UDA:A, …];   DELETE $uda_ids;
//! LET $del_n0   = [type::thing('BRAN','A'), …];  DELETE $del_n0;   -- 每个 noun 一组（含换 noun 的旧表、墓碑的基侧 noun）
//! INSERT INTO pe [ … ] ON DUPLICATE KEY UPDATE name = $input.name, … RETURN NONE;   -- 见下「pe 行为什么不删」
//! INSERT IGNORE INTO BRAN    [ … ] RETURN NONE;          -- 每个 noun 一条
//! INSERT IGNORE INTO ATT_UDA [ … ] RETURN NONE;
//! INSERT RELATION IGNORE INTO pe_owner [ … ] RETURN NONE;
//! ```
//!
//! **`pe` 行为什么不删、改成一条批量的 `INSERT … ON DUPLICATE KEY UPDATE`**（2026-09-06 实测，
//! `direct_window_replay_parity::production_window_replay_converges_from_every_crash_point`）：`pe` 是图上的
//! **顶点**——`pe_owner` / `ref_rev` / `tubi_relate` / 房间边都以它为端点。SurrealDB 删一个顶点记录时把
//! 挂在它身上的边一并删掉；「先删 `pe:X` 再插」在同一事务里没事，可 X 的 owner 边是由 **owner 的块**
//! 重插的、X 的 `ref_rev` 入边由**别的 referrer** 的窗口维护、`tubi_relate` 由**模型面**写——它们不在
//! X 的块里，X 的 `pe` 一删，这些边就静默消失，重生成之前 Plant UI 看不到那根管。所以 `pe` 行按
//! **同一 id 原地整行替换**（列表列出 `SPdmsElement::gen_sur_json` 的全部字段，语义 = 整行覆盖），
//! 一块一条批量语句，仍然没有逐元素的 UPSERT / UPDATE；墓碑走同一条（`deleted = true`）。
//! 其余三类行（noun 行、UDA 行、边）没有边挂着，照旧先删后插。
//!
//! **跨元素的唯一一笔写**是 `Add` 的「自己的旧出向边」清理（refno 复用：`DELETE pe:A->pe_owner`）
//! 与墓碑的同款——它动的是**别的元素**（旧 owner）名下的边。新 owner 在同一窗口里是 Modified、会把 A
//! 插进自己的成员边；要是新 owner 的块先跑、A 的清理后跑，就把刚插的新边删了。所以这一笔单独成
//! **前置块**，排在所有元素块之前（今天 `render_terminal_persist_statements` 把全部清理排在最前面，
//! 是同一条纪律）。其余删除只动元素自己的行，块与块之间没有交集。
//!
//! 逐元素的 UPSERT / UPDATE 从这里退出；读透形态刷新缓存行的 `ElementRows::read_through_statements`
//! 不是 rocksdb 口径，不在本模块。**本模块生产路径全文不含 `UPSERT ` 与逐记录的 `UPDATE <表>:`**，源码钉守着。

use std::collections::{BTreeMap, HashMap};

use aios_core::RefU64;

use crate::data_interface::e3d_collector::RetiredTypeRow;
use crate::data_interface::ele_ops::{
    ElementRows, PersistElementChange, PersistElementState, UdaKeyMap,
};

/// `pe` 行原地替换时逐列覆盖的字段——与 `SPdmsElement::gen_sur_json` 写出的字段一一对应
/// （`name` / `noun` / `dbnum` / `sesno` / `status_code` / `cata_hash` / `lock` / `deleted` / `refno` / `owner`）。
/// 这里少一列，那一列在增量里就永远停在旧值。
pub const PE_REPLACE_ASSIGNMENTS: &str = "name = $input.name, noun = $input.noun, dbnum = $input.dbnum, \
     sesno = $input.sesno, status_code = $input.status_code, cata_hash = $input.cata_hash, \
     lock = $input.lock, deleted = $input.deleted, refno = $input.refno, owner = $input.owner, \
     dependency = $input.dependency";

/// 写入口径：**没有逐元素的 UPSERT / UPDATE**（与 `model_db_adapter::WriteMode` 同名同义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// 库里还没有这一批：只插，撞上已有 id 就跳过（基线「已有行赢」）。
    Fresh,
    /// 先按 id 把这些元素的整组表示删干净，再插。
    Replace,
}

/// 一个待落库的元素。
pub enum BatchItem {
    /// Add / Modified：完整行。`is_add` 决定要不要清它自己的旧出向边（refno 复用）；
    /// `old_noun` 是窗口内换了类型的元素在旧表里的行（一并删）；`dependency` 标这是按 DESI
    /// 依赖入库的 CATA 元素（`pe.dependency = true`，plan §3 D6），不代表该库整库在场。
    Rows {
        rows: ElementRows,
        is_add: bool,
        old_noun: Option<String>,
        dependency: bool,
    },
    /// Deleted：墓碑。`base` 是基侧状态渲成的行（拿它的 noun 与 pe 行），`None` 就退化成最小墓碑。
    Tombstone {
        id: String,
        sesno: u32,
        base: Option<ElementRows>,
    },
}

impl BatchItem {
    fn id(&self) -> &str {
        match self {
            Self::Rows { rows, .. } => rows.id.as_str(),
            Self::Tombstone { id, .. } => id.as_str(),
        }
    }

    /// 要在删除块里清「自己的旧出向边」的项：Add（refno 复用）与墓碑（它在 owner 名下的边）。
    fn clears_own_owner_edge(&self) -> bool {
        match self {
            Self::Rows { is_add, .. } => *is_add,
            Self::Tombstone { .. } => true,
        }
    }
}

/// 一块渲染好的批：不含 `BEGIN / COMMIT`，也不含 pe 统计观测（那是 `dbnum_stats` 的事）。
#[derive(Debug, Clone, Default)]
pub struct RenderedElementBatch {
    /// 这一块碰到的全部 `pe` id（`ref0_ref1`），给 `dbnum_stats` 做前后观测。
    pub ids: Vec<String>,
    /// 删除块 + 插入块。
    pub sql: String,
    pub elements: usize,
    pub tombstones: usize,
    /// 基侧读不到、退化成最小墓碑的条数。> 0 要在回执里出声。
    pub tombstones_without_base: usize,
}

/// 从终态变更集合成批项。`retired` 是窗口内换了 noun 的元素（旧表行随删除块一起清）。
pub fn items_from_changes(
    changes: &[PersistElementChange],
    retired: &[RetiredTypeRow],
    dbnum: i32,
    uda_keys: &UdaKeyMap,
) -> Vec<BatchItem> {
    let old_nouns: HashMap<RefU64, &str> = retired
        .iter()
        .map(|row| (row.refno, row.old_noun.as_str()))
        .collect();
    changes
        .iter()
        .map(|change| {
            let id = change.id();
            match &change.state {
                PersistElementState::Add(data) | PersistElementState::Modified(data) => {
                    BatchItem::Rows {
                        rows: ElementRows::render(data, &id, dbnum, change.sesno, uda_keys),
                        is_add: change.is_add(),
                        old_noun: old_nouns.get(&change.refno).map(|noun| noun.to_string()),
                        dependency: false,
                    }
                }
                PersistElementState::Deleted { base } => BatchItem::Tombstone {
                    base: base
                        .as_ref()
                        .map(|base| ElementRows::render(base, &id, dbnum, change.sesno, uda_keys)),
                    id,
                    sesno: change.sesno,
                },
            }
        })
        .collect()
}

/// 前置块：`Add` / 墓碑「自己的旧出向边」清理（模块头「跨元素的唯一一笔写」）。`Fresh` 没有。
/// 空串 = 这一窗没有需要前置清理的元素。
pub fn render_preamble(items: &[BatchItem], mode: WriteMode) -> String {
    if mode == WriteMode::Fresh {
        return String::new();
    }
    let mut sql = String::new();
    for item in items.iter().filter(|item| item.clears_own_owner_edge()) {
        sql.push_str(&format!("DELETE pe:{}->pe_owner;\n", item.id()));
    }
    sql
}

/// 一块的删除块 + 插入块。
pub fn render_items(items: &[BatchItem], dbnum: i32, mode: WriteMode) -> RenderedElementBatch {
    let mut out = RenderedElementBatch {
        elements: items.len(),
        ..Default::default()
    };
    let mut sql = String::new();

    if mode == WriteMode::Replace {
        // 被引用的最后删，引用者先删：边 → UDA → noun 行 → pe。
        for item in items {
            sql.push_str(&format!(
                "DELETE pe_owner:[pe:{0}, NONE]..=[pe:{0}, ..];\n",
                item.id()
            ));
        }
        push_array_delete(
            &mut sql,
            "uda_ids",
            items.iter().map(|item| format!("ATT_UDA:{}", item.id())),
        );
        // noun 行按表分组：Rows 的当前 noun、换过类型的旧 noun、墓碑的基侧 noun。
        let mut by_noun: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for item in items {
            match item {
                BatchItem::Rows { rows, old_noun, .. } => {
                    if !rows.noun.is_empty() {
                        by_noun
                            .entry(rows.noun.clone())
                            .or_default()
                            .push(rows.id.clone());
                    }
                    if let Some(old) = old_noun.as_deref().filter(|old| !old.is_empty()) {
                        by_noun
                            .entry(old.to_string())
                            .or_default()
                            .push(rows.id.clone());
                    }
                }
                BatchItem::Tombstone {
                    id,
                    base: Some(base),
                    ..
                } if !base.noun.is_empty() => {
                    by_noun
                        .entry(base.noun.clone())
                        .or_default()
                        .push(id.clone());
                }
                BatchItem::Tombstone { .. } => {}
            }
        }
        for (index, (noun, ids)) in by_noun.iter().enumerate() {
            push_array_delete(
                &mut sql,
                &format!("del_n{index}"),
                ids.iter()
                    .map(|id| format!("type::thing({}, {})", sql_string(noun), sql_string(id))),
            );
        }
        // `pe` 行不删（模块头「pe 行为什么不删」）：它是图顶点，删了会把别的块 / 模型面挂在它身上的边一起带走。
    }

    // 插入块：被引用的先插，引用者后插：pe → noun 行 → UDA → 边。
    let mut pe_rows = Vec::with_capacity(items.len());
    let mut noun_rows: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut uda_rows = Vec::new();
    let mut edges = Vec::new();
    for item in items {
        out.ids.push(item.id().to_string());
        match item {
            BatchItem::Rows {
                rows, dependency, ..
            } => {
                pe_rows.push(if *dependency {
                    flag_dependency(&rows.pe_row_with_id())
                } else {
                    rows.pe_row_with_id()
                });
                if !rows.noun.is_empty() && !rows.noun_row.is_empty() {
                    noun_rows
                        .entry(rows.noun.clone())
                        .or_default()
                        .push(rows.noun_row.clone());
                }
                if let Some(uda) = rows.uda_row.as_ref() {
                    uda_rows.push(uda.clone());
                }
                edges.extend(rows.owner_edges.iter().cloned());
            }
            BatchItem::Tombstone { id, sesno, base } => {
                out.tombstones += 1;
                match base {
                    Some(base) => pe_rows.push(base.tombstone_row_with_id(*sesno)),
                    None => {
                        out.tombstones_without_base += 1;
                        pe_rows.push(ElementRows::minimal_tombstone_row(id, dbnum, *sesno));
                    }
                }
            }
        }
    }
    if !pe_rows.is_empty() {
        match mode {
            WriteMode::Fresh => sql.push_str(&format!(
                "INSERT IGNORE INTO pe [{}] RETURN NONE;\n",
                pe_rows.join(",")
            )),
            // 同一 id 原地整行替换：字段清单 = `SPdmsElement::gen_sur_json` 写出的全部字段，
            // 少列一个就是那一列静默保留旧值。
            WriteMode::Replace => sql.push_str(&format!(
                "INSERT INTO pe [{}] ON DUPLICATE KEY UPDATE {} RETURN NONE;\n",
                pe_rows.join(","),
                PE_REPLACE_ASSIGNMENTS
            )),
        }
    }
    for (noun, rows) in noun_rows {
        sql.push_str(&format!(
            "INSERT IGNORE INTO {noun} [{}] RETURN NONE;\n",
            rows.join(",")
        ));
    }
    if !uda_rows.is_empty() {
        sql.push_str(&format!(
            "INSERT IGNORE INTO ATT_UDA [{}] RETURN NONE;\n",
            uda_rows.join(",")
        ));
    }
    if !edges.is_empty() {
        sql.push_str(&format!(
            "INSERT RELATION IGNORE INTO pe_owner [{}] RETURN NONE;\n",
            edges.join(",")
        ));
    }
    out.sql = sql;
    out
}

/// 给 `pe` 行 JSON 加上 `"dependency": true`（紧跟开头的 `{`）。`SPdmsElement::gen_sur_json` 的对象以
/// `{` 起头、字段逐行——在它后面插一个字段不碰别的字段。
fn flag_dependency(pe_row: &str) -> String {
    match pe_row.find('{') {
        Some(brace) => format!(
            "{}{{\n  \"dependency\": true,{}",
            &pe_row[..brace],
            &pe_row[brace + 1..]
        ),
        None => pe_row.to_string(),
    }
}

fn push_array_delete(sql: &mut String, variable: &str, ids: impl Iterator<Item = String>) {
    let ids = ids.collect::<Vec<_>>();
    if ids.is_empty() {
        return;
    }
    sql.push_str(&format!(
        "LET ${variable} = [{}];\nDELETE ${variable};\n",
        ids.join(", ")
    ));
}

/// SurrealQL 字符串字面量（单引号，`\` 与 `'` 转义）——与 `dbnum_state::escape_surql_str` 同口径。
fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// 一次落库的账（给回执 / 日志）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElementBatchReport {
    pub transactions: usize,
    pub elements: usize,
    pub tombstones: usize,
    pub tombstones_without_base: usize,
    pub max_transaction_bytes: usize,
}

impl ElementBatchReport {
    /// 退化墓碑要在回执里点名。
    pub fn warning(&self, dbnum: u32) -> Option<String> {
        (self.tombstones_without_base > 0).then(|| {
            format!(
                "dbnum={dbnum}: {} 个被删元素在窗口基侧读不到，墓碑只带 id / dbnum / sesno / deleted，\
                 没有 noun / owner（交付记录标记与成员补删会把它们当链断跳过）",
                self.tombstones_without_base
            )
        })
    }
}

/// 整窗口 → 若干笔事务 SQL（含 `BEGIN / COMMIT` 与 pe 统计观测）。
///
/// 顺序：前置块（Add / 墓碑的旧出向边清理）→ 元素块 × N。每块 ≤ `tx_limits.elements` 个元素，
/// 再过 `pe_batch::transaction` 的字节 / 行上限，不合就劈半——与模型侧 `render_chunk`、
/// `pe_batch::batches` 同一个形状。
pub fn render_transactions(
    changes: &[PersistElementChange],
    retired: &[RetiredTypeRow],
    dbnum: i32,
    uda_keys: &UdaKeyMap,
    mode: WriteMode,
) -> anyhow::Result<(Vec<String>, ElementBatchReport)> {
    render_transactions_with_limits(
        changes,
        retired,
        dbnum,
        uda_keys,
        mode,
        crate::data_interface::tx_limits::data_tx_limits(),
    )
}

/// [`render_transactions`] 的显式上限形态（崩溃重放对拍把元素上限压到 1，让每个元素自成一块）。
pub fn render_transactions_with_limits(
    changes: &[PersistElementChange],
    retired: &[RetiredTypeRow],
    dbnum: i32,
    uda_keys: &UdaKeyMap,
    mode: WriteMode,
    limits: crate::data_interface::tx_limits::DataTxLimits,
) -> anyhow::Result<(Vec<String>, ElementBatchReport)> {
    let items = items_from_changes(changes, retired, dbnum, uda_keys);
    render_item_transactions_with_limits(&items, dbnum, mode, limits)
}

/// [`render_transactions`] 的已渲染行形态（基线把 `ElementRows` 直接喂进来）。
pub fn render_item_transactions(
    items: &[BatchItem],
    dbnum: i32,
    mode: WriteMode,
) -> anyhow::Result<(Vec<String>, ElementBatchReport)> {
    render_item_transactions_with_limits(
        items,
        dbnum,
        mode,
        crate::data_interface::tx_limits::data_tx_limits(),
    )
}

pub fn render_item_transactions_with_limits(
    items: &[BatchItem],
    dbnum: i32,
    mode: WriteMode,
    limits: crate::data_interface::tx_limits::DataTxLimits,
) -> anyhow::Result<(Vec<String>, ElementBatchReport)> {
    let mut transactions = Vec::new();
    let mut report = ElementBatchReport::default();

    let preamble = render_preamble(items, mode);
    if !preamble.is_empty() {
        // 前置块只删边、不碰 pe 行：不带 id。字节超限就按行劈半。
        push_preamble_transactions(dbnum as u32, &preamble, &mut transactions)?;
        report.transactions = transactions.len();
    }

    // 事务只包 `BEGIN / COMMIT` 与上限校验（ADR-071 D6：逐笔 pe 统计观测块已退，`dbnum_info_table`
    // 由任务收尾从目标索引序列化）；`ids` 只交去校验规范性。此前这里按 `StatisticsMode` 分
    // 「逐笔观测 / 基线延后」两档，两档合一之后那个枚举随 S7 删了。
    fn append(
        items: &[BatchItem],
        dbnum: i32,
        mode: WriteMode,
        out: &mut Vec<String>,
        report: &mut ElementBatchReport,
    ) -> anyhow::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let rendered = render_items(items, dbnum, mode);
        match crate::data_interface::pe_batch::transaction(dbnum as u32, &rendered.ids, &rendered.sql)
        {
            Ok(sql) => {
                report.max_transaction_bytes = report.max_transaction_bytes.max(sql.len());
                report.transactions += 1;
                report.elements += rendered.elements;
                report.tombstones += rendered.tombstones;
                report.tombstones_without_base += rendered.tombstones_without_base;
                out.push(sql);
                Ok(())
            }
            Err(_) if items.len() > 1 => {
                let middle = items.len() / 2;
                append(&items[..middle], dbnum, mode, out, report)?;
                append(&items[middle..], dbnum, mode, out, report)
            }
            Err(error) => Err(error.context(format!(
                "single element {} exceeds the data transaction limits",
                items[0].id()
            ))),
        }
    }

    for chunk in items.chunks(limits.elements.max(1)) {
        append(chunk, dbnum, mode, &mut transactions, &mut report)?;
    }
    Ok((transactions, report))
}

/// 前置块（一行一条 `DELETE …->pe_owner;`，不带元素 id）按字节 / 行上限切成事务，超限按行劈半。
fn push_preamble_transactions(
    dbnum: u32,
    writes: &str,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    match crate::data_interface::pe_batch::transaction(dbnum, &[], writes) {
        Ok(sql) => {
            out.push(sql);
            Ok(())
        }
        Err(error) => {
            let lines = writes.lines().collect::<Vec<_>>();
            if lines.len() <= 1 {
                return Err(error.context("preamble statement exceeds the data transaction limits"));
            }
            let middle = lines.len() / 2;
            let left = lines[..middle].join("\n") + "\n";
            let right = lines[middle..].join("\n") + "\n";
            push_preamble_transactions(dbnum, &left, out)?;
            push_preamble_transactions(dbnum, &right, out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::ele_data::EleData;
    use aios_core::{NamedAttrValue, RefU64Vec};

    const DBNUM: i32 = 8000;

    fn refu(n: u64) -> RefU64 {
        RefU64((24384u64 << 32) | n)
    }

    fn element(
        refno: RefU64,
        owner: RefU64,
        noun: &str,
        name: &str,
        children: &[RefU64],
    ) -> EleData {
        let mut ele = EleData::default();
        ele.refno = refno;
        ele.owner = owner;
        ele.children = RefU64Vec(children.to_vec());
        let map = &mut ele.whole_attmap.attmap.map;
        map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        map.insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
        map.insert("TYPE".into(), NamedAttrValue::StringType(noun.into()));
        map.insert("NAME".into(), NamedAttrValue::StringType(name.into()));
        map.insert("DBNUM".into(), NamedAttrValue::IntegerType(DBNUM));
        ele
    }

    fn change(refno: RefU64, sesno: u32, state: PersistElementState) -> PersistElementChange {
        PersistElementChange {
            refno,
            sesno,
            state,
        }
    }

    fn fixture() -> Vec<PersistElementChange> {
        let (zone, bran, equi, gone) = (refu(1), refu(2), refu(3), refu(4));
        vec![
            change(
                zone,
                7,
                PersistElementState::Modified(element(zone, refu(0), "ZONE", "/Z", &[bran, equi])),
            ),
            change(
                bran,
                7,
                PersistElementState::Add(element(bran, zone, "BRAN", "/B", &[])),
            ),
            change(
                equi,
                7,
                PersistElementState::Modified(element(equi, zone, "EQUI", "/E", &[])),
            ),
            change(
                gone,
                7,
                PersistElementState::Deleted {
                    base: Some(element(gone, zone, "BOX", "/GONE", &[])),
                },
            ),
        ]
    }

    #[test]
    fn replace_block_deletes_the_whole_group_then_inserts_one_statement_per_table() {
        let items = items_from_changes(&fixture(), &[], DBNUM, &UdaKeyMap::empty());
        let rendered = render_items(&items, DBNUM, WriteMode::Replace);
        let sql = &rendered.sql;
        let at = |needle: &str| {
            sql.find(needle)
                .unwrap_or_else(|| panic!("缺 {needle}\n{sql}"))
        };

        // 删除块：边 → UDA → noun，全部在插入块之前；`pe` 行不删（图顶点，见模块头）。
        let slots = at("DELETE pe_owner:[pe:24384_1, NONE]..=[pe:24384_1, ..];");
        let uda = at(
            "LET $uda_ids = [ATT_UDA:24384_1, ATT_UDA:24384_2, ATT_UDA:24384_3, ATT_UDA:24384_4];",
        );
        let noun_del = at("type::thing('BRAN', '24384_2')");
        let pe_ins = at("INSERT INTO pe [");
        assert!(
            slots < uda && uda < noun_del && noun_del < pe_ins,
            "顺序：边 → UDA → noun 删 → 插\n{sql}"
        );
        assert!(
            !sql.contains("DELETE $pe_ids"),
            "pe 是图顶点，不许删:\n{sql}"
        );
        assert!(!sql.contains("LET $pe_ids"), "{sql}");
        // 墓碑的基侧 noun（BOX）也在 noun 删除组里。
        assert!(sql.contains("type::thing('BOX', '24384_4')"), "{sql}");
        // pe：一条批量原地替换，字段清单齐全；其余每张表一条 INSERT IGNORE；边表带 RELATION。
        assert_eq!(sql.matches("INSERT INTO pe [").count(), 1);
        assert!(
            sql.contains(&format!(
                "] ON DUPLICATE KEY UPDATE {PE_REPLACE_ASSIGNMENTS} RETURN NONE;"
            )),
            "{sql}"
        );
        assert_eq!(sql.matches("INSERT IGNORE INTO ZONE [").count(), 1);
        assert_eq!(sql.matches("INSERT IGNORE INTO BRAN [").count(), 1);
        assert_eq!(sql.matches("INSERT IGNORE INTO EQUI [").count(), 1);
        assert_eq!(
            sql.matches("INSERT RELATION IGNORE INTO pe_owner [")
                .count(),
            1
        );
        // 墓碑行：deleted = true、noun / owner 保留、sesno 换成删除会话。
        assert!(sql.contains("\"deleted\": true"), "{sql}");
        assert!(sql.contains("\"noun\": \"BOX\""), "{sql}");
        assert_eq!(rendered.tombstones, 1);
        assert_eq!(rendered.tombstones_without_base, 0);
        assert_eq!(rendered.ids.len(), 4);
        // 生产纪律：没有逐元素的 UPSERT / UPDATE。
        assert!(!sql.contains("UPSERT "), "{sql}");
        assert!(!sql.contains("UPDATE pe:"), "{sql}");
    }

    /// `PE_REPLACE_ASSIGNMENTS` 必须覆盖 `SPdmsElement::gen_sur_json` 写出的每一个字段：
    /// 少一列，增量里那一列就永远停在旧值。
    #[test]
    fn pe_replace_assignments_cover_every_rendered_pe_field() {
        let items = items_from_changes(&fixture(), &[], DBNUM, &UdaKeyMap::empty());
        let BatchItem::Rows { rows, .. } = &items[0] else {
            panic!("first item is a Modified row")
        };
        // pe 行 JSON 里 record id 是裸的（`pe:…` / `ZONE:…`），不走 serde；扫 `"<key>":` 取字段名即可。
        let text = rows.pe_row_with_id();
        let mut fields = std::collections::BTreeSet::new();
        let mut rest = text.as_str();
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else { break };
            let key = &after[..end];
            let tail = &after[end + 1..];
            if tail.starts_with(':') {
                fields.insert(key.to_string());
            }
            rest = tail;
        }
        assert!(fields.len() >= 10, "pe 行字段扫描失灵: {fields:?}\n{text}");
        for field in fields.iter().filter(|key| *key != "id") {
            assert!(
                PE_REPLACE_ASSIGNMENTS.contains(&format!("{field} = $input.{field}")),
                "PE_REPLACE_ASSIGNMENTS 缺字段 {field}"
            );
        }
    }

    #[test]
    fn fresh_block_has_no_deletes_and_the_preamble_is_empty() {
        let items = items_from_changes(&fixture(), &[], DBNUM, &UdaKeyMap::empty());
        assert!(render_preamble(&items, WriteMode::Fresh).is_empty());
        let rendered = render_items(&items, DBNUM, WriteMode::Fresh);
        assert!(!rendered.sql.contains("DELETE"), "{}", rendered.sql);
        assert!(rendered.sql.starts_with("INSERT IGNORE INTO pe ["));
        assert!(
            !rendered.sql.contains("ON DUPLICATE KEY"),
            "基线只插、已有行赢"
        );
    }

    #[test]
    fn preamble_clears_own_owner_edge_only_for_adds_and_tombstones() {
        let items = items_from_changes(&fixture(), &[], DBNUM, &UdaKeyMap::empty());
        let preamble = render_preamble(&items, WriteMode::Replace);
        assert!(
            preamble.contains("DELETE pe:24384_2->pe_owner;"),
            "Add 清自己的旧出向边"
        );
        assert!(
            preamble.contains("DELETE pe:24384_4->pe_owner;"),
            "墓碑清自己在 owner 名下的边"
        );
        assert!(
            !preamble.contains("pe:24384_1->"),
            "Modified 不清自己的出向边（边由 owner 侧重插）"
        );
        assert!(!preamble.contains("pe:24384_3->"));
    }

    #[test]
    fn retired_noun_rows_join_the_delete_block() {
        let retired = vec![RetiredTypeRow {
            refno: refu(2),
            old_noun: "NOZZ".into(),
            new_noun: "BRAN".into(),
        }];
        let items = items_from_changes(&fixture(), &retired, DBNUM, &UdaKeyMap::empty());
        let rendered = render_items(&items, DBNUM, WriteMode::Replace);
        assert!(
            rendered.sql.contains("type::thing('NOZZ', '24384_2')"),
            "{}",
            rendered.sql
        );
    }

    #[test]
    fn a_tombstone_without_base_degrades_to_the_minimal_row_and_is_counted() {
        let changes = vec![change(
            refu(9),
            7,
            PersistElementState::Deleted { base: None },
        )];
        let items = items_from_changes(&changes, &[], DBNUM, &UdaKeyMap::empty());
        let rendered = render_items(&items, DBNUM, WriteMode::Replace);
        assert_eq!(rendered.tombstones_without_base, 1);
        assert!(
            rendered
                .sql
                .contains("\"id\": pe:24384_9, \"dbnum\": 8000, \"sesno\": 7, \"deleted\": true"),
            "{}",
            rendered.sql
        );
        assert!(
            !rendered.sql.contains("del_n"),
            "不知道 noun 就没有 noun 行可删"
        );
        let report = ElementBatchReport {
            tombstones_without_base: 1,
            ..Default::default()
        };
        assert!(report.warning(8000).is_some());
    }

    #[test]
    fn transactions_start_with_the_preamble_and_wrap_every_block_in_statistics() {
        let (txs, report) = render_transactions(
            &fixture(),
            &[],
            DBNUM,
            &UdaKeyMap::empty(),
            WriteMode::Replace,
        )
        .unwrap();
        assert_eq!(report.elements, 4);
        assert_eq!(report.tombstones, 1);
        assert!(txs.len() >= 2, "前置块 + 至少一个元素块: {}", txs.len());
        assert!(txs[0].contains("DELETE pe:24384_2->pe_owner;"));
        assert!(!txs[0].contains("INSERT"), "前置块只删边");
        for tx in &txs {
            assert!(tx.starts_with("BEGIN TRANSACTION;"));
            assert!(tx.trim_end().ends_with("COMMIT TRANSACTION;"));
        }
        // 观测块已退（ADR-071 D6）：元素块只有正文，统计由任务收尾从目标索引序列化。
        assert!(
            !txs[1].contains("dbnum_stats_state") && !txs[1].contains("dbnum_info_table"),
            "元素块不得再带 pe 统计观测"
        );
    }

    /// 元素事务只包 `BEGIN / COMMIT`、过字节 / 行上限，没有观测块（不点读 `$before` / `$after`、
    /// 不碰 `dbnum_info_table` / `dbnum_stats_state`）——ADR-071 D6；`StatisticsMode` 那个两档枚举随之删了。
    #[test]
    fn element_transactions_carry_no_statistics_observation_block() {
        let items = items_from_changes(&fixture(), &[], DBNUM, &UdaKeyMap::empty());
        let limits = crate::data_interface::tx_limits::DataTxLimits::default();
        let (deferred, deferred_report) =
            render_item_transactions_with_limits(&items, DBNUM, WriteMode::Fresh, limits).unwrap();
        assert_eq!(deferred.len(), 1);
        assert!(
            !include_str!("element_batch.rs")
                .split_once("#[cfg(test)]")
                .expect("tests follow")
                .0
                .contains("enum StatisticsMode"),
            "两档口径的枚举已删（ADR-071 D6 / S7）"
        );
        for marker in [
            "dbnum_stats_state",
            "dbnum_info_table",
            "$before_",
            "$after_",
            "FOR $row",
        ] {
            assert!(
                !deferred[0].contains(marker),
                "不得渲观测块（{marker}）:\n{}",
                deferred[0]
            );
        }
        assert!(deferred[0].starts_with("BEGIN TRANSACTION;\n"));
        assert!(deferred[0].trim_end().ends_with("COMMIT TRANSACTION;"));
        // 正文一字不差：事务只是包在外面的 BEGIN / COMMIT。
        let body = render_items(&items, DBNUM, WriteMode::Fresh).sql;
        assert!(deferred[0].contains(&body));
        assert_eq!(deferred_report.max_transaction_bytes, deferred[0].len());
    }

    /// 本模块是 rocksdb 数据面**唯一**的写法出口：生产代码里不许有 UPSERT，也不许有逐记录的
    /// `UPDATE <表>:<id>`；唯一放行的 UPDATE 是 `pe` 那条批量 `INSERT … ON DUPLICATE KEY UPDATE`
    /// （模块头「pe 行为什么不删」）。
    #[test]
    fn production_code_renders_no_upsert_or_per_record_update() {
        let source = include_str!("element_batch.rs");
        let production = source.split_once("#[cfg(test)]").expect("tests follow").0;
        let mut duplicate_key_updates = 0;
        for line in production.lines() {
            let code = line.split("//").next().unwrap_or("");
            assert!(
                !code.contains("UPSERT "),
                "element_batch.rs 生产代码不得渲染 UPSERT: {line}"
            );
            if code.contains("ON DUPLICATE KEY UPDATE") {
                duplicate_key_updates += 1;
                continue;
            }
            assert!(
                !code.contains("UPDATE "),
                "element_batch.rs 生产代码不得渲染逐记录 UPDATE: {line}"
            );
        }
        assert_eq!(duplicate_key_updates, 1, "pe 的批量原地替换只出现一处");
    }
}
