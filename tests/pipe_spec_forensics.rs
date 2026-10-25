//! 规格表落库结构取证（一次性探针测试，fixtures 缺席自动跳过）。
//!
//! 目标：为 `pipe-model` 的 `SpecCatalog` 生产实现拿到 SPEC/SELE/SPCO 树的真结构——
//! 每层的 noun、QUES/QUAL 题键、ANSW/TANS/MAXA/DEFA/TDEF 答案面，验证 §18.6/§18.8
//! 的 PML 语义到落库属性的映射。
//!
//! 跑法：`cargo test --test pipe_spec_forensics -- --nocapture`（需本机 ams 样例库 + 模板目录）。

use aios_database::data_interface::cata_closure::CataDbLocator;
use aios_database::data_interface::direct_index::INDEX_DIR_ENV;
use aios_database::data_interface::direct_store::{DbPin, DirectSchema, DirectStore};
use aios_core::RefU64;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

const AMS0001: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams0001_0001";
const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

struct NoLocator;
impl CataDbLocator for NoLocator {
    fn dbnum_of_ref0(&self, _ref0: u32) -> Option<u32> {
        None
    }
    fn db_type_of(&self, _dbnum: u32) -> Option<String> {
        None
    }
    fn file_of(&self, _dbnum: u32) -> Option<(String, PathBuf)> {
        None
    }
}

fn fixtures() -> Option<(PathBuf, PathBuf)> {
    let db = std::env::var_os("AMS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(AMS0001));
    let templates = std::env::var_os("AIOS_E3D_TEMPLATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT));
    if !db.is_file() || !templates.join("attlib.dat").is_file() {
        eprintln!("SKIP: fixtures not present ({db:?}, {templates:?})");
        return None;
    }
    Some((db, templates))
}

fn env_gate() -> std::sync::MutexGuard<'static, ()> {
    static GATE: OnceLock<Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("gate")
}

fn attr_str(store: &DirectStore, r: RefU64, key: &str) -> String {
    store
        .named_attmap(r)
        .ok()
        .and_then(|m| m.map.get(key).cloned())
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "-".into())
}

fn dump_node(store: &DirectStore, r: RefU64, depth: usize, lines: &mut Vec<String>) {
    if depth > 8 || lines.len() > 400 {
        return;
    }
    let noun = attr_str(store, r, "TYPE");
    let name = attr_str(store, r, "NAME");
    let keys = if matches!(noun.trim_matches('"'), "SELE" | "SPCO" | "SPEC") {
        format!(
            " QUES={} QUAL={} ANSW={} MAXA={} TANS={} DEFA={} TDEF={} CATR={}",
            attr_str(store, r, "QUES"),
            attr_str(store, r, "QUAL"),
            attr_str(store, r, "ANSW"),
            attr_str(store, r, "MAXA"),
            attr_str(store, r, "TANS"),
            attr_str(store, r, "DEFA"),
            attr_str(store, r, "TDEF"),
            attr_str(store, r, "CATR"),
        )
    } else {
        String::new()
    };
    lines.push(format!(
        "{:indent$}{} {} {}{keys}",
        "",
        noun.trim_matches('"'),
        r.0,
        name.trim_matches('"'),
        indent = depth * 2
    ));
    if let Ok(members) = store.members(r) {
        for m in members {
            dump_node(store, m, depth + 1, lines);
        }
    }
}

/// 普查模式：AMS_FILE 可覆盖目标文件（默认 ams0001_0001）；
/// 先打印 noun census 定身份，有 SPEC 才 dump 树。
#[test]
fn dump_spec_sele_tree() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let _gate = env_gate();
    let cache = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(INDEX_DIR_ENV, cache.path()) };
    let schema = Arc::new(DirectSchema::open(&templates).expect("schema"));
    let store = DirectStore::new(schema, Arc::new(NoLocator));
    // dbnum 从文件名尾段数字取（acp7000_0001 → 7000）；取不到用 AMS_DBNUM 或 1。
    let dbnum: i32 = std::env::var("AMS_DBNUM")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            let stem = db.file_stem()?.to_str()?;
            let digits: String = stem
                .split('_')
                .next()?
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            digits.parse().ok()
        })
        .unwrap_or(1);
    let db_type = std::env::var("AMS_DBTYPE").unwrap_or_else(|_| "CATA".into());
    println!("probe: {} dbnum={dbnum} db_type={db_type}", db.display());
    store.pin(DbPin {
        dbnum,
        db_type: db_type.clone(),
        file: db.clone(),
        sesno: None,
    });
    let idx = store.indexes(dbnum).expect("indexes");
    // noun census：先定这个文件是什么库（SPEC=0 说明不是 CATA）。
    let mut census = idx.noun_census();
    census.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!(
        "noun census (top15): {:?}",
        census.iter().take(15).collect::<Vec<_>>()
    );
    let specs = idx.refnos_of_noun("SPEC", None);
    let seles = idx.refnos_of_noun("SELE", None);
    let spcos = idx.refnos_of_noun("SPCO", None);
    println!(
        "census: SPEC={} SELE={} SPCO={}",
        specs.len(),
        seles.len(),
        spcos.len()
    );
    for spec in specs.iter().take(3) {
        let mut lines = Vec::new();
        dump_node(&store, *spec, 0, &mut lines);
        println!("==== SPEC {} ({} lines) ====", spec.0, lines.len());
        for l in lines {
            println!("{l}");
        }
    }
    // 抽查一个 SPCO 的祖先链：derive_answers 需要的 owner.QUES/QUAL 对。
    if let Some(spco) = spcos.first() {
        println!("==== SPCO {} ancestry ====", spco.0);
        let mut cur = *spco;
        for _ in 0..6 {
            let noun = attr_str(&store, cur, "TYPE");
            println!(
                "{} {} owner={} QUES={} QUAL={} ANSW={} TANS={} MAXA={}",
                noun.trim_matches('"'),
                cur.0,
                attr_str(&store, cur, "OWNER"),
                attr_str(&store, cur, "QUES"),
                attr_str(&store, cur, "QUAL"),
                attr_str(&store, cur, "ANSW"),
                attr_str(&store, cur, "TANS"),
                attr_str(&store, cur, "MAXA"),
            );
            let map = match store.named_attmap(cur) {
                Ok(m) => m,
                Err(_) => break,
            };
            let owner: RefU64 = map.get_owner().into();
            if owner == RefU64::default() || owner == cur {
                break;
            }
            cur = owner;
        }
    }
}

/// 端到端：真实 CATA 库上 `DirectSpecCatalog` 的回环验证——
/// 取一个真 SPCO `derive_answers` 出答卷，再 `table_lookup` 喂回去，
/// 应解析到某个 SPCO（成员序首匹配，允许是等价答卷的另一个 SPCO，
/// 但断言它的导出答卷与输入一致即证明链路自洽）。
#[test]
fn spec_catalog_roundtrip() {
    use aios_database::data_interface::direct_spec_catalog::DirectSpecCatalog;
    use pipe_model::SpecCatalog;

    let Some((db, templates)) = fixtures() else {
        return;
    };
    let _gate = env_gate();
    let cache = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(INDEX_DIR_ENV, cache.path()) };
    let schema = Arc::new(DirectSchema::open(&templates).expect("schema"));
    let store = Arc::new(DirectStore::new(schema, Arc::new(NoLocator)));
    let dbnum: i32 = std::env::var("AMS_DBNUM")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            let stem = db.file_stem()?.to_str()?;
            let digits: String = stem
                .split('_')
                .next()?
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            digits.parse().ok()
        })
        .unwrap_or(1);
    let db_type = std::env::var("AMS_DBTYPE").unwrap_or_else(|_| "CATA".into());
    store.pin(DbPin {
        dbnum,
        db_type,
        file: db.clone(),
        sesno: None,
    });
    let idx = store.indexes(dbnum).expect("indexes");
    let spcos = idx.refnos_of_noun("SPCO", None);
    let Some(spco) = spcos.first().copied() else {
        eprintln!("SKIP: no SPCO in {db:?} (not a CATA file?)");
        return;
    };

    let cat = DirectSpecCatalog::new(store.clone());
    assert!(cat.is_spco(spco), "sampled node must be SPCO");

    // derive_answers：沿祖先链重建答卷（root→leaf 序）。
    let qa = cat.derive_answers(spco).expect("derive_answers");
    println!("derived qa for SPCO {}: {:?}", spco.0, qa);
    assert!(!qa.is_empty());
    let ty = qa
        .iter()
        .find(|(k, _)| k == "TYPE")
        .map(|(_, v)| v.clone())
        .expect("TYPE pair in derived answers");

    // 找到该 SPCO 所在 SPEC：祖先链顶端（owner 链走到 noun==SPEC）。
    let mut spec = spco;
    for _ in 0..16 {
        let map = store.named_attmap(spec).expect("attmap");
        if map.get_type() == "SPEC" {
            break;
        }
        let owner: RefU64 = map.get_owner().into();
        if owner == RefU64::default() || owner == spec {
            panic!("SPCO ancestry never reaches SPEC");
        }
        spec = owner;
    }
    assert!(cat.root_selector(spec).is_some(), "SPEC root selector");

    // 回环：导出答卷喂回 table_lookup → 命中 SPCO，且其导出答卷等价。
    let hit = cat
        .table_lookup(spec, &ty, &qa)
        .expect("table_lookup round-trip");
    assert!(cat.is_spco(hit), "table_lookup must land on SPCO");
    let qa2 = cat.derive_answers(hit).expect("derive hit");
    println!("round-trip hit {}: {:?}", hit.0, qa2);
    assert_eq!(qa, qa2, "hit SPCO must carry equivalent answers");
}

/// 端到端：真实库上会话装配面验证——
/// 1) `probe_writable_dbnums`：声明证据缺席 → 会话默认可写，`ACCESSDB=Read`/
///    `LREFO` 收窄 → 只读 + problem（DESI 文件面没有 DB 元素，证据在 CURD 上）；
/// 2) `DirectPipeCatalog::alloc_refno(under)` 在**属主所在库**内取水，
///    断言返回 refno 的 dbnum 与属主库一致且不撞既有 refno。
/// 夹具缺席自动跳过（与上方测试同一 fixtures() 闸）。
#[test]
fn pipe_catalog_session_probe() {
    use aios_database::data_interface::direct_pipe_catalog::{
        probe_writable_dbnums, DirectPipeCatalog,
    };
    use aios_database::data_interface::mdb_membership::{DbAccess, MdbDatabase, DESI_STYP};
    use pipe_model::PipeCatalog;

    let Some((db, templates)) = fixtures() else {
        return;
    };
    let _gate = env_gate();
    let cache = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(INDEX_DIR_ENV, cache.path()) };
    let schema = Arc::new(DirectSchema::open(&templates).expect("schema"));
    let store = Arc::new(DirectStore::new(schema, Arc::new(NoLocator)));
    let dbnum: i32 = std::env::var("AMS_DBNUM")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            let stem = db.file_stem()?.to_str()?;
            let digits: String = stem
                .split('_')
                .next()?
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            digits.parse().ok()
        })
        .unwrap_or(1);
    let db_type = std::env::var("AMS_DBTYPE").unwrap_or_else(|_| "CATA".into());
    store.pin(DbPin {
        dbnum,
        db_type,
        file: db.clone(),
        sesno: None,
    });

    // 1) 写权限探测：文件面没有 DB 元素（DESI 库里 refnos_of_noun("DB") 为空），
    //    证据在 CURD 声明元素上——probe 走声明 access 回调 + 文件可达性。
    let idx = store.indexes(dbnum).expect("indexes");
    let db_elements = idx.refnos_of_noun("DB", None);
    println!("dbnum={dbnum} DB elements: {db_elements:?}");
    let declared = MdbDatabase {
        dbnum: dbnum as u32,
        styp: DESI_STYP,
        proj: 0,
        declared_project_code: None,
        name: "forensics-declared".into(),
        path: Some(db.clone()),
        project: None,
        shadowed: Vec::new(),
    };
    // 声明面没留证据 → E3D 会话默认可写。
    let (writable, problems) = probe_writable_dbnums(&store, [declared.clone()].iter(), |_| None);
    println!("default writable={writable:?} problems={problems:?}");
    assert!(writable.contains(&dbnum), "evidence-absent declared db must default writable");
    assert!(problems.is_empty(), "readable bound file must not surface problems: {problems:?}");
    // 声明收窄 → 只读 + problem。
    let (ro, ro_problems) = probe_writable_dbnums(&store, [declared.clone()].iter(), |_| {
        Some(DbAccess { access: Some("Read".into()), ..Default::default() })
    });
    assert!(!ro.contains(&dbnum), "ACCESSDB=Read must be read-only");
    assert!(!ro_problems.is_empty(), "read-only narrowing must surface a problem");
    let (ro2, _) = probe_writable_dbnums(&store, [declared].iter(), |_| {
        Some(DbAccess { reference_only: true, ..Default::default() })
    });
    assert!(!ro2.contains(&dbnum), "LREFO must be read-only");

    // 2) alloc_refno(under)：任取库内一个元素当属主，新 refno 必须同库且不撞号。
    let owner_ref = idx
        .noun_census()
        .iter()
        .flat_map(|(h, _)| idx.refnos_of_noun_hash(*h, None))
        .next()
        .expect("fixture db has elements");
    let cat = DirectPipeCatalog::new(store.clone(), writable.clone());
    let fresh = cat.alloc_refno(owner_ref);
    assert_ne!(fresh, RefU64::default(), "alloc_refno must not return nulref");
    if let Ok(resolved) = store.dbnum_of(fresh) {
        assert_eq!(
            resolved, dbnum,
            "allocated refno must land in the owner's db"
        );
    }
    // 不撞既有：在任一 noun 普查桶里都不应出现。
    let exists = idx
        .noun_census()
        .iter()
        .flat_map(|(h, _)| idx.refnos_of_noun_hash(*h, None))
        .any(|r| r == fresh);
    assert!(!exists, "allocated refno collides with an existing element");
    // 两次分配不互撞。
    let fresh2 = cat.alloc_refno(owner_ref);
    assert_ne!(fresh, fresh2, "allocations must be unique");
    println!("allocated {} / {} in dbnum={dbnum}", fresh.0, fresh2.0);
}

/// 端到端链路证明：真库 DirectStore → `DirectPipeCatalog` → `PipeModelService`
/// `create_branch` 出 `MutationBatch` → `render_batch` → `EditOp` →
/// `rendered_batch_to_ele_operations` → `EleOperationData`——全链在真 AMS
/// 设计库上走一遍（AMS_FILE 默认 ams0001，可指 ams8000 DESI 库）。
///
/// 断言三件事：
/// 1. `create_branch` 对真 PIPE 出批（PSPE/BORE 从库里真读，不 mock）；
/// 2. 批经渲染出 `EditOp::Add`，noun=BRAN、owner=真 PIPE、新 refno 解码回
///    属主同库（alloc 水位与 dbnum 路由生效）；
/// 3. 适配层吐 `EleOperationData::Add`——可进 `terminal_persist_changes` 收口。
/// 顺带跑 `validate_fabrication`（纯读面）确认真支管/成员遍历不炸。
#[test]
fn pipe_service_end_to_end_on_real_db() {
    use aios_database::data_interface::direct_pipe_catalog::DirectPipeCatalog;
    use aios_database::data_interface::pipe_model_adapter::rendered_batch_to_ele_operations;
    use aios_database::data_interface::ele_ops::EleOperationDetail;
    use pipe_model::render::{render_batch, EditOp};
    use pipe_model::{Context, Flow, PipeCatalog, PipeModel, PipeModelService};
    use std::collections::BTreeSet;

    let Some((db, templates)) = fixtures() else {
        return;
    };
    let _gate = env_gate();
    let cache = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(INDEX_DIR_ENV, cache.path()) };
    let schema = Arc::new(DirectSchema::open(&templates).expect("schema"));
    let store = Arc::new(DirectStore::new(schema, Arc::new(NoLocator)));
    let dbnum: i32 = std::env::var("AMS_DBNUM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let db_type = std::env::var("AMS_DBTYPE").unwrap_or_else(|_| "CATA".into());
    store.pin(DbPin {
        dbnum,
        db_type,
        file: db.clone(),
        sesno: None,
    });

    // 真 PIPE：要 PSPE+BORE 齐（create_branch 派生支管需要）。
    let idx = store.indexes(dbnum).expect("indexes");
    let writable: BTreeSet<i32> = [dbnum].into_iter().collect();
    let cat = DirectPipeCatalog::new(store.clone(), writable);
    let render_cat = DirectPipeCatalog::new(store.clone(), [dbnum].into_iter().collect());
    let pipes = idx.refnos_of_noun("PIPE", None);
    let pipe = pipes.iter().copied().find(|p| {
        cat.attr(*p, "PSPE").is_some() && cat.attr(*p, "BORE").is_some()
    });
    let Some(pipe) = pipe else {
        eprintln!("dbnum={dbnum} 里没有 PSPE+BORE 齐的 PIPE（共 {} 个），跳过", pipes.len());
        return;
    };
    let pipe_name = cat.name_of(pipe).unwrap_or_default();
    let brans_before: Vec<_> = cat
        .members_of(pipe)
        .into_iter()
        .filter(|m| cat.noun_of(*m).as_deref() == Some("BRAN"))
        .collect();
    println!("pipe={pipe_name} refno={} brans={:?}", pipe.0, brans_before.len());

    let ctx = Context {
        element: pipe,
        dbnum: dbnum as u32,
        flow: Flow::Forwards,
        end: None,
    };
    let service = PipeModelService::new(cat);

    // 1) create_branch：真 PIPE 上出批。
    let batch = service.create_branch(&ctx, pipe).expect("create_branch on real PIPE");
    assert!(!batch.ops.is_empty(), "create_branch 应产 op");

    // 2) render_batch → EditOp::Add：noun=BRAN、owner=pipe、refno 同库。
    let rendered = render_batch(&batch, &render_cat);
    let adds: Vec<_> = rendered
        .ops
        .iter()
        .filter_map(|op| match op {
            EditOp::Add(el) => Some(el),
            _ => None,
        })
        .collect();
    assert!(
        adds.iter().any(|el| el.noun_name == "BRAN" && el.owner == pipe),
        "批里应有一条 Add(BRAN under 真 PIPE)；实际 ops={:?}",
        rendered.ops.iter().map(|op| match op {
            EditOp::Add(el) => format!("Add({} owner={})", el.noun_name, el.owner.0),
            EditOp::Modify(_) => "Modify".into(),
            EditOp::Delete { .. } => "Delete".into(),
        }).collect::<Vec<_>>()
    );
    for el in &adds {
        assert_eq!(
            store.dbnum_of(el.refno).expect("allocated refno must decode a dbnum"),
            dbnum,
            "新建元素 refno 必须落在属主同库"
        );
    }
    // 显式 NAME 契约：新支管自动名 <pipe>/B<n>。
    let bran_name = adds
        .iter()
        .find(|el| el.noun_name == "BRAN")
        .map(|el| el.name.clone())
        .unwrap_or_default();
    assert!(
        bran_name.starts_with(&format!("{pipe_name}/B")),
        "支管自动名应形如 {pipe_name}/B<n>，实际 {bran_name}"
    );

    // 3) 适配层 → EleOperationData::Add（可进 terminal_persist_changes）。
    let ops = rendered_batch_to_ele_operations(&rendered, 999);
    assert!(
        ops.iter().any(|o| matches!(o.detail, EleOperationDetail::Add(_))),
        "适配层应吐 EleOperationData::Add"
    );
    assert!(ops.iter().all(|o| o.sesno == 999), "sesno 透传");

    // 4) validate_fabrication（纯读面）：真 PIPE 遍历不炸，状态机出定论。
    let (status, messages) = service
        .validate_fabrication(&ctx, pipe)
        .expect("validate_fabrication on real PIPE");
    println!("validate_fabrication: {status:?} messages={}", messages.len());
}
