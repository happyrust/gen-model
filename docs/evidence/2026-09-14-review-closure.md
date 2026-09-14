# 三维校审闭环实施记录（2026-09-14）

本记录区分业务链路、真实模型显示、源码静态对照及旧端运行对拍。未完成项不计 PASS，尚不能据此切换生产代理。

## 基线与保留

- 执行树：`feature/review-backend-port`，起始 `b0fd1561c`。
- 集成目标：`e3d-direct@02d445ba4`。
- 前端：`../plant3d-web`，未修改业务代码；原有工作树改动保留。
- 原有 10 个未提交文件已逐项查看：规范与 JSON 夹具主要为换行差异，UI 钩子和 U1 导航/名称输入有效，予以保留。
- 初始 SHA、状态与完整补丁保存在 `verification/review-closure-20260914/`；运行数据库、日志及 JWT 不纳入源码提交。

## 已实跑

| 项目 | 结果 | 证据/边界 |
| --- | --- | --- |
| Rust 构建 | PASS | `cargo build --features http_api --bin aios-database`；`D:/Rust/target-review-port/debug/aios-database.exe` |
| 校审单测 | PASS | `cargo test --features http_api --lib web_service::review`：150 passed |
| 全库 lib 测试 | 待基线比较 | 2149 passed / 9 failed / 136 ignored；失败名称与前日记录吻合，尚需同主线实跑比较 |
| 走查脚本自测 | PASS | `node scripts/review/walkthrough/selftest.mjs`：33 passed |
| U1/U2 通过 | PASS（接口链路） | `walkthrough/out/20260914-u2-fixed-02` |
| U1/U2 退回 | PASS（接口链路） | `walkthrough/out/20260914-u2-return-01` |
| U3 | PASS（接口与任务恢复） | `walkthrough/out/20260914-u3-fixed-04`；六接口、真实 sj→jd→sh、批注门禁、软删、独立负例 |
| U1/U2/U3 同次 env 走查 | PASS（接口链路） | `walkthrough/out/20260914-all-env-closure`，零 mock 告警、非预期校审失败、pageerror、遗漏接口 |
| 工作流 CLI | PASS | `test_workflow_e2e.sh http://localhost:8122`：53/53 |
| JSON CLI | PASS | `verify_platform_api_json.sh http://localhost:8122`：36 断言 |
| A6 完整持久化 | PASS | task `task-d4f9b294-70bf-4905-bf7c-069cd7291330`，两进程停止重启后 7 组非空 HTTP 快照一致；独立 runner 也复验通过 |

上述走查使用前端现有 E2E 构件注入钩子，证明接口业务链路；**不证明真实模型几何显示**。A4 总验收仍待模型显示和真实附件链接操作合并验收。

## 根因与修正

U2 403 实测正文为“当前用户 JD 不是 checker 节点负责人 JH”。夹具把角色 `jd` 当成账号 `JD`。改为显式设置任务 checker/reviewer=JH、JWT user_id=JH，恢复后确认节点与负责人；后端权限未放宽。另有 jd 阶段上传附件 403、未 fixed 即 agree 的 409，均按合法业务顺序修正脚本。

U3 必须先建真实任务，使用 form_id 恢复；embed URL 按前端现有函数组合 `data.query.form_id`。`annotations/check` 单独校验 success/data；`workflow/verify` 的阻断为 HTTP 200、passed=false。查询不能代替节点流转。

报告改为按通过/退回场景选择必需接口，前置失败后的步骤明确未执行；响应错误正文脱敏。A3 消除资源 404 假红，覆盖 37+1+6+2=46；静态附件通过实际上传和字节回读确认。A5 缺少负例夹具直接失败。

## 尚未完成的门槛

- A9：两种内存介质均已出现拒绝横幅，review/users/auth/files 四类路径全部 503；首次模型 health 为 200/degraded，新增初始化等待后复跑。
- A2/A5/A10：真正的 `plant-model-gen` 尚未在独立端口实跑。`:3100` 的 `plant-web-server` 不作对拍来源；继续 PARTIAL。
- 隔离主线集成、合并后的构建/测试/业务验收尚待执行。
- 真实模型显示与正式代理配置验证尚待执行。路径分流示例见 `scripts/review/nginx-review-split.conf`，需部署方合入既有 server 配置并运行 nginx -t；未部署。

切换前备份新端校审库与附件，旧端保留一个发布周期。回滚先暂停写入并保留新端新增数据；切代理不会把数据迁回旧端。不迁移项目管理、不接真实碰撞生产链、不建设 review WebSocket。
