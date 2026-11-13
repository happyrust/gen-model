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
        // `fn::room_code` 的返回形状随构建口径不同，两边都必须取到**同一条最强边**
        // 的 R301，只是拼不拼房间码前缀不一样：
        // - hd 版经 `fn::single_room_code` 读面板属主的名字，拼出 `1RX-R301`；
        // - hh 版直接读 `fn::room_num_of`（并带 `$uda_room` 标记），只回 `R301`。
        // 这个用例钉的是「取哪条边」，不是「拼不拼前缀」，所以按口径分支断言。
        #[cfg(feature = "project_hd")]
        let expected_code = "1RX-R301";
        #[cfg(not(feature = "project_hd"))]
        let expected_code = "R301";
        assert_eq!(
            first_code(&code_raw).as_deref(),
            Some(expected_code),
            "fn::room_code 同样取最强边（口径期望 {expected_code}），实际: {code_raw:?}"
        );
    }

    /// 设备、BRAN、区域、支吊架的房间按子孙件投票，不按存储序取首个。
    /// 每件一票投给自己的最强边；同票依次比 inside_count 合计、center_dist 合计、房间码。
    #[cfg(feature = "project_hd")]
    #[tokio::test]
    async fn owners_take_the_room_most_members_vote_for() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql_room_vote")
            .await
            .expect("select fixture db");
        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        // 房间面板：R201 有两块面板（5_11、5_12），用来钉「同一间房的票要合并」。
        let mut fixture = String::from(
            "CREATE pe:⟨5_100⟩ SET noun = 'FLOOR', name = '/1RX-RM01';
             CREATE pe:⟨5_10⟩ SET noun = 'SBFR', name = '/1RX-RM01-R201-VOLU', owner = pe:⟨5_100⟩;
             CREATE pe:⟨5_20⟩ SET noun = 'SBFR', name = '/1RX-RM01-R301-VOLU', owner = pe:⟨5_100⟩;
             CREATE pe:⟨5_30⟩ SET noun = 'SBFR', name = '/1RX-RM01-R401-VOLU', owner = pe:⟨5_100⟩;
             CREATE pe:⟨5_11⟩ SET noun = 'PANE', name = '', owner = pe:⟨5_10⟩;
             CREATE pe:⟨5_12⟩ SET noun = 'PANE', name = '', owner = pe:⟨5_10⟩;
             CREATE pe:⟨5_21⟩ SET noun = 'PANE', name = '', owner = pe:⟨5_20⟩;
             CREATE pe:⟨5_31⟩ SET noun = 'PANE', name = '', owner = pe:⟨5_30⟩;
             CREATE pe:⟨5_1000⟩ SET noun = 'EQUI', name = '/E1';
             CREATE pe:⟨5_2000⟩ SET noun = 'EQUI', name = '/E2';
             CREATE pe:⟨5_3000⟩ SET noun = 'STRU', name = '/S3';
             CREATE pe:⟨5_4000⟩ SET noun = 'REST', name = '/R4';
             CREATE pe:⟨5_5000⟩ SET noun = 'EQUI', name = '/E5';
             CREATE pe:⟨5_5100⟩ SET noun = 'SUBE', name = '/E5/SUB';
             RELATE pe:⟨5_5100⟩->pe_owner->pe:⟨5_5000⟩;
             CREATE pe:⟨5_6000⟩ SET noun = 'BRAN', name = '/P6/B1';
             CREATE pe:⟨5_7000⟩ SET noun = 'ZONE', name = '/Z7';
             CREATE pe:⟨5_7100⟩ SET noun = 'PIPE', name = '/P7';
             CREATE pe:⟨5_7110⟩ SET noun = 'BRAN', name = '/P7/B1';
             RELATE pe:⟨5_7100⟩->pe_owner->pe:⟨5_7000⟩;
             RELATE pe:⟨5_7110⟩->pe_owner->pe:⟨5_7100⟩;
             CREATE pe:⟨5_8000⟩ SET noun = 'SUPPO', name = '/H8';
             CREATE pe:⟨5_8100⟩ SET noun = 'FRMW', name = '/H8/F';
             CREATE pe:⟨5_8110⟩ SET noun = 'SBFR', name = '/H8/F/S';
             RELATE pe:⟨5_8100⟩->pe_owner->pe:⟨5_8000⟩;
             RELATE pe:⟨5_8110⟩->pe_owner->pe:⟨5_8100⟩;
             CREATE pe:⟨5_9000⟩ SET noun = 'EQUI', name = '/E9';
             CREATE pe:⟨5_9001⟩ SET noun = 'BOX', name = '';
             RELATE pe:⟨5_9001⟩->pe_owner->pe:⟨5_9000⟩;
             CREATE pe:⟨5_9500⟩ SET noun = 'EQUI', name = '/E95';\n",
        );
        // (属主, 成员, 面板, 房间号, inside_count, center_dist)
        let members: &[(&str, &str, &str, &str, u8, f32)] = &[
            // 多数决：R201 两票，压过单件最强的 R301。
            ("5_1000", "5_1001", "5_21", "R301", 8, 10.0),
            ("5_1000", "5_1002", "5_11", "R201", 8, 900.0),
            ("5_1000", "5_1003", "5_11", "R201", 4, 950.0),
            // 同票比 inside_count 合计：R301(8) 胜 R201(6)。
            ("5_2000", "5_2001", "5_21", "R301", 8, 900.0),
            ("5_2000", "5_2002", "5_11", "R201", 6, 100.0),
            // 再比 center_dist 合计：R201(200) 胜 R301(500)。
            ("5_3000", "5_3001", "5_21", "R301", 8, 500.0),
            ("5_3000", "5_3002", "5_11", "R201", 8, 200.0),
            // 全平比房间码：1RX-R201 在前。
            ("5_4000", "5_4001", "5_21", "R301", 8, 300.0),
            ("5_4000", "5_4002", "5_11", "R201", 8, 300.0),
            // 孙件也投票：SUBE 下两件在 R401，直接子件一件在 R201。
            ("5_5000", "5_5001", "5_11", "R201", 8, 10.0),
            ("5_5100", "5_5101", "5_31", "R401", 8, 900.0),
            ("5_5100", "5_5102", "5_31", "R401", 8, 900.0),
            // BRAN 回名次表：R401 两票；R301 与 R201 一票同 inside，比 center_dist。
            ("5_6000", "5_6001", "5_11", "R201", 8, 500.0),
            ("5_6000", "5_6002", "5_31", "R401", 8, 900.0),
            ("5_6000", "5_6003", "5_31", "R401", 8, 900.0),
            ("5_6000", "5_6004", "5_21", "R301", 8, 100.0),
            // 区域取曾孙件：R301 两票胜 R201 一票。
            ("5_7110", "5_7111", "5_11", "R201", 8, 10.0),
            ("5_7110", "5_7112", "5_21", "R301", 8, 900.0),
            ("5_7110", "5_7113", "5_21", "R301", 8, 900.0),
            // 支吊架同样取曾孙件。
            ("5_8110", "5_8111", "5_31", "R401", 8, 10.0),
            // 同一间房的两块面板合票：R201 两票（各一块面板）胜 R301 一票。
            ("5_9500", "5_9501", "5_11", "R201", 4, 900.0),
            ("5_9500", "5_9502", "5_12", "R201", 4, 900.0),
            ("5_9500", "5_9503", "5_21", "R301", 8, 1.0),
        ];
        for (owner, member, panel, room_num, inside, dist) in members {
            fixture.push_str(&format!(
                "CREATE pe:⟨{member}⟩ SET noun = 'BOX', name = '';
                 RELATE pe:⟨{member}⟩->pe_owner->pe:⟨{owner}⟩;
                 RELATE pe:⟨{panel}⟩->room_relate->pe:⟨{member}⟩
                     SET room_num = '{room_num}', inside_count = {inside}, center_dist = {dist};\n"
            ));
        }
        db.query(fixture)
            .await
            .expect("fixture reaches db")
            .check()
            .expect("fixture rows created");

        let owners = [
            "5_1000", "5_2000", "5_3000", "5_4000", "5_5000", "5_7000", "5_8000", "5_9000",
            "5_9500",
        ];
        let sql = owners
            .iter()
            .map(|key| format!("RETURN fn::room_code(pe:⟨{key}⟩);"))
            .chain(std::iter::once(
                "RETURN fn::room_code(pe:⟨5_6000⟩);".to_string(),
            ))
            .collect::<Vec<_>>()
            .join("\n");
        let mut response = db
            .query(sql)
            .await
            .expect("room vote probes reach db")
            .check()
            .expect("room vote probes execute");
        let mut codes = Vec::new();
        for index in 0..owners.len() {
            let raw: Option<serde_json::Value> = response.take(index).expect("decode room code");
            codes.push(first_code(&raw));
        }
        let bran: Vec<String> = response.take(owners.len()).expect("decode bran room codes");

        let expected = [
            Some("1RX-R201"),
            Some("1RX-R301"),
            Some("1RX-R201"),
            Some("1RX-R201"),
            Some("1RX-R401"),
            Some("1RX-R301"),
            Some("1RX-R401"),
            None,
            Some("1RX-R201"),
        ];
        for ((owner, code), want) in owners.iter().zip(&codes).zip(expected) {
            assert_eq!(code.as_deref(), want, "pe:{owner} 的房间码");
        }
        assert_eq!(
            bran,
            vec!["1RX-R401", "1RX-R301", "1RX-R201"],
            "BRAN 的房间按名次排"
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
        let no_pos: Vec<Vec<f32>> = response
            .take(4)
            .expect("decode get_world_pos without geometry");
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

    /// 跨两房构件的 `room_height` / `room_top_height` 必须是**确定性**顺序。
    ///
    /// 2026-10-30 真库实测：`pe:15516_104` 回 `[-280.0, -20.0]`，而同样落在这两间房的
    /// `pe:15516_15` / `pe:15516_4` 回 `[-20.0, -280.0]`。原因是旧实现用
    /// `array::distinct(...)` 收集面板标高，而 `array::distinct` **不排序**——只按元素
    /// 第一次出现的顺序去重。消费方 `fn::room_height(id)[0]`（core
    /// `aios_mgr.rs:91` 的 `query_own_room_panel_elevations`）因此取到哪间房全凭运气；
    /// bottom 与 top 又各自独立 distinct，顺序还可能不同步，拼出根本不存在的净高。
    ///
    /// 这里钉两件事：（1）多房间时数组顺序由面板记录 id 决定，与写入顺序无关；
    /// （2）`bottom[i]` 与 `top[i]` 来自同一块面板（净高配对）。
    #[tokio::test]
    async fn multi_room_elevations_are_ordered_by_panel_not_by_storage() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql_multi_room_order")
            .await
            .expect("select fixture db");
        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        // 两块面板：pe:⟨7_1⟩ 底 -20 / 顶 2705（低房间），pe:⟨7_2⟩ 底 -280 / 顶 3500。
        // 边的写入顺序刻意让「先写 id 大的面板（7_2）」，与记录 id 序相反：
        // 若 `fn::room_panels_of` 退化成按存储序，结果会是 [7_2, 7_1]。
        //
        // 直接测 `fn::room_panels_of` 而不是 `room_height`：后者要先按
        // `$pe.refno.TYPE` 选分支，而 mem 引擎上 `CREATE` 出来的 `refno` 表记录
        // 读不到 `TYPE`（真库 rocksdb 上正常），会让测试落进 else 分支而测不到多值。
        // 排序逻辑本体在 room_panels_of 里，room_height/top_height 只是把它的输出
        // 各自映射一次，所以这里钉住 room_panels_of 就钉住了顺序契约。
        db.query(
            "CREATE pe:⟨7_1⟩ SET noun = 'PANE', name = '';
             CREATE pe:⟨7_2⟩ SET noun = 'PANE', name = '';
             CREATE pe:⟨7_301⟩ SET noun = 'TUBI', refno = TUBI:⟨7_301⟩;
             CREATE pe:⟨7_302⟩ SET noun = 'TUBI', refno = TUBI:⟨7_302⟩;
             CREATE inst_relate:⟨7_1⟩ SET in = pe:⟨7_1⟩,
                 aabb_d = { mins: [0.0, 0.0, -20.0], maxs: [6000.0, 4000.0, 2705.0] };
             CREATE inst_relate:⟨7_2⟩ SET in = pe:⟨7_2⟩,
                 aabb_d = { mins: [0.0, 0.0, -280.0], maxs: [6000.0, 4000.0, 3500.0] };
             -- 先写 id 大的面板（pe:7_2，高房间）挂到 7_302，再写 id 小的
             -- （pe:7_1，低房间）挂到 7_301
             RELATE pe:⟨7_2⟩->room_relate->pe:⟨7_302⟩ SET room_num = 'R201', inside_count = 8, center_dist = 2053.8074;
             RELATE pe:⟨7_1⟩->room_relate->pe:⟨7_301⟩ SET room_num = 'R301', inside_count = 8, center_dist = 1081.1567;",
        )
        .await
        .expect("fixture reaches db")
        .check()
        .expect("fixture rows created");

        // `fn::room_panels_of` 回的是 record 数组，SDK 解成 `Thing` 而非 String，
        // 用 `type::string` 在库侧转成字符串再解码。
        let panels: Vec<String> = db
            .query("RETURN array::map(fn::room_panels_of([pe:⟨7_301⟩, pe:⟨7_302⟩]), |$p| type::string($p));")
            .await
            .expect("panel names reach db")
            .check()
            .expect("panel names execute")
            .take(0)
            .expect("decode panel names");
        assert_eq!(
            panels,
            vec!["pe:7_1".to_string(), "pe:7_2".to_string()],
            "面板按记录 id 升序，与边的写入顺序无关（写入是 7_2 在前）"
        );

        // 配对校验：把面板按 room_panels_of 的顺序映射成标高，bottom[i] 与 top[i]
        // 必须来自同一块面板，净高必须是真实房间高度（2725 / 3780），
        // 不能出现跨面板拼出来的假值。
        let mut response = db
            .query(
                "RETURN array::map(fn::room_panels_of([pe:⟨7_301⟩, pe:⟨7_302⟩]),
                    |$p| type::thing('inst_relate', record::id($p)).aabb_d.mins[2]);
                 RETURN array::map(fn::room_panels_of([pe:⟨7_301⟩, pe:⟨7_302⟩]),
                    |$p| type::thing('inst_relate', record::id($p)).aabb_d.maxs[2]);",
            )
            .await
            .expect("elevation probes reach db")
            .check()
            .expect("elevation probes execute");
        let bottom: Vec<f32> = response.take(0).expect("decode bottom");
        let top: Vec<f32> = response.take(1).expect("decode top");
        assert_eq!(
            bottom,
            vec![-20.0, -280.0],
            "底标高顺序跟随面板 id 序：7_1(-20) 在前，7_2(-280) 在后"
        );
        assert_eq!(
            top,
            vec![2705.0, 3500.0],
            "顶标高与底标高同序，bottom[i] 与 top[i] 是同一块面板"
        );
        for (i, (b, t)) in bottom.iter().zip(top.iter()).enumerate() {
            let clear = t - b;
            assert!(
                (clear - 2725.0).abs() < 0.01 || (clear - 3780.0).abs() < 0.01,
                "第 {i} 对净高 {clear} 不是任一房间的真实净高（2725 / 3780），说明 bottom 与 top 配错了面板"
            );
        }
    }

    /// 已删除构件（`pe.deleted = true`）的 `room_relate` 边不算房间归属。
    ///
    /// 2026-10-30 真库实测：库里 164 行 `deleted = true`，其中 196 条 `room_relate`
    /// 边的 `out` 端就是这些已删件（BEND 44 / FTUB 152），且它们都没有 `inst_relate`
    /// 几何。旧 `fn::room_relate_of` 不过滤 `deleted`，这些边会以「房间成员」身份被
    /// 读到，成员数虚高、标高取不到。过滤条件写成 `out.deleted != true` 而不是
    /// `!out.deleted`：大量老边的 `deleted` 是 NONE，`!NONE` 在 mem 引擎上不是 true。
    #[tokio::test]
    async fn deleted_members_do_not_count_as_room_occupancy() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql_deleted_member")
            .await
            .expect("select fixture db");
        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        db.query(
            "CREATE pe:⟨6_1⟩ SET noun = 'PANE', name = '';
             -- 活件：只有 deleted = false 的老记录形态（真库里多数行就是 false）
             CREATE pe:⟨6_100⟩ SET noun = 'TUBI', refno = TUBI:⟨6_100⟩, deleted = false;
             -- 已删件：deleted = true，且没有 inst_relate 几何
             CREATE pe:⟨6_200⟩ SET noun = 'BEND', refno = BEND:⟨6_200⟩, deleted = true;
             -- 老边形态：deleted 字段压根不存在（NONE）
             CREATE pe:⟨6_300⟩ SET noun = 'TUBI', refno = TUBI:⟨6_300⟩;
             RELATE pe:⟨6_1⟩->room_relate->pe:⟨6_100⟩ SET room_num = 'R301', inside_count = 8, center_dist = 1081.1567;
             RELATE pe:⟨6_1⟩->room_relate->pe:⟨6_200⟩ SET room_num = 'R301', inside_count = 8, center_dist = 1100.0;
             RELATE pe:⟨6_1⟩->room_relate->pe:⟨6_300⟩ SET room_num = 'R301', inside_count = 8, center_dist = 1120.0;",
        )
        .await
        .expect("fixture reaches db")
        .check()
        .expect("fixture rows created");

        let mut response = db
            .query(
                "RETURN fn::room_relate_of(pe:⟨6_100⟩).room_num;
                 RETURN fn::room_relate_of(pe:⟨6_200⟩);
                 RETURN fn::room_num_of(pe:⟨6_200⟩);
                 RETURN fn::room_relate_of(pe:⟨6_300⟩).room_num;",
            )
            .await
            .expect("deleted probes reach db")
            .check()
            .expect("deleted probes execute");
        let alive: Option<String> = response.take(0).expect("decode alive member");
        let deleted_edge: Option<serde_json::Value> =
            response.take(1).expect("decode deleted edge");
        let deleted_num: Option<String> = response.take(2).expect("decode deleted room_num");
        let legacy: Option<String> = response.take(3).expect("decode legacy member");

        assert_eq!(alive.as_deref(), Some("R301"), "活件照常取到房间");
        assert!(
            deleted_edge.is_none(),
            "已删件（deleted = true）不再有房间归属，实际: {deleted_edge:?}"
        );
        assert!(
            deleted_num.is_none(),
            "已删件的 room_num_of 也必须是 NONE，实际: {deleted_num:?}"
        );
        assert_eq!(
            legacy.as_deref(),
            Some("R301"),
            "deleted 字段为 NONE 的老记录不能被一起滤掉（`!deleted` 会误杀）"
        );
    }
}
