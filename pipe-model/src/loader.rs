//! J 组：CSV/表格导入（spec Phase D；功能文档 §18.12 ELEMENTLOADER）。
//!
//! 三段协议：
//! 1. `validate_headings`：必须 NAME+TYPE 列，OWNER 可选
//! 2. `analyse_rows`：逐行预检 → VALID/EXISTS/ERROR
//! 3. `process_data(new, update)`：VALID 走 `NEW <type> <name>`（handle(47,15)
//!    撞名行标 FAIL），EXISTS 走直赋
//!
//! 逐列写属性四规则：空值跳 / 'unset' 跳 / LOCK 整列跳 / DBREF 无 `/`=`=` 补 `/`。
//! 注意反直觉约定（§18.12）：`setAttribute` 成功也返回非空回显串，判 FAIL 靠
//! `empty()`——本实现 RowResult.message 恒为"最后消息"字段，成败只看 status。

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::Context;
use crate::error::PipeError;
use crate::service::PipeModelService;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use serde::{Deserialize, Serialize};

/// 一行导入数据（表头归一后的键值）。
#[derive(Debug, Clone)]
pub struct ImportRow {
    /// 行号（1 基，含表头占位时与 E3D 报表一致）。
    pub line: u32,
    /// 列名 → 原始值。
    pub cells: Vec<(String, String)>,
}

/// 导入表。
#[derive(Debug, Clone)]
pub struct ImportTable {
    /// 表头（第一行）。
    pub headings: Vec<String>,
    pub rows: Vec<ImportRow>,
    /// `processData(new, update)` 复选框语义：不勾建的 VALID 行不进批，
    /// 不勾改的 EXISTS 行不进批（§18.12 skipConnected 同款选项面）。
    pub create_new: bool,
    pub update_existing: bool,
}

/// 行预检态（ELEMENTLOADER 五值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowStatus {
    /// 可建。
    Valid,
    /// 已存在可走 update。
    Exists,
    /// 校验错误（标题/类型/属主）。
    Error,
    /// 执行失败。
    Fail,
    /// 执行成功（**回显串可能仍非空**，见模块文档）。
    Success,
}

/// 行级结论。
#[derive(Debug, Clone)]
pub struct RowResult {
    pub line: u32,
    pub status: RowStatus,
    /// "最后消息"字段：成功也可能非空，判 FAIL 只看 status。
    pub message: String,
}

/// 导入报告：逐行回显（§18.12 行级错误语义）。
#[derive(Debug, Default)]
pub struct ImportReport {
    pub results: Vec<RowResult>,
}

impl ImportReport {
    /// 表头校验：NAME+TYPE 必须（ELEMENTLOADER.validateHeadings）。
    /// 返回缺失列名列表。
    pub fn validate_headings(headings: &[String]) -> Vec<String> {
        ["NAME", "TYPE"]
            .iter()
            .filter(|req| !headings.iter().any(|h| h.eq_ignore_ascii_case(req)))
            .map(|s| s.to_string())
            .collect()
    }
}

/// 元数据列（非属性写列）。
const META_COLS: &[&str] = &["NAME", "TYPE", "OWNER"];

fn cell<'a>(row: &'a ImportRow, name: &str) -> Option<&'a str> {
    row.cells
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// 列值 → NamedAttrValue。
///
/// 规则（§18.12 `setAttribute` 类型探测序）：取现值类型 → STRING →
/// ATTDEFINITION.type=='TEXT' 防伪串 → DBREF。DBREF 值无 `/`/`=` 前缀补 `/`，
/// 反查失败落 StringType 并计 warning（不吞）。
fn value_to_attr(
    cat: &impl PipeCatalog,
    element: RefU64,
    name: &str,
    raw: &str,
) -> (NamedAttrValue, Option<String>) {
    let raw = raw.trim();
    match cat.attr(element, name) {
        Some(NamedAttrValue::F32Type(_)) => {
            (raw.parse::<f32>().map(NamedAttrValue::F32Type).unwrap_or(NamedAttrValue::InvalidType), None)
        }
        Some(NamedAttrValue::IntegerType(_)) => {
            (raw.parse::<i32>().map(NamedAttrValue::IntegerType).unwrap_or(NamedAttrValue::InvalidType), None)
        }
        Some(NamedAttrValue::BoolType(_)) => {
            let b = matches!(raw.to_ascii_uppercase().as_str(), "T" | "TRUE" | "1" | "YES");
            (NamedAttrValue::BoolType(b), None)
        }
        Some(NamedAttrValue::RefU64Type(_)) | None => {
            // DBREF 或无现值：补 `/` 反查；查不到落字符串 + warning。
            let mut v = raw.to_string();
            if !(v.starts_with('/') || v.starts_with('=')) {
                v = format!("/{v}");
            }
            match cat.refno_by_name(&v) {
                Some(r) => (NamedAttrValue::RefU64Type(r), None),
                None => (
                    NamedAttrValue::StringType(v.clone()),
                    Some(format!("{name}: DBREF 反查 {v} 失败，落字符串")),
                ),
            }
        }
        _ => (NamedAttrValue::StringType(raw.to_string()), None),
    }
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 三段协议主链（Story 45/46）。
    pub(crate) fn import_rows_impl(
        &self,
        ctx: &Context,
        table: &ImportTable,
    ) -> Result<(MutationBatch, ImportReport), PipeError> {
        // 1) 表头闸：NAME+TYPE 缺一整批不导。
        let missing = ImportReport::validate_headings(&table.headings);
        if !missing.is_empty() {
            return Err(PipeError::Blocking {
                message: format!("导入表头缺必备列：{}", missing.join(", ")),
            });
        }

        let mut batch = MutationBatch::named("import_rows");
        let mut report = ImportReport::default();

        for row in &table.rows {
            // 2) 行预检（analyseRows）。
            let name_raw = cell(row, "NAME").unwrap_or("").trim();
            let type_raw = cell(row, "TYPE").unwrap_or("").trim();
            if name_raw.is_empty() || type_raw.is_empty() {
                report.results.push(RowResult {
                    line: row.line,
                    status: RowStatus::Error,
                    message: "NAME/TYPE 缺失".into(),
                });
                continue;
            }
            let mut name = name_raw.to_string();
            if !name.starts_with('/') {
                name = format!("/{name}");
            }
            // OWNER：空=CE（§18.12 ownerEmpty→ce），显式名必须可反查。
            let owner = match cell(row, "OWNER").map(str::trim).filter(|s| !s.is_empty()) {
                None => ctx.element,
                Some(o) => {
                    let on = if o.starts_with('/') { o.to_string() } else { format!("/{o}") };
                    match self.catalog.refno_by_name(&on) {
                        Some(r) => r,
                        None => {
                            report.results.push(RowResult {
                                line: row.line,
                                status: RowStatus::Error,
                                message: format!("OWNER {on} 反查失败"),
                            });
                            continue;
                        }
                    }
                }
            };

            let existing = self.catalog.refno_by_name(&name);
            let (analysed, target) = match existing {
                Some(el) => (RowStatus::Exists, el),
                None => {
                    if !table.create_new {
                        report.results.push(RowResult {
                            line: row.line,
                            status: RowStatus::Valid,
                            message: "new 未勾选，行不入批".into(),
                        });
                        continue;
                    }
                    (RowStatus::Valid, self.catalog.alloc_refno(owner))
                }
            };
            if analysed == RowStatus::Exists && !table.update_existing {
                report.results.push(RowResult {
                    line: row.line,
                    status: RowStatus::Exists,
                    message: "update 未勾选，行不入批".into(),
                });
                continue;
            }

            // 3) processData：VALID→Create、EXISTS→SetAttrs；逐列四规则。
            if analysed == RowStatus::Valid {
                batch.ops.push(MutationOp::Create {
                    refno: target,
                    noun: type_raw.to_ascii_uppercase(),
                    under: owner,
                    name: Some(name.clone()),
                    attrs: NamedAttrMap::default(),
                });
            }
            let mut wrote = 0usize;
            for (col, raw) in &row.cells {
                if META_COLS.iter().any(|m| col.eq_ignore_ascii_case(m))
                    || col.eq_ignore_ascii_case("LOCK")
                {
                    continue; // 元数据列 + LOCK 整列跳
                }
                let raw = raw.trim();
                if raw.is_empty() || raw.eq_ignore_ascii_case("unset") {
                    continue; // 空值 / 'unset' 跳
                }
                let (v, warn) = value_to_attr(&self.catalog, target, col, raw);
                if let Some(w) = warn {
                    batch.warnings.push(crate::outcome::PipeWarning {
                        message: format!("行{} {w}", row.line),
                        side_effect: None,
                    });
                }
                if matches!(v, NamedAttrValue::InvalidType) {
                    continue;
                }
                batch.ops.push(MutationOp::SetAttrs {
                    element: target,
                    attrs: NamedAttrMap {
                        map: std::collections::BTreeMap::from([(col.clone(), v)]),
                    },
                });
                wrote += 1;
            }
            // handle(47,15) 撞名：VALID 但名字在批内已重复（同一批两个同名新行）。
            report.results.push(RowResult {
                line: row.line,
                status: RowStatus::Success,
                message: format!("{type_raw} {name}：写 {wrote} 属性"),
            });
        }

        // 批内同名新行检测（NEW <name> 撞名语义 handle(47,15)）。
        let mut seen = std::collections::HashSet::new();
        for op in &batch.ops {
            if let MutationOp::Create { name: Some(n), .. } = op {
                if !seen.insert(n.clone()) {
                    if let Some(r) = report
                        .results
                        .iter_mut()
                        .find(|r| r.message.contains(n.as_str()) && r.status == RowStatus::Success)
                    {
                        r.status = RowStatus::Fail;
                        r.message = format!("(47,15) 批内同名冲突 {n}");
                    }
                }
            }
        }
        Ok((batch, report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::PipeModel;
    use crate::testkit::{ctx, MockCatalog};

    fn table(rows: Vec<ImportRow>) -> ImportTable {
        ImportTable {
            headings: vec!["NAME".into(), "TYPE".into(), "OWNER".into(), "BORE".into(), "LOCK".into()],
            rows,
            create_new: true,
            update_existing: true,
        }
    }

    fn row(line: u32, cells: &[(&str, &str)]) -> ImportRow {
        ImportRow {
            line,
            cells: cells.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn missing_headings_blocks() {
        let cat = MockCatalog::new();
        let svc = PipeModelService::new(cat);
        let t = ImportTable {
            headings: vec!["NAME".into()],
            rows: vec![],
            create_new: true,
            update_existing: true,
        };
        assert!(matches!(
            svc.import_rows(&ctx(), &t).unwrap_err(),
            PipeError::Blocking { .. }
        ));
    }

    #[test]
    fn new_row_creates_with_columns() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1");
        cat.names.insert(RefU64(9), "/Z1".into());
        let svc = PipeModelService::new(cat);
        let mut c = ctx();
        c.element = RefU64(9); // OWNER 空 → CE
        let t = table(vec![row(2, &[
            ("NAME", "PIPE-X"), ("TYPE", "PIPE"), ("BORE", "100"),
            ("LOCK", "ignored"), ("OWNER", ""),
        ])]);
        let (batch, report) = svc.import_rows(&c, &t).unwrap();
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].status, RowStatus::Success);
        // Create + BORE SetAttrs；LOCK 列被跳。
        assert!(batch.ops.iter().any(|op| matches!(op, MutationOp::Create { noun, name, under, .. }
            if noun == "PIPE" && name.as_deref() == Some("/PIPE-X") && *under == RefU64(9))));
        assert!(batch.ops.iter().any(|op| matches!(op, MutationOp::SetAttrs { element, attrs }
            if attrs.contains_key("BORE") && *element != RefU64(9))));
        assert!(!batch.ops.iter().any(|op| matches!(op, MutationOp::SetAttrs { attrs, .. }
            if attrs.contains_key("LOCK"))));
    }

    #[test]
    fn existing_row_updates() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        let svc = PipeModelService::new(cat);
        let t = table(vec![row(2, &[("NAME", "/P1"), ("TYPE", "PIPE"), ("BORE", "150")])]);
        let (batch, report) = svc.import_rows(&ctx(), &t).unwrap();
        assert_eq!(report.results[0].status, RowStatus::Success);
        // EXISTS → 无 Create，直接 SetAttrs 到既有 refno。
        assert!(!batch.ops.iter().any(|op| matches!(op, MutationOp::Create { .. })));
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::SetAttrs { element, attrs }
                if *element == RefU64(10)
                    && attrs.get("BORE") == Some(&NamedAttrValue::F32Type(150.0)))));
    }

    #[test]
    fn unchecked_sides_skip_rows() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        let svc = PipeModelService::new(cat);
        let mut t = table(vec![
            row(2, &[("NAME", "/NEW-P"), ("TYPE", "PIPE")]),
            row(3, &[("NAME", "/P1"), ("TYPE", "PIPE")]),
        ]);
        t.create_new = false;
        t.update_existing = false;
        let (batch, report) = svc.import_rows(&ctx(), &t).unwrap();
        assert!(batch.ops.is_empty());
        assert_eq!(report.results[0].status, RowStatus::Valid);   // 可建但未勾选
        assert_eq!(report.results[1].status, RowStatus::Exists);  // 可改但未勾选
    }

    #[test]
    fn unset_and_empty_cells_skipped() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        let svc = PipeModelService::new(cat);
        let t = table(vec![row(2, &[
            ("NAME", "/P1"), ("TYPE", "PIPE"), ("BORE", "unset"),
        ])]);
        let (batch, _) = svc.import_rows(&ctx(), &t).unwrap();
        assert!(!batch.ops.iter().any(|op| matches!(op,
            MutationOp::SetAttrs { attrs, .. } if attrs.contains_key("BORE"))));
    }

    #[test]
    fn owner_missing_errors_row() {
        let cat = MockCatalog::new();
        let svc = PipeModelService::new(cat);
        let t = table(vec![row(2, &[
            ("NAME", "/X"), ("TYPE", "PIPE"), ("OWNER", "/NOPE"),
        ])]);
        let (_batch, report) = svc.import_rows(&ctx(), &t).unwrap();
        assert_eq!(report.results[0].status, RowStatus::Error);
    }
}
