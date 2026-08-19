# Implementation Plan

1. 在 pdms-io 净窗口合成后，从父成员净变化构造删除候选、搬迁集合与不可达子树。
2. 以目标会话 OWNER 成员关系仲裁候选，失败阻断，成功后去重合成 `Deleted`。
3. 将补删统计接入口径 warning、执行结果和 staging 落库链。
4. 增加停服维护 CLI，以固定窗口重放和可达性审计纠正已提交数据，保持水位不变。
5. 用纯测试、真实 236 对拍、live 纠正与仓库质量门验证并留证。

## Constitution Check

- 水位仍是数据承诺；仲裁或提交失败不推进，纠正不降低水位。
- 自动和手动在线路径继续共用唯一 `collect_window`；维护 CLI 只允许停服运行。
- 所有异常显式上浮；无静默 `continue`、默认空集合或 warning 降级。
