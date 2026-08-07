//! 房间归属的手工重建入口（诊断 + 修复）。
//!
//! 起因：真库上 `room_relate` 全库只剩 1 条，而 `room_panel_relate` 有 497 行
//! （350 属 1112、147 属 7997——147 正是本项目的在册面板数）。也就是说全量重建**跑过**，
//! 房间到面板那张表写成了，成员边却没了。
//!
//! 最合理的解释：那次重建跑在一棵**缺了 7997 的空间树**上。7997 的几何是后来才生成的
//! （现在 45638 条 `inst_relate`、2184 块 PANE 里 2182 块有几何）。面板自己有几何，所以
//! `cal_room_refnos` 返回的不是 `NoGeometry`（那会被跳过、不写），而是 `Computed(空集)`
//! ——拿面板包围盒去树上捞候选时什么都捞不到。紧接着 `save_room_relate` 先清后写，把
//! 这块面板的存量边全删了。现有的空树守卫按 `is_empty()` 判，而那时树里有 1112 的 2039
//! 条，不空，所以挡不住。
//!
//! **这个解释已经被一次独立的重建证实。** 2026-08-06 16:30 前后，7997 的几何与树都到位
//! 之后有人重跑了一次重建，`room_relate` 从 1 条变成 **35296 条**（按成员库号拆：7997
//! 占 35280、1112 占 14、7999 与 8000 各 1）。同一段代码、同一个库，差别只在树里有没有
//! 7997 的包围盒。
//!
//! 顺带坐实了另一件事：**成员是设计图元，不是设备。** 7997 里有 `inst_relate` 的是 BOX /
//! CYLI / EXTR / NCYL / NBOX 这类图元，而 EQUI、VALV、SCTN、BRAN、PIPE 一条都没有——它们
//! 的几何挂在子图元上。所以「R512 里有哪些成员」查出来是一堆图元。要的若是设备粒度，那
//! 是另一个待定问题，不是这里的 bug。
//!
//! 留着这个入口是因为它比「起一次 `run_cli`」轻，而且能按 `AIOS_ROOM_KEYWORD` 收到一间房。
//!
//! ```text
//! # 一、先验证诊断：只重建 /1RX-RM05-R512 一间，对上文档记过的 1466 条
//! $env:AIOS_ROOM_KEYWORD="-RM05-R512"
//! cargo test --features http_api --test room_rebuild_repair -- --ignored --nocapture
//!
//! # 二、再全量修复（不设这个变量就用配置里的 room_key_word）
//! Remove-Item Env:AIOS_ROOM_KEYWORD
//! cargo test --features http_api --test room_rebuild_repair -- --ignored --nocapture
//! ```

use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{SUL_DB, get_db_option};
use aios_database::fast_model::aabb_tree::load_project_tree_verified;
use aios_database::fast_model::room_model::build_room_relations;
use surrealdb::opt::{Config, auth::Root};

async fn connect_live() {
    let endpoint = std::env::var("AIOS_LIVE_WS").unwrap_or_else(|_| "ws://localhost:8009".into());
    let ns = std::env::var("AIOS_LIVE_NS").unwrap_or_else(|_| "1516".into());
    let db = std::env::var("AIOS_LIVE_DB").unwrap_or_else(|_| "AvevaMarineSample".into());
    SUL_DB
        .connect((endpoint, Config::default().ast_payload()))
        .with_capacity(1000)
        .await
        .expect("connect live");
    SUL_DB.use_ns(&ns).use_db(&db).await.expect("use ns/db");
    SUL_DB
        .signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
}

/// 一张表的行数。
///
/// `SELECT VALUE count() FROM t GROUP ALL` 交出来的是 `{ count: N }` 对象而不是裸整数，
/// 直接按整数反序列化会炸；套一层子查询把 `c` 投影出来才拿得到值。
async fn count_of(table: &str) -> i64 {
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE c FROM (SELECT count() AS c FROM {table} GROUP ALL);"
        ))
        .await
        .expect("count query")
        .check()
        .expect("valid count query");
    let rows: Vec<i64> = response.take(0).expect("decode count");
    rows.into_iter().next().unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: 按面板先清后写，重建真实项目库的房间归属"]
async fn rebuild_room_membership_on_the_live_project() {
    connect_live().await;

    let mut db_option = get_db_option().clone();
    match std::env::var("AIOS_ROOM_KEYWORD") {
        Ok(keyword) => {
            println!("[repair] 只重建命中 {keyword} 的房间");
            db_option.room_key_word = Some(vec![keyword]);
        }
        Err(_) => println!(
            "[repair] 用配置里的 room_key_word：{:?}",
            db_option.get_room_key_word()
        ),
    }

    // 走与 `run_cli` 同一条加载路径：sidecar 的 epoch 与库一致才信树文件，否则从库指针重建。
    load_project_tree_verified()
        .await
        .expect("load spatial tree");
    let tree_entries = GLOBAL_AABB_TREE.read().await.tree.size();
    println!("[repair] 空间树 {tree_entries} 条");
    assert!(
        tree_entries > 0,
        "空间树是空的——重建会被守卫拒跑，这是对的。先修树再来"
    );

    let before_members = count_of("room_relate").await;
    let before_panels = count_of("room_panel_relate").await;
    println!("[repair] 重建前: room_relate={before_members} room_panel_relate={before_panels}");

    // 单块面板失败不该中断整轮，函数内部已按面板聚合；这里与 `lib.rs` 的调用点同口径，
    // 把失败降级成打印，再用前后计数说话。
    if let Err(error) = build_room_relations(&db_option).await {
        println!("[repair] 重建未完全成功（逐面板原因已聚合）: {error:#}");
    }

    let after_members = count_of("room_relate").await;
    let after_panels = count_of("room_panel_relate").await;
    println!(
        "[repair] 重建后: room_relate={after_members}（{:+}）room_panel_relate={after_panels}",
        after_members - before_members
    );

    // 判据是「算出来了」而不是「变多了」：重建幂等，在已经修好的库上再跑一次前后相等，
    // 那是正常的。真正的失败态是算完之后仍然是空——那就是当初把 1466 条清掉的那一幕。
    assert!(
        after_members > 0,
        "重建之后成员边仍然是 0——先查空间树里有没有本项目的包围盒，\
         以及这些面板的 .mesh 在不在盘上"
    );
}
