//! 交付删除记录（`datacenter_delete`）的只读读者——`GET /api/v1/datacenter/deletes` 的数据面
//! （spec §4.14；plant-ui #1 的对外口，2026-09-23 用户拍板）。
//!
//! 拍板的三条：对外口放 gen-model、**只读**、不带 `instance_code`——那是 rs-server
//! `change_name_into_illegal` 的编码规则，本仓不复制（plan 2026-09-22 D3），调用方拿 `name`
//! 原文自己编；鉴权同 `/api/v1`。gen-model 对这张表仍**只写不删**（D7）：本模块一条 `DELETE`
//! 都没有，删者仍是 rs-server（中台 `remove-apply` 成功之后）。
//!
//! 每行多带 `published`（`datacenter_version:{refno}` 有没有行）与 `published_status`（那行的
//! `status`）：从未提资过的单元被删 / 改名也会落行（D6，gen-model 不判「发布过没有」），调用方靠
//! 这一格跳过它们。过滤条件全是字面量（refno 先解成 [`RefU64`] 再渲成 `pe:⟨…⟩`、`reason` 只认两个
//! 枚举、其余是整数），没有拼进去的原文字串。

use std::collections::HashMap;
use std::collections::HashSet;

use aios_core::pdms_types::{RefU64, RefnoEnum};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::increment_pipeline::DATACENTER_DELETE;

/// `limit` 缺省。
pub const DEFAULT_LIMIT: usize = 500;
/// `limit` 上限：超过就夹到这里（不报错——调用方要的是「尽量多」，不是精确数）。
pub const MAX_LIMIT: usize = 5000;
/// `published` 联查一次问几条 `datacenter_version` 行。
const PUBLISHED_LOOKUP_CHUNK: usize = 200;
const DATACENTER_VERSION: &str = "datacenter_version";

/// 记录为什么要删：交付单元自身被删 / 自身改了位号（`render_datacenter_delete_upsert` 的 `reason`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteReason {
    Deleted,
    Renamed,
}

impl DeleteReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
        }
    }

    /// 大小写不敏感；别的字串一律 `None`（调用方回 400）。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "deleted" => Some(Self::Deleted),
            "renamed" => Some(Self::Renamed),
            _ => None,
        }
    }
}

/// 校验过的过滤条件。空 = 整表。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteRecordFilter {
    /// 只要 `zone_refno` 在其中的行（走 `idx_datacenter_delete_zone`）。空 = 不按 ZONE 筛。
    pub zone_refnos: Vec<RefU64>,
    pub dbnum: Option<u32>,
    pub reason: Option<DeleteReason>,
    /// 只要 `sesno >= since_sesno` 的行（发生在这一版及之后）。
    pub since_sesno: Option<u32>,
    /// 跳过前几条（`START`）。
    pub start: usize,
    /// 一页几条；`0` 按 [`DEFAULT_LIMIT`]，超过 [`MAX_LIMIT`] 夹到上限。
    pub limit: usize,
}

impl DeleteRecordFilter {
    /// 实际生效的页大小。
    pub fn effective_limit(&self) -> usize {
        if self.limit == 0 {
            DEFAULT_LIMIT
        } else {
            self.limit.min(MAX_LIMIT)
        }
    }

    /// `" WHERE …"` 或空串。条件全是字面量：refno 渲成 `pe:⟨…⟩`、`reason` 是枚举、数字是数字。
    fn where_clause(&self) -> String {
        let mut conditions = Vec::new();
        if !self.zone_refnos.is_empty() {
            let mut seen = HashSet::new();
            let zones = self
                .zone_refnos
                .iter()
                .filter(|zone| seen.insert(**zone))
                .map(RefU64::to_pe_key)
                .collect::<Vec<_>>()
                .join(", ");
            conditions.push(format!("zone_refno IN [{zones}]"));
        }
        if let Some(dbnum) = self.dbnum {
            conditions.push(format!("dbnum = {dbnum}"));
        }
        if let Some(reason) = self.reason {
            conditions.push(format!("reason = '{}'", reason.as_str()));
        }
        if let Some(since) = self.since_sesno {
            conditions.push(format!("sesno >= {since}"));
        }
        if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        }
    }

    /// 这一页的行：按 `dbnum, sesno, name` 稳定排序，翻页不漂。
    pub(crate) fn page_sql(&self) -> String {
        format!(
            "SELECT refno, name, type, zone_refno, dbnum, sesno, sesno_time, reason \
             FROM {DATACENTER_DELETE}{} ORDER BY dbnum, sesno, name LIMIT {} START {};",
            self.where_clause(),
            self.effective_limit(),
            self.start,
        )
    }

    /// 过滤后的总数（不分页）。
    pub(crate) fn count_sql(&self) -> String {
        format!(
            "SELECT count() AS n FROM {DATACENTER_DELETE}{} GROUP ALL;",
            self.where_clause()
        )
    }
}

/// 一行交付删除记录的对外形状（spec §3：refno 一律 `a/b`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRecordRow {
    /// 交付单元参考号 `a/b`。
    pub refno: String,
    /// 要在中台删掉的那个位号（E3D `NAME` 原文，含首位 `/`）。
    pub name: String,
    /// 交付单元 noun（`BRAN` / `SUPPO` / `EQUI` / `HANG` …）。
    #[serde(rename = "type")]
    pub noun: String,
    /// 沿 owner 链最近的 ZONE `a/b`；链断为 `null`。
    pub zone_refno: Option<String>,
    pub dbnum: u32,
    /// 发生在哪一版（本窗 T 端会话号）。
    pub sesno: u32,
    /// 那一版在 E3D 里被写入的时刻（RFC3339）；读不到为 `null`。
    pub sesno_time: Option<String>,
    /// `deleted` | `renamed`。
    pub reason: String,
    /// `datacenter_version:{refno}` 有行 = 这个单元提资过。`false` 的行中台没见过，调用方跳过。
    pub published: bool,
    /// 那行的 `status`（rs-server 写的 `Insert` / `Modify` / `Delete`）；`published = false` 时 `null`。
    pub published_status: Option<String>,
}

/// `GET /api/v1/datacenter/deletes` 的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRecordsPage {
    pub items: Vec<DeleteRecordRow>,
    /// 过滤后的总行数（不分页）。
    pub total: usize,
    pub start: usize,
    /// 实际生效的页大小（缺省 / 夹过上限之后的值）。
    pub limit: usize,
    /// `start + items.len() < total`。
    pub has_more: bool,
}

#[derive(Debug, Deserialize)]
struct RawRow {
    refno: RefnoEnum,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "type")]
    noun: Option<String>,
    #[serde(default)]
    zone_refno: Option<RefnoEnum>,
    #[serde(default)]
    dbnum: Option<u32>,
    #[serde(default)]
    sesno: Option<u32>,
    #[serde(default)]
    sesno_time: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CountRow {
    n: usize,
}

#[derive(Debug, Deserialize)]
struct PublishedRow {
    id: RefnoEnum,
    #[serde(default)]
    status: Option<String>,
}

/// 读一页交付删除记录。只读：三条 `SELECT`（页、总数、`published` 联查），不写不删。
pub async fn list_delete_records(
    db: &Surreal<Any>,
    filter: &DeleteRecordFilter,
) -> anyhow::Result<DeleteRecordsPage> {
    let limit = filter.effective_limit();
    let mut response = db
        .query(filter.page_sql())
        .query(filter.count_sql())
        .await
        .context("读 datacenter_delete 传输失败")?
        .check()
        .context("读 datacenter_delete 语句失败")?;
    let rows: Vec<RawRow> = response.take(0).context("解 datacenter_delete 行")?;
    let counts: Vec<CountRow> = response.take(1).context("解 datacenter_delete 总数")?;
    let total = counts.first().map(|row| row.n).unwrap_or(0);

    let published = published_status_of(db, rows.iter().map(|row| row.refno.refno())).await?;

    let items = rows
        .into_iter()
        .map(|row| {
            let refno = row.refno.refno();
            let status = published.get(&refno).cloned();
            DeleteRecordRow {
                refno: refno.to_pdms_str(),
                name: row.name.unwrap_or_default(),
                noun: row.noun.unwrap_or_default(),
                zone_refno: row.zone_refno.map(|zone| zone.refno().to_pdms_str()),
                dbnum: row.dbnum.unwrap_or_default(),
                sesno: row.sesno.unwrap_or_default(),
                sesno_time: row.sesno_time,
                reason: row.reason.unwrap_or_default(),
                published: status.is_some(),
                published_status: status.flatten(),
            }
        })
        .collect::<Vec<_>>();
    let has_more = filter.start + items.len() < total;
    Ok(DeleteRecordsPage {
        items,
        total,
        start: filter.start,
        limit,
        has_more,
    })
}

/// `datacenter_version:{refno}` 的 `status`：有行 → `Some(status)`（`status` 字段缺 → `Some(None)`），
/// 没行 → 键不在。显式 record id 点查、`record::exists` 过滤，表还不存在的新库也稳定回空。
async fn published_status_of(
    db: &Surreal<Any>,
    refnos: impl Iterator<Item = RefU64>,
) -> anyhow::Result<HashMap<RefU64, Option<String>>> {
    let mut distinct = HashSet::new();
    let refnos = refnos
        .filter(|refno| distinct.insert(*refno))
        .collect::<Vec<_>>();
    let mut out = HashMap::new();
    for chunk in refnos.chunks(PUBLISHED_LOOKUP_CHUNK) {
        let keys = chunk
            .iter()
            .map(|refno| refno.to_table_key(DATACENTER_VERSION))
            .collect::<Vec<_>>()
            .join(", ");
        let rows: Vec<PublishedRow> = db
            .query(format!(
                "SELECT id, status FROM [{keys}] WHERE record::exists(id);"
            ))
            .await
            .context("读 datacenter_version 传输失败")?
            .check()
            .context("读 datacenter_version 语句失败")?
            .take(0)
            .context("解 datacenter_version 行")?;
        for row in rows {
            out.insert(row.id.refno(), row.status);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::increment_pipeline::{
        DatacenterDeleteRecord, render_datacenter_delete_upsert,
    };
    use surrealdb::engine::any::connect;

    fn refu(n: u64) -> RefU64 {
        RefU64((7997u64 << 32) | n)
    }

    /// 过滤条件全是字面量、顺序固定；空过滤没有 WHERE；`limit` 缺省 / 夹上限；`START` 跟着 `start`。
    #[test]
    fn the_sql_shape_is_literal_and_stable() {
        let all = DeleteRecordFilter::default();
        assert_eq!(
            all.page_sql(),
            "SELECT refno, name, type, zone_refno, dbnum, sesno, sesno_time, reason \
             FROM datacenter_delete ORDER BY dbnum, sesno, name LIMIT 500 START 0;"
        );
        assert_eq!(
            all.count_sql(),
            "SELECT count() AS n FROM datacenter_delete GROUP ALL;"
        );

        let narrow = DeleteRecordFilter {
            zone_refnos: vec![refu(5), refu(6), refu(5)],
            dbnum: Some(7997),
            reason: Some(DeleteReason::Renamed),
            since_sesno: Some(12),
            start: 40,
            limit: 20,
        };
        assert_eq!(
            narrow.page_sql(),
            "SELECT refno, name, type, zone_refno, dbnum, sesno, sesno_time, reason \
             FROM datacenter_delete WHERE zone_refno IN [pe:7997_5, pe:7997_6] AND dbnum = 7997 \
             AND reason = 'renamed' AND sesno >= 12 ORDER BY dbnum, sesno, name LIMIT 20 START 40;",
            "重复的 ZONE 去重、顺序保留"
        );
        assert_eq!(
            narrow.count_sql(),
            "SELECT count() AS n FROM datacenter_delete WHERE zone_refno IN [pe:7997_5, pe:7997_6] \
             AND dbnum = 7997 AND reason = 'renamed' AND sesno >= 12 GROUP ALL;"
        );

        let capped = DeleteRecordFilter {
            limit: MAX_LIMIT + 1,
            ..Default::default()
        };
        assert_eq!(capped.effective_limit(), MAX_LIMIT);
        assert_eq!(
            DeleteReason::parse(" Deleted "),
            Some(DeleteReason::Deleted)
        );
        assert_eq!(DeleteReason::parse("dropped"), None);
    }

    async fn seeded() -> Surreal<Any> {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("dc_delete")
            .use_db("reader")
            .await
            .expect("use db");
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .expect("schema transport")
            .check()
            .expect("schema");
        let rows = [
            (3u64, "/A-OLD", "BRAN", Some(refu(5)), 12u32, "renamed"),
            (3, "/A-MID", "BRAN", Some(refu(5)), 14, "renamed"),
            (21, "/E'Q\\1", "EQUI", None, 30, "deleted"),
            (40, "/S-1", "SUPPO", Some(refu(6)), 31, "deleted"),
        ];
        for (n, name, noun, zone, sesno, reason) in rows {
            db.query(render_datacenter_delete_upsert(&DatacenterDeleteRecord {
                refno: refu(n),
                name,
                noun,
                zone,
                dbnum: 7997,
                sesno,
                sesno_time: (sesno == 12).then_some("2026-09-22T10:00:00+08:00"),
                reason,
            }))
            .await
            .expect("upsert transport")
            .check()
            .expect("upsert");
        }
        db
    }

    /// 整表 / 按 ZONE / 按 reason / 按 sesno 起点 / 分页都按同一份稳定序回；`published` 在
    /// `datacenter_version` 表还不存在时稳定为 `false`，有行之后只对那个 refno 翻真、带 `status`。
    /// 全程一条写语句都没有落在 `datacenter_delete` 上（只读读者）。
    #[tokio::test(flavor = "multi_thread")]
    async fn lists_pages_filters_and_the_published_flag() {
        let db = seeded().await;

        let all = list_delete_records(&db, &DeleteRecordFilter::default())
            .await
            .expect("list all");
        assert_eq!(all.total, 4);
        assert_eq!(all.limit, DEFAULT_LIMIT);
        assert!(!all.has_more);
        assert_eq!(
            all.items
                .iter()
                .map(|row| (row.sesno, row.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (12, "/A-OLD"),
                (14, "/A-MID"),
                (30, "/E'Q\\1"),
                (31, "/S-1")
            ],
            "按 dbnum, sesno, name 稳定排序"
        );
        let first = &all.items[0];
        assert_eq!(first.refno, "7997/3");
        assert_eq!(first.noun, "BRAN");
        assert_eq!(first.zone_refno.as_deref(), Some("7997/5"));
        assert_eq!(first.dbnum, 7997);
        assert_eq!(
            first.sesno_time.as_deref(),
            Some("2026-09-22T10:00:00+08:00")
        );
        assert_eq!(first.reason, "renamed");
        assert!(
            all.items.iter().all(|row| !row.published),
            "datacenter_version 表还不存在：没人提资过，一行都不算 published"
        );
        let orphan = &all.items[2];
        assert_eq!(orphan.zone_refno, None, "链断的 zone_refno 是 NONE → null");
        assert_eq!(orphan.name, "/E'Q\\1", "转义过的位号原样回来");

        let by_zone = list_delete_records(
            &db,
            &DeleteRecordFilter {
                zone_refnos: vec![refu(5)],
                ..Default::default()
            },
        )
        .await
        .expect("by zone");
        assert_eq!(by_zone.total, 2);
        assert!(by_zone.items.iter().all(|row| row.refno == "7997/3"));

        let deleted_since_31 = list_delete_records(
            &db,
            &DeleteRecordFilter {
                reason: Some(DeleteReason::Deleted),
                since_sesno: Some(31),
                ..Default::default()
            },
        )
        .await
        .expect("deleted since 31");
        assert_eq!(
            deleted_since_31
                .items
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["/S-1"]
        );
        assert_eq!(deleted_since_31.total, 1);

        let page = list_delete_records(
            &db,
            &DeleteRecordFilter {
                start: 1,
                limit: 2,
                ..Default::default()
            },
        )
        .await
        .expect("page");
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["/A-MID", "/E'Q\\1"]
        );
        assert_eq!((page.total, page.start, page.limit), (4, 1, 2));
        assert!(page.has_more, "1 + 2 < 4");

        // 只有 EQUI 21 提资过：published 只对它翻真、带 status。
        db.query("UPSERT datacenter_version:7997_21 CONTENT { instance_code: 'E_Q_1', status: 'Insert' };")
            .await
            .expect("publish transport")
            .check()
            .expect("publish");
        let after = list_delete_records(&db, &DeleteRecordFilter::default())
            .await
            .expect("list after publish");
        let flags = after
            .items
            .iter()
            .map(|row| {
                (
                    row.refno.as_str(),
                    row.published,
                    row.published_status.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            flags,
            vec![
                ("7997/3", false, None),
                ("7997/3", false, None),
                ("7997/21", true, Some("Insert")),
                ("7997/40", false, None),
            ]
        );

        let still: Vec<CountRow> = db
            .query("SELECT count() AS n FROM datacenter_delete GROUP ALL;")
            .await
            .expect("count transport")
            .check()
            .expect("count")
            .take(0)
            .expect("count rows");
        assert_eq!(still[0].n, 4, "读者不删行");
    }
}
