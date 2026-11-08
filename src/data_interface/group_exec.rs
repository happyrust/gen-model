//! 分组写侧执行器：把 [`plan_stages`](super::group_write::plan_stages) 排好的 [`Stage`]
//! 落到 e3d-io 的 [`Transaction`] 上。
//!
//! 一个库一个 Transaction：按序跑完 creates、记下 refno，再跑 fills（[`Slot::Created`] 回填），
//! 最后提交一次。跨库没有两阶段提交：逐库提交，失败即停，已提交的库不回滚——口径见
//! `docs/plans/2026-11-08-gpwl-gpset-m5-write-side.md` §3.4。
//!
//! e3d-io 的写侧只进这个文件；[`group_write`](super::group_write) 保持零 e3d-io 依赖。

use std::path::{Path, PathBuf};

use aios_core::RefU64;
use e3d_attlib::db1_hash;
use e3d_io::RefNo;
use e3d_io::db_element::template_file_for;
use e3d_io::record::template::TemplateProvider;
use e3d_io::write::group::GpsetMember;
use e3d_io::write::{self as e3d_write, SessionRequest, SpatialPolicy, Transaction};

use super::group_write::{Create, Fill, MemberPosition, Skipped, Slot, Stage};
use super::refno_route::to_refno;

/// GPWL / GPSET / GPITEM 只在设计库里；模板文件和模板类型都按它取。
const DESIGN_DB_TYPE: &str = "DESI";

/// 写会话的 `USER` / `DESC`：e3d-io 要求 ASCII，分别不超过 36 / 632 字节。
const SESSION_USER: &str = "gen-model";
const SESSION_COMMENT: &str = "gen-model GPWL/GPSET group write";

/// 执行结果：逐条回报，不做原子性承诺（跨库口径见设计文档 §3.4）。
///
/// `created` / `added` 只收已提交的库：失败的那个库整批没写，它的条目不出现。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecReport {
    /// 已提交的库号，按提交顺序。
    pub committed: Vec<u32>,
    pub created: Vec<Created>,
    pub added: Vec<Added>,
    /// 调用方传进来的 already_member 标记，原样带回；这些条目照样执行过，结果在 `added` 里。
    pub skipped: Vec<Skipped>,
    /// 失败即停，所以至多一条。
    pub errors: Vec<ExecError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Created {
    pub index: usize,
    pub refno: RefU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Added {
    pub index: usize,
    pub item: RefU64,
    /// 新建了 GPITEM 为 `true`；目标早已在组里、e3d-io 交回原有 GPITEM 为 `false`。
    pub inserted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecError {
    /// 出错的那条 op；找不到库文件、打开库、提交这类不属于某一条 op 的失败为 `None`。
    pub index: Option<usize>,
    pub stage_dbnum: u32,
    pub error: String,
}

/// 一个库一个 Transaction：按 Stage 顺序跑 creates（记下 refno），再跑 fills（Slot::Created 回填），
/// 提交。失败即停，不回滚已提交的库。
///
/// `template_dir` 里要有设计库模板 `desvir.dat`，`db_file` 把库号映射到库文件。只有开始写之前的
/// 准备（打开模板）失败才返回 `Err`；开始写之后的失败记进 [`ExecReport::errors`]，调用方才看得见
/// 哪些库已经提交。
///
/// 同步执行：e3d-io 的写侧全是同步接口，`Transaction` 也不是 `Send`；异步调用方自己 `spawn_blocking`。
pub fn execute(
    template_dir: &Path,
    db_file: impl Fn(u32) -> Option<PathBuf>,
    stages: &[Stage],
    skipped: &[Skipped],
) -> anyhow::Result<ExecReport> {
    let template = template_file_for(template_dir, DESIGN_DB_TYPE)?;
    let mut provider = TemplateProvider::open(&template, Some(db1_hash(DESIGN_DB_TYPE)))
        .map_err(|e| anyhow::anyhow!("open template {}: {e}", template.display()))?;
    let request = SessionRequest {
        user: SESSION_USER,
        comment: SESSION_COMMENT,
        clock: None,
    };
    let mut report = ExecReport {
        skipped: skipped.to_vec(),
        ..ExecReport::default()
    };
    for stage in stages {
        match run_stage(stage, &db_file, &mut provider, &request) {
            Ok((created, added)) => {
                report.committed.push(stage.dbnum);
                report.created.extend(created);
                report.added.extend(added);
            }
            Err(error) => {
                report.errors.push(error);
                break;
            }
        }
    }
    Ok(report)
}

/// 一个库：跑完 creates，再跑 fills，提交。交回的 refno 已按提交报告的 `reminted` 换成落盘的
/// 编号——提交时库若已被别的会话推进，新建元素会被改号。
fn run_stage(
    stage: &Stage,
    db_file: &impl Fn(u32) -> Option<PathBuf>,
    provider: &mut TemplateProvider,
    request: &SessionRequest<'_>,
) -> Result<(Vec<Created>, Vec<Added>), ExecError> {
    let fail = |index: Option<usize>, error: String| ExecError {
        index,
        stage_dbnum: stage.dbnum,
        error,
    };
    let path = db_file(stage.dbnum)
        .ok_or_else(|| fail(None, format!("no database file for dbnum {}", stage.dbnum)))?;
    let mut tx = Transaction::open(&path, provider)
        .map_err(|e| fail(None, format!("open {}: {e}", path.display())))?
        .with_spatial(SpatialPolicy::Off);

    let mut minted: Vec<RefNo> = Vec::with_capacity(stage.creates.len());
    let mut created = Vec::with_capacity(stage.creates.len());
    for create in &stage.creates {
        let (index, result) = match create {
            Create::World { index, world, name } => {
                (*index, tx.insert_gpwl(to_refno(*world), name))
            }
            Create::Set { index, gpwl, name } => {
                let gpwl = container(*gpwl, &minted).map_err(|e| fail(Some(*index), e))?;
                (*index, tx.insert_gpset(gpwl, name))
            }
        };
        let refno = result.map_err(|e| fail(Some(index), e.to_string()))?;
        minted.push(refno);
        created.push((index, refno));
    }

    let mut added = Vec::new();
    for fill in &stage.fills {
        match fill {
            Fill::AddMember {
                index,
                gpset,
                target,
                at,
            } => {
                let gpset = container(*gpset, &minted).map_err(|e| fail(Some(*index), e))?;
                let member = tx
                    .insert_gpset_member(gpset, to_refno(*target), position(*at))
                    .map_err(|e| fail(Some(*index), e.to_string()))?;
                added.push((*index, member));
            }
            Fill::SetRules {
                index,
                gpset,
                scosel,
            } => {
                let gpset = container(*gpset, &minted).map_err(|e| fail(Some(*index), e))?;
                tx.set_gpset_rules(gpset, scosel)
                    .map_err(|e| fail(Some(*index), e.to_string()))?;
            }
        }
    }

    let commit = tx
        .commit_reporting(request)
        .map_err(|e| fail(None, format!("commit: {e}")))?;
    let written = |refno: RefNo| {
        let refno = commit
            .reminted
            .iter()
            .find(|(given, _)| *given == refno)
            .map_or(refno, |&(_, written)| written);
        ref_u64(refno)
    };
    let created = created
        .into_iter()
        .map(|(index, refno)| Created {
            index,
            refno: written(refno),
        })
        .collect();
    let added = added
        .into_iter()
        .map(|(index, member)| Added {
            index,
            item: written(member.item()),
            inserted: matches!(member, GpsetMember::Inserted(_)),
        })
        .collect();
    Ok((created, added))
}

/// `Created(k)` 是本库第 k 个已建容器。`plan_stages` 只产出指向前面的槽；手拼的 Stage 指错了
/// 报错，不 panic。
fn container(slot: Slot, minted: &[RefNo]) -> Result<RefNo, String> {
    match slot {
        Slot::Existing(refno) => Ok(to_refno(refno)),
        Slot::Created(k) => minted
            .get(k)
            .copied()
            .ok_or_else(|| format!("{slot:?} names no container created earlier in this stage")),
    }
}

fn position(at: MemberPosition) -> e3d_write::MemberPosition {
    match at {
        MemberPosition::Last => e3d_write::MemberPosition::Last,
        MemberPosition::First => e3d_write::MemberPosition::First,
        MemberPosition::After(anchor) => e3d_write::MemberPosition::After(to_refno(anchor)),
        MemberPosition::Before(anchor) => e3d_write::MemberPosition::Before(to_refno(anchor)),
    }
}

fn ref_u64(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}

#[cfg(test)]
mod tests {
    use e3d_io::ReadOnlyEngine;
    use e3d_io::engine::ScanTier;
    use e3d_io::record::element::ParsedElement;
    use e3d_io::record::explicit::ExplicitValue;
    use e3d_io::write::group::attrs;

    use super::*;
    use crate::data_interface::group_write::{ContainerRef, GroupWriteOp, SkipReason, plan_stages};
    use crate::data_interface::refno_route::refno_dbnum_of;

    const SAMPLE_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000";
    /// 写这个库的副本：GPWL / GPSET / GPITEM 建在里面。
    ///
    /// 是 `aps250110` 而不是 `aps250218`：后者只有 WORL 和 MNUM 两个元素，既没有能当成员的
    /// 设计元素，库本身也不在项目 MDB 里（E3D 报每个 refno 都是 undefined），那样的副本演练
    /// 证明不了形状。`aps250110` 是装真实设备的库（EQUI / VALV / BRAN 各若干）。
    const GROUP_DB: &str = "aps250110_0001";
    /// 跨库 GPREF 的目标库：副本只读，挑目标、读回时确认目标在。
    const TARGET_DB: &str = "aps250209_0001";
    /// aps250204 里 `/AREA01_PIPE` 的 SCOSEL 原字（scosel-real-samples.json 里最短的 35 字样本之一）。
    /// 写真字节、不经 `scosel::encode`：执行器的验收不依赖编解码器。
    const AREA01_PIPE_SCOSEL: [i32; 35] = [
        35, 100016, 1, 100009, 3, 100007, 640493, 100015, 27, 1, 106, 4, 604480, 1, 120, 0, 1602,
        4, 2, 640493, 6, 1701, 118, 10, 65, 82, 69, 65, 48, 49, 80, 73, 80, 69, 402,
    ];

    fn template_dir() -> PathBuf {
        e3d_io::bundled_schema_root().join("e3d31").join("binaries")
    }

    /// 库里的 WORL，和第一个能当成员的设计元素（不是 WORL、不是 MNUM，也不是分组容器）。
    ///
    /// `MNUM` 必须排除：它几乎总排在 WORL 后面，是最容易被当成「设计元素」的系统 noun。拿它
    /// 当 GPREF 的目标，E3D 会答 `is a reference to an illegal element`。
    fn world_and_design_element(path: &Path) -> (Option<RefNo>, Option<RefNo>) {
        let mut engine = ReadOnlyEngine::open(path).expect("open");
        let world_hash = db1_hash("WORL");
        let mnum_hash = db1_hash("MNUM");
        let (mut world, mut design) = (None, None);
        for element in engine.scan_elements(ScanTier::Header).expect("scan") {
            let element = element.expect("element");
            if element.noun_hash == world_hash {
                world.get_or_insert(element.refno);
                continue;
            }
            if element.noun_hash == mnum_hash {
                continue;
            }
            let noun = e3d_attlib::db1_dehash(element.noun_hash).unwrap_or_default();
            if design.is_none() && !noun.is_empty() && noun != "MNUM" && !noun.starts_with("GP") {
                design = Some(element.refno);
            }
            if world.is_some() && design.is_some() {
                break;
            }
        }
        (world, design)
    }

    /// 记录上的显式属性，hunk 已解开：存在记录外的值读回成它指向的数组。
    fn explicit_of(path: &Path, refno: RefNo, attr: &str) -> Option<ExplicitValue> {
        let mut engine = ReadOnlyEngine::open(path).expect("open");
        let view = engine.find_element(refno).expect("read")?;
        let mut parsed =
            ParsedElement::parse_as(&view.raw_bytes, engine.text_format()).expect("parse");
        engine.resolve_hunks(&mut parsed).expect("resolve hunks");
        let hash = db1_hash(attr);
        parsed
            .explicit_attributes
            .iter()
            .find(|explicit| explicit.hash == hash)
            .map(|explicit| explicit.value.clone())
    }

    fn members_of(path: &Path, refno: RefNo) -> Vec<RefNo> {
        let mut engine = ReadOnlyEngine::open(path).expect("open");
        let view = engine.find_element(refno).expect("read").expect("exists");
        ParsedElement::parse_as(&view.raw_bytes, engine.text_format())
            .expect("parse")
            .members
    }

    /// 设计文档 §4 第 2 步那一批（GPWL → 两个 GPSET：一个放同库、跨库两条 GPITEM，一个只写
    /// SCOSEL）在样本库副本上执行再读回；再对已有 GPSET 补一条 already_member。只写 GPITEM 所在
    /// 库的副本，目标库的副本必须原样不动。
    #[test]
    #[ignore = "需要本机真库 aps250110_0001 / aps250209_0001"]
    fn a_group_written_on_a_real_copy_reads_back_through_gpref_and_scosel() {
        let copies = tempfile::tempdir().expect("tempdir");
        for db in [GROUP_DB, TARGET_DB] {
            std::fs::copy(Path::new(SAMPLE_DIR).join(db), copies.path().join(db))
                .unwrap_or_else(|e| panic!("copy {db}: {e}"));
        }
        let group_db = copies.path().join(GROUP_DB);
        let target_db = copies.path().join(TARGET_DB);

        let (Some(world), Some(here)) = world_and_design_element(&group_db) else {
            panic!("{GROUP_DB} has no WORL or no design element");
        };
        let (_, Some(elsewhere)) = world_and_design_element(&target_db) else {
            panic!("{TARGET_DB} has no design element");
        };
        let group_dbnum = refno_dbnum_of(world).expect("dbnum of the world");
        assert_eq!(refno_dbnum_of(here), Some(group_dbnum));
        assert_ne!(
            refno_dbnum_of(elsewhere),
            Some(group_dbnum),
            "the second target lives in another database"
        );
        eprintln!(
            "[group_exec] world {world:?}, here {here:?} (db {group_dbnum}), elsewhere {elsewhere:?} (db {:?})",
            refno_dbnum_of(elsewhere)
        );

        let ops = vec![
            GroupWriteOp::CreateWorld {
                world: ref_u64(world),
                name: "/GEN-MODEL-M5".into(),
            },
            GroupWriteOp::CreateSet {
                gpwl: ContainerRef::InBatch(0),
                name: "/GEN-MODEL-M5/ITEMS".into(),
            },
            GroupWriteOp::CreateSet {
                gpwl: ContainerRef::InBatch(0),
                name: "/GEN-MODEL-M5/RULES".into(),
            },
            GroupWriteOp::AddMember {
                gpset: ContainerRef::InBatch(1),
                target: ref_u64(here),
                at: MemberPosition::Last,
            },
            GroupWriteOp::AddMember {
                gpset: ContainerRef::InBatch(1),
                target: ref_u64(elsewhere),
                at: MemberPosition::Last,
            },
            GroupWriteOp::SetRules {
                gpset: ContainerRef::InBatch(2),
                scosel: AREA01_PIPE_SCOSEL.to_vec(),
            },
        ];
        let db_file = |dbnum: u32| (dbnum == group_dbnum).then(|| group_db.clone());
        let stages = plan_stages(&ops).expect("plan");
        let report = execute(&template_dir(), db_file, &stages, &[]).expect("execute");
        eprintln!("[group_exec] batch: {report:?}");

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.committed, [group_dbnum]);
        let created: Vec<usize> = report.created.iter().map(|c| c.index).collect();
        assert_eq!(created, [0, 1, 2]);
        let [gpwl, items, rules_set] = [0, 1, 2].map(|k| to_refno(report.created[k].refno));
        let added: Vec<(usize, bool)> =
            report.added.iter().map(|a| (a.index, a.inserted)).collect();
        assert_eq!(added, [(3, true), (4, true)]);
        let gpitems: Vec<RefNo> = report.added.iter().map(|a| to_refno(a.item)).collect();

        assert!(
            members_of(&group_db, world).contains(&gpwl),
            "the world lists the GPWL"
        );
        assert_eq!(
            members_of(&group_db, gpwl),
            [items, rules_set],
            "the GPWL lists both GPSETs"
        );
        assert_eq!(
            members_of(&group_db, items),
            gpitems,
            "the GPSET lists both GPITEMs in order"
        );
        for (gpitem, target) in gpitems.iter().zip([here, elsewhere]) {
            match explicit_of(&group_db, *gpitem, attrs::GPREF) {
                Some(ExplicitValue::RefNo(read)) => assert_eq!(read, target, "GPREF of {gpitem:?}"),
                other => panic!("GPREF of {gpitem:?} reads back as {other:?}"),
            }
        }
        let mut target_engine = ReadOnlyEngine::open(&target_db).expect("open the target copy");
        assert!(
            target_engine
                .find_element(elsewhere)
                .expect("read")
                .is_some(),
            "the cross-database target exists in its own database"
        );
        match explicit_of(&group_db, rules_set, attrs::SCOSEL) {
            Some(ExplicitValue::IntArray(read)) => {
                assert_eq!(read, AREA01_PIPE_SCOSEL, "SCOSEL reads back word for word")
            }
            other => panic!("SCOSEL reads back as {other:?}"),
        }

        let again = [GroupWriteOp::AddMember {
            gpset: ContainerRef::Existing(ref_u64(items)),
            target: ref_u64(here),
            at: MemberPosition::Last,
        }];
        let skipped = [Skipped {
            index: 0,
            reason: SkipReason::AlreadyMember,
        }];
        let stages = plan_stages(&again).expect("plan");
        let report = execute(&template_dir(), db_file, &stages, &skipped).expect("execute");
        eprintln!("[group_exec] already-member batch: {report:?}");

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.skipped, skipped);
        assert_eq!(
            report.added,
            [Added {
                index: 0,
                item: ref_u64(gpitems[0]),
                inserted: false,
            }]
        );
        assert_eq!(
            members_of(&group_db, items).len(),
            2,
            "no second GPITEM for the same target"
        );

        let original = std::fs::read(Path::new(SAMPLE_DIR).join(TARGET_DB)).expect("read original");
        assert!(
            std::fs::read(&target_db).expect("read copy") == original,
            "the target database copy is untouched"
        );
    }
}
