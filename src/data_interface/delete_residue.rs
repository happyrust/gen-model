//! 删除落库的设计数据残留清理(gen-model#30)。
//!
//! `EleOperationDetail::Deleted` 的 `to_surql` 只渲染一条
//! `UPDATE pe:<根> SET deleted = true, sesno = <s>`,而 E3D 会话流对级联删除的
//! 子元素通常**不产生**独立 Deleted 记录。于是删除一个层级后(现场证据见 #30):
//!
//! - 子树 `pe` 行连 tombstone 都没有(`deleted = false` 原样留存);
//! - 名词属性行(`EQUI:<r>` / `BOX:<r>` …)整行残留——plant-ui 按 refno 重新定位
//!   已删除元素时,属性面板会重新显示这些旧属性(plant-ui#3 的删除场景残留形态);
//! - 以被删元素为属主的 `pe_owner` 边悬挂。
//!
//! 模型产物(inst_relate / inst_info / geo_relate / aabb)不归这里:它们由
//! [`super::helper::delete_inst_relate_subtree`] 的专门链路清理,现场验证是干净的。
//!
//! 本模块在渲染完主数据语句后,对每个**真删**根元素枚举**窗口前持久层**的 pe 子树
//! (被删元素已从文件 refno 索引消失,这份拓扑只能来自持久层——与产物预载同一依据,
//! 见 [`super::staging`] 的 mutation 预载),补渲染三类幂等语句:
//!
//! 1. 子树每个成员 `UPDATE pe:<r> SET deleted = true, sesno = <s>`(根的 tombstone
//!    与 `to_surql` 的那条重复,幂等,保留它是为了不依赖上游渲染顺序);
//! 2. 按成员 `pe.noun` 删除名词行 `DELETE <NOUN>:<r>`;
//! 3. 删除以成员为属主的 `pe_owner` 边——按记录 id 范围直接寻址
//!    (`pe_owner:[<owner>, 0]..=[<owner>, MAX]`),**不要**写成
//!    `DELETE pe_owner WHERE out = <owner>`:`(in, out)` 唯一索引前缀是 `in`,
//!    只给 `out` 走全表扫描,#14 实测 3.83s/次 vs 主键寻址亚毫秒。
//!
//! 「成员作为 child 挂在**存活**属主成员表里的那条边」不归这里管:属主的成员表
//! 变化在会话流里以 Modified(children_changed)出现,由属主侧的成员边重渲染收敛
//! (现场验证:ZONE→被删 EQUI 的边在删除会话后已消失,残留的只有反方向)。
//!
//! 语句进入与主数据同一条持久化通道(直写分块事务 / 暂存 journal 重放),全部幂等,
//! 失败随窗口整体重试,不引入新的补偿面。

use std::collections::BTreeMap;

use aios_core::RefnoEnum;
use pdms_io::io::{EleOperationData, EleOperationDetail};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

/// `pe_owner` 槽位上界。槽位是成员在属主成员表里的序号(#14),u32 值域足够。
const PE_OWNER_SLOT_END: u64 = 4_294_967_295;

/// 收集本窗口**真删**操作的 `(根 refno, 删除会话号)`,同 refno 取最大会话号。
///
/// `restore_finally_live_deletes` 已在收集阶段把「属主搬移产生的临时 Deleted、
/// Save Work 后仍存活」的条目替换成 final upsert,走到渲染的 Deleted 都是真删。
fn deleted_roots(range_eles: &BTreeMap<u32, Vec<EleOperationData>>) -> Vec<(RefnoEnum, u32)> {
    let mut roots: BTreeMap<RefnoEnum, u32> = BTreeMap::new();
    for (sesno, ops) in range_eles {
        for op in ops {
            if matches!(op.detail, EleOperationDetail::Deleted) {
                let refno = RefnoEnum::from(op.refno);
                let slot = roots.entry(refno).or_insert(*sesno);
                *slot = (*slot).max(*sesno);
            }
        }
    }
    roots.into_iter().collect()
}

/// 名词表名守卫,与 [`super::fast_delete`] 同一判据:dabacon 名词全为大写字母/
/// 数字/下划线,拒绝其余任何值——`noun` 来自数据行,拼进 SQL 前必须白名单化。
fn valid_noun_table(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

#[derive(Debug, serde::Deserialize)]
struct PeNounRow {
    id: surrealdb::sql::Thing,
    #[serde(default)]
    noun: Option<String>,
}

/// 渲染本窗口全部删除根的残留清理语句。没有真删操作时不触碰数据库,返回空。
///
/// `source` 是**窗口前状态**的读取端:生产路径传持久层 `SUL_DB`(暂存窗口内
/// 它仍指向持久层,预载同款用法);测试传 mem 实例。
pub(crate) async fn render_delete_residue_statements(
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    source: &Surreal<Any>,
) -> anyhow::Result<Vec<String>> {
    let roots = deleted_roots(range_eles);
    if roots.is_empty() {
        return Ok(Vec::new());
    }

    let mut statements = Vec::new();
    let mut member_total = 0usize;
    let mut skipped_nouns = 0usize;
    for (root, sesno) in &roots {
        // 自身 + 全部后代。子树收集失败必须上抛让窗口整体重试:静默跳过会把
        // 残留永久固化,而且不会再有第二次机会——写回后持久层拓扑就没了。
        let subtree = crate::data_interface::helper::collect_pe_subtree_refnos_from(
            source,
            std::slice::from_ref(root),
        )
        .await
        .map_err(|e| anyhow::anyhow!("枚举删除根 {root} 的持久层子树失败: {e}"))?;
        let mut members: Vec<RefnoEnum> = subtree.into_iter().collect();
        members.sort_unstable();

        for chunk in members.chunks(256) {
            let keys = chunk
                .iter()
                .map(RefnoEnum::to_pe_key)
                .collect::<Vec<_>>()
                .join(",");
            // 按记录 id 直接寻址(不要 WHERE id IN:全表扫描,见 preload 同款注释);
            // exists 过滤掉「窗口内新增又删除、持久层从未有过」的成员。
            let mut response = source
                .query(format!(
                    "SELECT id, noun FROM [{keys}] WHERE record::exists(id);"
                ))
                .await
                .map_err(|e| anyhow::anyhow!("读取删除子树成员 noun 失败: {e}"))?
                .check()
                .map_err(|e| anyhow::anyhow!("读取删除子树成员 noun 语句失败: {e}"))?;
            for row in response
                .take::<Vec<PeNounRow>>(0)
                .map_err(|e| anyhow::anyhow!("解析删除子树成员 noun 失败: {e}"))?
            {
                let refno = crate::data_interface::helper::pe_thing_to_refno(row.id)?;
                member_total += 1;
                let pe_key = refno.to_pe_key();
                statements.push(format!(
                    "UPDATE {pe_key} SET deleted = true, sesno = {sesno}"
                ));
                match row.noun.as_deref() {
                    Some(noun) if valid_noun_table(noun) => {
                        statements.push(format!("DELETE {}", refno.refno().to_type_key(noun)));
                    }
                    _ => skipped_nouns += 1,
                }
                statements.push(format!(
                    "DELETE pe_owner:[{pe_key}, 0]..=[{pe_key}, {PE_OWNER_SLOT_END}]"
                ));
            }
        }
    }

    if !statements.is_empty() {
        println!(
            "删除残留清理:{} 个删除根,持久层子树成员 {member_total},补语句 {} 条{}",
            roots.len(),
            statements.len(),
            if skipped_nouns > 0 {
                format!("(其中 {skipped_nouns} 个成员 noun 缺失/非法,跳过名词行删除)")
            } else {
                String::new()
            }
        );
    }
    Ok(statements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::RefU64;
    use surrealdb::engine::any::connect;

    fn refno(tail: u64) -> RefU64 {
        RefU64((4000000009_u64 << 32) | tail)
    }

    fn window_with_delete(root: RefU64, sesno: u32) -> BTreeMap<u32, Vec<EleOperationData>> {
        let mut range = BTreeMap::new();
        range.insert(
            sesno,
            vec![EleOperationData::new(root, sesno, EleOperationDetail::Deleted)],
        );
        range
    }

    async fn run_statements(db: &Surreal<Any>, statements: &[String]) {
        for sql in statements {
            db.query(sql.as_str())
                .await
                .expect("execute residue statement")
                .check()
                .expect("residue statement succeeds");
        }
    }

    /// 删除根的残留清理必须覆盖整个子树:子件补 tombstone、两级名词行删除、
    /// 属主边按 id 范围删除;不在子树里的兄弟一根毫毛都不能动。
    #[tokio::test(flavor = "multi_thread")]
    async fn residue_covers_subtree_nouns_and_owner_edges_sparing_siblings() {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("residue").await.expect("use db");
        db.query(
            "create pe:4000000009_10 set noun = 'EQUI', deleted = false;
             create pe:4000000009_11 set noun = 'BOX', deleted = false;
             create pe:4000000009_20 set noun = 'EQUI', deleted = false;
             create EQUI:4000000009_10; create BOX:4000000009_11; create EQUI:4000000009_20;
             insert relation into pe_owner { id: [pe:4000000009_10, 0], in: pe:4000000009_11, out: pe:4000000009_10 };",
        )
        .await
        .expect("create fixture")
        .check()
        .expect("valid fixture");

        let statements =
            render_delete_residue_statements(&window_with_delete(refno(10), 203), &db)
                .await
                .expect("render residue");
        assert!(
            statements
                .iter()
                .any(|s| s.contains("pe:4000000009_11") && s.contains("deleted = true")),
            "child tombstone missing: {statements:?}"
        );
        run_statements(&db, &statements).await;

        let mut response = db
            .query(
                "return [
                    pe:4000000009_10.deleted,
                    pe:4000000009_11.deleted,
                    pe:4000000009_11.sesno,
                    record::exists(EQUI:4000000009_10),
                    record::exists(BOX:4000000009_11),
                    record::exists(pe_owner:[pe:4000000009_10, 0]),
                    record::exists(EQUI:4000000009_20),
                    pe:4000000009_20.deleted
                ];",
            )
            .await
            .expect("query state")
            .check()
            .expect("valid state query");
        let state = response
            .take::<Vec<serde_json::Value>>(0)
            .expect("decode state");

        assert_eq!(state[0], serde_json::json!(true), "root tombstone");
        assert_eq!(state[1], serde_json::json!(true), "child tombstone");
        assert_eq!(state[2], serde_json::json!(203), "child sesno = delete session");
        assert_eq!(state[3], serde_json::json!(false), "root noun row deleted");
        assert_eq!(state[4], serde_json::json!(false), "child noun row deleted");
        assert_eq!(state[5], serde_json::json!(false), "owner edge deleted");
        assert_eq!(state[6], serde_json::json!(true), "sibling noun row intact");
        assert_eq!(state[7], serde_json::json!(false), "sibling pe untouched");
    }

    /// noun 缺失或非白名单值时跳过名词行删除,但 tombstone 与边清理照常。
    #[tokio::test(flavor = "multi_thread")]
    async fn missing_or_invalid_noun_skips_noun_row_but_keeps_tombstone() {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("residue_noun").await.expect("use db");
        db.query("create pe:4000000009_30 set deleted = false;")
            .await
            .expect("create fixture")
            .check()
            .expect("valid fixture");

        let statements =
            render_delete_residue_statements(&window_with_delete(refno(30), 7), &db)
                .await
                .expect("render residue");
        assert!(
            statements.iter().all(|s| !s.starts_with("DELETE EQUI")
                && !s.contains("DELETE :")
                && !s.to_lowercase().contains("delete none")),
            "no noun-row delete may be rendered: {statements:?}"
        );
        assert!(
            statements
                .iter()
                .any(|s| s.contains("pe:4000000009_30") && s.contains("deleted = true")),
            "tombstone missing: {statements:?}"
        );
        assert!(
            statements
                .iter()
                .any(|s| s.contains("pe_owner:[pe:4000000009_30, 0]..")),
            "owner-edge range delete missing: {statements:?}"
        );
    }

    /// 窗口里没有真删操作时零开销:不查库、不渲染。
    #[tokio::test(flavor = "multi_thread")]
    async fn window_without_deletes_renders_nothing() {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("residue_empty").await.expect("use db");
        let statements = render_delete_residue_statements(&BTreeMap::new(), &db)
            .await
            .expect("render residue");
        assert!(statements.is_empty());
    }
}
