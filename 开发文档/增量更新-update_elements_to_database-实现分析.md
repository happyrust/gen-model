# PdmsIO::update_elements_to_database 实现分析

- 入口位置：external/pdms-io/src/io.rs:807 起（函数签名见下）
- 功能概述：基于 `range_eles: BTreeMap<u32, Vec<EleOperationData>>`，按 sesno 批量落库 sessions、element_changes；在 `update_main_data=true` 时再批量生成并执行主数据 SurrealQL（类型表、pe 表、关系表）
- 关键点：
  - 始终保存 sessions 记录与 element_changes 历史
  - `update_main_data` 控制是否对主数据执行 CREATE/UPSERT/UPDATE（便于仅做历史留痕或仅审计）
  - 批处理优化：按 100 条一批执行 INSERT/UPSERT 以降低往返开销

## 函数签名与位置

```rust
pub async fn update_elements_to_database(
    &mut self,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    update_main_data: bool,
) -> anyhow::Result<()>
```

- 文件：/Volumes/DPC/work/_external/pdms-io/src/io.rs
- 行号：约 807-1035

## 输入数据结构摘要

- `EleOperationData { refno: RefU64, sesno: u32, detail: EleOperationDetail }`
- `EleOperationDetail` 四种：`Add(EleData) | Modified(ModifiedElement) | Deleted | None`
- `to_surql(&self, id, dbnum, sesno)`：将操作转为 SurrealQL，内部：
  - Add：INSERT pe、CREATE 类型表记录、INSERT pe_owner 关系
  - Modified：DELETE 旧 pe_owner → UPSERT MERGE 字段/uda → UPDATE pe.sesno/pe.name
  - Deleted：UPDATE pe.deleted=true, pe.sesno=sesno

## 处理流程

```mermaid
flowchart TD
A[range_eles 输入] --> B[收集所有 sesno]
B --> C[批量 INSERT IGNORE sessions (100/批)]
C --> D[统计每 ses 的 add/modify/delete 数量]
D --> E[UPDATE sessions:dbnum_sesno 写回计数]
A --> F[生成 element_changes 记录]
F --> G[批量 INSERT IGNORE element_changes (100/批)]
A --> H{update_main_data?}
H -- 否 --> Z[完成（仅会话与历史）]
H -- 是 --> I[逐元素生成 SurrealQL]
I --> J{操作类型}
J -- 新增 --> K[INSERT pe + CREATE type:id + 插入 pe_owner]
J -- 修改 --> L[删除旧关系 + UPSERT MERGE 字段/uda + UPDATE pe]
J -- 删除 --> M[UPDATE pe.deleted=true, sesno]
K --> N[按 100/批 执行 SurrealQL]
L --> N
M --> N
N --> Z[完成]
```

## 关键实现片段（示例节选）

- 批生成并执行元素 SurrealQL（受 `update_main_data` 控制）：

```rust
if update_main_data {
    let mut surql_batch = Vec::new();
    for (&sesno, elements) in range_eles {
        for element in elements {
            let id = element.refno.to_string();
            let surql = element.to_surql(&id, dbnum, sesno);
            if !surql.is_empty() { surql_batch.push(surql); /* 满100执行 */ }
        }
    }
    // flush 末尾未满 100 的 batch
}
```

- 新增元素的 SurrealQL：

```rust
// 1) INSERT pe [...]
// 2) CREATE {type}:{id} CONTENT {...}
// 3) INSERT RELATION INTO pe_owner [...]
```

- 修改元素的 SurrealQL：

```rust
// DELETE pe:{id}<-pe_owner;
// UPSERT {noun}:{id} MERGE { 普通字段 + "uda":{...} + 引用字段(pe:{refno}) }
// UPDATE pe:{id} SET sesno=..., [name='..']
```

- 删除元素（软删除）：

```rust
UPDATE pe:{id} SET deleted = true, sesno = {sesno}
```

- 历史 element_changes（每元素/每 ses 一条）：

```rust
INSERT IGNORE INTO element_changes [
  { id: [pe:..., sesno], refno: ..., operation_type: "新增|修改|删除",
    entity_type: type, timestamp: d"...", session_id: sessions:db_ses,
    sesno: ..., details: <patch-json> }
]
```

## 会话与统计

- 会话记录先批量 INSERT IGNORE，再按统计 UPDATE 三个计数字段：
  - add_count / modify_count / delete_count
- 计数来自 `range_eles` 遍历 `EleOperationDetail` 变更类型的 tally

## UDA 与普通属性处理要点

- UDA 合并为嵌套对象 `uda: { key: value }`，`RefU64Type` 特判为 `{ type: "refno", value: "pe:{refno}" }`
- 普通/显式属性：
  - 普通字段 → `main_fields` MERGE
  - 记录字段（RefU64Type） → `records_sql` 以 `key: pe:{refno}` 形式拼接
  - 显式属性 `NAME` → 同步 `UPDATE pe:{id} SET name='..', sesno=..`
- children 变化：先 DELETE 旧 `pe_owner`，后 INSERT 新 children 关系

## 何时设置 update_main_data=false

- 仅做历史审计/演示，不希望改动主数据；或先快速落地 sessions/element_changes，再在后台异步主数据更新

## 相关源码定位

- `PdmsIO::update_elements_to_database`：`src/io.rs` 807-1035
- `EleOperationDetail::to_surql`：`src/io.rs` 439-501
- `ModifiedElement::to_modify_surql`：`src/io.rs` 176-337
- `EleOperationData` 定义：`src/io.rs` 376-419

---

最后更新：由自动化助手生成（基于 gitee.com/happydpc/pdms-io 源码）

