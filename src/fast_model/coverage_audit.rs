//! 生成覆盖对齐 · 阶段 2 的 catch-all 观测（`docs/plans/generation-coverage-align.md`）。
//!
//! dabacon 字典认定为几何的 noun = `primitive ∪ geomset ∪ extrusion`（当前快照 395 个），
//! 而 `gen_geos_data` 的四个生成桶走的是硬编码名单，两者差集 291 个 noun 是「可能被静默
//! 漏掉」的**上界**——层级式生成让其中大部分由 catalogue 在生成根子树里展开渲染，并不需要
//! 进任何顶层名单。因此真实缺口只能靠运行期观测收敛：本模块在真实生成时记录「差集 noun
//! 确实出现在生成子树里」的实证，只写日志和计数，不改变任何生成结果。
//!
//! 默认关闭。设 `AIOS_GEOM_COVERAGE_AUDIT=on`（或 `1`/`true`）后随生成一起跑。

use aios_core::{RefnoEnum, SUL_DB};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 开关环境变量；与 `AIOS_CATA_CLOSURE_MODE` 同款「默认 Off + opt-in」。
const ENV_KEY: &str = "AIOS_GEOM_COVERAGE_AUDIT";

/// 单条 noun 统计语句里最多带多少个 pe 记录 id，避免语句过长。
const NOUN_LOOKUP_CHUNK: usize = 2000;

/// 每个分段最多回查多少个命中元素；超出只统计前 N 个并标记截断，防止观测拖慢生成。
const MAX_LOOKUP_PER_SEGMENT: usize = 50_000;

#[derive(Debug, Deserialize)]
struct NounCountRow {
    noun: String,
    cnt: usize,
}

/// 本进程累计的「名单外几何 noun → 命中元素数」。
fn counters() -> &'static Mutex<BTreeMap<String, usize>> {
    static COUNTS: OnceLock<Mutex<BTreeMap<String, usize>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 观测是否开启。
pub fn audit_enabled() -> bool {
    match std::env::var(ENV_KEY) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "on" | "1" | "true" | "yes")
        }
        Err(_) => false,
    }
}

/// dict 认几何、却不在任何生成路由/覆盖名单里的 noun（与缺口清单同一口径）。
fn uncovered_nouns() -> &'static [String] {
    static UNCOVERED: OnceLock<Vec<String>> = OnceLock::new();
    UNCOVERED
        .get_or_init(|| {
            let clf = parse_pdms_db::dict::default_noun_classifier();
            parse_pdms_db::dict::uncovered_geometry_nouns(clf)
                .into_iter()
                .collect()
        })
        .as_slice()
}

/// 观测一个生成分段：统计差集 noun 在本段子树里的真实命中数。
///
/// 失败一律降级为日志——观测绝不能影响生成结果，也不能让生成失败。
pub async fn audit_segment(target_refnos: &[RefnoEnum], skip_exist: bool) {
    if !audit_enabled() || target_refnos.is_empty() {
        return;
    }
    let nouns = uncovered_nouns();
    if nouns.is_empty() {
        log::warn!("[coverage_audit] 内嵌 noun_flags.json 不可用，跳过覆盖观测");
        return;
    }
    let noun_refs: Vec<&str> = nouns.iter().map(|s| s.as_str()).collect();
    let hits = match aios_core::query_multi_deep_versioned_children_filter_inst(
        target_refnos,
        &noun_refs,
        skip_exist,
    )
    .await
    {
        Ok(hits) => hits,
        Err(e) => {
            log::warn!("[coverage_audit] 子树查询失败，跳过本段观测: {e}");
            return;
        }
    };
    let hits: Vec<RefnoEnum> = hits.into_iter().collect();
    if hits.is_empty() {
        return;
    }
    let truncated = hits.len() > MAX_LOOKUP_PER_SEGMENT;
    let sampled = if truncated {
        &hits[..MAX_LOOKUP_PER_SEGMENT]
    } else {
        hits.as_slice()
    };
    let counts = match count_by_noun(sampled).await {
        Ok(counts) => counts,
        Err(e) => {
            log::warn!("[coverage_audit] noun 统计失败，跳过本段观测: {e}");
            return;
        }
    };
    if counts.is_empty() {
        return;
    }
    log::warn!(
        "[coverage_audit] 本段命中名单外几何 noun {} 种 / {} 个元素{}: {}",
        counts.len(),
        sampled.len(),
        if truncated {
            format!("（子树共 {} 个，已截断统计）", hits.len())
        } else {
            String::new()
        },
        fmt_counts(&counts)
    );
    if let Ok(mut total) = counters().lock() {
        for (noun, cnt) in counts {
            *total.entry(noun).or_default() += cnt;
        }
    }
}

/// 打印本次生成的累计观测结果并清空，供下一次生成重新计数。
pub fn report_and_reset() {
    if !audit_enabled() {
        return;
    }
    let Ok(mut total) = counters().lock() else {
        return;
    };
    if total.is_empty() {
        println!("[coverage_audit] 未发现名单外几何 noun（dict 认几何的 noun 全部落在生成路由内）");
        return;
    }
    let elements: usize = total.values().sum();
    println!(
        "[coverage_audit] 本次生成命中名单外几何 noun {} 种 / {} 个元素: {}",
        total.len(),
        elements,
        fmt_counts(&total)
    );
    println!(
        "[coverage_audit] 以上是「dict 认几何但不在顶层路由名单」的实证；\
         其中由 catalogue 子树展开渲染的属正常，需人工判定后再决定是否补路由"
    );
    total.clear();
}

fn fmt_counts(counts: &BTreeMap<String, usize>) -> String {
    let mut pairs: Vec<(&String, &usize)> = counts.iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    pairs
        .iter()
        .map(|(noun, cnt)| format!("{noun}={cnt}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 按 noun 聚合一批元素；分块下发避免单条语句过长。
async fn count_by_noun(refnos: &[RefnoEnum]) -> anyhow::Result<BTreeMap<String, usize>> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for chunk in refnos.chunks(NOUN_LOOKUP_CHUNK) {
        let ids = chunk
            .iter()
            .map(|r| r.to_pe_key())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("select noun, count() as cnt from [{ids}] group by noun");
        let rows = SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("query noun histogram failed: {e}"))?
            .take::<Vec<NounCountRow>>(0)
            .map_err(|e| anyhow::anyhow!("decode noun histogram failed: {e}"))?;
        for row in rows {
            let noun = row.noun.trim().to_ascii_uppercase();
            if noun.is_empty() {
                continue;
            }
            *counts.entry(noun).or_default() += row.cnt;
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::generation_root::{
        GenerationNode, configured_delivery_unit_types, resolve_element_generation_root,
    };
    use crate::data_interface::helper::pe_thing_to_refno;
    use std::collections::{BTreeSet, HashMap, HashSet};
    use surrealdb::sql::Thing;

    #[derive(Deserialize)]
    struct PeAuditRow {
        id: Thing,
        #[serde(default)]
        owner: Option<Thing>,
        noun: String,
        #[serde(default)]
        name: String,
    }

    #[test]
    fn uncovered_nouns_exclude_routed_geometry() {
        let nouns = uncovered_nouns();
        if nouns.is_empty() {
            return; // 无内嵌字典的环境软跳过
        }
        assert!(
            !nouns.iter().any(|n| n == "BOX"),
            "BOX 已在 prim 路由名单内"
        );
        assert!(
            nouns.iter().any(|n| n == "POINSP"),
            "POINSP 属名单外几何，应被观测覆盖"
        );
    }

    #[test]
    fn counts_are_rendered_by_descending_hit_count() {
        let counts = BTreeMap::from([
            ("AIDLIN".to_string(), 3),
            ("POINSP".to_string(), 42),
            ("HPLATE".to_string(), 42),
        ]);
        assert_eq!(fmt_counts(&counts), "HPLATE=42, POINSP=42, AIDLIN=3");
    }

    #[tokio::test]
    #[ignore = "manual live: read-only histogram of uncovered geometry nouns"]
    async fn live_database_uncovered_noun_histogram() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let nouns = uncovered_nouns()
            .iter()
            .map(|noun| format!("'{noun}'"))
            .collect::<Vec<_>>()
            .join(",");
        let rows = SUL_DB
            .query(format!(
                "SELECT noun, count() AS cnt FROM pe WHERE noun IN [{nouns}] GROUP BY noun"
            ))
            .await
            .expect("query uncovered noun histogram")
            .take::<Vec<NounCountRow>>(0)
            .expect("decode uncovered noun histogram");
        let counts = rows
            .into_iter()
            .map(|row| (row.noun, row.cnt))
            .collect::<BTreeMap<_, _>>();
        println!(
            "[coverage_audit] 实库命中名单外几何 noun {} 种 / {} 个元素: {}",
            counts.len(),
            counts.values().sum::<usize>(),
            fmt_counts(&counts)
        );
    }

    #[tokio::test]
    #[ignore = "manual live: map every uncovered geometry element to a modeled generation root"]
    async fn live_database_uncovered_nouns_resolve_to_modeled_roots() {
        const PAGE_SIZE: usize = 5_000;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let mut rows = Vec::new();
        loop {
            let offset = rows.len();
            let page = SUL_DB
                .query(format!(
                    "SELECT id, owner, noun, name FROM pe WHERE deleted != true \
                     ORDER BY id LIMIT {PAGE_SIZE} START {offset}"
                ))
                .await
                .expect("query active PE graph page")
                .take::<Vec<PeAuditRow>>(0)
                .expect("decode active PE graph page");
            let page_len = page.len();
            rows.extend(page);
            if page_len < PAGE_SIZE {
                break;
            }
        }
        let mut graph = HashMap::with_capacity(rows.len());
        for row in rows {
            let refno = pe_thing_to_refno(row.id).expect("decode PE id");
            let owner = row
                .owner
                .map(RefnoEnum::from)
                .filter(|owner| owner.is_valid() && *owner != refno);
            graph.insert(
                refno,
                GenerationNode {
                    owner,
                    noun: row.noun,
                    name: row.name,
                },
            );
        }

        let directly_modeled = SUL_DB
            .query("SELECT VALUE in FROM inst_relate")
            .await
            .expect("query modeled roots")
            .take::<Vec<Thing>>(0)
            .expect("decode modeled roots")
            .into_iter()
            .map(pe_thing_to_refno)
            .collect::<anyhow::Result<HashSet<_>>>()
            .expect("decode modeled root ids");
        let mut modeled_subtrees = directly_modeled.clone();
        for modeled in directly_modeled {
            let mut current = modeled;
            for _ in 0..crate::data_interface::generation_root::MAX_ANCESTOR_DEPTH {
                let Some(owner) = graph.get(&current).and_then(|node| node.owner) else {
                    break;
                };
                if !modeled_subtrees.insert(owner) {
                    break;
                }
                current = owner;
            }
        }
        let uncovered = uncovered_nouns()
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let unit_types = configured_delivery_unit_types();
        let mut stats: BTreeMap<
            String,
            (usize, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>),
        > = BTreeMap::new();
        let mut root_nouns: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

        for (refno, node) in &graph {
            if !uncovered.contains(node.noun.trim().to_ascii_uppercase().as_str()) {
                continue;
            }
            let stat = stats.entry(node.noun.clone()).or_default();
            stat.0 += 1;
            let Some(root) =
                resolve_element_generation_root(*refno, &unit_types, |id| graph.get(&id).cloned())
            else {
                stat.3.insert(refno.to_pdms_str());
                continue;
            };
            stat.1.insert(root.root.to_pdms_str());
            *root_nouns
                .entry(node.noun.clone())
                .or_default()
                .entry(root.noun)
                .or_default() += 1;
            if !modeled_subtrees.contains(&root.root) {
                stat.2.insert(root.root.to_pdms_str());
            }
        }

        for (noun, (elements, roots, missing, unresolved)) in &stats {
            println!(
                "[coverage_audit] {noun}: elements={elements}, roots={}, missing_roots={}, unresolved={}, missing_sample={:?}, unresolved_sample={:?}",
                roots.len(),
                missing.len(),
                unresolved.len(),
                missing.iter().take(5).collect::<Vec<_>>(),
                unresolved.iter().take(10).collect::<Vec<_>>()
            );
            println!(
                "[coverage_audit] {noun}: root_nouns={:?}",
                root_nouns.get(noun).cloned().unwrap_or_default()
            );
        }
    }
}
