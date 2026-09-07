//! Q5 度量：把一个 MDB 声明的全部 DESI 用 e3d-io 装进内存，要多久、占多少。
//!
//! 起因是「启动 MDB 初始化改成 e3d-io → 内存、库降为备份」这条路上的第一个未知数。
//! 今天的 `init_mdb`（`db_model.rs:392`）只从 SYS 文件解出成员名单，**一个元素都不载入**；
//! 元素数据的唯一落点是 SurrealDB 的 `pe` / `ATT_*`。要把它搬进内存，先得知道各档粒度
//! 的价钱——而这几档正对应「内存里到底装什么」的几种选法：
//!
//! | 档 | 装的东西 | 今天谁已经这么干 |
//! |---|---|---|
//! | `index` | `indexed_refnos()`：refno 清单 + ref0→dbnum 归属 | `direct_tree.rs` 建 `InMemoryCataLocator`（懒的，第一次树请求才建） |
//! | `header` | 每元素 noun + owner（`ScanTier::Header`） | `MdbMemoryStore` 今天就是这一档 |
//! | `named` | 加上存储的 `NAME`（`ScanTier::Named`） | 没人。树面板显示的那一格、生成根回执上的 `name` 都是它 |
//! | `extract` | 每元素整张属性表（`NamedAttrMap`） | 没人，**也不打算做**——量它是为了把「全属性常驻」这条路钉死在数字上 |
//!
//! 外加一档 `owner-parity`，它不量东西、只对账：逐元素比**记录头自报的属主**与**属性表里
//! 那个 `OWNER`**。`MdbMemoryStore` 的祖先链走的是前者、`DirectStore` 的属性走的是后者，
//! 两者要是能不一样，选根就会悄悄落在另一个祖先上——而那不报错。
//!
//! # 三条量法纪律
//!
//! 1. **一次只跑一档，分进程跑**（`--tier`）。叠着跑量不准：第二档的 RSS 增量里混着第一档
//!    已经不用、但分配器还没还给系统的那部分，而那笔账没有任何办法拆开。
//! 2. **结果留在内存里直到量完 RSS**。中途释放量到的是峰值退潮之后的水位，不是常驻成本
//!    ——而「常驻多少」正是 Q1 要的那个数。
//! 3. **不开 `DbSet::with_element_cache()`**。全库一次读，每个元素恰好读一遍，池级缓存买不到
//!    任何命中、只多一份拷贝。ADR-059 R1 自己写着「长驻进程按库全量读别开这个开关」。
//!
//! 报的是**串行**墙钟。并行下界另报一行「最慢的单库」——库之间互不依赖，真做起来按库并行是
//! 自然的，但那时的瓶颈就是那一个库，不是总和。
//!
//! ```text
//! cargo run --release --bin mdb_memory_probe -- --tier index
//! cargo run --release --bin mdb_memory_probe -- --tier header
//! cargo run --release --bin mdb_memory_probe -- --tier named
//! cargo run --release --bin mdb_memory_probe -- --tier extract --json out/q5-extract.json
//! ```
//!
//! `extract` 与 `owner-parity` 要 `AIOS_E3D_TEMPLATE_DIR`（模板与 attlib），与服务读的是同一份。
//! 只读，不连 Surreal，不写库。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use aios_core::NamedAttrMap;
use aios_database::data_interface::db_file_header::DbFileHeader;
use aios_database::data_interface::direct_attmap;
use aios_database::data_interface::direct_store::DirectSchema;
use aios_database::data_interface::e3d_collector::open_pinned_set;
use aios_database::data_interface::mdb_membership::{self, DESI_STYP};
use clap::Parser;
use e3d_io::refno::RefNo;
use e3d_io::{ReadOnlyEngine, ScanTier};

#[derive(Parser, Debug)]
#[command(name = "mdb_memory_probe")]
struct Args {
    #[arg(long, default_value = "AvevaMarineSample")]
    project: String,
    #[arg(long, default_value = "/ALL")]
    mdb: String,
    /// `index` / `header` / `extract` / `owner-parity`。量的那三档一档一个进程，
    /// 别在一个进程里叠着跑。
    #[arg(long, default_value = "index")]
    tier: Tier,
    /// 只量这几个 dbnum（逗号分隔）；不给就是 MDB 声明的全部 DESI。
    #[arg(long)]
    dbnums: Option<String>,
    /// 只量前 N 个库（按 dbnum 升序），用来先跑个小样看量级。
    #[arg(long)]
    limit: Option<usize>,
    #[arg(long)]
    json: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Tier {
    Index,
    Header,
    Named,
    Extract,
    /// 不量东西，只对账：记录头的属主 vs 属性表的 `OWNER`。
    OwnerParity,
}

impl Tier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Header => "header",
            Self::Named => "named",
            Self::Extract => "extract",
            Self::OwnerParity => "owner-parity",
        }
    }

    /// 要不要开模板与 attlib（解属性才要）。
    fn needs_schema(self) -> bool {
        matches!(self, Self::Extract | Self::OwnerParity)
    }
}

/// 一个库量出来的一行。
#[derive(Debug, serde::Serialize)]
struct DbRow {
    dbnum: u32,
    file: String,
    file_mib: f64,
    sesno: u32,
    elements: usize,
    /// 这一档没能装进内存的元素数。`index` / `header` 恒 0；`extract` 上是
    /// `to_named_attmap` 解不出来的那些——它们在真做起来时就是「这个元素没有属性」。
    failed: usize,
    millis: u128,
    /// 量完这个库之后的进程 RSS。
    rss_mib: f64,
}

fn rss_bytes(system: &mut sysinfo::System) -> u64 {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system
        .process(pid)
        .map(|process| process.memory())
        .unwrap_or(0)
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// 三档各自留在内存里的东西。**留着**才量得出常驻成本，所以它一路带到最后一次 RSS 之后。
#[derive(Default)]
struct Retained {
    /// `index`：direct_tree 今天建的那两张表。
    affiliations: HashMap<u32, u32>,
    refnos: Vec<Vec<RefNo>>,
    /// `header`：refno → (noun_hash, owner)。
    headers: Vec<HashMap<RefNo, (u32, RefNo)>>,
    /// `named`：加上存储的 `NAME`。没有 NAME 的元素不占字符串（`None`）——
    /// 存一个空串是给每个元素凭空加一份分配，而多数元素本来就没有名字。
    named: Vec<HashMap<RefNo, (u32, RefNo, Option<String>)>>,
    /// `extract`：整张属性表，`pe` / `ATT_*` 那一份的内存对应物。
    attrs: Vec<Vec<(RefNo, NamedAttrMap)>>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let db_option = aios_core::get_db_option();
    let membership = mdb_membership::resolve(db_option, &args.project, &args.mdb)?;

    let wanted: Option<Vec<u32>> = args.dbnums.as_deref().map(|list| {
        list.split(',')
            .filter_map(|item| item.trim().parse::<u32>().ok())
            .collect()
    });
    let mut targets: Vec<(u32, PathBuf)> = membership
        .of_type(DESI_STYP)
        .filter(|db| wanted.as_ref().is_none_or(|list| list.contains(&db.dbnum)))
        .filter_map(|db| db.path.clone().map(|path| (db.dbnum, path)))
        .collect();
    targets.sort();
    if let Some(limit) = args.limit {
        targets.truncate(limit);
    }
    // 声明了却不在盘上的那些不是 0 行，是量不到——混进「全 MDB 要多少内存」里会把
    // 结论压低，而低多少没人说得出。
    let unresolved: Vec<u32> = membership
        .of_type(DESI_STYP)
        .filter(|db| db.path.is_none())
        .map(|db| db.dbnum)
        .collect();

    println!(
        "MDB {} / {}：声明 DESI {} 个，本轮量 {} 个（盘上缺 {} 个：{:?}）",
        membership.mdb(),
        membership.project(),
        membership.of_type(DESI_STYP).count(),
        targets.len(),
        unresolved.len(),
        unresolved
    );
    println!(
        "档位 = {}（一档一个进程；叠着跑第二档的 RSS 增量里混着第一档没还给系统的那部分）\n",
        args.tier.as_str()
    );

    let schema = if args.tier.needs_schema() {
        Some(DirectSchema::open_from_env()?)
    } else {
        None
    };
    let mut mismatches: Vec<String> = Vec::new();

    let mut system = sysinfo::System::new();
    let baseline = rss_bytes(&mut system);
    println!(
        "基线 RSS {:.1} MiB（成员名单已解完，还没装任何元素）\n",
        mib(baseline)
    );
    println!(
        "{:<10} {:<24} {:>9} {:>7} {:>10} {:>8} {:>10} {:>10}",
        "dbnum", "file", "file MiB", "ses", "elements", "failed", "ms", "RSS MiB"
    );

    let mut retained = Retained::default();
    let mut rows: Vec<DbRow> = Vec::new();
    let mut skipped: Vec<(u32, String)> = Vec::new();
    let total_started = Instant::now();

    for (dbnum, path) in &targets {
        let started = Instant::now();
        let row = match load_one(
            *dbnum,
            path,
            args.tier,
            schema.as_ref(),
            &mut retained,
            &mut mismatches,
        ) {
            Ok(loaded) => loaded,
            Err(error) => {
                skipped.push((*dbnum, format!("{error:#}")));
                continue;
            }
        };
        let millis = started.elapsed().as_millis();
        let rss = rss_bytes(&mut system);
        let row = DbRow {
            millis,
            rss_mib: mib(rss),
            ..row
        };
        println!(
            "{:<10} {:<24} {:>9.1} {:>7} {:>10} {:>8} {:>10} {:>10.1}",
            row.dbnum,
            row.file,
            row.file_mib,
            row.sesno,
            row.elements,
            row.failed,
            row.millis,
            row.rss_mib
        );
        rows.push(row);
    }

    let wall = total_started.elapsed();
    // RSS 必须在 `retained` 还活着的时候读。下面那句 black_box 就是拦住优化器
    // 把它提前丢掉的——丢掉之后量的是退潮水位，不是常驻。
    let peak = rss_bytes(&mut system);
    std::hint::black_box(&retained);

    let elements: usize = rows.iter().map(|row| row.elements).sum();
    let failed: usize = rows.iter().map(|row| row.failed).sum();
    let file_mib: f64 = rows.iter().map(|row| row.file_mib).sum();
    let slowest = rows.iter().max_by_key(|row| row.millis);

    println!("\n—— {} 档合计 ——", args.tier.as_str());
    println!(
        "  库 {} 个（跳过 {}），文件合计 {:.1} MiB，元素 {}{}",
        rows.len(),
        skipped.len(),
        file_mib,
        elements,
        if failed > 0 {
            format!("，其中 {failed} 个没能装进内存")
        } else {
            String::new()
        }
    );
    println!(
        "  串行墙钟 {:.2}s；最慢的单库 {} ({:.2}s) —— 按库并行的下界就是它，不是总和",
        wall.as_secs_f64(),
        slowest.map(|row| row.dbnum).unwrap_or(0),
        slowest.map(|row| row.millis as f64 / 1000.0).unwrap_or(0.0)
    );
    println!(
        "  常驻 RSS {:.1} MiB（基线 {:.1} → {:.1}），每元素 {:.0} 字节",
        mib(peak.saturating_sub(baseline)),
        mib(baseline),
        mib(peak),
        if elements > 0 {
            peak.saturating_sub(baseline) as f64 / elements as f64
        } else {
            0.0
        }
    );
    if args.tier == Tier::OwnerParity {
        if mismatches.is_empty() {
            println!(
                "  记录头的属主与属性表 `OWNER` **逐元素一致**（{elements} 个，零分歧）\
                 —— MdbMemoryStore 的祖先链走头、DirectStore 的属性走属性表，两条读法同一个答案"
            );
        } else {
            println!(
                "  ！记录头属主与属性表 `OWNER` 有 {} 处不一致：",
                mismatches.len()
            );
            for line in mismatches.iter().take(20) {
                println!("    {line}");
            }
            if mismatches.len() > 20 {
                println!("    …… 其余 {} 处见 --json", mismatches.len() - 20);
            }
        }
    }
    for (dbnum, error) in &skipped {
        println!("  ! dbnum={dbnum} 量不了：{error}");
    }
    if !unresolved.is_empty() {
        println!(
            "  ! 盘上缺 {} 个声明的 DESI，上面的数**不含**它们",
            unresolved.len()
        );
    }

    if let Some(path) = args.json {
        let doc = serde_json::json!({
            "project": args.project,
            "mdb": args.mdb,
            "tier": args.tier.as_str(),
            "declared_desi": membership.of_type(DESI_STYP).count(),
            "measured_dbs": rows.len(),
            "unresolved_dbnums": unresolved,
            "skipped": skipped.iter().map(|(dbnum, error)| serde_json::json!({"dbnum": dbnum, "error": error})).collect::<Vec<_>>(),
            "elements": elements,
            "failed_elements": failed,
            "file_mib": file_mib,
            "wall_secs": wall.as_secs_f64(),
            "slowest_db_millis": slowest.map(|row| row.millis).unwrap_or(0),
            "rss_baseline_bytes": baseline,
            "rss_peak_bytes": peak,
            "rss_resident_bytes": peak.saturating_sub(baseline),
            "owner_mismatches": mismatches,
            "per_db": rows,
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
        println!("\n已写出 {}", path.display());
    }
    Ok(())
}

/// 装一个库。`millis` / `rss_mib` 由调用方填——它们量的是这一趟调用本身。
fn load_one(
    dbnum: u32,
    path: &std::path::Path,
    tier: Tier,
    schema: Option<&DirectSchema>,
    retained: &mut Retained,
    mismatches: &mut Vec<String>,
) -> anyhow::Result<DbRow> {
    let file_mib = mib(std::fs::metadata(path)?.len());
    let file = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let header = DbFileHeader::read(path)?;
    let sesno = header.latest_sesno();
    let mut failed = 0usize;

    let elements = match tier {
        Tier::Index => {
            let mut engine = ReadOnlyEngine::open(path)?;
            let indexed = engine.indexed_refnos()?;
            let mut refnos = Vec::with_capacity(indexed.len());
            for (refno, _, _) in indexed {
                // direct_tree 建的就是这张 ref0→dbnum 表，逐字同形。
                retained.affiliations.insert(refno.word0, dbnum);
                refnos.push(refno);
            }
            let count = refnos.len();
            retained.refnos.push(refnos);
            count
        }
        Tier::Header => {
            let mut engine = ReadOnlyEngine::open(path)?;
            let mut table: HashMap<RefNo, (u32, RefNo)> = HashMap::new();
            for scanned in engine.scan_elements(ScanTier::Header)? {
                match scanned {
                    Ok(scanned) => {
                        table.insert(scanned.refno, (scanned.noun_hash, scanned.owner));
                    }
                    Err(_) => failed += 1,
                }
            }
            let count = table.len();
            retained.headers.push(table);
            count
        }
        Tier::Named => {
            let mut engine = ReadOnlyEngine::open(path)?;
            let mut table: HashMap<RefNo, (u32, RefNo, Option<String>)> = HashMap::new();
            for scanned in engine.scan_elements(ScanTier::Named)? {
                match scanned {
                    Ok(scanned) => {
                        table.insert(
                            scanned.refno,
                            (scanned.noun_hash, scanned.owner, scanned.name),
                        );
                    }
                    Err(_) => failed += 1,
                }
            }
            let count = table.len();
            retained.named.push(table);
            count
        }
        Tier::Extract => {
            let schema = schema.expect("extract 档一定带 schema");
            let mut engine = ReadOnlyEngine::open(path)?;
            let indexed = engine.indexed_refnos()?;
            drop(engine);
            // 与生产同一条读法（`mdb_membership::SysDb::open` / `e3d_collector`）：
            // 按 header 自报的最新会话钉死。**不开池级缓存**，理由见文件头第 3 条。
            let set = open_pinned_set(schema, path, &header.db_type, sesno)?;
            let mut attrs = Vec::with_capacity(indexed.len());
            for (refno, _, _) in indexed {
                let element = set.element(refno);
                let Ok(extraction) = element.extraction() else {
                    failed += 1;
                    continue;
                };
                match direct_attmap::to_named_attmap(&extraction) {
                    Ok(converted) => attrs.push((refno, converted.map)),
                    Err(_) => failed += 1,
                }
            }
            let count = attrs.len();
            retained.attrs.push(attrs);
            count
        }
        Tier::OwnerParity => {
            let schema = schema.expect("owner-parity 档一定带 schema");
            let mut engine = ReadOnlyEngine::open(path)?;
            // 头扫描给一份「记录头说属主是谁」，抽取给一份「属性表说 OWNER 是谁」。
            let mut heads: Vec<(RefNo, RefNo)> = Vec::new();
            for scanned in engine.scan_elements(ScanTier::Header)? {
                match scanned {
                    Ok(scanned) => heads.push((scanned.refno, scanned.owner)),
                    Err(_) => failed += 1,
                }
            }
            drop(engine);
            let set = open_pinned_set(schema, path, &header.db_type, sesno)?;
            let mut compared = 0usize;
            for (refno, head_owner) in heads {
                let Ok(extraction) = set.element(refno).extraction() else {
                    failed += 1;
                    continue;
                };
                let Ok(converted) = direct_attmap::to_named_attmap(&extraction) else {
                    failed += 1;
                    continue;
                };
                // `direct_tree` 判链尾的口径：0 与自指都当「没有属主」。
                let from_attrs = converted
                    .map
                    .get_refu64("OWNER")
                    .filter(|owner| owner.0 != 0)
                    .unwrap_or_default();
                let from_head = if head_owner.is_valid() && head_owner != refno {
                    aios_core::RefU64::from_two_nums(head_owner.word0, head_owner.word1)
                } else {
                    aios_core::RefU64::default()
                };
                if from_head != from_attrs {
                    mismatches.push(format!(
                        "dbnum={dbnum} refno={}/{} 头说属主 {from_head}，属性表说 {from_attrs}",
                        refno.word0, refno.word1
                    ));
                }
                compared += 1;
            }
            compared
        }
    };

    Ok(DbRow {
        dbnum,
        file,
        file_mib,
        sesno,
        elements,
        failed,
        millis: 0,
        rss_mib: 0.0,
    })
}
