//! 墙体 JLDATU/PLDATU/FIXING 基准帧回归探针（E3D RVM oracle 对拍）。
//!
//! 根因（2026-08-29，.context/会话-2026-08-29.md 任务 2）：PLDATU.POSL="OBOW" 在墙
//! Profile 的 PTSS（/STD-WALLS-SINGLE-PTSS）里没有 PTCA——OBOW/IBOW 是 E3D 的
//! **墙体隐式 pline**，目录里不落地。`query_pline` 查无此键时兜底 `plax=+X`，
//! 令 JLDATU⊗PLDATU 复合帧绕 SPINE 切向反 180°（X 轴该朝世界下方，算成了上方），
//! FIXING 局部 POS.x=-ZDIS 被映到墙底**下方** ZDIS 处（应为上方）——/W-HOLE-W 的
//! NLCY 负圆柱因此不与墙体相交，布尔白减，WALL 开孔全部丢失（WF04 WALL 6 现象）。
//!
//! 修复：aios-core `query_pline` 对 WALL/STWALL 宿主合成隐式 pline
//! （OBOW→plax=-X 外侧面 / IBOW→plax=+X 内侧面，pt=0）。
//!
//! oracle 出处：`test_data/rvm/1RS-WF03-W-C-RR001.rvm.json` 节点
//! "FIXING 1 of PLDATUM 1 of JLDATUM /1RS03TT3502T" 的世界变换
//! translation=(-15559.96, -2743.64, 2150.0)（E3D 导出，毫米）。
//! 反解验证：plax=-X 时复合链算得 (-15560.1, -2743.64, 2150.0)，误差 0.15mm。
//!
//! ```text
//! cargo test --test wall_fixing_datum_probe -- --ignored --nocapture
//! ```
//! 前置：SurrealDB ws://localhost:8009（ns 1516 / AvevaMarineSample）在线，
//! 可用 AIOS_LIVE_WS / AIOS_LIVE_NS / AIOS_LIVE_DB 覆盖。

use aios_core::{RefnoEnum, SUL_DB};
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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: 依赖 8009 实库（AMS 样例工程），对拍 E3D RVM oracle"]
async fn wall_fixing_datum_matches_rvm_oracle() {
    connect_live().await;

    // WF03 环形墙 17496_105912（墙底 z=-400）：JLDATU /1RS03TT3502T ZDIS=2550
    // → PLDATU(POSL=OBOW, YDIR=+Z) → FIXING 17496_137183。E3D oracle 见文件头。
    let refno: RefnoEnum = "17496_137183".into();
    let t = aios_core::get_world_transform(refno)
        .await
        .expect("query WF03 fixing transform")
        .expect("WF03 fixing transform exists");
    let got = t.translation;
    let expect = [-15559.96_f32, -2743.64, 2150.0];
    println!("WF03 FIXING 17496_137183 world = {got:?}, oracle = {expect:?}");
    for (axis, e) in ["x", "y", "z"].iter().zip(expect) {
        let g = match *axis {
            "x" => got.x,
            "y" => got.y,
            _ => got.z,
        };
        assert!(
            (g - e).abs() < 5.0,
            "WF03 FIXING 世界位 {axis}={g} 偏离 E3D oracle {e}（>5mm）"
        );
    }

    // WF04 WALL 6 = 17496_106253（墙底 z=3600，高 3900）：JLDATU ZDIS=1400
    // → FIXING 17496_127680。反号 bug 下 z≈2200（墙外，孔丢失）；正确值 3600+1400=5000。
    let refno6: RefnoEnum = "17496_127680".into();
    let t6 = aios_core::get_world_transform(refno6)
        .await
        .expect("query WALL 6 fixing transform")
        .expect("WALL 6 fixing transform exists");
    println!("WALL6 FIXING 17496_127680 world = {:?}", t6.translation);
    assert!(
        (t6.translation.z - 5000.0).abs() < 5.0,
        "WALL 6 FIXING 世界 z={} 应为 5000（墙 z 范围 3600..7500 内）",
        t6.translation.z
    );
}
