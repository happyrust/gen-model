//! 本期执行范围：当前 MDB 声明的 DESI 库。
//!
//! 范围过去与 MDB 无关——「项目目录里认得出的库」× 类型白名单 ×
//! 手写的 `manual_db_nums`。AvevaMarineSample 目录里躺着 287 个 DESI，
//! 而人真正打开的 MDB `/ALL` 只声明了 29 个，界面就照 287 个列。
//!
//! 反过来，手写名单收窄时又会把 MDB 里的库悄悄挡掉：issue #10 的 7999 就这么在
//! 「每 30 秒发现一次增量、每次跳过」里躺了几周。所以那几个配置项已经从增量判定
//! 里拿掉了（2026-08-06），**这里是增量范围的唯一定义**。它们仍供全量模型生成与
//! 按需基线解析使用，与「这个库要不要增量」无关。
//!
//! 取的是 **MDB 声明口径**，不是「已解析出 SITE 的库」（rs-core 的
//! `query_mdb_db_nums` 是后者，模型树用它）：MDB 列了却从没导入过的库正是
//! `initialization_required` 那一档，该出现在范围里等人确认；用交集口径它们
//! 永远进不来。

use std::collections::BTreeSet;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use aios_core::{DBType, SUL_DB};

use crate::data_interface::sesno_range::COLD_START_DB_TYPES;

/// 同名多条 MDB 取 CURD 最长的那条。设计库与目录库的 SYS 同时解析时，同名
/// `/ALL` 会并存，而目录侧那条的 CURD 往往只有一项甚至为空——`limit 1` 会在
/// 两条之间随机挑，挑中目录侧范围就只剩一两个库。rs-core 的 `MDB_DESI_DBNOS`
/// 是模型树在同一个坑上踩出来的，这里同源。
const MDB_DBNOS: &str = r#"(select dbnos, array::len(dbnos) as n
    from (select (select value DBNO from CURD.refno
                 where type::string(STYP) IN $element_db_types) as dbnos
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

/// 看门狗事件路径的范围缓存（单槽）。
///
/// 名单只在 SYS meta 批次落库时才会变——那一刻 `batch_worker` 会调
/// [`invalidate_scope_cache`]，TTL 只是漏失效的兜底。缓存要解决两件事：
/// 文件事件不再每次都去查一遍几乎不变的名单；SUL_DB 瞬时不可用（连接抖动、
/// 服务器重启）时用暖缓存放行并告警，别让一次抖动把整批文件事件丢掉
/// （2026-08-06 现场：范围解析撞上断连，事件不入队且无重试）。
struct CachedScope {
    scope: UpdateScope,
    resolved_at: Instant,
}

static SCOPE_CACHE: RwLock<Option<CachedScope>> = RwLock::new(None);

/// 缓存视为新鲜的时长。`AIOS_SCOPE_CACHE_SECS` 覆盖，0 = 关闭缓存
/// （每次事件都重查，回到旧行为）。
fn scope_cache_ttl() -> Duration {
    static TTL: OnceLock<Duration> = OnceLock::new();
    *TTL.get_or_init(|| {
        let secs = std::env::var("AIOS_SCOPE_CACHE_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300);
        Duration::from_secs(secs)
    })
}

/// SYS meta 批次刚落库、MDB/CURD 可能已变时调用（`batch_worker` 与
/// `SCOPE_DIRTY` 同点置位）。
pub fn invalidate_scope_cache() {
    if let Ok(mut guard) = SCOPE_CACHE.write() {
        *guard = None;
    }
}

/// 取同名缓存；`max_age` 为 `None` 时不限时（陈旧回退用）。
fn cache_get(name: &str, max_age: Option<Duration>) -> Option<UpdateScope> {
    let guard = SCOPE_CACHE.read().ok()?;
    let cached = guard.as_ref()?;
    if cached.scope.mdb != name {
        return None;
    }
    if let Some(limit) = max_age {
        if cached.resolved_at.elapsed() >= limit {
            return None;
        }
    }
    Some(cached.scope.clone())
}

fn cache_store(scope: &UpdateScope) {
    if let Ok(mut guard) = SCOPE_CACHE.write() {
        *guard = Some(CachedScope {
            scope: scope.clone(),
            resolved_at: Instant::now(),
        });
    }
}

/// 这个 MDB 声明的 DESI / ISOD 库号，取自 **SYS 库文件**那一份——`init_mdb` 启动时
/// 解析并 `install` 进进程内存的 [`super::mdb_membership`]，没登记过就是 `None`。
///
/// 它存在的理由只有一个：全新命名空间上 `MDB` 表是空的，而同一个进程在几十毫秒之前
/// 已经从 `<项目>sys` 文件里把同一份名单读出来了。库侧那句「尚未解析」在事实上只对库
/// 成立，两份名单的成员判据本来就同一个——文件侧 `element_databases()` 过滤
/// `DESI_STYP | ISOD_STYP`，库侧 [`MDB_DBNOS`] 绑的 `element_db_types` 正是这两个值。
///
/// **用 `get` 而不是 `resolve`**：`get` 只读一次进程内的登记表，`resolve` 会把 SYS 库整棵树
/// 扫一遍。看门狗事件路径每隔几秒就会走到这里（缓存未命中时），在它背后藏一次全树扫
/// 不能接受——`mdb_membership::get` 自己的文档也把这条定成纪律。拿不到就回 `None`，
/// 由调用方维持退化行为：`run_cli` 里 `init_mdb` 失败只告警不中止，「没登记」是真会
/// 发生的状态，不是兜底摆设。
///
/// `get` 按 `(project, mdb)` 精确匹配，所以「请求带的 MDB 名与配置里的不是同一个」自然
/// 回 `None`。这一档必须落空：拿配置那个 MDB 的名单去服务另一个 MDB 的请求，正是
/// `manual_update::update_scope` 明文要挡的「界面显示一个 MDB 的范围、服务端跑另一个」。
fn file_side_declared(name: &str) -> Option<Vec<u32>> {
    let project = aios_core::get_db_option().project_name.clone();
    let membership = super::mdb_membership::get(&project, name)?;
    Some(
        membership
            .element_databases()
            .map(|database| database.dbnum)
            .collect(),
    )
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
        let fetched = Self::fetch(&name).await;
        Self::finish(name, fetched, false)
    }

    /// [`Self::resolve`] 的看门狗事件版：名单几乎不变，事件却按 mtime 轮询源源
    /// 不断，先吃缓存（TTL 内直接命中，SYS meta 落库时被显式失效）；查询失败且有
    /// 同名暖缓存时**放行并告警**——瞬时的连接故障不该把整批文件事件丢掉。
    /// 配置错误（MDB 名不存在）不吃缓存也不进缓存，照常上抛，见 [`Self::finish`]。
    pub async fn resolve_cached(mdb: &str) -> anyhow::Result<Self> {
        let name = aios_core::helper::to_e3d_name(mdb).into_owned();
        let ttl = scope_cache_ttl();
        if ttl.is_zero() {
            // 缓存被配置关掉：与 resolve 完全同义。
            let fetched = Self::fetch(&name).await;
            return Self::finish(name, fetched, false);
        }
        if let Some(hit) = cache_get(&name, Some(ttl)) {
            return Ok(hit);
        }
        let fetched = Self::fetch(&name).await;
        Self::finish(name, fetched, true)
    }

    /// 当前 MDB 的范围来自 SYS 源文件，与房间预加载共用成员解析器。
    /// 每次 fresh 扫描重新解析，事件路径仍由 TTL 和 SYS 更新失效控制频率。
    /// 历史 MDB 表即使非空，也不能覆盖当前源声明；源读取失败直接返回错误。
    /// 查询其他 MDB 的旧管理入口暂时保留持久层解析。
    async fn fetch(name: &str) -> anyhow::Result<(Vec<String>, Vec<u32>)> {
        let option = aios_core::get_db_option();
        if name == aios_core::helper::to_e3d_name(&option.mdb_name).as_ref() {
            let started = Instant::now();
            let membership = super::mdb_membership::resolve(option, &option.project_name, name)?;
            // 保留所有声明成员，包括缺件库；缺件是初始化阻断，不是缩小范围的依据。
            let dbnos = membership
                .element_databases()
                .map(|database| database.dbnum)
                .collect::<Vec<_>>();
            println!(
                "[ams-phase] phase=mdb_scope project={} mdb={name} source=sys-file dbnums={} elapsed_ms={}",
                option.project_name,
                dbnos.len(),
                started.elapsed().as_millis()
            );
            super::mdb_membership::install(membership);
            return Ok((vec![name.to_owned()], dbnos));
        }
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::distinct(SELECT VALUE NAME FROM MDB);\nRETURN {MDB_DBNOS};"
            ))
            .bind(("mdb", name.to_owned()))
            .bind((
                "element_db_types",
                vec![
                    u8::from(DBType::DESI).to_string(),
                    super::mdb_membership::ISOD_STYP.to_string(),
                ],
            ))
            .await?;
        let known: Vec<String> = response.take(0)?;
        let dbnos: Vec<u32> = response.take(1)?;
        Ok((known, dbnos))
    }

    /// 把查询结果解释成范围（纯函数部分）。唯一的错误出口是「库里有 MDB、
    /// 但没有叫这个名字的」——配置错误，要人修。
    ///
    /// `from_files` 是[文件侧声明][file_side_declared]，**只在库侧一条 MDB 都没有时**才会被
    /// 求值（所以是 `FnOnce` 而不是取好的值：常态路径不该为它付任何代价）。库侧非空的三个
    /// 出口一个字不看它——那三档库里是有内容的，拿文件侧去盖就成了「两个来源不一致时听
    /// 文件的」，那是另一个决定。
    fn interpret(
        name: String,
        known: Vec<String>,
        dbnos: Vec<u32>,
        from_files: impl FnOnce() -> Option<Vec<u32>>,
    ) -> anyhow::Result<Self> {
        let mut desi: BTreeSet<u32> = dbnos.into_iter().collect();

        let warning = if !desi.is_empty() {
            None
        } else if known.is_empty() {
            // 空表不算回答：`init_mdb` 跑过但这个 MDB 一个设计库都没声明时，退化那句话
            // 仍然是对的，而「取自文件、0 个设计库」只会让人以为文件侧也答不出来。
            match from_files().filter(|declared| !declared.is_empty()) {
                Some(declared) => {
                    desi = declared.into_iter().collect();
                    Some(format!(
                        "库侧 MDB 表还是空的（SYS meta 尚未摄入），本轮执行范围改取自 SYS 库文件里 \
                         {name} 的声明：{} 个设计库。SYS meta 一落库即自动切回库侧名单",
                        desi.len()
                    ))
                }
                None => Some(format!(
                    "库里还没有任何 MDB——存放 MDB/CURD 的 SYS meta 库尚未解析，\
                     文件侧也没有 {name} 的已登记声明可用。\
                     本期只解析 SYS meta 把成员名单建起来，设计库一个都不跑；\
                     完成后再跑一次即可拿到真正的执行范围"
                )),
            }
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

    /// 解释查询结果并维护缓存。三种出口：
    ///
    /// - 查询成功 → 解释。配置错误（名字打错）**清缓存**并上抛——陈旧名单会把
    ///   改名后的现实装成没事；成功则写缓存并返回。
    /// - 查询失败（基础设施）且 `stale_fallback` → 有同名缓存（不限时）就放行并
    ///   告警，没有就上抛（冷缓存维持 fail-closed：宁可这轮不跑）。
    /// - 查询失败且不许回退（fresh 路径：启动重扫 / 手动触发 / 周期对账）→ 上抛。
    fn finish(
        name: String,
        fetched: anyhow::Result<(Vec<String>, Vec<u32>)>,
        stale_fallback: bool,
    ) -> anyhow::Result<Self> {
        match fetched {
            // `name` 只 clone 不移动：下面的 `Err` 分支还要用它取缓存与拼错误。
            Ok((known, dbnos)) => {
                match Self::interpret(name.clone(), known, dbnos, || file_side_declared(&name)) {
                    Ok(scope) => {
                        cache_store(&scope);
                        Ok(scope)
                    }
                    Err(config_error) => {
                        invalidate_scope_cache();
                        Err(config_error)
                    }
                }
            }
            Err(infra_error) => {
                if stale_fallback {
                    if let Some(stale) = cache_get(&name, None) {
                        let msg = format!(
                            "解析 MDB {name} 的执行范围失败，暂用缓存名单放行（{} 个 DESI 库）。\
                             缓存会在 SYS meta 批次落库或下一次成功解析时刷新: {infra_error:#}",
                            stale.desi.len()
                        );
                        log::warn!("{msg}");
                        eprintln!("{msg}");
                        return Ok(stale);
                    }
                }
                Err(infra_error)
            }
        }
    }

    /// 直接给一份声明名单，只给测试用：`resolve` 要连真库，而范围门的调用方
    /// （`in_scope_with` 等）散在别的模块里，构造不出 `UpdateScope`。
    #[cfg(test)]
    pub(crate) fn for_tests(mdb: &str, desi: &[u32]) -> Self {
        Self {
            mdb: mdb.to_string(),
            desi: desi.iter().copied().collect(),
            unrestricted: false,
            warning: None,
        }
    }

    /// 不设限。按 dbnum 点名的入口用它——回归工具与按需初始化的调用方已经自己
    /// 决定了要哪个库，再套一层 MDB 门只会把点名挡掉。同理，`watch_dbnums` /
    /// `--debug-dbnum` 那两道**监听**门对它也不生效（`in_scope_with` 按
    /// [`UpdateScope::is_unrestricted`] 放行）：那两道门收窄的是 watcher 自己扫出来的活，
    /// 点名的库不是扫出来的。
    pub fn unrestricted() -> Self {
        Self {
            mdb: String::new(),
            desi: BTreeSet::new(),
            unrestricted: true,
            warning: None,
        }
    }

    /// 这是不是按 dbnum 点名的那种不设限范围（[`UpdateScope::unrestricted`]）。
    pub fn is_unrestricted(&self) -> bool {
        self.unrestricted
    }

    /// 范围没解出设计库名单时的那句话，调用方要原样放进自己的 warnings。
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// 这个库进不进本期执行范围。
    ///
    /// SYS meta（SYST / DICT / COMM / MISC / GLB / GLOB）**不受 MDB 门控**：MDB 的成员名单本身
    /// 就存在这些库里，[`UpdateScope::resolve`] 读的就是它们。把它们挡在外面，
    /// 以后往 MDB 里加一个库，范围表永远看不见这个新成员——范围定义会自己冻住。
    ///
    /// 除此之外只认 DESI / ISOD，且必须是本 MDB 声明过的。CATA 文件只进入
    /// `cata_closure` 的依赖身份清单，供模型生成时按引用读取；它们不进入初始化
    /// 数据批次，也不全量解析属性进持久层（ADR-058 D1）。显式点名入口仍可用
    /// [`UpdateScope::unrestricted`] 执行运维基线。
    pub fn admits(&self, db_type: &str, dbnum: u32) -> bool {
        if COLD_START_DB_TYPES.contains(&db_type) {
            return true;
        }
        if self.unrestricted {
            return true;
        }
        matches!(db_type, "DESI" | "ISOD") && self.desi.contains(&dbnum)
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

    #[tokio::test]
    #[ignore = "requires AMS source files and an isolated DB_OPTION_FILE; no database connection"]
    async fn ams_mdb_initialization_scope_without_database() -> anyhow::Result<()> {
        let option = aios_core::get_db_option();
        anyhow::ensure!(
            option.project_name == "AvevaMarineSample"
                && option.mdb_name.trim_start_matches('/') == "ALL",
            "requires AMS /ALL"
        );
        let started = Instant::now();
        let scope = UpdateScope::resolve("ALL").await?;
        let membership = super::super::mdb_membership::get(&option.project_name, "/ALL")
            .expect("scope and room initialization share the installed source declaration");
        let expected: BTreeSet<_> = membership.element_databases().map(|db| db.dbnum).collect();
        assert_eq!(scope.declared_desi().collect::<BTreeSet<_>>(), expected);
        let design: BTreeSet<_> = membership
            .of_type(super::super::mdb_membership::DESI_STYP)
            .map(|db| db.dbnum)
            .collect();
        let missing = membership
            .unresolved()
            .filter(|db| db.styp == super::super::mdb_membership::DESI_STYP)
            .collect::<Vec<_>>();
        println!("AMS_MDB_DESI declared={} missing={missing:?}", design.len());
        // ADR-073 D8：缺文件的 DESI 不再让房间输入整份 Err——它们按号进 `missing_design`，
        // 冻结源恰好覆盖「声明的 DESI 减去缺文件的」。
        let frozen = crate::fast_model::room_topology::latest_frozen_sources()
            .await
            .expect("missing DESI files degrade the room reference, they do not fail it");
        let missing_dbnums: BTreeSet<u32> = missing.iter().map(|db| db.dbnum).collect();
        assert_eq!(
            frozen
                .missing_design
                .iter()
                .map(|(dbnum, _)| *dbnum)
                .collect::<BTreeSet<_>>(),
            missing_dbnums,
            "every declared DESI without a file is named for degradation"
        );
        assert_eq!(
            frozen
                .sources
                .iter()
                .map(|source| source.dbnum() as u32)
                .collect::<BTreeSet<_>>(),
            design
                .difference(&missing_dbnums)
                .copied()
                .collect::<BTreeSet<_>>(),
            "room preload must cover precisely the MDB DESI files that exist"
        );
        if !missing.is_empty() {
            println!("AMS_MDB_ROOM_DEGRADED missing={missing_dbnums:?}");
        }
        assert!(
            scope.admits("DESI", 7015),
            "an unresolved model dependency must not remove a declared database"
        );
        assert!(
            !scope.admits("DESI", 3001),
            "project directory contents do not extend MDB scope"
        );
        println!(
            "AMS_MDB_SCOPE dbnums={expected:?} elapsed_ms={}",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    fn scope(desi: &[u32]) -> UpdateScope {
        UpdateScope {
            mdb: "/ALL".into(),
            desi: desi.iter().copied().collect(),
            unrestricted: false,
            warning: None,
        }
    }

    /// 正常初始化只认 MDB 声明的设计库；CATA 保留在按需依赖清单里。
    #[test]
    fn only_desi_declared_by_this_mdb_gets_in() {
        let scope = scope(&[7997, 8000]);
        assert!(scope.admits("DESI", 8000));
        assert!(scope.admits("ISOD", 8000));
        assert!(!scope.admits("DESI", 3001), "MDB 外的设计库不进范围");
        assert!(!scope.admits("CATA", 8000), "CATA 不进入全量数据初始化");
    }

    /// e3d-io 的属性面把显式数值保存成字符串，旧存量又可能仍是数字；范围查询必须
    /// 先归一类型，否则 `"1" != 1` 会把完整 CURD 误判成零个 DESI。
    #[test]
    fn mdb_scope_query_accepts_string_and_numeric_styp_storage() {
        assert!(MDB_DBNOS.contains("type::string(STYP) IN $element_db_types"));
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
    /// 预览对所有人直接失败。连**配置的**已解析 AMS 库（8009 或 testbed），
    /// `--ignored resolves_the_real_mdb` 单跑。
    ///
    /// 断言分两层（批次 2 决策 4）：结构断言对任何解析过 /ALL 的靶成立；
    /// 「恰好 29 个 DESI」是 8009 生产库的快照语义，写死会让一切别的靶必红
    /// （批次 1 实测），改由 `AIOS_EXPECT_DESI_COUNT` 门控——8009 批次清单里
    /// 带上它即恢复原断言力。
    #[tokio::test]
    #[ignore = "需要本地已解析 AMS 的 SurrealDB（配置目标；精确数断言由 AIOS_EXPECT_DESI_COUNT 门控）"]
    async fn resolves_the_real_mdb_declaration() {
        aios_core::init_test_surreal().await;
        let scope = UpdateScope::resolve("ALL").await.expect("解出 /ALL 的范围");

        assert_eq!(scope.mdb(), "/ALL", "名字要补上前导斜杠");
        let declared: Vec<u32> = scope.declared_desi().collect();
        assert!(
            !declared.is_empty(),
            "/ALL 的 CURD 里必须解出至少一个 DESI 库号"
        );
        if let Ok(expected) = std::env::var("AIOS_EXPECT_DESI_COUNT") {
            let expected: usize = expected.parse().expect("AIOS_EXPECT_DESI_COUNT 要是数字");
            assert_eq!(
                declared.len(),
                expected,
                "/ALL 的 CURD DESI 库号数与靶声明不符"
            );
        }
        // 配置声明的手动库号必须都在范围内——这是「范围来自 MDB 声明」与部署
        // 配置互证的结构断言，不依赖具体靶的库数。
        for dbnum in aios_core::get_db_option().manual_db_nums.iter().flatten() {
            assert!(
                scope.admits("DESI", *dbnum),
                "配置的 manual_db_nums 成员 {dbnum} 应在 /ALL 范围内"
            );
        }
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

    // ---- 库侧为空时回落到文件侧声明（2026-09-07：YCYK-E3D 新项目必须跑两趟的对症）----
    //
    // 计划：docs/plans/2026-09-07-update-scope-file-side-mdb-fallback-plan.md
    // 这四个用例直接调 `interpret`：回落的注入点是它那个 `FnOnce` 入参，走 `finish` 就得
    // 造一份进程级 registry，而 `finish` 那几个用例正是本次不想碰的东西。

    /// 库侧一条 MDB 都没有、文件侧有声明 → 用文件侧的，别再空跑一趟。
    #[test]
    fn an_empty_store_falls_back_to_the_file_side_declaration() {
        let scope = UpdateScope::interpret("/SLYK".into(), vec![], vec![], || {
            Some(vec![5525, 7015, 5107])
        })
        .expect("回落不是错误出口");

        assert_eq!(
            scope.declared_desi().collect::<Vec<_>>(),
            [5107, 5525, 7015]
        );
        assert!(scope.admits("DESI", 5525));
        assert!(!scope.admits("DESI", 8000), "回落的是那份声明，不是不设限");
        let warning = scope.warning().expect("换了口径就必须出声");
        assert!(
            warning.contains("SYS 库文件") && warning.contains("/SLYK"),
            "要说清名单这一轮取自哪儿、是哪个 MDB：{warning}"
        );
    }

    /// 文件侧没登记（`init_mdb` 没跑过 / 失败了 / MDB 名对不上）→ 与引入回落之前逐字相同。
    #[test]
    fn without_a_file_side_declaration_it_still_bootstraps() {
        let scope =
            UpdateScope::interpret("/SLYK".into(), vec![], vec![], || None).expect("不该 bail");

        assert_eq!(scope.declared_desi().count(), 0);
        assert!(
            scope.admits("SYST", 8191),
            "SYS meta 照常解析，把名单建起来"
        );
        assert!(!scope.admits("DESI", 5525), "设计库一个都不跑");
        assert!(
            scope.warning().is_some_and(|w| w.contains("再跑一次")),
            "退化那句话要原样留着"
        );
    }

    /// 空表不是回答。`init_mdb` 跑过、但这个 MDB 一个设计库都没声明时，说「取自文件、
    /// 0 个设计库」只会让人以为文件侧也答不出来，退化那句话才是对的。
    #[test]
    fn an_empty_file_side_declaration_is_not_an_answer() {
        let scope = UpdateScope::interpret("/SLYK".into(), vec![], vec![], || Some(vec![]))
            .expect("不该 bail");

        assert_eq!(scope.declared_desi().count(), 0);
        assert!(scope.warning().is_some_and(|w| w.contains("再跑一次")));
    }

    /// **只动库侧全空那一档。** 库里有 MDB 时，另外三个出口一个字不看文件侧——那三档
    /// 库里是有内容的，拿文件侧去盖就成了「两个来源不一致时听文件的」，那是另一个决定。
    #[test]
    fn a_non_empty_store_never_consults_the_file_side() {
        let loud = || -> Option<Vec<u32>> { panic!("库侧非空时不许去问文件侧") };

        // 有这个 MDB、但它没声明 DESI：照旧是告警，不是回落。
        let scope = UpdateScope::interpret("/SLYK".into(), vec!["/SLYK".into()], vec![], loud)
            .expect("不该 bail");
        assert_eq!(scope.declared_desi().count(), 0);
        assert!(scope.warning().is_some_and(|w| w.contains("CURD")));

        // 名字打错：照旧 bail，不许被文件侧救回来。
        let err = UpdateScope::interpret("/SLYK".into(), vec!["/OTHER".into()], vec![], loud)
            .expect_err("名字不存在必须报错");
        assert!(
            err.to_string().contains("/OTHER"),
            "错误要列出实际有的：{err}"
        );

        // 库侧解出了名单：连问都不问。
        let scope = UpdateScope::interpret("/SLYK".into(), vec!["/SLYK".into()], vec![8000], loud)
            .expect("正常范围");
        assert!(scope.admits("DESI", 8000));
        assert!(scope.warning().is_none());
    }

    // ---- 范围缓存（2026-08-06：连接抖动把文件事件整批丢掉的对症）----
    //
    // 缓存是进程级单槽，下面的用例都要先清场并全程持锁串行，防止并行测试互踩。

    static CACHE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fetched(known: &[&str], dbnos: &[u32]) -> anyhow::Result<(Vec<String>, Vec<u32>)> {
        Ok((
            known.iter().map(|s| s.to_string()).collect(),
            dbnos.to_vec(),
        ))
    }

    /// 事件路径的核心承诺：SUL_DB 瞬时不可用时，暖缓存放行、fresh 路径照常上抛。
    #[test]
    fn a_warm_cache_admits_events_through_an_infra_outage() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        invalidate_scope_cache();

        // 一次成功解析把缓存焐热。
        let scope =
            UpdateScope::finish("/T".into(), fetched(&["/T"], &[7999]), false).expect("成功解析");
        assert!(scope.admits("DESI", 7999));

        // 基础设施错误 + 允许回退（事件路径）→ 暖缓存放行。
        let stale = UpdateScope::finish("/T".into(), Err(anyhow::anyhow!("断连")), true)
            .expect("暖缓存必须放行");
        assert!(stale.admits("DESI", 7999));
        assert!(!stale.admits("DESI", 3001), "放行的是缓存名单，不是不设限");

        // 同样的错误、不许回退（启动重扫 / 手动 / 周期对账）→ 原样上抛。
        assert!(
            UpdateScope::finish("/T".into(), Err(anyhow::anyhow!("断连")), false).is_err(),
            "fresh 路径不吃缓存"
        );
    }

    /// 冷缓存维持 fail-closed：宁可这轮不跑，也不能凭空造一份范围。
    #[test]
    fn a_cold_cache_still_fails_closed() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        invalidate_scope_cache();
        assert!(UpdateScope::finish("/T".into(), Err(anyhow::anyhow!("断连")), true).is_err());
    }

    /// 名字打错是配置错误：不吃缓存、还要把缓存清掉——陈旧名单会把改名后的现实
    /// 装成没事，后续连断连都探测不到。
    #[test]
    fn an_unknown_mdb_is_a_config_error_and_clears_the_cache() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        invalidate_scope_cache();

        UpdateScope::finish("/T".into(), fetched(&["/T"], &[7999]), false).expect("焐热");
        let err = UpdateScope::finish("/T".into(), fetched(&["/OTHER"], &[]), true)
            .expect_err("名字不存在必须报错，即便有暖缓存");
        assert!(err.to_string().contains("/T"), "错误要点名: {err}");

        // 缓存已被清掉：再遇断连没有可放行的东西。
        assert!(UpdateScope::finish("/T".into(), Err(anyhow::anyhow!("断连")), true).is_err());
    }

    /// 缓存按 MDB 名配对：别人的名单救不了你。
    #[test]
    fn the_cache_only_serves_its_own_mdb() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        invalidate_scope_cache();

        UpdateScope::finish("/A".into(), fetched(&["/A"], &[1]), false).expect("焐热 /A");
        assert!(
            UpdateScope::finish("/B".into(), Err(anyhow::anyhow!("断连")), true).is_err(),
            "/B 的失败不能拿 /A 的名单放行"
        );
    }

    /// TTL 语义：限时命中按新鲜度判，回退取用不限时。
    #[test]
    fn cache_freshness_is_only_enforced_for_direct_hits() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        invalidate_scope_cache();

        UpdateScope::finish("/T".into(), fetched(&["/T"], &[7999]), false).expect("焐热");
        assert!(
            cache_get("/T", Some(Duration::ZERO)).is_none(),
            "零 TTL 下直接命中永远失效"
        );
        assert!(
            cache_get("/T", None).is_some(),
            "回退取用不限时——陈旧也好过丢事件"
        );
    }
}
