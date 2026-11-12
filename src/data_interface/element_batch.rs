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
//! **一个元素自己就超过一笔事务的上限**（gen-model #155）：元素的行里只有成员边（`pe_owner`，一名成员一行）会随
//! 数据无限变长——目录库里一个 `CATE` 名下上万个 `SCOM`、一个 `SECT` 名下两万多个成员都是真数据，而依赖闭包把
//! 用到的目录件的属主链整条带进来，每个属主带着自己完整的成员表。切块上限（`tx_limits`）是**怎么切**的依据，不是
//! 拒收数据的理由：块先按元素劈半；劈到只剩一个元素还放不下，就把它的成员边切出去——头块带它自己的全部行
//! （`Replace` 的删除块在内）和装得下的前一段边，余下的边照槽位顺序一段一笔续在后面（[`append_oversize_element`]）。
//! **续块排在头块之后**，这是事务之间唯一的先后依赖：`Replace` 的头块先把旧的子槽位整段删掉，续块才往里插。
//! 执行口都照返回的顺序逐笔执行；重放从头再来一遍，终态相同。
//!
//! 逐元素的 UPSERT / UPDATE 从这里退出；读透形态刷新缓存行的 `ElementRows::read_through_statements`
//! 不是 rocksdb 口径，不在本模块。**本模块生产路径全文不含 `UPSERT ` 与逐记录的 `UPDATE <表>:`**，源码钉守着。

use std::collections::{BTreeMap, HashMap};

use aios_core::RefU64;

use crate::data_interface::e3d_collector::RetiredTypeRow;
use crate::data_interface::ele_ops::{
    ElementRows, PersistElementChange, PersistElementState, UdaKeyMap,
};
use crate::data_interface::pe_batch;
use crate::data_interface::tx_limits::DataTxLimits;

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

/// 一块渲染好的批：不含 `BEGIN / COMMIT`（那是 `pe_batch` 的事）。
#[derive(Debug, Clone, Default)]
pub struct RenderedElementBatch {
    /// 这一块碰到的全部 `pe` id（`ref0_ref1`），只交去校验规范性。
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
    render_items_with_edge_cap(items, dbnum, mode, usize::MAX)
}

/// 同 [`render_items`]，但每个元素只带打头的 `edge_cap` 条成员边——超限元素的头块用，余下的边由续块补上
/// （[`append_oversize_element`]）。别的行一行不少。
fn render_items_with_edge_cap(
    items: &[BatchItem],
    dbnum: i32,
    mode: WriteMode,
    edge_cap: usize,
) -> RenderedElementBatch {
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
                        .push(stamp_owner_columns(&rows.noun_row, dbnum, rows.sesno()));
                }
                if let Some(uda) = rows.uda_row.as_ref() {
                    uda_rows.push(stamp_owner_columns(uda, dbnum, rows.sesno()));
                }
                edges.extend(rows.owner_edges.iter().take(edge_cap).cloned());
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
        sql.push_str(&owner_edge_insert(&edges));
    }
    out.sql = sql;
    out
}

const OWNER_EDGE_INSERT_HEAD: &str = "INSERT RELATION IGNORE INTO pe_owner [";
const OWNER_EDGE_INSERT_TAIL: &str = "] RETURN NONE;\n";

/// 成员边那一条语句。整块里的（[`render_items`]）与超限元素分段写的（[`append_oversize_element`]）是同一种写法。
fn owner_edge_insert(edges: &[String]) -> String {
    format!(
        "{OWNER_EDGE_INSERT_HEAD}{}{OWNER_EDGE_INSERT_TAIL}",
        edges.join(",")
    )
}

/// 给 noun 行 / `ATT_UDA` 行 JSON 补上 `dbnum` / `sesno` 两列（计划 §9-6）。
///
/// `pe` 行本来就有这两列（`SPdmsElement::gen_sur_json`），noun 行与 UDA 行没有——它们的内容
/// 来自属性映射（`NamedAttrMap::gen_sur_json` / `gen_sur_json_uda`），里面只有大写的 E3D
/// 属性名。而回退到水位靠的正是按 `(dbnum, sesno)` 的谓词 `DELETE`
/// （[`fast_delete::render_rollback`](crate::data_interface::fast_delete)）：少这两列的表删不动，
/// 只能整库 wipe 才退得干净。
///
/// 与 [`flag_dependency`] 同一种插法：对象以 `{` 起头、字段逐行，在它后面插两个字段不碰
/// 别的字段。小写也是刻意的——属性名一律大写（`DBNUM` / `TYPE`），撞不上。
///
/// **只覆盖本次改造之后写下的行**：历史行没有这两列，回退时按 `RollbackReport::legacy_rows_note`
/// 点名，不装作删干净了。
pub(crate) fn stamp_owner_columns(row: &str, dbnum: i32, sesno: u32) -> String {
    match row.find('{') {
        Some(brace) => format!(
            "{}{{ \"dbnum\": {dbnum}, \"sesno\": {sesno},{}",
            &row[..brace],
            &row[brace + 1..]
        ),
        None => row.to_string(),
    }
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
    /// 自己就超过一笔事务的上限、成员边分几笔写的元素（[`append_oversize_element`]）。
    pub split_elements: Vec<SplitElement>,
}

/// 一个成员边分笔写的元素：它名下多少成员、一共占了几笔事务（头块 + 续块）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitElement {
    pub id: String,
    pub members: usize,
    pub transactions: usize,
}

impl ElementBatchReport {
    /// 有元素的成员边分笔写时给日志的一句话。这是正常落库，不进回执的 warnings。
    pub fn split_notice(&self, dbnum: u32) -> Option<String> {
        let largest = self
            .split_elements
            .iter()
            .max_by_key(|split| split.members)?;
        Some(format!(
            "dbnum={dbnum}: {} 个元素名下的成员一笔事务装不下，成员边分笔写入（最多的 {}：{} 名成员，分 {} 笔）",
            self.split_elements.len(),
            largest.id,
            largest.members,
            largest.transactions
        ))
    }

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

/// 整窗口 → 若干笔事务 SQL（含 `BEGIN / COMMIT`）。
///
/// 顺序：前置块（Add / 墓碑的旧出向边清理）→ 元素块 × N。每块 ≤ `tx_limits.elements` 个元素，
/// 再过 `pe_batch::transaction_with_limits` 的字节 / 行上限，不合就劈半——与模型侧 `render_chunk`、
/// `pe_batch::batches` 同一个形状；劈到只剩一个元素还不合，把它的成员边分几笔写
/// （[`append_oversize_element`]）。**返回的事务要照顺序逐笔执行**。
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

/// 三个上限都照 `limits` 走（此前字节 / 行两项不看这个参数，一律读进程配置；生产传的就是进程配置，
/// 结果相同，只是显式形态现在名副其实）。
pub fn render_item_transactions_with_limits(
    items: &[BatchItem],
    dbnum: i32,
    mode: WriteMode,
    limits: DataTxLimits,
) -> anyhow::Result<(Vec<String>, ElementBatchReport)> {
    let mut transactions = Vec::new();
    let mut report = ElementBatchReport::default();

    let preamble = render_preamble(items, mode);
    if !preamble.is_empty() {
        // 前置块只删边、不碰 pe 行：不带 id。字节超限就按行劈半。
        push_preamble_transactions(dbnum as u32, &preamble, limits, &mut transactions)?;
        report.transactions = transactions.len();
    }

    // 事务只包 `BEGIN / COMMIT` 与上限校验（ADR-071 D6：逐笔 pe 统计观测块已退，Ref0 统计随后
    // 整体退役）；`ids` 只交去校验规范性。此前这里按 `StatisticsMode` 分
    // 「逐笔观测 / 基线延后」两档，两档合一之后那个枚举随 S7 删了。
    fn append(
        items: &[BatchItem],
        dbnum: i32,
        mode: WriteMode,
        limits: DataTxLimits,
        out: &mut Vec<String>,
        report: &mut ElementBatchReport,
    ) -> anyhow::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        // 光数成员边就知道放不下的块不渲：渲出来要过上限得把整段正文解析一遍数行，几兆正文在调试版里是几秒，
        // 而一个上万成员的属主会让它所在的每一层劈半都白解析一遍。
        let failure = if certainly_exceeds(items, limits) {
            None
        } else {
            let rendered = render_items(items, dbnum, mode);
            match pe_batch::transaction_with_limits(
                dbnum as u32,
                &rendered.ids,
                &rendered.sql,
                limits,
            ) {
                Ok(sql) => {
                    report.max_transaction_bytes = report.max_transaction_bytes.max(sql.len());
                    report.transactions += 1;
                    report.elements += rendered.elements;
                    report.tombstones += rendered.tombstones;
                    report.tombstones_without_base += rendered.tombstones_without_base;
                    out.push(sql);
                    return Ok(());
                }
                Err(error) => Some(error),
            }
        };
        if items.len() > 1 {
            let middle = items.len() / 2;
            append(&items[..middle], dbnum, mode, limits, out, report)?;
            return append(&items[middle..], dbnum, mode, limits, out, report);
        }
        match failure {
            // 不是大小的事（id 不规范、正文解析不了）：点名是哪个元素，原样上报。
            Some(error) if !error.is::<pe_batch::ExceedsTxLimits>() => Err(error.context(format!(
                "render the transaction of element {}",
                items[0].id()
            ))),
            // 只剩一个元素还是超限：能切的只有它的成员边。
            whole => append_oversize_element(&items[0], dbnum, mode, limits, whole, out, report),
        }
    }

    for chunk in items.chunks(limits.elements.max(1)) {
        append(chunk, dbnum, mode, limits, &mut transactions, &mut report)?;
    }
    Ok((transactions, report))
}

/// 不渲就知道这一块过不了上限：每个元素至少一行 `pe`、每条成员边一行，边的字节也都在正文里——
/// 这个下界过了上限，整块一定过。只用来少做无用功，判「放得下」仍以渲出来的正文为准。
fn certainly_exceeds(items: &[BatchItem], limits: DataTxLimits) -> bool {
    let (mut rows, mut bytes) = (0u64, 0usize);
    for item in items {
        rows += 1;
        if let BatchItem::Rows { rows: element, .. } = item {
            rows += element.owner_edges.len() as u64;
            bytes += element.owner_edges.iter().map(String::len).sum::<usize>();
        }
        if rows > limits.max_write_rows || bytes > limits.max_bytes {
            return true;
        }
    }
    false
}

/// 一笔事务里还装得下多少：写行与字节。
#[derive(Clone, Copy)]
struct Room {
    rows: u64,
    bytes: usize,
}

/// `edges` 打头的多少条装得进 `room`：一条边一行，字节是它自己再加一个分隔的逗号（头一条没有）。
fn edges_that_fit(edges: &[String], room: Room) -> usize {
    let mut bytes = 0usize;
    let mut count = 0usize;
    for edge in edges {
        let with_this = bytes + edge.len() + usize::from(count > 0);
        if count as u64 >= room.rows || with_this > room.bytes {
            break;
        }
        bytes = with_this;
        count += 1;
    }
    count
}

/// 一个元素自己就超过一笔事务的上限：把它的成员边切出去分几笔写（gen-model #155）。
///
/// 一个元素的行里只有成员边（`pe_owner`，一名成员一行）会随数据无限变长——MDS 支吊架目录里一个 `CATE` 名下
/// 10,920 个 `SCOM`，光边就过了 5000 行；它自己的 `pe` / noun / UDA 行各只有一行。所以：
///
/// - **头块**：这个元素自己的全部行（`Replace` 的删除块在内，含子槽位的整段区间删），加上还装得下的前一段成员边；
/// - **续块**：余下的成员边照槽位顺序一段一笔，每笔只有一条 `INSERT RELATION IGNORE`。
///
/// **续块排在头块之后**——本模块里唯一一处事务之间的先后依赖。`Replace` 下头块先把旧的子槽位整段删掉，续块才
/// 往里插：先插后删会把刚插的边带走；也不能让每笔续块各删各的一段槽位，成员换了位置时新槽位的边会撞上还没删的
/// 旧槽位那一条（`unique_pe_owner (in, out)`）。执行口（`land_increment_window`、`persist_latest_main_data`、
/// `cata_dependency` 的落库与前移刷新、`window_repair`、基线那条 lane 的单写者）都照返回的顺序逐笔执行；重放从头
/// 再来一遍，头块重删、各块重插，终态相同。`Fresh` 只插不删，没有这个依赖。
///
/// 头块落定到最后一笔续块落定之间，库里这个元素的成员表是不全的——与「一窗分多笔事务落库」同一种中间态，
/// 水位推进之前没有人把它当终态读。
///
/// 真切不动的只剩两种，照旧上报（文案仍以 `single element … exceeds the data transaction limits` 起头）：
/// 墓碑、没有成员的元素、或它自己的那几行就超限；上限小到连一条边都装不下。
///
/// 行数不靠解析正文来数：它自己的行量一次（很小），边是一条一行——头块 = 自己的行数 + 带的边数，续块 = 边数
/// （`pe_batch::transaction_with_known_rows`）。一个上万成员的属主光边就一兆正文，逐笔解析在调试版里是好几秒。
fn append_oversize_element(
    item: &BatchItem,
    dbnum: i32,
    mode: WriteMode,
    limits: DataTxLimits,
    // 整只渲出来过上限时的那个错（调用方渲过就给；没渲过——光数边就知道放不下——是 `None`）。
    whole: Option<anyhow::Error>,
    out: &mut Vec<String>,
    report: &mut ElementBatchReport,
) -> anyhow::Result<()> {
    let id = item.id().to_string();
    let too_big = |error: anyhow::Error| {
        error.context(format!(
            "single element {id} exceeds the data transaction limits"
        ))
    };
    let db = dbnum as u32;
    let alone = std::slice::from_ref(item);
    // 切不动时拿来当原因上报的：整只渲出来过上限的那个错。调用方渲过就用它的，没渲过现渲一遍。
    let whole_failure = move || {
        whole.unwrap_or_else(|| {
            let rendered = render_items(alone, dbnum, mode);
            pe_batch::transaction_with_limits(db, &rendered.ids, &rendered.sql, limits)
                .err()
                .unwrap_or_else(|| anyhow::anyhow!("its rows do not fit one transaction"))
        })
    };
    let edges: &[String] = match item {
        BatchItem::Rows { rows, .. } => &rows.owner_edges,
        BatchItem::Tombstone { .. } => &[],
    };
    if edges.is_empty() {
        // 没有成员边可切：超限的是它自己的行。
        return Err(too_big(whole_failure()));
    }
    let ids = [id.clone()];
    let render_failed =
        |error: anyhow::Error| error.context(format!("render the transaction of element {id}"));

    // 头块先不带边量一遍：它自己的行占多少，剩下的地方才是边的。
    let own = render_items_with_edge_cap(alone, dbnum, mode, 0);
    let own_size = pe_batch::measure(db, &own.ids, &own.sql).map_err(&render_failed)?;
    if own_size.bytes > limits.max_bytes || own_size.rows > limits.max_write_rows {
        return Err(too_big(whole_failure().context(format!(
            "its own rows alone take {} bytes / {} write rows, before any of its {} member edges",
            own_size.bytes,
            own_size.rows,
            edges.len()
        ))));
    }
    // 只装边的一笔事务里，边之外的固定开销（BEGIN / COMMIT 与那条语句的头尾）：拿只装头一条边的事务量出来。
    let one_edge =
        pe_batch::measure(db, &ids, &owner_edge_insert(&edges[..1])).map_err(&render_failed)?;
    let head_room = Room {
        rows: limits.max_write_rows - own_size.rows,
        bytes: (limits.max_bytes - own_size.bytes)
            .saturating_sub(OWNER_EDGE_INSERT_HEAD.len() + OWNER_EDGE_INSERT_TAIL.len()),
    };
    let rest_room = Room {
        rows: limits.max_write_rows,
        bytes: limits
            .max_bytes
            .saturating_sub(one_edge.bytes - edges[0].len()),
    };

    // 先全部渲好再入账：中途切不动就整只不落账。
    let mut built = Vec::new();
    // 段长是照行数与字节算出来的，照理正好放得下；万一算得不准，减半再试——不让整库为此失败。
    let mut head_edges = edges_that_fit(edges, head_room);
    loop {
        let head = render_items_with_edge_cap(alone, dbnum, mode, head_edges);
        let head_rows = own_size.rows + head_edges as u64;
        match pe_batch::transaction_with_known_rows(db, &head.ids, &head.sql, head_rows, limits) {
            Ok(sql) => {
                built.push(sql);
                break;
            }
            Err(error) if error.is::<pe_batch::ExceedsTxLimits>() => {
                if head_edges == 0 {
                    return Err(too_big(error));
                }
                head_edges /= 2;
            }
            Err(error) => return Err(render_failed(error)),
        }
    }
    let mut rest = &edges[head_edges..];
    while !rest.is_empty() {
        let mut take = edges_that_fit(rest, rest_room).max(1);
        loop {
            let chunk = owner_edge_insert(&rest[..take]);
            match pe_batch::transaction_with_known_rows(db, &ids, &chunk, take as u64, limits) {
                Ok(sql) => {
                    built.push(sql);
                    break;
                }
                Err(error) if error.is::<pe_batch::ExceedsTxLimits>() => {
                    if take == 1 {
                        // 连一条边都装不下：上限小得不成样子。
                        return Err(too_big(error.context("one member edge alone does not fit")));
                    }
                    take /= 2;
                }
                Err(error) => return Err(render_failed(error)),
            }
        }
        rest = &rest[take..];
    }

    for sql in &built {
        report.max_transaction_bytes = report.max_transaction_bytes.max(sql.len());
    }
    report.transactions += built.len();
    report.elements += 1;
    report.split_elements.push(SplitElement {
        id,
        members: edges.len(),
        transactions: built.len(),
    });
    out.extend(built);
    Ok(())
}

/// 前置块（一行一条 `DELETE …->pe_owner;`，不带元素 id）按字节 / 行上限切成事务，超限按行劈半。
fn push_preamble_transactions(
    dbnum: u32,
    writes: &str,
    limits: DataTxLimits,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    match pe_batch::transaction_with_limits(dbnum, &[], writes, limits) {
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
            push_preamble_transactions(dbnum, &left, limits, out)?;
            push_preamble_transactions(dbnum, &right, limits, out)
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

    /// 计划 §9-6：noun 行与 `ATT_UDA` 行必须带 `dbnum` + `sesno`——回退到水位是按这两列的
    /// 谓词 `DELETE`，少一列那张表就删不动。两种写法（基线 `Fresh` / 增量 `Replace`）都要带。
    #[test]
    fn noun_and_uda_rows_carry_the_columns_the_rollback_predicate_needs() {
        for mode in [WriteMode::Fresh, WriteMode::Replace] {
            let items = items_from_changes(&fixture(), &[], DBNUM, &UdaKeyMap::empty());
            let sql = render_items(&items, DBNUM, mode).sql;
            for noun in ["ZONE", "BRAN", "EQUI"] {
                assert!(
                    sql.contains(&format!(
                        "INSERT IGNORE INTO {noun} [{{ \"dbnum\": 8000, \"sesno\": 7,"
                    )),
                    "{mode:?}: {noun} 行缺回退谓词要的两列\n{sql}"
                );
            }
            // 大写的 E3D 属性名照旧写出，两者撞不上。
            assert!(sql.contains("\"DBNUM\": 8000"), "{mode:?}\n{sql}");
        }
    }

    /// UDA 行走的是另一条渲染（`gen_sur_json_uda`），同样经 [`stamp_owner_columns`]。
    /// 插入点是开头那个 `{` 的后面——对象结构不许被破坏，没有 `{` 的输入原样返回。
    #[test]
    fn the_stamp_goes_right_after_the_opening_brace_and_keeps_the_object_valid() {
        let stamped = stamp_owner_columns(
            "{\n  \"id\": \"24384_1\",\n  \"TYPE\": \"EQUI\"\n}",
            8000,
            7,
        );
        assert_eq!(
            stamped,
            "{ \"dbnum\": 8000, \"sesno\": 7,\n  \"id\": \"24384_1\",\n  \"TYPE\": \"EQUI\"\n}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&stamped).is_ok(),
            "{stamped}"
        );
        assert_eq!(stamp_owner_columns("", 8000, 7), "");
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
        // 观测块已退（ADR-071 D6）：元素块只有正文，Ref0 统计已整体退役。
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

    /// 已渲染行形态的批项（基线与 CATA 依赖行走的那条入口）：一个元素一项，`sesno` 7。
    fn rows_items(elements: &[EleData]) -> Vec<BatchItem> {
        elements
            .iter()
            .map(|element| BatchItem::Rows {
                rows: ElementRows::render(
                    element,
                    &element.refno.to_string(),
                    DBNUM,
                    7,
                    &UdaKeyMap::empty(),
                ),
                is_add: false,
                old_noun: None,
                dependency: false,
            })
            .collect()
    }

    /// 一笔事务里属于 `owner` 的那条成员边语句的方括号内容（没有就 `None`）。
    fn owner_edges_in<'a>(tx: &'a str, owner: RefU64) -> Option<&'a str> {
        let first = format!("{OWNER_EDGE_INSERT_HEAD}{{ id: pe_owner:[pe:{owner}, ");
        let start = tx.find(&first)? + OWNER_EDGE_INSERT_HEAD.len();
        let end = tx[start..].find(OWNER_EDGE_INSERT_TAIL)? + start;
        Some(&tx[start..end])
    }

    /// 各笔事务里 `owner` 的成员边照事务顺序接起来（中间补上分隔的逗号）——该与它的整份成员边逐字相同。
    fn owner_edges_across(txs: &[String], owner: RefU64) -> String {
        txs.iter()
            .filter_map(|tx| owner_edges_in(tx, owner))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn assert_within(txs: &[String], limits: DataTxLimits) {
        for (index, tx) in txs.iter().enumerate() {
            let rows =
                crate::data_interface::staging::replay_safe::estimate_write_rows(tx).unwrap();
            assert!(
                tx.len() <= limits.max_bytes && rows <= limits.max_write_rows,
                "第 {index} 笔事务 {} 字节 / {rows} 行，超过上限 {limits:?}",
                tx.len()
            );
            assert!(tx.starts_with("BEGIN TRANSACTION;\n"), "第 {index} 笔");
            assert!(
                tx.trim_end().ends_with("COMMIT TRANSACTION;"),
                "第 {index} 笔"
            );
        }
    }

    /// 一个元素名下的成员比一笔事务装得下的还多（gen-model #155）。照着真数据的样子：MDS 支吊架目录 7320 的
    /// `CATE /ATTA-FT03` 名下 10,920 个 `SCOM`——APS 支撑库的依赖闭包沿属主链把它带进来，光成员边就过了缺省的
    /// 5000 行，此前渲染在 `single element 15512_5547 exceeds the data transaction limits` 上失败，整库初始化挂起。
    ///
    /// 现在：它的成员边分笔写，每一笔都在缺省上限以内；它自己的行只出现一次；一万多条边一条不少、照槽位顺序；
    /// 排在它前后的元素照常各成一块。
    #[test]
    fn an_element_with_more_members_than_one_transaction_holds_is_cut_along_its_member_edges() {
        const MEMBERS: u64 = 10_920;
        let limits = DataTxLimits::default();
        let (sect, cate) = (refu(4520), refu(5547));
        let members: Vec<RefU64> = (0..MEMBERS).map(|n| refu(100_000 + n)).collect();
        let items = rows_items(&[
            element(sect, refu(2), "SECT", "/MDS-FT", &[cate]),
            element(cate, sect, "CATE", "/ATTA-FT03", &members),
            element(members[0], cate, "SCOM", "/FT03-1", &[]),
        ]);
        let BatchItem::Rows { rows: whole, .. } = &items[1] else {
            unreachable!()
        };
        assert_eq!(whole.owner_edges.len() as u64, MEMBERS);

        let (txs, report) =
            render_item_transactions_with_limits(&items, DBNUM, WriteMode::Fresh, limits).unwrap();
        assert_within(&txs, limits);

        // 自己的两行（pe + CATE）占头块 2 行，头块带 4,998 条边；余下 5,922 条分 5,000 + 922 两笔。
        assert_eq!(
            report.split_elements,
            vec![SplitElement {
                id: cate.to_string(),
                members: MEMBERS as usize,
                transactions: 3,
            }]
        );
        assert_eq!(report.elements, 3, "三个元素都落账，分笔写的那个只算一次");
        assert_eq!(report.transactions, txs.len());
        assert_eq!(txs.len(), 5, "SECT 一笔、CATE 三笔、SCOM 一笔");
        let per_tx: Vec<usize> = txs
            .iter()
            .map(|tx| owner_edges_in(tx, cate).map_or(0, |edges| edges.matches("{ id: ").count()))
            .collect();
        assert_eq!(per_tx, vec![0, 4_998, 5_000, 922, 0]);

        // 一万多条边一条不少、顺序不变。
        assert_eq!(owner_edges_across(&txs, cate), whole.owner_edges.join(","));
        // 它自己的行只在头块里。
        let own_row = format!("\"id\": pe:{cate}");
        assert_eq!(
            txs.iter()
                .map(|tx| tx.matches(&own_row).count())
                .collect::<Vec<_>>(),
            vec![0, 1, 0, 0, 0]
        );
        assert!(
            txs[1].contains("INSERT IGNORE INTO CATE ["),
            "{}",
            &txs[1][..400]
        );
        // 续块只有边。
        for tx in &txs[2..4] {
            assert_eq!(
                tx.matches(';').count(),
                3,
                "BEGIN、一条 INSERT RELATION、COMMIT"
            );
            assert!(tx.contains(OWNER_EDGE_INSERT_HEAD));
        }
        // 前后的元素照常：SECT 带着它那一条边，SCOM 没有成员。
        assert!(txs[0].contains("INSERT IGNORE INTO SECT ["));
        assert!(owner_edges_in(&txs[0], sect).is_some());
        assert!(txs[4].contains("INSERT IGNORE INTO SCOM [") && !txs[4].contains("pe_owner"));

        let notice = report.split_notice(7320).expect("分笔写要说一声");
        assert!(
            notice.contains(&cate.to_string())
                && notice.contains("10920")
                && notice.contains("3 笔"),
            "{notice}"
        );
        assert!(ElementBatchReport::default().split_notice(7320).is_none());
    }

    /// 头块带这个元素自己的全部行和装得下的前一段边；续块只有边、照槽位顺序。`Replace` 的删除块（子槽位的整段
    /// 区间删在内）只在头块里出现一次——续块里要是也删，后一笔会把前一笔刚插的边带走。前置块不受影响。
    #[test]
    fn the_head_carries_the_elements_own_rows_and_the_rest_are_edge_only_transactions() {
        let owner = refu(1);
        let members: Vec<RefU64> = (0..25).map(|n| refu(100 + n)).collect();
        let limits = DataTxLimits {
            max_write_rows: 10,
            ..Default::default()
        };
        let range_delete = format!("DELETE pe_owner:[pe:{owner}, NONE]..=[pe:{owner}, ..];");
        let mut head_edges = Vec::new();
        for mode in [WriteMode::Fresh, WriteMode::Replace] {
            let items = rows_items(&[element(owner, refu(0), "ZONE", "/Z", &members)]);
            let BatchItem::Rows { rows: whole, .. } = &items[0] else {
                unreachable!()
            };
            let own = render_items_with_edge_cap(&items, DBNUM, mode, 0);
            let own_rows = pe_batch::measure(DBNUM as u32, &own.ids, &own.sql)
                .unwrap()
                .rows;

            let (txs, report) =
                render_item_transactions_with_limits(&items, DBNUM, mode, limits).unwrap();
            assert_within(&txs, limits);
            assert_eq!(txs.len(), 3, "{mode:?}");
            assert_eq!(report.transactions, 3);
            assert_eq!(report.elements, 1);
            assert_eq!(report.split_elements.len(), 1);
            assert_eq!(owner_edges_across(&txs, owner), whole.owner_edges.join(","));

            // 头块：自己的行一行不少（就是不带边渲出来的那一段），后面跟着装得下的边。
            assert!(txs[0].contains(own.sql.trim_end()), "{mode:?}\n{}", txs[0]);
            let in_head =
                owner_edges_in(&txs[0], owner).map_or(0, |edges| edges.matches("{ id: ").count());
            assert_eq!(in_head as u64, limits.max_write_rows - own_rows, "{mode:?}");
            head_edges.push(in_head);
            // 续块：只有一条成员边语句，没有删除。
            for tx in &txs[1..] {
                assert_eq!(tx.matches(';').count(), 3, "{mode:?}\n{tx}");
                assert!(!tx.contains("DELETE"), "{mode:?}：续块不删\n{tx}");
                assert!(owner_edges_in(tx, owner).is_some());
            }
            assert_eq!(
                txs.iter()
                    .map(|tx| tx.matches(&range_delete).count())
                    .sum::<usize>(),
                usize::from(mode == WriteMode::Replace),
                "{mode:?}：子槽位的区间删只在 Replace 的头块里出现一次"
            );
            if mode == WriteMode::Replace {
                assert!(txs[0].contains(&range_delete));
                assert!(
                    txs[0].find(&range_delete).unwrap()
                        < txs[0].find(OWNER_EDGE_INSERT_HEAD).unwrap(),
                    "先删后插"
                );
            }
        }
        // Fresh 自己只有 pe + ZONE 两行，头块带 8 条边；Replace 多三条删除，头块带得少。
        assert_eq!(head_edges[0], 8);
        assert!(head_edges[1] < head_edges[0], "{head_edges:?}");

        // 窗口形态（前置块 + 元素块）：Add 的旧出向边清理照旧排在最前面，不掺进分笔写的那几笔。
        let changes = vec![
            change(
                owner,
                7,
                PersistElementState::Add(element(owner, refu(0), "ZONE", "/Z", &members)),
            ),
            change(
                members[0],
                7,
                PersistElementState::Add(element(members[0], owner, "EQUI", "/E", &[])),
            ),
        ];
        let (txs, report) = render_transactions_with_limits(
            &changes,
            &[],
            DBNUM,
            &UdaKeyMap::empty(),
            WriteMode::Replace,
            limits,
        )
        .unwrap();
        assert_within(&txs, limits);
        assert!(
            txs[0].contains(&format!("DELETE pe:{owner}->pe_owner;")) && !txs[0].contains("INSERT")
        );
        assert_eq!(report.elements, 2);
        assert_eq!(report.split_elements.len(), 1);
        assert_eq!(report.transactions, txs.len());
        let first_edges = txs
            .iter()
            .position(|tx| owner_edges_in(tx, owner).is_some())
            .unwrap();
        assert!(
            txs[first_edges].contains(&range_delete),
            "带边的第一笔就是头块"
        );
    }

    /// 字节上限同样照切：型材目录里一个 `SECT` 名下两万四千个成员，光边就约 2.0 MB、贴着 2 MiB 的上限。行数宽裕、字节紧时
    /// 每一笔仍在上限以内，边一条不少。
    #[test]
    fn member_edges_are_cut_by_the_byte_limit_too() {
        let owner = refu(1);
        let members: Vec<RefU64> = (0..60).map(|n| refu(100 + n)).collect();
        let items = rows_items(&[element(owner, refu(0), "SECT", "/S", &members)]);
        let BatchItem::Rows { rows: whole, .. } = &items[0] else {
            unreachable!()
        };
        let own = render_items_with_edge_cap(&items, DBNUM, WriteMode::Fresh, 0);
        let own_bytes = pe_batch::measure(DBNUM as u32, &own.ids, &own.sql)
            .unwrap()
            .bytes;
        // 头块除了自己的行只剩 600 字节给边（一条边 70 来字节）。
        let limits = DataTxLimits {
            max_bytes: own_bytes + 600,
            ..Default::default()
        };
        let (txs, report) =
            render_item_transactions_with_limits(&items, DBNUM, WriteMode::Fresh, limits).unwrap();
        assert_within(&txs, limits);
        assert!(txs.len() >= 3, "{} 笔", txs.len());
        assert_eq!(report.split_elements[0].transactions, txs.len());
        assert_eq!(owner_edges_across(&txs, owner), whole.owner_edges.join(","));
        // 每笔都装到装不下为止：再添下一条边就过字节上限。
        let edge_bytes = whole.owner_edges[0].len() + 1;
        for tx in &txs[..txs.len() - 1] {
            assert!(
                tx.len() + edge_bytes + 2 > limits.max_bytes,
                "没装满：{} 字节",
                tx.len()
            );
        }
    }

    /// 真切不动的照旧上报、点名元素：成员边之外没有可切的东西。
    #[test]
    fn what_cannot_be_cut_is_still_reported_with_the_element_named() {
        let one_row = DataTxLimits {
            max_write_rows: 1,
            ..Default::default()
        };
        // 有成员，但它自己的两行（pe + ZONE）就超过 1 行。
        let items = rows_items(&[element(refu(1), refu(0), "ZONE", "/Z", &[refu(2), refu(3)])]);
        let error = render_item_transactions_with_limits(&items, DBNUM, WriteMode::Fresh, one_row)
            .unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("single element 24384_1 exceeds the data transaction limits")
                && text.contains("its own rows alone take")
                && text.contains("2 write rows"),
            "{text}"
        );
        // 没有成员：没什么可切。
        let items = rows_items(&[element(refu(9), refu(0), "BOX", "/B", &[])]);
        let error = render_item_transactions_with_limits(&items, DBNUM, WriteMode::Fresh, one_row)
            .unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("single element 24384_9 exceeds the data transaction limits")
                && text.contains("2 write rows > 1"),
            "{text}"
        );
        assert!(!text.contains("statistics"), "{text}");
        // 墓碑同理。
        let changes = vec![change(
            refu(4),
            7,
            PersistElementState::Deleted {
                base: Some(element(refu(4), refu(1), "BOX", "/GONE", &[])),
            },
        )];
        let error = render_transactions_with_limits(
            &changes,
            &[],
            DBNUM,
            &UdaKeyMap::empty(),
            WriteMode::Replace,
            one_row,
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("exceeds the data transaction limits"),
            "{error:#}"
        );
    }

    async fn members_in_db(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        owner: RefU64,
    ) -> Vec<String> {
        let mut response = db
            .query(format!(
                "SELECT VALUE in FROM pe_owner:[pe:{owner}, NONE]..=[pe:{owner}, ..];"
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        response
            .take::<Vec<surrealdb::sql::Thing>>(0)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// 落到库里对不对（库里带着 `unique_pe_owner (in, out)`，成员换槽位时先删后插的顺序错了会撞它）：
    ///
    /// 1. 基线（`Fresh`）：分笔写的元素，库里它名下的边与成员表一条不差、照槽位顺序；
    /// 2. 增量（`Replace`）换一份成员表——砍掉一截、剩下的全部换位置、再添几个——照顺序落完，旧边一条不剩；
    /// 3. 中途断掉（只落了头块和一笔续块）再从头重放，收敛到同一终态；再重放一遍不变。
    #[tokio::test]
    async fn a_split_element_lands_and_replays_to_exactly_its_member_list() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("element_batch").use_db("split").await.unwrap();
        db.query(
            "DEFINE TABLE pe_owner TYPE RELATION IN pe OUT pe SCHEMALESS; \
             DEFINE INDEX unique_pe_owner ON pe_owner FIELDS in, out UNIQUE;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let limits = DataTxLimits {
            max_write_rows: 40,
            ..Default::default()
        };
        let owner = refu(1);
        let first: Vec<RefU64> = (0..150).map(|n| refu(1000 + n)).collect();
        let second: Vec<RefU64> = first[30..]
            .iter()
            .rev()
            .copied()
            .chain((0..5).map(|n| refu(5000 + n)))
            .collect();
        let keys = |members: &[RefU64]| {
            members
                .iter()
                .map(|member| member.to_pe_key())
                .collect::<Vec<_>>()
        };
        let render = |members: &[RefU64], mode: WriteMode| {
            let items = rows_items(&[element(owner, refu(0), "ZONE", "/Z", members)]);
            let (txs, report) =
                render_item_transactions_with_limits(&items, DBNUM, mode, limits).unwrap();
            assert_eq!(report.split_elements.len(), 1, "{mode:?}：这个元素要分笔写");
            assert!(txs.len() >= 4, "{mode:?}：{} 笔", txs.len());
            txs
        };

        for tx in &render(&first, WriteMode::Fresh) {
            pe_batch::execute_on(&db, tx).await.unwrap();
        }
        assert_eq!(members_in_db(&db, owner).await, keys(&first));

        let replace = render(&second, WriteMode::Replace);
        // 中途断掉：头块 + 一笔续块落了，后面的没落。
        for tx in &replace[..2] {
            pe_batch::execute_on(&db, tx).await.unwrap();
        }
        let partial = members_in_db(&db, owner).await;
        assert!(
            partial.len() < second.len() && partial == keys(&second)[..partial.len()],
            "中间态是新成员表打头的一段（{} / {}），旧边已经整段删掉",
            partial.len(),
            second.len()
        );
        // 从头重放两遍。
        for _ in 0..2 {
            for tx in &replace {
                pe_batch::execute_on(&db, tx).await.unwrap();
            }
            assert_eq!(members_in_db(&db, owner).await, keys(&second));
        }
        // 元素自己的行是新的那一份，只有一行。
        let mut response = db
            .query(format!(
                "SELECT count() AS n FROM pe GROUP ALL; RETURN pe:{owner}.noun;"
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        #[derive(serde::Deserialize)]
        struct Count {
            n: i64,
        }
        let rows: Vec<Count> = response.take(0).unwrap();
        assert_eq!(rows[0].n, 1);
        assert_eq!(
            response.take::<Option<String>>(1).unwrap().as_deref(),
            Some("ZONE")
        );
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
