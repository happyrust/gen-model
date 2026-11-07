//! SYS 固定结构导航（WORL `24575/3` → MDBW `24575/7` → 成员）的门。
//!
//! 真库那两条只在装了 E3D 3.1 AMS 的机器上跑，没装就打印 skip；文本护栏那条哪儿都跑。
//! 不在这里断言 `read_stats.scan_calls`——lib 测试并行，全局计数器会被别的用例撞；
//! 「启动 scan_calls == 0」由 `refno_routing_stage0_baseline` 与懒加载切片的验收脚本量。

use super::*;

const AMS_SYS: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\amssys";

fn installed_ams_sys() -> Option<&'static Path> {
    let path = Path::new(AMS_SYS);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skip: {AMS_SYS} 不在本机");
        None
    }
}

/// 两个固定位置在真库上就是 WORL 与 MDBW：owner 为空、noun 对得上、名字是 `/*` 与 `/*M`。
#[test]
fn the_sys_world_and_mdb_world_sit_where_the_constants_say() {
    let Some(sys) = installed_ams_sys() else {
        return;
    };
    let sys_db = SysDb::open(sys).expect("open AMS SYS");
    let world = sys_db.set.element(SYS_WORLD);
    assert!(world.exists().unwrap(), "{SYS_WORLD} 不在索引里");
    assert_eq!(world.noun_hash().unwrap(), e3d_attlib::db1_hash("WORL"));
    assert!(world.owner().unwrap().is_none(), "WORL 不该有属主");
    assert_eq!(world.get_string("NAME").unwrap().as_deref(), Some("/*"));

    let mdbw = sys_db.set.element(SYS_MDB_WORLD);
    assert!(mdbw.exists().unwrap(), "{SYS_MDB_WORLD} 不在索引里");
    assert_eq!(mdbw.noun_hash().unwrap(), e3d_attlib::db1_hash("MDBW"));
    assert_eq!(
        mdbw.owner().unwrap().map(|owner| owner.refno()),
        Some(SYS_WORLD),
        "MDBW 的属主必须是 WORL"
    );
    assert_eq!(mdbw.get_string("NAME").unwrap().as_deref(), Some("/*M"));
}

/// 导航找到的 `/ALL` 与 Named 档全库扫找到的逐个相同；不是 MDB 名字的回空，不报错。
#[test]
fn mdb_lookup_by_navigation_agrees_with_the_named_scan() {
    let Some(sys) = installed_ams_sys() else {
        return;
    };
    let sys_db = SysDb::open(sys).expect("open AMS SYS");
    let by_navigation = sys_db.mdbs_named("/ALL").expect("mdbs_named");
    assert_eq!(
        by_navigation.len(),
        1,
        "AMS SYS 里 /ALL 恰好一个：{by_navigation:?}"
    );
    let by_scan = sys_db.named("/ALL").expect("named");
    assert_eq!(by_navigation, by_scan);
    assert!(
        sys_db.mdbs_named("/NOT-AN-MDB").unwrap().is_empty(),
        "不存在的 MDB 名字回空"
    );
    // WORL 自己叫 `/*`，但它不是 MDB：导航只认 MDBW 下 noun 为 MDB 的成员。
    assert!(sys_db.mdbs_named("/*").unwrap().is_empty());
}

/// 正常路径不许扫：`mdbs_named` 函数体里没有 `scan_elements` / `find_named`，
/// 唯一的 `self.named(` 是格式假设被打破时那条出声回落。
#[test]
fn the_navigation_path_never_scans() {
    let source = include_str!("mdb_membership.rs");
    let start = source
        .find("fn mdbs_named(&self, name: &str)")
        .expect("mdbs_named exists");
    let body = &source[start..];
    let end = body.find("\n    }\n").expect("function end");
    let body = &body[..end];
    assert!(!body.contains("scan_elements("));
    assert!(!body.contains("find_named("));
    assert_eq!(
        body.matches("self.named(").count(),
        1,
        "只允许一处出声回落到扫描"
    );
    assert!(body.contains("report_sys_error("), "回落必须出声");
    let declaration_start = source
        .find("fn read_declaration(")
        .expect("read_declaration exists");
    let declaration = &source[declaration_start..];
    let declaration = &declaration[..declaration.find("\n}\n").expect("function end")];
    assert!(
        declaration.contains(".mdbs_named(wanted)") && !declaration.contains(".named(wanted)"),
        "read_declaration 必须走导航路径"
    );
}
