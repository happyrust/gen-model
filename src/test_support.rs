//! 测试侧的一次性初始化入口。
//!
//! `aios_core::init_test_surreal()` 每次调用都会对进程级 `SUL_DB` 再做一次
//! `connect`。SDK 对同一实例的第二次 `connect` 必返 `Already connected`，所以
//! 一个测试二进制里只要有两个测试各自调用它，第二个之后全红——`--test-threads=1`
//! 也救不了，因为问题不在并发而在「单例只能连一次」。
//!
//! 这里把首次结果缓存进 [`DbOption`]，后续调用直接复用。**所有需要真库连接的
//! 测试都必须走 [`init_test_db`]**，不要再直接调 `aios_core::init_test_surreal()`。

use aios_core::options::DbOption;
use tokio::sync::OnceCell;

/// 首次初始化成功后缓存的 `DbOption`。
///
/// 只缓存成功结果：初始化失败（配置缺、8009 没起）不写缓存，下一个测试还能重试
/// 并报出自己的错误，而不是继承一个陈旧的失败。
static TEST_DB_OPTION: OnceCell<DbOption> = OnceCell::const_new();

/// 幂等的测试库初始化：首次真连，之后复用同一个 `DbOption`。
///
/// 返回 `DbOption` 的克隆，调用方可以随便改自己的副本。
pub async fn init_test_db() -> Result<DbOption, aios_core::error::HandleError> {
    TEST_DB_OPTION
        .get_or_try_init(|| async { aios_core::init_test_surreal().await })
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
