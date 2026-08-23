# Spec 019：布尔成品平表的存量收敛

**Created**: 2026-08-20  
**Status**: Proposed  
**Input**: 对 gen-model `89f8b06b` 与 plant-ui `dbb348e25`（2026-08-20 布尔成品显示修复）的代码审核结论。该次修复只覆盖了「新布尔成功」与「`insts_flat = NONE` 的行」，而旧代码时代所有已布尔的行都是「先被清扫回填成正体、后写 `booled_id`」的形态——现场基线 `baseline-db.json` 即为铁证（`booled_id` 已在，`insts_flat` 仍是带 `scale=[1,1,234]` 的正体）。当次只对 `inst_relate:24381_36945` 一行做了手工 UPDATE。平表读 `query_insts_flat` 原样信任 `insts_flat`，三副本齐活就不落 slim 兜底，因此其他库/其他行的同型脏数据会继续把错误正体端给查看者。

## User Scenarios & Testing

### User Story 1 - 存量库的布尔行自动收敛（Priority: P1）

作为模型查看者，我希望在任何一个旧库上启动服务后，所有已有布尔成品的构件都自动以成品网格显示，而不需要有人对着单行手敲 UPDATE，从而让 RM13 半球那类显示错误在全库范围内一次性绝迹。

**Independent Test**: 在 `mem://` 库中植入一行 `booled_id` 有值、`insts_flat` 仍为带缩放正体的 inst_relate，跑一轮平表清扫，断言该行收敛为 `[{ geo_hash: booled_id }]`，且第二轮圈到 0 行。

**Acceptance Scenarios**:

1. **Given** 一行 `booled_id` 有值且 `insts_flat` 与之不符（正体残留或空数组），**When** 平表清扫执行，**Then** 该行 `insts_flat` 变为 `[{ geo_hash: booled_id }]`，不带 transform 字段（读端按单位变换处理）。
2. **Given** 修复已收敛的库，**When** 清扫再次执行，**Then** 修复段圈到 0 行，日志只有开始/完成两行。
3. **Given** 一行没有 `booled_id`，**When** 清扫执行，**Then** 该行的 `insts_flat` 逐字节不变。
4. **Given** 一行 `booled_id` 为空串或字面 `'none'` 脏值，**When** 清扫执行，**Then** 该行按「无成品」处理不被改写，且计数在日志中可见。

### User Story 2 - 平表读与其余读路径同语义（Priority: P1）

作为查看端，我希望平表读（`query_insts_flat`）对布尔行给出与 slim / insts / zone 三路完全一致的答案：insts 是成品单实例、`has_neg` 为真——即便对面是一台还没部署修复清扫的旧 gen-model 服务的库，我也不显示错误正体。

**Independent Test**: 对同一 booled refno 分别走四条读路径，断言 insts 的 geo_hash 与 has_neg 一致。

**Acceptance Scenarios**:

1. **Given** 一行 `booled_id` 有值而 `insts_flat` 尚未修复，**When** 平表读取行，**Then** 返回的 insts 为 `[{ geo_hash: booled_id }]` 单位变换单实例，`has_neg = true`。
2. **Given** 一行没有 `booled_id`，**When** 平表读取行，**Then** insts 原样返回 `insts_flat`，`has_neg = false`，三分法兜底行为不变。
3. **Given** 库中旧行缺 `has_neg` 可投影的任何字段组合，**When** 平表读取行，**Then** 反序列化不失败（`#[serde(default)]`）。

### User Story 3 - 双引擎产出同形态（Priority: P2）

作为增量链路维护者，我希望 Manifold 与 OCC 布尔成功后写出的 inst_relate 行形态一致（`booled_id` / `booled` / `insts_flat` 三字段等价），使按 `booled` 过滤的人工排查与后续工具不因引擎不同而分叉。

**Acceptance Scenarios**:

1. **Given** Manifold 布尔成功，**When** 落库语句执行，**Then** 行带 `booled = true`，与 OCC 路径等价。

### Edge Cases

- `bad_bool = true` 且保留旧 `booled_id` 的行（空差集不覆盖设计）：修复段照样把平表对齐到旧成品——继续显示上一版好的布尔结果，与 slim 路径行为一致。
- `insts_flat = []` 空数组但 `booled_id` 有值：属于「与成品不符」，修复。
- 无 `aabb.d` 的行（从未进过读者视野）：修复无害，谓词不必为它设防。
- 修复段与既有 NONE 清扫在「`booled_id` 有值且 `insts_flat = NONE`」上存在覆盖重叠：两者写出同一值，幂等无冲突。

## Requirements

### Functional Requirements

- **FR-001**: 平表清扫 MUST 增加修复段：圈「`booled_id` 有值而 `insts_flat` 与之不符」的行，批量改写为 `[{ geo_hash: booled_id }]`，幂等、自收敛（修复后的行不再命中谓词）。
- **FR-002**: 修复段 MUST 复用清扫既有的两个挂点（启动序列 + worker 空闲轮脏位门控），不引入新的触发机制或队列 action。
- **FR-003**: 修复段 MUST 不触碰无 `booled_id` 的行；正体行的 `insts_flat` 修复前后逐字节一致。
- **FR-004**: `query_insts_flat` MUST 在 SQL 投影内优先 `booled_id`（`IF booled_id != NONE THEN [{ geo_hash: booled_id }] ELSE insts_flat END`）并投影 `booled_id != NONE AS has_neg`，使平表读与 slim / insts / zone 同语义；三分法兜底不变。
- **FR-005**: Manifold 布尔成功路径 MUST 写 `booled = true`，与 OCC 等价。
- **FR-006**: `booled_id` 为空串或字面 `'none'` 的脏值 MUST 当缺失处理，禁止把坏值写进 `insts_flat`；此类行的存在 MUST 可见（日志计数或证据查询），不得静默。
- **FR-007**: 修复段 MUST 走持久层非 journal 路径（与既有清扫同族），不进暂存窗口、不推水位。
- **FR-008**: 回归测试 MUST 在恢复旧写法（删掉修复段、或平表读退回原样信任 `insts_flat`）后变红。
- **FR-009**: `sweep_inst_relate_flat` 文档注释中「行只会缺不会错」的断言 MUST 修订——存量布尔行恰恰是「错」，修复段就是为它而设。

### Key Entities

- **inst_relate 平表副本**：`insts_flat`（派生缓存）、`aabb_d`、`world_trans_d`；本特性只矫正 `insts_flat`。
- **布尔成品标识**：`booled_id`（成品网格 id，真值）、`booled`（布尔完成标志）。
- **has_neg 投影**：读路径告知查看端「该行显示的是布尔成品」的口径，四路必须一致。

## Success Criteria

- **SC-001**: AMS 实库（8009）执行清扫后，「`booled_id` 有值而 `insts_flat` 不符」的行数为 0；紧接着的第二轮修复段圈到 0 行。
- **SC-002**: 实库抽查无 `booled_id` 的行，修复前后 `insts_flat` 完全一致。
- **SC-003**: 同一 booled refno 走 flat / slim / insts / zone 四条读路径，insts 的 geo_hash 集合与 has_neg 全部一致。
- **SC-004**: 两引擎布尔成功后写出的行在 `booled_id` / `booled` / `insts_flat` 三字段上形态等价。
- **SC-005**: 删掉修复段或还原平表读投影，对应回归测试变红。

## Assumptions

- inst_relate 是无模式表，改写 `insts_flat` 与补投影不需要 schema 迁移。
- booled 行在全表中占比很小；修复谓词无索引可用，按批全表扫的一次性成本可接受（53k 行库上同型条件表达式已在 SurrealDB 2.x 实测 µs~ms 级）。
- fork SurrealDB 2.1.4 对数组首元素取值的具体语法（`insts_flat[0].geo_hash` 或 `array::first(insts_flat).geo_hash`）以 `fork_surreal_compat` 双引擎实测为准，plan 落定其一。
- 查看端 `ModelHashInst.transform` 已带 `#[serde(default)]`，省略 transform 的成品实例落成单位变换，无需读端改动。
