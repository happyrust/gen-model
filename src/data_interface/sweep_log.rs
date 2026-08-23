//! 一轮重扫的结论攒成一整块，只在它与上一块不同时才打。
//!
//! 稳态下 `PollWatcher` 每 30s 触发一次重扫，而重扫的结论——哪些库被项目优先级
//! 遮蔽、哪些被监听限定挡在外面、CATA 依赖清单是第几版——只要现场没人动文件就
//! 逐字不变。照直打的结果是每小时 840 行完全相同的话：真正的变化淹在自己的回声
//! 里，日志文件也涨得没道理。
//!
//! 所以这里把「一轮的结论」当成一个整体：内容变了就整块打出来（连同耗时），
//! 没变就一个字不打。为了不让沉默看起来像看门狗死了，每 [`DIGEST_EVERY`] 轮
//! 静默会出一行摘要，说清这段时间共重复了多少轮。
//!
//! 攒不到（没人开轮，比如手动路径直接调进来）就原样直打——退化后的行为与本模块
//! 引入前逐字相同，这是刻意的：任何一条走不到 `begin`/`finish` 的路径都不该因此
//! 丢日志。

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// 连续多少轮结论无变化才出一行摘要。30s 一轮 × 60 ≈ 半小时。
const DIGEST_EVERY: u64 = 60;

struct OpenRound {
    origin: String,
    lines: Vec<String>,
}

#[derive(Default)]
struct LastPrinted {
    /// `None` = 这个 origin 还没打过任何一块，首轮无条件打印。
    fingerprint: Option<u64>,
    /// 自上次真正打印以来，结论逐字相同的轮数。
    repeats: u64,
}

#[derive(Default)]
struct SweepLog {
    open: Option<OpenRound>,
    seen: HashMap<String, LastPrinted>,
}

fn state() -> &'static Mutex<SweepLog> {
    static STATE: OnceLock<Mutex<SweepLog>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, SweepLog> {
    state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 开一轮。同一时刻只会有一轮重扫在跑（启动重扫先于 `async_watch` 事件循环，
/// 循环内部又是串行的），真撞上了就以后来者为准——丢一轮日志远好过把两轮的
/// 结论混成一块、算出一个谁都对不上的指纹。
pub fn begin(origin: &str) {
    let mut log = lock();
    log.open = Some(OpenRound {
        origin: origin.to_string(),
        lines: Vec::new(),
    });
}

/// 记一条本轮结论。没开轮就直接打（见模块文档的退化约定）。
pub fn note(line: String) {
    let mut log = lock();
    match log.open.as_mut() {
        Some(round) => round.lines.push(line),
        None => {
            drop(log);
            println!("{line}");
        }
    }
}

/// 收一轮：与上一轮比对，变了就整块打出来，没变就攒着。
///
/// `elapsed` 只在真打的时候用得上——它每轮都不一样，因此不参与指纹，否则永远
/// 判「有变化」。
pub fn finish(origin: &str, elapsed: Duration) {
    let mut log = lock();
    let Some(round) = log.open.take() else {
        return;
    };
    // 撞轮了（`begin` 被后来者覆盖过）：这块攒的东西归谁说不清，照直打完拉倒。
    if round.origin != origin {
        drop(log);
        for line in round.lines {
            println!("{line}");
        }
        println!(
            "[{origin}] 重扫（重建队列）总耗时: {} 秒",
            elapsed.as_secs_f32()
        );
        return;
    }

    let mut hasher = DefaultHasher::new();
    round.lines.hash(&mut hasher);
    let fingerprint = hasher.finish();

    let seen = log.seen.entry(round.origin.clone()).or_default();
    if seen.fingerprint == Some(fingerprint) {
        seen.repeats += 1;
        let repeats = seen.repeats;
        drop(log);
        if repeats % DIGEST_EVERY == 0 {
            println!(
                "[{origin}] 连续 {repeats} 轮重扫结论无变化（最近一轮 {:.2}s）；\
                 上一次打印的那一块仍然成立，看门狗在跑。",
                elapsed.as_secs_f32()
            );
        }
        return;
    }

    let skipped = seen.repeats;
    seen.fingerprint = Some(fingerprint);
    seen.repeats = 0;
    drop(log);

    if skipped > 0 {
        println!("[{origin}] 重扫结论有变化（此前 {skipped} 轮与上一块逐字相同，未打印）：");
    }
    for line in round.lines {
        println!("{line}");
    }
    println!(
        "[{origin}] 重扫（重建队列）总耗时: {} 秒",
        elapsed.as_secs_f32()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 缓冲区是进程级的单个槽位，所以这些断言必须串成一条跑——分成几个
    /// `#[test]` 会被测试框架并发调度，互相把对方开的轮清掉。
    #[test]
    fn the_round_buffer_collapses_repeats_without_ever_swallowing_a_stray_line() {
        let origin = "unit-sweep-log";
        {
            let mut log = lock();
            log.open = None;
            log.seen.remove(origin);
        }

        // 没开轮 = 直接打，不进缓冲区。这是所有未走 begin/finish 的调用方的兜底。
        note("散装一行".to_string());
        assert!(lock().open.is_none(), "没开轮的行不该被攒起来");

        // 开了轮就只攒不打。
        begin(origin);
        note("甲".to_string());
        note("乙".to_string());
        assert_eq!(
            lock().open.as_ref().expect("轮还开着").lines,
            vec!["甲".to_string(), "乙".to_string()]
        );
        finish(origin, Duration::from_millis(10));
        let first = lock().seen[origin].fingerprint;
        assert!(first.is_some(), "首轮必须打印并记下指纹");
        assert_eq!(lock().seen[origin].repeats, 0);

        // 逐字相同的一轮只记账。
        begin(origin);
        note("甲".to_string());
        note("乙".to_string());
        finish(origin, Duration::from_millis(10));
        assert_eq!(lock().seen[origin].repeats, 1, "相同的一轮不该再打印");
        assert_eq!(lock().seen[origin].fingerprint, first);

        // 换一块内容立刻恢复打印，重复计数归零。
        begin(origin);
        note("丙".to_string());
        finish(origin, Duration::from_millis(10));
        let log = lock();
        let seen = &log.seen[origin];
        assert_eq!(seen.repeats, 0, "变化过后重新开始计数");
        assert_ne!(seen.fingerprint, first);
    }
}
