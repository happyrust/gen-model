# 当前修改检查（2026-09-12）

## 已验证

- `cargo check --lib` 通过。
- 完整库测试：1985 通过，15 失败，失败项已记录在本轮终端输出。
- `SessionPage.machine` 适配已修复为 `SessionPage.user`。
- `MdbRouteDirectory::route_refno` 已改为调用 `e3d_io::RefNo::dbno()`，避免本地位运算漂移。

## 当前阻塞

1. `DirectTreeService::open` 已不再要求预热骨架，元素访问按目标库惰性 pin；只有显式 `skeleton()` 元数据兼容接口才会按需建立旧快照。
2. `ElementAttributeService::open` 已不再读取 `MdbMemoryStore`，改用当前 MDB 的 `MdbRouteDirectory` 和独立 UDA catalog；底层仍是 DirectStore，尚未切到共享 DbSet 池。
3. `DirectStore::DirectStoreError::UnresolvedRef0` 已删除；`CataDbLocator::resolve_ref0` 仅保留在目录闭包扫描接口，不再参与 DirectStore DESI 路由。
4. 目前没有 `refno_native_guards` / `refno_native_api_contract` 测试目标，计划中的源码护栏尚未落库。
5. 库测试中 `an_unresolvable_ref0_is_an_error_not_a_guess` 与新位解码语义冲突，需要重写为“dbnum 路由后目标库缺失/不匹配”的测试。

## 本轮新增结果

- `/dbnums` 的 bootstrap/read-through 分支已改为直接读取 `MdbRouteDirectory`，不会调用 `service.skeleton()`。
- 路由目录现在提供文件头 dbnum/type 校验、最新会话号和文件身份 fingerprint；树服务用 fingerprint 判断刷新。
- 树、属性服务的普通打开路径均不再要求预热骨架。
- `cargo check --lib` 通过；handlers 测试 29 通过、1 个已有前端资源计数断言失败。

## 下一步

先把树和属性服务统一到一个 `DbSet` 视图，删除全量 pin/locator 依赖；然后补生产源码护栏、单飞和定向失效测试，最后重跑 AMS HTTP、SAVEWORK/恢复及打包验收。
