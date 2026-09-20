//! 一个 refno 到底在哪个库文件里——按「谁的索引里有它」逐文件回答，并往回翻会话。
//!
//! 起因：YCYK-E3D 首次初始化时四个 BRAN 卡在 `generation_hold`，错误一律是
//! `db 7004 会话 3 的索引里没有 15196/40431` 这个形状。「索引里没有」是取数层在**一个**
//! 文件的**一个**会话上得到的结论，而定性要回答的是另外三件事：
//!
//! 1. 这个 Ref0 是不是真归那个库——别的文件有没有也认领它（Ref0 库归属冲突时，取数层挑
//!    中的那一份读不出元素，而元素其实在另一份里，报出来的话跟「元素不存在」一模一样）；
//! 2. 当前会话确实没有，那**更早的会话**有没有——有就是「目录件被删了、设计还指着它」，
//!    没有就是「设计从来指着一个不存在的号」，两者对甲方是两句不同的话；
//! 3. 顺带把 Ref0 在盘上到底有多少个元素报出来，好判断是整个库对不上还是单点缺件。
//!
//! 只读：不连 SurrealDB，不写任何东西，也不碰 `DbOption.toml`。
//!
//! ```text
//! cargo run --release --bin refno_locator_probe -- \
//!     --dir "D:/AVEVA/Projects/E3D3.1/YCYK-E3D/acw000" \
//!     --refno 15196/40431 --refno 16596/35338
//! ```

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Parser;
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use e3d_io::session::SessionSelector;

#[derive(Parser, Debug)]
#[command(name = "refno_locator_probe")]
#[command(about = "按索引回答「这个 refno 在哪个库文件、哪个会话里」")]
struct Args {
    /// 库文件目录，可重复。目录下每个打得开的文件都会被问一遍。
    #[arg(long, required = true)]
    dir: Vec<PathBuf>,
    /// 要找的参考号，`word0/word1` 或 `word0_word1`，可重复。
    #[arg(long, required = true)]
    refno: Vec<String>,
    /// 当前会话找不到时，往回翻多少个会话找它最后一次出现。0 = 不翻。
    #[arg(long, default_value_t = 40)]
    history: usize,
}

fn parse_refno(text: &str) -> anyhow::Result<RefNo> {
    let (a, b) = text
        .split_once('/')
        .or_else(|| text.split_once('_'))
        .ok_or_else(|| anyhow::anyhow!("参考号要写成 word0/word1 或 word0_word1：{text}"))?;
    Ok(RefNo::new(a.trim().parse()?, b.trim().parse()?))
}

/// 一个文件的索引里有什么，只走索引树、不读元素记录。
struct FileIndex {
    path: PathBuf,
    sesno: u32,
    /// 索引里出现过的全部 Ref0 → 该 Ref0 下的元素个数。
    ref0_counts: BTreeMap<u32, usize>,
    /// `RefNo` 只实现了 `Hash + Eq`，没有 `Ord`，所以这里是 hash 集不是有序集。
    refnos: HashSet<RefNo>,
}

fn read_index(path: &Path) -> anyhow::Result<FileIndex> {
    let mut engine = ReadOnlyEngine::open(path)?;
    let sesno = engine.session().sesno;
    let mut ref0_counts = BTreeMap::new();
    let mut refnos = HashSet::new();
    for (refno, _, _) in engine.indexed_refnos()? {
        *ref0_counts.entry(refno.word0).or_insert(0usize) += 1;
        refnos.insert(refno);
    }
    Ok(FileIndex {
        path: path.to_path_buf(),
        sesno,
        ref0_counts,
        refnos,
    })
}

/// 往回翻会话，报这个 refno 最后一次出现在哪个会话。
///
/// 逐会话开一次索引：会话页是 copy-on-write 的，每个会话带着当时的索引根，所以
/// 「在会话 N 存在吗」只有按那个根问才算数，不能拿最新的根去减。
fn last_seen_session(path: &Path, refno: RefNo, limit: usize) -> anyhow::Result<Option<u32>> {
    if limit == 0 {
        return Ok(None);
    }
    // `SessionInfo::session_id` 是 `sesno`——那个结构体里五个字段有三个名字是错的，
    // 只有 `session_id` 与 `end_page_no` 名副其实（e3d-io `session/mod.rs` 自己写着）。
    let sessions = ReadOnlyEngine::open(path)?.sessions()?;
    for info in sessions.iter().take(limit) {
        let mut engine =
            match ReadOnlyEngine::open_at(path, SessionSelector::Exact(info.session_id)) {
                Ok(engine) => engine,
                // 链上被 MERGE CHANGES 丢掉的会话开不出来，不是错误，跳过。
                Err(_) => continue,
            };
        if engine
            .indexed_refnos()?
            .iter()
            .any(|(candidate, _, _)| *candidate == refno)
        {
            return Ok(Some(info.session_id));
        }
    }
    Ok(None)
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let targets = args
        .refno
        .iter()
        .map(|text| parse_refno(text))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut files = Vec::new();
    for dir in &args.dir {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();

    let mut indexes = Vec::new();
    let mut unreadable = Vec::new();
    for path in &files {
        match read_index(path) {
            Ok(index) => indexes.push(index),
            Err(error) => unreadable.push((path.clone(), error.to_string())),
        }
    }
    println!(
        "扫了 {} 个文件：{} 个读得出索引，{} 个打不开（多半不是库文件）",
        files.len(),
        indexes.len(),
        unreadable.len()
    );

    for refno in targets {
        println!("\n=== {}/{} ===", refno.word0, refno.word1);
        let claimants: Vec<&FileIndex> = indexes
            .iter()
            .filter(|index| index.ref0_counts.contains_key(&refno.word0))
            .collect();
        if claimants.is_empty() {
            println!(
                "  Ref0 {} 在这些目录下**一个文件都没有**——不是「被删了」，是没有任何库\
                 认领这个 Ref0（对应 cata_closure 的 `no dbnum mapping`）",
                refno.word0
            );
            continue;
        }
        if claimants.len() > 1 {
            println!(
                "  ⚠ Ref0 {} 被 {} 个文件同时认领（库归属冲突，取数层挑中哪一份是不确定的）",
                refno.word0,
                claimants.len()
            );
        }
        for index in claimants {
            let count = index.ref0_counts[&refno.word0];
            let here = index.refnos.contains(&refno);
            println!(
                "  [{}] {}（会话 {}）：Ref0 下 {count} 个元素，这一个{}",
                if here { "有" } else { "无" },
                index.path.display(),
                index.sesno,
                if here {
                    "**在**索引里"
                } else {
                    "**不在**索引里"
                }
            );
            if here {
                continue;
            }
            match last_seen_session(&index.path, refno, args.history) {
                Ok(Some(sesno)) => {
                    println!("      往回翻：会话 {sesno} 还有它 —— 之后被删了，设计仍指着它")
                }
                Ok(None) => println!(
                    "      往回翻最近 {} 个会话：一次都没出现过 —— 设计指的是一个这个库从未有过的号",
                    args.history
                ),
                Err(error) => println!("      往回翻失败：{error}"),
            }
        }
    }

    if !unreadable.is_empty() {
        println!("\n打不开的（前 5 个）：");
        for (path, error) in unreadable.iter().take(5) {
            println!("  {} — {error}", path.display());
        }
    }
    Ok(())
}
