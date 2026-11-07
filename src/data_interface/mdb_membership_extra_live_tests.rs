//! 真库门（手动）：`extra_element_dbnums` 补充成员（ADR-095）在当前 `DbOption.toml` 指向的真实
//! AMS 项目上是否真的进了成员、路由、范围与模型源，以及跨库引用能否经补充的 CATA 读通。
//!
//! ```text
//! cargo test --lib -- --ignored extra_members_live --nocapture --test-threads=1
//! ```
//!
//! 期望的配置是 `extra_element_dbnums = [251095, 251137, 251049]`：两个 DESI（`/PIPE_4DK_FWD`、
//! `/HVAC_2DK_FWD`）加上 251095 管件 SPRE 指向的 CATA `/SP_LO`。

use std::collections::BTreeSet;
use std::sync::Arc;

use aios_core::{NamedAttrValue, RefU64};
use e3d_io::ReadOnlyEngine;
use e3d_io::engine::ScanTier;
use e3d_io::refno::RefNo;

use super::super::cata_closure::CataDbLocator;
use super::super::direct_store::{DirectSchema, DirectStore};
use super::super::mdb_route_directory::MdbRouteDirectory;
use super::super::update_scope::UpdateScope;
use super::*;

const PIPE: u32 = 251095;
const HVAC: u32 = 251137;
const SP_LO: u32 = 251049;
/// 两个补充设计库的 SITE（`/PIPE_4DK_FWD` = `2013287639/1`，`/HVAC_2DK_FWD` = `2013287681/1`）。
const PIPE_SITE: (u32, u32) = (2013287639, 1);
const HVAC_SITE: (u32, u32) = (2013287681, 1);
/// 251095 里一个法兰：SPRE 指向 251049 的 `/SP_LO/FLOF-150`（8009 `pe:2013287593_7932`）。
const PIPE_FLAN: (u32, u32) = (2013287639, 100);
const SP_LO_SPCO: (u32, u32) = (2013287593, 7932);

fn live_membership() -> MdbMembership {
    let option = aios_core::get_db_option();
    assert_eq!(
        crate::options::extra_element_dbnums(),
        vec![PIPE, HVAC, SP_LO],
        "本门按 DbOption.toml 的 extra_element_dbnums = [251095, 251137, 251049] 写"
    );
    resolve(option, &option.project_name, &option.mdb_name)
        .expect("resolve MDB membership from SYS")
}

#[test]
#[ignore = "manual live: needs DbOption.toml pointing at the real AMS project"]
fn extra_members_live_bind_to_their_own_files_and_stay_out_of_the_declaration() {
    let membership = live_membership();
    let extras: Vec<(u32, i64, String)> = membership
        .extra_databases()
        .iter()
        .map(|db| {
            let file = db
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            (db.dbnum, db.styp, file)
        })
        .collect();
    eprintln!("[extra-live] extras={extras:?}");
    assert_eq!(
        extras,
        vec![
            (PIPE, DESI_STYP, "ams251095_0001".to_string()),
            (HVAC, DESI_STYP, "ams251137_0001".to_string()),
            (SP_LO, CATA_STYP, "ams251049_0001".to_string()),
        ]
    );
    // 每份文件自报的库号与类型就是收下时定的那个。
    for db in membership.extra_databases() {
        let header = super::super::db_file_header::DbFileHeader::read(db.path.as_ref().unwrap())
            .expect("read extra file header");
        assert_eq!(header.dbnum, db.dbnum as i32);
    }
    // 声明口径不变：CURD 里没有它们。
    assert!(
        membership
            .databases()
            .iter()
            .all(|db| ![PIPE, HVAC, SP_LO].contains(&db.dbnum)),
        "补充成员不得混进 CURD 声明"
    );
    let extra_problems: Vec<&String> = membership
        .problems()
        .iter()
        .filter(|problem| problem.contains("extra_element_dbnums"))
        .collect();
    assert!(extra_problems.is_empty(), "{extra_problems:#?}");
    // 只增不减：模型面 = 声明 + 补充，一个不少。
    assert_eq!(
        membership.model_databases().count(),
        membership.databases().len() + 3
    );
}

#[test]
#[ignore = "manual live: needs DbOption.toml pointing at the real AMS project"]
fn extra_members_live_route_after_every_declared_element_database() {
    let membership = live_membership();
    let directory = MdbRouteDirectory::from_membership(&membership, "T:/unused");
    let declared: Vec<u32> = membership
        .element_databases()
        .filter(|db| db.path.is_some())
        .map(|db| db.dbnum)
        .collect();
    let routed: Vec<u32> = directory
        .element_databases()
        .map(|route| route.dbnum)
        .collect();
    eprintln!(
        "[extra-live] declared element dbs={} routed element dbs={} tail={:?}",
        declared.len(),
        routed.len(),
        &routed[routed.len().saturating_sub(3)..]
    );
    // CURD 序在前、补充在后；CATA 不是元素库，不进 element_databases。
    assert!(
        routed.starts_with(&declared),
        "声明的元素库次序被打乱或少了"
    );
    assert_eq!(&routed[declared.len()..], &[PIPE, HVAC]);
    assert_eq!(
        directory.route_dbno(SP_LO).expect("SP_LO 路由").db_type,
        "CATA"
    );
    for dbnum in [PIPE, HVAC, SP_LO] {
        assert!(directory.contains(dbnum));
    }
    let header_errors: Vec<_> = directory
        .verify_all_headers()
        .into_iter()
        .filter(|error| {
            let text = error.to_string();
            [PIPE, HVAC, SP_LO]
                .iter()
                .any(|dbnum| text.contains(&dbnum.to_string()))
        })
        .collect();
    assert!(header_errors.is_empty(), "{header_errors:#?}");
}

#[test]
#[ignore = "manual live: needs DbOption.toml pointing at the real AMS project"]
fn extra_members_live_are_admitted_without_narrowing_the_declared_scope() {
    let _lock = REGISTRY_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let option = aios_core::get_db_option();
    let mdb = aios_core::helper::to_e3d_name(&option.mdb_name).into_owned();
    let _restore = RegistryRestore::capture(&option.project_name, &mdb);
    let membership = install(live_membership());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let scope = runtime
        .block_on(UpdateScope::resolve(&option.mdb_name))
        .expect("resolve the configured MDB scope");
    assert!(scope.admits("DESI", PIPE) && scope.admits("DESI", HVAC));
    // CATA 只进模型面的目录定位，不进增量执行范围（ADR-095 D7）。
    assert!(!scope.admits("CATA", SP_LO));
    let declared: BTreeSet<u32> = scope.declared_desi().collect();
    assert!(!declared.contains(&PIPE) && !declared.contains(&HVAC));
    for db in membership.element_databases() {
        let db_type = if db.styp == ISOD_STYP { "ISOD" } else { "DESI" };
        assert!(
            scope.admits(db_type, db.dbnum),
            "声明的 {} 被挡掉了",
            db.dbnum
        );
    }
    eprintln!(
        "[extra-live] scope declared={} extra={:?}",
        declared.len(),
        scope.extra_dbnums().collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "manual live: needs DbOption.toml pointing at the real AMS project"]
fn extra_members_live_feed_the_model_sources_and_root_routing() {
    let (pins, locator) =
        crate::fast_model::e3d_model_service::current_mdb_sources().expect("current model sources");
    for (dbnum, db_type) in [(PIPE, "DESI"), (HVAC, "DESI"), (SP_LO, "CATA")] {
        let pin = pins
            .iter()
            .find(|pin| pin.dbnum == dbnum as i32)
            .unwrap_or_else(|| panic!("模型源没钉 {dbnum}"));
        assert!(pin.db_type.eq_ignore_ascii_case(db_type), "{pin:?}");
        assert!(locator.file_of(dbnum).is_some(), "目录定位器找不到 {dbnum}");
    }
    let site = |(a, b): (u32, u32)| RefNo::new(a, b);
    assert_eq!(
        crate::data_interface::model_source::dbnum_of_root(site(PIPE_SITE)).unwrap(),
        PIPE
    );
    assert_eq!(
        crate::data_interface::model_source::dbnum_of_root(site(HVAC_SITE)).unwrap(),
        HVAC
    );
}

#[test]
#[ignore = "manual live: needs DbOption.toml pointing at the real AMS project"]
fn extra_members_live_pipe_spec_references_read_through_the_extra_catalogue() {
    let membership = live_membership();
    let schema = Arc::new(DirectSchema::open_from_env().expect("load E3D schema pack"));
    let directory = MdbRouteDirectory::from_membership(&membership, schema.template_dir());
    let store = DirectStore::new(schema, Arc::new(directory));
    let flan = RefU64::from_two_nums(PIPE_FLAN.0, PIPE_FLAN.1);
    let attrs = store.attrs(flan).expect("read the 251095 flange");
    let Some(NamedAttrValue::RefU64Type(spre)) = attrs.map.map.get("SPRE") else {
        panic!(
            "法兰没有 SPRE: {:?}",
            attrs.map.map.keys().collect::<Vec<_>>()
        );
    };
    assert_eq!(*spre, RefU64::from_two_nums(SP_LO_SPCO.0, SP_LO_SPCO.1));
    assert_eq!(store.dbnum_of(*spre).unwrap(), SP_LO as i32);
    let target = store.attrs(*spre).expect("SPRE 经补充的 CATA 读通");
    assert!(!target.map.map.is_empty());

    // 扫整个 251095：凡指向 251049 的引用都要读得通——这正是补 CATA 之前 941 个元件失败的地方。
    let pipe_file = membership
        .extra_databases()
        .iter()
        .find(|db| db.dbnum == PIPE)
        .and_then(|db| db.path.clone())
        .unwrap();
    let mut engine = ReadOnlyEngine::open(&pipe_file).expect("open 251095");
    let (mut elements, mut followed) = (0usize, 0usize);
    let mut targets: BTreeSet<i32> = BTreeSet::new();
    for item in engine.scan_elements(ScanTier::Header).expect("scan 251095") {
        let element = item.expect("scan item");
        elements += 1;
        let refno = RefU64::from_two_nums(element.refno.word0, element.refno.word1);
        let attrs = store
            .attrs(refno)
            .unwrap_or_else(|error| panic!("读 {refno} 失败: {error}"));
        for (name, value) in &attrs.map.map {
            let NamedAttrValue::RefU64Type(target) = value else {
                continue;
            };
            if store.locator_dbnum(*target).ok().flatten() != Some(SP_LO as i32) {
                continue;
            }
            store.attrs(*target).unwrap_or_else(|error| {
                panic!("{refno}.{name} = {target}（251049）读不通: {error}")
            });
            targets.insert(SP_LO as i32);
            followed += 1;
        }
    }
    eprintln!("[extra-live] 251095 elements={elements} refs_into_251049={followed}");
    assert!(followed > 0 && targets.contains(&(SP_LO as i32)));
}

/// 文件与 8009 对账用：按 TYPE 数两个补充设计库的元素，打成一行 JSON（脚本拿它比 `pe`）。
#[test]
#[ignore = "manual live: needs DbOption.toml pointing at the real AMS project"]
fn extra_members_live_count_nouns_for_the_database_cross_check() {
    let membership = live_membership();
    let schema = Arc::new(DirectSchema::open_from_env().expect("load E3D schema pack"));
    let directory = MdbRouteDirectory::from_membership(&membership, schema.template_dir());
    let store = DirectStore::new(schema, Arc::new(directory));
    for dbnum in [PIPE, HVAC] {
        let file = membership
            .extra_databases()
            .iter()
            .find(|db| db.dbnum == dbnum)
            .and_then(|db| db.path.clone())
            .unwrap();
        let mut engine = ReadOnlyEngine::open(&file).expect("open extra file");
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for item in engine.scan_elements(ScanTier::Header).expect("scan") {
            let element = item.expect("scan item");
            let refno = RefU64::from_two_nums(element.refno.word0, element.refno.word1);
            let attrs = store.attrs(refno).expect("read element");
            let noun = match attrs.map.map.get("TYPE") {
                Some(NamedAttrValue::StringType(noun)) | Some(NamedAttrValue::WordType(noun)) => {
                    noun.clone()
                }
                other => format!("{other:?}"),
            };
            *counts.entry(noun).or_default() += 1;
        }
        eprintln!(
            "[extra-live-nouns] {dbnum} {}",
            serde_json::to_string(&counts).unwrap()
        );
    }
}
