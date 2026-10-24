//! 交付单元的房间归属快照：一批开工前、收尾后各读一次，进单元回执的 `rooms_before` / `rooms_after`
//! （增量记录页「房间 前 → 后」，计划 2026-10-24 W1）。
//!
//! 房间归属是库上的 `panel -> room_relate -> element` 边（ADR-073 D1 / D4），生成根落库那一事务里先删后插。
//! 「之前在哪」在删边那一刻就没了，事后问 `e3d.room.lookup` 只拿得到「之后」——所以必须在开工之前读一遍。
//! 读的是本库权威面 `SUL_DB`，按根聚合（根自己 + `anc` 下的全部构件）。不读投影：投影里的边是这进程刚算的
//! 「后」，而「前」就是库上那份。
//!
//! 不读的场合（回执里两格都缺席，前端不画）：本进程算不了房间；读透形态（边在进程 store，不在 `SUL_DB`）；
//! 一批超过 [`MAX_ROOTS`] 根（初始化 / 整库重建：那不是「一轮增量改了什么」的账，每根多两次往返也不值）。
//! 读失败只记日志，不拦批次。
//!
//! 边怎么读（GitHub #66）：按构件走**边目标** `type::thing('pe', …)<-room_relate`（计划 `Iterate Edges`），不按 `out`
//! 过滤 `room_relate` 表——`out` 够不着 `unique_room_relate (in, out)` 的前缀，那一句是整表扫（`Iterate Table`），
//! 夹具 15.7 万条边上实测 694 ms / 句 vs 边目标 0.4 ms；第一版每构件一句扫表，7997 一窗 60 根 1117 构件前后两次快照吃掉
//! 29 分钟（`db-20261024-231937-000000`）。与 `room_model` 写侧「边目标」的改写同一个教训。

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use aios_core::{RefU64, RefnoEnum};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

/// 一批超过这个根数就不拍快照。增量窗口通常一位数；初始化一次规划数千根。
pub const MAX_ROOTS: usize = 64;

/// 一根挂在的一间房（按面板）：这根下有 `members` 个构件被判到这块面板的房间里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomRef {
    pub room_num: String,
    /// `a_b`（`record::id(in)`）。
    pub panel_refno: String,
    pub members: u32,
}

/// `room_relate` 读回来的一条边。
#[derive(Debug, Clone, Deserialize)]
struct Edge {
    panel: String,
    #[serde(default)]
    room_num: Option<String>,
}

/// 按根拍一次快照：`a/b` → 这根此刻挂在哪些房间。不拍（见模块注）或读失败时返回空表——调用方把「没有这根」
/// 当「没拍」，回执里那一格缺席；拍了但没有房间的根是 `Some(vec![])`。
pub async fn snapshot(roots: &[String]) -> HashMap<String, Vec<RoomRef>> {
    if roots.is_empty()
        || roots.len() > MAX_ROOTS
        || !crate::options::room_membership()
        || crate::options::data_face().is_read_through()
    {
        return HashMap::new();
    }
    match snapshot_on(&aios_core::SUL_DB, roots).await {
        Ok(map) => map,
        Err(error) => {
            log::warn!("单元房间快照读失败，回执里不带房间前后: {error:#}");
            HashMap::new()
        }
    }
}

/// 在指定库上拍：每根两步——先按 `anc` / `in` 两路索引拿到根名下的构件（与 `model_record_reader::durable_scope_reads`
/// 同一条纪律：两路分开、不写 OR），再把每个构件的 `room_relate` 边按边目标一句一构件拼成一次往返
/// （[`edge_statement`]）。解不出的根跳过。
pub(crate) async fn snapshot_on(
    db: &Surreal<Any>,
    roots: &[String],
) -> anyhow::Result<HashMap<String, Vec<RoomRef>>> {
    let mut out = HashMap::with_capacity(roots.len());
    for root in roots {
        let Ok(refno) = RefU64::from_str(root) else {
            continue;
        };
        let root_enum = RefnoEnum::from(refno);
        let packed = root_enum.refno().0;
        let owner = root_enum.to_pe_key();
        let mut response = db
            .query(format!(
                "SELECT VALUE record::id(in) FROM inst_relate WHERE anc CONTAINS {packed};\n\
                 SELECT VALUE record::id(in) FROM inst_relate WHERE in = {owner};"
            ))
            .await?
            .check()?;
        let mut members: Vec<String> = response.take(0)?;
        members.extend(response.take::<Vec<String>>(1)?);
        // 根自己也是构件（EQUI 根的边就挂在根上）；没有模型行的根也要问一句。
        members.push(owner.trim_start_matches("pe:").to_owned());
        members.sort_unstable();
        members.dedup();
        let edges = edges_of(db, &members).await?;
        out.insert(root.clone(), aggregate(edges));
    }
    Ok(out)
}

/// 一个构件的房间边：从构件记录沿入边走（`Iterate Edges`）。构件记录不存在 / 没有边时回空表，不报错。
fn edge_statement(member: &str) -> String {
    format!(
        "SELECT record::id(in) AS panel, room_num FROM type::thing('pe', '{}')<-room_relate;",
        member.replace('\'', "")
    )
}

async fn edges_of(db: &Surreal<Any>, members: &[String]) -> anyhow::Result<Vec<Edge>> {
    let mut edges = Vec::new();
    // 一次往返最多拼这么多句：一根 BRAN 几十个构件，分块只是防极端根把一条请求拼到几万句。
    for chunk in members.chunks(256) {
        let sql = chunk
            .iter()
            .map(|member| edge_statement(member))
            .collect::<Vec<_>>()
            .join("\n");
        let mut response = db.query(sql).await?.check()?;
        for index in 0..chunk.len() {
            edges.extend(response.take::<Vec<Edge>>(index)?);
        }
    }
    Ok(edges)
}

/// 按 (房间号, 面板) 归并、数构件；没有房间号的边（老行）记成空串。顺序是房间号 → 面板，两次快照可以逐项比。
fn aggregate(edges: Vec<Edge>) -> Vec<RoomRef> {
    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    for edge in edges {
        *counts
            .entry((edge.room_num.unwrap_or_default(), edge.panel))
            .or_default() += 1;
    }
    counts
        .into_iter()
        .map(|((room_num, panel_refno), members)| RoomRef {
            room_num,
            panel_refno,
            members,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_db() -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test")
            .use_db("unit_room_snapshot")
            .await
            .unwrap();
        db
    }

    /// 根名下的构件按 `anc` 与 `in` 两路收齐，根自己也算；边按 (房间, 面板) 归并数构件；
    /// 没有边的根是 `Some(vec![])`（拍了、不属于任何房间），解不出的根不进表。
    #[tokio::test]
    async fn a_root_snapshot_counts_members_per_room_and_panel() {
        let db = mem_db().await;
        let root: RefnoEnum = "24384_23257".into();
        let packed = root.refno().0;
        db.query(format!(
            "CREATE inst_relate:a SET in = type::thing('pe','24384_23260'), anc = [{packed}];\n\
             CREATE inst_relate:b SET in = type::thing('pe','24384_23262'), anc = [{packed}];\n\
             CREATE inst_relate:c SET in = type::thing('pe','24384_23257'), anc = [];\n\
             CREATE inst_relate:d SET in = type::thing('pe','24384_99999'), anc = [1];\n\
             RELATE pe:24381_1204->room_relate:e1->pe:24384_23262 SET room_num = 'R113', inside_count = 8;\n\
             RELATE pe:24381_35186->room_relate:e2->pe:24384_23262 SET room_num = 'R345', inside_count = 2;\n\
             RELATE pe:24381_1204->room_relate:e3->pe:24384_23260 SET room_num = 'R113', inside_count = 8;\n\
             RELATE pe:24381_1->room_relate:e4->pe:24384_99999 SET room_num = 'R999', inside_count = 8;"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        // 读边走边目标（#66）：计划沿边走（rocksdb 上 `Iterate Edges`，mem 引擎展开成逐条 `Iterate Thing`），不许出现
        // `Iterate Table`——`out` 够不着 `unique_room_relate` 的前缀，按 `out` 过滤表就是整表扫，15.7 万条边上 694 ms / 句。
        let statement = edge_statement("24384_23262");
        assert!(
            statement.contains("<-room_relate") && !statement.contains("FROM room_relate WHERE"),
            "{statement}"
        );
        let mut response = db
            .query(format!("{} EXPLAIN;", statement.trim_end_matches(';')))
            .await
            .unwrap()
            .check()
            .unwrap();
        let plan: surrealdb::Value = response.take(0).unwrap();
        let plan = plan.to_string();
        assert!(
            !plan.contains("Iterate Table")
                && (plan.contains("Iterate Edges") || plan.contains("Iterate Thing")),
            "{plan}"
        );
        let production = include_str!("unit_room_snapshot.rs")
            .split_once("#[cfg(test)]")
            .unwrap()
            .0;
        assert!(
            !production.contains("FROM room_relate WHERE"),
            "按 out 过滤是整表扫，不许回来"
        );

        let snap = snapshot_on(
            &db,
            &[
                "24384/23257".to_owned(),
                "24384/24776".to_owned(),
                "garbage".to_owned(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            snap.get("24384/23257").unwrap(),
            &vec![
                RoomRef {
                    room_num: "R113".into(),
                    panel_refno: "24381_1204".into(),
                    members: 2
                },
                RoomRef {
                    room_num: "R345".into(),
                    panel_refno: "24381_35186".into(),
                    members: 1
                },
            ]
        );
        assert_eq!(
            snap.get("24384/24776").unwrap(),
            &Vec::<RoomRef>::new(),
            "拍了、没房间"
        );
        assert!(!snap.contains_key("garbage"));
    }

    /// 门：空表、超过根数上限都不读库（回空表，回执里缺席）。
    #[tokio::test]
    async fn the_gate_skips_empty_and_oversized_batches() {
        assert!(snapshot(&[]).await.is_empty());
        let many: Vec<String> = (0..=MAX_ROOTS).map(|i| format!("1/{i}")).collect();
        assert!(snapshot(&many).await.is_empty());
    }
}
