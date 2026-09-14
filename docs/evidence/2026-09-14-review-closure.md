# 三维校审闭环实施记录（2026-09-14）

本记录区分业务链路、真实模型显示、源码静态对照及旧端运行对拍。未完成项不计 PASS，当前集成分支尚不能切换生产代理。

## 基线与集成范围

- 校审执行基线：`feature/review-backend-port@4400d8bab`（起始 `b0fd1561c`）。
- 主线基线：`e3d-direct@02d445ba4`。
- 重构基线：`gen-model-refactor@54a7c8b9d`；仅合入已提交 HEAD，原工作树 28 个已修改文件及未跟踪文件未改动，状态和补丁保存在 `verification/review-closure-20260914/refactor-preexisting-*`。
- 隔离集成分支：`codex/review-backend-closure`；合并提交为 `2d672bf5c`（校审）和 `8537a9c3f`（重构）。
- 前端：`../plant3d-web`，未修改业务代码；浏览器验收使用现有 Vite/Playwright 能力。
- 运行数据库、日志、JWT、截图和 JSON 证据保存在 `verification/review-closure-20260914/`，不纳入源码提交。

合入 ADR-074 后，校审持久化闸已适配两档介质：`rocksdb` 可启用校审域，`mem` 启动时拒绝校审路由。gen-model 不再自行拉起数据库。原校审持久化决策因 ADR 编号冲突迁至 `ADR-075-review-data-persistence.md`。

## 合并后实跑结果

| 项目 | 结果 | 证据/边界 |
| --- | --- | --- |
| Rust check/build | PASS | `cargo check --features http_api`；`cargo build --features http_api --bin aios-database`；集成二进制 `D:/Rust/target-review-integration/debug/aios-database.exe` |
| 校审单测 | PASS | `cargo test --features http_api --lib web_service::review`：150 passed |
| 全库 lib 测试 | PASS | 集成分支 2035 passed / 0 failed / 107 ignored；同一主线基线 1905 passed / 0 failed / 110 ignored，无新增失败 |
| 走查脚本自测 | PASS | `node scripts/review/walkthrough/selftest.mjs`：33 passed |
| U1/U2 通过 | PASS | `walkthrough/out/20260914-integrated-all-env-2`，正常链路无 mock 兜底、非预期 4xx/5xx、遗漏接口或页面错误 |
| U1/U2 退回 | PASS | `walkthrough/out/20260914-integrated-return`，退回场景独立闭环 |
| U3 | PASS | 同次 env 走查使用真实任务和 formId；六接口、sj→jd→sh 流转、批注门禁、软删、独立鉴权负例全部通过 |
| 工作流 CLI | PASS | `integrated-workflow-cli.log`：53/53 |
| JSON CLI | PASS | `integrated-json-cli.log`：36 断言 |
| A3 路由 | PASS | `integrated-acceptance.json`：37 条核心接口 + 1 条静态附件 + 6 条 PMS + 2 条集成接口，共 46/46 |
| A6 完整持久化 | PASS | task `task-d4f9b294-70bf-4905-bf7c-069cd7291330`；数据库与 API 停止重启后，任务、记录、评论、状态、非空工作流历史及附件哈希等 7 组快照一致；重启脚本再次复验通过 |
| A9 内存拒绝 | PASS | ADR-074 canonical `mem` 下，四类校审路径返回 503，模型 health 仍为 200；`integrated-acceptance.json` |
| 真实模型显示 | PASS | 合并后二进制、AMS 8000、`24384_22402`：`model/ensure` 为 Generated；浏览器登记 2 个 refno、场景对象数 1、`.mesh` 200、`.glb` 0、pageerror 0；`integrated-real-model.json/.png` |

U1 的附件通过实际 HTTP 上传、静态链接读取和字节比较。U1 走查仍使用前端 E2E 构件注入参考模型；因此接口业务闭环和真实模型显示分别留证，不把构件注入写成生产模型证据。

## 根因与修正

U2 403 实测正文为“当前用户 JD 不是 checker 节点负责人 JH”。夹具原先把角色 `jd` 当成账号 `JD`。现改为显式设置任务 checker/reviewer=JH、JWT user_id=JH，恢复后确认节点与负责人；后端权限未放宽。jd 阶段上传附件 403、未 fixed 即 agree 的 409 也按合法业务顺序修正脚本。

U3 先建真实任务并使用 form_id 恢复。`annotations/check` 单独校验 `success/data`；`workflow/verify` 实际执行节点流转并验证节点、负责人和历史。查询不再代替流转，找不到任务和错误身份作为独立负例。

报告按通过/退回场景选择必需接口，前置失败后的步骤明确未执行；响应错误正文脱敏。路由判据区分资源 404 与路由未注册，静态附件通过实际上传和字节回读确认。

## 尚未完成的门槛

- A2/A5/A10：真正的旧 `plant-model-gen` 无法在独立端口构建。它引用的本地 path patch `D:/work/plant-code/pdms-io-fork/crates/parse_pdms_db` 不含所需 package；当前 `:3100` 的 `plant-web-server` 不作旧端来源，因此继续 PARTIAL。
- 正式代理配置尚未部署验证。`scripts/review/nginx-review-split.conf` 已按路径分流，仍需部署方合入实际 server 配置并运行 `nginx -t` 与同源浏览器验收。
- 真实碰撞生产数据源按批准范围保留 P2，不影响本轮校审闭环代码验收。

由于验收规则明确规定 PARTIAL 不计完成，在补齐真正旧端运行对拍前，不将隔离集成分支合入 `e3d-direct`。切换前备份新端校审库与附件，旧端保留一个发布周期；回滚先暂停写入并保留新端新增数据。不迁移项目管理，不建设 review WebSocket。
