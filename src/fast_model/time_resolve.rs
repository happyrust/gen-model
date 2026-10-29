//! 时间点解析（design §1.5）。
//!
//! - 时间点语法：[`parse_instant`] / [`parse_instant_text`]。
//! - 链序选取：[`select_latest_at`]，会话链（Legacy `generate` 的 `select_session_at`）
//!   与版本表（[`resolve_at_rows`]）共用。
//!
//! 语法与前端 `src/utils/versionScrub/timeResolve.ts::parseInstant` 相同：
//!
//! ```text
//! YYYY-MM-DD[T| ]HH:mm[:ss[.f{1,3}]][Z|±HH:MM]
//! ```
//!
//! - 日期分隔只认 `-`，日期与时刻之间只认大写 `T` 或一个空格，偏移只认大写 `Z` 或 `±HH:MM`。
//! - 小数秒只能跟在秒之后，1 到 3 位；解析结果**截到秒**（小数部分丢弃）。
//! - 日历与时刻按真实取值校验：`2026-02-30`、`24:00`、`:60` 秒、偏移 `+24:00` 都不合法。
//! - 不带偏移的时间点保留为 [`ParsedInstant::Floating`]，由调用方按 Chain_Order 中
//!   第一条带时间行的偏移解释（Requirement 8.1，design §1.5 第 4 点）；校验阶段还不知道那条行。

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};

/// 一个语法合法的时间点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedInstant {
    /// 文本自带 `Z` 或 `±HH:MM`。
    Offset(DateTime<FixedOffset>),
    /// 文本不带偏移：墙上时间，偏移待定。
    Floating(NaiveDateTime),
}

impl ParsedInstant {
    /// 给不带偏移的时间点补上 `fallback`；自带偏移的原样返回。
    pub fn with_fallback(self, fallback: FixedOffset) -> DateTime<FixedOffset> {
        match self {
            Self::Offset(at) => at,
            // FixedOffset 下本地时间与瞬时一一对应，`single()` 不会落空。
            Self::Floating(naive) => fallback
                .from_local_datetime(&naive)
                .single()
                .expect("fixed offset maps local time to exactly one instant"),
        }
    }
}

/// 解析时间点文本，不带偏移时保留为 [`ParsedInstant::Floating`]。语法不合法返回 `None`。
pub fn parse_instant_text(text: &str) -> Option<ParsedInstant> {
    let b = text.as_bytes();
    // 最短 `YYYY-MM-DDTHH:mm` = 16 字节。
    if b.len() < 16 {
        return None;
    }
    let year = digits(b, 0, 4)?;
    expect(b, 4, b'-')?;
    let month = digits(b, 5, 2)?;
    expect(b, 7, b'-')?;
    let day = digits(b, 8, 2)?;
    if !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let hour = digits(b, 11, 2)?;
    expect(b, 13, b':')?;
    let minute = digits(b, 14, 2)?;
    let mut pos = 16;

    let mut second = 0;
    if b.get(pos) == Some(&b':') {
        second = digits(b, pos + 1, 2)?;
        pos += 3;
        if b.get(pos) == Some(&b'.') {
            let start = pos + 1;
            let mut end = start;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            if !(1..=3).contains(&(end - start)) {
                return None;
            }
            pos = end;
        }
    }

    let offset = match b.get(pos) {
        None => None,
        Some(b'Z') => {
            pos += 1;
            Some(FixedOffset::east_opt(0)?)
        }
        Some(&sign @ (b'+' | b'-')) => {
            let oh = digits(b, pos + 1, 2)?;
            expect(b, pos + 3, b':')?;
            let om = digits(b, pos + 4, 2)?;
            if oh > 23 || om > 59 {
                return None;
            }
            pos += 6;
            let secs = (oh * 3600 + om * 60) as i32;
            Some(FixedOffset::east_opt(if sign == b'-' {
                -secs
            } else {
                secs
            })?)
        }
        Some(_) => return None,
    };
    if pos != b.len() {
        return None;
    }

    let date = NaiveDate::from_ymd_opt(year as i32, month, day)?;
    let time = NaiveTime::from_hms_opt(hour, minute, second)?;
    let naive = NaiveDateTime::new(date, time);
    Some(match offset {
        Some(offset) => ParsedInstant::Offset(offset.from_local_datetime(&naive).single()?),
        None => ParsedInstant::Floating(naive),
    })
}

/// design §1.5 的签名：不带偏移的时间点按 `fallback_offset` 解释。
///
/// `fallback_offset` 为 `None` 时按 UTC 解释。按链序规则，只有版本表一行带时间的都没有时
/// 才拿不到偏移，那时结果直接是 `NoResolvable`，不会拿这个时间点去比较。
pub fn parse_instant(
    text: &str,
    fallback_offset: Option<FixedOffset>,
) -> Option<DateTime<FixedOffset>> {
    let fallback = fallback_offset.unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC"));
    parse_instant_text(text).map(|parsed| parsed.with_fallback(fallback))
}

/// 版本表的一行：会话号与 Session_Time 原文（RFC 3339；没有时间为 `None`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInstant {
    pub sesno: u32,
    pub session_time: Option<String>,
}

/// 时间点解析失败的原因（Requirement 8.3、8.6、8.8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// 时间点文本不符合语法。
    InvalidFormat,
    /// 有带时间的行，但都晚于时间点。
    BeforeOldest,
    /// 一行带时间的都没有（含空表）。
    NoResolvable,
}

/// [`select_latest_at`] 的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum SelectAt<'a, T> {
    Found(&'a T),
    BeforeOldest,
    NoTimed,
}

/// 链序规则（design §1.5）：`items` 按 Chain_Order（sesno 升序）给出，从末尾往前
/// 逐行检查，跳过 `time_of` 为 `None` 的行，返回第一个时间 ≤ `at` 的行。
///
/// 双方截到秒、按 UTC 瞬时比较。结果是「带时间且 ≤ t 的行中最靠链头者」，不一定是
/// 时间最接近 t 的行：会话时间可以不单调（会话 1 常晚于其后整条链）。
pub fn select_latest_at<'a, T>(
    items: &'a [T],
    time_of: impl Fn(&T) -> Option<DateTime<FixedOffset>>,
    at: DateTime<FixedOffset>,
) -> SelectAt<'a, T> {
    let at_secs = at.timestamp();
    let mut any_timed = false;
    for item in items.iter().rev() {
        let Some(time) = time_of(item) else {
            continue;
        };
        any_timed = true;
        if time.timestamp() <= at_secs {
            return SelectAt::Found(item);
        }
    }
    if any_timed {
        SelectAt::BeforeOldest
    } else {
        SelectAt::NoTimed
    }
}

/// 在版本表行上解析时间点 `at`，返回命中行的 `(sesno, session_time 原文)`。
///
/// `rows` 按 sesno 升序。`at` 不带偏移时按 Chain_Order 中第一条带时间行的偏移解释；
/// 解不出的 `session_time` 按无时间行处理。语法检查先于一切：`at` 不合法总是
/// [`ResolveError::InvalidFormat`]，即使表是空的。
pub fn resolve_at_rows(rows: &[VersionInstant], at: &str) -> Result<(u32, String), ResolveError> {
    let parsed = parse_instant_text(at).ok_or(ResolveError::InvalidFormat)?;
    resolve_parsed_at_rows(rows, parsed)
}

/// [`resolve_at_rows`] 的已解析版本，供已经校验过时间点的调用方（如实时几何路由）使用。
pub fn resolve_parsed_at_rows(
    rows: &[VersionInstant],
    at: ParsedInstant,
) -> Result<(u32, String), ResolveError> {
    let time_of = |row: &VersionInstant| {
        row.session_time
            .as_deref()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
    };
    let Some(first_offset) = rows
        .iter()
        .find_map(|row| time_of(row).map(|t| *t.offset()))
    else {
        return Err(ResolveError::NoResolvable);
    };
    match select_latest_at(rows, time_of, at.with_fallback(first_offset)) {
        SelectAt::Found(row) => Ok((
            row.sesno,
            row.session_time.clone().expect("found row carries a time"),
        )),
        SelectAt::BeforeOldest => Err(ResolveError::BeforeOldest),
        SelectAt::NoTimed => Err(ResolveError::NoResolvable),
    }
}

fn digits(b: &[u8], start: usize, len: usize) -> Option<u32> {
    let slice = b.get(start..start + len)?;
    slice.iter().try_fold(0u32, |acc, &c| {
        c.is_ascii_digit().then(|| acc * 10 + u32::from(c - b'0'))
    })
}

fn expect(b: &[u8], at: usize, want: u8) -> Option<()> {
    (b.get(at) == Some(&want)).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset_of(text: &str) -> DateTime<FixedOffset> {
        match parse_instant_text(text) {
            Some(ParsedInstant::Offset(at)) => at,
            other => panic!("{text:?} → {other:?}"),
        }
    }

    fn floating_of(text: &str) -> NaiveDateTime {
        match parse_instant_text(text) {
            Some(ParsedInstant::Floating(at)) => at,
            other => panic!("{text:?} → {other:?}"),
        }
    }

    #[test]
    fn accepts_every_form_of_the_grammar() {
        assert_eq!(
            offset_of("2026-01-02T03:04:05Z").to_rfc3339(),
            "2026-01-02T03:04:05+00:00"
        );
        assert_eq!(
            offset_of("2026-01-02 03:04+08:00").to_rfc3339(),
            "2026-01-02T03:04:00+08:00"
        );
        assert_eq!(
            offset_of("2026-01-02T03:04:05.123-05:30").to_rfc3339(),
            "2026-01-02T03:04:05-05:30"
        );
        assert_eq!(
            floating_of("2026-06-01 10:00").to_string(),
            "2026-06-01 10:00:00"
        );
        assert_eq!(
            floating_of("2026-06-01T09:30:00.9").to_string(),
            "2026-06-01 09:30:00"
        );
    }

    #[test]
    fn truncates_fractional_seconds() {
        assert_eq!(
            offset_of("2026-05-01T08:00:00.999Z"),
            offset_of("2026-05-01T08:00:00Z")
        );
    }

    #[test]
    fn floating_instant_takes_the_fallback_offset() {
        let plus8 = FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(
            parse_instant("2026-06-01T09:30:00", Some(plus8))
                .unwrap()
                .to_rfc3339(),
            "2026-06-01T09:30:00+08:00"
        );
        // 自带偏移的不受 fallback 影响。
        assert_eq!(
            parse_instant("2026-06-01T09:30:00Z", Some(plus8))
                .unwrap()
                .to_rfc3339(),
            "2026-06-01T09:30:00+00:00"
        );
        assert_eq!(
            parse_instant("2026-06-01T09:30:00", None)
                .unwrap()
                .to_rfc3339(),
            "2026-06-01T09:30:00+00:00"
        );
    }

    #[test]
    fn rejects_text_outside_the_grammar() {
        for text in [
            "",
            "not-a-time",
            "2026-01-02",
            "2026-01-02T03",
            "2026-01-02T3:04",
            "2026/01/02T03:04",
            "2026-01-02t03:04",
            "2026-01-02  03:04",
            "2026-01-02T03:04Z ",
            " 2026-01-02T03:04Z",
            "2026-01-02T03:04z",
            "2026-01-02T03:04.5",
            "2026-01-02T03:04:05.",
            "2026-01-02T03:04:05.1234",
            "2026-01-02T03:04:05+08",
            "2026-01-02T03:04:05+0800",
            "2026-01-02T03:04:05+24:00",
            "2026-01-02T03:04:05+08:60",
            "2026-02-30T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-01-02T24:00:00Z",
            "2026-01-02T23:60:00Z",
            "2026-01-02T23:59:60Z",
            "+2026-01-02T03:04Z",
            "２０２６-01-02T03:04Z",
        ] {
            assert_eq!(parse_instant_text(text), None, "{text:?}");
            assert_eq!(parse_instant(text, None), None, "{text:?}");
        }
    }

    fn rows(items: &[(u32, Option<&str>)]) -> Vec<VersionInstant> {
        items
            .iter()
            .map(|&(sesno, time)| VersionInstant {
                sesno,
                session_time: time.map(str::to_string),
            })
            .collect()
    }

    fn hit(sesno: u32, time: &str) -> Result<(u32, String), ResolveError> {
        Ok((sesno, time.to_string()))
    }

    #[test]
    fn select_latest_at_walks_back_from_chain_head() {
        let items = [(1, 30), (2, 10), (3, 20), (4, 40)];
        let time_of =
            |&(_, secs): &(u32, i64)| DateTime::from_timestamp(secs, 0).map(|t| t.fixed_offset());
        let at = |secs| DateTime::from_timestamp(secs, 0).unwrap().fixed_offset();
        // 链序优先于最接近的时间：t=35 时会话 1（30）更近，但会话 3（20）在链上更靠后。
        assert_eq!(
            select_latest_at(&items, time_of, at(35)),
            SelectAt::Found(&(3, 20))
        );
        assert_eq!(
            select_latest_at(&items, time_of, at(40)),
            SelectAt::Found(&(4, 40))
        );
        assert_eq!(
            select_latest_at(&items, time_of, at(15)),
            SelectAt::Found(&(2, 10))
        );
        assert_eq!(
            select_latest_at(&items, time_of, at(9)),
            SelectAt::BeforeOldest
        );
        let empty: [(u32, i64); 0] = [];
        assert_eq!(select_latest_at(&empty, time_of, at(9)), SelectAt::NoTimed);
        assert_eq!(
            select_latest_at(&items, |_| None, at(100)),
            SelectAt::<(u32, i64)>::NoTimed
        );
    }

    #[test]
    fn select_latest_at_compares_at_second_precision() {
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap();
        let items = [t("2026-05-01T08:00:00.700Z")];
        // 同一秒内、小数部分更小的时间点仍算 ≥ 该行。
        assert_eq!(
            select_latest_at(&items, |x| Some(*x), t("2026-05-01T08:00:00.200Z")),
            SelectAt::Found(&items[0])
        );
        assert_eq!(
            select_latest_at(&items, |x| Some(*x), t("2026-05-01T07:59:59.999Z")),
            SelectAt::BeforeOldest
        );
    }

    #[test]
    fn resolve_at_rows_follows_chain_order() {
        let table = rows(&[
            (1, Some("2016-01-05T01:12:00+00:00")),
            (2, Some("2014-08-06T02:00:00+00:00")),
            (37, Some("2015-03-11T06:20:00+00:00")),
            (64, Some("2015-08-19T08:45:00+00:00")),
        ]);
        assert_eq!(
            resolve_at_rows(&table, "2026-09-08T11:00:00Z"),
            hit(64, "2015-08-19T08:45:00+00:00")
        );
        assert_eq!(
            resolve_at_rows(&table, "2015-06-01T00:00:00Z"),
            hit(37, "2015-03-11T06:20:00+00:00")
        );
        assert_eq!(
            resolve_at_rows(&table, "2014-01-01T00:00:00Z"),
            Err(ResolveError::BeforeOldest)
        );
    }

    #[test]
    fn resolve_at_rows_same_second_picks_largest_sesno() {
        let table = rows(&[
            (20, Some("2026-05-01T08:00:00+00:00")),
            (21, Some("2026-05-01T08:00:00+00:00")),
            (22, Some("2026-05-01T08:00:00+00:00")),
            (23, Some("2026-05-02T00:00:00+00:00")),
        ]);
        assert_eq!(
            resolve_at_rows(&table, "2026-05-01T08:00:00.999Z"),
            hit(22, "2026-05-01T08:00:00+00:00")
        );
        assert_eq!(
            resolve_at_rows(&table, "2026-05-01T07:59:59.999Z"),
            Err(ResolveError::BeforeOldest)
        );
    }

    #[test]
    fn resolve_at_rows_skips_untimed_rows() {
        let table = rows(&[
            (1, None),
            (2, Some("2026-07-01T00:00:00+00:00")),
            (3, None),
            (4, Some("2026-07-02T00:00:00+00:00")),
            (5, None),
        ]);
        assert_eq!(
            resolve_at_rows(&table, "2026-08-01T00:00:00Z"),
            hit(4, "2026-07-02T00:00:00+00:00")
        );
        assert_eq!(
            resolve_at_rows(&table, "2026-07-01T12:00:00Z"),
            hit(2, "2026-07-01T00:00:00+00:00")
        );
        assert_eq!(
            resolve_at_rows(&table, "2026-06-30T23:59:59Z"),
            Err(ResolveError::BeforeOldest)
        );
    }

    #[test]
    fn resolve_at_rows_floating_instant_uses_first_timed_row_offset() {
        let table = rows(&[
            (3, None),
            (4, Some("2026-06-01T09:00:00+08:00")),
            (5, Some("2026-06-01T10:00:00+08:00")),
        ]);
        assert_eq!(
            resolve_at_rows(&table, "2026-06-01T09:30:00"),
            hit(4, "2026-06-01T09:00:00+08:00")
        );
        assert_eq!(
            resolve_at_rows(&table, "2026-06-01T08:59:59"),
            Err(ResolveError::BeforeOldest)
        );
        // 偏移取链序中第一条带时间的行（UTC），不是最后一条（+08:00）：
        // 03:00 UTC = 11:00+08:00 ≥ 会话 2。
        let mixed = rows(&[
            (1, Some("2026-06-01T00:00:00Z")),
            (2, Some("2026-06-01T10:00:00+08:00")),
        ]);
        assert_eq!(
            resolve_at_rows(&mixed, "2026-06-01T03:00:00"),
            hit(2, "2026-06-01T10:00:00+08:00")
        );
    }

    #[test]
    fn resolve_at_rows_reports_no_resolvable_and_invalid_format() {
        let untimed = rows(&[(1, None), (2, None)]);
        assert_eq!(
            resolve_at_rows(&untimed, "2026-01-01T00:00:00Z"),
            Err(ResolveError::NoResolvable)
        );
        assert_eq!(
            resolve_at_rows(&untimed, "2026-01-01T00:00:00"),
            Err(ResolveError::NoResolvable)
        );
        assert_eq!(
            resolve_at_rows(&[], "2026-01-01T00:00:00Z"),
            Err(ResolveError::NoResolvable)
        );
        // 语法检查先于空表判断。
        assert_eq!(
            resolve_at_rows(&[], "not-a-time"),
            Err(ResolveError::InvalidFormat)
        );
        assert_eq!(
            resolve_at_rows(&untimed, "2026-02-30T00:00:00Z"),
            Err(ResolveError::InvalidFormat)
        );
    }

    /// 共享样例：`invalid-format` 用例的 `at` 必须被拒，其余用例的 `at` 必须能解析。
    #[test]
    fn shared_fixture_instants_parse_as_expected() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/fixtures/version-time-resolve-cases.json"
        );
        let text = std::fs::read_to_string(path).expect("read shared fixture");
        let fixture: serde_json::Value = serde_json::from_str(&text).expect("fixture json");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let at = case["at"].as_str().unwrap();
            let invalid = case["expect"]["error"].as_str() == Some("invalid-format");
            assert_eq!(
                parse_instant_text(at).is_none(),
                invalid,
                "case {name}: at={at:?}"
            );
        }
    }
}
