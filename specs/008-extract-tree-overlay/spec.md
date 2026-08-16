# Feature Specification：抽取树叠加

## User Stories

### US1：主库与唯一抽取是同一个逻辑库

同项目同时存在 `ams7355` 与 `ams7355_0001` 时，系统按一个 dbnum 处理，登记叶子，
不 F6 Duplicate 阻断。

### US2：兄弟抽取仍然阻断

`ams9990_0001` 与 `ams9990_0002` 不是父子。无 CLAIM 时一律 Duplicate。人手副本
进不了候选白名单。

### US3：叶子是水位权威，父层只补缺号

增量窗口与 `applied_sesno` / `file_latest_sesno` 只跟叶子。叶子索引没有的 refno
从主库读；同一 refno 叶子覆盖父层。

## Functional Requirements

- **FR-001**：抽取家族由文件名解析，不读 SYS；头 `db_no` 与文件名库号不一致则阻断。
- **FR-002**：归并发生在 Duplicate 之前；手动、自动、Catalogue、全量 collect 共用
  同一纯函数。
- **FR-003**：仅主库、仅叶子、主库+唯一叶子均可选中；多个 `_NNNN` 阻断。
- **FR-004**：同家族主库改挂叶子为 PathMigrated；叶子 sesno 倒退则重建。
- **FR-005**：水位键仍是裸 dbnum；父路径可从叶子文件名重算。
- **FR-006**：按需解析叶子 miss 再打开父文件；基线在父层有独有号时补缺，不把父层
  会话并进叶子窗口。

## Success Criteria

- `ams7355` + `ams7355_0001` 不阻断且登记叶子。
- `ams9990_0001` + `ams9990_0002` 继续 Duplicate。
- 头库号与文件名不一致响亮失败，不猜。

## Assumptions

- 本仓不移植 dabacon opcode 134。
- 不按 SYS CLAIM 在兄弟抽取中选主。
