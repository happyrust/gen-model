//! `pipe_model::PipeCatalog` 的生产实现：架在 [`DirectStore`]（e3d-io 文件直读面）上。
//!
//! 读面映射与 `e3d_collector::assemble_ele_data` 同一出处：
//! noun 名取 `ElementExtraction.noun_name`（descriptor 层）、owner 取 `extraction.owner`
//! （成员记录面，不是 `pe_owner` 边——`NamedAttrMap` 里的 `OWNER` identity 键作快路）、
//! 成员表走 `DirectStore::members`（**原序**，BRAN 成员序即管路走向，不重排）、
//! 属性走 `named_attmap`（`REFNO`/`OWNER`/`TYPE` identity 键随隐式面返回）。
//!
//! 写侧词是会话语义，能取证的取证、取证不了的显式注入，不猜：
//! - `writable_dbnums`：本次会话可写的 dbnum 集——[`probe_writable_dbnums`]
//!   读 `MdbMembership` 里各 CURD 声明元素的写权限证据（`ACCESSDB=Read` 或
//!   `LREFO` 收窄才只读，缺席按 E3D 会话默认可写；见 `DbAccess::writable`），
//!   外加文件可达性 fail-closed 检查；也可由接入方直接注入裁决集。
//!   不在集内的元素 `writable()=false`，服务会在建批前拒；
//! - `alloc_refno(under)`：E3D `NEW X` 恒落在**属主所在库**——库号由
//!   `DirectStore::dbnum_of(under)` 位解，每库一条懒播种的 refno 高水位
//!   （该库派生索引里已见 refno 最大值起递增）。属主解不出库号或索引拿不到
//!   时退 `EDIT_FALLBACK_BASE` 起的计数段——该段与摄入行号段隔离（与
//!   `MockCatalog` 的 1_000_000 基同一纪律），属**测试/降级档**；
//! - `name_exists`/`refno_by_name` 扫全部已 pin 库的派生名索引（`DbIndexes::find_named`
//!   存 NAME 原文——调用方负责规范化，与索引契约一致）。

use crate::data_interface::direct_store::DirectStore;
use crate::data_interface::mdb_membership::{DbAccess, MdbDatabase};
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use dashmap::DashMap;
use e3d_io::refno::RefNo;
use pipe_model::PipeCatalog;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// 库内 refno 水位拿不到时的降级起点（与 MockCatalog 的分配段同纪律）。
const EDIT_FALLBACK_BASE: u64 = 1_000_000;

/// 按 MDB 声明逐库探测 `writable_dbnums`。
///
/// 写权限证据在 **CURD 声明元素**（SYS 侧 `TYPE=DB`）上，由
/// `read_declaration` 读 SYS 时采集进 [`MdbMembership`]——DESI 数据文件里
/// 没有 DB 元素（样本库实测：`refnos_of_noun("DB")` 为空），文件面没有
/// `ACCESSDB` 可读。裁决：
/// 1. 声明了但没绑到文件 → 不可写 + problem；
/// 2. 文件 pin/索引打不开 → 不可写 + problem（不可读 fail-closed）；
/// 3. 声明证据收窄（`ACCESSDB=Read` 或 `LREFO`）→ 不可写 + problem；
/// 4. 其余按 E3D 会话默认可写（声明进 MDB 即可写，`ACCESSDB` 缺席是常态）。
///
/// `dbs` 传 `membership.element_databases()`（DESI/ISOD——管道模型只写元素库）；
/// `access_of` 传 `|n| membership.access_of(n).cloned()`（测试可喂本地证据表）。
/// 返回 `(可写集, 问题表)`：失读/收窄必须可见，不只记日志。
pub fn probe_writable_dbnums<'a>(
    store: &DirectStore,
    dbs: impl IntoIterator<Item = &'a MdbDatabase>,
    access_of: impl Fn(u32) -> Option<DbAccess>,
) -> (BTreeSet<i32>, Vec<String>) {
    let mut writable = BTreeSet::new();
    let mut problems = Vec::new();
    for db in dbs {
        let dbnum = db.dbnum as i32;
        let Some(path) = db.path.as_ref() else {
            problems.push(format!(
                "dbnum={dbnum} ({}) 声明了但没绑到文件，按不可写计",
                db.name
            ));
            continue;
        };
        if let Err(error) = store.indexes(dbnum) {
            problems.push(format!(
                "dbnum={dbnum} ({}) {} 打不开，按不可写计：{error:#}",
                db.name,
                path.display()
            ));
            continue;
        }
        match access_of(db.dbnum) {
            Some(access) if !access.writable() => {
                problems.push(format!(
                    "dbnum={dbnum} ({}) 声明收窄只读：ACCESSDB={:?} LREFO={}",
                    db.name, access.access, access.reference_only
                ));
            }
            _ => {
                writable.insert(dbnum);
            }
        }
    }
    (writable, problems)
}

/// 文件直读面的 `PipeCatalog`。
///
/// 构造后须先 `pin`/`pin_from_locator` 把目标库钉进来（读 `attrs`/`members` 会
/// 按需自 pin，但 `writable`/`alloc_refno`/名索引只认已 pin 的库）。
pub struct DirectPipeCatalog {
    store: Arc<DirectStore>,
    /// 本会话可写库（空集 = 全部只读——建批前的拒止是服务侧语义，这里如实报）。
    writable_dbnums: BTreeSet<i32>,
    /// 每 dbnum 一条 refno 水位：懒播种于该库派生索引的已见 refno 最大值，
    /// 之后单调递增。落键 = `RefU64` 打包值（word0 高、序号低）。
    waterlines: DashMap<i32, u64>,
    /// 降级段序号（属主解不出库号时的隔离段）。
    seq: AtomicU64,
}

impl DirectPipeCatalog {
    pub fn new(store: Arc<DirectStore>, writable_dbnums: BTreeSet<i32>) -> Self {
        Self {
            store,
            writable_dbnums,
            waterlines: DashMap::new(),
            seq: AtomicU64::new(0),
        }
    }

    /// 懒播某库 refno 高水位：索引里全部已见 refno 的最大值。
    /// 返回该库下一个可用打包值；索引拿不到返回 `None`。
    fn next_in_db(&self, dbnum: i32) -> Option<u64> {
        let mut entry = self.waterlines.entry(dbnum).or_insert_with(|| {
            self.store
                .indexes(dbnum)
                .map(|idx| {
                    idx.noun_census()
                        .iter()
                        .flat_map(|(hash, _)| idx.refnos_of_noun_hash(*hash, None))
                        .map(|r| r.0)
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0)
        });
        if *entry == 0 {
            // 索引空/缺——水位没证据，不许在真号段里产裸 refno，交降级段。
            drop(entry);
            return None;
        }
        *entry += 1;
        Some(*entry)
    }
}

impl PipeCatalog for DirectPipeCatalog {
    fn noun_of(&self, refno: RefU64) -> Option<String> {
        let ty = self.store.named_attmap(refno).ok()?.get_type().to_string();
        (ty != "unset").then_some(ty)
    }

    fn name_of(&self, refno: RefU64) -> Option<String> {
        self.store.named_attmap(refno).ok()?.get_name()
    }

    fn owner_of(&self, refno: RefU64) -> Option<RefU64> {
        // 快路：identity 键 OWNER 随隐式面返回；拿不到（owner 不落属性的 noun）走
        // descriptor 抽取的 owner 字段——与 assemble_ele_data 同一出处。
        if let Ok(map) = self.store.named_attmap(refno) {
            let owner: RefU64 = map.get_owner().into();
            if owner != RefU64::default() {
                return Some(owner);
            }
        }
        let snap = self.store.element_snapshot(refno).ok()?;
        let owner = RefU64::from_two_nums(snap.extraction.owner.word0, snap.extraction.owner.word1);
        (owner != RefU64::default()).then_some(owner)
    }

    fn members_of(&self, refno: RefU64) -> Vec<RefU64> {
        self.store.members(refno).unwrap_or_default()
    }

    fn attr(&self, refno: RefU64, name: &str) -> Option<NamedAttrValue> {
        self.store.named_attmap(refno).ok()?.map.get(name).cloned()
    }

    fn attmap_of(&self, refno: RefU64) -> Option<NamedAttrMap> {
        self.store.named_attmap(refno).ok()
    }

    fn name_exists(&self, name: &str) -> bool {
        self.refno_by_name(name).is_some()
    }

    fn refno_by_name(&self, name: &str) -> Option<RefU64> {
        for dbnum in self.store.opened_dbnums() {
            if let Ok(idx) = self.store.indexes(dbnum) {
                if let Some(hit) = idx.find_named(name).into_iter().next() {
                    return Some(hit);
                }
            }
        }
        None
    }

    fn writable(&self, refno: RefU64) -> bool {
        self.store
            .dbnum_of(refno)
            .map(|dbnum| self.writable_dbnums.contains(&dbnum))
            .unwrap_or(false)
    }

    fn alloc_refno(&self, under: RefU64) -> RefU64 {
        // E3D `NEW X` 恒落在属主所在库：库号位解自属主 refno。
        let dbnum = self.store.dbnum_of(under).ok();
        match dbnum.and_then(|d| self.next_in_db(d)) {
            Some(raw) => {
                let (word0, word1) = (raw >> 32, raw & 0xFFFF_FFFF);
                RefU64::from_two_nums(word0 as u32, word1 as u32)
            }
            None => RefU64(EDIT_FALLBACK_BASE + self.seq.fetch_add(1, Ordering::SeqCst)),
        }
    }
}

/// `RefNo`（e3d-io 双词）→ `RefU64` 的换算——与 `e3d_collector::refu64` 同一式。
#[allow(dead_code)]
fn refu64(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}
