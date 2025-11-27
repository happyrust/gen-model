# 管道建模设计 (Tubi Relate Refactoring)

## 1. 背景
原有的 `tubi_relate` 设计中，`leave` 和 `arrive` 作为属性存在，边的方向（`in` -> `out`）定义不清晰或未充分利用 SurrealDB 的图特性。为了更好地支持拓扑查询和几何关联，对 `tubi_relate` 进行重构。

## 2. 新的设计方案

### 2.1 关系定义 (Schema)
`tubi_relate` 作为一个 **Edge** 表，直接表示管道段从“离开点”到“到达点”的连接。

- **Table**: `tubi_relate`
- **ID**: `tubi_relate:[bran_refno, index]`
  - `bran_refno`: 所属 Branch 的 Refno (e.g. `1234_5678`)
  - `index`: 管道段在 Branch 中的序号 (0, 1, 2...)
  - **优势**: 通过 ID 可以直接定位特定 Branch 的所有管道段，无需遍历。

### 2.2 边方向 (Direction)
利用 SurrealDB 的 `in` 和 `out` 字段表示拓扑方向：
- **in (Start)**: `leave_refno` (该段管道的起始 PE 节点)
- **out (End)**: `arrive_refno` (该段管道的结束 PE 节点)

> **注意**: 移除了原有的 `leave` 和 `arrive` 字段，直接使用 `in` 和 `out`。

### 2.3 属性字段 (Properties)
- **geo**: `record<inst_geo>`
  - 关联到 `inst_geo` 表中的几何体记录。
  - 格式: `inst_geo:⟨hash⟩`
- **aabb**: `object` (包围盒数据)
- **world_trans**: `object` (世界变换矩阵)
- **bore_size**: `string` (管径描述)
- **bad**: `bool` (是否为异常段，如距离过近或方向错误)
- **system**: `record<pe>` (可选，所属系统，生成阶段写入 owner 作为系统参考)
- **dt**: `datetime` (创建/更新时间，生成阶段写入 `fn::ses_date(in)` )

### 2.4 数据流向
1. **生成阶段** (`cata_model.rs`):
   - 计算起点/终点 PE。
   - 生成 `inst_geo`。
   - 插入关系: `RELATE leave->tubi_relate:[bran, idx]->arrive SET geo=inst_geo:⟨hash⟩, aabb=..., world_trans=..., bore_size=..., bad=..., system=owner_refno, dt=fn::ses_date(leave)`（`in/out` 即为起点/终点，不再写 `leave/arrive` 属性）

2. **查询阶段**:
   - **按 Branch 查询**: `SELECT * FROM tubi_relate WHERE record::id(id)[0] = $bran_refno`
   - **拓扑遍历**:
     - 向下流向: `SELECT out FROM ->tubi_relate`
     - 向上流向: `SELECT in FROM <-tubi_relate`
   - **几何获取**: 通过 `geo` 字段关联 `inst_geo`。

## 3. 迁移与兼容性
- 所有使用 `tubi_relate.leave` 的查询需改为 `tubi_relate.in`。
- 所有使用 `tubi_relate.arrive` 的查询需改为 `tubi_relate.out`。
- `fn::query_tubi_to` 和 `fn::query_tubi_from` 已更新适配。
- `inst_relate` 相关的逻辑保持不变，`tubi_relate` 独立作为管道专用的拓扑表达。
- 材料清单函数（`rs_surreal/material_list/gy/gy_tubi.surql`、`gps/gps_tubi.surql`）已改为依赖 `in/out` 和关系上的 `world_trans`，不再读取 `leave/arrive`。
- 已有历史数据需迁移：复制旧记录的 `leave/arrive` 到 `in/out`，移除旧字段，并补充 `geo` 关联，避免查询混用。

## 4. 索引优化
- 建议对 `tubi_relate` 的 ID 进行分片或索引优化（SurrealDB 默认对 ID 有索引）。
- 可对 `geo` 字段建立索引以便反向查询几何复用情况。
