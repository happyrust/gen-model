//! CATA 窗口 S→T 的**变动目录件集合**——目录反向级联的触发源（ADR-058 D2，
//! `docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md` P2）。
//!
//! direct / 读透形态下目录库**没有数据批次**：`build_cata_cascade_plan` 吃的 `range_eles`
//! 在这条路上根本不存在，「目录件变了 → 谁引用它」这一问的第一半——**哪些目录件变了**——
//! 只能从文件自己回答。本模块就是那一半：对一个目录库文件做会话索引差分，把候选逐个读成
//! `(refno, 变更类, noun, owner)`，并顺手报出该库在 T 端**有哪些反向引用表**——第二半
//! （反查引用者，P3）要靠它们；一张都没有时（`ams7351_0001` 那种，见
//! `docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md` §3.5）P3 只能退化。
//!
//! 与 e3d-model `collect_window` 的分工：那边是 L1 索引候选（refno + 增/删/改），
//! 这里多读一层记录头拿 noun / owner（模板无关，`ParsedElement::parse`），并把
//! `Modified { relocated_only }`——两端字节完全相同、只是搬了位置——**剔出**变动集：
//! 那不是变更，重算它引用者的每一根都是白算。
//!
//! 输入只有文件与两个会话号，不碰任何库；阻塞 I/O，异步上下文请放进 `spawn_blocking`。
//! S 从哪来、T 是谁、算出来的集合交给谁，都不归这里——见
//! [`crate::data_interface::model_update_pending`] 的目录版本核对（启动 / 读透形态的文件
//! 事件都汇到那里）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Context;
use e3d_attlib::db1_dehash;
use e3d_io::RefNo;
use e3d_io::index::reference::ReferenceTable;
use e3d_io::record::element::ParsedElement;
use e3d_io::session::{ChangeKind, DbView};
use serde::{Deserialize, Serialize};

/// 一个目录件在窗口里发生了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogueChangeKind {
    /// T 端有、S 端没有。**也是种子**：设计侧可能早就存着一个当时解不出的目录 refno，
    /// 目录件后来出现时要唤醒那些根（`build_cata_cascade_plan` 同一口径）。
    Added,
    /// 两端都有、记录被重写且字节不同。
    Modified,
    /// S 端有、T 端没有。
    Deleted,
}

/// 窗口里一个真正变了的目录件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueChange {
    /// `dbno/seq` 形式，与 `RefnoEnum::to_pdms_str` 同一写法。**显示用**——`word0` 还带着
    /// extent 位，从这个字符串反解不回原 refno，反查请用 [`Self::refno`]。
    pub refno: String,
    /// 原样的 `(word0, word1)`，反查点表用它。
    pub key: (u32, u32),
    pub kind: CatalogueChangeKind,
    /// 记录头里的 noun（T 端；删除取 S 端）。记录头读不出来是 `None`，**不是**「没有类型」。
    pub noun: Option<String>,
    /// 记录头里的 owner，同上取端。
    pub owner: Option<String>,
}

impl CatalogueChange {
    /// 反查用的原 refno。
    pub fn refno(&self) -> RefNo {
        RefNo::new(self.key.0, self.key.1)
    }
}

/// 一个目录库一窗的变动集合与它的反查条件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueWindow {
    pub dbnum: u32,
    pub base_sesno: u32,
    pub target_sesno: u32,
    /// 真正变了的目录件（已剔除 `relocated_only`），按 refno 升序。
    pub changes: Vec<CatalogueChange>,
    /// 索引说改了、两端字节却完全相同的候选数——被剔出 `changes` 的那些。
    pub relocated_only: usize,
    /// 记录头读不出来（noun / owner 为 `None`）的变动数，`changes` 里仍保留它们。
    pub unreadable_records: usize,
    /// 候选数超过调用方给的上限时**不逐条读记录**：这里是索引差分报的候选总数，
    /// `changes` 为空、`relocated_only` 为 0。一次性导入的窗口（几万到上百万个 Inserted）
    /// 就是这个形状——它本来也不是「改一个目录件」那种能精确反查的窗口。
    pub truncated_candidates: Option<usize>,
    /// T 端目录里**通过判据**的反向引用表名（4 字键、无值、引用者是本库 refno；
    /// `TYPEX` 那类同形表被判据挡在外面）。空 = 这个库不留任何反向表，P3 对它只能退化。
    pub reference_tables: Vec<String>,
}

/// 一窗的回执 / `/health` 形状：只有数，不带逐件清单（清单是 P3 的输入，不上 `/health`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueWindowReport {
    pub dbnum: u32,
    pub base_sesno: u32,
    pub target_sesno: u32,
    pub changes: usize,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub relocated_only: usize,
    pub unreadable_records: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_candidates: Option<usize>,
    pub nouns: BTreeMap<String, usize>,
    pub reference_tables: Vec<String>,
    /// 窗口收不了（会话不在链上、文件打不开……）时的原因；此时上面的数全是 0。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CatalogueWindowReport {
    pub fn from_window(window: &CatalogueWindow) -> Self {
        Self {
            dbnum: window.dbnum,
            base_sesno: window.base_sesno,
            target_sesno: window.target_sesno,
            changes: window.changes.len(),
            added: window.added(),
            modified: window.modified(),
            deleted: window.deleted(),
            relocated_only: window.relocated_only,
            unreadable_records: window.unreadable_records,
            truncated_candidates: window.truncated_candidates,
            nouns: window.nouns(),
            reference_tables: window.reference_tables.clone(),
            error: None,
        }
    }

    pub fn failed(dbnum: u32, base_sesno: u32, target_sesno: u32, error: String) -> Self {
        Self {
            dbnum,
            base_sesno,
            target_sesno,
            changes: 0,
            added: 0,
            modified: 0,
            deleted: 0,
            relocated_only: 0,
            unreadable_records: 0,
            truncated_candidates: None,
            nouns: BTreeMap::new(),
            reference_tables: Vec::new(),
            error: Some(error),
        }
    }

    /// 横幅一行；收不了的窗口说原因。
    pub fn banner_line(&self) -> String {
        if let Some(error) = &self.error {
            return format!(
                "目录窗口 dbnum={} {}→{} 收不了（这一窗的变动目录件**未知**，不是零）：{error}",
                self.dbnum, self.base_sesno, self.target_sesno
            );
        }
        let nouns = self
            .nouns
            .iter()
            .map(|(noun, count)| format!("{noun}×{count}"))
            .collect::<Vec<_>>()
            .join(" ");
        let tables = if self.reference_tables.is_empty() {
            "无反向引用表（P3 只能退化为该库依赖者全部过期）".to_string()
        } else {
            format!("反向引用表 [{}]", self.reference_tables.join(" "))
        };
        match self.truncated_candidates {
            Some(candidates) => format!(
                "目录窗口 dbnum={} {}→{}：索引候选 {candidates} 个，超过逐条读记录的上限，\
                 未按件列出（导入级窗口，不是精确反查的形状）；{tables}",
                self.dbnum, self.base_sesno, self.target_sesno
            ),
            None => format!(
                "目录窗口 dbnum={} {}→{}：变动目录件 {}（+{} ~{} -{}，剔除仅搬迁 {}）{}{}；{}",
                self.dbnum,
                self.base_sesno,
                self.target_sesno,
                self.changes,
                self.added,
                self.modified,
                self.deleted,
                self.relocated_only,
                if nouns.is_empty() {
                    String::new()
                } else {
                    format!(" [{nouns}]")
                },
                if self.unreadable_records > 0 {
                    format!("，{} 条记录头读不出", self.unreadable_records)
                } else {
                    String::new()
                },
                tables
            ),
        }
    }
}

impl CatalogueWindow {
    pub fn added(&self) -> usize {
        self.count(CatalogueChangeKind::Added)
    }

    pub fn modified(&self) -> usize {
        self.count(CatalogueChangeKind::Modified)
    }

    pub fn deleted(&self) -> usize {
        self.count(CatalogueChangeKind::Deleted)
    }

    fn count(&self, kind: CatalogueChangeKind) -> usize {
        self.changes
            .iter()
            .filter(|change| change.kind == kind)
            .count()
    }

    /// `noun → 变动数`，横幅与回执用；读不出 noun 的记在 `"?"` 下。
    pub fn nouns(&self) -> BTreeMap<String, usize> {
        let mut nouns = BTreeMap::new();
        for change in &self.changes {
            *nouns
                .entry(change.noun.clone().unwrap_or_else(|| "?".to_string()))
                .or_default() += 1;
        }
        nouns
    }

    /// 横幅一行：说清变了多少、变了什么、反查有没有表可用。
    pub fn summary_line(&self) -> String {
        CatalogueWindowReport::from_window(self).banner_line()
    }
}

/// 判「是不是反向引用表」时最多看多少条：`TYPEX` 第一条就跨库，多看几条只是保险。
const REFERENCE_TABLE_SAMPLE: usize = 8;

/// 收一窗：索引差分 → 逐候选读记录头 → T 端反向表清单。不设候选上限。
///
/// 两个会话都必须在文件的会话链上（e3d-io `diff_sessions` 的判据，链序为准），否则报错——
/// 「记在案的目录会话已被 MERGE CHANGES 折掉」是调用方要处理的形状，这里不猜。
pub fn collect_catalogue_window(
    dbnum: u32,
    file: &Path,
    base_sesno: u32,
    target_sesno: u32,
) -> anyhow::Result<CatalogueWindow> {
    collect_catalogue_window_bounded(dbnum, file, base_sesno, target_sesno, usize::MAX)
}

/// 同上，但索引候选超过 `max_records` 时不逐条读记录，只报候选总数
/// （[`CatalogueWindow::truncated_candidates`]）。启动路径用它兜住导入级窗口。
pub fn collect_catalogue_window_bounded(
    dbnum: u32,
    file: &Path,
    base_sesno: u32,
    target_sesno: u32,
    max_records: usize,
) -> anyhow::Result<CatalogueWindow> {
    let mut view = DbView::open(file)
        .with_context(|| format!("open catalogue file {} of dbnum {dbnum}", file.display()))?;
    let diff = view
        .diff_sessions(base_sesno, target_sesno)
        .with_context(|| {
            format!(
                "diff catalogue sessions {base_sesno}→{target_sesno} of {}",
                file.display()
            )
        })?;
    let candidates = diff.changes().len();
    if candidates > max_records {
        return Ok(CatalogueWindow {
            dbnum,
            base_sesno,
            target_sesno,
            changes: Vec::new(),
            relocated_only: 0,
            unreadable_records: 0,
            truncated_candidates: Some(candidates),
            reference_tables: reference_tables_at(file, target_sesno)?,
        });
    }

    let mut changes = Vec::new();
    let mut relocated_only = 0usize;
    let mut unreadable_records = 0usize;
    for element in view.changed_elements(&diff) {
        let element = element.with_context(|| {
            format!(
                "read a changed catalogue record of {} in {base_sesno}→{target_sesno}",
                file.display()
            )
        })?;
        let kind = match element.kind {
            ChangeKind::Inserted => CatalogueChangeKind::Added,
            ChangeKind::Deleted => CatalogueChangeKind::Deleted,
            ChangeKind::Modified {
                relocated_only: true,
            } => {
                relocated_only += 1;
                continue;
            }
            ChangeKind::Modified {
                relocated_only: false,
            } => CatalogueChangeKind::Modified,
        };
        // 删除取 S 端记录、其余取 T 端——`record()` 正是这个偏好。
        let (noun, owner) = match ParsedElement::parse(&element.record().raw_bytes) {
            Ok(parsed) => (
                Some(
                    db1_dehash(parsed.noun_hash)
                        .unwrap_or_else(|| format!("#{}", parsed.noun_hash)),
                ),
                Some(refno_text(parsed.owner)),
            ),
            Err(_) => {
                unreadable_records += 1;
                (None, None)
            }
        };
        changes.push(CatalogueChange {
            refno: refno_text(element.refno),
            key: (element.refno.word0, element.refno.word1),
            kind,
            noun,
            owner,
        });
    }
    changes.sort_by(|a, b| a.refno.cmp(&b.refno));

    let reference_tables = reference_tables_at(file, target_sesno)?;
    Ok(CatalogueWindow {
        dbnum,
        base_sesno,
        target_sesno,
        changes,
        relocated_only,
        unreadable_records,
        truncated_candidates: None,
        reference_tables,
    })
}

/// T 端目录里通过判据的反向引用表名，升序。
///
/// 判据与 e3d-io `tests/reference_table_real.rs` 同一条：页能按引用表解出来，**且**采样到的
/// 引用者全是本库 refno（`db_put_reference` 把边记在引用库里，键后半不可能跨库）。
/// `TYPEX` 第一条就跨库，被挡在外面；`NAME` / `SPAMAP` 那类别种表连页都解不出来。
pub fn reference_tables_at(file: &Path, sesno: u32) -> anyhow::Result<Vec<String>> {
    let mut view = DbView::open_at(file, sesno)
        .with_context(|| format!("open {} at session {sesno}", file.display()))?;
    let own_dbno = view.descriptor().db_mark;
    let tables: Vec<_> = view.tables().to_vec();
    let mut names = BTreeSet::new();
    for table in tables {
        let Ok(mut reference) = ReferenceTable::open(&mut view, table.name_hash) else {
            continue;
        };
        let mut local = true;
        let mut seen = 0usize;
        for entry in reference.iter().take(REFERENCE_TABLE_SAMPLE) {
            let Ok(entry) = entry else {
                local = false;
                break;
            };
            seen += 1;
            if entry.referrer.dbno() != own_dbno {
                local = false;
                break;
            }
        }
        if local && seen > 0 {
            names.insert(
                db1_dehash(table.name_hash).unwrap_or_else(|| format!("#{}", table.name_hash)),
            );
        }
    }
    Ok(names.into_iter().collect())
}

fn refno_text(refno: RefNo) -> String {
    format!("{}/{}", refno.dbno(), refno.word1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMS5052: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5052_0001";
    const AMS7351: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7351_0001";

    fn fixture(path: &str) -> Option<&Path> {
        let path = Path::new(path);
        if path.is_file() {
            Some(path)
        } else {
            println!("skip: fixture {} is not installed", path.display());
            None
        }
    }

    /// 计划 P2 的验收门：`ams5052_0001 184→185` 恰好一个 Modified 目录件（09-04 inventory
    /// §2.10 探明的「改一个目录件」窗口），没有增删、noun 读得出、反向表清单非空。
    #[test]
    fn ams5052_184_to_185_changes_exactly_one_catalogue_element() {
        let Some(file) = fixture(AMS5052) else { return };
        let window = collect_catalogue_window(5052, file, 184, 185).expect("collect window");
        println!("{}", window.summary_line());
        assert_eq!(window.changes.len(), 1, "{:?}", window.changes);
        assert_eq!(window.modified(), 1);
        assert_eq!(window.added() + window.deleted(), 0);
        let change = &window.changes[0];
        assert!(change.noun.is_some(), "noun must be readable: {change:?}");
        assert!(change.owner.is_some(), "owner must be readable: {change:?}");
        assert_eq!(window.unreadable_records, 0);
        assert!(
            window.reference_tables.iter().any(|name| name == "CATR"),
            "a live catalogue keeps its CATR table: {:?}",
            window.reference_tables
        );
        assert!(
            !window.reference_tables.iter().any(|name| name == "TYPEX"),
            "TYPEX is a lookalike, never a reference table"
        );
        assert_eq!(window.truncated_candidates, None);
    }

    /// 上限只截逐条读记录那一步：候选总数照报、反向表照列，`changes` 为空且说明原因。
    #[test]
    fn a_window_over_the_record_cap_reports_its_candidate_count_instead() {
        let Some(file) = fixture(AMS5052) else { return };
        let window =
            collect_catalogue_window_bounded(5052, file, 184, 185, 0).expect("collect window");
        assert_eq!(window.truncated_candidates, Some(1));
        assert!(window.changes.is_empty());
        assert!(!window.reference_tables.is_empty());
        let line = window.summary_line();
        assert!(line.contains("索引候选 1 个"), "{line}");
        assert!(line.contains("反向引用表 ["), "{line}");
    }

    /// 收不了的窗口在回执里是「未知」，不是零个变动。
    #[test]
    fn a_failed_window_is_reported_as_unknown_not_empty() {
        let report =
            CatalogueWindowReport::failed(7000, 5, 9, "session 5 is not on the chain".into());
        let line = report.banner_line();
        assert!(line.contains("dbnum=7000 5→9 收不了"), "{line}");
        assert!(line.contains("未知"), "{line}");
        assert_eq!(report.changes, 0);
        assert!(report.error.is_some());
        let json = serde_json::to_value(&report).expect("serialises");
        assert!(json.get("error").is_some());
        assert!(json.get("truncated_candidates").is_none(), "{json}");
    }

    /// `ams7351_0001` 一张反向表都没有——`reference_tables` 必须为空并在横幅上说出来，
    /// 这是 P3 退路 (ii) 的开关。
    #[test]
    fn a_catalogue_without_reference_tables_says_so() {
        let Some(file) = fixture(AMS7351) else { return };
        let latest = DbView::open(file).expect("open ams7351").selected_session();
        let tables = reference_tables_at(file, latest).expect("list tables");
        assert!(tables.is_empty(), "ams7351_0001 now states {tables:?}");
        let window = CatalogueWindow {
            dbnum: 7351,
            base_sesno: latest,
            target_sesno: latest,
            changes: Vec::new(),
            relocated_only: 0,
            unreadable_records: 0,
            truncated_candidates: None,
            reference_tables: tables,
        };
        assert!(window.summary_line().contains("无反向引用表"));
    }

    /// 横幅把三态、剔除数、noun 直方与表清单都说出来；读不出的记录头单独点名。
    #[test]
    fn the_summary_line_names_counts_nouns_and_tables() {
        let window = CatalogueWindow {
            dbnum: 5052,
            base_sesno: 184,
            target_sesno: 185,
            changes: vec![
                CatalogueChange {
                    refno: "5052/10".into(),
                    key: (5052, 10),
                    kind: CatalogueChangeKind::Modified,
                    noun: Some("SCOM".into()),
                    owner: Some("5052/1".into()),
                },
                CatalogueChange {
                    refno: "5052/11".into(),
                    key: (5052, 11),
                    kind: CatalogueChangeKind::Added,
                    noun: None,
                    owner: None,
                },
            ],
            relocated_only: 3,
            unreadable_records: 1,
            truncated_candidates: None,
            reference_tables: vec!["CATR".into(), "GMRE".into()],
        };
        let line = window.summary_line();
        assert!(line.contains("dbnum=5052 184→185"), "{line}");
        assert!(
            line.contains("变动目录件 2（+1 ~1 -0，剔除仅搬迁 3）"),
            "{line}"
        );
        assert!(line.contains("SCOM×1"), "{line}");
        assert!(line.contains("?×1"), "{line}");
        assert!(line.contains("1 条记录头读不出"), "{line}");
        assert!(line.contains("反向引用表 [CATR GMRE]"), "{line}");
        assert_eq!(window.nouns().get("SCOM"), Some(&1));
    }
}
