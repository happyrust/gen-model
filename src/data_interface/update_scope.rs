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

    /// 只做那一趟 SurrealQL：这里的任何错误都是**基础设施**错误（连接断、
    /// 服务器重启、形状解不出来），与「名字打错」这类配置错误分属两类，
    /// 缓存回退只对前者生效。
    async fn fetch(name: &str) -> anyhow::Result<(Vec<String>, Vec<u32>)> {
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::distinct(SELECT VALUE NAME FROM MDB);\nRETURN {MDB_DBNOS};"
            ))
            .bind(("mdb", name.to_owned()))
            .bind(("db_type", u8::from(DBType::DESI)))
            .await?;
        let known: Vec<String> = response.take(0)?;
        let dbnos: Vec<u32> = response.take(1)?;
        Ok((known, dbnos))
    }

    /// 把查询结果解释成范围（纯函数部分）。唯一的错误出口是「库里有 MDB、
    /// 但没有叫这个名字的」——配置错误，要人修。
    fn interpret(name: String, known: Vec<String>, dbnos: Vec<u32>) -> anyhow::Result<Self> {
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
            Ok((known, dbnos)) => match Self::interpret(name, known, dbnos) {
                Ok(scope) => {
                    cache_store(&scope);
                    Ok(scope)
                }
                Err(config_error) => {
                    invalidate_scope_cache();
                    Err(config_error)
                }
            },
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
    /// ADR-025 将 CATA 提升为正式 Catalogue 阶段；它不受 MDB 的 DESI 清单约束，
    /// 但仍须经过跨项目优先级、重复身份和阶段屏障裁决。
    pub fn admits(&self, db_type: &str, dbnum: u32) -> bool {
        if COLD_START_DB_TYPES.contains(&db_type) {
            return true;
        }
        if self.unrestricted {
            return true;
        }
        db_type == "CATA" || (db_type == "DESI" && self.desi.contains(&dbnum))
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

    /// Design 只认 MDB 声明的 DESI；Catalogue 独立放行 CATA。
    #[test]
    fn only_desi_declared_by_this_mdb_gets_in() {
        let scope = scope(&[7997, 8000]);
        assert!(scope.admits("DESI", 8000));
        assert!(!scope.admits("DESI", 3001), "MDB 外的设计库不进范围");
        assert!(
            scope.admits("CATA", 8000),
            "ADR-025：CATA 进入 Catalogue 阶段"
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
