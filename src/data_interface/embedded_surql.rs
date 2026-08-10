//! 内置 SurrealDB 函数集：`resource/surreal/` 的**编译期快照**。
//!
//! 部署包的 `resource/surreal/` 是随包拷贝的，会漂移：现场 test-worklspace 的
//! bin 停在 2025-06——`gen_root.surql` 整个缺失、`common.surql` 差 3KB，活库
//! （1516/AvevaMarineSample）因此缺 `fn::room_relate_of` / `fn::room_num_of` 与
//! 全部 12 个 `fn::gen_root_*`，`fn::room_code` 一直是旧语义。更糟的是旧 exe 每次
//! 启动都把旧函数体再灌一遍（`define_common_functions` 无条件加载 CWD 脚本），
//! 手工灌一次新的、下次重启又被打回去。
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
        "gen_root.surql",
        include_str!("../../resource/surreal/gen_root.surql"),
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
/// 房间语义与 gen-root 巡检会退化。收口硬前置（`fn::anc_u64` 等）不在这里：
/// 那是 `selfcheck_surreal_functions` 的硬门，缺了直接拒绝启动。
const MARQUEE_FUNCTIONS: &[&str] = &[
    "room_relate_of",
    "room_num_of",
    "gen_root_of",
    "gen_roots_todo",
];

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
    /// 每次启动都会把它灌回去），内置灌入之后必须（1）核验面四个新函数全部就位，
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
}
