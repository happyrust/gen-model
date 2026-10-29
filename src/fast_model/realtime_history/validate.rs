//! 实时历史几何查询的入参校验（design §1.2 第 1、2 步）。
//!
//! handler 以 `Option<String>` 接全部参数、交给 [`validate_request`] 裁决，避免 axum `Query`
//! 反序列化失败时回 axum 自带的 400 体。一个请求同时有多处错时只报优先级最高的一条
//! （Requirement 6.16）：
//!
//! 1. 字段（Requirement 6.6）：缺 / 非法 `dbnum` → 缺 `unit_refno` → refno 格式错 → refno 所属库 ≠ `dbnum`。
//!    前两者是 400 `INVALID_FIELD`，后两者是 400 `INVALID_REFNO`，`detail.field` 写参数名。
//! 2. 选择器（Requirement 6.2）：`sesno` 与 `time` 恰好一个；`sesno` ∈ [1, 2³¹−1]；`time` 合
//!    [`parse_instant_text`] 的语法。否则 400 `INVALID_SELECTOR`，`detail.params` 列出无效参数名
//!    （都带或都不带时为 `["sesno", "time"]`）。
//!
//! 空串与只含空白的值：`dbnum` / `unit_refno` 按缺字段处理；`sesno` / `time` 按「带了但无效」处理。

use e3d_io::refno::RefNo;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::fast_model::time_resolve::{ParsedInstant, parse_instant_text};

/// `sesno` 上限 2³¹−1。
pub const MAX_SESNO: u32 = i32::MAX as u32;

/// 原样收下的查询参数。全部是 `Option<String>`，反序列化本身不会失败。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RealtimeQueryParams {
    pub dbnum: Option<String>,
    pub unit_refno: Option<String>,
    pub sesno: Option<String>,
    pub time: Option<String>,
}

/// 会话选择器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeSelector {
    Sesno(u32),
    /// 时间点；不带偏移时为 [`ParsedInstant::Floating`]，解析会话时按版本表链序中
    /// 第一条带时间行的偏移解释（design §1.5 第 4 点）。
    At(ParsedInstant),
}

/// 通过校验的请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedRequest {
    pub dbnum: u32,
    pub unit: RefNo,
    pub selector: RealtimeSelector,
}

/// 校验错误。HTTP 状态一律 400。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// 缺字段或字段值非法（`dbnum` / `unit_refno`）。
    InvalidField {
        field: &'static str,
        message: String,
    },
    /// refno 格式错，或所属库与 `dbnum` 不一致。
    InvalidRefno {
        field: &'static str,
        message: String,
    },
    /// 选择器无效；`params` 按 `sesno`、`time` 的顺序列出。
    InvalidSelector {
        params: Vec<&'static str>,
        message: String,
    },
}

impl RequestError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidField { .. } => "INVALID_FIELD",
            Self::InvalidRefno { .. } => "INVALID_REFNO",
            Self::InvalidSelector { .. } => "INVALID_SELECTOR",
        }
    }

    pub fn status(&self) -> u16 {
        400
    }

    pub fn message(&self) -> &str {
        match self {
            Self::InvalidField { message, .. }
            | Self::InvalidRefno { message, .. }
            | Self::InvalidSelector { message, .. } => message,
        }
    }

    /// 错误体里的 `detail`：字段错为 `{ "field": … }`，选择器错为 `{ "params": [ … ] }`。
    pub fn detail(&self) -> Value {
        match self {
            Self::InvalidField { field, .. } | Self::InvalidRefno { field, .. } => {
                json!({ "field": field })
            }
            Self::InvalidSelector { params, .. } => json!({ "params": params }),
        }
    }
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for RequestError {}

/// 按 Requirement 6.16 的优先级校验，返回第一条错误或通过校验的请求。
pub fn validate_request(params: &RealtimeQueryParams) -> Result<ValidatedRequest, RequestError> {
    let (dbnum, unit) = validate_fields(params)?;
    let selector = validate_selector(params.sesno.as_deref(), params.time.as_deref())?;
    Ok(ValidatedRequest {
        dbnum,
        unit,
        selector,
    })
}

fn validate_fields(params: &RealtimeQueryParams) -> Result<(u32, RefNo), RequestError> {
    let dbnum_text =
        present(params.dbnum.as_deref()).ok_or_else(|| RequestError::InvalidField {
            field: "dbnum",
            message: "missing required parameter dbnum".to_string(),
        })?;
    let dbnum = parse_positive(dbnum_text).ok_or_else(|| RequestError::InvalidField {
        field: "dbnum",
        message: format!("dbnum must be a positive integer, got {dbnum_text:?}"),
    })?;
    let refno_text =
        present(params.unit_refno.as_deref()).ok_or_else(|| RequestError::InvalidField {
            field: "unit_refno",
            message: "missing required parameter unit_refno".to_string(),
        })?;
    let unit = parse_refno(refno_text).ok_or_else(|| RequestError::InvalidRefno {
        field: "unit_refno",
        message: format!(
            "unit_refno must be two positive integers joined by '_' or '/', got {refno_text:?}"
        ),
    })?;
    if unit.dbno() != dbnum {
        return Err(RequestError::InvalidRefno {
            field: "unit_refno",
            message: format!(
                "unit_refno {refno_text} belongs to dbnum {}, not requested dbnum {dbnum}",
                unit.dbno()
            ),
        });
    }
    Ok((dbnum, unit))
}

fn validate_selector(
    sesno: Option<&str>,
    time: Option<&str>,
) -> Result<RealtimeSelector, RequestError> {
    match (sesno, time) {
        (Some(_), Some(_)) => Err(RequestError::InvalidSelector {
            params: vec!["sesno", "time"],
            message: "sesno and time are mutually exclusive".to_string(),
        }),
        (None, None) => Err(RequestError::InvalidSelector {
            params: vec!["sesno", "time"],
            message: "exactly one of sesno or time is required".to_string(),
        }),
        (Some(text), None) => parse_positive(text)
            .filter(|value| *value <= MAX_SESNO)
            .map(RealtimeSelector::Sesno)
            .ok_or_else(|| RequestError::InvalidSelector {
                params: vec!["sesno"],
                message: format!("sesno must be an integer in [1, {MAX_SESNO}], got {text:?}"),
            }),
        (None, Some(text)) => parse_instant_text(text)
            .map(RealtimeSelector::At)
            .ok_or_else(|| RequestError::InvalidSelector {
                params: vec!["time"],
                message: format!(
                    "time must match YYYY-MM-DD[T| ]HH:mm[:ss[.fff]][Z|±HH:MM], got {text:?}"
                ),
            }),
    }
}

/// 非空白值；空串或只含空白视为没带。
fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.trim().is_empty())
}

/// 十进制正整数（只认 ASCII 数字，可带前导零），落在 `u32` 内且 > 0。
fn parse_positive(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok().filter(|value| *value > 0)
}

/// 两个正整数以 `_` 或 `/` 连接，如 `24384_23257`、`24384/23257`。
fn parse_refno(text: &str) -> Option<RefNo> {
    let (word0, word1) = text.split_once(['_', '/'])?;
    Some(RefNo::new(parse_positive(word0)?, parse_positive(word1)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 24384 = 0x5F40：dbno = (24384 & 0x1FFF) | ((24384 >> 13) & 0x3E000) = 8000。
    const DBNUM: &str = "8000";
    const UNIT: &str = "24384_23257";

    fn params(
        dbnum: Option<&str>,
        unit_refno: Option<&str>,
        sesno: Option<&str>,
        time: Option<&str>,
    ) -> RealtimeQueryParams {
        RealtimeQueryParams {
            dbnum: dbnum.map(str::to_string),
            unit_refno: unit_refno.map(str::to_string),
            sesno: sesno.map(str::to_string),
            time: time.map(str::to_string),
        }
    }

    fn error_of(p: RealtimeQueryParams) -> RequestError {
        validate_request(&p).expect_err("expected a validation error")
    }

    #[test]
    fn accepts_sesno_selector_with_either_separator() {
        for refno in ["24384_23257", "24384/23257"] {
            let ok = validate_request(&params(Some(DBNUM), Some(refno), Some("42"), None))
                .expect("valid request");
            assert_eq!(ok.dbnum, 8000);
            assert_eq!(ok.unit, RefNo::new(24384, 23257));
            assert_eq!(ok.selector, RealtimeSelector::Sesno(42));
        }
    }

    #[test]
    fn accepts_sesno_bounds() {
        for (text, value) in [("1", 1), ("2147483647", MAX_SESNO)] {
            let ok = validate_request(&params(Some(DBNUM), Some(UNIT), Some(text), None)).unwrap();
            assert_eq!(ok.selector, RealtimeSelector::Sesno(value));
        }
    }

    #[test]
    fn accepts_time_selector_keeping_missing_offset_floating() {
        let ok = validate_request(&params(
            Some(DBNUM),
            Some(UNIT),
            None,
            Some("2026-06-01 09:30"),
        ))
        .unwrap();
        assert!(matches!(
            ok.selector,
            RealtimeSelector::At(ParsedInstant::Floating(_))
        ));
        let ok = validate_request(&params(
            Some(DBNUM),
            Some(UNIT),
            None,
            Some("2026-06-01T09:30:00+08:00"),
        ))
        .unwrap();
        assert!(matches!(
            ok.selector,
            RealtimeSelector::At(ParsedInstant::Offset(_))
        ));
    }

    #[test]
    fn missing_or_bad_dbnum_is_invalid_field() {
        for dbnum in [
            None,
            Some(""),
            Some("  "),
            Some("abc"),
            Some("0"),
            Some("-1"),
            Some("99999999999"),
        ] {
            let e = error_of(params(dbnum, Some(UNIT), Some("1"), None));
            assert_eq!(e.code(), "INVALID_FIELD", "dbnum={dbnum:?}");
            assert_eq!(e.status(), 400);
            assert_eq!(e.detail(), json!({ "field": "dbnum" }));
        }
    }

    #[test]
    fn missing_unit_refno_is_invalid_field() {
        for refno in [None, Some(""), Some(" ")] {
            let e = error_of(params(Some(DBNUM), refno, Some("1"), None));
            assert_eq!(e.code(), "INVALID_FIELD", "unit_refno={refno:?}");
            assert_eq!(e.detail(), json!({ "field": "unit_refno" }));
        }
    }

    #[test]
    fn malformed_refno_is_invalid_refno() {
        for refno in [
            "24384",
            "24384-23257",
            "24384_",
            "_23257",
            "0_23257",
            "24384_0",
            "24384_-5",
            "24384_23257_1",
            "24384/23257/1",
            "a_b",
            " 24384_23257",
            "24384_99999999999",
        ] {
            let e = error_of(params(Some(DBNUM), Some(refno), Some("1"), None));
            assert_eq!(e.code(), "INVALID_REFNO", "unit_refno={refno:?}");
            assert_eq!(e.status(), 400);
            assert_eq!(e.detail(), json!({ "field": "unit_refno" }));
        }
    }

    #[test]
    fn refno_from_another_db_is_invalid_refno() {
        let e = error_of(params(Some("8001"), Some(UNIT), Some("1"), None));
        assert_eq!(e.code(), "INVALID_REFNO");
        assert_eq!(e.detail(), json!({ "field": "unit_refno" }));
        assert!(e.message().contains("8000"), "{}", e.message());
    }

    #[test]
    fn selector_both_or_neither_names_both_params() {
        for (sesno, time) in [
            (Some("1"), Some("2026-01-01T00:00:00Z")),
            (None, None),
            (Some(""), Some("")),
        ] {
            let e = error_of(params(Some(DBNUM), Some(UNIT), sesno, time));
            assert_eq!(
                e.code(),
                "INVALID_SELECTOR",
                "sesno={sesno:?} time={time:?}"
            );
            assert_eq!(e.status(), 400);
            assert_eq!(e.detail(), json!({ "params": ["sesno", "time"] }));
        }
    }

    #[test]
    fn sesno_out_of_range_or_not_integer_names_sesno() {
        for sesno in [
            "0",
            "-1",
            "2147483648",
            "4294967296",
            "1.5",
            "abc",
            "",
            " 1",
            "+1",
        ] {
            let e = error_of(params(Some(DBNUM), Some(UNIT), Some(sesno), None));
            assert_eq!(e.code(), "INVALID_SELECTOR", "sesno={sesno:?}");
            assert_eq!(e.detail(), json!({ "params": ["sesno"] }));
        }
    }

    #[test]
    fn bad_time_names_time() {
        for time in [
            "",
            "not-a-time",
            "2026-01-02",
            "2026-02-30T00:00:00Z",
            "2026-01-02T24:00:00Z",
        ] {
            let e = error_of(params(Some(DBNUM), Some(UNIT), None, Some(time)));
            assert_eq!(e.code(), "INVALID_SELECTOR", "time={time:?}");
            assert_eq!(e.detail(), json!({ "params": ["time"] }));
        }
    }

    /// Requirement 6.16：字段错误压过选择器错误；字段内按 dbnum → 缺 refno → refno 格式 → 库不一致。
    #[test]
    fn reports_only_the_highest_priority_error() {
        // 缺 dbnum 且 refno 格式错、选择器也错 → INVALID_FIELD dbnum。
        let e = error_of(params(None, Some("garbage"), None, None));
        assert_eq!(
            (e.code(), e.detail()),
            ("INVALID_FIELD", json!({ "field": "dbnum" }))
        );
        // dbnum 合法、缺 refno、选择器错 → INVALID_FIELD unit_refno。
        let e = error_of(params(Some(DBNUM), None, Some("0"), Some("x")));
        assert_eq!(
            (e.code(), e.detail()),
            ("INVALID_FIELD", json!({ "field": "unit_refno" }))
        );
        // refno 格式错、选择器错 → INVALID_REFNO。
        let e = error_of(params(Some(DBNUM), Some("garbage"), Some("0"), None));
        assert_eq!(e.code(), "INVALID_REFNO");
        // 库不一致、选择器两个都带 → INVALID_REFNO。
        let e = error_of(params(
            Some("8001"),
            Some(UNIT),
            Some("1"),
            Some("2026-01-01T00:00:00Z"),
        ));
        assert_eq!(e.code(), "INVALID_REFNO");
        // 字段全对 → 才轮到选择器。
        let e = error_of(params(Some(DBNUM), Some(UNIT), Some("0"), None));
        assert_eq!(e.code(), "INVALID_SELECTOR");
    }

    #[test]
    fn params_deserialize_from_any_string_values_without_failing() {
        let p: RealtimeQueryParams = serde_json::from_value(json!({
            "dbnum": "abc",
            "unit_refno": "24384/23257",
            "sesno": "",
            "extra": "1",
        }))
        .expect("all-Option<String> params never fail to deserialize");
        assert_eq!(p.dbnum.as_deref(), Some("abc"));
        assert_eq!(p.unit_refno.as_deref(), Some("24384/23257"));
        assert_eq!(p.sesno.as_deref(), Some(""));
        assert_eq!(p.time, None);
    }

    // Feature: model-version-timeline-scrub, Property 33: 请求校验与错误优先级（后端）
    // **Validates: Requirements 6.2, 6.6, 6.16**
    //
    // 生成器给每个参数带上「缺失 / 合法 / 非法」标签，参考判定只看标签与
    // `RefNo::new(..).dbno()`，不复用被测的解析函数。
    mod prop_request_validation {
        use proptest::prelude::*;

        use super::*;

        /// 生成器给出的参数分类。
        #[derive(Debug, Clone)]
        enum DbnumCase {
            Missing(Option<String>),
            Invalid(String),
            /// 合法，取值等于 refno 的库号（refno 不合法时取 `fallback`）；`zeros` 为前导零个数。
            MatchRefno {
                zeros: usize,
                fallback: u32,
            },
            Other {
                text: String,
                value: u32,
            },
        }

        #[derive(Debug, Clone)]
        enum RefnoCase {
            Missing(Option<String>),
            Malformed(String),
            Valid { text: String, refno: RefNo },
        }

        #[derive(Debug, Clone)]
        enum SesnoCase {
            Absent,
            Invalid(String),
            Valid { text: String, value: u32 },
        }

        #[derive(Debug, Clone)]
        enum TimeCase {
            Absent,
            Invalid(String),
            Valid { text: String, has_offset: bool },
        }

        #[derive(Debug, Clone)]
        struct Case {
            dbnum: DbnumCase,
            refno: RefnoCase,
            sesno: SesnoCase,
            time: TimeCase,
        }

        impl Case {
            fn dbnum_value(&self) -> Option<u32> {
                match &self.dbnum {
                    DbnumCase::Missing(_) | DbnumCase::Invalid(_) => None,
                    DbnumCase::MatchRefno { fallback, .. } => Some(match &self.refno {
                        RefnoCase::Valid { refno, .. } => refno.dbno(),
                        _ => *fallback,
                    }),
                    DbnumCase::Other { value, .. } => Some(*value),
                }
            }

            fn to_params(&self) -> RealtimeQueryParams {
                let dbnum = match &self.dbnum {
                    DbnumCase::Missing(text) => text.clone(),
                    DbnumCase::Invalid(text) => Some(text.clone()),
                    DbnumCase::MatchRefno { zeros, .. } => Some(format!(
                        "{}{}",
                        "0".repeat(*zeros),
                        self.dbnum_value().unwrap()
                    )),
                    DbnumCase::Other { text, .. } => Some(text.clone()),
                };
                let unit_refno = match &self.refno {
                    RefnoCase::Missing(text) => text.clone(),
                    RefnoCase::Malformed(text) | RefnoCase::Valid { text, .. } => {
                        Some(text.clone())
                    }
                };
                let sesno = match &self.sesno {
                    SesnoCase::Absent => None,
                    SesnoCase::Invalid(text) | SesnoCase::Valid { text, .. } => Some(text.clone()),
                };
                let time = match &self.time {
                    TimeCase::Absent => None,
                    TimeCase::Invalid(text) | TimeCase::Valid { text, .. } => Some(text.clone()),
                };
                RealtimeQueryParams {
                    dbnum,
                    unit_refno,
                    sesno,
                    time,
                }
            }
        }

        /// 期望结果：错误码 + detail，或通过时的 (dbnum, refno, sesno 值 / 时间是否带偏移)。
        #[derive(Debug, PartialEq)]
        enum Expected {
            Err(&'static str, Value),
            OkSesno(u32, RefNo, u32),
            OkTime(u32, RefNo, bool),
        }

        /// 参考判定：缺/非法 dbnum → 缺 unit_refno → refno 格式错 → 库不一致 → 选择器。
        fn reference(case: &Case) -> Expected {
            let Some(dbnum) = case.dbnum_value() else {
                return Expected::Err("INVALID_FIELD", json!({ "field": "dbnum" }));
            };
            let refno = match &case.refno {
                RefnoCase::Missing(_) => {
                    return Expected::Err("INVALID_FIELD", json!({ "field": "unit_refno" }));
                }
                RefnoCase::Malformed(_) => {
                    return Expected::Err("INVALID_REFNO", json!({ "field": "unit_refno" }));
                }
                RefnoCase::Valid { refno, .. } => *refno,
            };
            if refno.dbno() != dbnum {
                return Expected::Err("INVALID_REFNO", json!({ "field": "unit_refno" }));
            }
            match (&case.sesno, &case.time) {
                (SesnoCase::Absent, TimeCase::Absent)
                | (SesnoCase::Invalid(_) | SesnoCase::Valid { .. }, TimeCase::Invalid(_))
                | (SesnoCase::Invalid(_) | SesnoCase::Valid { .. }, TimeCase::Valid { .. }) => {
                    Expected::Err("INVALID_SELECTOR", json!({ "params": ["sesno", "time"] }))
                }
                (SesnoCase::Invalid(_), TimeCase::Absent) => {
                    Expected::Err("INVALID_SELECTOR", json!({ "params": ["sesno"] }))
                }
                (SesnoCase::Absent, TimeCase::Invalid(_)) => {
                    Expected::Err("INVALID_SELECTOR", json!({ "params": ["time"] }))
                }
                (SesnoCase::Valid { value, .. }, TimeCase::Absent) => {
                    Expected::OkSesno(dbnum, refno, *value)
                }
                (SesnoCase::Absent, TimeCase::Valid { has_offset, .. }) => {
                    Expected::OkTime(dbnum, refno, *has_offset)
                }
            }
        }

        fn actual(params: &RealtimeQueryParams) -> Expected {
            match validate_request(params) {
                Err(e) => {
                    assert_eq!(e.status(), 400);
                    Expected::Err(e.code(), e.detail())
                }
                Ok(ok) => match ok.selector {
                    RealtimeSelector::Sesno(n) => Expected::OkSesno(ok.dbnum, ok.unit, n),
                    RealtimeSelector::At(at) => {
                        Expected::OkTime(ok.dbnum, ok.unit, matches!(at, ParsedInstant::Offset(_)))
                    }
                },
            }
        }

        // ---- 文本生成 ----

        fn missing_text() -> impl Strategy<Value = Option<String>> {
            prop_oneof![
                Just(None),
                Just(Some(String::new())),
                "[ \t]{1,3}".prop_map(Some),
            ]
        }

        /// 十进制正整数文本，可带前导零。
        fn positive_text(
            range: std::ops::RangeInclusive<u32>,
        ) -> impl Strategy<Value = (String, u32)> {
            (range, 0usize..3).prop_map(|(n, zeros)| (format!("{}{}", "0".repeat(zeros), n), n))
        }

        /// 非空白、但不是 u32 内正整数的文本；不含 `_` 与 `/`。
        fn non_positive_text() -> impl Strategy<Value = String> {
            prop_oneof![
                "0{1,4}",
                (1u32..=u32::MAX).prop_map(|n| format!("-{n}")),
                (1u32..=u32::MAX).prop_map(|n| format!("+{n}")),
                (1u32..=u32::MAX).prop_map(|n| format!(" {n}")),
                (u64::from(u32::MAX) + 1..=u64::MAX).prop_map(|n| n.to_string()),
                "[a-z]{1,6}",
                "[0-9]{1,4}[.a-z+ ][0-9]{0,3}",
            ]
        }

        fn dbnum_case() -> impl Strategy<Value = DbnumCase> {
            prop_oneof![
                1 => missing_text().prop_map(DbnumCase::Missing),
                1 => non_positive_text().prop_map(DbnumCase::Invalid),
                4 => (0usize..3, 1u32..=u32::MAX)
                    .prop_map(|(zeros, fallback)| DbnumCase::MatchRefno { zeros, fallback }),
                1 => positive_text(1..=u32::MAX)
                    .prop_map(|(text, value)| DbnumCase::Other { text, value }),
            ]
        }

        fn refno_case() -> impl Strategy<Value = RefnoCase> {
            let word0 = prop_oneof![1u32..=100_000, 1u32..=u32::MAX]
                .prop_filter("refno 库号须为正", |w0| RefNo::new(*w0, 1).dbno() != 0);
            let valid = (
                word0,
                positive_text(1..=u32::MAX),
                prop_oneof![Just('_'), Just('/')],
            )
                .prop_map(|(w0, (w1_text, w1), sep)| RefnoCase::Valid {
                    text: format!("{w0}{sep}{w1_text}"),
                    refno: RefNo::new(w0, w1),
                });
            let n = 1u32..=u32::MAX;
            let sep = prop_oneof![Just('_'), Just('/')];
            let malformed = prop_oneof![
                n.clone().prop_map(|a| a.to_string()),
                (n.clone(), n.clone()).prop_map(|(a, b)| format!("{a}-{b}")),
                (n.clone(), sep.clone()).prop_map(|(a, s)| format!("{a}{s}")),
                (n.clone(), sep.clone()).prop_map(|(b, s)| format!("{s}{b}")),
                (non_positive_text(), sep.clone(), n.clone())
                    .prop_map(|(bad, s, b)| format!("{bad}{s}{b}")),
                (n.clone(), sep.clone(), non_positive_text())
                    .prop_map(|(a, s, bad)| format!("{a}{s}{bad}")),
                (n.clone(), sep.clone(), n.clone(), sep.clone(), n.clone())
                    .prop_map(|(a, s1, b, s2, c)| format!("{a}{s1}{b}{s2}{c}")),
                (n.clone(), sep, n).prop_map(|(a, s, b)| format!(" {a}{s}{b}")),
            ];
            prop_oneof![
                1 => missing_text().prop_map(RefnoCase::Missing),
                2 => malformed.prop_map(RefnoCase::Malformed),
                5 => valid,
            ]
        }

        fn sesno_case() -> impl Strategy<Value = SesnoCase> {
            prop_oneof![
                2 => Just(SesnoCase::Absent),
                2 => prop_oneof![
                    non_positive_text(),
                    (u64::from(MAX_SESNO) + 1..=u64::from(u32::MAX)).prop_map(|n| n.to_string()),
                    Just(String::new()),
                    "[ ]{1,2}",
                ]
                .prop_map(SesnoCase::Invalid),
                3 => positive_text(1..=MAX_SESNO)
                    .prop_map(|(text, value)| SesnoCase::Valid { text, value }),
            ]
        }

        /// 合法时间点：`YYYY-MM-DD[T| ]HH:mm[:ss[.f{1,3}]][Z|±HH:MM]`，日 ≤ 28 保证日历有效。
        fn valid_time() -> impl Strategy<Value = (String, bool)> {
            let date = (1970u32..=2099, 1u32..=12, 1u32..=28);
            let clock = (0u32..=23, 0u32..=59);
            let seconds = proptest::option::of((0u32..=59, proptest::option::of("[0-9]{1,3}")));
            let offset = prop_oneof![
                Just(None),
                Just(Some("Z".to_string())),
                (prop_oneof![Just('+'), Just('-')], 0u32..=23, 0u32..=59)
                    .prop_map(|(s, h, m)| Some(format!("{s}{h:02}:{m:02}"))),
            ];
            (
                date,
                prop_oneof![Just('T'), Just(' ')],
                clock,
                seconds,
                offset,
            )
                .prop_map(|((y, mo, d), sep, (h, mi), seconds, offset)| {
                    let mut text = format!("{y:04}-{mo:02}-{d:02}{sep}{h:02}:{mi:02}");
                    if let Some((s, frac)) = seconds {
                        text.push_str(&format!(":{s:02}"));
                        if let Some(frac) = frac {
                            text.push('.');
                            text.push_str(&frac);
                        }
                    }
                    let has_offset = offset.is_some();
                    if let Some(offset) = offset {
                        text.push_str(&offset);
                    }
                    (text, has_offset)
                })
        }

        fn invalid_time() -> impl Strategy<Value = String> {
            let y = 1970u32..=2099;
            prop_oneof![
                Just(String::new()),
                "[a-z ]{0,12}",
                y.clone().prop_map(|y| format!("{y:04}-01-02")),
                y.clone().prop_map(|y| format!("{y:04}-02-30T00:00:00Z")),
                y.clone().prop_map(|y| format!("{y:04}-13-01T00:00:00Z")),
                y.clone().prop_map(|y| format!("{y:04}-01-02T24:00:00Z")),
                y.clone().prop_map(|y| format!("{y:04}-01-02T10:60")),
                y.clone().prop_map(|y| format!("{y:04}-01-02T10:00:60")),
                y.clone()
                    .prop_map(|y| format!("{y:04}-01-02T10:00:00.1234")),
                y.clone().prop_map(|y| format!("{y:04}-01-02T10:00.5")),
                y.clone().prop_map(|y| format!("{y:04}-01-02t10:00")),
                y.clone().prop_map(|y| format!("{y:04}/01/02 10:00")),
                y.clone().prop_map(|y| format!("{y:04}-01-02T10:00+24:00")),
                y.clone().prop_map(|y| format!("{y:04}-01-02T10:00z")),
                valid_time().prop_map(|(t, _)| format!("{t}x")),
                valid_time().prop_map(|(t, _)| format!(" {t}")),
            ]
        }

        fn time_case() -> impl Strategy<Value = TimeCase> {
            prop_oneof![
                2 => Just(TimeCase::Absent),
                2 => invalid_time().prop_map(TimeCase::Invalid),
                3 => valid_time().prop_map(|(text, has_offset)| TimeCase::Valid { text, has_offset }),
            ]
        }

        fn case() -> impl Strategy<Value = Case> {
            (dbnum_case(), refno_case(), sesno_case(), time_case()).prop_map(
                |(dbnum, refno, sesno, time)| Case {
                    dbnum,
                    refno,
                    sesno,
                    time,
                },
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(100))]

            #[test]
            fn validation_reports_highest_priority_error(case in case()) {
                let params = case.to_params();
                prop_assert_eq!(actual(&params), reference(&case), "params = {:?}", params);
            }
        }
    }
}
