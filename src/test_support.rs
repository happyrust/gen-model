//! 测试侧的一次性初始化入口。
//!
//! 这里要同时解决两个坑，两个都只在「一个测试二进制里跑多条真库测试」时暴露：
//!
//! 1. **`Already connected`**：`aios_core::init_test_surreal()` 每调一次就对进程级
//!    `SUL_DB` 再做一次 `connect`。SDK 对同一实例的第二次 `connect` 必返
//!    `Already connected`，所以第二个测试起全红——`--test-threads=1` 也救不了，
//!    问题不在并发而在「单例只能连一次」。
//!
//! 2. **`sending into a closed channel`**：`connect` 会在当前 runtime 上 spawn
//!    连接层的 router 任务。`#[tokio::test]` 默认一条测试一个 current_thread
//!    runtime，那条测试跑完 runtime 一落，router 任务跟着没，`SUL_DB` 就成了一个
//!    通道已关闭的空壳。表现是**第一条测试绿、后面每条都撞 closed channel**。
//!    同一个机制在 `web_service::review::platform::tests.rs:49` 有过完整记录。
//!
//! 两个坑的解法是同一个：连接只做一次，而且**做在一个永不 drop 的进程级 runtime
//! 上**。那个 runtime 由 [`engine_runtime`] 建，另起一条驻留线程一直 `block_on`
//! 空 future，driver 就一直在跑，router 任务不会被回收。各测试自己的 runtime 只
//! 往 router 的通道上发消息，跨 runtime 收发没有问题。
//!
//! **所有需要真库连接的测试都必须走 [`init_test_db`]**，不要再直接调
//! `aios_core::init_test_surreal()`，也不要直接对 `SUL_DB` 调 `connect`。

use std::sync::OnceLock;

use aios_core::options::DbOption;
use tokio::sync::OnceCell;

/// 承载 `SUL_DB` 连接层 router 的进程级 runtime，**永不 drop**。
static ENGINE_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// 驻留线程的启动闸，保证只起一条。
static KEEPALIVE: OnceLock<()> = OnceLock::new();

/// 首次初始化成功后缓存的 `DbOption`。
///
/// 只缓存成功结果：初始化失败（配置缺、8009 没起）不写缓存，下一个测试还能重试
/// 并报出自己的错误，而不是继承一个陈旧的失败。
static TEST_DB_OPTION: OnceCell<DbOption> = OnceCell::const_new();

/// 取进程级 engine runtime，首次调用时建好并起驻留线程。
///
/// 驻留线程里 `block_on(pending())` 永不返回，runtime 的 IO / timer driver 因此
/// 一直在跑，router 任务不会被回收。进程退出时线程被强杀，不需要清理。
fn engine_runtime() -> &'static tokio::runtime::Runtime {
    let runtime = ENGINE_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("测试 engine runtime 应当能建起来")
    });
    KEEPALIVE.get_or_init(|| {
        std::thread::spawn(move || {
            runtime.block_on(std::future::pending::<()>());
        });
    });
    runtime
}

/// 幂等的测试库初始化：首次在 engine runtime 上真连，之后复用同一个 `DbOption`。
///
/// 返回 `DbOption` 的克隆，调用方可以随便改自己的副本。
pub async fn init_test_db() -> Result<DbOption, aios_core::error::HandleError> {
    TEST_DB_OPTION
        .get_or_try_init(|| async {
            // 连接必须在 engine runtime 上建：那里才是 router 任务的归宿。
            // `get_or_try_init` 保证这段只跑一次，不存在并发抢连。
            let handle = engine_runtime().handle().clone();
            match handle
                .spawn(async { aios_core::init_test_surreal().await })
                .await
            {
                Ok(result) => result,
                Err(join_error) => panic!("测试库初始化任务没能跑完: {join_error}"),
            }
        })
        .await
        .cloned()
}

/// [`init_test_db`] 的 panic 版本，给那些原来直接 `.expect("连接配置库")` 的测试用。
///
/// `context` 会进 panic 消息，便于定位是哪个测试连不上。
pub async fn init_test_db_or_panic(context: &str) -> DbOption {
    match init_test_db().await {
        Ok(option) => option,
        Err(err) => panic!("{context}: {err:?}"),
    }
}

/// 手动更新类 live 测试要打的靶项目。
///
/// 原来这些测试都写 `std::env::var("AIOS_MANUAL_UPDATE_PROJECT").expect(...)`，不配
/// 环境变量就直接 panic。而同一个测试里 `AIOS_MANUAL_UPDATE_DBNUM` 早就有默认值
/// （`unwrap_or(7998)`）——同一组参数一半有默认一半没有，只是历史遗留，不是设计。
///
/// 默认值取配置里的 `project_name`（`DbOption.toml` 的那一项），也就是这套测试一直
/// 在用的那个项目。显式配了环境变量仍然优先，跨项目调试照旧能覆盖。
///
/// 连库失败会 panic 并把原因带出来——调用方都紧跟在 [`init_test_db`] 之后，那里已经
/// 成功过一次，这里再失败就是配置真坏了，不该静默兜底。
pub async fn manual_update_project() -> String {
    if let Ok(value) = std::env::var("AIOS_MANUAL_UPDATE_PROJECT") {
        return value;
    }
    let option = init_test_db_or_panic("读取项目名以推导 AIOS_MANUAL_UPDATE_PROJECT 默认值")
        .await;
    option.project_name
}
