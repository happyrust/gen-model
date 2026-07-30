//! 本期执行范围：当前 MDB 声明的 DESI 库。
//!
//! 范围过去与 MDB 无关——「项目目录里认得出的库」× 类型白名单 ×
//! 手写的 `manual_db_nums`。AvevaMarineSample 目录里躺着 287 个 DESI，
//! 而人真正打开的 MDB `/ALL` 只声明了 29 个，界面就照 287 个列。
//!
//! 取的是 **MDB 声明口径**，不是「已解析出 SITE 的库」（rs-core 的
//! `query_mdb_db_nums` 是后者，模型树用它）：MDB 列了却从没导入过的库正是
//! `initialization_required` 那一档，该出现在范围里等人确认；用交集口径它们
//! 永远进不来。

use std::collections::BTreeSet;

use aios_core::{DBType, SUL_DB};

use crate::data_interface::sesno_range::COLD_START_DB_TYPES;

/// 同名多条 MDB 取 CURD 最长的那条。设计库与目录库的 SYS 同时解析时，同名
/// `/ALL` 会并存，而目录侧那条的 CURD 往往只有一项甚至为空——`limit 1` 会在
/// 两条之间随机挑，挑中目录侧范围就只剩一两个库。rs-core 的 `MDB_DESI_DBNOS`
/// 是模型树在同一个坑上踩出来的，这里同源。
const MDB_DBNOS: &str = r#"(select dbnos, array::len(dbnos) as n
    from (select (select value DBNO from CURD.refno where STYP = $db_type) as dbnos
          from MDB where NAME = $mdb)
    order by n desc limit 1)[0].dbnos ?? []"#;

/// 一次扫描的执行范围。
#[derive(Debug, Clone)]
pub struct UpdateScope {
    mdb: String,
    /// MDB 的 CURD 里 STYP=DESI 的那些库号，升序去重。
    desi: BTreeSet<u32>,
    unrestricted: bool,
    /// 范围没解出名单时那句要说给人听的话。调用方必须把它放进自己的 warnings，
    /// 否则「这次为什么只跑了几个系统库」没有出处。
    warning: Option<String>,
}

impl UpdateScope {
    /// 解出 `mdb` 声明的 DESI 库号。
    ///
    /// 空名单**不报错**，这是有意的：范围名单存在 SYS meta 库（`amssys`）里，而
    /// SYS 库要跑一次更新才会被解析——一上来就 bail 的话，新项目、或者刚换过
    /// SurrealDB 命名空间的项目会卡成死结：想更新得先有范围，想有范围得先更新。
    /// 所以库里一条 MDB 都没有时退化成「只解析 SYS meta」，把名单的来源先建起来，
    /// 下一轮就有真范围了；同时留一句告警，不让它悄悄发生。
    ///
    /// 真正报错的只有一种：**库里有 MDB，但没有叫这个名字的**——那是名字打错，
    /// 退化成 bootstrap 只会让人以为项目是空的。错误里带上库里实际有哪些 MDB。
    pub async fn resolve(mdb: &str) -> anyhow::Result<Self> {
        let name = aios_core::helper::to_e3d_name(mdb).into_owned();
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::distinct(SELECT VALUE NAME FROM MDB);\nRETURN {MDB_DBNOS};"
            ))
            .bind(("mdb", name.clone()))
            .bind(("db_type", u8::from(DBType::DESI)))
            .await?;
        let known: Vec<String> = response.take(0)?;
        let dbnos: Vec<u32> = response.take(1)?;
        let desi: BTreeSet<u32> = dbnos.into_iter().collect();

        let warning = if !desi.is_empty() {
            None
        } else if known.is_empty() {
            Some(format!(
                "库里还没有任何 MDB——存放 MDB/CURD 的 SYS meta 库尚未解析。\
                 本期只解析 SYS meta 把 {name} 的成员名单建起来，设计库一个都不跑；\
                 完成后再跑一次即可拿到真正的执行范围"
            ))
        } else if !known.iter().any(|n| n == &name) {
            let mut sample = known.clone();
            sample.sort();
            sample.truncate(10);
            anyhow::bail!(
                "库里没有名为 {name} 的 MDB，本期执行范围无从谈起。已解析出的 MDB：{}{}",
                sample.join(" / "),
                if known.len() > sample.len() {
                    format!("（共 {} 个）", known.len())
                } else {
                    String::new()
                }
            );
        } else {
            Some(format!(
                "MDB {name} 存在，但它的 CURD 里一个 DESI 设计库都没声明，\
                 本期没有设计库可跑。若不符预期，请检查该 MDB 的 CURD 是否解析完整"
            ))
        };

        Ok(Self {
            mdb: name,
            desi,
            unrestricted: false,
            warning,
        })
    }

    /// 不设限。按 dbnum 点名的入口用它——回归工具与按需初始化的调用方已经自己
    /// 决定了要哪个库，再套一层 MDB 门只会把点名挡掉。
    pub fn unrestricted() -> Self {
        Self {
            mdb: String::new(),
            desi: BTreeSet::new(),
            unrestricted: true,
            warning: None,
        }
    }

    /// 范围没解出设计库名单时的那句话，调用方要原样放进自己的 warnings。
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// 这个库进不进本期执行范围。
    ///
    /// SYS meta（SYST / DICT / GLB / GLOB）**不受 MDB 门控**：MDB 的成员名单本身
    /// 就存在这些库里，[`UpdateScope::resolve`] 读的就是它们。把它们挡在外面，
    /// 以后往 MDB 里加一个库，范围表永远看不见这个新成员——范围定义会自己冻住。
    ///
    /// 除此之外只认 DESI，且必须是本 MDB 声明过的。CATA 参与不了模型交付，
    /// 目录变化要靠重新生成而不是数据批次，这次一律不进范围。
    ///
    /// **这一句是全仓「目录改动会不会触发设计实例重生成」的唯一决定点**
    /// （2026-07-31 决策 A，spec 001 · US5）。ADR-008 / F8 那套 CATA 反向级联规划
    /// （`model_update_plan::build_cata_cascade_plan`）已经实现并有单测，但只要这里
    /// 不放行 CATA，它在生产路径上就一次都跑不到——读那段代码的人很容易以为
    /// 「改目录会触发重生成」，实际不会。要启用就改这里，并按那边文档列的三件事
    /// 一并补齐（新 ADR + 端到端 live 测试）。
    pub fn admits(&self, db_type: &str, dbnum: u32) -> bool {
        if COLD_START_DB_TYPES.contains(&db_type) {
            return true;
        }
        if self.unrestricted {
            return true;
        }
        db_type == "DESI" && self.desi.contains(&dbnum)
    }

    /// MDB 名（已带前导 `/`）。不设限时是空串。
    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    /// MDB 声明的 DESI 库号，升序。
    pub fn declared_desi(&self) -> impl Iterator<Item = u32> + '_ {
        self.desi.iter().copied()
    }

    /// 是否按 dbnum 逐个点名（`unrestricted` 时恒为假：那种模式下没有"声明"可言）。
    pub fn declares(&self, dbnum: u32) -> bool {
        !self.unrestricted && self.desi.contains(&dbnum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(desi: &[u32]) -> UpdateScope {
        UpdateScope {
            mdb: "/ALL".into(),
            desi: desi.iter().copied().collect(),
            unrestricted: false,
            warning: None,
        }
    }

    /// 范围只认「本 MDB 声明的 DESI」，CATA 与 MDB 外的 DESI 一样进不来。
    #[test]
    fn only_desi_declared_by_this_mdb_gets_in() {
        let scope = scope(&[7997, 8000]);
        assert!(scope.admits("DESI", 8000));
        assert!(!scope.admits("DESI", 3001), "MDB 外的设计库不进范围");
        assert!(
            !scope.admits("CATA", 8000),
            "CATA 不进范围，即便同号在名单里"
        );
    }

    /// SYS meta 绕过 MDB 门：范围名单本身就是从这些库里读出来的，挡住它们
    /// 等于让范围定义再也刷新不了。
    #[test]
    fn sys_meta_bypasses_the_mdb_gate() {
        let scope = scope(&[7997]);
        for db_type in COLD_START_DB_TYPES {
            assert!(scope.admits(db_type, 8191), "{db_type} 应当绕过 MDB 门");
        }
    }

    /// 点名入口不设限，否则回归工具指定的 dbnum 会被自己的门挡掉。
    #[test]
    fn unrestricted_admits_everything_but_declares_nothing() {
        let scope = UpdateScope::unrestricted();
        assert!(scope.admits("DESI", 3001));
        assert!(scope.admits("CATA", 5052));
        assert!(!scope.declares(3001));
    }

    /// 真库一跑：上面三个纯函数测不到 SurrealQL 那一趟，而 `resolve` 是整条链
    /// 的命门——`take(0)` 解不出 `Vec<u32>` 的话它会一路 bail 成「范围为空」，
    /// 预览对所有人直接失败。要连 8009 上的 AvevaMarineSample，故 `ignore`：
    /// `cargo test -- --ignored resolves_the_real_mdb`。
    #[tokio::test]
    #[ignore = "需要本地 SurrealDB（8009 / ns 1516 / AvevaMarineSample）"]
    async fn resolves_the_real_mdb_declaration() {
        aios_core::init_test_surreal().await;
        let scope = UpdateScope::resolve("ALL").await.expect("解出 /ALL 的范围");

        assert_eq!(scope.mdb(), "/ALL", "名字要补上前导斜杠");
        let declared: Vec<u32> = scope.declared_desi().collect();
        assert_eq!(declared.len(), 29, "/ALL 的 CURD 里有 29 个 DESI 库号");
        // 8000 是 AMS 的主设计库；3001 在项目目录里有文件却不属于这个 MDB
        // ——它正是过去被算进范围的那 258 个之一。
        assert!(scope.admits("DESI", 8000));
        assert!(!scope.admits("DESI", 3001));

        assert!(scope.warning().is_none(), "解出名单就不该有告警");

        // 名字打错必须报错，而且要告诉人库里实际有哪些 MDB——退化成 bootstrap
        // 只会让人以为项目是空的。
        let err = UpdateScope::resolve("NO_SUCH_MDB")
            .await
            .expect_err("名字不存在应当报错")
            .to_string();
        assert!(err.contains("NO_SUCH_MDB"), "错误里要点名：{err}");
        assert!(err.contains("ALL"), "错误里要列出实际存在的 MDB：{err}");
    }

    /// 库里一条 MDB 都没有时不许 bail：范围名单存在 SYS meta 库里，而 SYS 库要跑
    /// 一次更新才会被解析——bail 的话新项目会卡成「想更新得先有范围、想有范围得
    /// 先更新」的死结。退化成只解析 SYS meta，并且必须留一句告警。
    #[tokio::test]
    #[ignore = "需要一个没有 MDB 表的空 SurrealDB 命名空间"]
    async fn an_unparsed_project_bootstraps_instead_of_deadlocking() {
        aios_core::init_test_surreal().await;
        let scope = UpdateScope::resolve("ALL").await.expect("不该 bail");
        assert_eq!(scope.declared_desi().count(), 0);
        assert!(
            scope.admits("SYST", 8191),
            "SYS meta 照常解析，把名单建起来"
        );
        assert!(!scope.admits("DESI", 8000), "设计库一个都不跑");
        assert!(scope.warning().is_some(), "不许悄悄发生");
    }
}
