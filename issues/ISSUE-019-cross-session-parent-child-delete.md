# Issue #19：跨会话父子删除被最终 OWNER 状态误判

## Issue 信息

- **类型**：Bug
- **优先级**：High
- **状态**：Fixed / Testing
- **创建日期**：2026-08-10
- **相关模块**：`pdms_io` 历史版本解析、增量采集、删除模型计划
- **测试库**：dbnum 8000，sesno `24 → 25 → 26`

## 问题描述

同一增量窗口内，sesno 25 先删除 EQUI `24384/24778` 下的 BOX
`24384/24779`，sesno 26 再删除该 EQUI。直接从 sesno 26 最终文件采集
`25..=26` 时，旧实现使用最终 OWNER 状态判断历史元素是否存在，导致父 EQUI
在 sesno 25 被提前判为 Deleted，同时丢失 BOX 的真实删除操作。

### 修复前

```text
sesno 25: 24384_24778 Deleted       # 错误提前删除
sesno 26: 24384_24775 Modified
sesno 26: 24384_24778 Deleted
missing : 24384_24779 Deleted @ 25
```

### 预期结果

```text
sesno 25: 24384_24778 EQUI Modified(CACHID, children)
sesno 25: 24384_24779 BOX Deleted
sesno 26: 24384_24775 ZONE Modified(children)
sesno 26: 24384_24778 EQUI Deleted
```

## 根本原因与修复

`pdms_io::PdmsIO::get_refno_operation_status` 原先通过
`auto_get_raw_element(owner)` 读取文件最终索引中的 OWNER。历史会话的存在性因此被
后续会话覆盖。

修复提交 `pdms_io@5c9e00e3c46f7d6f7c548583020b66e0ad23368a`：

- 新增按会话边界读取的内部方法 `raw_element_at_or_before`；
- OWNER 成员判断读取目标 sesno 可见的最新版本；
- 以 `(sesno, refno)` 缓存历史原始元素，避免窗口内重复解析；
- 保持 `EleOperationData` 与 `IncrementPipeline::collect_changes` 接口不变。

## 测试夹具

权威夹具位于：

```text
tests/fixtures/issues/issue-019-cross-session-parent-child-delete/
```

`db8000-sesno24-26.zip` 包含三个真实 session-chain 快照。原始数据共
32,397,312 bytes，ZIP 为 4,497,104 bytes。`manifest.json` 和
`SHA256SUMS` 固化文件大小、SHA256、会话号和元素存在状态；`comparisons/` 保存修复
前后可审查的 JSON 对比。

测试只依赖 ZIP：运行时校验归档哈希、拒绝未声明或不安全路径、解压到唯一临时目录，
并再次校验三个原始文件的大小和 SHA256。

## 验证

```powershell
cargo test --test db8000_two_delete_fixture -- --nocapture
cargo test --lib child_delete_then_parent_delete_across_sessions_schedules_only_the_parent -- --nocapture
sigmap validate
```

GitHub Actions 通过 `.github/workflows/windows-tests.yml` 在 pull request 和 `main`
分支自动执行同一组可移植案例。

验收标准：

- 最终文件 `collect_changes(25..=26)` 精确返回四条操作；
- 合并净变化为 BOX Deleted、EQUI Deleted、ZONE Modified；
- 删除调度仅保留父 EQUI 的一次递归 `DeleteCleanup`；
- ZIP 小于 6 MiB，且删除旧原始工作目录后仍能独立回放。

## 风险与回滚

- 风险集中在历史 OWNER 查询次数；会话级缓存限制了额外 I/O。
- 回滚 `gen-model` 的 `pdms_io` rev 和锁文件即可恢复旧解析器；夹具与对比证据继续保留，
  用于确认回滚后问题重新出现。

## 标签

`bug` `incremental-update` `pdms-io` `deletion` `regression-fixture` `dbnum-8000`
