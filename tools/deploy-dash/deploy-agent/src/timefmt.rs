//! 时刻的格式：ISO 8601 带本地时区（`2026-11-12T19:20:03+08:00`）、日志行的 `HH:MM:SS.mmm`。
//! 不引 chrono：Windows 上用 `SystemTimeToTzSpecificLocalTime` 换成本地时间（按那一刻的时区规则，夏令时也对），
//! 时区偏移 = 本地时间 − UTC。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 本地时间的几个分量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
    /// 本地相对 UTC 的分钟数（东八区是 480）。
    pub offset_min: i32,
}

/// `2026-11-12T19:20:03+08:00`
pub fn iso(t: SystemTime) -> String {
    let c = local(t);
    let sign = if c.offset_min < 0 { '-' } else { '+' };
    let off = c.offset_min.unsigned_abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        c.year,
        c.month,
        c.day,
        c.hour,
        c.minute,
        c.second,
        off / 60,
        off % 60
    )
}

pub fn now_iso() -> String {
    iso(SystemTime::now())
}

/// 日志行的时刻 `15:32:07.120`（同面板 `winsys::local_time_hms`）。
pub fn hms_millis(t: SystemTime) -> String {
    let c = local(t);
    format!("{:02}:{:02}:{:02}.{:03}", c.hour, c.minute, c.second, c.millis)
}

pub fn now_hms() -> String {
    hms_millis(SystemTime::now())
}

/// `15:32:08`（同面板 `ui::format::clock`）。
pub fn clock(t: SystemTime) -> String {
    let c = local(t);
    format!("{:02}:{:02}:{:02}", c.hour, c.minute, c.second)
}

/// 文件名里用的紧凑时刻 `20261112-153208`。
pub fn stamp(t: SystemTime) -> String {
    let c = local(t);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", c.year, c.month, c.day, c.hour, c.minute, c.second)
}

/// 换成本地时间。
pub fn local(t: SystemTime) -> Civil {
    let since = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let utc = utc_civil(since);
    match sys::to_local(since) {
        Some(local) => local,
        None => utc,
    }
}

fn utc_civil(since: Duration) -> Civil {
    let secs = since.as_secs() as i64;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    Civil {
        year,
        month,
        day,
        hour: (rem / 3600) as u32,
        minute: (rem % 3600 / 60) as u32,
        second: (rem % 60) as u32,
        millis: since.subsec_millis(),
        offset_min: 0,
    }
}

/// Howard Hinnant 的 civil_from_days。
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Howard Hinnant 的 days_from_civil。
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::time::Duration;

    use super::{Civil, days_from_civil};

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        millis: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToSystemTime(ft: *const FileTime, st: *mut SystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(tz: *const c_void, utc: *const SystemTime, local: *mut SystemTime) -> i32;
    }

    /// 1601-01-01 到 1970-01-01 的 100 纳秒数。
    const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;

    pub fn to_local(since_unix: Duration) -> Option<Civil> {
        let ticks = since_unix.as_nanos() as u64 / 100 + EPOCH_DIFF_100NS;
        let ft = FileTime { low: ticks as u32, high: (ticks >> 32) as u32 };
        let mut utc = SystemTime::default();
        let mut loc = SystemTime::default();
        // SAFETY：两个结构体按 Win32 的布局声明，指针都指向栈上活着的变量。
        let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut loc) != 0 };
        if !ok {
            return None;
        }
        let minutes = |st: &SystemTime| days_from_civil(st.year as i64, st.month as u32, st.day as u32) * 1440 + st.hour as i64 * 60 + st.minute as i64;
        Some(Civil {
            year: loc.year as i64,
            month: loc.month as u32,
            day: loc.day as u32,
            hour: loc.hour as u32,
            minute: loc.minute as u32,
            second: loc.second as u32,
            millis: loc.millis as u32,
            offset_min: (minutes(&loc) - minutes(&utc)) as i32,
        })
    }
}

#[cfg(not(windows))]
mod sys {
    use std::time::Duration;

    use super::Civil;

    pub fn to_local(_since_unix: Duration) -> Option<Civil> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-719_468i64, -1, 0, 1, 10_957, 20_769, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2026, 11, 12), 20_769);
    }

    #[test]
    fn iso_has_offset_and_matches_utc() {
        // 2026-11-12T11:20:03Z
        let t = UNIX_EPOCH + Duration::from_secs(20_769 * 86_400 + 11 * 3600 + 20 * 60 + 3);
        let s = iso(t);
        assert_eq!(s.len(), 25, "{s}");
        assert_eq!(&s[10..11], "T");
        let c = local(t);
        let back = (days_from_civil(c.year, c.month, c.day) * 86_400 + (c.hour * 3600 + c.minute * 60 + c.second) as i64) - c.offset_min as i64 * 60;
        assert_eq!(back, 20_769 * 86_400 + 11 * 3600 + 20 * 60 + 3);
        assert_eq!(hms_millis(t + Duration::from_millis(120)).len(), 12);
    }
}
