//! MBD 宿主测试共用的开库姿势（`.planning/2026-11-07-mbd-all-types-host-readers/guide.md` §5）。
//!
//! 直读 AvevaMarineSample 的 DESI 库，目录库经 AMS 优先的链式 resolver 惰性解析：WCC 支撑的 PCLA / SCTN
//! 目录在 ZDJ 7600，墙的 SPCO 在 acp7001，MDS 在 acp7320；dbnum 7000 在 ams000 与 acp000 各有一份，三个目录合进
//! 一次扫描会被判成分不出正身，所以每个目录单独扫、按序问（`aios_database::mbd::host::ChainedDbResolver`）。
//! 服务端按 MDB 解析的 `build_set` 是 `pub(crate)`，测试里用不了。
//!
//! 样本库与模板目录不随仓库走：缺哪个都 `eprintln!("SKIP …")` 后返回 `None`，调用方直接 return（不用 `#[ignore]`）。
//! 用法：`#[path = "common/mbd_db.rs"] mod mbd_db;`（不经 `common/mod.rs`，那边要连 SurrealDB）。

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_database::data_interface::direct_store::TEMPLATE_DIR_ENV;
use aios_database::mbd::host::ChainedDbResolver;
use e3d_io::db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for};
use e3d_io::uda_catalog::UdaCatalog;
use e3d_model::db_discovery::DirectoryDbResolver;

pub const AMS000: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";
pub const ACP000: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000";
pub const ZDJ000: &str = r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000";
/// 链式解析的顺序：AMS 优先。
pub const DIRS: [&str; 3] = [AMS000, ACP000, ZDJ000];
pub const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

pub fn template_dir() -> PathBuf {
    std::env::var_os(TEMPLATE_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT))
}

/// 钉入 `ams000` 下的这些 DESI 库（如 `ams7999_0001`），目录库按链惰性解析。
pub fn open(dbs: &[&str]) -> Option<Arc<DbSet>> {
    open_with(dbs, false)
}

/// 同 [`open`]，再挂上三个目录里全部 DICT 库的 UDA 定义（按目录顺序、库号升序合并，先到先得）。
/// 生产按 MDB 的 DICT 名单（`CURD` 序）读同一种表；AMS 的 `/ALL` 还有 DICT 在 SCB 底下，这里不全。
pub fn open_with_uda(dbs: &[&str]) -> Option<Arc<DbSet>> {
    open_with(dbs, true)
}

fn open_with(dbs: &[&str], with_uda: bool) -> Option<Arc<DbSet>> {
    let templates = template_dir();
    if !templates.join("attlib.dat").is_file() || !Path::new(AMS000).is_dir() {
        eprintln!("SKIP: sample databases or templates not present ({AMS000}, {templates:?})");
        return None;
    }
    if let Some(missing) = dbs.iter().find(|db| !Path::new(AMS000).join(db).is_file()) {
        eprintln!("SKIP: {missing} not present in {AMS000}");
        return None;
    }
    let attlib = match e3d_attlib::AttlibData::parse_file(templates.join("attlib.dat")) {
        Ok(attlib) => Arc::new(attlib),
        Err(error) => {
            eprintln!("SKIP: attlib.dat does not parse: {error}");
            return None;
        }
    };
    let resolvers: Vec<DirectoryDbResolver> = DIRS
        .iter()
        .filter(|dir| Path::new(dir).is_dir())
        .filter_map(|dir| DirectoryDbResolver::scan(&[PathBuf::from(dir)], &templates).ok())
        .collect();
    let uda = with_uda.then(|| {
        let mut merged = UdaCatalog::default();
        for resolver in &resolvers {
            let (catalog, unreadable) =
                resolver.uda_catalog(&resolver.dbnos_of_type("DICT"), &templates, &attlib);
            for (dbno, file, error) in unreadable {
                eprintln!("note: DICT {dbno} ({}) unreadable: {error}", file.display());
            }
            merged.merge_in_order(catalog);
        }
        Arc::new(merged)
    });
    let chain = ChainedDbResolver(
        resolvers
            .into_iter()
            .map(|resolver| Box::new(resolver) as Box<dyn DbFileResolver>)
            .collect(),
    );
    let mut set = DbSet::with_resolver(attlib, Box::new(chain)).with_element_cache();
    if let Some(uda) = uda {
        set = set.with_uda_catalog(uda);
    }
    let template = template_file_for(&templates, "DESI").ok()?;
    for db in dbs {
        if let Err(error) = set.add_db(DbFilePin {
            file: Path::new(AMS000).join(db),
            template: template.clone(),
            db_type: Some("DESI".to_string()),
            sesno: None,
        }) {
            eprintln!("SKIP: cannot pin {db}: {error}");
            return None;
        }
    }
    Some(Arc::new(set))
}
