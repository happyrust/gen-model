use aios_core::RefnoEnum;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::Serialize;
use std::backtrace::Backtrace;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// RefNo 错误类型
#[derive(Debug, Clone, Copy, Serialize, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum RefnoErrorKind {
    FormatInvalid,
    Duplicate,
    Missing,
    NotFound,
    TypeMismatch,
    ConversionFailed,
    ZeroOrNegative,
    RelationOrphan,
    OwnerMissing,
    Unexpected,
}

/// RefNo 错误所在阶段
#[derive(Debug, Clone, Copy, Serialize, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum RefnoErrorStage {
    InputParse,
    Query,
    Build,
    Relation,
    Export,
}

/// 单条错误记录
#[derive(Debug, Clone, Serialize)]
pub struct RefnoErrorRecord {
    pub timestamp: DateTime<Utc>,
    pub kind: RefnoErrorKind,
    pub stage: RefnoErrorStage,
    pub module: &'static str,
    pub operation: &'static str,
    pub refno_raw: Option<String>,
    pub refno: Option<String>,
    pub related_refnos: Vec<String>,
    pub db_num: Option<u32>,
    pub message: String,
    pub backtrace: String,
}

/// 错误统计摘要
#[derive(Debug, Clone, Serialize)]
pub struct RefnoErrorSummary {
    pub total: usize,
    pub by_kind: BTreeMap<RefnoErrorKind, usize>,
    pub by_stage: BTreeMap<RefnoErrorStage, usize>,
}

/// 全局错误存储
pub static REFNO_ERROR_STORE: Lazy<RefnoErrorStore> =
    Lazy::new(|| RefnoErrorStore::new("logs/refno_errors.jsonl", 2000));

/// 可复用的错误存储实现
pub struct RefnoErrorStore {
    records: Mutex<Vec<RefnoErrorRecord>>,
    kind_count: DashMap<RefnoErrorKind, usize>,
    stage_count: DashMap<RefnoErrorStage, usize>,
    max_records: usize,
    log_path: PathBuf,
}

impl RefnoErrorStore {
    pub fn new(log_path: impl Into<PathBuf>, max_records: usize) -> Self {
        Self {
            records: Mutex::new(Vec::with_capacity(max_records.min(1024))),
            kind_count: DashMap::new(),
            stage_count: DashMap::new(),
            max_records,
            log_path: log_path.into(),
        }
    }

    pub fn record(&self, record: RefnoErrorRecord) {
        {
            let mut guard = self.records.lock().expect("record mutex poisoned");
            if guard.len() >= self.max_records {
                guard.remove(0);
            }
            guard.push(record.clone());
        }
        *self.kind_count.entry(record.kind).or_default() += 1;
        *self.stage_count.entry(record.stage).or_default() += 1;
        self.persist(&record);
    }

    pub fn recent(&self, limit: usize) -> Vec<RefnoErrorRecord> {
        let guard = self.records.lock().expect("record mutex poisoned");
        guard.iter().rev().take(limit).cloned().collect()
    }

    pub fn summary(&self) -> RefnoErrorSummary {
        let mut by_kind = BTreeMap::new();
        for entry in self.kind_count.iter() {
            by_kind.insert(*entry.key(), *entry.value());
        }

        let mut by_stage = BTreeMap::new();
        for entry in self.stage_count.iter() {
            by_stage.insert(*entry.key(), *entry.value());
        }

        let total = by_kind.values().copied().sum();

        RefnoErrorSummary {
            total,
            by_kind,
            by_stage,
        }
    }

    fn persist(&self, record: &RefnoErrorRecord) {
        if let Some(parent) = self.log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            if let Ok(line) = serde_json::to_string(record) {
                let _ = writeln!(file, "{}", line);
            }
        }
    }
}

fn refno_to_string(refno: &RefnoEnum) -> String {
    format!("{:?}", refno)
}

/// 记录一条 RefNo 错误，并自动捕获堆栈
#[allow(clippy::too_many_arguments)]
pub fn record_refno_error(
    kind: RefnoErrorKind,
    stage: RefnoErrorStage,
    module: &'static str,
    operation: &'static str,
    message: impl Into<String>,
    refno: Option<&RefnoEnum>,
    refno_raw: Option<&str>,
    related_refnos: &[RefnoEnum],
    db_num: Option<u32>,
) {
    let record = RefnoErrorRecord {
        timestamp: Utc::now(),
        kind,
        stage,
        module,
        operation,
        refno_raw: refno_raw.map(|s| s.to_string()),
        refno: refno.map(refno_to_string),
        related_refnos: related_refnos.iter().map(refno_to_string).collect(),
        db_num,
        message: message.into(),
        backtrace: Backtrace::force_capture().to_string(),
    };

    REFNO_ERROR_STORE.record(record);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_summarize() {
        let store = RefnoErrorStore::new(
            std::env::temp_dir().join("refno_errors_test.jsonl"),
            16,
        );

        let sample_refno = RefnoEnum::Refno(aios_core::RefU64(1));
        store.record(RefnoErrorRecord {
            timestamp: Utc::now(),
            kind: RefnoErrorKind::Duplicate,
            stage: RefnoErrorStage::InputParse,
            module: "test",
            operation: "insert",
            refno_raw: Some("1".to_string()),
            refno: Some(refno_to_string(&sample_refno)),
            related_refnos: vec![],
            db_num: Some(1),
            message: "dup".to_string(),
            backtrace: String::new(),
        });

        let summary = store.summary();
        assert_eq!(summary.total, 1);
        assert_eq!(
            *summary.by_kind.get(&RefnoErrorKind::Duplicate).unwrap_or(&0),
            1
        );
        assert_eq!(
            *summary
                .by_stage
                .get(&RefnoErrorStage::InputParse)
                .unwrap_or(&0),
            1
        );
    }
}
