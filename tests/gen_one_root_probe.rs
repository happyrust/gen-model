//! 抽一个生成根跑一遍，看「几何 → 包围盒 → 空间树 → 房间成员」这条链是不是真的一路跟出来。
//!
//! 起因：8000 有 1095 条 `inst_relate`，其中 1093 条既没有 `aabb` 指针、`out->geo_relate`
//! 也是 **0 条**——实例行建了、`world_trans` 有了，实例下面却挂着空气。也就是说 8000 的
//! 模型生成只做了一半。1112 那 2000 条同理。
//!
//! 这决定了修法：手工补指针是空跑（`update_inst_relate_aabbs_by_refnos` 算新值靠的就是
//! `out->geo_relate` 那侧的 `aabb`，没有边就无从算起，而「算不出就用行内旧指针」这条兜底
//! 对它们也不成立——它们本来就没指针）。真正要做的是**把模型生成补上**，而库里
//! `model_update_pending` 已经排着 2967 条 8000 的 `regen_root` 就是这件事。
//!
//! 这条探针只动一个根，用来在铺开之前确认那条链确实通。默认靶子是 8000 的一台设备；
//! 换靶子设 `AIOS_PROBE_ROOT`。
//!
//! ```text
//! cargo test --features http_api --test gen_one_root_probe -- --ignored --nocapture
//! ```

use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{RefnoEnum, SUL_DB};
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::aabb_tree::load_project_tree_verified;
use surrealdb::opt::{Config, auth::Root};

/// `/1-LNR-Q005-PJ`，8000 里的一台设备。EQUI 本身就是交付单元类型，自成一个生成根。
const DEFAULT_ROOT: &str = "24384/24776";

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

async fn scalar(sql: &str) -> i64 {
    let mut response = SUL_DB
        .query(sql)
        .await
        .expect("probe query")
        .check()
        .expect("valid probe query");
    let rows: Vec<i64> = response.take(0).expect("decode probe");
    rows.into_iter().next().unwrap_or(0)
}

/// 这个根的子树在链路各段各留下多少东西。
async fn chain_counts(root: RefnoEnum) -> (i64, i64, i64) {
    let key = root.to_pe_key();
    // 子树用 pe_owner 反向闭包取；根自己也算进去。
    let subtree = format!(
        "(SELECT VALUE id FROM pe WHERE id = {key} OR id IN \
         (SELECT VALUE in FROM pe_owner WHERE out = {key}))"
    );
    let insts = scalar(&format!(
        "SELECT VALUE c FROM (SELECT count() AS c FROM inst_relate WHERE in IN {subtree} GROUP ALL);"
    ))
    .await;
    let with_geo = scalar(&format!(
        "SELECT VALUE c FROM (SELECT count() AS c FROM inst_relate \
         WHERE in IN {subtree} AND count(out->geo_relate) > 0 GROUP ALL);"
    ))
    .await;
    let with_aabb = scalar(&format!(
        "SELECT VALUE c FROM (SELECT count() AS c FROM inst_relate \
         WHERE in IN {subtree} AND aabb.d != none GROUP ALL);"
    ))
    .await;
    (insts, with_geo, with_aabb)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: 对真实项目库生成一个根（写模型实例、几何与 mesh 文件）"]
async fn generating_one_root_fills_geometry_aabb_and_tree() {
    connect_live().await;

    let root_refno = std::env::var("AIOS_PROBE_ROOT").unwrap_or_else(|_| DEFAULT_ROOT.into());
    let root = RefnoEnum::from(root_refno.as_str());
    println!("[probe] 靶子生成根 {root_refno}");

    load_project_tree_verified()
        .await
        .expect("load spatial tree");
    let tree_before = GLOBAL_AABB_TREE.read().await.tree.size();
    let (insts_before, geo_before, aabb_before) = chain_counts(root).await;
    println!(
        "[probe] 生成前: inst_relate={insts_before} 其中有几何={geo_before} 有包围盒={aabb_before}，空间树={tree_before}"
    );

    let mgr = AiosDBManager::init_form_config()
        .await
        .expect("init db manager");
    let result = mgr
        .ensure_model_generated(root, false)
        .await
        .unwrap_or_else(|error| panic!("生成 {root_refno} 失败: {error:#}"));
    println!(
        "[probe] 生成结果: root={} status={:?} 可渲染={} 已写入={}",
        result.generation_root,
        result.status,
        result.model_instance_count,
        result.generated_instance_count
    );

    let tree_after = GLOBAL_AABB_TREE.read().await.tree.size();
    let (insts_after, geo_after, aabb_after) = chain_counts(root).await;
    println!(
        "[probe] 生成后: inst_relate={insts_after} 其中有几何={geo_after} 有包围盒={aabb_after}，空间树={tree_after}"
    );

    // 链路的判据不是「实例变多了」——`ensure` 幂等，实例行本来就在。要看的是那两段断掉的
    // 环节接没接上：实例下面挂上几何，以及包围盒指针写出来。
    assert!(
        geo_after > geo_before,
        "生成之后实例仍然没有 geo_relate（{geo_before} -> {geo_after}）——\
         那 2967 条 regen_root 消化掉也补不出包围盒，得回头查生成路径本身"
    );
    assert!(
        aabb_after > aabb_before,
        "几何有了但包围盒指针仍然没有（{aabb_before} -> {aabb_after}）——\
         断在 update_inst_relate_aabbs_by_refnos 那一段"
    );
}
