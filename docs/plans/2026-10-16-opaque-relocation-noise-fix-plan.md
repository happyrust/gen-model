# 同值保存 opaque 误报修复计划：搬迁地址字归一化

日期：2026-10-16；状态：**已执行（R5 已关闭）**；协作轮次：ROOM-20261016-01（协作仍暂停，本计划为单会话任务）。
起草：Agent-A（grill 手工模拟：第一轮被跳过，按惯例“全按推荐”推进）；批准位：用户（代 Plannotator）。

> 目标：消除已复现的冗余重算误报——同值保存仅成员块物理地址变化时，差分器给 `opaque=true`
> 并把父根列入 regen（`verification/model-orientation-increment-20261016/verification.md` §8 红测）。

## 1. 证据与根因（全部已复核）

- 红测输出：`ElementDiff { attributes: [], opaque: true }`，`relocated_header_words=[(8, 3, 11), (9, 352257, 278529)]`，
  `regen=["16192/10"]`；j173/j174 均为 1 failed（exit=101）。
- `vendor/e3d-io/src/write/record.rs:862-868`（SHA `a9188984…`，已复核）：头字 8/9 = 成员块地址
  （`page` + `pack_offset(members_at, ext)`），记录搬迁即变；同理 6/7 = explicit 块地址。
- `vendor/e3d-model/src/element_diff.rs:227-231`（SHA `1521a7a9…`，已复核）：`record_identical` 全量比较
  `implicit_data`，纯搬迁落到 266-270 得 `opaque=true`。
- 工作区内三门全不放行 opaque（`window_root_plan.rs:364`、`9060`、placement-only/刚体路径），父根冗余 regen。
- 红测已排除夹具偷改：noun/owner/members/显式属性相同，`implicit_data[44..]` 相同，local placement 相同。

## 2. grill 共识（G1–G4，全按推荐）

| ID | 问题 | 共识 |
|---|---|---|
| G1 | 修复口径 | vendor 侧 `record_identical` 比较 `implicit_data` 时**排除字 6..=9**，其余逐字比 |
| G2 | 谁改 vendor | 我出精确 patch 文本，用户应用（vendor 在 Bridge 允许根外，我只能读不能写） |
| G3 | 验收 | 全套：vendor 仓 2 新测试 + 工作区红测转绿 + 全库回归 + problems/changes 核对 |
| G4 | 计划落点 | 本文件 |

否决项：gen-model 侧二次确认（`ElementDiff` 只有 opaque 布尔，区分噪音需重读 DbSet，重复差分器职责）。

## 3. 变更设计（vendor 仓， patch 文本）

### 3.1 `element_diff.rs`：`record_identical` 门（约 227-231 行）

把：

```rust
let record_identical = before.noun_hash == after.noun_hash
    && before.owner == after.owner
    && before.members == after.members
    && before.implicit_data == after.implicit_data
    && before.explicit_attributes == after.explicit_attributes;
```

改为：

```rust
let record_identical = before.noun_hash == after.noun_hash
    && before.owner == after.owner
    && before.members == after.members
    && implicit_data_equal_ignoring_block_addresses(&before.implicit_data, &after.implicit_data)
    && before.explicit_attributes == after.explicit_attributes;
```

新增辅助函数（与 `member_delta` 并列）：

```rust
/// 隐式区相等：忽略已证明的纯搬迁地址字。
///
/// `record.rs::encode` 语义：字 6/7 = explicit 块地址，字 8/9 = 成员块地址
/// （页号 + 打包偏移）；记录搬迁即变，与内容无关。
/// 安全性：成员增删由 `members` 门覆盖，显式内容由 `explicit_attributes` 门覆盖；
/// 其余字（0-5、10+，含长度字与块大小字）仍逐字比，UDA/未解码变化照样落 opaque。
/// TUBI 门与下游三门不受影响。
fn implicit_data_equal_ignoring_block_addresses(before: &[u8], after: &[u8]) -> bool {
    if before.len() != after.len() {
        return false;
    }
    const ADDR_BYTES: std::ops::Range<usize> = 24..40; // 字 6..=9
    before
        .iter()
        .zip(after.iter())
        .enumerate()
        .all(|(i, (a, b))| ADDR_BYTES.contains(&i) || a == b)
}
```

> **R1 确认点**：若 `implicit_data` 是 `&[u32]`（字数组）而非字节数组，把 `24..40` 换成字索引 `6..10`，
> 其余不动。应用时看 `parsed` 结构定义一行即可确认。

### 3.2 vendor 仓新增 2 测试

1. `relocated_record_counts_as_unchanged`：两端记录除头字 6..=9 外逐字相同
   → `diff_element` 返回 `is_unchanged()`（锁住本次修复）。
2. `non_address_implicit_change_is_still_opaque`：改动 6..=9 之外的隐式字、无属性差异
   → 仍 `opaque=true`（锁住哨兵不被削弱；gen-model 侧 writer 造不出这种输入，必须落 vendor 仓）。

### 3.3 工作区改动

**零业务改动**。验证通过后仅追加 `verification/model-orientation-increment-20261016/verification.md` §9
（转绿记录：命令/exit/计数/版本），并更新交接记录。

## 4. 实施步骤

| ID | 动作 | 执行者 | 说明 |
|---|---|---|---|
| R1 | 确认 `implicit_data` 类型并应用 §3.1 + §3.2 | 用户 | vendor 文件我不可写 |
| R2 | vendor 仓跑新测试 + 该仓回归 | 用户 | 测试命令在 vendor 仓执行 |
| R3 | 工作区验证：定向 `synthetic_orientation_` → 全库 `cargo test --offline --lib` → `get_problems` → `get_changes` | Agent-A | 见 §5 矩阵 |
| R4 | verification.md 追加 §9，交接文件记关闭 | Agent-A | 仅文档 |
| R5 | 批准关闭（代 Plannotator 批准位） | 用户 | 本计划状态改为“已批准/已执行” |

## 5. 验证矩阵（G3 全套）

| ID | 断言 | 通过标准 |
|---|---|---|
| V1 | vendor 2 新测试 | 全过；该仓既有测试无新增失败 |
| V2 | 工作区红测转绿 | `synthetic_orientation_` 9 passed / 0 failed |
| V3 | 全库回归 | `cargo test --offline --lib` 0 failed（ignored 照实记录，不算通过） |
| V4 | 诊断与 diff | `get_problems` 0 错误；`get_changes` 工作区除 §9 文档外零改动 |

任一失败即停：保留现场（命令/exit/输出），不放宽断言、不加 ignored，回 §3 复议。

## 6. 风险与不做事项

- 类型假设风险由 R1 一行确认消除；6/7 纳入的安全性见 §3.1 注释（explicit 空/非空翻转必带内容差异，
  由 `explicit_attributes` 门捕获，不会误判 unchanged）。
- 回滚：vendor 改动为单函数 + 2 测试，`git checkout` 即回；工作区本轮除文档外零改动，无需回滚。
- 不做：gen-model 生产门改动、TUBI 门改动、真实工程 E2E、性能基准、提交/推送。

## 7. 批准栏

- [x] 批准：按 §3–§5 执行（实际：父目录根切通后 R1–R4 均由 Agent-A 实施，用户 R5 关闭）
- [ ] 驳回/改口径：无

批准后本文件状态改为“已批准”，R5 关闭时改为“已执行”。
