//! 内置 SurrealDB 函数集：`resource/surreal/` 的**编译期快照**。
//!
//! 部署包的 `resource/surreal/` 是随包拷贝的，会漂移：现场 test-worklspace 的
//! bin 停在 2025-06——`common.surql` 差 3KB，活库（1516/AvevaMarineSample）因此缺
//! `fn::room_relate_of` / `fn::room_num_of`，`fn::room_code` 一直是旧语义。更糟的是旧 exe 每次
//! 启动都把旧函数体再灌一遍（`define_common_functions` 无条件加载 CWD 脚本），
//! 手工灌一次新的、下次重启又被打回去。
//!
//! `gen_root.surql`（`gen_root` 表的索引与 12 个 `fn::gen_root_*` 巡检函数）随 `gen_root` 表退役
//! （ADR-079）从快照里摘掉；存量库里的表与函数由 `retire_publish_run_tables` 一次性删。
//!
//! 这里把仓库的脚本按 `include_str!` 冻进二进制，启动序列在磁盘加载**之后**再灌
//! 一遍内置版：站点自有的额外脚本继续生效，但与仓库同名的函数以内置版为准，
//! 任何旧运行环境都会被抬到当前函数集。代价是「改磁盘 surql 文件热修函数」不再
//! 生效——函数随代码版本走，改函数请改仓库并重新发版。
//!
//! 执行语义与 rs-core 的 `define_common_functions_on` 保持一致：整文件 `query`，
//! 语句级错误吞掉（脚本靠 `REMOVE FUNCTION` + `DEFINE FUNCTION` 自愈，全新库上
//! REMOVE 必然报错；见 `docs/2026-08-05_fork-surreal-compat-findings.md`）。只有
//! 送达不了持久层的传输错误才上抛。

use anyhow::Context;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

/// hd 版 `fn::room_code`（读 `fn::room_relate_of` 的排序边语义，ADR-010 §5）。
/// 单独提出来：目录序里 `_hh` 排在它后面覆盖同名函数，`project_hd` 构建要重放它。
const HD_ROOM_CODE: &str = include_str!("../../resource/surreal/fn_query_room_code.surql");

/// `resource/surreal/` 的编译期快照，按文件名升序——与
/// `define_common_functions_on` 对目录 `sort()` 后的加载顺序一致，保证 hh 对
/// room_code 的覆盖关系不因来源（磁盘/内置）而异。
pub const EMBEDDED_SURQL: &[(&str, &str)] = &[
    (
        "common.surql",
        include_str!("../../resource/surreal/common.surql"),
    ),
    ("fn_query_room_code.surql", HD_ROOM_CODE),
    (
        "fn_query_room_code_hh.surql",
        include_str!("../../resource/surreal/fn_query_room_code_hh.surql"),
    ),
    (
        "fn_query_room_height.surql",
        include_str!("../../resource/surreal/fn_query_room_height.surql"),
    ),
    (
        "get_room_nodes.surql",
        include_str!("../../resource/surreal/get_room_nodes.surql"),
    ),
    (
        "gy_common.surql",
        include_str!("../../resource/surreal/gy_common.surql"),
    ),
    (
        "init_status.surql",
        include_str!("../../resource/surreal/init_status.surql"),
    ),
    (
        "material_common.surql",
        include_str!("../../resource/surreal/material_common.surql"),
    ),
];

/// 「新环境适配」的核验面：这几个函数只存在于 2026 版脚本里，旧部署的磁盘文件
/// 不含它们——内置灌入之后还缺，说明引擎拒绝了新脚本（语句错误被按惯例吞掉），
/// 房间语义会退化。收口硬前置（`fn::anc_u64` 等）不在这里：
/// 那是 `selfcheck_surreal_functions` 的硬门，缺了直接拒绝启动。
/// （`gen_root_of` / `gen_roots_todo` 随 `gen_root.surql` 退役，ADR-079。）
const MARQUEE_FUNCTIONS: &[&str] = &["room_relate_of", "room_num_of"];

/// 把内置函数集灌进指定库。磁盘脚本加载**之后**调用，同名函数以内置版收尾。
pub async fn define_embedded_functions_on(db: &Surreal<Any>) -> anyhow::Result<()> {
    for (name, text) in EMBEDDED_SURQL {
        db.query(*text)
            .await
            .with_context(|| format!("灌入内置 {name} 失败（未送达持久层）"))?;
    }
    // D11（ADR-010）：目录序里 hh 排在 hd 之后，同名 fn::room_code 被 hh 版覆盖。
    // project_hd 构建加载完成后重放 hd 版，把覆盖再覆盖回来；project_hh 构建
    // 无需处理——hh 本就是最后生效的那份。
    #[cfg(feature = "project_hd")]
    db.query(HD_ROOM_CODE)
        .await
        .context("重放内置 hd 版 fn::room_code 失败（未送达持久层）")?;
    println!(
        "已灌入内置 surreal 函数集（{} 个文件，编译期快照，覆盖同名旧版）",
        EMBEDDED_SURQL.len()
    );
    Ok(())
}

/// [`define_embedded_functions_on`] 的进程主库（`SUL_DB`）入口。
pub async fn define_embedded_functions() -> anyhow::Result<()> {
    define_embedded_functions_on(&aios_core::SUL_DB).await
}

/// 内置灌入后核验：返回 [`MARQUEE_FUNCTIONS`] 里仍不存在的那些。空即健康。
pub async fn missing_embedded_functions_on(db: &Surreal<Any>) -> anyhow::Result<Vec<String>> {
    let mut response = db
        .query("INFO FOR DB;")
        .await
        .context("读取数据库函数清单失败")?
        .check()
        .context("INFO FOR DB 语句失败")?;
    let info: Option<serde_json::Value> = response.take(0).context("解码 INFO FOR DB 失败")?;
    let functions = info
        .as_ref()
        .and_then(|v| v.get("functions"))
        .cloned()
        .unwrap_or_default();
    Ok(MARQUEE_FUNCTIONS
        .iter()
        .filter(|name| functions.get(**name).is_none())
        .map(|name| (*name).to_string())
        .collect())
}

/// [`missing_embedded_functions_on`] 的进程主库入口。
pub async fn missing_embedded_functions() -> anyhow::Result<Vec<String>> {
    missing_embedded_functions_on(&aios_core::SUL_DB).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fn::room_code` 的返回值解成「首个房间码」。
    ///
    /// 契约是**数组**（消费方一律 `fn::room_code($pe)[0]`），rocksdb 引擎如约回
    /// `["1RX-R144"]`；但 mem 引擎会把 `RETURN [$x]` 的单元素数组扁平化成一个裸
    /// 字符串（2026-10-29 实测）。两种形状都接受，空/NONE 归一成 `None`。
    fn first_code(raw: &Option<serde_json::Value>) -> Option<String> {
        match raw {
            Some(serde_json::Value::Array(items)) => items
                .first()
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    }

    /// 快照名单与磁盘目录必须互为镜像：加了第 9 个脚本却忘了登记，这里挂；
    /// 改名/删除则直接 `include_str!` 编译失败。缺一边都意味着「二进制自带
    /// 函数集」这句话不再成立。
    #[test]
    fn the_embedded_set_mirrors_the_resource_directory() {
        let mut on_disk = std::fs::read_dir("resource/surreal")
            .expect("resource/surreal 在仓库根下")
            .map(|entry| entry.expect("读目录项").file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        on_disk.sort();
        let embedded: Vec<String> = EMBEDDED_SURQL
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        assert_eq!(
            embedded, on_disk,
            "内置快照与 resource/surreal 目录不一致：左=内置，右=磁盘"
        );
    }

    /// 老运行环境的现场重演：库里已经站着一个旧语义的 `fn::room_code`（部署 exe
    /// 每次启动都会把它灌回去），内置灌入之后必须（1）核验面的新函数全部就位，
    /// （2）`fn::room_code` 被抬到当前语义——hd 构建读 `fn::room_relate_of` 的
    /// 排序边，hh 构建读 `fn::room_num_of`。
    #[tokio::test]
    async fn an_old_database_is_lifted_to_the_current_function_set() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql")
            .await
            .expect("select fixture db");

        db.query("DEFINE FUNCTION fn::room_code($pe: record) { RETURN NONE; };")
            .await
            .expect("stale define reaches db")
            .check()
            .expect("stale room_code defined");

        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        let missing = missing_embedded_functions_on(&db)
            .await
            .expect("marquee probe runs");
        assert!(missing.is_empty(), "内置灌入后仍缺: {missing:?}");

        let mut response = db
            .query("INFO FOR DB;")
            .await
            .expect("info reaches db")
            .check()
            .expect("info statement");
        let info: Option<serde_json::Value> = response.take(0).expect("decode info");
        let room_code = info
            .as_ref()
            .and_then(|v| v.get("functions"))
            .and_then(|f| f.get("room_code"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(!room_code.is_empty(), "room_code 在内置灌入后必须存在");
        // hd 版经 `fn::single_room_code` → `fn::room_relate_of` 取最强边；
        // hh 版直接读 `fn::room_num_of`，并带 `$uda_room` 标记。
        #[cfg(feature = "project_hd")]
        {
            assert!(
                room_code.contains("single_room_code"),
                "hd 版 room_code 必须经 fn::single_room_code 读 ADR-010 §5 的最强边，实际: {room_code}"
            );
            assert!(
                !room_code.contains("$uda_room"),
                "project_hd 构建的 room_code 不应是 hh 版（$uda_room 是 hh 独有标记），实际: {room_code}"
            );
        }
        #[cfg(not(feature = "project_hd"))]
        {
            assert!(
                room_code.contains("room_num_of"),
                "hh 版 room_code 必须经 fn::room_num_of，实际: {room_code}"
            );
            assert!(
                room_code.contains("$uda_room"),
                "非 hd 构建的 room_code 应是 hh 版，实际: {room_code}"
            );
        }
    }

    /// hd 版 `fn::room_code` 对挂着 `room_relate` 边的构件必须回出「前缀-房间号」。
    /// 2026-09-16 rocksdb 真库上它对每个构件都回 NONE：函数体用 `{ $code = …; }` 给变量
    /// 赋值，SurrealDB 2.x 里块的值是最后一条语句的值、赋值语句的值是 NONE，于是
    /// `$code == NONE → RETURN NONE`。分支改成表达式后这里钉住四字与五字房间号两条路。
    ///
    /// 2026-10-29 起 `fn::room_code` 统一返回**数组**（与 hh 版、与 core 的
    /// `fn::room_code(pe)[0]` 消费方式一致），所以这里按数组解码、取首个元素。
    #[cfg(feature = "project_hd")]
    #[tokio::test]
    async fn hd_room_code_returns_prefix_and_room_number_for_a_member_with_an_edge() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql_room_code")
            .await
            .expect("select fixture db");
        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        db.query(
            "CREATE FLOOR:⟨2_72903⟩ SET TYPE = 'FLOOR';
             CREATE FLOOR:⟨2_72904⟩ SET TYPE = 'FLOOR';
             CREATE FLOOR:⟨2_72905⟩ SET TYPE = 'FLOOR';
             -- 房间码前缀取「面板属主名字的第一段」。层级与真库一致：
             -- 楼层面板(FLOOR) ← SBFR ← 房间面板(PANE)，边的 in 端是房间面板。
             -- 真库样例：room_relate:24381_34862_15516_10 的 in 是 pe:24381_34862（PANE，
             -- owner 是 SBFR pe:24381_34850），out 是构件 pe:15516_10。
             CREATE pe:⟨1_1440⟩ SET noun = 'FLOOR', name = '/1RX-RM01';
             CREATE pe:⟨1_1444⟩ SET noun = 'SBFR', name = '/1RX-RM01-R144-VOLU', owner = pe:⟨1_1440⟩;
             CREATE pe:⟨1_1445⟩ SET noun = 'PANE', name = '', owner = pe:⟨1_1444⟩;
             -- 房间面板：owner 指向 SBFR，single_room_code 会再上一层取楼名。
             CREATE pe:⟨2_72903⟩ SET noun = 'PANE', name = '', owner = pe:⟨1_1444⟩;
             CREATE pe:⟨2_72904⟩ SET noun = 'PANE', name = '', owner = pe:⟨1_1444⟩;
             -- 无房间边的对照件
             CREATE pe:⟨2_72905⟩ SET noun = 'TUBI', name = 'TUBI-1';
             -- 边的方向与真库一致：in = 面板，out = 构件。
             RELATE pe:⟨2_72903⟩->room_relate->pe:⟨1_1445⟩ SET room_num = 'R144', inside_count = 8, center_dist = 2960.4;
             RELATE pe:⟨2_72904⟩->room_relate->pe:⟨1_1445⟩ SET room_num = 'R0144', inside_count = 8, center_dist = 2960.4;",
        )
        .await
        .expect("fixture reaches db")
        .check()
        .expect("fixture rows created");

        // 构件侧取码：PANE 走 room_code 的 else 分支 → [fn::single_room_code($pe)]。
        // 两条边里 R144（center 2960.4）与 R0144（center 2960.4）同强，第三级
        // room_num asc 让 R0144 排前 → 归一化后两者都是 1RX-R144，结果确定。
        let mut response = db
            .query(
                "RETURN fn::room_code(pe:⟨1_1445⟩);
                 RETURN fn::single_room_code(pe:⟨1_1445⟩);
                 RETURN fn::room_code(pe:⟨2_72905⟩);",
            )
            .await
            .expect("room_code reaches db")
            .check()
            .expect("room_code statements");
        // mem 引擎会把 `RETURN [$x]` 的单元素数组扁平化成一个字符串，rocksdb 不会
        // （8009 真库上 `fn::room_code` 回 `["1RX-R144"]`）。两种形状都解，取首个元素。
        let raw: Option<serde_json::Value> = response.take(0).expect("decode room code");
        let code = first_code(&raw);
        let single: Option<String> = response.take(1).expect("decode single_room_code");
        let raw_none: Option<serde_json::Value> =
            response.take(2).expect("decode member without edge");
        assert_eq!(
            code.as_deref(),
            Some("1RX-R144"),
            "房间码：面板属主名前缀 + 房间号（四字 R144 与五字 R0144 归一后同形），实际: {raw:?}"
        );
        assert_eq!(
            single.as_deref(),
            Some("1RX-R144"),
            "五字房间号 R0144 → 去掉第二位；四字 R144 原样"
        );
        assert_eq!(
            first_code(&raw_none),
            None,
            "没有 room_relate 边的构件仍回 NONE，实际: {raw_none:?}"
        );
    }

    /// 跨两房构件：`room_code` / `get_room_number` 必须取 ADR-010 §5 的最强边
    /// （inside_count desc, center_dist asc, room_num asc），不是存储序首条。
    /// 真库 pe:15516_10 的两条边是 R301（inside 8 / center 1081.16）与
    /// R201（inside 8 / center 2053.81），旧取法按 id 序取到 R201。
    #[tokio::test]
    async fn cross_room_member_takes_the_strongest_edge_not_the_first_stored() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql_cross_room")
            .await
            .expect("select fixture db");
        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        db.query(
            "CREATE FLOOR:⟨9_1⟩ SET TYPE = 'FLOOR';
             CREATE FLOOR:⟨9_2⟩ SET TYPE = 'FLOOR';
             CREATE pe:⟨9_100⟩ SET noun = 'FLOOR', name = '/1RX-RM01';
             CREATE pe:⟨9_10⟩ SET noun = 'SBFR', name = '/1RX-RM01-R201-VOLU', owner = pe:⟨9_100⟩;
             CREATE pe:⟨9_20⟩ SET noun = 'SBFR', name = '/1RX-RM01-R301-VOLU', owner = pe:⟨9_100⟩;
             CREATE pe:⟨9_30⟩ SET noun = 'PANE', name = '', owner = pe:⟨9_10⟩;
             -- 两个房间面板（边的 in 端），owner 各指向对应的 SBFR：
             -- 前缀由 SBFR 的上一层楼名给出，房间号由边上的 room_num 给出。
             CREATE pe:⟨9_11⟩ SET noun = 'PANE', name = '', owner = pe:⟨9_10⟩;
             CREATE pe:⟨9_21⟩ SET noun = 'PANE', name = '', owner = pe:⟨9_20⟩;
             CREATE pe:⟨9_1⟩ SET noun = 'FLOOR', refno = FLOOR:⟨9_1⟩;
             CREATE pe:⟨9_2⟩ SET noun = 'FLOOR', refno = FLOOR:⟨9_2⟩;
             -- 先写弱边（center_dist 大），再写强边：存储序与强度相反
             -- 方向与真库一致：in = 面板，out = 构件。
             RELATE pe:⟨9_11⟩->room_relate->pe:⟨9_30⟩ SET room_num = 'R201', inside_count = 8, center_dist = 2053.8074;
             RELATE pe:⟨9_21⟩->room_relate->pe:⟨9_30⟩ SET room_num = 'R301', inside_count = 8, center_dist = 1081.1567;",
        )
        .await
        .expect("fixture reaches db")
        .check()
        .expect("fixture rows created");

        let mut response = db
            .query(
                "RETURN fn::room_relate_of(pe:⟨9_30⟩).room_num;
                 RETURN fn::room_num_of(pe:⟨9_30⟩);
                 RETURN fn::get_room_number(pe:⟨9_30⟩);
                 RETURN fn::room_code(pe:⟨9_30⟩);",
            )
            .await
            .expect("cross-room probes reach db")
            .check()
            .expect("cross-room probes execute");
        let strongest: Option<String> = response.take(0).expect("decode strongest edge");
        let num: Option<String> = response.take(1).expect("decode room_num_of");
        let number: Option<String> = response.take(2).expect("decode get_room_number");
        let code_raw: Option<serde_json::Value> = response.take(3).expect("decode room_code");
        assert_eq!(
            strongest.as_deref(),
            Some("R301"),
            "最强边 = center_dist 更小的 R301（inside_count 相同时比 center_dist）"
        );
        assert_eq!(num.as_deref(), Some("R301"), "fn::room_num_of 走同一条边");
        assert_eq!(
            number.as_deref(),
            Some("R301"),
            "fn::get_room_number 必须走 fn::room_num_of，不能按存储序取首条"
        );
        assert_eq!(
            first_code(&code_raw).as_deref(),
            Some("1RX-R301"),
            "fn::room_code 同样取最强边，实际: {code_raw:?}"
        );
    }

    /// `fn::room_height` / `fn::room_top_height` 必须读到面板的 aabb 真值。
    /// 旧写法 `<-room_relate.in.id->inst_relate.aabb.d.mins[2]` 把 inst_relate 当成
    /// 从 pe 出去的边，而它是普通表（`inst_relate:⟨ref0_ref1⟩`），整条链恒 NONE：
    /// 真库上 `room_height(pe:15516_10)` 回 `[null]`，core 按 `Vec<f32>` 解码失败后
    /// 静默落 0.0（aios_mgr.rs:85-101），所有仪表底标高都是 0。
    #[tokio::test]
    async fn room_height_reads_the_panel_aabb_by_key() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql_room_height")
            .await
            .expect("select fixture db");
        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        db.query(
            "CREATE FLOOR:⟨8_1⟩ SET TYPE = 'FLOOR';
             CREATE pe:⟨8_1⟩ SET noun = 'FLOOR', refno = FLOOR:⟨8_1⟩;
             CREATE pe:⟨8_100⟩ SET noun = 'STRU', refno = STRU:⟨8_100⟩;
             CREATE pe:⟨8_200⟩ SET noun = 'TUBI', refno = TUBI:⟨8_200⟩;
             CREATE pe:⟨8_300⟩ SET noun = 'TUBI', refno = TUBI:⟨8_300⟩;
             CREATE inst_relate:⟨8_1⟩ SET in = pe:⟨8_1⟩,
                 aabb_d = { mins: [0.0, 0.0, -20.0], maxs: [6000.0, 4000.0, 2705.0] },
                 world_trans_d = { translation: [0.0, 0.0, 0.0] };
             CREATE inst_relate:⟨8_100⟩ SET in = pe:⟨8_100⟩,
                 aabb_d = { mins: [10.0, 10.0, 225.0], maxs: [20.0, 20.0, 300.0] },
                 world_trans_d = { translation: [-50.0, 0.0, 262.5] };
             RELATE pe:⟨8_1⟩->room_relate->pe:⟨8_100⟩ SET room_num = 'R301', inside_count = 8, center_dist = 1081.1567;",
        )
        .await
        .expect("fixture reaches db")
        .check()
        .expect("fixture rows created");

        let mut response = db
            .query(
                "RETURN fn::room_height(pe:⟨8_100⟩);
                 RETURN fn::room_top_height(pe:⟨8_100⟩);
                 RETURN fn::room_height(pe:⟨8_200⟩);
                 RETURN fn::get_world_pos(pe:⟨8_100⟩);
                 RETURN fn::get_world_pos(pe:⟨8_300⟩);",
            )
            .await
            .expect("geometry probes reach db")
            .check()
            .expect("geometry probes execute");
        let bottom: Vec<f32> = response.take(0).expect("decode room_height");
        let top: Vec<f32> = response.take(1).expect("decode room_top_height");
        let orphan: Vec<f32> = response.take(2).expect("decode room_height without edge");
        let pos: Vec<Vec<f32>> = response.take(3).expect("decode get_world_pos");
        let no_pos: Vec<Vec<f32>> = response.take(4).expect("decode get_world_pos without geometry");
        assert_eq!(bottom, vec![-20.0], "底标高 = 面板 aabb_d.mins[2]");
        assert_eq!(top, vec![2705.0], "顶标高 = 面板 aabb_d.maxs[2]");
        assert!(
            orphan.is_empty(),
            "没有 room_relate 边的件回空数组（不能混 NONE，否则 Vec<f32> 解码整体失败）"
        );
        assert_eq!(
            pos,
            vec![vec![-50.0, 0.0, 262.5]],
            "get_world_pos 按键读 world_trans_d，形状仍是 [[x,y,z]]"
        );
        assert!(
            no_pos.is_empty() || no_pos == vec![Vec::<f32>::new()],
            "缺几何的件回 [[]]（与旧写法形状一致），不能回 NONE：{no_pos:?}"
        );
    }

}
