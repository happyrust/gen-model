//! GitHub #144 —— 级联目标根与单元级发布（ADR-080）的交界。
//!
//! 窗口规划器的三条级联（结构连接 C2、电缆端点 C8、设计引用 P4）把「对端 / 引用者」所在的生成根并进 `regen`，
//! 只到**根**。根怎么重算由单元级发布定：执行体按根从 `plan_update(S→T)` 切子计划（[`restrict_plan_to_root`]），
//! 子计划为空才回退整根重算，不空就只重建子计划点名的那几件。级联拉进来的根如果同一窗里自己也有改动，子计划
//! 不空——而级联要它重算的那一件（被改动的关节切的梁、引用了被改动元素的桥架件）不在其中，因为 `plan_update`
//! 不认识这条跨元素的依赖。
//!
//! 四组：
//! - **纯内存**：手搓一窗，走生产同一段（规划器选根 → 工作单带单元级提示 → 执行体切子计划）。
//! - **真库，假定源**：ams7999 等库里带目录正体的 `PJOI` 与它 `CREF` 的梁，把其中一端当成一窗仅有的候选
//!   喂给 e3d-model 的 `plan_update_with`，看它点名了谁、子计划切出来是什么。2026-11-11 钉的是缺口（两个方向都不
//!   点名对端）；2026-11-12 e3d-model 的关节级联接上主关节之后改钉「两端一起点名」——四本库那条是手动的，
//!   aps7166 那条随整套跑。
//! - **量法**（手动）：真实会话窗口上，级联目标根里有多少今天走局部重建；顺带找自然窗口里「一端被点名、
//!   对端没被点名」的结构连接，两端各建一次比几何。
//! - **真库，真实窗口**（手动）：电缆 / 桥架库的自然窗口上，电缆端点（C8）与设计引用（P4）的目标根修之前带
//!   单元级提示、修之后不带，同一窗里别的根两态相同。
//!
//! 这个文件是 `e3d_model_service` 的子模块：[`restrict_plan_to_root`] 与 `RootSubplan` 是那里的私有项。

use e3d_model::increment::AffectedClosure;

use super::*;
use crate::data_interface::generation_root::{
    GenerationRoot, GenerationRootKind, enumerate_generation_roots, refno_from_e3d, refno_to_e3d,
    resolve_delivery_unit_types,
};
use crate::data_interface::manual_update::{
    DeliveryUnitSummary, UnitPublishHint, UnitTask, collect_unit_tasks,
};
use crate::data_interface::structural_connection::{
    StructuralConnectionCascade, roots_on_owner_chain,
};
use crate::data_interface::window_root_plan::{
    PersistedRoot, WindowImpact, WindowRootPlan, WindowRootSources, change_metadata_tier,
    plan_window_roots,
};

fn key_of(refno: RefNo) -> (u32, u32) {
    (refno.word0, refno.word1)
}

/// 生产同一段折算，逐句照 `window_root_plan::build_model_update_plan_from_window`（本窗每个 regen 根一条
/// `DeliveryUnitSummary`；规划没退化就带上单元级发布的基线 S、D10 名单与级联目标根的名单）与 `manual_update.rs`
/// 里把计划交给 [`collect_unit_tasks`] 的那一处。开关 `model_unit_level_publish` 按缺省（开）算，不读配置；
/// `settled` 取 `roots_S` 全集（测试没有进程内账本）。
fn window_unit_tasks(
    dbnum: u32,
    sources: &WindowRootSources,
    roots_s: &[PersistedRoot],
    impact: &WindowImpact,
    plan: &WindowRootPlan,
) -> Vec<UnitTask> {
    window_unit_tasks_as(dbnum, sources, roots_s, impact, plan, true)
}

/// [`window_unit_tasks`] 的本体。`with_cascade_target_list = false` 重放修之前的折算：那时没有级联目标根这张
/// 名单，等于名单恒空，其余逐句相同——真实窗口上拿它当「修之前」那一态。
fn window_unit_tasks_as(
    dbnum: u32,
    sources: &WindowRootSources,
    roots_s: &[PersistedRoot],
    impact: &WindowImpact,
    plan: &WindowRootPlan,
    with_cascade_target_list: bool,
) -> Vec<UnitTask> {
    let settled: BTreeSet<RefnoEnum> = roots_s.iter().map(|root| root.refno).collect();
    let units: Vec<DeliveryUnitSummary> = plan
        .regen
        .iter()
        .map(|root| DeliveryUnitSummary {
            root_refno: root.root.to_pdms_str(),
            noun: root.noun.clone(),
            name: root.name.clone(),
            will_generate: true,
            ..Default::default()
        })
        .collect();
    let unit_publish_base_sesno = plan.degraded.is_none().then_some(sources.base_sesno);
    let no_model_only_roots: Vec<String> = if unit_publish_base_sesno.is_some() {
        sources
            .no_model_only(roots_s, &settled, impact, plan)
            .roots
            .iter()
            .map(|root| root.to_pdms_str())
            .collect()
    } else {
        Vec::new()
    };
    let cascade_target_roots: Vec<String> =
        if with_cascade_target_list && unit_publish_base_sesno.is_some() {
            sources.unit_publish_cascade_targets(plan)
        } else {
            Vec::new()
        };
    collect_unit_tasks(
        &units,
        &[],
        dbnum,
        sources.target_sesno as i32,
        unit_publish_base_sesno,
        &no_model_only_roots,
        &cascade_target_roots,
    )
}

fn task_of_root<'a>(tasks: &'a [UnitTask], root: RefnoEnum) -> Option<&'a UnitTask> {
    let name = root.to_pdms_str();
    tasks.iter().find(|task| task.root_refno == name)
}

// ───────────────────────────── 纯内存 ─────────────────────────────

const FIXTURE_WORD0: u32 = 24384;
const FIXTURE_DBNUM: u32 = 7999;
const BASE_SESNO: u32 = 41;
const TARGET_SESNO: u32 = 42;

fn rf(n: u32) -> RefNo {
    RefNo::new(FIXTURE_WORD0, n)
}

fn rr(n: u32) -> RefnoEnum {
    refno_from_e3d(rf(n))
}

fn fixture_keys(ids: &[u32]) -> BTreeSet<(u32, u32)> {
    ids.iter().map(|id| (FIXTURE_WORD0, *id)).collect()
}

fn fixture_root(n: u32) -> GenerationRoot {
    GenerationRoot {
        root: rr(n),
        noun: "FRMW".into(),
        name: format!("/FRMW-{n}"),
        kind: GenerationRootKind::Normal,
    }
}

/// 两端相同的属主表：`STRU 1` 之下两个 `FRMW` 根（10、20）。
/// 根 10 名下：关节 11、被它切的梁 12。根 20 名下：被关节 11 切的另一根梁 21、与这件事无关的板 22。
fn fixture_owners() -> BTreeMap<(u32, u32), RefNo> {
    [
        (1, 1),
        (10, 1),
        (20, 1),
        (11, 10),
        (12, 10),
        (21, 20),
        (22, 20),
    ]
    .into_iter()
    .map(|(element, owner)| ((FIXTURE_WORD0, element), rf(owner)))
    .collect()
}

/// 手搓一窗的文件侧输入：`regenerate` 是 `plan_update(S→T)` 点名要重建的单元，`closure` 是它们的祖先闭包。
/// 三条级联都当关着给（`Default`），由用例自己开、自己填目标集。
fn fixture_sources(regenerate: &[u32], closure: &[u32]) -> WindowRootSources {
    WindowRootSources {
        base_sesno: BASE_SESNO,
        target_sesno: TARGET_SESNO,
        roots_s: [10, 20]
            .iter()
            .map(|n| PersistedRoot::from(&fixture_root(*n)))
            .collect(),
        roots_t: vec![fixture_root(10), fixture_root(20)],
        plan: UpdatePlan {
            regenerate: fixture_keys(regenerate),
            remove: BTreeSet::new(),
            regenerate_derived: BTreeSet::new(),
            derived_stale: BTreeSet::new(),
            ledger: ChangeLedger::new(),
            full_rebuild: None,
            report: Default::default(),
        },
        closure: AffectedClosure {
            refnos: fixture_keys(closure),
            unreadable: Vec::new(),
        },
        index_keys: 100,
        xgeometry_observed: 0,
        xgeometry_unreadable: 0,
        nested_roots: 0,
        duplicate_coverage_elements: 0,
        structural_connection: Default::default(),
        cable_endpoint: Default::default(),
        design_reference: Default::default(),
        rigid: Default::default(),
        removed_units: Vec::new(),
        changes: Default::default(),
        timings: Default::default(),
    }
}

/// GitHub #144：一个根既被闭包直接命中（它名下这一窗有自己的改动）、又是某条级联的目标（别处变了、它名下另一件
/// 的几何要跟），就不能走单元级发布——必须整根重算。
///
/// 走的是生产同一段：规划器选根（`impact` → `plan_window_roots`）、工作单带提示（`collect_unit_tasks`）、
/// 执行体切子计划（`restrict_plan_to_root`，`UnitDeltaProducer::produce` 用的就是它）。每个用例先核两条前提，再下结论：
/// 1. 这根的子计划**不空**（空的话执行体本来就回退整根重算，那条路今天是对的）；
/// 2. 被牵连的那一件**不在**子计划里（`plan_update` 不认识跨元素的依赖）；
/// 3. 所以这根的任务不许带单元级提示——带着它，执行体只重建子计划里那几件，被牵连的那一件留在旧几何上。
///
/// 对照：同一窗里只被直接命中、不是级联目标的那一根，提示一字不变。
#[test]
fn a_cascade_target_root_that_also_changed_itself_is_regenerated_whole_not_by_unit() {
    let owners = fixture_owners();
    // (用例, 哪条级联, 级联目标根, 被牵连却没被点名的那一件, 对照根)
    // 四个用例里 `plan_update` 点名的都是关节 11 与板 22，闭包是它们的祖先，两根都被直接命中。
    let cases: [(&str, &str, u32, u32, u32); 4] = [
        (
            "C2 跨根：关节 11（根 10）变了，它切的梁 21 在根 20；根 20 自己另有板 22 变了",
            "C2",
            20,
            21,
            10,
        ),
        (
            "C8 跨根：端点 11 变了，引用它的电缆段 21 在根 20；根 20 自己另有 22 变了",
            "C8",
            20,
            21,
            10,
        ),
        (
            "P4 跨根：11 变了，POSREF 指着它的 21 在根 20；根 20 自己另有 22 变了",
            "P4",
            20,
            21,
            10,
        ),
        (
            "C2 同根：关节 11 变了，它切的梁 12 与它同在根 10",
            "C2",
            10,
            12,
            20,
        ),
    ];
    for (label, cascade, target_root, implicated, control_root) in cases {
        let mut sources = fixture_sources(&[11, 22], &[1, 10, 11, 20, 22]);
        sources.structural_connection.enabled = true;
        sources.cable_endpoint.enabled = true;
        sources.design_reference.enabled = true;
        match cascade {
            "C2" => {
                sources
                    .structural_connection
                    .far_end_roots
                    .insert(rr(target_root));
            }
            "C8" => {
                sources
                    .cable_endpoint
                    .referrer_roots
                    .insert(rr(target_root));
            }
            "P4" => {
                sources
                    .design_reference
                    .referrer_roots
                    .insert(rr(target_root));
            }
            other => unreachable!("{other}"),
        }
        let roots_s = sources.roots_s.clone();
        let impact = sources.impact(&roots_s);
        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
        assert_eq!(plan.degraded, None, "{label}");
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![rr(10), rr(20)],
            "{label}：两根都被闭包直接命中，都在 regen 里"
        );
        assert!(
            sources.cascaded_roots().contains(&rr(target_root)),
            "{label}：根 {target_root} 是级联目标"
        );

        // 执行体那一半：同一窗的计划按根切子计划。
        let sets = window_plan_sets(
            FIXTURE_DBNUM,
            Path::new("issue-144-fixture"),
            BASE_SESNO,
            TARGET_SESNO,
            &sources.plan,
            None,
        );
        let subplan =
            restrict_plan_to_root(&sets, rf(target_root), &owners, &owners).expect("subplan");
        assert!(
            !subplan.is_empty(),
            "{label}：前提 1——根 {target_root} 自己这一窗也有改动，子计划不空（空的话执行体本来就回退整根重算）"
        );
        assert!(
            !subplan.regenerate.contains(&(FIXTURE_WORD0, implicated)),
            "{label}：前提 2——plan_update 不认识这条跨元素依赖，被牵连的 {implicated} 不在子计划里"
        );

        // 规划器那一半：本窗的工作单。
        let tasks = window_unit_tasks(FIXTURE_DBNUM, &sources, &roots_s, &impact, &plan);
        let control = task_of_root(&tasks, rr(control_root)).expect("control root task");
        assert_eq!(
            control.unit_publish,
            Some(UnitPublishHint {
                source_start_sesno: BASE_SESNO,
                no_model_only: false,
            }),
            "{label}：对照——根 {control_root} 只被直接命中、不是级联目标，照旧带单元级提示"
        );
        let task = task_of_root(&tasks, rr(target_root)).expect("cascade target root task");
        assert!(task.transform.is_none(), "{label}：级联目标根不走刚体前移");
        assert!(
            task.unit_publish.is_none(),
            "{label}：根 {target_root} 既被闭包直接命中、又是级联目标。它的子计划 = {subplan:?}（不空），被牵连的 \
             {implicated} 不在其中；任务却带着单元级提示 {:?}——执行体只会重建子计划里那几件，{implicated} 留在旧几何上。\
             这种根必须整根重算（GitHub #144）",
            task.unit_publish
        );
    }
}

/// GitHub #144 的验收第三条：级联目标根之外的根，单元级发布的提示不变。
///
/// 四种窗口里根 10 都只是被闭包直接命中：三条级联都关着；缺省配置（C2 关、C8 / P4 开）没有目标；三条都开着没有
/// 目标；三条都开着、目标是**另一根**（20）。根 10 的任务每一种都带基线 S 的提示。
///
/// 第二种是只有 C2 认得的依赖（`FIXING ↔ STWALL`、`SCOJ` / `SUBJ` 关节、跨库的结构连接……）在缺省配置下的样子：
/// C2 关着不读边，根 10 不在任何目标集里。这条规则靠级联的目标集认根，目标集里没有的它罩不住。（「带目录正体的
/// PJOI 与同根的梁」「不成单元却给梁出刀的关节与被它切的梁」原先也是这个样子；2026-11-12 起由 e3d-model 的关节
/// 级联在单元这一层接住，见本文件的真库测试。）
#[test]
fn roots_outside_every_cascade_target_set_keep_their_unit_publish_hint() {
    // (用例, C2 开关, C8 / P4 开关, 三条都开时的级联目标根)
    let cases: [(&str, bool, bool, Option<u32>); 4] = [
        ("三条级联都关着", false, false, None),
        ("缺省配置：C2 关、C8 / P4 开，没有目标", false, true, None),
        ("三条都开着，没有目标", true, true, None),
        ("三条都开着，目标是另一根（20）", true, true, Some(20)),
    ];
    for (label, c2, others, target) in cases {
        let mut sources = fixture_sources(&[11, 22], &[1, 10, 11, 20, 22]);
        sources.structural_connection.enabled = c2;
        sources.cable_endpoint.enabled = others;
        sources.design_reference.enabled = others;
        if let Some(target) = target {
            sources
                .structural_connection
                .far_end_roots
                .insert(rr(target));
        }
        let roots_s = sources.roots_s.clone();
        let impact = sources.impact(&roots_s);
        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
        assert!(
            !sources.cascaded_roots().contains(&rr(10)),
            "{label}：根 10 不是级联目标"
        );
        let tasks = window_unit_tasks(FIXTURE_DBNUM, &sources, &roots_s, &impact, &plan);
        assert_eq!(
            task_of_root(&tasks, rr(10))
                .expect("root 10 task")
                .unit_publish,
            Some(UnitPublishHint {
                source_start_sesno: BASE_SESNO,
                no_model_only: false,
            }),
            "{label}：根 10 照旧带单元级提示"
        );
    }
}

// ───────────────────────────── 真库（手动） ─────────────────────────────

const AMS_PROJECT_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";
const APS_PROJECT_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000";
const CATALOGUE_PROJECT_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000";

fn env_dir(name: &str, default: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| default.into()))
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// 一个项目目录的目录库表，多个 `DbSet` 共用一次扫描。
struct SharedResolver(Arc<e3d_model::DirectoryDbResolver>);

impl DbFileResolver for SharedResolver {
    fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
        DbFileResolver::resolve(&*self.0, dbno)
    }
}

/// 一个项目的开库现场：设计库钉会话，目录库按库号从项目目录与 AVEVA 目录项目里找（与 e3d-model
/// `tests/primary_joint_frame_real.rs` 同一种开法）。带目录正体的关节算不算模型单元要读目录，不挂目录库
/// 分类就不是生产那一份。
struct Project {
    attlib: PathBuf,
    desi_template: PathBuf,
    resolver: Arc<e3d_model::DirectoryDbResolver>,
}

impl Project {
    fn open(schema: &DirectSchema, project_dir: &Path) -> Self {
        let mut dirs = vec![project_dir.to_path_buf()];
        let catalogue = env_dir("AIOS_ACP_PROJECT_DIR", CATALOGUE_PROJECT_DIR);
        if catalogue.is_dir() && catalogue.as_path() != project_dir {
            dirs.push(catalogue);
        }
        let resolver = e3d_model::DirectoryDbResolver::scan(&dirs, schema.template_dir())
            .expect("scan the project directories into a dbno → file table");
        Self {
            attlib: schema.template_dir().join("attlib.dat"),
            desi_template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
            resolver: Arc::new(resolver),
        }
    }

    fn set_at(&self, file: &Path, sesno: u32) -> Option<Arc<DbSet>> {
        let set = Arc::new(
            DbSet::with_attlib_file_and_resolver(
                &self.attlib,
                Box::new(SharedResolver(self.resolver.clone())),
            )
            .ok()?,
        );
        set.add_db(DbFilePin {
            file: file.to_path_buf(),
            template: self.desi_template.clone(),
            db_type: Some("DESI".into()),
            sesno: Some(sesno),
        })
        .ok()?;
        Some(set)
    }
}

/// 会话链上相邻的两步（旧 → 新），取最后 `last` 步（0 = 整条链）。引擎给的会话是新 → 旧序。
fn chain_steps(file: &Path, last: usize) -> Result<Vec<(u32, u32)>, String> {
    let sessions: Vec<u32> = ReadOnlyEngine::open(file)
        .map_err(|error| format!("打不开：{error}"))?
        .sessions()
        .map_err(|error| format!("会话链读不出：{error}"))?
        .iter()
        .map(|session| session.session_id)
        .collect();
    if sessions.len() < 2 {
        return Err(format!("只有 {} 个会话", sessions.len()));
    }
    let mut steps: Vec<(u32, u32)> = sessions
        .windows(2)
        .take(if last == 0 { usize::MAX } else { last })
        .map(|pair| (pair[1], pair[0]))
        .collect();
    steps.reverse();
    Ok(steps)
}

/// 整条链并成一窗：最早的会话 → 最新的会话。最早取号不小于 2 的第一个——1 号多半是建库时的空会话，
/// 变更记录里记的真实命中（`aps250176_0001` 2→16 之类）也是从 2 号起。`chain` 是 [`chain_steps`] 给的整条链。
fn span_window(chain: &[(u32, u32)]) -> Option<(u32, u32)> {
    let newest = chain.last()?.1;
    let oldest = chain
        .iter()
        .map(|step| step.0)
        .find(|sesno| *sesno >= 2)
        .or_else(|| chain.first().map(|step| step.0))?;
    (oldest < newest).then_some((oldest, newest))
}

fn root_keys_of(roots: &[GenerationRoot]) -> BTreeSet<(u32, u32)> {
    roots
        .iter()
        .map(|root| key_of(refno_to_e3d(root.root)))
        .collect()
}

const JOINT_NOUNS: [&str; 6] = ["PJOI", "SJOI", "SUBJ", "SCOJ", "PCOJ", "FIXING"];
const BEAM_NOUNS: [&str; 4] = ["SCTN", "GENSEC", "STWALL", "WALL"];

/// 结构连接的同库对端，回指成立才算：关节（`CREF`）→ 梁，梁的 `JOIS` / `JOIE` 得指回来；梁（`JOIS` / `JOIE`）→
/// 关节，关节的 `CREF` 得指回来。读挂、跨库、对端不在都当没有。
fn connection_partners(set: &Arc<DbSet>, refno: RefNo, noun: &str) -> Vec<RefNo> {
    let element = set.element(refno);
    let mut out = Vec::new();
    if JOINT_NOUNS.contains(&noun) {
        if let Ok(Some(beam)) = element.get_ref("CREF")
            && beam.dbno() == refno.dbno()
        {
            let far = set.element(beam);
            let points_back = [far.get_ref("JOIS"), far.get_ref("JOIE")]
                .into_iter()
                .any(|joint| matches!(joint, Ok(Some(joint)) if joint == refno));
            if points_back {
                out.push(beam);
            }
        }
    } else if BEAM_NOUNS.contains(&noun) {
        for attribute in ["JOIS", "JOIE"] {
            if let Ok(Some(joint)) = element.get_ref(attribute)
                && joint.dbno() == refno.dbno()
                && matches!(set.element(joint).get_ref("CREF"), Ok(Some(beam)) if beam == refno)
                && !out.contains(&joint)
            {
                out.push(joint);
            }
        }
    }
    out
}

fn is_model_unit(set: &Arc<DbSet>, refno: RefNo) -> bool {
    matches!(
        e3d_model::category::is_model_unit_of(&set.element(refno)),
        Ok(true)
    )
}

/// 一件生成物拿来比的量：体积、世界包围盒、三角数（与 `window_root_plan` 探针的 `ParityInvariants` 同一口径，
/// 实体本来就在世界系里，包围盒带着位置）。
#[derive(Debug, Clone)]
struct UnitShape {
    volume: f64,
    aabb_min: [f64; 3],
    aabb_max: [f64; 3],
    triangles: usize,
}

impl UnitShape {
    fn of(generated: &GeneratedElement) -> Self {
        let mesh = e3d_model::TriMesh::from_manifold(&generated.solid);
        let (aabb_min, aabb_max) = mesh.aabb().unwrap_or(([0.0; 3], [0.0; 3]));
        Self {
            volume: generated.solid.volume(),
            aabb_min,
            aabb_max,
            triangles: mesh.triangle_count(),
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0);
        self.triangles == other.triangles
            && close(self.volume, other.volume)
            && self
                .aabb_min
                .iter()
                .zip(&other.aabb_min)
                .all(|(a, b)| close(*a, *b))
            && self
                .aabb_max
                .iter()
                .zip(&other.aabb_max)
                .all(|(a, b)| close(*a, *b))
    }
}

/// 照增量执行那条路（`element_category` → `model_element`）建一件；这一端没有它、或有依据地不出实体回 `None`。
fn shape_of_unit(set: &Arc<DbSet>, refno: RefNo) -> Result<Option<UnitShape>, String> {
    let element = set.element(refno);
    if !element.exists().map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let noun = element.element_type().map_err(|error| error.to_string())?;
    let category =
        e3d_model::category::element_category(&element, e3d_model::category::classify(&noun))
            .map_err(|error| error.to_string())?;
    match e3d_model::elmodl::model_element(&element, category).map_err(|error| error.to_string())? {
        e3d_model::elmodl::Modeled::Solid(generated) => Ok(Some(UnitShape::of(&generated))),
        e3d_model::elmodl::Modeled::Skipped { .. } => Ok(None),
    }
}

/// 梁建模时，`joint` 这只关节的目录刀在梁的账上留下的痕迹。e3d-model `elmodl::collect_joint_negatives` 对 `SCTN`
/// 的每只放行的关节恰好留一种：刀被 NAPP 门挡下、或有几何源却一把刀也没有 → `notes` 里一行带关节号；没有几何源
/// 或读挂 → `negatives_skipped` / `joint_cuts_explained` 里一条；什么都没留 = 它的刀全数交出、扣在梁上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JointCutTrace {
    /// `SCTN`、没留痕迹、梁上确有负体扣过：这只关节的刀扣在梁上——梁的几何吃这只关节。
    Applied,
    /// 有几何源、源里一把负体都没有：不出刀，梁的几何不吃它的刀。
    NoCutters,
    /// 没有几何源 / 读挂：不出刀。
    Skipped,
    /// 账上分不出：NAPP 门挡下了若干把（别的有没有扣上看不出），或梁不是 `SCTN`，或没留痕迹而梁上一把负体都没扣。
    Unclear,
    /// 梁这一端建不出实体。
    BeamNotModeled,
}

impl JointCutTrace {
    fn label(self) -> &'static str {
        match self {
            Self::Applied => "刀扣在梁上",
            Self::NoCutters => "有几何源、没有负体，不出刀",
            Self::Skipped => "没有几何源或读挂，不出刀",
            Self::Unclear => "账上分不出",
            Self::BeamNotModeled => "梁建不出实体",
        }
    }
}

fn joint_cut_trace(set: &Arc<DbSet>, beam: RefNo, joint: RefNo) -> JointCutTrace {
    let element = set.element(beam);
    let Ok(noun) = element.element_type() else {
        return JointCutTrace::BeamNotModeled;
    };
    let modeled =
        e3d_model::category::element_category(&element, e3d_model::category::classify(&noun))
            .ok()
            .and_then(|category| e3d_model::elmodl::model_element(&element, category).ok());
    let Some(e3d_model::elmodl::Modeled::Solid(generated)) = modeled else {
        return JointCutTrace::BeamNotModeled;
    };
    if generated
        .negatives_skipped
        .iter()
        .chain(generated.joint_cuts_explained.iter())
        .any(|skipped| skipped.refno == joint)
    {
        return JointCutTrace::Skipped;
    }
    let tag = format!("关节 {joint}：");
    if let Some(note) = generated.notes.iter().find(|note| note.contains(&tag)) {
        return if note.contains("没有负体") {
            JointCutTrace::NoCutters
        } else {
            JointCutTrace::Unclear
        };
    }
    if noun == "SCTN" && generated.negatives_applied > 0 {
        JointCutTrace::Applied
    } else {
        JointCutTrace::Unclear
    }
}

/// 梁建模时一共扣了几把负体（建不出实体给 0）。与 [`joint_cut_trace`] 合着看：账上分不出的那一类（NAPP 门挡下了
/// 若干把）靠它知道其余的刀是不是真扣上了。
fn negatives_applied_on(set: &Arc<DbSet>, beam: RefNo) -> usize {
    let element = set.element(beam);
    let Ok(noun) = element.element_type() else {
        return 0;
    };
    let modeled =
        e3d_model::category::element_category(&element, e3d_model::category::classify(&noun))
            .ok()
            .and_then(|category| e3d_model::elmodl::model_element(&element, category).ok());
    match modeled {
        Some(e3d_model::elmodl::Modeled::Solid(generated)) => {
            usize::try_from(generated.negatives_applied).unwrap_or(0)
        }
        _ => 0,
    }
}

/// 这一端的两次建模结果变没变：`Some(true)` = 变了，`None` = 有一端建不出来（读挂 / 建模失败）。
fn unit_changed(base: &Arc<DbSet>, target: &Arc<DbSet>, refno: RefNo) -> Option<bool> {
    match (shape_of_unit(base, refno), shape_of_unit(target, refno)) {
        (Ok(Some(before)), Ok(Some(after))) => Some(!before.same_as(&after)),
        (Ok(None), Ok(None)) => Some(false),
        (Ok(_), Ok(_)) => Some(true),
        _ => None,
    }
}

/// 整根生成一次，逐件记形状。
fn shapes_under_root(
    set: &Arc<DbSet>,
    root: RefNo,
) -> Result<BTreeMap<GeometryId, UnitShape>, String> {
    let outcome =
        e3d_model::generate_subtree(set.element(root)).map_err(|error| error.to_string())?;
    Ok(outcome
        .elements
        .iter()
        .map(|generated| (generated.geometry_id.clone(), UnitShape::of(generated)))
        .collect())
}

/// 一根在窗口两端各整根生成一次的对照。
struct WholeRootDiff {
    /// S 端整根生成出的几何件数——回执上的 `geometry_count` 就是这个数，单元级发布对少于
    /// [`SMALL_ROOT_UNITS`] 条的根本来就回退整根。
    records_at_s: usize,
    records_at_t: usize,
    /// 两端形状不同（或只在一端有）的件数。
    changed: usize,
    /// 其中来源单元不在子计划里的：局部重建不会去碰它，整根重算才会带上。
    stale: Vec<String>,
}

impl WholeRootDiff {
    fn line(&self) -> String {
        format!(
            "两端整根各生成一次：S {} 件 / T {} 件，形状变了 {} 件，其中不在子计划里（局部重建会漏掉）{} 件{}",
            self.records_at_s,
            self.records_at_t,
            self.changed,
            self.stale.len(),
            if self.stale.is_empty() {
                String::new()
            } else {
                format!(
                    "：{}",
                    self.stale
                        .iter()
                        .take(6)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("、")
                )
            }
        )
    }
}

fn whole_root_diff(
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
    root: RefNo,
    subplan: &RootSubplan,
) -> Result<WholeRootDiff, String> {
    let before = shapes_under_root(base, root)?;
    let after = shapes_under_root(target, root)?;
    let ids: BTreeSet<&GeometryId> = before.keys().chain(after.keys()).collect();
    let mut changed = 0usize;
    let mut stale: Vec<String> = Vec::new();
    for id in ids {
        let same = match (before.get(id), after.get(id)) {
            (Some(a), Some(b)) => a.same_as(b),
            _ => false,
        };
        if same {
            continue;
        }
        changed += 1;
        let covered = match geometry_source_refno(id) {
            Ok(source) => {
                let source = key_of(source);
                subplan.regenerate.contains(&source)
                    || subplan.remove.contains(&source)
                    || subplan.regenerate_derived.contains(&source)
                    || subplan.retired_containers.contains(&source)
            }
            Err(_) => false,
        };
        if !covered {
            stale.push(format!("{id:?}"));
        }
    }
    Ok(WholeRootDiff {
        records_at_s: before.len(),
        records_at_t: after.len(),
        changed,
        stale,
    })
}

/// 假定窗：`candidates` 是这一窗仅有的候选。按**新增**候选给——修改候选要两端真有差异才分得出改了什么，
/// 同一个会话上没有；而 `plan_update_with` 把候选上卷成单元之后，关节级联 / 宿主级联对新增与修改是同一段代码
/// （都从 `regenerate ∪ remove` 出发）。
fn hypothetical_window(sesno: u32, candidates: &[RefNo]) -> e3d_model::increment::WindowDiff {
    let nowhere = e3d_io::index::RecordPosition {
        page: e3d_io::page::PageId::new(1, 0),
        offset_words: 0,
    };
    e3d_model::increment::WindowDiff {
        base_sesno: sesno,
        target_sesno: sesno,
        base_root: nowhere.page,
        target_root: nowhere.page,
        diff: e3d_io::index::CompletedIndexDiff {
            changes: candidates
                .iter()
                .map(|&refno| e3d_io::index::IndexCandidate::Inserted {
                    refno,
                    target: nowhere,
                })
                .collect(),
            tally: Default::default(),
            diagnostics: Default::default(),
            io: Default::default(),
        },
    }
}

/// 一个库在某个会话上的静态现场。
struct Site {
    file: PathBuf,
    dbnum: u32,
    sesno: u32,
    set: Arc<DbSet>,
    owners: Arc<BTreeMap<(u32, u32), RefNo>>,
    roots: Vec<GenerationRoot>,
    root_keys: BTreeSet<(u32, u32)>,
}

/// 假定窗在一个根上的事实。
#[derive(Debug, Clone, Copy, Default)]
struct RootFacts {
    /// 这一窗的根规划退化了（整窗不带单元级提示）。假定窗只有一两个候选，不该发生。
    degraded: bool,
    /// 这根在本窗的 regen 里。
    in_regen: bool,
    /// 这根是某条开着的级联的目标。
    cascaded: bool,
    /// 这根的子计划不空。
    subplan_names_something: bool,
    /// 子计划里有被牵连的那一件。
    subplan_names_implicated: bool,
    /// 这根的任务带单元级提示。
    hinted: bool,
}

impl RootFacts {
    /// #144 的形状：根在 regen 里、子计划不空、被牵连的那一件不在子计划里。
    fn leaves_implicated_out(&self) -> bool {
        !self.degraded
            && self.in_regen
            && self.subplan_names_something
            && !self.subplan_names_implicated
    }
}

impl Site {
    fn open(project: &Project, file: &Path, sesno: u32) -> Self {
        let set = project.set_at(file, sesno).expect("pin the DESI file");
        let index = scan_index(file, Some(sesno)).expect("scan index");
        let roots =
            enumerate_generation_roots(&set, &index.roots, &resolve_delivery_unit_types(&[]))
                .expect("enumerate generation roots");
        let root_keys = root_keys_of(&roots);
        Self {
            file: file.to_path_buf(),
            dbnum: index.dbnum,
            sesno,
            set,
            owners: index.owners.clone(),
            roots,
            root_keys,
        }
    }

    /// `element` 属主链上的根，自下而上（第一个是最近的根）。
    fn root_chain(&self, element: RefNo) -> Vec<RefNo> {
        roots_on_owner_chain(&self.owners, &self.root_keys, element)
    }

    /// 假定 `candidates` 是这一窗仅有的候选：e3d-model 真的 `plan_update_with` 给计划，真的 `affected_closure` 给
    /// 闭包；C2 开着就用生产同一段 `StructuralConnectionCascade::collect` 读边，关着就是 `off()`。C8 / P4 按缺省
    /// 开着、目标集空（一只关节或一根梁不是它们的源）。然后问根 `root`：在不在 regen、是不是级联目标、子计划里
    /// 有什么、任务带不带单元级提示。同时回 `plan_update` 点名要重建的单元。
    fn hypothetical(
        &self,
        candidates: &[RefNo],
        c2_on: bool,
        root: RefNo,
        implicated: RefNo,
    ) -> (RootFacts, BTreeSet<(u32, u32)>) {
        let window = hypothetical_window(self.sesno, candidates);
        let plan = plan_update_with(
            &self.set,
            &self.set,
            &window,
            PlanLimits::default(),
            change_metadata_tier,
        );
        let named = plan.regenerate.clone();
        let closure = plan.affected_closure(&self.set, &self.set);
        let structural_connection = if c2_on {
            StructuralConnectionCascade::collect(
                &plan,
                &self.set,
                &self.set,
                &self.owners,
                &self.roots,
            )
        } else {
            StructuralConnectionCascade::off()
        };
        let mut sources = WindowRootSources {
            base_sesno: self.sesno,
            target_sesno: self.sesno,
            roots_s: self.roots.iter().map(PersistedRoot::from).collect(),
            roots_t: self.roots.clone(),
            plan,
            closure,
            index_keys: self.owners.len(),
            xgeometry_observed: 0,
            xgeometry_unreadable: 0,
            nested_roots: 0,
            duplicate_coverage_elements: 0,
            structural_connection,
            cable_endpoint: Default::default(),
            design_reference: Default::default(),
            rigid: Default::default(),
            removed_units: Vec::new(),
            changes: Default::default(),
            timings: Default::default(),
        };
        sources.cable_endpoint.enabled = true;
        sources.design_reference.enabled = true;
        let impact = sources.impact(&sources.roots_s);
        let root_plan = plan_window_roots(&sources.roots_t, &sources.roots_s, &impact);
        let root_enum = refno_from_e3d(root);
        let sets = window_plan_sets(
            self.dbnum,
            &self.file,
            self.sesno,
            self.sesno,
            &sources.plan,
            None,
        );
        let subplan = restrict_plan_to_root(&sets, root, &self.owners, &self.owners)
            .expect("restrict the hypothetical plan to the root");
        let tasks = window_unit_tasks(self.dbnum, &sources, &sources.roots_s, &impact, &root_plan);
        let facts = RootFacts {
            degraded: root_plan.degraded.is_some(),
            in_regen: root_plan
                .regen
                .iter()
                .any(|candidate| candidate.root == root_enum),
            cascaded: sources.cascaded_roots().contains(&root_enum),
            subplan_names_something: !subplan.is_empty(),
            subplan_names_implicated: subplan.regenerate.contains(&key_of(implicated)),
            hinted: task_of_root(&tasks, root_enum).is_some_and(|task| task.unit_publish.is_some()),
        };
        (facts, named)
    }
}

/// 假定源在一类形状上的账。
#[derive(Debug, Default)]
struct ShapeTally {
    /// 走过假定窗的对数。
    checked: usize,
    /// `plan_update` 连变的那一端都没点名（它在这个会话上不成单元 / 读挂）——不算进下面任何一格。
    changed_end_not_named: usize,
    /// `plan_update` 自己就把对端点了名（不是这张票的事）。
    partner_named_by_plan: usize,
    /// 对端所在的根子计划为空（今天回退整根重算，是对的）或假定窗退化了。
    whole_root_today: usize,
    /// #144 的形状：变的那一端被点名、对端没被点名，对端所在的根在 regen 里、子计划不空、里面没有对端。
    left_out: usize,
    /// 那些里，C2 关着时：这根不是任何级联的目标 / 任务带单元级提示。
    c2_off_not_cascaded: usize,
    c2_off_hinted: usize,
    /// 那些里，C2 开着时：这根是级联目标 / 是级联目标而任务仍带单元级提示 / 不是级联目标（边没过 `STRU` 门之类）。
    c2_on_cascaded: usize,
    c2_on_cascaded_hinted: usize,
    c2_on_not_cascaded: usize,
    /// 第一对成立 #144 形状的，留一行给人看。
    example: Option<String>,
}

impl ShapeTally {
    /// 记一对。`changed` / `partner` 是变的那一端与对端，`named` 是 `plan_update` 点名要重建的单元。
    fn record(
        &mut self,
        what: &str,
        changed: RefNo,
        partner: RefNo,
        root: RefNo,
        named: &BTreeSet<(u32, u32)>,
        off: RootFacts,
        on: RootFacts,
    ) {
        self.checked += 1;
        if !named.contains(&key_of(changed)) {
            self.changed_end_not_named += 1;
            return;
        }
        if named.contains(&key_of(partner)) {
            self.partner_named_by_plan += 1;
            return;
        }
        if !(off.leaves_implicated_out() && on.leaves_implicated_out()) {
            self.whole_root_today += 1;
            return;
        }
        self.left_out += 1;
        self.c2_off_not_cascaded += usize::from(!off.cascaded);
        self.c2_off_hinted += usize::from(off.hinted);
        if on.cascaded {
            self.c2_on_cascaded += 1;
            self.c2_on_cascaded_hinted += usize::from(on.hinted);
        } else {
            self.c2_on_not_cascaded += 1;
        }
        if self.example.is_none() {
            self.example = Some(format!(
                "{what}：变的 {changed}，对端 {partner}，对端的根 {root}；plan_update 点名 {named:?}；C2 关 {off:?}；C2 开 {on:?}"
            ));
        }
    }

    fn add(&mut self, other: &Self) {
        self.checked += other.checked;
        self.changed_end_not_named += other.changed_end_not_named;
        self.partner_named_by_plan += other.partner_named_by_plan;
        self.whole_root_today += other.whole_root_today;
        self.left_out += other.left_out;
        self.c2_off_not_cascaded += other.c2_off_not_cascaded;
        self.c2_off_hinted += other.c2_off_hinted;
        self.c2_on_cascaded += other.c2_on_cascaded;
        self.c2_on_cascaded_hinted += other.c2_on_cascaded_hinted;
        self.c2_on_not_cascaded += other.c2_on_not_cascaded;
        if self.example.is_none() {
            self.example = other.example.clone();
        }
    }

    fn line(&self) -> String {
        format!(
            "走过 {} 对：变的那端没被点名 {}，plan_update 自己点了对端 {}，对端的根今天就整根重算 {}，#144 的形状 {}。\
             那 {} 对里：C2 关——不是级联目标 {}、任务带单元级提示 {}；C2 开——是级联目标 {}（其中任务仍带单元级提示 {}）、不是级联目标 {}",
            self.checked,
            self.changed_end_not_named,
            self.partner_named_by_plan,
            self.whole_root_today,
            self.left_out,
            self.left_out,
            self.c2_off_not_cascaded,
            self.c2_off_hinted,
            self.c2_on_cascaded,
            self.c2_on_cascaded_hinted,
            self.c2_on_not_cascaded
        )
    }
}

/// GitHub #144，真库上的假定源：带目录正体的 `PJOI`（自己是模型单元）与它 `CREF` 的梁（梁的 `JOIS` / `JOIE` 指回它）。
///
/// 两端互相吃：梁建模时扣关节给的刀（e3d-model `elmodl::collect_joint_negatives` 接受 `JOIS` / `JOIE` 指向的
/// `PJOI`；aps7166 的端板把被接的柱削短 10～25 mm），关节的原点在垂直于关节 Z 的平面里挪到梁 `JLIN` 那条线上、
/// 目录件经 `APAR` 读梁（`datum::primary_joint_world`）。
///
/// e3d-model `plan_update` 自己的同库关节级联原先只认 FIXING ↔ GENSEC 与 SJOI ↔ SCTN，这一对它两个方向都不点名
/// （2026-11-11 这条测试钉的就是那个缺口：2,145 个假定窗全部成立）。**2026-11-12 起 `increment::joint_beam_noun` /
/// `joint_fixings_of` 接上了主关节**，这条测试改钉接上之后的样子。
///
/// 三种形状各走一遍假定窗（把一端当成一窗仅有的候选，喂给真的 `plan_update_with`）：
/// - **同根，关节变**：梁最近的根在关节的根链上。候选 = 关节；看梁最近的根。只走梁是 `SCTN` 的对——`GENSEC` 不吃
///   主关节的负体，这个方向对它不成立。
/// - **同根，梁变**：关节最近的根在梁的根链上。候选 = 梁；看关节最近的根。
/// - **跨根，关节变 + 对端根另有改动**：梁最近的根不在关节的根链上。候选 = 关节，加上梁那个根名下另一件模型单元；
///   看梁最近的根。
///
/// 每一对两态（C2 关 / 开）各问一次：根在不在 regen、是不是级联目标、子计划、任务带不带单元级提示。
///
/// 断言：
/// 1. ams7999 里同根那两种形状都**存在**（语料前提）；
/// 2. 变的那一端被点名的每一对，`plan_update` 自己把对端也点了名——不靠 C2，两个方向、同根跨根都是；#144 那种
///    「对端留在旧几何上」的形状一对都不剩；
/// 3. aps7166 在本机时，同根的对里至少有一根梁真的被这只关节的目录刀扣过（「关节变了梁的几何要跟」在生成这一侧
///    的依据；ams7999 / ams251183 那 1,071 对里是 0 根——它们成立的是反方向）。
///
/// 只读文件：`AIOS_AMS_PROJECT_DIR` / `AIOS_APS_PROJECT_DIR` / `AIOS_ACP_PROJECT_DIR`（缺省本机样例工程）+
/// `AIOS_E3D_TEMPLATE_DIR`。每类最多走 `AIOS_144_PAIRS_PER_CLASS` 对（缺省 60）。会话取各库文件的最新会话。
#[test]
#[ignore = "manual live: reads real DESI files of the AMS / APS sample projects; needs the E3D template directory"]
fn live_a_catalogue_joint_and_the_beam_it_cuts_are_planned_together() {
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let per_class = env_usize("AIOS_144_PAIRS_PER_CLASS", 60);
    // 第一本是 #144 票面点名的（关节与梁同根）；中间两本关节与梁常在不同的根下，给跨根那一种形状；最后一本是端板
    // 削柱的那一本（E3D 取证过）。
    let libraries: [(&Path, &str); 4] = [
        (ams.as_path(), "ams7999_0001"),
        (ams.as_path(), "ams251183_0001"),
        (aps.as_path(), "aps7200_0001"),
        (aps.as_path(), "aps7166_0001"),
    ];
    let mut aps7166_present = false;
    if !libraries[0].0.join(libraries[0].1).is_file() {
        eprintln!(
            "skip: 语料不在本机（{}）——装了 E3D 样例库的机器上这条会真跑",
            libraries[0].0.join(libraries[0].1).display()
        );
        return;
    }
    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    let pjoi_hash = e3d_attlib::db1_hash("PJOI");
    let mut joint_changed = ShapeTally::default();
    let mut beam_changed = ShapeTally::default();
    let mut cross_root = ShapeTally::default();
    let mut ams7999_same_root = 0usize;
    let (mut beams_checked, mut beams_cut_by_their_joint) = (0usize, 0usize);

    for (project_dir, name) in libraries {
        let file = project_dir.join(name);
        if !file.is_file() {
            println!("[#144] skip {name}: 文件不在");
            continue;
        }
        let sesno = match chain_steps(&file, 1) {
            Ok(steps) => steps[0].1,
            Err(reason) => {
                println!("[#144] skip {name}: {reason}");
                continue;
            }
        };
        let project = projects
            .entry(project_dir.to_path_buf())
            .or_insert_with(|| Project::open(&schema, project_dir));
        let site = Site::open(project, &file, sesno);

        // 库里的 PJOI：带目录正体的（自己是模型单元）、其中 `CREF` 的梁回指成立且梁也是模型单元的。
        let mut engine = ReadOnlyEngine::open_at(&file, sesno).expect("open the DESI file");
        let joints: Vec<RefNo> = engine
            .scan_elements(ScanTier::Header)
            .expect("scan element headers")
            .filter_map(|scanned| scanned.ok())
            .filter(|scanned| scanned.noun_hash == pjoi_hash)
            .map(|scanned| scanned.refno)
            .collect();
        drop(engine);
        let mut drawn = 0usize;
        // (关节, 梁, 梁最近的根, 关节最近的根, 梁最近的根在关节的根链上)
        let mut pairs: Vec<(RefNo, RefNo, RefNo, RefNo, bool)> = Vec::new();
        for &joint in &joints {
            if !is_model_unit(&site.set, joint) {
                continue;
            }
            drawn += 1;
            for beam in connection_partners(&site.set, joint, "PJOI") {
                if !is_model_unit(&site.set, beam) {
                    continue;
                }
                let joint_chain = site.root_chain(joint);
                let beam_chain = site.root_chain(beam);
                let (Some(&joint_root), Some(&beam_root)) =
                    (joint_chain.first(), beam_chain.first())
                else {
                    continue;
                };
                pairs.push((
                    joint,
                    beam,
                    beam_root,
                    joint_root,
                    joint_chain.contains(&beam_root),
                ));
            }
        }
        let same_root: Vec<(RefNo, RefNo, RefNo, RefNo, bool)> =
            pairs.iter().copied().filter(|pair| pair.4).collect();
        let other_root: Vec<(RefNo, RefNo, RefNo, RefNo, bool)> =
            pairs.iter().copied().filter(|pair| !pair.4).collect();
        println!(
            "[#144] {name}@{sesno}: 元素 {} 根 {}；PJOI {} 只，带目录正体（模型单元）{} 只；与梁回指成立且梁是模型单元 {} 对\
             ——梁最近的根在关节根链上 {} 对，不在 {} 对",
            site.owners.len(),
            site.roots.len(),
            joints.len(),
            drawn,
            pairs.len(),
            same_root.len(),
            other_root.len()
        );
        if name == "ams7999_0001" {
            ams7999_same_root = same_root.len();
        }
        if name == "aps7166_0001" {
            aps7166_present = true;
        }
        // 正向（关节变 → 梁）只对吃主关节负体的梁成立：`SCTN`。
        let takes_joint_cuts = |beam: RefNo| {
            site.set
                .element(beam)
                .element_type()
                .is_ok_and(|noun| noun == "SCTN")
        };

        // 同根：关节变 → 看梁的根；梁变 → 看关节的根。
        let mut joint_tally = ShapeTally::default();
        let mut beam_tally = ShapeTally::default();
        for &(joint, beam, beam_root, joint_root, _) in same_root.iter().take(per_class) {
            if takes_joint_cuts(beam) {
                let (off, named) = site.hypothetical(&[joint], false, beam_root, beam);
                let (on, _) = site.hypothetical(&[joint], true, beam_root, beam);
                joint_tally.record(name, joint, beam, beam_root, &named, off, on);
            }
            // 反方向只在关节最近的根也在梁的根链上时才是「同根」。
            if !site.root_chain(beam).contains(&joint_root) {
                continue;
            }
            let (off, named) = site.hypothetical(&[beam], false, joint_root, joint);
            let (on, _) = site.hypothetical(&[beam], true, joint_root, joint);
            beam_tally.record(name, beam, joint, joint_root, &named, off, on);
        }
        println!("[#144]   同根，关节变：{}", joint_tally.line());
        println!("[#144]   同根，梁变：{}", beam_tally.line());
        joint_changed.add(&joint_tally);
        beam_changed.add(&beam_tally);

        // 「关节变了梁的几何要跟」在真库上的依据：同根那几对里，梁建模时这只关节的目录刀留下的痕迹（只给人看）。
        let mut traces: BTreeMap<&'static str, usize> = BTreeMap::new();
        for &(joint, beam, ..) in same_root.iter().take(per_class) {
            let trace = joint_cut_trace(&site.set, beam, joint);
            beams_checked += 1;
            // 「账上分不出」里有一种是 NAPP 门挡下了若干把、其余照交（aps7166 的端板：4 把螺栓孔挡下，削短柱的
            // NSEX 照交）——梁是 SCTN 且确实扣过负体时也算。
            let cut = trace == JointCutTrace::Applied
                || (trace == JointCutTrace::Unclear
                    && takes_joint_cuts(beam)
                    && negatives_applied_on(&site.set, beam) > 0);
            beams_cut_by_their_joint += usize::from(cut);
            *traces.entry(trace.label()).or_default() += 1;
        }
        println!("[#144]   同根那几对里，梁建模时这只关节的目录刀：{traces:?}");

        // 跨根：关节变，梁那个根名下另有一件模型单元也变了。
        let mut cross_tally = ShapeTally::default();
        if !other_root.is_empty() {
            // 每个根名下（按最近的根归属）的元素，只为挑「另一件」。
            let mut members: BTreeMap<(u32, u32), Vec<RefNo>> = BTreeMap::new();
            for &(word0, word1) in site.owners.keys() {
                let element = RefNo::new(word0, word1);
                if let Some(nearest) = site.root_chain(element).first() {
                    members.entry(key_of(*nearest)).or_default().push(element);
                }
            }
            for &(joint, beam, beam_root, _, _) in other_root.iter().take(per_class) {
                if !takes_joint_cuts(beam) {
                    continue;
                }
                // 「另一件」：梁那个根名下、这一对两端之外的第一件模型单元（结构根里多半是另一根梁）。挑不到就不走
                // 这一对。
                let other = members
                    .get(&key_of(beam_root))
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|candidate| *candidate != beam && *candidate != joint)
                    .find(|candidate| is_model_unit(&site.set, *candidate));
                let Some(other) = other else {
                    continue;
                };
                let (off, named) = site.hypothetical(&[joint, other], false, beam_root, beam);
                let (on, _) = site.hypothetical(&[joint, other], true, beam_root, beam);
                cross_tally.record(name, joint, beam, beam_root, &named, off, on);
            }
        }
        println!(
            "[#144]   跨根，关节变 + 对端根另有改动：{}",
            cross_tally.line()
        );
        cross_root.add(&cross_tally);
    }

    for (what, tally) in [
        ("同根，关节变", &joint_changed),
        ("同根，梁变", &beam_changed),
        ("跨根，关节变 + 对端根另有改动", &cross_root),
    ] {
        println!("[#144] ==== {what}：{}", tally.line());
        if let Some(example) = &tally.example {
            println!("[#144]      例：{example}");
        }
    }
    println!(
        "[#144] ==== 同根的对里建过 {beams_checked} 根梁，其中扣过负体、且这只关节交出过刀的 {beams_cut_by_their_joint} 根\
         （梁的几何吃这只关节：关节变了、梁没重建，梁就留在旧刀上）"
    );

    // 1. 语料前提：ams7999 里这一形状存在，两个方向都走到了。
    assert!(
        ams7999_same_root > 0,
        "ams7999 里没有「带目录正体的 PJOI 与同根的梁」——语料变了"
    );
    assert!(
        joint_changed.checked > 0 && beam_changed.checked > 0,
        "两个方向都该走到假定窗：关节变 {}；梁变 {}",
        joint_changed.line(),
        beam_changed.line()
    );
    for (what, tally) in [
        ("同根，关节变", &joint_changed),
        ("同根，梁变", &beam_changed),
        ("跨根，关节变 + 对端根另有改动", &cross_root),
    ] {
        // 2. 接上之后：变的那一端被点名的每一对，`plan_update` 自己把对端也点了名（不靠 C2）。
        assert_eq!(
            tally.partner_named_by_plan,
            tally.checked - tally.changed_end_not_named,
            "{what}：变的那一端被点名的每一对，对端都该由 e3d-model 的关节级联点名：{}",
            tally.line()
        );
        assert_eq!(
            (tally.left_out, tally.whole_root_today),
            (0, 0),
            "{what}：不该再有「对端留在旧几何上」的对：{}",
            tally.line()
        );
    }
    assert!(
        joint_changed.partner_named_by_plan > 0 && beam_changed.partner_named_by_plan > 0,
        "两个方向都该有对端被点名的对：关节变 {}；梁变 {}",
        joint_changed.line(),
        beam_changed.line()
    );
    // 3. 生成这一侧的依据：aps7166 的端板把被接的柱削短了。
    if aps7166_present {
        assert!(
            beams_cut_by_their_joint > 0,
            "aps7166 在本机，同根的对里该有被这只关节的目录刀扣过的梁（建过 {beams_checked} 根）"
        );
    }
}

/// 主关节的关节级联在缺省配置（C2 关）下的样子，aps7166 真库、假定窗。随整套跑；语料不在本机就跳过。
///
/// aps7166 `/SB3` 的端板主关节挂在 `PNOD ← SBFR` 下，目录件 `/Endplates-Column-JOINT` 的 NSEX 把被接的柱削短
/// 10～25 mm（E3D 导出的柱同样短这么多，2026-11-11 取证）。e3d-model 的关节级联 2026-11-12 接上主关节
/// （`increment::joint_beam_noun` / `joint_fixings_of` / `implicit_frame_host_of`）之后，下面几件事都不靠 C2：
///
/// 1. 只改主关节 → 被它削的柱被 `plan_update` 点名，柱所在的根在 regen 里、子计划里有这根柱；
/// 2. 只改柱 → 主关节被点名，它所在的根的子计划里有它；
/// 3. 只改 PNOD（主关节的站位读它的 `NPOS`）→ 主关节被点名，柱跟着被点名；
/// 4. 不相关的改动不拉人：改一根没有主关节认领的型钢，一只主关节都不点名；改一只没有梁认领的主关节，只点名它自己。
///
/// 每一种都把一端当成一窗仅有的候选，喂给真的 `plan_update_with`（[`Site::hypothetical`]）。
#[test]
fn live_aps7166_a_primary_joint_and_the_column_it_cuts_are_planned_together() {
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let file = aps.join("aps7166_0001");
    if !file.is_file() {
        eprintln!("skip: 语料不在本机（{}）", file.display());
        return;
    }
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let sesno = chain_steps(&file, 1).expect("aps7166 的会话链")[0].1;
    let project = Project::open(&schema, &aps);
    let site = Site::open(&project, &file, sesno);
    let noun_of = |refno: RefNo| {
        site.set
            .element(refno)
            .element_type()
            .unwrap_or_else(|_| "?".into())
    };
    let named_joints = |named: &BTreeSet<(u32, u32)>| -> Vec<RefNo> {
        named
            .iter()
            .map(|&(word0, word1)| RefNo::new(word0, word1))
            .filter(|refno| noun_of(*refno) == "PJOI")
            .collect()
    };

    let (pjoi_hash, sctn_hash) = (e3d_attlib::db1_hash("PJOI"), e3d_attlib::db1_hash("SCTN"));
    let mut engine = ReadOnlyEngine::open_at(&file, sesno).expect("open the DESI file");
    let (mut joints, mut sections): (Vec<RefNo>, Vec<RefNo>) = (Vec::new(), Vec::new());
    for scanned in engine
        .scan_elements(ScanTier::Header)
        .expect("scan element headers")
        .filter_map(|scanned| scanned.ok())
    {
        if scanned.noun_hash == pjoi_hash {
            joints.push(scanned.refno);
        } else if scanned.noun_hash == sctn_hash {
            sections.push(scanned.refno);
        }
    }
    drop(engine);

    // 带目录正体（自成单元）的主关节：有 SCTN 认领的成对，没有的单列。
    let mut pairs: Vec<(RefNo, RefNo)> = Vec::new();
    let mut unclaimed: Vec<RefNo> = Vec::new();
    for &joint in &joints {
        if !is_model_unit(&site.set, joint) {
            continue;
        }
        let beams: Vec<RefNo> = connection_partners(&site.set, joint, "PJOI")
            .into_iter()
            .filter(|beam| noun_of(*beam) == "SCTN" && is_model_unit(&site.set, *beam))
            .collect();
        match beams.first() {
            Some(&beam) => pairs.push((joint, beam)),
            None => unclaimed.push(joint),
        }
    }
    println!(
        "[PJOI] aps7166@{sesno}: PJOI {} 只，自成单元且有 SCTN 认领 {} 对，自成单元而没有梁认领 {} 只；SCTN {} 根",
        joints.len(),
        pairs.len(),
        unclaimed.len(),
        sections.len()
    );
    assert!(
        !pairs.is_empty(),
        "aps7166 里该有带目录正体、被 SCTN 认领的主关节（端板 `/Endplates-Column-JOINT` 那 7 只）"
    );

    let claimed_beams: BTreeSet<(u32, u32)> = pairs.iter().map(|(_, beam)| key_of(*beam)).collect();
    let mut cut = 0usize;
    let (mut by_unit_when_c2_off, mut whole_root_when_c2_on) = (0usize, 0usize);
    let mut traces: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &(joint, beam) in &pairs {
        let (Some(&beam_root), Some(&joint_root)) = (
            site.root_chain(beam).first(),
            site.root_chain(joint).first(),
        ) else {
            panic!("{joint} / {beam}：两端都该在某个生成根下");
        };
        // 1. 只改主关节。
        let (facts, named) = site.hypothetical(&[joint], false, beam_root, beam);
        assert!(
            named.contains(&key_of(joint)) && named.contains(&key_of(beam)),
            "只改主关节 {joint}：它自己与被它削的柱 {beam} 都该被点名：{named:?}"
        );
        assert!(
            !facts.degraded && facts.in_regen && facts.subplan_names_implicated,
            "只改主关节 {joint}：柱 {beam} 的根 {beam_root} 在 regen 里、子计划里有这根柱：{facts:?}"
        );
        assert!(!facts.cascaded, "C2 关着：这不是 gen-model 哪条级联的事");
        // 同一窗 C2 开着：柱的根成了结构连接级联的目标，#144 的规则让它整根重算（不带单元级提示）——结果一样对，
        // 只是从「多重建一根柱」变成「整根过一遍」。数出来给 #34 的决策材料用，不下断言之外的结论。
        let (on, _) = site.hypothetical(&[joint], true, beam_root, beam);
        by_unit_when_c2_off += usize::from(facts.hinted);
        whole_root_when_c2_on += usize::from(on.cascaded && !on.hinted);
        assert!(
            !(on.cascaded && on.hinted),
            "C2 开着、柱的根是级联目标时不许带单元级提示（GitHub #144）：{on:?}"
        );
        // 2. 只改柱。
        let (facts, named) = site.hypothetical(&[beam], false, joint_root, joint);
        assert!(
            named.contains(&key_of(beam)) && named.contains(&key_of(joint)),
            "只改柱 {beam}：它自己与它认领的主关节 {joint} 都该被点名：{named:?}"
        );
        assert!(
            !facts.degraded && facts.in_regen && facts.subplan_names_implicated,
            "只改柱 {beam}：主关节 {joint} 的根 {joint_root} 在 regen 里、子计划里有这只主关节：{facts:?}"
        );
        // 3. 只改 PNOD。
        let node = site
            .set
            .element(joint)
            .owner()
            .expect("read the owner")
            .expect("PJOI 的属主")
            .refno();
        assert_eq!(noun_of(node), "PNOD");
        let (_, named) = site.hypothetical(&[node], false, joint_root, joint);
        assert!(
            named.contains(&key_of(joint)) && named.contains(&key_of(beam)),
            "只改 PNOD {node}：它名下的主关节 {joint} 与被主关节削的柱 {beam} 都该被点名：{named:?}"
        );
        // 生成这一侧的依据：柱建模时扣过负体，而这只主关节交出过刀。端板的目录件有 9 把负体，NAPP 门在关节路径上
        // 挡下 4 把螺栓孔（E3D 同样不拿它们切柱），削短柱的那把 NSEX 照交——账上是「刀扣在梁上」或「挡下若干把」。
        let trace = joint_cut_trace(&site.set, beam, joint);
        let applied = negatives_applied_on(&site.set, beam);
        *traces.entry(trace.label()).or_default() += 1;
        cut += usize::from(
            applied > 0 && matches!(trace, JointCutTrace::Applied | JointCutTrace::Unclear),
        );
    }
    println!(
        "[PJOI] aps7166: {} 对里，柱建模时扣过负体、且这只主关节交出过刀的 {cut} 根；这只主关节在柱的账上：{traces:?}",
        pairs.len()
    );
    assert!(
        cut > 0,
        "aps7166 的端板把被接的柱削短——{} 对里该有扣过主关节的刀的柱：{traces:?}",
        pairs.len()
    );
    println!(
        "[PJOI] aps7166: 只改主关节的 {} 个假定窗里，C2 关着时柱的根走单元级发布（只重建点名的几件）{by_unit_when_c2_off} 个；\
         C2 开着时柱的根成了级联目标、整根重算 {whole_root_when_c2_on} 个",
        pairs.len()
    );

    // 4a. 改一根没有主关节认领的型钢：一只主关节都不点名。
    let free_section = sections.iter().copied().find(|section| {
        !claimed_beams.contains(&key_of(*section))
            && is_model_unit(&site.set, *section)
            && ["JOIS", "JOIE"].iter().all(|end| {
                site.set
                    .element(*section)
                    .get_ref(end)
                    .ok()
                    .flatten()
                    .is_none_or(|joint| noun_of(joint) != "PJOI")
            })
    });
    let free_section = free_section.expect("aps7166 里该有两端都不接主关节的型钢");
    let free_root = *site
        .root_chain(free_section)
        .first()
        .expect("型钢在某个生成根下");
    let (_, named) = site.hypothetical(&[free_section], false, free_root, free_section);
    assert!(
        named.contains(&key_of(free_section)),
        "{free_section} 自己该被点名"
    );
    assert_eq!(
        named_joints(&named),
        Vec::<RefNo>::new(),
        "改一根没有主关节认领的型钢 {free_section}，不该点名任何主关节：{named:?}"
    );
    // 4b. 改一只没有梁认领的主关节：只点名它自己。
    match unclaimed.first() {
        Some(&lonely) => {
            let root = *site
                .root_chain(lonely)
                .first()
                .expect("主关节在某个生成根下");
            let (_, named) = site.hypothetical(&[lonely], false, root, lonely);
            assert_eq!(
                named,
                BTreeSet::from([key_of(lonely)]),
                "改一只没有梁认领的主关节 {lonely}，只该点名它自己"
            );
        }
        None => println!("[PJOI] aps7166: 没有「自成单元而没有梁认领」的主关节，4b 没走"),
    }
}

/// 不成模型单元的关节（没有目录正体、只有负体）在缺省配置（C2 关）下的样子，aps7324 真库、假定窗。随整套跑；
/// 语料不在本机就跳过。
///
/// aps7324 的 MDS 支架里有一族这样的关节——`/MDS/FT54A~D` 那 16 根竖放角钢 `JOIE` 端的主关节 `/MDS_Joint-for-FT54`：
/// 目录件没有正体集、负体集里一把 NSCT。关节不画（不是模型单元），杆照样被它切（E3D 导出的杆逐根对过，e3d-model
/// `tests/primary_joint_frame_real.rs`）。e3d-model 的关节级联原先只从「要重建或已删除的单元」出发，这种关节进不了
/// 那个集合：只改它、只挪它的节点，被它切的型钢停在旧刀口。2026-11-12 起补了一条从候选出发的腿
/// （`increment::loose_joint_consumer_of`），下面几件事都不靠 C2：
///
/// 1. 只改这只关节 → 被它切的型钢被 `plan_update` 点名，型钢所在的根在 regen 里、子计划里有这根型钢；关节自己
///    不被点名（它没有东西可重建）；
/// 2. 只挪它的节点（主关节的 PNOD、截面关节的 SNOD）→ 型钢同样被点名；
/// 3. 只改型钢 → 不点名这只关节；
/// 4. 不相关的改动不拉人：一只没有几何源的关节（梁从它那里什么都取不到）动了，认领它的型钢不被点名。
///
/// 「给型钢出刀」的依据取自生成这一侧：型钢建模时扣过负体，而这只关节在型钢的账上没有留下「不出刀」的痕迹
/// （[`joint_cut_trace`] 的 `Applied`）。每一种都把一端当成一窗仅有的候选，喂给真的 `plan_update_with`
/// （[`Site::hypothetical`]）。
#[test]
fn live_aps7324_a_joint_without_a_body_and_the_section_it_cuts_are_planned_together() {
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let file = aps.join("aps7324_0001");
    if !file.is_file() {
        eprintln!("skip: 语料不在本机（{}）", file.display());
        return;
    }
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let sesno = chain_steps(&file, 1).expect("aps7324 的会话链")[0].1;
    let project = Project::open(&schema, &aps);
    let site = Site::open(&project, &file, sesno);
    let noun_of = |refno: RefNo| {
        site.set
            .element(refno)
            .element_type()
            .unwrap_or_else(|_| "?".into())
    };
    let owner_of = |refno: RefNo| -> Option<RefNo> {
        site.set
            .element(refno)
            .owner()
            .ok()
            .flatten()
            .map(|owner| owner.refno())
    };

    let sctn_hash = e3d_attlib::db1_hash("SCTN");
    let mut engine = ReadOnlyEngine::open_at(&file, sesno).expect("open the DESI file");
    let sections: Vec<RefNo> = engine
        .scan_elements(ScanTier::Header)
        .expect("scan element headers")
        .filter_map(|scanned| scanned.ok())
        .filter(|scanned| scanned.noun_hash == sctn_hash)
        .map(|scanned| scanned.refno)
        .collect();
    drop(engine);

    // 型钢两端不成单元、`CREF` 又回指这根型钢的 SJOI / PJOI：(关节, 型钢)。
    let mut loose: Vec<(RefNo, RefNo)> = Vec::new();
    for &beam in &sections {
        if !is_model_unit(&site.set, beam) {
            continue;
        }
        for joint in connection_partners(&site.set, beam, "SCTN") {
            if matches!(noun_of(joint).as_str(), "SJOI" | "PJOI")
                && !is_model_unit(&site.set, joint)
            {
                loose.push((joint, beam));
            }
        }
    }
    // 给型钢出刀的（关节带 `SPRE`，型钢的账上是「刀扣在梁上」）与不出刀的（没有 `SPRE`，账上是「没有几何源」；
    // 关节挂在这根型钢自己名下的不算——改它本来就上卷到这根型钢）。建模贵，各封顶。
    let has_spre = |joint: RefNo| matches!(site.set.element(joint).get_ref("SPRE"), Ok(Some(_)));
    let cap = env_usize("AIOS_144_PAIRS_PER_CLASS", 24);
    let mut cutting: Vec<(RefNo, RefNo)> = Vec::new();
    let mut sourceless: Vec<(RefNo, RefNo)> = Vec::new();
    let mut traces: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &(joint, beam) in &loose {
        if has_spre(joint) {
            if cutting.len() >= cap {
                continue;
            }
            let trace = joint_cut_trace(&site.set, beam, joint);
            *traces.entry(trace.label()).or_default() += 1;
            if trace == JointCutTrace::Applied {
                cutting.push((joint, beam));
            }
        } else if sourceless.len() < 8
            && owner_of(joint).and_then(owner_of) != Some(beam)
            && joint_cut_trace(&site.set, beam, joint) == JointCutTrace::Skipped
        {
            sourceless.push((joint, beam));
        }
    }
    println!(
        "[关节·不成单元] aps7324@{sesno}: SCTN {} 根；型钢两端不成单元、CREF 回指的关节 {} 只；带 SPRE 的在型钢账上：\
         {traces:?}；走假定窗的：给型钢出刀 {} 对，没有几何源 {} 对",
        sections.len(),
        loose.len(),
        cutting.len(),
        sourceless.len()
    );
    assert!(
        cutting.len() >= 16,
        "aps7324 里该有不成单元却给型钢出刀的关节（`/MDS/FT54A~D` 的端关节那一族，至少 16 对）：{traces:?}"
    );

    let (mut by_unit_when_c2_off, mut whole_root_when_c2_on) = (0usize, 0usize);
    let (mut same_root, mut with_neighbour) = (0usize, 0usize);
    let mut by_noun: BTreeMap<String, usize> = BTreeMap::new();
    for &(joint, beam) in &cutting {
        *by_noun.entry(noun_of(joint)).or_default() += 1;
        let beam_root = *site
            .root_chain(beam)
            .first()
            .unwrap_or_else(|| panic!("{beam}：型钢该在某个生成根下"));
        // 关节与被它切的型钢在不在同一个生成根里。在同一个根里、这一窗这根又只有这一处改动时，修之前靠
        // 「子计划为空 → 回退整根重算」歪打正着是对的；不在同一个根里、或同根另有改动，型钢就停在旧刀口。
        same_root += usize::from(site.root_chain(joint).first() == Some(&beam_root));
        // 1. 只改这只关节。
        let (facts, named) = site.hypothetical(&[joint], false, beam_root, beam);
        assert!(
            named.contains(&key_of(beam)),
            "只改不成单元的关节 {joint}：被它切的型钢 {beam} 该被点名：{named:?}"
        );
        // 1b. 同一窗里型钢的根另有一处改动（同根的另一根型钢）：子计划不空，修之前单元级发布只重建那一根、
        // 被切的型钢不在里面；现在两根都在。
        if let Some(&neighbour) = sections.iter().find(|&&other| {
            other != beam
                && is_model_unit(&site.set, other)
                && site.root_chain(other).first() == Some(&beam_root)
        }) {
            with_neighbour += 1;
            let (facts, named) = site.hypothetical(&[joint, neighbour], false, beam_root, beam);
            assert!(
                named.contains(&key_of(beam)) && named.contains(&key_of(neighbour)),
                "改不成单元的关节 {joint} 与同根的另一根型钢 {neighbour}：被切的型钢 {beam} 与那一根都该被点名：{named:?}"
            );
            assert!(
                !facts.degraded && facts.in_regen && facts.subplan_names_implicated,
                "同根另有改动时，型钢 {beam} 的根 {beam_root} 的子计划里也要有这根型钢：{facts:?}"
            );
        }
        assert!(
            !named.contains(&key_of(joint)),
            "不成单元的关节 {joint} 自己没有东西可重建，不该被点名：{named:?}"
        );
        assert!(
            !facts.degraded && facts.in_regen && facts.subplan_names_implicated,
            "只改不成单元的关节 {joint}：型钢 {beam} 的根 {beam_root} 在 regen 里、子计划里有这根型钢：{facts:?}"
        );
        assert!(!facts.cascaded, "C2 关着：这不是 gen-model 哪条级联的事");
        // 同一窗 C2 开着：C2 不问关节成不成单元，照样从它的 `CREF` 读到这条边，型钢的根成了级联目标、整根重算。
        // 数出来给 #34 的决策材料用。
        let (on, _) = site.hypothetical(&[joint], true, beam_root, beam);
        by_unit_when_c2_off += usize::from(facts.hinted);
        whole_root_when_c2_on += usize::from(on.cascaded && !on.hinted);
        assert!(
            !(on.cascaded && on.hinted),
            "C2 开着、型钢的根是级联目标时不许带单元级提示（GitHub #144）：{on:?}"
        );
        // 2. 只挪它的节点：主关节的站位读 PNOD 的 `NPOS`，截面关节的读 SNOD 的 `ZDIS` 与宿主的轴。
        let node = owner_of(joint).unwrap_or_else(|| panic!("{joint}：关节该有属主"));
        assert!(
            matches!(noun_of(node).as_str(), "PNOD" | "SNOD"),
            "{joint} 的属主 {node} 该是 PNOD / SNOD"
        );
        let (_, named) = site.hypothetical(&[node], false, beam_root, beam);
        assert!(
            named.contains(&key_of(beam)),
            "只挪节点 {node}：它名下不成单元的关节 {joint} 切的型钢 {beam} 该被点名：{named:?}"
        );
        // 3. 只改型钢：关节没有东西可重建。
        let (_, named) = site.hypothetical(&[beam], false, beam_root, beam);
        assert!(
            named.contains(&key_of(beam)) && !named.contains(&key_of(joint)),
            "只改型钢 {beam}：点名它自己，不点名不成单元的关节 {joint}：{named:?}"
        );
    }
    println!(
        "[关节·不成单元] aps7324: 给型钢出刀的 {} 对（{by_noun:?}；关节与型钢同根 {same_root} 对，同根另有型钢可一起改 \
         {with_neighbour} 对）假定窗都点名了型钢；只改关节的假定窗里，C2 关着时型钢的根走单元级发布 {by_unit_when_c2_off} 个；\
         C2 开着时型钢的根成了级联目标、整根重算 {whole_root_when_c2_on} 个",
        cutting.len()
    );

    // 4. 没有几何源的关节动了：认领它的型钢不该被点名。
    for &(joint, beam) in &sourceless {
        let beam_root = *site
            .root_chain(beam)
            .first()
            .unwrap_or_else(|| panic!("{beam}：型钢该在某个生成根下"));
        let (_, named) = site.hypothetical(&[joint], false, beam_root, beam);
        assert!(
            !named.contains(&key_of(beam)),
            "没有几何源的关节 {joint} 动了，不该带出认领它的型钢 {beam}：{named:?}"
        );
    }
    if sourceless.is_empty() {
        println!("[关节·不成单元] aps7324: 没有「没有几何源、被型钢认领」的关节，第 4 步没走");
    }
}

/// 量法（手动）：「不成单元的关节给梁出刀」这条腿接上之后，真实会话窗口上多点名了多少单元、规划慢了多少。
///
/// 走遍下面几本库的整条会话链（`AIOS_JOINT_LAST_STEPS=n` 只走每本库最后 n 步；候选数超过
/// `AIOS_JOINT_MAX_CANDIDATES`、缺省两万的导入级大窗跳过；`AIOS_JOINT_MEASURE_FILES`（`;` 分隔的文件路径）整体替换
/// 名单），每窗只做 `collect_window` + `plan_update_with`（生产同一个元数据门），从计划的级联记录里数：
/// - **关节带出的梁**：每条记录恰好是一根原本不在计划里、被这条腿新带进来的梁；每一根在窗口两端各建一次，数
///   「真的变了」的有几根（没变的是多算，不是错算）；
/// - 其中由子树遍历带出来的（挪了关节的节点 / 宿主 / 上层容器）另记一格；
/// - **关节失去目录正体**：窗口左端自成单元、右端不成单元的 SJOI / PJOI（清掉 `SPRE`、换成没有正体的目录件），
///   这条腿把它放进重建集，执行时擦掉旧几何；
/// - 指到别的库的（只在回执里点名）。
///
/// 每本库另记 `plan_update_with` 的合计耗时：同一批窗、同一条量法，拿接上之前的程序再跑一遍就是规划开销的对照。
/// 前五本是正式后端监听 / 回放套件用的库，中间是带目录正体主关节多的，最后四本是有这种关节的 MDS 支架库。只打印。
#[test]
#[ignore = "manual live measurement: walks whole DESI session chains of the AMS / APS sample projects; needs the E3D template directory"]
fn live_measure_what_the_bodiless_joint_cascade_adds_on_natural_windows() {
    #[derive(Default)]
    struct Tally {
        windows: usize,
        skipped: usize,
        hit_windows: usize,
        regenerate: usize,
        plan_millis: u128,
        /// 这条腿新带进来的梁，窗口两端各建一次：变了 / 没变 / 有一端建不出来。
        beams: usize,
        beams_changed: usize,
        beams_same: usize,
        beams_unknown: usize,
        /// 子树遍历（节点 / 宿主 / 上层容器挪了）登记的梁数——含本来就在计划里的。
        via_subtree: usize,
        lost_body: usize,
        foreign: usize,
    }
    impl Tally {
        fn add(&mut self, other: &Self) {
            self.windows += other.windows;
            self.skipped += other.skipped;
            self.hit_windows += other.hit_windows;
            self.regenerate += other.regenerate;
            self.plan_millis += other.plan_millis;
            self.beams += other.beams;
            self.beams_changed += other.beams_changed;
            self.beams_same += other.beams_same;
            self.beams_unknown += other.beams_unknown;
            self.via_subtree += other.via_subtree;
            self.lost_body += other.lost_body;
            self.foreign += other.foreign;
        }
        fn line(&self, cap: usize) -> String {
            format!(
                "{} 窗（另 {} 窗候选超 {cap} 跳过），计划重建合计 {} 件，plan_update 合计 {} ms；这条腿命中 {} 窗：\
                 关节带出梁 {} 根（两端各建一次：变了 {} / 没变 {} / 建不出 {}），子树遍历登记 {} 根，\
                 关节失去目录正体 {} 只，指到别的库 {} 条",
                self.windows,
                self.skipped,
                self.regenerate,
                self.plan_millis,
                self.hit_windows,
                self.beams,
                self.beams_changed,
                self.beams_same,
                self.beams_unknown,
                self.via_subtree,
                self.lost_body,
                self.foreign
            )
        }
    }
    let started = std::time::Instant::now();
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let last = env_usize("AIOS_JOINT_LAST_STEPS", 0);
    let cap = env_usize("AIOS_JOINT_MAX_CANDIDATES", 20_000);
    let files: Vec<PathBuf> = match std::env::var("AIOS_JOINT_MEASURE_FILES") {
        Ok(value) => value
            .split(';')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(_) => [
            "ams7997_0001",
            "ams7998_0001",
            "ams8000_0001",
            "ams251095_0001",
            "ams251137_0001",
            "ams7999_0001",
            "ams251183_0001",
        ]
        .iter()
        .map(|name| ams.join(name))
        .chain(
            [
                "aps7166_0001",
                "aps7200_0001",
                "aps7324_0001",
                "aps7350_0001",
                "aps250104_0001",
                "aps250102_0001",
                "aps7329_0001",
            ]
            .iter()
            .map(|name| aps.join(name)),
        )
        .chain(
            ["ams7329_0001", "ams7324_0001", "ams7350_0001"]
                .iter()
                .map(|name| ams.join(name)),
        )
        .collect(),
    };
    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    let mut total = Tally::default();
    for file in &files {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !file.is_file() {
            println!("[关节·不成单元] skip {name}: 文件不在");
            continue;
        }
        let steps = match chain_steps(file, last) {
            Ok(steps) => steps,
            Err(reason) => {
                println!("[关节·不成单元] skip {name}: {reason}");
                continue;
            }
        };
        let project_dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let project = projects
            .entry(project_dir.clone())
            .or_insert_with(|| Project::open(&schema, &project_dir));
        let mut tally = Tally::default();
        // 这本库整条链上节点（PNOD / SNOD）被改写的方式（账本的原话）→ 次数：`NPOS` / `ZDIS` 真动过几回。
        let mut node_changes: BTreeMap<String, usize> = BTreeMap::new();
        for (base_sesno, target_sesno) in steps {
            let window = match e3d_model::increment::collect_window(file, base_sesno, target_sesno)
            {
                Ok(window) => window,
                Err(error) => {
                    println!(
                        "[关节·不成单元]   {name} {base_sesno}→{target_sesno}: 收窗口失败：{error}"
                    );
                    continue;
                }
            };
            let candidates = window.diff.changes.len();
            if candidates > cap {
                tally.skipped += 1;
                continue;
            }
            tally.windows += 1;
            if candidates == 0 {
                continue;
            }
            let (Some(base), Some(target)) = (
                project.set_at(file, base_sesno),
                project.set_at(file, target_sesno),
            ) else {
                println!("[关节·不成单元]   {name} {base_sesno}→{target_sesno}: 两端开不了");
                continue;
            };
            let planning = std::time::Instant::now();
            let plan = plan_update_with(
                &base,
                &target,
                &window,
                PlanLimits::default(),
                change_metadata_tier,
            );
            tally.plan_millis += planning.elapsed().as_millis();
            tally.regenerate += plan.regenerate.len();
            for entry in plan.ledger.entries() {
                if matches!(entry.noun.as_str(), "PNOD" | "SNOD")
                    && entry.kind == e3d_model::ledger::ChangeKind::AttributesModified
                {
                    let detail: String = entry.detail.chars().take(80).collect();
                    *node_changes
                        .entry(format!("{}：{detail}", entry.noun))
                        .or_default() += 1;
                }
            }
            let (mut beams, mut changed, mut same, mut unknown) = (0usize, 0usize, 0usize, 0usize);
            let mut via_subtree = 0usize;
            for incident in &plan.report.cascades {
                if incident.detail.starts_with("这只关节不成模型单元") {
                    beams += 1;
                    let beam = incident
                        .detail
                        .split("指着它的梁 ")
                        .nth(1)
                        .and_then(|rest| rest.split(' ').next())
                        .and_then(crate::data_interface::cross_db_handoff::parse_refno);
                    match beam.and_then(|beam| unit_changed(&base, &target, beam)) {
                        Some(true) => changed += 1,
                        Some(false) => same += 1,
                        None => unknown += 1,
                    }
                } else if let Some(rest) =
                    incident.detail.split("子树里不成单元的关节另带出 ").nth(1)
                {
                    via_subtree += rest
                        .split(' ')
                        .next()
                        .and_then(|count| count.parse::<usize>().ok())
                        .unwrap_or(0);
                }
            }
            // 窗口左端自成单元、右端不成单元的 SJOI / PJOI：只有「关节失去目录正体」那一句会把它放进重建集。
            let lost_body = plan
                .regenerate
                .iter()
                .map(|&(word0, word1)| RefNo::new(word0, word1))
                .filter(|&refno| {
                    matches!(
                        target.element(refno).element_type().as_deref(),
                        Ok("SJOI" | "PJOI")
                    ) && !is_model_unit(&target, refno)
                        && is_model_unit(&base, refno)
                })
                .count();
            let foreign = plan
                .report
                .notes
                .iter()
                .filter(|note| note.detail.starts_with("不成单元的关节的 CREF 指到别的库"))
                .count();
            if beams + via_subtree + lost_body + foreign > 0 {
                tally.hit_windows += 1;
                println!(
                    "[关节·不成单元]   {name} {base_sesno}→{target_sesno}: 候选 {candidates}，计划重建 {} 件；关节带出梁 \
                     {beams} 根（两端各建一次：变了 {changed}、没变 {same}、建不出 {unknown}），子树遍历登记 \
                     {via_subtree} 根，关节失去目录正体 {lost_body} 只，指到别的库 {foreign} 条",
                    plan.regenerate.len()
                );
                // 这一窗里关节 / 节点到底改了什么（账本的原话，同样的合并计数）：看带出来的梁是不是白建。
                let mut causes: BTreeMap<String, usize> = BTreeMap::new();
                for entry in plan.ledger.entries() {
                    if matches!(entry.noun.as_str(), "PJOI" | "SJOI" | "PNOD" | "SNOD") {
                        let detail: String = entry.detail.chars().take(100).collect();
                        *causes
                            .entry(format!("{} {}：{detail}", entry.noun, entry.kind.label()))
                            .or_default() += 1;
                    }
                }
                let mut causes: Vec<(String, usize)> = causes.into_iter().collect();
                causes.sort_by(|left, right| right.1.cmp(&left.1));
                for (cause, count) in causes.into_iter().take(8) {
                    println!("[关节·不成单元]     账本：{count} × {cause}");
                }
            }
            tally.beams += beams;
            tally.beams_changed += changed;
            tally.beams_same += same;
            tally.beams_unknown += unknown;
            tally.via_subtree += via_subtree;
            tally.lost_body += lost_body;
            tally.foreign += foreign;
        }
        println!("[关节·不成单元] == {name}: {}", tally.line(cap));
        let mut node_changes: Vec<(String, usize)> = node_changes.into_iter().collect();
        node_changes.sort_by(|left, right| right.1.cmp(&left.1));
        for (change, count) in node_changes.into_iter().take(6) {
            println!("[关节·不成单元]    节点属性修改 {count} × {change}");
        }
        total.add(&tally);
    }
    println!(
        "[关节·不成单元] ==== 合计（{:.0} 秒）{}",
        started.elapsed().as_secs_f64(),
        total.line(cap)
    );
    assert!(total.windows > 0, "一窗都没量到——目录或模板目录不对");
}

/// 普查（手动）：自己成模型单元、刀却落在**另一件**单元上的那些件——改它，吃它刀的宿主会不会被 `plan_update` 点名。
///
/// 关节级联管的是「梁从 JOIS / JOIE 指的关节取负体」。建模里另有几处宿主向自己名下的件收刀，那些件自己都是模型
/// 单元（改它们上卷到它们自己为止）：
/// - `WALL` ← 名下带 `DESP` 的 `FIXING`（墙上的套管 / 开孔件，`elmodl::collect_host_fixings`）；
/// - `GWALL` ← 名下带 `DESP` 的 `SBFI`（墙上套管，`elmodl::collect_host_sleeves`）；
/// - `SCTN` ← 名下的 `FITT` / `CMFI`（成员链，`elmodl::collect_member_joint_negatives`）。
///
/// 每一类从几本库里取样（每类最多 `AIOS_144_PAIRS_PER_CLASS` 对，缺省 60），把那一件当成一窗仅有的候选喂给真的
/// `plan_update_with`，数：宿主被点名的、没被点名的（其中宿主的根在 regen 里、子计划不空、里面没有宿主——单元级
/// 发布只重建点名的那一件，宿主留在旧刀口上），以及宿主建模时确实扣过负体的。只打印、不下断言：这是给新票用的数，
/// 不是这张票修的东西。
#[test]
#[ignore = "manual live census: reads real DESI files of the AMS / APS sample projects; needs the E3D template directory"]
fn live_census_units_whose_cutters_land_on_another_unit() {
    fn descend(
        set: &Arc<DbSet>,
        at: RefNo,
        wanted: &str,
        out: &mut Vec<RefNo>,
        budget: &mut usize,
    ) {
        let Ok(members) = set.element(at).members() else {
            return;
        };
        for member in members {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let Ok(noun) = member.element_type() else {
                continue;
            };
            if noun == wanted {
                if matches!(member.get_double_array("DESP"), Ok(Some(desp)) if desp.len() >= 10) {
                    out.push(member.refno());
                }
            } else {
                descend(set, member.refno(), wanted, out, budget);
            }
        }
    }
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let cap = env_usize("AIOS_144_PAIRS_PER_CLASS", 60);
    // (工程目录, 库, 宿主 noun, 收刀的件 noun, 件是不是宿主的直属成员)
    let classes: [(&Path, &str, &str, &str, bool); 5] = [
        (ams.as_path(), "ams1112_0001", "WALL", "FIXING", false),
        (ams.as_path(), "ams1112_0001", "GWALL", "SBFI", false),
        (aps.as_path(), "aps250110_0001", "WALL", "FIXING", false),
        (ams.as_path(), "ams7997_0001", "SCTN", "FITT", true),
        (aps.as_path(), "aps250110_0001", "SCTN", "FITT", true),
    ];
    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    let mut sites: BTreeMap<PathBuf, Site> = BTreeMap::new();
    for (project_dir, name, host_noun, part_noun, direct) in classes {
        let file = project_dir.join(name);
        if !file.is_file() {
            println!("[宿主收刀] skip {name}: 文件不在");
            continue;
        }
        let Ok(steps) = chain_steps(&file, 1) else {
            println!("[宿主收刀] skip {name}: 会话链读不出");
            continue;
        };
        let sesno = steps[0].1;
        let project = projects
            .entry(project_dir.to_path_buf())
            .or_insert_with(|| Project::open(&schema, project_dir));
        let site = sites
            .entry(file.clone())
            .or_insert_with(|| Site::open(project, &file, sesno));
        let host_hash = e3d_attlib::db1_hash(host_noun);
        let mut engine = ReadOnlyEngine::open_at(&file, sesno).expect("open the DESI file");
        let hosts: Vec<RefNo> = engine
            .scan_elements(ScanTier::Header)
            .expect("scan element headers")
            .filter_map(|scanned| scanned.ok())
            .filter(|scanned| scanned.noun_hash == host_hash)
            .map(|scanned| scanned.refno)
            .collect();
        drop(engine);
        // (件, 宿主)
        let mut pairs: Vec<(RefNo, RefNo)> = Vec::new();
        let mut hosts_with_parts = 0usize;
        for &host in &hosts {
            if !is_model_unit(&site.set, host) {
                continue;
            }
            let mut parts = Vec::new();
            if direct {
                if let Ok(members) = site.set.element(host).members() {
                    parts.extend(
                        members
                            .filter(|member| {
                                matches!(member.element_type().as_deref(), Ok(noun) if noun == part_noun)
                            })
                            .map(|member| member.refno()),
                    );
                }
            } else {
                let mut budget = 20_000usize;
                descend(&site.set, host, part_noun, &mut parts, &mut budget);
            }
            parts.retain(|part| is_model_unit(&site.set, *part));
            hosts_with_parts += usize::from(!parts.is_empty());
            pairs.extend(parts.into_iter().map(|part| (part, host)));
        }
        let (mut named_host, mut not_named, mut left_out, mut host_cut) =
            (0usize, 0usize, 0usize, 0usize);
        let mut example: Option<String> = None;
        let step = (pairs.len() / cap.max(1)).max(1);
        let sampled: Vec<(RefNo, RefNo)> = pairs.iter().copied().step_by(step).take(cap).collect();
        for &(part, host) in &sampled {
            let Some(&host_root) = site.root_chain(host).first() else {
                continue;
            };
            let (facts, named) = site.hypothetical(&[part], false, host_root, host);
            if named.contains(&key_of(host)) {
                named_host += 1;
                continue;
            }
            not_named += 1;
            left_out += usize::from(facts.leaves_implicated_out());
            let cuts = negatives_applied_on(&site.set, host);
            host_cut += usize::from(cuts > 0);
            if example.is_none() {
                example = Some(format!(
                    "{part_noun} {part} → {host_noun} {host}（宿主的根 {host_root}，宿主建模时扣过 {cuts} 把负体）：\
                     plan_update 点名 {named:?}；{facts:?}"
                ));
            }
        }
        println!(
            "[宿主收刀] {name}@{sesno} {host_noun} ← {part_noun}: 宿主 {} 件，其中名下有这种件的 {hosts_with_parts} 件，\
             共 {} 对；取样 {} 对：改这一件时宿主被点名 {named_host}，没被点名 {not_named}（其中宿主的根在 regen 里、\
             子计划不空、里面没有宿主 {left_out}；宿主建模时确实扣过负体 {host_cut}）",
            hosts.len(),
            pairs.len(),
            sampled.len()
        );
        if let Some(example) = example {
            println!("[宿主收刀]   例：{example}");
        }
    }
}

/// 量法（手动）：真实会话窗口里，「收刀的件被点名、吃它刀的宿主墙没被点名」出现过几处，宿主的形状两端到底变没变。
///
/// 只看 [`live_census_units_whose_cutters_land_on_another_unit`] 里依据最硬的两类——`WALL` ← 带 `DESP` 的 `FIXING`、
/// `GWALL` ← 带 `DESP` 的 `SBFI`（墙上的套管 / 开孔件，墙的洞照着它的 `DESP` 开）。走整条会话链
/// （`AIOS_HOSTCUT_LAST_STEPS=n` 只走最后 n 步；候选数超过 `AIOS_HOSTCUT_MAX_CANDIDATES`、缺省两万的导入窗跳过；
/// `AIOS_HOSTCUT_MEASURE_FILES`（`;` 分隔的文件路径）整体替换名单，缺省是 ams1112），每窗只做 `collect_window` +
/// `plan_update_with`；计划里要重建或要删除的这种件，沿属主链找到宿主墙，宿主没被点名的记一处，宿主在窗口两端各建
/// 一次比形状（`AIOS_HOSTCUT_COMPARE` 封顶，缺省 300）。「形状变了」的每一处都是：单元级发布只重建了套管，墙上的洞
/// 停在旧尺寸 / 旧位置。只打印，不下断言。
#[test]
#[ignore = "manual live measurement: walks whole DESI session chains of the AMS sample project; needs the E3D template directory"]
fn live_measure_host_walls_left_on_old_holes_on_natural_windows() {
    #[derive(Default)]
    struct Tally {
        windows: usize,
        skipped: usize,
        hit_windows: usize,
        /// 件被点名（重建 / 删除）、宿主墙没被点名的处数，与其中宿主两端各建一次的结论。
        left: usize,
        changed: usize,
        same: usize,
        unknown: usize,
        not_compared: usize,
        /// 件被点名、宿主也被点名的处数（今天就对）。
        both_named: usize,
    }
    fn host_wall(set: &Arc<DbSet>, part: RefNo, part_noun: &str) -> Option<RefNo> {
        let wanted = match part_noun {
            "FIXING" => "WALL",
            "SBFI" => "GWALL",
            _ => return None,
        };
        let element = set.element(part);
        if !matches!(element.get_double_array("DESP"), Ok(Some(desp)) if desp.len() >= 10) {
            return None;
        }
        let mut current = element.owner().ok().flatten()?;
        for _ in 0..12 {
            let noun = current.element_type().ok()?;
            if noun == wanted {
                return Some(current.refno());
            }
            // 建模下钻时不进同类件（`collect_host_fixings` / `collect_host_sleeves`）：套在另一只里面的不算。
            if noun == part_noun {
                return None;
            }
            current = current.owner().ok().flatten()?;
        }
        None
    }
    let started = std::time::Instant::now();
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let last = env_usize("AIOS_HOSTCUT_LAST_STEPS", 0);
    let cap = env_usize("AIOS_HOSTCUT_MAX_CANDIDATES", 20_000);
    let mut compare_left = env_usize("AIOS_HOSTCUT_COMPARE", 300);
    let files: Vec<PathBuf> = match std::env::var("AIOS_HOSTCUT_MEASURE_FILES") {
        Ok(value) => value
            .split(';')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(_) => vec![ams.join("ams1112_0001")],
    };
    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    for file in &files {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !file.is_file() {
            println!("[宿主收刀·自然窗] skip {name}: 文件不在");
            continue;
        }
        let steps = match chain_steps(file, last) {
            Ok(steps) => steps,
            Err(reason) => {
                println!("[宿主收刀·自然窗] skip {name}: {reason}");
                continue;
            }
        };
        let project_dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let project = projects
            .entry(project_dir.clone())
            .or_insert_with(|| Project::open(&schema, &project_dir));
        let mut tally = Tally::default();
        for (base_sesno, target_sesno) in steps {
            let window = match e3d_model::increment::collect_window(file, base_sesno, target_sesno)
            {
                Ok(window) => window,
                Err(error) => {
                    println!(
                        "[宿主收刀·自然窗]   {name} {base_sesno}→{target_sesno}: 收窗口失败：{error}"
                    );
                    continue;
                }
            };
            let candidates = window.diff.changes.len();
            if candidates > cap {
                tally.skipped += 1;
                continue;
            }
            tally.windows += 1;
            if candidates == 0 {
                continue;
            }
            let (Some(base), Some(target)) = (
                project.set_at(file, base_sesno),
                project.set_at(file, target_sesno),
            ) else {
                println!("[宿主收刀·自然窗]   {name} {base_sesno}→{target_sesno}: 两端开不了");
                continue;
            };
            let plan = plan_update_with(
                &base,
                &target,
                &window,
                PlanLimits::default(),
                change_metadata_tier,
            );
            let named: BTreeSet<(u32, u32)> = plan
                .regenerate
                .iter()
                .chain(plan.remove.iter())
                .copied()
                .collect();
            // 宿主 → 这一窗点了名的件数；重建的件在 target 端找宿主，删掉的在 base 端找。
            let mut hosts: BTreeMap<(u32, u32), usize> = BTreeMap::new();
            for (units, side) in [(&plan.regenerate, &target), (&plan.remove, &base)] {
                for &(word0, word1) in units {
                    let part = RefNo::new(word0, word1);
                    let Ok(noun) = side.element(part).element_type() else {
                        continue;
                    };
                    if let Some(host) = host_wall(side, part, &noun) {
                        *hosts.entry(key_of(host)).or_default() += 1;
                    }
                }
            }
            let (mut left, mut changed, mut same, mut unknown, mut not_compared, mut both) =
                (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
            for (&host, &parts) in &hosts {
                if named.contains(&host) {
                    both += parts;
                    continue;
                }
                let host = RefNo::new(host.0, host.1);
                if !target.element(host).exists().unwrap_or(false) {
                    continue;
                }
                left += parts;
                if compare_left == 0 {
                    not_compared += parts;
                    continue;
                }
                compare_left -= 1;
                match unit_changed(&base, &target, host) {
                    Some(true) => changed += parts,
                    Some(false) => same += parts,
                    None => unknown += parts,
                }
            }
            if left > 0 {
                tally.hit_windows += 1;
                println!(
                    "[宿主收刀·自然窗]   {name} {base_sesno}→{target_sesno}: 候选 {candidates}，计划重建 {} 件；套管 / 开孔件\
                     被点名而宿主墙没被点名 {left} 处（宿主两端各建一次：形状变了 {changed}、没变 {same}、建不出 \
                     {unknown}、没比 {not_compared}）；宿主也被点名 {both} 处",
                    plan.regenerate.len()
                );
            }
            tally.left += left;
            tally.changed += changed;
            tally.same += same;
            tally.unknown += unknown;
            tally.not_compared += not_compared;
            tally.both_named += both;
        }
        println!(
            "[宿主收刀·自然窗] == {name}: {} 窗（另 {} 窗候选超 {cap} 跳过）；套管 / 开孔件被点名的：宿主墙也被点名 {} 处，\
             宿主墙没被点名 {} 处、分在 {} 窗（宿主两端各建一次：形状变了 {}、没变 {}、建不出 {}、没比 {}）；用时 {:.0} 秒",
            tally.windows,
            tally.skipped,
            tally.both_named,
            tally.left,
            tally.hit_windows,
            tally.changed,
            tally.same,
            tally.unknown,
            tally.not_compared,
            started.elapsed().as_secs_f64()
        );
    }
}

/// 量法（手动）：主关节接进 e3d-model 的关节级联之后，真实会话窗口上多点名了多少单元。
///
/// 走遍下面几本库的整条会话链（`AIOS_PJOI_LAST_STEPS=n` 只走每本库最后 n 步；候选数超过
/// `AIOS_PJOI_MAX_CANDIDATES`、缺省两万的导入级大窗跳过），每窗只做 `collect_window` + `plan_update_with`（生产同一个
/// 元数据门），从计划的级联记录里数主关节那三条腿各带出了什么：
/// - 正向：`PJOI` 要重建或已删除 → 认领它的 `SCTN`。每条记录恰好是一根**原本不在计划里**、被这条腿新带进来的梁；
///   对每一根再在窗口两端各建一次，数「真的变了」的有几根（没变的是多算，不是错算）。
/// - 反向：梁要重建或已删除 → 它认领的 `PJOI`，同样每条记录是一只新带进来的主关节。
/// - `PNOD`：现算标架宿主是 `PNOD` → 它名下的主关节。这一格是**命中数**，含本来就在计划里的——建库 / 导入的窗里
///   `PNOD` 与主关节一起新建，主关节自己就是候选。所以另数「窗口左端就已经在的主关节」（它们才是因为挪 `PNOD`
///   才被带出来的）以及其中两端建模结果真的变了的。
///
/// 前五本是正式后端监听 / 回放套件用的库，后面是带目录正体主关节多的。只打印，不下数值断言（数是语料的性质）。
#[test]
#[ignore = "manual live measurement: walks whole DESI session chains of the AMS / APS sample projects; needs the E3D template directory"]
fn live_measure_what_the_primary_joint_cascade_adds_on_natural_windows() {
    #[derive(Default)]
    struct Tally {
        windows: usize,
        skipped: usize,
        hit_windows: usize,
        forward: usize,
        reverse: usize,
        node_hosts: usize,
        node_units: usize,
        regenerate: usize,
        /// 正向带出来的梁里，窗口两端各建一次：变了 / 没变 / 有一端建不出来。
        forward_changed: usize,
        forward_same: usize,
        forward_unknown: usize,
        /// `PNOD` 宿主名下、窗口左端就已经在的主关节，以及其中两端建模结果变了的。
        node_existing: usize,
        node_existing_changed: usize,
    }
    let started = std::time::Instant::now();
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let last = env_usize("AIOS_PJOI_LAST_STEPS", 0);
    let cap = env_usize("AIOS_PJOI_MAX_CANDIDATES", 20_000);
    let libraries: [(&Path, &str); 13] = [
        (ams.as_path(), "ams7997_0001"),
        (ams.as_path(), "ams7998_0001"),
        (ams.as_path(), "ams8000_0001"),
        (ams.as_path(), "ams251095_0001"),
        (ams.as_path(), "ams251137_0001"),
        (ams.as_path(), "ams7999_0001"),
        (ams.as_path(), "ams251183_0001"),
        (aps.as_path(), "aps7166_0001"),
        (aps.as_path(), "aps7200_0001"),
        (aps.as_path(), "aps7324_0001"),
        (aps.as_path(), "aps7350_0001"),
        (aps.as_path(), "aps250104_0001"),
        (aps.as_path(), "aps250102_0001"),
    ];
    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    let mut total = Tally::default();
    for (project_dir, name) in libraries {
        let file = project_dir.join(name);
        if !file.is_file() {
            println!("[PJOI] skip {name}: 文件不在");
            continue;
        }
        let steps = match chain_steps(&file, last) {
            Ok(steps) => steps,
            Err(reason) => {
                println!("[PJOI] skip {name}: {reason}");
                continue;
            }
        };
        let project = projects
            .entry(project_dir.to_path_buf())
            .or_insert_with(|| Project::open(&schema, project_dir));
        let mut tally = Tally::default();
        for (base_sesno, target_sesno) in steps {
            let window = match e3d_model::increment::collect_window(&file, base_sesno, target_sesno)
            {
                Ok(window) => window,
                Err(error) => {
                    println!("[PJOI]   {name} {base_sesno}→{target_sesno}: 收窗口失败：{error}");
                    continue;
                }
            };
            let candidates = window.diff.changes.len();
            if candidates > cap {
                tally.skipped += 1;
                continue;
            }
            tally.windows += 1;
            if candidates == 0 {
                continue;
            }
            let (Some(base), Some(target)) = (
                project.set_at(&file, base_sesno),
                project.set_at(&file, target_sesno),
            ) else {
                println!("[PJOI]   {name} {base_sesno}→{target_sesno}: 两端开不了");
                continue;
            };
            let plan = plan_update_with(
                &base,
                &target,
                &window,
                PlanLimits::default(),
                change_metadata_tier,
            );
            tally.regenerate += plan.regenerate.len();
            let noun_of = |refno: RefNo| {
                target
                    .element(refno)
                    .element_type()
                    .or_else(|_| base.element(refno).element_type())
                    .unwrap_or_default()
            };
            let (mut forward, mut reverse, mut node_hosts, mut node_units) =
                (0usize, 0usize, 0usize, 0usize);
            let (mut forward_changed, mut forward_same, mut forward_unknown) =
                (0usize, 0usize, 0usize);
            let (mut node_existing, mut node_existing_changed) = (0usize, 0usize);
            for incident in &plan.report.cascades {
                if incident.noun == "PJOI" && incident.detail.starts_with("关节负体的来源") {
                    forward += 1;
                    let beam = incident
                        .detail
                        .split("指着它的梁 ")
                        .nth(1)
                        .and_then(|rest| rest.split(' ').next())
                        .and_then(crate::data_interface::cross_db_handoff::parse_refno);
                    match beam.and_then(|beam| unit_changed(&base, &target, beam)) {
                        Some(true) => forward_changed += 1,
                        Some(false) => forward_same += 1,
                        None => forward_unknown += 1,
                    }
                } else if incident.noun == "PNOD"
                    && incident.detail.starts_with("现算标架的输入变了")
                {
                    node_hosts += 1;
                    node_units += incident
                        .detail
                        .split("级联重建整棵宿主子树 ")
                        .nth(1)
                        .and_then(|rest| rest.split(' ').next())
                        .and_then(|count| count.parse::<usize>().ok())
                        .unwrap_or(0);
                    let node =
                        crate::data_interface::cross_db_handoff::parse_refno(&incident.refno);
                    let mut joints: Vec<RefNo> = Vec::new();
                    if let Some(node) = node {
                        let node = target.element(node);
                        if let Ok(members) = node.members() {
                            joints.extend(members.map(|member| member.refno()));
                        }
                    }
                    for joint in joints {
                        if noun_of(joint) != "PJOI"
                            || !is_model_unit(&target, joint)
                            || !base.element(joint).exists().unwrap_or(false)
                        {
                            continue;
                        }
                        node_existing += 1;
                        node_existing_changed +=
                            usize::from(unit_changed(&base, &target, joint) == Some(true));
                    }
                } else if let Some(rest) = incident.detail.strip_prefix("关节 ") {
                    let joint = rest
                        .split(' ')
                        .next()
                        .and_then(crate::data_interface::cross_db_handoff::parse_refno);
                    if joint.is_some_and(|joint| noun_of(joint) == "PJOI") {
                        reverse += 1;
                    }
                }
            }
            if forward + reverse + node_hosts > 0 {
                tally.hit_windows += 1;
                println!(
                    "[PJOI]   {name} {base_sesno}→{target_sesno}: 候选 {candidates}，计划重建 {} 件；主关节带出梁 \
                     {forward} 根（两端各建一次：变了 {forward_changed}、没变 {forward_same}、建不出 \
                     {forward_unknown}）、梁带出主关节 {reverse} 只、PNOD 宿主 {node_hosts} 个（命中 {node_units} \
                     件，其中左端就在的主关节 {node_existing} 只、两端建模结果变了 {node_existing_changed} 只）",
                    plan.regenerate.len()
                );
            }
            tally.forward += forward;
            tally.reverse += reverse;
            tally.node_hosts += node_hosts;
            tally.node_units += node_units;
            tally.forward_changed += forward_changed;
            tally.forward_same += forward_same;
            tally.forward_unknown += forward_unknown;
            tally.node_existing += node_existing;
            tally.node_existing_changed += node_existing_changed;
        }
        println!(
            "[PJOI] {name}: {} 窗（另 {} 窗候选超 {cap} 跳过），计划重建合计 {} 件；主关节那三条腿命中 {} 窗：\
             主关节带出梁 {} 根（变了 {} / 没变 {} / 建不出 {}）、梁带出主关节 {} 只、PNOD 宿主 {} 个（命中 {} 件，\
             左端就在的主关节 {} 只、其中变了 {} 只）",
            tally.windows,
            tally.skipped,
            tally.regenerate,
            tally.hit_windows,
            tally.forward,
            tally.forward_changed,
            tally.forward_same,
            tally.forward_unknown,
            tally.reverse,
            tally.node_hosts,
            tally.node_units,
            tally.node_existing,
            tally.node_existing_changed
        );
        total.windows += tally.windows;
        total.skipped += tally.skipped;
        total.hit_windows += tally.hit_windows;
        total.forward += tally.forward;
        total.reverse += tally.reverse;
        total.node_hosts += tally.node_hosts;
        total.node_units += tally.node_units;
        total.regenerate += tally.regenerate;
        total.forward_changed += tally.forward_changed;
        total.forward_same += tally.forward_same;
        total.forward_unknown += tally.forward_unknown;
        total.node_existing += tally.node_existing;
        total.node_existing_changed += tally.node_existing_changed;
    }
    println!(
        "[PJOI] 合计 {} 窗（另 {} 窗跳过），计划重建合计 {} 件；主关节那三条腿命中 {} 窗：主关节带出梁 {} 根\
         （变了 {} / 没变 {} / 建不出 {}）、梁带出主关节 {} 只、PNOD 宿主 {} 个（命中 {} 件，左端就在的主关节 {} 只、\
         其中变了 {} 只）；用时 {:.0} 秒",
        total.windows,
        total.skipped,
        total.regenerate,
        total.hit_windows,
        total.forward,
        total.forward_changed,
        total.forward_same,
        total.forward_unknown,
        total.reverse,
        total.node_hosts,
        total.node_units,
        total.node_existing,
        total.node_existing_changed,
        started.elapsed().as_secs_f64()
    );
}

// ───────────────────────────── 量法（手动） ─────────────────────────────

/// 上一轮量 C2 用的那 20 个库（`structural_connection::tests::live::live_measure_how_much_the_cascade_widens_regen_on_real_windows`
/// 的缺省名单）：正式后端监听的五个库、AMS 里结构连接边多的八个、APS 七个。
const MEASURE_AMS_FILES: [&str; 13] = [
    "ams7997_0001",
    "ams7998_0001",
    "ams8000_0001",
    "ams251095_0001",
    "ams251137_0001",
    "ams7999_0001",
    "ams7324_0001",
    "ams251183_0001",
    "ams251181_0001",
    "ams251180_0001",
    "ams251173_0001",
    "ams251172_0001",
    "ams251182_0001",
];
const MEASURE_APS_FILES: [&str; 7] = [
    "aps7200_0001",
    "aps7166_0001",
    "aps7324_0001",
    "aps250110_0001",
    "aps250108_0001",
    "aps250112_0001",
    "aps250209_0001",
];

/// 单元级发布对少于这么多条记录的根本来就回退整根（`UnitDeltaProducer::produce` 里的 `MIN_RECORDS_FOR_UNIT_PUBLISH`）。
const SMALL_ROOT_UNITS: usize = 10;

/// 一个「翻」的根（修之前带单元级提示且子计划不空，修之后整根重算）今天实际走哪条路。提示只是「可以走局部」，
/// 执行体在它后面还有两道门（`republish_roots_by_unit_delta_on` 的 `no cached receipt`、`produce` 的小根门）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlipKind {
    /// S 端没有这根（本窗新建）：没有旧回执，执行体本来就回退整根重算。
    NewRoot,
    /// S 端有，但几何记录不足 [`SMALL_ROOT_UNITS`] 条：小根门本来就让它回退整根重算。
    SmallRoot,
    /// S 端有且够大：今天真的走局部重建，修之后改成整根重算——修法的代价只有这一格。
    Effective,
}

impl FlipKind {
    fn label(self) -> &'static str {
        match self {
            Self::NewRoot => "本窗新建（没有旧回执，今天本来就整根重算）",
            Self::SmallRoot => "S 端有、不足 10 件（小根门今天本来就让它整根重算）",
            Self::Effective => "S 端有且不少于 10 件（今天局部重建，修之后改道）",
        }
    }
}

#[derive(Debug, Default, Clone)]
struct FlipTally {
    roots: usize,
    new_roots: usize,
    small_roots: usize,
    effective: usize,
    /// 名下元素数（T 端）：全部 / 只算真正改道的。
    elements: usize,
    effective_elements: usize,
    /// 子计划里的单元数（原本只重建这么多）：全部 / 只算真正改道的。
    units: usize,
    effective_units: usize,
}

impl FlipTally {
    fn record(&mut self, kind: FlipKind, elements: usize, units: usize) {
        self.roots += 1;
        self.elements += elements;
        self.units += units;
        match kind {
            FlipKind::NewRoot => self.new_roots += 1,
            FlipKind::SmallRoot => self.small_roots += 1,
            FlipKind::Effective => {
                self.effective += 1;
                self.effective_elements += elements;
                self.effective_units += units;
            }
        }
    }

    fn add(&mut self, other: &Self) {
        self.roots += other.roots;
        self.new_roots += other.new_roots;
        self.small_roots += other.small_roots;
        self.effective += other.effective;
        self.elements += other.elements;
        self.effective_elements += other.effective_elements;
        self.units += other.units;
        self.effective_units += other.effective_units;
    }

    fn line(&self) -> String {
        format!(
            "{} 根（本窗新建 {}，S 端不足 {SMALL_ROOT_UNITS} 件 {}，真正改道 {}——名下元素 {}、原本只重建 {} 个单元；\
             全部 {} 根名下元素 {}、子计划单元 {}）",
            self.roots,
            self.new_roots,
            self.small_roots,
            self.effective,
            self.effective_elements,
            self.effective_units,
            self.roots,
            self.elements,
            self.units
        )
    }
}

#[derive(Debug, Default, Clone)]
struct MeasureTally {
    windows: usize,
    /// 规划退化的窗：整窗不带单元级提示，每个持久根整根重算。
    degraded_windows: usize,
    regen: usize,
    /// 非退化窗的 regen 根：今天每一个都带单元级提示。
    hinted: usize,
    /// 其中子计划不空：今天走局部重建（没算执行体后面那两道门，见 [`FlipKind`]）。
    partial: usize,
    /// 其中子计划为空：今天回退整根重算（或 D10 只推回执）。
    empty_subplan: usize,
    /// 带提示的根里，是 C8 / P4（缺省开着）目标的。
    targets_default: usize,
    /// 带提示的根里，是 C2 目标的（C2 开着才算数）。
    targets_c2: usize,
    /// 缺省配置下翻的根（子计划不空 ∧ C8 / P4 目标）。
    flips_default: FlipTally,
    /// C2 也开着时**另外**多出来的：子计划不空 ∧ 只是 C2 的目标。
    flips_c2: FlipTally,
    /// 自然窗口：结构连接的一端被 `plan_update` 点名、同库对端是模型单元却没被点名。
    unnamed_partners: usize,
    /// 其中对端所在的根今天走局部重建、子计划里没有它。
    unnamed_left_out: usize,
    /// 其中对端所在的根根本不在 regen 里（C2 关着的跨根缺口，#34）。
    unnamed_root_not_in_regen: usize,
    /// 上面两类里两端各建一次、形状确实不同的。
    unnamed_changed: usize,
    /// 翻的根里两端整根各生成一次比过的 / 其中有「形状变了却不在子计划里」的单元的。
    compared_roots: usize,
    compared_roots_with_stale_units: usize,
    stale_units: usize,
}

impl MeasureTally {
    fn add(&mut self, other: &Self) {
        self.windows += other.windows;
        self.degraded_windows += other.degraded_windows;
        self.regen += other.regen;
        self.hinted += other.hinted;
        self.partial += other.partial;
        self.empty_subplan += other.empty_subplan;
        self.targets_default += other.targets_default;
        self.targets_c2 += other.targets_c2;
        self.flips_default.add(&other.flips_default);
        self.flips_c2.add(&other.flips_c2);
        self.unnamed_partners += other.unnamed_partners;
        self.unnamed_left_out += other.unnamed_left_out;
        self.unnamed_root_not_in_regen += other.unnamed_root_not_in_regen;
        self.unnamed_changed += other.unnamed_changed;
        self.compared_roots += other.compared_roots;
        self.compared_roots_with_stale_units += other.compared_roots_with_stale_units;
        self.stale_units += other.stale_units;
    }

    fn line(&self) -> String {
        format!(
            "{} 窗（退化 {} 窗）；regen 根 {}，其中带单元级提示 {}（子计划不空 {}，子计划空 = 回退整根 {}）；\
             带提示的根里 C8 / P4 目标 {}、C2 目标 {}。修之后不再带提示、整根重算的：缺省配置 {}；C2 也开着另加 {}。\
             自然窗口里结构连接一端被点名、对端单元没被点名 {} 处（对端的根局部重建漏掉它 {}，对端的根不在 regen {}；其中形状确实变了 {}）。\
             翻的根两端整根比过 {} 根，其中有漏件的 {} 根（{} 件）",
            self.windows,
            self.degraded_windows,
            self.regen,
            self.hinted,
            self.partial,
            self.empty_subplan,
            self.targets_default,
            self.targets_c2,
            self.flips_default.line(),
            self.flips_c2.line(),
            self.unnamed_partners,
            self.unnamed_left_out,
            self.unnamed_root_not_in_regen,
            self.unnamed_changed,
            self.compared_roots,
            self.compared_roots_with_stale_units,
            self.stale_units
        )
    }
}

/// 量法（手动）：「级联目标根不带单元级提示、整根重算」这条规则的代价，外加自然窗口里漏件的实证。
///
/// 逐库走会话链的最后 `AIOS_144_MEASURE_STEPS_PER_DB` 步（缺省 12，0 = 整条链），每一步跑一遍生产同款的
/// `WindowRootSources::collect` → `impact` → `plan_window_roots`（缺省配置：C2 关，C8 / P4 开），再按生产同一个
/// [`restrict_plan_to_root`] 给每个 regen 根切子计划：
///
/// - **代价**：非退化窗的 regen 根今天都带单元级提示；子计划不空的可以走局部重建。其中是级联目标的，修之后不带
///   提示、整根重算——缺省配置看 C8 ∪ P4 的目标集；C2 的目标集另跑一次 `StructuralConnectionCascade::collect`
///   单独记（C2 只往 `touched` 里加根，它加进来的根子计划必空，不影响这个数）。提示之后执行体还有两道门，所以
///   每个翻的根再分三格（[`FlipKind`]）：本窗新建（没有旧回执）、S 端不足 10 件（小根门）、真正改道——只有
///   最后一格是修法的代价。缺省配置下翻的、S 端有的根都两端整根各生成一次，拿 S 端的件数判小根门；只是 C2
///   目标的按 S 端名下模型单元数估（封顶之内的同样生成一次）。
/// - **漏件的实证**：`plan_update` 点名要重建的单元里，结构连接的同库对端（回指成立、自己是模型单元）没被点名的，
///   两端各建一次比形状；翻的根两端整根各生成一次，找「形状变了却不在子计划里」的单元。
///
/// 只读文件、不碰库。库的名单缺省是上一轮量 C2 的那 20 个，`AIOS_144_MEASURE_FILES`（`;` 分隔的文件路径）可整体
/// 替换。`AIOS_144_MEASURE_WINDOWS` 选窗：`steps`（缺省，相邻会话）、`span`（整条链并成一窗，[`span_window`]）、
/// `both`。`AIOS_144_MEASURE_BUDGET_SECS`（缺省 5400）到了就不再开新窗；`AIOS_144_MEASURE_COMPARE_ROOTS`
/// （缺省 12，名下元素超过 3000 的根不比）与 `AIOS_144_MEASURE_COMPARE_UNITS`（缺省 200）封顶两端生成的次数。
/// 结果打到 stdout（`--nocapture`）。
#[test]
#[ignore = "manual live measurement: walks real DESI session chains; needs the E3D sample projects and template directory"]
fn live_measure_what_regenerating_cascade_targets_whole_costs() {
    use std::time::Instant;

    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let files: Vec<PathBuf> = match std::env::var("AIOS_144_MEASURE_FILES") {
        Ok(value) => value
            .split(';')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(_) => MEASURE_AMS_FILES
            .iter()
            .map(|name| ams.join(name))
            .chain(MEASURE_APS_FILES.iter().map(|name| aps.join(name)))
            .collect(),
    };
    let per_db = env_usize("AIOS_144_MEASURE_STEPS_PER_DB", 12);
    let windows_kind =
        std::env::var("AIOS_144_MEASURE_WINDOWS").unwrap_or_else(|_| "steps".to_string());
    let budget_secs = env_usize("AIOS_144_MEASURE_BUDGET_SECS", 5400) as f64;
    let mut compare_roots_left = env_usize("AIOS_144_MEASURE_COMPARE_ROOTS", 12);
    let mut compare_units_left = env_usize("AIOS_144_MEASURE_COMPARE_UNITS", 200);
    let unit_types = resolve_delivery_unit_types(&[]);
    let started = Instant::now();
    println!(
        "[#144 量] 开关（配置）：unit_level_publish={} transform_cheap_path={} skip_metadata_only={}；\
         本量法把 C2 摁关、C8 / P4 摁开（缺省配置），选窗 {windows_kind}，每库最后 {per_db} 步",
        crate::options::model_unit_level_publish(),
        crate::options::model_transform_cheap_path(),
        crate::options::model_skip_metadata_only_changes()
    );

    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    let mut grand = MeasureTally::default();
    let mut budget_exhausted = false;
    for file in &files {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !file.is_file() {
            println!("[#144 量] skip {name}: 文件不在");
            continue;
        }
        let chain = match chain_steps(file, 0) {
            Ok(chain) => chain,
            Err(reason) => {
                println!("[#144 量] skip {name}: {reason}");
                continue;
            }
        };
        let mut steps: Vec<(u32, u32)> = if windows_kind == "span" {
            Vec::new()
        } else {
            let take = if per_db == 0 {
                chain.len()
            } else {
                per_db.min(chain.len())
            };
            chain[chain.len() - take..].to_vec()
        };
        if windows_kind != "steps"
            && let Some(span) = span_window(&chain)
            && !steps.contains(&span)
        {
            steps.push(span);
        }
        let project_dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let project = projects
            .entry(project_dir.clone())
            .or_insert_with(|| Project::open(&schema, &project_dir));
        let mut totals = MeasureTally::default();
        for (base_sesno, target_sesno) in steps {
            let label = format!("{name} {base_sesno}→{target_sesno}");
            if started.elapsed().as_secs_f64() > budget_secs {
                budget_exhausted = true;
                break;
            }
            let (Some(base), Some(target)) = (
                project.set_at(file, base_sesno),
                project.set_at(file, target_sesno),
            ) else {
                println!("[#144 量] skip {label}: 两端钉不住");
                continue;
            };
            let (base_index, target_index) = match (
                scan_index(file, Some(base_sesno)),
                scan_index(file, Some(target_sesno)),
            ) {
                (Ok(base_index), Ok(target_index)) => (base_index, target_index),
                (Err(error), _) | (_, Err(error)) => {
                    println!("[#144 量] skip {label}: 索引扫不出：{error:#}");
                    continue;
                }
            };
            let roots_s: Vec<PersistedRoot> =
                match enumerate_generation_roots(&base, &base_index.roots, &unit_types) {
                    Ok(roots) => roots.iter().map(PersistedRoot::from).collect(),
                    Err(error) => {
                        println!("[#144 量] skip {label}: S 端根枚举失败：{error:#}");
                        continue;
                    }
                };
            let collected = {
                let _c2 = crate::options::StructuralConnectionCascadeOverride::set(false);
                let _c8 = crate::options::CableEndpointCascadeOverride::set(true);
                let _p4 = crate::options::DesignReferenceCascadeOverride::set(true);
                WindowRootSources::collect(
                    file,
                    base_sesno,
                    target_sesno,
                    &base,
                    &target,
                    &unit_types,
                    &[],
                )
            };
            let sources = match collected {
                Ok(sources) => sources,
                Err(error) => {
                    println!("[#144 量] skip {label}: 规划失败：{error:#}");
                    continue;
                }
            };
            let impact = sources.impact(&roots_s);
            let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
            let c2 = StructuralConnectionCascade::collect(
                &sources.plan,
                &base,
                &target,
                &target_index.owners,
                &sources.roots_t,
            );
            let targets_default = sources.cascaded_roots();
            let sets = window_plan_sets(
                base_index.dbnum,
                file,
                base_sesno,
                target_sesno,
                &sources.plan,
                None,
            );
            let hinted_window = plan.degraded.is_none();
            totals.windows += 1;
            totals.regen += plan.regen.len();
            if !hinted_window {
                totals.degraded_windows += 1;
                continue;
            }

            // 每个 regen 根的子计划（生产同一个函数）。
            let mut subplans: BTreeMap<(u32, u32), RootSubplan> = BTreeMap::new();
            // (根, 缺省配置下就翻, 子计划里的单元数, 哪几条级联)
            let mut flipped: Vec<(RefNo, bool, usize, String)> = Vec::new();
            for root in &plan.regen {
                let refno = refno_to_e3d(root.root);
                let subplan = match restrict_plan_to_root(
                    &sets,
                    refno,
                    &base_index.owners,
                    &target_index.owners,
                ) {
                    Ok(subplan) => subplan,
                    Err(error) => {
                        println!("[#144 量] {label}: 根 {refno} 切不出子计划：{error:#}");
                        continue;
                    }
                };
                totals.hinted += 1;
                let by_c8 = sources.cable_endpoint.referrer_roots.contains(&root.root);
                let by_p4 = sources.design_reference.referrer_roots.contains(&root.root);
                let by_default = targets_default.contains(&root.root);
                let by_c2 = c2.far_end_roots.contains(&root.root);
                totals.targets_default += usize::from(by_default);
                totals.targets_c2 += usize::from(by_c2);
                if subplan.is_empty() {
                    totals.empty_subplan += 1;
                } else {
                    totals.partial += 1;
                    if by_default || by_c2 {
                        let kinds = [(by_c2, "C2"), (by_c8, "C8"), (by_p4, "P4")]
                            .into_iter()
                            .filter_map(|(hit, kind)| hit.then_some(kind))
                            .collect::<Vec<_>>()
                            .join("+");
                        let units = subplan.regenerate.len()
                            + subplan.remove.len()
                            + subplan.regenerate_derived.len();
                        flipped.push((refno, by_default, units, kinds));
                    }
                }
                subplans.insert(key_of(refno), subplan);
            }

            // 翻的根：名下元素数、今天实际走哪条路、两端整根各生成一次找漏件。
            if !flipped.is_empty() {
                let flipped_keys: BTreeSet<(u32, u32)> =
                    flipped.iter().map(|flip| key_of(flip.0)).collect();
                let mut members: BTreeMap<(u32, u32), Vec<RefNo>> = BTreeMap::new();
                for &(word0, word1) in target_index.owners.keys() {
                    let element = RefNo::new(word0, word1);
                    for root in roots_on_owner_chain(&target_index.owners, &flipped_keys, element) {
                        members.entry(key_of(root)).or_default().push(element);
                    }
                }
                for (root, by_default, units, kinds) in &flipped {
                    let under = members
                        .get(&key_of(*root))
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    let persisted_at_s = roots_s
                        .iter()
                        .any(|persisted| persisted.refno == refno_from_e3d(*root));
                    let mut units_at_s = 0usize;
                    for member in under {
                        if units_at_s >= SMALL_ROOT_UNITS {
                            break;
                        }
                        if is_model_unit(&base, *member) {
                            units_at_s += 1;
                        }
                    }
                    // 缺省配置下翻的根都比（修法真正改到的就是它们），只是 C2 目标的按封顶比。
                    let compare = persisted_at_s
                        && under.len() <= 3000
                        && (*by_default || compare_roots_left > 0);
                    let diff = if compare {
                        if !*by_default {
                            compare_roots_left -= 1;
                        }
                        Some(whole_root_diff(
                            &base,
                            &target,
                            *root,
                            &subplans[&key_of(*root)],
                        ))
                    } else {
                        None
                    };
                    let records_at_s = match &diff {
                        Some(Ok(diff)) => Some(diff.records_at_s),
                        _ => None,
                    };
                    let kind = if !persisted_at_s {
                        FlipKind::NewRoot
                    } else if records_at_s.unwrap_or(units_at_s) >= SMALL_ROOT_UNITS {
                        FlipKind::Effective
                    } else {
                        FlipKind::SmallRoot
                    };
                    if *by_default {
                        totals.flips_default.record(kind, under.len(), *units);
                    } else {
                        totals.flips_c2.record(kind, under.len(), *units);
                    }
                    let verdict = match &diff {
                        None if !persisted_at_s => "S 端没有这根，不用比".to_string(),
                        None => "没比（封顶或根太大）".to_string(),
                        Some(Ok(diff)) => {
                            totals.compared_roots += 1;
                            if !diff.stale.is_empty() {
                                totals.compared_roots_with_stale_units += 1;
                                totals.stale_units += diff.stale.len();
                            }
                            diff.line()
                        }
                        Some(Err(error)) => format!("整根生成失败：{error}"),
                    };
                    println!(
                        "[#144 量] {label}: 根 {root} 是 {kinds} 的目标、子计划不空（{units} 个单元）——修之后{}不带提示、整根重算；\
                         名下元素 {}，{}；{verdict}",
                        if *by_default {
                            "（缺省配置）"
                        } else {
                            "（C2 开着时）"
                        },
                        under.len(),
                        kind.label()
                    );
                }
            }

            // 自然窗口里的漏件：结构连接的一端被点名，同库对端是模型单元却没被点名。
            let named: BTreeSet<(u32, u32)> = sources
                .plan
                .regenerate
                .iter()
                .chain(sources.plan.remove.iter())
                .copied()
                .collect();
            let root_keys_t = root_keys_of(&sources.roots_t);
            let regen_keys: BTreeSet<(u32, u32)> = plan
                .regen
                .iter()
                .map(|root| key_of(refno_to_e3d(root.root)))
                .collect();
            for &unit in &sources.plan.regenerate {
                let refno = RefNo::new(unit.0, unit.1);
                let Ok(noun) = target.element(refno).element_type() else {
                    continue;
                };
                for partner in connection_partners(&target, refno, &noun) {
                    if named.contains(&key_of(partner)) || !is_model_unit(&target, partner) {
                        continue;
                    }
                    totals.unnamed_partners += 1;
                    let partner_noun = target
                        .element(partner)
                        .element_type()
                        .unwrap_or_else(|_| "?".into());
                    let nearest = roots_on_owner_chain(&target_index.owners, &root_keys_t, partner)
                        .into_iter()
                        .next();
                    let (stale, state) = match nearest {
                        None => (false, "对端不在任何生成根下".to_string()),
                        Some(root) if !regen_keys.contains(&key_of(root)) => {
                            totals.unnamed_root_not_in_regen += 1;
                            (
                                true,
                                format!("对端的根 {root} 不在 regen 里（C2 关着的跨根缺口）"),
                            )
                        }
                        Some(root) => match subplans.get(&key_of(root)) {
                            Some(subplan) if !subplan.is_empty() => {
                                totals.unnamed_left_out += 1;
                                let root_enum = refno_from_e3d(root);
                                (
                                    true,
                                    format!(
                                        "对端的根 {root} 今天走局部重建，子计划里没有它（这根{}）",
                                        if targets_default.contains(&root_enum) {
                                            "是 C8 / P4 的目标：修之后整根重算"
                                        } else if c2.far_end_roots.contains(&root_enum) {
                                            "只是 C2 的目标：C2 开着修法才罩得住"
                                        } else {
                                            "不是任何级联的目标"
                                        }
                                    ),
                                )
                            }
                            _ => (
                                false,
                                format!("对端的根 {root} 子计划为空，回退整根重算（今天就对）"),
                            ),
                        },
                    };
                    let changed = if stale && compare_units_left > 0 {
                        compare_units_left -= 1;
                        unit_changed(&base, &target, partner)
                    } else {
                        None
                    };
                    if changed == Some(true) {
                        totals.unnamed_changed += 1;
                    }
                    println!(
                        "[#144 量] {label}: {noun} {refno} 被点名，对端 {partner_noun} {partner} 没被点名——{state}；两端各建一次：{}",
                        match changed {
                            Some(true) => "形状变了（今天的增量结果与整根重算不同）",
                            Some(false) => "形状没变",
                            None if stale => "没比（封顶或建不出）",
                            None => "不用比",
                        }
                    );
                }
            }
        }
        println!("[#144 量] == {name}: {}", totals.line());
        grand.add(&totals);
        if budget_exhausted {
            println!("[#144 量] 时间预算 {budget_secs:.0} 秒用完，后面的库没走");
            break;
        }
    }
    println!(
        "[#144 量] ==== 合计（{:.0} 秒）{}",
        started.elapsed().as_secs_f64(),
        grand.line()
    );
    assert!(grand.windows > 0, "一窗都没量到——目录或模板目录不对");
}

// ───────────────────────────── 量法：D10 在自然窗口上（手动） ─────────────────────────────

/// D10 没点名的根卡在哪一条：`window_root_plan` 里 `classify_no_model_only_root` 的拒因（英文定句）按句首归格。
/// 措辞改了就落到最后一格，数不会丢。
fn receipt_only_rejection_class(reason: &str) -> &'static str {
    if reason.starts_with("root is not a settled") {
        "S 端没有这根"
    } else if reason.starts_with("root is not directly hit") {
        "不是闭包直接命中的"
    } else if reason.starts_with("root is a target of a cascade") {
        "是级联的目标"
    } else if reason.starts_with("touched, but no candidate") {
        "归不到候选"
    } else {
        "名下有要重建或只改站位的候选"
    }
}

#[derive(Debug, Default, Clone)]
struct ReceiptOnlyTally {
    windows: usize,
    /// 规划退化的窗：整窗不分桶，每个持久根整根重算。
    degraded_windows: usize,
    /// 非退化窗的 regen 根（三条级联都开着）。
    regen: usize,
    /// 其中子计划为空：没有 D10 的话执行体回退整根重算。
    empty_subplan: usize,
    /// D10 点名的根（只推回执）与它们名下的元素数（T 端）。
    receipt_only: usize,
    receipt_only_elements: usize,
    /// 点名的根里子计划不空的：执行体照子计划局部重建，不走只推回执那条路。
    receipt_only_with_subplan: usize,
    /// 点名的根里，结构连接级联关着（缺省配置）时也在 `regen` 里的——今天白算的整根重算。
    receipt_only_regen_today: usize,
    /// 两端整根各生成一次比过的根 / 其中有「形状变了却不在子计划里」的根 / 那样的件数 / 生成失败的根。
    compared: usize,
    compared_changed_roots: usize,
    changed_units: usize,
    compare_failed: usize,
    compare_secs: f64,
    /// 子计划为空却没被点名的根，按拒因分格。
    rejected_empty: BTreeMap<&'static str, usize>,
}

impl ReceiptOnlyTally {
    fn add(&mut self, other: &Self) {
        self.windows += other.windows;
        self.degraded_windows += other.degraded_windows;
        self.regen += other.regen;
        self.empty_subplan += other.empty_subplan;
        self.receipt_only += other.receipt_only;
        self.receipt_only_elements += other.receipt_only_elements;
        self.receipt_only_with_subplan += other.receipt_only_with_subplan;
        self.receipt_only_regen_today += other.receipt_only_regen_today;
        self.compared += other.compared;
        self.compared_changed_roots += other.compared_changed_roots;
        self.changed_units += other.changed_units;
        self.compare_failed += other.compare_failed;
        self.compare_secs += other.compare_secs;
        for (class, count) in &other.rejected_empty {
            *self.rejected_empty.entry(*class).or_default() += *count;
        }
    }

    fn line(&self) -> String {
        let rejected = if self.rejected_empty.is_empty() {
            "无".to_string()
        } else {
            self.rejected_empty
                .iter()
                .map(|(class, count)| format!("{class} {count}"))
                .collect::<Vec<_>>()
                .join("、")
        };
        format!(
            "{} 窗（退化 {} 窗）；非退化窗 regen 根 {}，其中子计划为空 {}；D10 点名只推回执 {} 根（名下元素 {}；\
             子计划不空 {}；结构连接级联关着时今天也在 regen 里 {}）；两端整根比过 {} 根（{:.0} 秒），\
             有「形状变了却不在子计划里」的 {} 根 {} 件，生成失败 {} 根；子计划为空却没点名的：{rejected}",
            self.windows,
            self.degraded_windows,
            self.regen,
            self.empty_subplan,
            self.receipt_only,
            self.receipt_only_elements,
            self.receipt_only_with_subplan,
            self.receipt_only_regen_today,
            self.compared,
            self.compare_secs,
            self.compared_changed_roots,
            self.changed_units,
            self.compare_failed
        )
    }
}

/// 量法（手动）：三条级联都开着时，单元级发布的 D10（只被 `no_model` 候选碰到的根只推回执）在自然窗口上点名
/// 多少根、点得对不对。把结构连接级联的缺省翻开（GitHub #34），连带后果就是 D10 第一次在缺省配置下生效
/// ——这条量法给的是翻之前要看的数。
///
/// 逐库走会话链的最后 `AIOS_D10_MEASURE_STEPS_PER_DB` 步（缺省 12，0 = 整条链），每一步把 C2 / C8 / P4 都摁开，跑
/// 生产同款的 `WindowRootSources::collect` → `impact` → `plan_window_roots` → `no_model_only`：
///
/// - **收益**：D10 点名的根数与名下元素数。这些根今天（C2 关，D10 不分桶）子计划为空、回退整根重算，算出来的
///   几何与 S 端逐字相同；有 D10 点名的窗再把 C2 摁关规划一遍，核对它们今天确实在 `regen` 里。
/// - **风险**：点名的根在窗口两端各整根生成一次（`AIOS_D10_MEASURE_COMPARE_ROOTS` 封顶，缺省 150；每窗最多
///   `AIOS_D10_MEASURE_COMPARE_PER_WINDOW`，缺省 8；名下元素超过 3000 的不比），「形状变了却不在子计划里」的件数
///   应当是 0——不是 0 就是 D10 把一根该重算的只推了回执。今天这种根靠 E3D 盖的戳做闭包种子整根重算收住；
///   D10 生效之后只有级联的目标集挡得住它。
/// - 顺带把「子计划为空却没被点名」的根按拒因分格，看白算的整根重算还剩多少、卡在哪一条。
///
/// 只读文件、不碰库、不下数值断言（点名多少是语料的性质）。库的名单与
/// [`live_measure_what_regenerating_cascade_targets_whole_costs`] 相同，`AIOS_D10_MEASURE_FILES`（`;` 分隔的文件
/// 路径）可整体替换；`AIOS_D10_MEASURE_BUDGET_SECS`（缺省 3600）到了就不再开新窗。结果打到 stdout（`--nocapture`）。
#[test]
#[ignore = "manual live measurement: walks real DESI session chains; needs the E3D sample projects and template directory"]
fn live_measure_what_the_receipt_only_bucket_does_with_all_three_cascades_on() {
    use std::time::Instant;

    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let files: Vec<PathBuf> = match std::env::var("AIOS_D10_MEASURE_FILES") {
        Ok(value) => value
            .split(';')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(_) => MEASURE_AMS_FILES
            .iter()
            .map(|name| ams.join(name))
            .chain(MEASURE_APS_FILES.iter().map(|name| aps.join(name)))
            .collect(),
    };
    let per_db = env_usize("AIOS_D10_MEASURE_STEPS_PER_DB", 12);
    let budget_secs = env_usize("AIOS_D10_MEASURE_BUDGET_SECS", 3600) as f64;
    let mut compare_left = env_usize("AIOS_D10_MEASURE_COMPARE_ROOTS", 150);
    let compare_per_window = env_usize("AIOS_D10_MEASURE_COMPARE_PER_WINDOW", 8);
    let unit_types = resolve_delivery_unit_types(&[]);
    let started = Instant::now();
    println!(
        "[D10 量] 开关（配置）：unit_level_publish={} skip_metadata_only={}；本量法把 C2 / C8 / P4 都摁开，\
         每库最后 {per_db} 步",
        crate::options::model_unit_level_publish(),
        crate::options::model_skip_metadata_only_changes()
    );
    assert!(
        crate::options::model_unit_level_publish(),
        "单元级发布关着：按根归因的候选表不收，D10 一根都点不了名，这条量法量不出东西"
    );

    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    let mut grand = ReceiptOnlyTally::default();
    let mut budget_exhausted = false;
    for file in &files {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !file.is_file() {
            println!("[D10 量] skip {name}: 文件不在");
            continue;
        }
        let steps = match chain_steps(file, per_db) {
            Ok(steps) => steps,
            Err(reason) => {
                println!("[D10 量] skip {name}: {reason}");
                continue;
            }
        };
        let project_dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let project = projects
            .entry(project_dir.clone())
            .or_insert_with(|| Project::open(&schema, &project_dir));
        let mut totals = ReceiptOnlyTally::default();
        for (base_sesno, target_sesno) in steps {
            let label = format!("{name} {base_sesno}→{target_sesno}");
            if started.elapsed().as_secs_f64() > budget_secs {
                budget_exhausted = true;
                break;
            }
            let (Some(base), Some(target)) = (
                project.set_at(file, base_sesno),
                project.set_at(file, target_sesno),
            ) else {
                println!("[D10 量] skip {label}: 两端钉不住");
                continue;
            };
            let (base_index, target_index) = match (
                scan_index(file, Some(base_sesno)),
                scan_index(file, Some(target_sesno)),
            ) {
                (Ok(base_index), Ok(target_index)) => (base_index, target_index),
                (Err(error), _) | (_, Err(error)) => {
                    println!("[D10 量] skip {label}: 索引扫不出：{error:#}");
                    continue;
                }
            };
            let roots_s: Vec<PersistedRoot> =
                match enumerate_generation_roots(&base, &base_index.roots, &unit_types) {
                    Ok(roots) => roots.iter().map(PersistedRoot::from).collect(),
                    Err(error) => {
                        println!("[D10 量] skip {label}: S 端根枚举失败：{error:#}");
                        continue;
                    }
                };
            // 生产同款的规划；结构连接级联按给的摁，C8 / P4 摁开。
            let collect_with = |structural_connection: bool| {
                let _c2 =
                    crate::options::StructuralConnectionCascadeOverride::set(structural_connection);
                let _c8 = crate::options::CableEndpointCascadeOverride::set(true);
                let _p4 = crate::options::DesignReferenceCascadeOverride::set(true);
                WindowRootSources::collect(
                    file,
                    base_sesno,
                    target_sesno,
                    &base,
                    &target,
                    &unit_types,
                    &[],
                )
            };
            let sources = match collect_with(true) {
                Ok(sources) => sources,
                Err(error) => {
                    println!("[D10 量] skip {label}: 规划失败：{error:#}");
                    continue;
                }
            };
            let impact = sources.impact(&roots_s);
            let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
            totals.windows += 1;
            if plan.degraded.is_some() {
                totals.degraded_windows += 1;
                continue;
            }
            totals.regen += plan.regen.len();
            let settled: BTreeSet<RefnoEnum> = roots_s.iter().map(|root| root.refno).collect();
            let classification = sources.no_model_only(&roots_s, &settled, &impact, &plan);

            // 每个 regen 根的子计划（生产同一个函数）。
            let sets = window_plan_sets(
                base_index.dbnum,
                file,
                base_sesno,
                target_sesno,
                &sources.plan,
                None,
            );
            let mut subplans: BTreeMap<(u32, u32), RootSubplan> = BTreeMap::new();
            for root in &plan.regen {
                let refno = refno_to_e3d(root.root);
                match restrict_plan_to_root(&sets, refno, &base_index.owners, &target_index.owners)
                {
                    Ok(subplan) => {
                        if subplan.is_empty() {
                            totals.empty_subplan += 1;
                        }
                        subplans.insert(key_of(refno), subplan);
                    }
                    Err(error) => {
                        println!("[D10 量] {label}: 根 {refno} 切不出子计划：{error:#}");
                    }
                }
            }
            for (root, reason) in &classification.rejected {
                let empty = subplans
                    .get(&key_of(refno_to_e3d(*root)))
                    .is_some_and(RootSubplan::is_empty);
                if empty {
                    *totals
                        .rejected_empty
                        .entry(receipt_only_rejection_class(reason))
                        .or_default() += 1;
                }
            }
            if classification.roots.is_empty() {
                continue;
            }

            // 点名的根名下的元素数（T 端）。
            let named_keys: BTreeSet<(u32, u32)> = classification
                .roots
                .iter()
                .map(|root| key_of(refno_to_e3d(*root)))
                .collect();
            let mut members: BTreeMap<(u32, u32), usize> = BTreeMap::new();
            for &(word0, word1) in target_index.owners.keys() {
                let element = RefNo::new(word0, word1);
                for root in roots_on_owner_chain(&target_index.owners, &named_keys, element) {
                    *members.entry(key_of(root)).or_default() += 1;
                }
            }
            // 今天（结构连接级联关着）这些根在不在 regen 里。
            let regen_today: Option<BTreeSet<RefnoEnum>> = match collect_with(false) {
                Ok(today) => {
                    let impact_today = today.impact(&roots_s);
                    let plan_today = plan_window_roots(&today.roots_t, &roots_s, &impact_today);
                    Some(plan_today.regen.iter().map(|root| root.root).collect())
                }
                Err(error) => {
                    println!("[D10 量] {label}: 结构连接级联关着那一态规划失败：{error:#}");
                    None
                }
            };

            let mut elements_here = 0usize;
            let mut compared_here = 0usize;
            let mut changed_here = 0usize;
            for root in &classification.roots {
                let refno = refno_to_e3d(*root);
                let key = key_of(refno);
                let under = members.get(&key).copied().unwrap_or(0);
                elements_here += under;
                totals.receipt_only += 1;
                totals.receipt_only_elements += under;
                if regen_today
                    .as_ref()
                    .is_some_and(|regen| regen.contains(root))
                {
                    totals.receipt_only_regen_today += 1;
                }
                let Some(subplan) = subplans.get(&key) else {
                    continue;
                };
                if !subplan.is_empty() {
                    totals.receipt_only_with_subplan += 1;
                }
                if compare_left == 0 || compared_here >= compare_per_window || under > 3000 {
                    continue;
                }
                compare_left -= 1;
                compared_here += 1;
                let tick = Instant::now();
                let diff = whole_root_diff(&base, &target, refno, subplan);
                totals.compare_secs += tick.elapsed().as_secs_f64();
                match diff {
                    Ok(diff) => {
                        totals.compared += 1;
                        if !diff.stale.is_empty() {
                            totals.compared_changed_roots += 1;
                            totals.changed_units += diff.stale.len();
                            changed_here += diff.stale.len();
                            println!(
                                "[D10 量] {label}: 根 {refno} 被点名只推回执（名下元素 {under}），可是{}",
                                diff.line()
                            );
                        }
                    }
                    Err(error) => {
                        totals.compare_failed += 1;
                        println!("[D10 量] {label}: 根 {refno} 整根生成失败：{error}");
                    }
                }
            }
            println!(
                "[D10 量] {label}: regen {} 根，D10 点名只推回执 {} 根（名下元素 {elements_here}），\
                 两端整根比过 {compared_here} 根、形状变了却不在子计划里 {changed_here} 件",
                plan.regen.len(),
                classification.roots.len()
            );
        }
        println!("[D10 量] == {name}: {}", totals.line());
        grand.add(&totals);
        if budget_exhausted {
            println!("[D10 量] 时间预算 {budget_secs:.0} 秒用完，后面的库没走");
            break;
        }
    }
    println!(
        "[D10 量] ==== 合计（{:.0} 秒）{}",
        started.elapsed().as_secs_f64(),
        grand.line()
    );
    assert!(grand.windows > 0, "一窗都没量到——目录或模板目录不对");
}

// ───────────────────────────── 真库，真实窗口（手动） ─────────────────────────────

/// 自然窗口的缺省名单：电缆 / 桥架内容多、变更记录里记过电缆端点（C8）或设计引用（P4）自然命中的库。
const NATURAL_APS_FILES: [&str; 4] = [
    "aps7200_0001",
    "aps250176_0001",
    "aps250179_0001",
    "aps250514_0001",
];
const NATURAL_AMS_FILES: [&str; 1] = ["ams251155_0001"];

/// GitHub #144，真库上的真实窗口：缺省配置（C2 关，C8 / P4 开）下，自然窗口里那些「是电缆端点（C8）或设计
/// 引用（P4）级联的目标、自己这一窗又有改动」的根。
///
/// 走生产同一段：`WindowRootSources::collect` → `impact` → `plan_window_roots` → 规划器的折算 →
/// [`collect_unit_tasks`]，子计划用生产同一个 [`restrict_plan_to_root`]。每库走相邻会话的每一步，外加整条链并成
/// 的一窗（[`span_window`]——变更记录里记的 C8 命中就是这种窗）。「修之前」那一态用不给级联目标名单的同一段
/// 折算重放（[`window_unit_tasks_as`]）。
///
/// 断言（每一窗、每个 regen 根）：
/// 1. 级联目标根：修之前带单元级提示，修之后不带；
/// 2. 其余的根：两态的提示逐字相同——「级联目标根之外的根行为不变」在真实窗口上的那一半；
/// 3. 名单上至少有一个「级联目标 ∧ 子计划不空」的根。一个都没有说明名单或目录不对，这条测试在空转。
///
/// 给人看、不断言的：那些根今天实际走哪条路（本窗新建 / S 端已有），S 端已有的两端整根各生成一次，有没有
/// 「形状变了却不在子计划里」的件——有就是局部重建在自然数据上漏件的实证。
///
/// 只读文件。`AIOS_144_NATURAL_FILES`（`;` 分隔的文件路径）整体替换名单；`AIOS_144_NATURAL_COMPARE_ROOTS`
/// （缺省 24，名下元素超过 3000 的根不比）封顶整根生成的次数。
#[test]
#[ignore = "manual live: plans real DESI session windows of the APS / AMS sample projects; needs the E3D template directory"]
fn live_natural_cable_and_design_reference_targets_lose_their_unit_publish_hint() {
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    let ams = env_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
    let aps = env_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
    let files: Vec<PathBuf> = match std::env::var("AIOS_144_NATURAL_FILES") {
        Ok(value) => value
            .split(';')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(_) => NATURAL_APS_FILES
            .iter()
            .map(|name| aps.join(name))
            .chain(NATURAL_AMS_FILES.iter().map(|name| ams.join(name)))
            .collect(),
    };
    let mut compare_roots_left = env_usize("AIOS_144_NATURAL_COMPARE_ROOTS", 24);
    let unit_types = resolve_delivery_unit_types(&[]);

    let mut projects: BTreeMap<PathBuf, Project> = BTreeMap::new();
    let (mut windows, mut degraded_windows, mut regen_roots) = (0usize, 0usize, 0usize);
    let (mut targets, mut others) = (0usize, 0usize);
    let (mut samples, mut samples_new, mut samples_persisted) = (0usize, 0usize, 0usize);
    let (mut compared, mut compared_with_stale, mut stale_units) = (0usize, 0usize, 0usize);
    for file in &files {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !file.is_file() {
            println!("[#144 自然窗] skip {name}: 文件不在");
            continue;
        }
        let chain = match chain_steps(file, 0) {
            Ok(chain) => chain,
            Err(reason) => {
                println!("[#144 自然窗] skip {name}: {reason}");
                continue;
            }
        };
        let mut steps = chain.clone();
        if let Some(span) = span_window(&chain)
            && !steps.contains(&span)
        {
            steps.push(span);
        }
        let project_dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let project = projects
            .entry(project_dir.clone())
            .or_insert_with(|| Project::open(&schema, &project_dir));
        for (base_sesno, target_sesno) in steps {
            let label = format!("{name} {base_sesno}→{target_sesno}");
            let (Some(base), Some(target)) = (
                project.set_at(file, base_sesno),
                project.set_at(file, target_sesno),
            ) else {
                println!("[#144 自然窗] skip {label}: 两端钉不住");
                continue;
            };
            let (base_index, target_index) = match (
                scan_index(file, Some(base_sesno)),
                scan_index(file, Some(target_sesno)),
            ) {
                (Ok(base_index), Ok(target_index)) => (base_index, target_index),
                (Err(error), _) | (_, Err(error)) => {
                    println!("[#144 自然窗] skip {label}: 索引扫不出：{error:#}");
                    continue;
                }
            };
            let roots_s: Vec<PersistedRoot> =
                match enumerate_generation_roots(&base, &base_index.roots, &unit_types) {
                    Ok(roots) => roots.iter().map(PersistedRoot::from).collect(),
                    Err(error) => {
                        println!("[#144 自然窗] skip {label}: S 端根枚举失败：{error:#}");
                        continue;
                    }
                };
            let collected = {
                let _c2 = crate::options::StructuralConnectionCascadeOverride::set(false);
                let _c8 = crate::options::CableEndpointCascadeOverride::set(true);
                let _p4 = crate::options::DesignReferenceCascadeOverride::set(true);
                WindowRootSources::collect(
                    file,
                    base_sesno,
                    target_sesno,
                    &base,
                    &target,
                    &unit_types,
                    &[],
                )
            };
            let sources = match collected {
                Ok(sources) => sources,
                Err(error) => {
                    println!("[#144 自然窗] skip {label}: 规划失败：{error:#}");
                    continue;
                }
            };
            let impact = sources.impact(&roots_s);
            let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
            windows += 1;
            regen_roots += plan.regen.len();
            if plan.degraded.is_some() {
                degraded_windows += 1;
                continue;
            }
            let cascade_targets = sources.cascaded_roots();
            let sets = window_plan_sets(
                base_index.dbnum,
                file,
                base_sesno,
                target_sesno,
                &sources.plan,
                None,
            );
            let before =
                window_unit_tasks_as(base_index.dbnum, &sources, &roots_s, &impact, &plan, false);
            let after = window_unit_tasks(base_index.dbnum, &sources, &roots_s, &impact, &plan);

            // (根, 哪几条级联, 子计划)
            let mut window_samples: Vec<(RefNo, String, RootSubplan)> = Vec::new();
            for root in &plan.regen {
                let refno = refno_to_e3d(root.root);
                let was = &task_of_root(&before, root.root)
                    .unwrap_or_else(|| panic!("{label}: 根 {refno} 修之前的折算里没有任务"))
                    .unit_publish;
                let now = &task_of_root(&after, root.root)
                    .unwrap_or_else(|| panic!("{label}: 根 {refno} 修之后的折算里没有任务"))
                    .unit_publish;
                if !cascade_targets.contains(&root.root) {
                    others += 1;
                    assert_eq!(
                        now, was,
                        "{label}: 根 {refno} 不是任何级联的目标，单元级提示不该变"
                    );
                    continue;
                }
                targets += 1;
                assert!(
                    was.is_some(),
                    "{label}: 根 {refno} 是级联目标；修之前，非退化窗的每个 regen 根都带单元级提示"
                );
                assert!(
                    now.is_none(),
                    "{label}: 根 {refno} 是级联目标，修之后不该带单元级提示（{now:?}）"
                );
                let subplan =
                    restrict_plan_to_root(&sets, refno, &base_index.owners, &target_index.owners)
                        .unwrap_or_else(|error| {
                            panic!("{label}: 根 {refno} 切不出子计划：{error:#}")
                        });
                if subplan.is_empty() {
                    continue;
                }
                let kinds = [
                    (
                        sources.cable_endpoint.referrer_roots.contains(&root.root),
                        "C8",
                    ),
                    (
                        sources.design_reference.referrer_roots.contains(&root.root),
                        "P4",
                    ),
                ]
                .into_iter()
                .filter_map(|(hit, kind)| hit.then_some(kind))
                .collect::<Vec<_>>()
                .join("+");
                window_samples.push((refno, kinds, subplan));
            }
            if window_samples.is_empty() {
                continue;
            }

            // 名下元素数（T 端），一遍属主表数完这一窗的样本根。
            let sample_keys: BTreeSet<(u32, u32)> = window_samples
                .iter()
                .map(|sample| key_of(sample.0))
                .collect();
            let mut elements_under: BTreeMap<(u32, u32), usize> = BTreeMap::new();
            for &(word0, word1) in target_index.owners.keys() {
                let element = RefNo::new(word0, word1);
                for root in roots_on_owner_chain(&target_index.owners, &sample_keys, element) {
                    *elements_under.entry(key_of(root)).or_default() += 1;
                }
            }
            for (refno, kinds, subplan) in &window_samples {
                samples += 1;
                let persisted_at_s = roots_s
                    .iter()
                    .any(|persisted| persisted.refno == refno_from_e3d(*refno));
                let under = elements_under.get(&key_of(*refno)).copied().unwrap_or(0);
                let units = subplan.regenerate.len()
                    + subplan.remove.len()
                    + subplan.regenerate_derived.len();
                let verdict = if !persisted_at_s {
                    samples_new += 1;
                    "S 端没有这根（本窗新建）：没有旧回执，今天本来就整根重算".to_string()
                } else {
                    samples_persisted += 1;
                    if compare_roots_left > 0 && under <= 3000 {
                        compare_roots_left -= 1;
                        match whole_root_diff(&base, &target, *refno, subplan) {
                            Ok(diff) => {
                                compared += 1;
                                if !diff.stale.is_empty() {
                                    compared_with_stale += 1;
                                    stale_units += diff.stale.len();
                                }
                                format!("S 端有这根；{}", diff.line())
                            }
                            Err(error) => format!("S 端有这根；整根生成失败：{error}"),
                        }
                    } else {
                        "S 端有这根；没比（封顶或根太大）".to_string()
                    }
                };
                println!(
                    "[#144 自然窗] {label}: 根 {refno} 是 {kinds} 的目标、子计划不空（{units} 个单元，名下元素 {under}）——\
                     修之前带单元级提示，修之后不带、整根重算；{verdict}"
                );
            }
        }
    }
    println!(
        "[#144 自然窗] 合计 {windows} 窗（退化 {degraded_windows} 窗），regen 根 {regen_roots}；非退化窗里级联（C8 / P4）目标根 {targets} 个\
         （修之前都带单元级提示，修之后都不带），其余 {others} 个根两态提示逐字相同。目标根里子计划不空的 {samples} 个\
         （本窗新建 {samples_new}，S 端已有 {samples_persisted}）；S 端已有的两端整根比过 {compared} 根，其中有\
         「形状变了却不在子计划里」的 {compared_with_stale} 根（{stale_units} 件）"
    );
    assert!(windows > 0, "一窗都没走到——目录或模板目录不对");
    assert!(
        samples > 0,
        "名单上没有一个「级联目标 ∧ 子计划不空」的根——这条测试在空转，名单或目录不对"
    );
}
