//! `MdbReadProvider` 的按库失效语义（lazy-startup 阶段 5 / 母计划不变量 7）。
//!
//! 文件变化 / 损坏 / 恢复 / `invalidate_cached` 只刷目标 dbnum：其它库的已开会话与元素缓存按 Arc
//! 原样带进下一个视图，旧视图持有者继续读它冻结那一刻的数据，全程 `read_stats.scan_calls == 0`。
//!
//! 两套夹具：
//! * **合成两库**（永远跑）：四页合成库照 `mdb_route_directory.rs::session_db_pages` 写，模板只在
//!   开库时校验类型，读的是记录（`parsed()`），不抽描述符；
//! * **AMS 副本**（`#[ignore]`，`-- --ignored` 手动跑）：把 `ams8000_0001` / `ams7998_0001` 复制到
//!   临时目录，一切改坏 / 恢复只落在副本上，源文件 SHA-256 前后一致。可重复的跑法是
//!   `python verification/run-ams-invalidation.py`。
//!
//! `read_stats` 计数器是进程全局的：两条测试共用一把门锁，验收命令带 `--test-threads=1`。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use aios_core::RefU64;
use aios_database::data_interface::direct_store::{DirectSchema, DirectStoreError, TEMPLATE_DIR_ENV};
use aios_database::data_interface::mdb_membership::{MdbDatabase, MdbMembership};
use aios_database::data_interface::mdb_read_store::{MdbReadProvider, MdbReadStore, invalidate_cached};
use aios_database::data_interface::mdb_route_directory::MdbRouteDirectory;
use e3d_io::db_element::DbElementError;
use e3d_io::read_stats::{self, ReadStats};
use e3d_io::record::element::ParsedElement;
use e3d_io::refno::RefNo;
use sha2::{Digest, Sha256};

// ---- 共用 ----------------------------------------------------------------------------------

fn gate() -> MutexGuard<'static, ()> {
    static GATE: OnceLock<Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `attlib.dat` + `*vir.dat` 所在目录：显式 `AIOS_E3D_TEMPLATE_DIR` 优先，否则 e3d-io 随源码携带的
/// e3d31 schema pack（`DbOption.toml` 钉的就是 e3d31）。
fn template_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(TEMPLATE_DIR_ENV) {
        return PathBuf::from(dir);
    }
    let pack = e3d_io::schema::SchemaPack::open_version(e3d_io::bundled_schema_root(), "e3d31")
        .expect("bundled e3d31 schema pack opens");
    pack.attlib()
        .expect("schema pack names attlib.dat")
        .parent()
        .expect("attlib.dat has a parent directory")
        .to_path_buf()
}

fn schema(template_dir: &Path) -> Arc<DirectSchema> {
    Arc::new(DirectSchema::open(template_dir).expect("schema opens from the template directory"))
}

fn declared(dbnum: u32, name: &str, path: &Path) -> MdbDatabase {
    MdbDatabase {
        dbnum,
        styp: aios_database::data_interface::mdb_membership::DESI_STYP,
        proj: 0,
        declared_project_code: None,
        name: name.to_string(),
        path: Some(path.to_path_buf()),
        project: Some("t574".to_string()),
        shadowed: Vec::new(),
    }
}

fn refu64(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}

fn sorted(dbnums: Vec<i32>) -> Vec<i32> {
    let mut dbnums = dbnums;
    dbnums.sort_unstable();
    dbnums
}

/// 上一个视图有、下一个视图没有的槽：`fork_without(changed)` 丢的恰是 `changed ∩ 已开`，两库都开着时
/// 这个差集就是变化集本身。
fn dropped(older: &MdbReadStore, newer: &MdbReadStore) -> Vec<i32> {
    let kept: BTreeSet<i32> = newer.opened_dbnums().into_iter().collect();
    sorted(older.opened_dbnums().into_iter().filter(|dbnum| !kept.contains(dbnum)).collect())
}

fn parsed(view: &MdbReadStore, refno: RefU64) -> anyhow::Result<Arc<ParsedElement>> {
    Ok(view.element(refno)?.parsed()?)
}

fn same_record(left: &ParsedElement, right: &ParsedElement) -> bool {
    left.refno == right.refno
        && left.noun_hash == right.noun_hash
        && left.owner == right.owner
        && left.members == right.members
        && left.implicit_data == right.implicit_data
}

/// 进程全局计数器的分段读数：`opens()` 给上一次调用以来新开的库数，`total()` 给从起点到现在的差值。
struct Lap {
    start: ReadStats,
    last: ReadStats,
}

impl Lap {
    fn start() -> Self {
        let now = read_stats::snapshot();
        Self { start: now, last: now }
    }

    fn opens(&mut self) -> u64 {
        let now = read_stats::snapshot();
        let delta = now.delta_from(self.last);
        self.last = now;
        delta.database_opens
    }

    fn total(&self) -> ReadStats {
        read_stats::snapshot().delta_from(self.start)
    }
}

/// 单调推进的 mtime：`DbSource::is_current` 比的是长度 + mtime + 创建时间，两次写落在同一个时钟刻度里
/// 会被当成没变，所以每次改动后都显式把 mtime 往前拨，不赌文件系统的时间粒度。
struct IdentityClock(SystemTime);

impl IdentityClock {
    fn new() -> Self {
        Self(SystemTime::now() + Duration::from_secs(5))
    }

    fn bump(&mut self, path: &Path) {
        self.0 += Duration::from_secs(2);
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for touching mtime")
            .set_modified(self.0)
            .expect("set mtime");
    }
}

fn assert_zero_scans(total: &ReadStats) {
    assert_eq!(
        (total.scan_calls, total.scanned_elements),
        (0, 0),
        "the whole run must not scan: {}",
        total.summary()
    );
}

// ---- 合成两库 ------------------------------------------------------------------------------

const WORDS_PER_PAGE: u32 = 64;
const PAGE_BYTES: usize = WORDS_PER_PAGE as usize * 4;

/// `RefNo::dbno` 的逆：低 13 位原位，extent 放 bits 13..25，高位段左移 13。
fn word0(dbno: u32, extent: u32) -> u32 {
    (dbno & 0x1FFF) | ((extent & 0x1FFF) << 13) | ((dbno & 0x3E000) << 13)
}

fn put_be(page: &mut [u8], offset: usize, value: u32) {
    page[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// `DbSet` 开得起来、`parsed()` 读得出来的最小库（照 `mdb_route_directory.rs::session_db_pages`）：
/// 第 0 页描述符（自报 dbnum / DESI 类型哈希 / 会话页在第 1 页）、第 1 页会话页（`+0x0C` 会话号）、
/// 第 2 页一条索引叶、第 3 页一条十一字记录。唯一的元素是 `word0(dbnum, 1) / 42`。
fn session_db_pages(dbnum: u32, sesno: u32) -> Vec<u8> {
    let element = RefNo::new(word0(dbnum, 1), 42);
    let mut pages = vec![vec![0_u8; PAGE_BYTES]; 4];
    put_be(&mut pages[0], 0x04, 2);
    put_be(&mut pages[0], 0x08, dbnum);
    put_be(&mut pages[0], 0x20, e3d_attlib::db1_hash("DESI"));
    put_be(&mut pages[0], 0x28, 1);
    put_be(&mut pages[0], 0x2C, 1);
    put_be(&mut pages[0], 0x30, 1);
    put_be(&mut pages[0], 0x34, WORDS_PER_PAGE);
    put_be(&mut pages[0], 0x38, element.word0);
    put_be(&mut pages[1], 0x00, 3);
    put_be(&mut pages[1], 0x04, u32::MAX);
    put_be(&mut pages[1], 0x0C, sesno);
    put_be(&mut pages[1], 0x1C, 2);
    put_be(&mut pages[1], 0x20, 1);
    put_be(&mut pages[2], 0x00, 5);
    for (index, value) in [13_387_743_u32, 0, 2, 2, 0, 53].into_iter().enumerate() {
        put_be(&mut pages[2], (index + 1) * 4, value);
    }
    for (index, value) in [element.word0, element.word1, 3, (20_u32 << 12) | 1].into_iter().enumerate() {
        put_be(&mut pages[2], (7 + index) * 4, value);
    }
    put_be(&mut pages[3], 0x00, 7);
    for (index, value) in [11, element.word0, element.word1, 0x000A_AA01, 0, 0, 0, 0, 0, 0, 0]
        .into_iter()
        .enumerate()
    {
        put_be(&mut pages[3], 40 + index * 4, value);
    }
    pages.concat()
}

/// 写一个库；`extra_pages` 模拟追加写过的尾巴（E3D 的保存是追加写，长度随会话一起变）。
fn write_session_db(dir: &Path, name: &str, dbnum: u32, sesno: u32, extra_pages: usize) -> PathBuf {
    let mut bytes = session_db_pages(dbnum, sesno);
    bytes.extend(std::iter::repeat_n(0_u8, PAGE_BYTES * extra_pages));
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write synthetic session db");
    path
}

/// 库 A 会话前进：`view()` 的 epoch +1、只丢 A 的槽；重读 A 开库 1 次、重读 B 开库 0 次且解析结果
/// 是同一个 `Arc`；旧视图持有者仍读 A 的旧会话。`invalidate_cached` 对 B 走同一组断言。
#[test]
fn a_session_advance_forks_only_that_database() {
    let _gate = gate();
    let tmp = tempfile::tempdir().expect("tempdir");
    let file_a = write_session_db(tmp.path(), "syn1000_0001", 1_000, 37, 0);
    let file_b = write_session_db(tmp.path(), "syn1001_0001", 1_001, 5, 0);
    let templates = template_dir();
    let directory = MdbRouteDirectory::from_databases(
        "t574-synthetic",
        "ALL",
        &[declared(1_000, "/A", &file_a), declared(1_001, "/B", &file_b)],
        &templates,
    );
    let provider = MdbReadProvider::install_for_tests(
        "t574-synthetic",
        "ALL",
        Arc::new(MdbMembership::default()),
        schema(&templates),
        directory,
    )
    .expect("provider over the synthetic directory");
    let element_a = refu64(RefNo::new(word0(1_000, 1), 42));
    let element_b = refu64(RefNo::new(word0(1_001, 1), 42));
    let mut lap = Lap::start();

    // 首个视图：构造不开库，第一次读才开；两库各开一次。
    let v1 = provider.view().expect("view 1");
    assert_eq!(v1.epoch, 1);
    assert!(v1.opened_dbnums().is_empty(), "view() itself opens nothing");
    assert_eq!(lap.opens(), 0);
    let a1 = parsed(&v1, element_a).expect("A reads");
    let b1 = parsed(&v1, element_b).expect("B reads");
    assert_eq!(lap.opens(), 2);
    assert_eq!(sorted(v1.opened_dbnums()), vec![1_000, 1_001]);
    assert_eq!(v1.cached_elements(), 2);
    assert_eq!(v1.metadata(1_000).unwrap().source.sesno, 37);
    assert_eq!(v1.metadata(1_001).unwrap().source.sesno, 5);

    // 没动：同一个视图，零开库。
    let unchanged = provider.view().expect("view unchanged");
    assert!(Arc::ptr_eq(&unchanged, &v1));
    assert_eq!(lap.opens(), 0);

    // A SAVEWORK：会话 37 → 38，文件长了一页。fork 只丢 A 的槽，本身不开库。
    write_session_db(tmp.path(), "syn1000_0001", 1_000, 38, 1);
    let v2 = provider.view().expect("view 2");
    assert_eq!(v2.epoch, 2, "one changed database advances the epoch by one");
    assert!(!Arc::ptr_eq(&v2, &v1));
    assert_eq!(dropped(&v1, &v2), vec![1_000], "only A's slot is dropped");
    assert_eq!(sorted(v2.opened_dbnums()), vec![1_001]);
    assert_eq!(v2.cached_elements(), 1, "B's cache entry is carried over, A's is gone");
    assert_eq!(sorted(v1.opened_dbnums()), vec![1_000, 1_001], "the old view keeps its pins");
    assert_eq!(lap.opens(), 0);

    // 重读 A：恰好开 1 次，落在新会话；重读 B：0 次，同一个 Arc。
    let a2 = parsed(&v2, element_a).expect("A rereads after the save");
    assert_eq!(lap.opens(), 1, "rereading A reopens exactly A");
    assert_eq!(v2.metadata(1_000).unwrap().source.sesno, 38);
    assert!(!Arc::ptr_eq(&a1, &a2), "A was parsed again from the new session");
    assert!(same_record(&a1, &a2));
    let b2 = parsed(&v2, element_b).expect("B rereads");
    assert_eq!(lap.opens(), 0, "rereading B opens nothing");
    assert!(Arc::ptr_eq(&b1, &b2), "B's parsed record is the very same Arc");
    assert_eq!(v2.metadata(1_001).unwrap().source.sesno, 5);
    assert_eq!(v2.cached_elements(), 2);

    // 旧视图持有者：A 还是会话 37 的那份，零 I/O。
    let a1_again = parsed(&v1, element_a).expect("old view still reads A");
    assert!(Arc::ptr_eq(&a1, &a1_again));
    assert_eq!(v1.metadata(1_000).unwrap().source.sesno, 37);
    assert_eq!(lap.opens(), 0);

    // `invalidate_cached`（targeted_model_refresh 调的那条）对 B：同一组断言。
    invalidate_cached("t574-synthetic", "ALL", 1_001).expect("invalidate B");
    let v3 = provider.view().expect("view 3");
    assert_eq!(v3.epoch, 3);
    assert_eq!(dropped(&v2, &v3), vec![1_001]);
    assert_eq!(v3.cached_elements(), 1);
    assert_eq!(lap.opens(), 0);
    let a3 = parsed(&v3, element_a).expect("A after invalidating B");
    assert!(Arc::ptr_eq(&a2, &a3), "A is untouched by B's invalidation");
    assert_eq!(lap.opens(), 0);
    let b3 = parsed(&v3, element_b).expect("B after invalidation");
    assert_eq!(lap.opens(), 1, "rereading B reopens exactly B");
    assert!(!Arc::ptr_eq(&b2, &b3));
    assert!(same_record(&b2, &b3));
    assert!(Arc::ptr_eq(&b2, &parsed(&v2, element_b).unwrap()), "the older view keeps its B");

    // 没开过的库与没登记的 provider：不丢任何槽、不开任何库。
    invalidate_cached("t574-synthetic", "ALL", 4_242).expect("unknown dbnum is harmless");
    let v4 = provider.view().expect("view 4");
    assert_eq!(sorted(v4.opened_dbnums()), vec![1_000, 1_001]);
    assert_eq!(v4.cached_elements(), 2);
    invalidate_cached("t574-nobody", "ALL", 1_000).expect("unregistered provider is a no-op");
    assert_eq!(lap.opens(), 0);

    let total = lap.total();
    assert_eq!(total.database_opens, 4, "{}", total.summary());
    assert_zero_scans(&total);
    println!("[synthetic] {}", total.summary());
}

// ---- AMS 副本 -------------------------------------------------------------------------------

const AMS_DB_DIR_ENV: &str = "AIOS_AMS_DB_DIR";
const AMS_DB_DIR_DEFAULT: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";
const AMS_SOURCES: [(u32, &str); 2] = [(8_000, "ams8000_0001"), (7_998, "ams7998_0001")];
/// `verification/run-ams-lazy-http.py` 读过的 8000 元素；7998 的元素从它的 WORL 成员表取，不写死。
const AMS8000_ELEMENT: (u32, u32) = (24_384, 23_257);

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    hex::encode(Sha256::digest(&bytes))
}

fn source_digests(dir: &Path) -> Vec<(String, String)> {
    AMS_SOURCES
        .iter()
        .map(|(_, name)| (name.to_string(), sha256_file(&dir.join(name))))
        .collect()
}

/// 把 8000 副本改坏成 `damaged`，再恢复成 `original`，两段各自断言：
/// 坏着时新视图只有 8000 读不出、7998 与其余库 0 新开库、旧视图照读改坏前的 8000；
/// 恢复后下一个视图重开 8000 恰 1 次且读通。返回恢复后的视图。
#[allow(clippy::too_many_arguments)]
fn damage_then_recover(
    label: &str,
    provider: &MdbReadProvider,
    copy_8000: &Path,
    damaged: &[u8],
    original: &[u8],
    clock: &mut IdentityClock,
    lap: &mut Lap,
    before: &Arc<MdbReadStore>,
    e8000: RefU64,
    e7998: RefU64,
    p8000_before: &Arc<ParsedElement>,
    p7998: &Arc<ParsedElement>,
) -> Arc<MdbReadStore> {
    std::fs::write(copy_8000, damaged).expect("write damaged copy");
    clock.bump(copy_8000);
    let broken = provider.view().expect("view while damaged");
    assert_eq!(broken.epoch, before.epoch + 1, "{label}: the damaged database advances the epoch by one");
    assert_eq!(dropped(before, &broken), vec![8_000], "{label}: only 8000 is forked away");
    assert_eq!(sorted(broken.opened_dbnums()), vec![7_998]);
    assert_eq!(lap.opens(), 0, "{label}: forking opens nothing");

    let error = parsed(&broken, e8000).expect_err("the damaged 8000 must not read");
    assert!(
        matches!(error.downcast_ref::<DbElementError>(), Some(DbElementError::Open { .. })),
        "{label}: an element read of the damaged 8000 fails at open, got {error:#}"
    );
    println!("[ams:{label}] new view 8000 element error (DbElementError::Open): {error:#}");
    assert!(
        matches!(broken.metadata(8_000), Err(DirectStoreError::Session { dbnum: 8_000, .. })),
        "{label}: metadata of the damaged 8000 surfaces as DirectStoreError::Session on 8000"
    );
    assert_eq!(lap.opens(), 0, "{label}: a failed open is not an open");
    assert!(
        Arc::ptr_eq(p7998, &parsed(&broken, e7998).expect("7998 keeps reading")),
        "{label}: 7998 answers from the shared cache"
    );
    assert_eq!(lap.opens(), 0, "{label}: 7998 and everything else open nothing");
    assert!(
        Arc::ptr_eq(p8000_before, &parsed(before, e8000).expect("old view reads 8000")),
        "{label}: the old view still hands out the pre-damage 8000 record"
    );
    assert_eq!(before.metadata(8_000).unwrap().dbnum, 8_000);
    assert_eq!(lap.opens(), 0);
    let still_broken = provider.view().expect("view again while damaged");
    assert!(Arc::ptr_eq(&still_broken, &broken), "{label}: nothing left to fork while 8000 stays closed");
    assert_eq!(lap.opens(), 0);

    std::fs::write(copy_8000, original).expect("restore original bytes");
    clock.bump(copy_8000);
    let recovered = provider.view().expect("view after restore");
    assert_eq!(recovered.epoch, broken.epoch, "{label}: 8000 was not open, so there is nothing to fork");
    assert_eq!(sorted(recovered.opened_dbnums()), vec![7_998]);
    assert_eq!(lap.opens(), 0);
    let p8000_after = parsed(&recovered, e8000).expect("8000 reads again after restore");
    assert_eq!(lap.opens(), 1, "{label}: the restored 8000 is reopened exactly once");
    assert!(same_record(p8000_before, &p8000_after), "{label}: restored bytes give the same record");
    assert!(Arc::ptr_eq(&p8000_after, &parsed(&recovered, e8000).unwrap()));
    assert!(Arc::ptr_eq(p7998, &parsed(&recovered, e7998).unwrap()));
    assert_eq!(lap.opens(), 0);
    assert_eq!(sorted(recovered.opened_dbnums()), vec![7_998, 8_000]);
    recovered
}

/// AMS 副本：换文件身份 / 改坏描述符页 / 截断 / 恢复 / `invalidate_cached(8000)`，每一步只动 8000。
#[test]
#[ignore = "manual: copies AMS ams8000_0001 + ams7998_0001 into a tempdir; run with `-- --ignored --test-threads=1`"]
fn ams_copies_invalidate_only_the_changed_database() {
    let _gate = gate();
    let source_dir = std::env::var_os(AMS_DB_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(AMS_DB_DIR_DEFAULT));
    let digests_before = source_digests(&source_dir);
    for (name, digest) in &digests_before {
        println!("[ams] source {name} sha256={digest}");
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let copies: Vec<(u32, PathBuf)> = AMS_SOURCES
        .iter()
        .map(|(dbnum, name)| {
            let copy = tmp.path().join(name);
            std::fs::copy(source_dir.join(name), &copy)
                .unwrap_or_else(|error| panic!("copy {name} into the tempdir: {error}"));
            (*dbnum, copy)
        })
        .collect();
    let copy_8000 = copies.iter().find(|(dbnum, _)| *dbnum == 8_000).unwrap().1.clone();
    let original_8000 = std::fs::read(&copy_8000).expect("read the 8000 copy");

    let templates = template_dir();
    let directory = MdbRouteDirectory::from_databases(
        "t574-ams-copy",
        "ALL",
        &copies
            .iter()
            .map(|(dbnum, path)| declared(*dbnum, &format!("/AMS/COPY{dbnum}"), path))
            .collect::<Vec<_>>(),
        &templates,
    );
    assert!(directory.verify_all_headers().is_empty(), "copies self-report their dbnums");
    let provider = MdbReadProvider::install_for_tests(
        "t574-ams-copy",
        "ALL",
        Arc::new(MdbMembership::default()),
        schema(&templates),
        directory,
    )
    .expect("provider over the copies");
    let mut lap = Lap::start();
    let mut clock = IdentityClock::new();

    // 首个视图：两库各开一次；8000 用固定元素，7998 用它的 WORL 记录本身——7998 是 AMS 最小的
    // 设计库，最新会话上 WORL 可能一个成员都没有。
    let v1 = provider.view().expect("view 1");
    assert_eq!(v1.epoch, 1);
    let world_7998 = refu64(v1.metadata(7_998).expect("7998 metadata").world);
    let e7998 = world_7998;
    let e8000 = RefU64::from_two_nums(AMS8000_ELEMENT.0, AMS8000_ELEMENT.1);
    let p8000_v1 = parsed(&v1, e8000).expect("8000 element reads");
    let p7998_v1 = parsed(&v1, e7998).expect("7998 element reads");
    let snapshot_8000 = v1.element_snapshot(e8000).expect("8000 attributes extract");
    assert_eq!((snapshot_8000.dbnum, snapshot_8000.db_type.as_str()), (8_000, "DESI"));
    let sesno_8000 = v1.metadata(8_000).unwrap().source.sesno;
    assert_eq!(lap.opens(), 2);
    assert_eq!(sorted(v1.opened_dbnums()), vec![7_998, 8_000]);
    println!("[ams] 8000 sesno={sesno_8000} element={e8000:?}; 7998 element=WORL {e7998:?}");

    // (1) 换文件身份，字节不变：只有 8000 进变化集。
    clock.bump(&copy_8000);
    let v2 = provider.view().expect("view 2");
    assert_eq!(v2.epoch, 2);
    assert_eq!(dropped(&v1, &v2), vec![8_000], "identity change: only 8000 in the changed set");
    assert_eq!(sorted(v1.opened_dbnums()), vec![7_998, 8_000]);
    assert_eq!(lap.opens(), 0);
    let p7998_v2 = parsed(&v2, e7998).expect("7998 rereads");
    assert!(Arc::ptr_eq(&p7998_v1, &p7998_v2));
    assert_eq!(lap.opens(), 0, "7998 is not reopened");
    let p8000_v2 = parsed(&v2, e8000).expect("8000 rereads");
    assert_eq!(lap.opens(), 1, "8000 is reopened exactly once");
    assert!(!Arc::ptr_eq(&p8000_v1, &p8000_v2));
    assert!(same_record(&p8000_v1, &p8000_v2));
    assert_eq!(v2.metadata(8_000).unwrap().source.sesno, sesno_8000);
    assert!(Arc::ptr_eq(&p8000_v1, &parsed(&v1, e8000).unwrap()), "the old view keeps its 8000");
    assert_eq!(lap.opens(), 0);

    // (2) 改坏描述符页 → 恢复；(3) 截断到 1 KiB → 恢复。
    let mut garbage_head = original_8000.clone();
    garbage_head[..4096].fill(0xFF);
    let v3 = damage_then_recover(
        "descriptor pages overwritten", &provider, &copy_8000, &garbage_head, &original_8000,
        &mut clock, &mut lap, &v2, e8000, e7998, &p8000_v2, &p7998_v1,
    );
    let p8000_v3 = parsed(&v3, e8000).unwrap();
    let v4 = damage_then_recover(
        "file truncated to 1 KiB", &provider, &copy_8000, &original_8000[..1024], &original_8000,
        &mut clock, &mut lap, &v3, e8000, e7998, &p8000_v3, &p7998_v1,
    );
    let p8000_v4 = parsed(&v4, e8000).unwrap();
    assert_eq!(v4.metadata(8_000).unwrap().source.sesno, sesno_8000);

    // (4) `invalidate_cached(project, mdb, 8000)`：targeted_model_refresh 调的那条，同一组断言。
    invalidate_cached("t574-ams-copy", "ALL", 8_000).expect("invalidate 8000");
    let v5 = provider.view().expect("view after invalidate_cached");
    assert_eq!(v5.epoch, v4.epoch + 1);
    assert_eq!(dropped(&v4, &v5), vec![8_000]);
    assert_eq!(sorted(v5.opened_dbnums()), vec![7_998]);
    assert_eq!(lap.opens(), 0);
    assert!(Arc::ptr_eq(&p7998_v1, &parsed(&v5, e7998).unwrap()), "7998 survives 8000's invalidation");
    assert_eq!(lap.opens(), 0);
    let p8000_v5 = parsed(&v5, e8000).expect("8000 after invalidate_cached");
    assert_eq!(lap.opens(), 1, "invalidate_cached(8000) costs exactly one reopen of 8000");
    assert!(!Arc::ptr_eq(&p8000_v4, &p8000_v5));
    assert!(same_record(&p8000_v4, &p8000_v5));
    assert!(Arc::ptr_eq(&p8000_v4, &parsed(&v4, e8000).unwrap()), "the older view keeps its 8000");
    assert_eq!(lap.opens(), 0);
    let snapshot_again = v5.element_snapshot(e8000).expect("8000 attributes extract again");
    assert_eq!(snapshot_again.explicit_attributes, snapshot_8000.explicit_attributes);
    assert_eq!(lap.opens(), 0);

    // 全程零扫描；源文件一个字节没动。
    let total = lap.total();
    assert_eq!(total.database_opens, 6, "{}", total.summary());
    assert_zero_scans(&total);
    let digests_after = source_digests(&source_dir);
    assert_eq!(digests_after, digests_before, "source database files must be untouched");
    for (name, digest) in &digests_after {
        println!("[ams] source {name} sha256={digest} (unchanged)");
    }
    println!("[ams] {}", total.summary());
}
