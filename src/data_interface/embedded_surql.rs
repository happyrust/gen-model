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
        #[cfg(feature = "project_hd")]
        assert!(
            room_code.contains("room_relate_of"),
            "project_hd 构建的 room_code 必须是读排序边的 hd 版，实际: {room_code}"
        );
        #[cfg(not(feature = "project_hd"))]
        assert!(
            room_code.contains("room_num_of"),
            "非 hd 构建的 room_code 应是 hh 版，实际: {room_code}"
        );
    }

    /// hd 版 `fn::room_code` 对挂着 `room_relate` 边的构件必须回出「前缀-房间号」。
    /// 2026-09-16 rocksdb 真库上它对每个构件都回 NONE：函数体用 `{ $code = …; }` 给变量
    /// 赋值，SurrealDB 2.x 里块的值是最后一条语句的值、赋值语句的值是 NONE，于是
    /// `$code == NONE → RETURN NONE`。分支改成表达式后这里钉住四字与五字房间号两条路。
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
             CREATE pe:⟨1_1444⟩ SET noun = 'SBFR', name = '/1RX-RM01-R144-VOLU';
             CREATE pe:⟨1_1445⟩ SET noun = 'PANE', name = '', owner = pe:⟨1_1444⟩;
             CREATE pe:⟨2_72903⟩ SET noun = 'FLOOR', refno = FLOOR:⟨2_72903⟩;
             CREATE pe:⟨2_72904⟩ SET noun = 'FLOOR', refno = FLOOR:⟨2_72904⟩;
             CREATE pe:⟨2_72905⟩ SET noun = 'FLOOR', refno = FLOOR:⟨2_72905⟩;
             RELATE pe:⟨1_1445⟩->room_relate->pe:⟨2_72903⟩ SET room_num = 'R144', inside_count = 8, center_dist = 2960.4;
             RELATE pe:⟨1_1445⟩->room_relate->pe:⟨2_72904⟩ SET room_num = 'R0144', inside_count = 8, center_dist = 2960.4;",
        )
        .await
        .expect("fixture reaches db")
        .check()
        .expect("fixture rows created");

        let mut response = db
            .query(
                "RETURN fn::room_code(pe:⟨2_72903⟩);
                 RETURN fn::room_code(pe:⟨2_72904⟩);
                 RETURN fn::room_code(pe:⟨2_72905⟩);",
            )
            .await
            .expect("room_code reaches db")
            .check()
            .expect("room_code statements");
        let four: Option<String> = response.take(0).expect("decode four-char code");
        let five: Option<String> = response.take(1).expect("decode five-char code");
        let none: Option<String> = response.take(2).expect("decode member without edge");
        assert_eq!(
            four.as_deref(),
            Some("1RX-R144"),
            "四字房间号：面板属主名前缀 + 房间号"
        );
        assert_eq!(
            five.as_deref(),
            Some("1RX-R144"),
            "五字房间号 R0144 → 去掉第二位"
        );
        assert_eq!(none, None, "没有 room_relate 边的构件仍回 NONE");
    }

    /// `gy_common.surql` 里那一个字面量必须与 `aios_core::model_storage::MODEL_STORAGE_VERSION` 同值。
    ///
    /// SurrealQL 没有跨语言常量，`let $storage_version_expected = 1;` 是唯一一处手抄。
    /// 改版本号时如果只改 Rust 常量、忘了改这里，读侧会拿新版本去比一条旧字面量，
    /// 库里的记录永远「对不上」——所有库都被判成老库，而且没有任何报错指向这一行。
    /// 顺带钉住错误文本的两个片段：稳定码与可复制的重建入口。
    #[test]
    fn the_surql_storage_version_literal_matches_the_crate_constant() {
        let text = EMBEDDED_SURQL
            .iter()
            .find(|(name, _)| *name == "gy_common.surql")
            .map(|(_, text)| *text)
            .expect("gy_common.surql 必须在内置快照里");

        let expected_literal = format!(
            "let $storage_version_expected = {};",
            aios_core::model_storage::MODEL_STORAGE_VERSION
        );
        assert!(
            text.contains(&expected_literal),
            "gy_common.surql 的版本字面量与 aios_core 常量不同值，应当是 `{expected_literal}`"
        );
        assert!(
            text.contains("MODEL_STORAGE_VERSION_MISMATCH"),
            "错误文本必须带稳定码，rs-server 按它映射 409"
        );
        assert!(
            text.contains("/model/rebuild"),
            "错误文本必须带可复制的重建入口"
        );
    }

    /// 老库（没有 `storage_version:current` 这条记录）上 `fn::pe_ptset_of` 必须 THROW，
    /// 不能静默回 NONE——否则 `fn::bran_comp_len` 这类长度函数会回 0.0，被误当成
    /// 「这根没有点」。补上记录之后同一次调用必须不再报错（这里没有 `pe_ptset` 行，
    /// 所以回 NONE，那是「这根还没按新版发布过」的正常状态）。
    #[tokio::test]
    async fn pe_ptset_of_throws_on_a_database_without_the_storage_version_record() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("embedded_surql_storage_version")
            .await
            .expect("select fixture db");
        define_embedded_functions_on(&db)
            .await
            .expect("embedded load succeeds");

        db.query("CREATE pe:⟨24381_100817⟩ SET dbnum = 7997;")
            .await
            .expect("fixture reaches db")
            .check()
            .expect("fixture row created");

        let expected = aios_core::model_storage::MODEL_STORAGE_VERSION;
        let error = db
            .query("RETURN fn::pe_ptset_of(pe:⟨24381_100817⟩);")
            .await
            .expect("statement reaches db")
            .check()
            .expect_err("缺版本记录时必须 THROW，不能静默回 NONE")
            .to_string();
        for fragment in [
            "MODEL_STORAGE_VERSION_MISMATCH".to_owned(),
            "dbnum=7997".to_owned(),
            "stored=none".to_owned(),
            format!("expected={expected}"),
            "POST /api/v1/dbnums/7997/model/rebuild".to_owned(),
        ] {
            assert!(
                error.contains(&fragment),
                "错误文本缺少片段 `{fragment}`，实际: {error}"
            );
        }

        db.query(format!(
            "UPSERT storage_version:current CONTENT {{ version: {expected}, shape: 'test', rebuild: 'test' }};"
        ))
        .await
        .expect("seed storage version reaches db")
        .check()
        .expect("seed storage version statement");

        let mut response = db
            .query("RETURN fn::pe_ptset_of(pe:⟨24381_100817⟩);")
            .await
            .expect("statement reaches db")
            .check()
            .expect("版本对上之后不应再报错");
        let ptset: Option<serde_json::Value> = response.take(0).expect("decode ptset");
        assert!(ptset.is_none(), "没有 pe_ptset 行时应回 NONE，实际: {ptset:?}");
    }
}
