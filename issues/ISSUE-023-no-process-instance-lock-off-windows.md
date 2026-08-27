# Issue #023: 非 Windows 上没有进程单实例锁，部署到 CentOS 7 后可并发启动第二个写入者

## 📋 Issue 信息

- **Issue ID**: #023
- **类型**: Bug 🐛
- **优先级**: Critical 🔴
- **状态**: Open 📝
- **创建日期**: 2026-08-26
- **相关模块**: `src/lib.rs`（`acquire_process_instance_lock`）、`run_app` / `run_cli` 启动序、
  `fast_model/concurrency.rs`、`data_interface/model_concurrency.rs`、`data_interface/staging`

## 🔍 问题描述

`acquire_process_instance_lock` 的文档写着「mutating 管线不允许有第二个进程并发驱动」，
`run_app` 与 `run_cli` 都在连库、清工作单、装空间树、起 watcher 与 worker **之前**调用它，
Python 绑定的 `full_init` 也共用同一把锁。

但完整实现只存在于 `#[cfg(windows)]`。非 Windows 分支是：

```rust
#[cfg(not(windows))]
pub fn acquire_process_instance_lock(_db_option: &DbOption) -> anyhow::Result<()> {
    Ok(())
}
```

配套的单实例测试也是 `#[cfg(all(test, windows))]`，所以 CI 在 Linux 上不会发现这件事。

而部署目标是 CentOS 7（`cargo zigbuild --target x86_64-unknown-linux-gnu.2.17`）。

### 预期行为

第二个进程在启动早期因拿不到项目级独占锁而退出，且退出信息指出持锁者。

### 实际行为

非 Windows 上第二个进程直接放行，成为同一份 dabacon、同一个 SurrealDB、同一批 mesh 文件
的第二个写入者。

## 🔬 问题分析

### 根本原因

单实例保护是平台相关实现，只做了 Windows 的 deny-share 句柄，Unix 侧留了空函数，
且守卫测试同样只在 Windows 下编译，缺口没有任何回归网。

### 影响范围

单实例是**很多不变量的隐含前提**，一起失效：

- `GeometryGate` 是进程内全局：两个实例把实际几何并发从 `geometry_workers` 放大一倍，
  三个实例三倍。ADR-052「额度是唯一限流阀门」在 CentOS 上不成立。
- `model_concurrency` 的 `EFFECTIVE`、延迟窗口、K=1 基线都是进程全局，各算各的。
- ADR-050 的 `model_update_pending` 启动清理：第二个进程启动会清掉第一个进程正在跑的工作单。
- 暂存窗口（ADR-017）的 `StagedExecutor` 串行语义只在进程内成立。
- AABB 项目树文件、mesh 文件可能被两个实例同时写。
- 水位推进与副作用的同事务保证（ADR-001）跨进程无效。

若现场靠 systemd / 容器编排 / 启动脚本保证了单实例，则实际不触发——但**代码本身没有这个保证**，
而它是被当作前提写进注释和多条 ADR 的。

### 相关代码

- `src/lib.rs`：`open_process_instance_lock`、`process_instance_lock_path`、
  `acquire_process_instance_lock`（两个 cfg 分支）、`mod process_instance_lock_tests`
- 调用点：`run_app`、`run_cli`，以及 `python/` 绑定的 `full_init`

## 🛠️ 解决方案

### 方案概述

在 Unix 上用项目目录里的同一个锁文件做 `flock(LOCK_EX | LOCK_NB)`（或 `fcntl` 写锁），
句柄保留到进程结束，语义与 Windows 的 deny-share 对齐；拿不到锁时报错退出并指出持锁者。

### 技术实现

- 锁路径复用现有的 `process_instance_lock_path`（`<project_dir>/.gen-model.instance.lock`），
  两个平台同一个文件名，便于运维辨认。
- 句柄存进现有的进程级 `OnceLock`，保证重复调用幂等（`run_app` 与 `run_cli` 都会调）。
- 失败信息要能回答「谁占着」：把 pid 与项目名写进锁文件内容，冲突时读出来一起报。
- 网络文件系统上 `flock` 语义不可靠，锁文件所在目录若是网络盘要显式告警而不是静默放行
  （静默放行正是本 issue 的形态）。

### 风险评估

- 低。它只在启动早期增加一次失败路径；现有单实例部署不受影响。
- 需要留意：进程被 `SIGKILL` 后 `flock` 由内核释放，不会留下需要人工清理的陈旧锁；
  这一点优于「靠锁文件是否存在判断」的写法，不要退化成后者。

## 🧪 测试验证

### 测试计划

把守卫测试从 `#[cfg(all(test, windows))]` 改成跨平台，并补一条**真子进程**级回归：
父进程拿锁后 spawn 同一个二进制的第二个实例，断言它非零退出且 stderr 含持锁者信息。

### 测试用例

1. 同一进程内重复调用 `acquire_process_instance_lock` 幂等成功。
2. 同一进程内换一个项目名再调用，报「已持有项目 X 的锁」。
3. 子进程在父进程持锁期间启动 → 非零退出；父进程退出后 → 成功启动。
4. 锁文件所在目录不可写 / 不存在时的行为明确（报错，不静默放行）。

### 验证标准

在 Linux 上 case 3 稳定复现「第二个实例起不来」，且该用例在把实现改回空函数时会红。

## 🔄 后续行动

### 立即行动

- [ ] 确认现场 CentOS 部署当前靠什么保证单实例（systemd `Restart=` 策略、容器、还是人工）
- [x] 实现 Unix `flock` 分支并把守卫测试跨平台化（2026-08-27，specs/033 T001）：
      `open_advisory_process_instance_lock` 用 `File::try_lock`（Unix=flock、
      Windows=LockFileEx），Unix 分支薄委托它；锁挂在 open file description 上，
      SIGKILL 后内核回收，没有退化成「看文件存不存在」。冲突时读回持锁者写入的
      project/pid/started_at 一起报。**advisory 函数体在所有平台编译**，守卫测试
      三条改为跨平台（锁路径映射、平台原生打开器拒二开、advisory 路径
      拿-拒-放全流程带「锁被占用」信息断言）——本 issue 的成因正是非 Windows 代码
      在 Windows 机器上零编译零测试。尚欠：真机 Linux 上跑一遍（本机无 zigbuild，
      交叉验证未做），发布前必须补。
- [ ] 补子进程级回归测试（父进程持锁时 spawn 第二个实例，断言非零退出且 stderr
      指出持锁者；适合放 e2e 而不是 lib 单测）

### 预防措施

- [ ] 任何写成「进程内全局即全局」的不变量，注释里注明它依赖单实例锁
- [ ] `#[cfg(windows)]` 的安全性实现一律要求配一个非 Windows 分支的显式决策
      （实现，或明确 `compile_error!`），不接受空 `Ok(())`

## 📚 相关文档

- **ADR-052**：几何并发额度只覆盖 CPU 执行段——本 issue 是它的前置条件
- **specs/033-geometry-execution-domain**：FR-1 直接引用本 issue
- ADR-001（水位是提交承诺）、ADR-011（唯一批次队列）、ADR-017（暂存窗口）、
  ADR-050（进程级模型工作单）——它们的前提都包含单实例

## 🏷️ 标签

bug critical concurrency deployment linux process-lock

---

**发现方式**: 2026-08-26 对模型生成并发做外部模型审核（oracle · GPT-5.6 Sol · Pro thinking，
会话 `model-gen-concurrenc-efficiency-review`）时指出，随后在本仓源码上复核确认。
