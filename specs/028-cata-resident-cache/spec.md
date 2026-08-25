# 028 CATA 常驻解析缓存规格

## 目标

在 RocksDB 权威数据落盘后跨页复用不可变 SCOM 解析结果，减少 AMS 8000 初始化和稳态更新的
目录查询与表达式解析，同时保证 staging 隔离、提交一致性、资源上限和错误可重试性。

## 功能要求

1. 每次 SCOM 加载必须显式携带 `Committed(AuthorityId)` 或 `StagedPage` 读域；只有前者准入。
2. committed 缓存返回 `Arc<ScomInfo>`，同 authority/key/epoch 的并发 miss 只执行一次 loader。
3. 数据库错误可重试，目录数据缺陷写 `geom_error`；任何失败不得形成常驻负缓存。
4. 权威提交成功后按实际依赖 selective invalidation；未知关系、删除端点不全或依赖覆盖不全
   必须 full-authority。提交失败、abort 和 drop 不清 committed cache、不推进 epoch。
5. 缓存受字节数、条目数和单项大小限制；容量为零时结果与 RocksDB + 页内缓存等价。
6. 不使用 Legacy、长期 `kv-mem`、SCOM redb 或第二个几何 Semaphore。

## 验收标准

- 100 个同键请求只有一个 loader；取消、失败、跨 epoch、跨 authority 和 staging 隔离测试通过。
- 默认驻留不超过 32 MiB/16,384 条，单项最多 4 MiB，回收低水位为上限 90%。
- 三组 cache-on/off 独立空 RocksDB 的模型、mesh SHA-256、AABB、布尔、规范化表和
  `geom_error` 语义集合一致。
- AMS 8000 总耗时中位数不超过 848.728 秒，目标 808.312 秒；cache-on 相对 off 退化不超过
  3%，峰值 Working Set 不超过 274,027,520 B。
- 完成边界使用初始化完成事件，不等待 `pending=0`。

## 停止条件

staging 数据进入 committed cache、依赖不完整而未 full fallback、结果不等价、内存门超限，
或错误未进入既定收口时，保持常驻缓存默认关闭。
