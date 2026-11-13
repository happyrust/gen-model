//! 数字、时长、时刻的格式。规则照 skill：没有数据写「—」，不写假的 0；精度够决策用就行。
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const NONE: &str = "—";

pub fn gb(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0} GB")
    } else {
        format!("{v:.1} GB")
    }
}

pub fn pct(v: f64) -> String {
    if v > 0.0 && v < 1.0 { "<1%".into() } else { format!("{v:.0}%") }
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 「2 小时 14 分」「3 分 20 秒」「45 秒」「1 天 3 小时」。
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 86_400 {
        format!("{} 天 {} 小时", s / 86_400, s % 86_400 / 3600)
    } else if s >= 3600 {
        format!("{} 小时 {} 分", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{} 分 {} 秒", s / 60, s % 60)
    } else {
        format!("{s} 秒")
    }
}

/// 表格里放不下时的短写：「2时14分」「3分20秒」「45秒」。
pub fn duration_short(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 86_400 {
        format!("{}天{}时", s / 86_400, s % 86_400 / 3600)
    } else if s >= 3600 {
        format!("{}时{}分", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{}分{}秒", s / 60, s % 60)
    } else {
        format!("{s}秒")
    }
}

pub fn secs1(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 { format!("{s:.0} 秒") } else { duration(d) }
}

/// 本地时区相对 UTC 的秒数（GetLocalTime 和 GetSystemTime 的差，算一次）。
fn local_offset_secs() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(crate::winsys::local_utc_offset_secs)
}

fn civil(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + local_offset_secs();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant 的 civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32, (rem % 60) as u32)
}

/// 「15:32:08」
pub fn clock(t: SystemTime) -> String {
    let (_, _, _, h, mi, s) = civil(t);
    format!("{h:02}:{mi:02}:{s:02}")
}

/// 「11-12 13:18:02」
pub fn date_clock(t: SystemTime) -> String {
    let (_, m, d, h, mi, s) = civil(t);
    format!("{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(thousands(182_406), "182,406");
        assert_eq!(thousands(999), "999");
        assert_eq!(duration(Duration::from_secs(2 * 3600 + 14 * 60 + 5)), "2 小时 14 分");
        assert_eq!(duration(Duration::from_secs(119)), "1 分 59 秒");
        assert_eq!(gb(11.98), "12.0 GB");
        assert_eq!(pct(0.4), "<1%");
    }
}
