//! 真库门（手动）：`DirectStore` 以 [`MdbRouteDirectory`] 为定位器时的跨库读通与文件自证。
//!
//! 切片 `2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` W3 的验收写的是「双库夹具跨库
//! 引用读通」——仓库里没有可入库的 dabacon 夹具（一个最小的库也是几十页 B+tree），所以这一条
//! 拿当前 `DbOption.toml` 指向的真实 MDB 跑：
//!
//! ```text
//! cargo test --lib -- --ignored direct_store_refno_live --nocapture
//! ```
//!
//! 两条门：
//! 1. 从一个 DESI 库扫元素，凡带引用属性指向**别的库**的，`store.attrs(target)` 必须读通，
//!    且目标库由 `RefNo::dbno()` 解出、经路由目录开——路上没有任何 ref0 表；
//! 2. 把 dbnum N+1 钉到 dbnum N 的文件上，开库必须报 `DbNumberMismatch`（不变量 3）。

use std::collections::BTreeSet;
use std::sync::Arc;

use aios_core::{NamedAttrValue, RefU64};
use e3d_io::ReadOnlyEngine;
use e3d_io::engine::ScanTier;

// 本文件经 `direct_store.rs` 尾部的 `#[path] mod refno_live_tests;` 挂进来：`super` 就是 `direct_store`。
use super::super::mdb_membership;
use super::super::mdb_route_directory::{DbRoute, MdbRouteDirectory};
use super::{DbPin, DirectSchema, DirectStore, DirectStoreError};

/// 扫多少个元素还找不到跨库引用就放弃（AMS 8000 前几十个元素里就有 SPRE / CATR）。
const SCAN_BUDGET: usize = 5_000;
/// 跟通多少条跨库引用算够。
const FOLLOW_TARGET: usize = 25;

fn live_store() -> (Arc<DirectStore>, MdbRouteDirectory) {
    let option = aios_core::get_db_option();
    let schema = Arc::new(DirectSchema::open_from_env().expect("load E3D schema pack"));
    let membership = mdb_membership::resolve(option, &option.project_name, &option.mdb_name)
        .expect("resolve MDB membership from SYS");
    let directory = MdbRouteDirectory::from_membership(&membership, schema.template_dir());
    assert!(
        directory.verify_all_headers().is_empty(),
        "路由目录里有文件头自证不过的库，先修数据再跑这条门"
    );
    let store = Arc::new(DirectStore::new(schema, Arc::new(directory.clone())));
    (store, directory)
}

fn first_element_db(directory: &MdbRouteDirectory) -> DbRoute {
    directory
        .element_databases()
        .next()
        .cloned()
        .expect("MDB 至少要声明一个 DESI / ISOD 库")
}

#[test]
#[ignore = "manual live: needs DbOption.toml pointing at a real E3D project and MDB"]
fn direct_store_refno_live_cross_db_references_read_through_by_decoding_the_target() {
    let (store, directory) = live_store();
    let desi = first_element_db(&directory);
    let own_dbnum = desi.dbnum as i32;

    let mut engine = ReadOnlyEngine::open(&desi.path).expect("open the DESI file");
    let mut examined = 0usize;
    let mut followed = 0usize;
    let mut foreign_dbs: BTreeSet<i32> = BTreeSet::new();
    let mut outside_project = 0usize;

    for item in engine
        .scan_elements(ScanTier::Header)
        .expect("scan the DESI index")
    {
        let element = item.expect("scan item");
        examined += 1;
        if examined > SCAN_BUDGET || followed >= FOLLOW_TARGET {
            break;
        }
        let refno = RefU64::from_two_nums(element.refno.word0, element.refno.word1);
        // 门 1a：扫出来的每个元素，解码出的库就是正在扫的库。
        assert_eq!(
            store.dbnum_of(refno).expect("scanned refno routes"),
            own_dbnum,
            "{refno} 解出的 dbnum 与它所在的文件不一致"
        );
        let attrs = store
            .attrs(refno)
            .unwrap_or_else(|error| panic!("读本库元素 {refno} 失败: {error}"));
        for (name, value) in &attrs.map.map {
            let NamedAttrValue::RefU64Type(target) = value else {
                continue;
            };
            // 指向本工程之外（或非法）的引用是常态：问路给 None，跳过，不算失败。
            let Some(target_db) = store
                .locator_dbnum(*target)
                .expect("locator_dbnum is infallible")
            else {
                outside_project += 1;
                continue;
            };
            if target_db == own_dbnum {
                continue;
            }
            // 门 1b：跨库引用按解码出的 dbnum 换库读通。
            let target_attrs = store.attrs(*target).unwrap_or_else(|error| {
                panic!("跟随 {refno}.{name} = {target} 到 dbnum {target_db} 失败: {error}")
            });
            assert!(
                !target_attrs.map.map.is_empty(),
                "{target}（dbnum {target_db}）读出来是空属性表"
            );
            assert_eq!(store.dbnum_of(*target).unwrap(), target_db);
            foreign_dbs.insert(target_db);
            followed += 1;
        }
    }

    eprintln!(
        "[refno-live] DESI {own_dbnum}: examined={examined} followed={followed} foreign_dbs={foreign_dbs:?} outside_project={outside_project} open_sessions={:?}",
        store
    );
    assert!(
        followed > 0,
        "扫了 {examined} 个元素没有一条可跟的跨库引用——换一个 DESI 库或调大 SCAN_BUDGET"
    );
    assert!(
        foreign_dbs
            .iter()
            .all(|dbnum| directory.contains(*dbnum as u32)),
        "跟到的库必须都在路由目录里: {foreign_dbs:?}"
    );
}

#[test]
#[ignore = "manual live: needs DbOption.toml pointing at a real E3D project and MDB"]
fn direct_store_refno_live_a_file_claiming_another_dbnum_is_refused_at_open() {
    let (store, directory) = live_store();
    let desi = first_element_db(&directory);
    let wrong_dbnum = desi.dbnum as i32 + 1;
    // 把 N+1 钉到 N 的文件上：路径存在、能开、会话能解——只有文件自报的库号不对。
    store.pin(DbPin {
        dbnum: wrong_dbnum,
        db_type: desi.db_type.clone(),
        file: desi.path.clone(),
        sesno: None,
    });
    let probe = RefU64::from_two_nums(desi.dbnum, 1);
    let error = store.attrs_in(wrong_dbnum, probe).unwrap_err();
    assert_eq!(
        error,
        DirectStoreError::DbNumberMismatch {
            dbnum: wrong_dbnum,
            actual: desi.dbnum as i32,
            file: desi.path.clone(),
        },
        "{error}"
    );
    assert!(
        store.pinned_sesno(wrong_dbnum).is_none(),
        "核头失败的库不得留下已开会话"
    );
}
