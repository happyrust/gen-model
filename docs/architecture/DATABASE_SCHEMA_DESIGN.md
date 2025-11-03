# 数据库架构设计文档

## 概述

本文档对 gen-model 项目的核心数据库表进行了系统的架构设计记录，包括表结构、字段含义、数据流向、更新时机等详细信息。

当前支持的数据库后端：
- **SurrealDB**（主要）
- **MySQL**（备选）
- **Kuzu**（实验性）

---

## 表设计索引

| 表名 | 用途 | 后端 | 状态 | 更新时机 |
|------|------|------|------|---------|
| **pe** | 核心元素表 | SurrealDB | ✅ 生产 | 解析时/EVENT自动 |
| **dbnum_info_table** | 元数据统计表 | SurrealDB | ✅ 生产 | 解析时/EVENT自动 |
| **pe_{dbnum}** | 按数据库分表 | SurrealDB | ✅ 生产 | 解析时 |
| **pe_owner** | 元素关系表 | SurrealDB | ✅ 生产 | 解析时 |
| **PDMS_ELEMENTS_TABLE** | 元素信息表 | MySQL | ✅ 生产 | 解析时 |
| **PDMS_DBNO_INFOS_TABLE** | 数据库信息表 | MySQL | ✅ 生产 | 解析时 |
| **event_changes** | 事件变更表 | SurrealDB | ✅ 生产 | 实时 |
| **sesno_increment** | 会话号增量表 | SurrealDB | ✅ 生产 | 实时 |

---

## 表详细设计

### 1. dbnum_info_table - 元数据统计表

**用途**: 记录每个 ref_0 下的 PDMS 元素统计信息，用于版本管理和增量更新

**后端**: SurrealDB

**记录 ID 格式**: `dbnum_info_table:{ref_0}`

#### 字段设计

| 字段名 | 类型 | 必填 | 初值 | 用途 | 更新时机 | 示例 |
|--------|------|------|------|------|---------|------|
| **id** | String | ✅ | 自动生成 | 记录唯一标识 | 创建时 | `dbnum_info_table:17496` |
| **dbnum** | Integer (i32) | ✅ | 解析时 | 数据库号 | 不变 | `1112` |
| **ref_0** | Unsigned Integer (u64) | ✅ | 分解得出 | RefU64 的高 32 位 | 不变 | `17496` |
| **count** | Integer (i32) | ✅ | 累计 | 该 ref_0 下的元素总数 | EVENT 维护 | `156` |
| **sesno** | Integer (i32) | ✅ | 最大值 | 该 ref_0 下的最大会话号 | EVENT 维护 | `15` |
| **max_ref1** | Unsigned Integer (u64) | ✅ | 最大值 | 该 ref_0 下最大的 ref_1 值 | EVENT 维护 | `266203` |
| **file_name** | String | ✅ | 解析时 | 源文件名 | 不变 | `PIPE0001` |
| **db_type** | String | ✅ | 解析时 | 数据库类型 | 不变 | `DESI` 或 `CATA` |
| **project** | String | ✅ | 解析时 | 项目名称 | 不变 | `Sample` |
| **file_sesno** | Unsigned Integer (u32) | ❌ | null | 源文件中的最新会话号 | 手动同步 | `18` |
| **auto_update** | Boolean | ❌ | false | 是否启用自动更新 | WebUI 设置 | `true/false` |
| **updating** | Boolean | ❌ | false | 是否正在执行更新 | 任务状态改变 | `true/false` |
| **last_update_result** | String | ❌ | null | 最后一次更新的结果 | 任务完成时 | `Success` 或 `Failed` |
| **last_update_at** | Unsigned Integer (u64) | ❌ | null | 最后一次更新的时间戳(ms) | 任务完成时 | `1699123456789` |
| **auto_update_type** | String | ❌ | null | 自动更新的类型 | WebUI 设置 | `ParseOnly` / `ParseAndModel` / `Full` |

#### 字段分类

**源数据信息** (不变)
- dbnum: 数据库号
- ref_0: 参考号高位
- file_name: 源文件
- db_type: 数据库类型
- project: 项目

**统计数据** (EVENT 自动维护)
- count: 元素计数
- sesno: 最大会话号
- max_ref1: 最大 ref_1

**版本追踪** (手动同步)
- file_sesno: 文件会话号

**更新状态** (任务管理)
- auto_update: 自动更新开关
- updating: 更新进行中标志
- last_update_result: 更新结果
- last_update_at: 更新时间
- auto_update_type: 更新类型

#### 参考号分解

```
RefU64 = ref_0 (高32位) << 32 | ref_1 (低32位)

例子：
refno = 0x0000445000266203
ref_0 = 0x00004450 = 17496     (dbnum_info_table:17496)
ref_1 = 0x00266203 = 2500099   (该 ref_0 下的元素)
```

#### 保存流程

```
时间点                    操作                              涉及字段
─────────────────────────────────────────────────────
初始化                   创建内存统计 map
                        
解析 PDMS 元素            按 ref_0 分组：
                        - count += 1
                        - sesno = max(sesno, elem.sesno)
                        - max_ref1 = max(max_ref1, elem.ref_1)

生成 SQL                 创建 UPSERT 语句              file_name, db_type
                        
执行 UPSERT              保存到 SurrealDB              dbnum, count, sesno, max_ref1
                                                      file_name, db_type, project

定义 EVENT               自动维护后续更新              count, sesno, max_ref1
                        (PE 表变化时触发)             (实时)

用户操作(WebUI)          设置自动更新                  auto_update, auto_update_type
                       触发任务执行                  updating = true
                       任务完成                      updating = false
                                                    last_update_result, last_update_at

文件扫描                 同步文件元数据                file_sesno (手动触发)
```

#### UPSERT 语句示例

```sql
UPSERT dbnum_info_table:17496 SET 
  dbnum = 1112,
  count = count?:0 + 156,
  sesno = math::max([sesno?:0, 15]),
  max_ref1 = math::max([max_ref1?:0, 266203]),
  file_name = 'PIPE0001',
  db_type = 'DESI';
```

**语义说明**：
- `count?:0` - 如果字段不存在则默认为 0，支持增量更新
- `math::max()` - 取新旧值中的较大值，确保单调性
- 支持批量插入或更新，幂等性设计

#### EVENT 自动维护规则

```sql
DEFINE EVENT update_dbnum_event ON pe WHEN 
  $event = "CREATE" OR $event = "UPDATE" OR $event = "DELETE" 
THEN {
  LET $dbnum = $value.dbnum;
  LET $id = record::id($value.id);
  LET $ref_0 = array::at($id, 0);
  LET $ref_1 = array::at($id, 1);
  
  IF $event = "CREATE" {
    UPSERT dbnum_info_table:$ref_0 MERGE {
      count: count?:0 + 1,
      sesno: math::max([sesno?:0, $value.sesno]),
      max_ref1: math::max([max_ref1?:0, $ref_1])
    };
  } ELSE IF $event = "DELETE" {
    UPSERT dbnum_info_table:$ref_0 MERGE {
      count: count - 1,
      sesno: math::max([sesno?:0, $value.sesno]),
      max_ref1: math::max([max_ref1?:0, $ref_1])
    } WHERE count > 0;
  };
};
```

#### 查询示例

**查看某个 ref_0 的统计信息**
```sql
SELECT * FROM dbnum_info_table:17496;
```

**查看所有需要更新的数据库**
```sql
SELECT dbnum, file_name, sesno, file_sesno 
FROM dbnum_info_table 
WHERE sesno < file_sesno
ORDER BY dbnum;
```

**统计每个项目的元素总数**
```sql
SELECT project, SUM(count) as total_elements, COUNT(*) as ref_0_count
FROM dbnum_info_table 
GROUP BY project
ORDER BY total_elements DESC;
```

**查看更新失败的记录**
```sql
SELECT dbnum, file_name, last_update_result, last_update_at 
FROM dbnum_info_table 
WHERE last_update_result = 'Failed'
ORDER BY last_update_at DESC;
```

#### 性能指标

- **记录粒度**: ref_0 级别（通常 1 ref_0 = 100-1000+ 个元素）
- **更新频率**: 解析时一次，之后通过 EVENT 自动维护
- **查询效率**: O(1)（按 ref_0 直接查询）
- **存储空间**: 约 100-500 字节/记录

---

## 相关表结构

### pe 表 (SurrealDB)

```
表记录 ID: pe:[{ref_0}, {ref_1}]

核心字段:
- id: [{ref_0}, {ref_1}]         # 数组格式的复合 ID
- dbnum: i32                      # 数据库号
- refno: String                   # PE 参考号
- noun: String                    # PE 名称
- owner: [{ref_0_owner}, ...]    # 所有者关系
- children: [{child_id}, ...]    # 子节点关系
- sesno: i32                      # 会话号
- name: String                    # 元素名称
- type: String                    # 元素类型
```

**与 dbnum_info_table 的关系**:
- pe.dbnum → dbnum_info_table.dbnum
- pe.id[0] → dbnum_info_table.ref_0 (作为分组关键字)
- pe.sesno 影响 dbnum_info_table.sesno 的更新

### pe_{dbnum} 表 (SurrealDB)

```
分表规则: 按 dbnum 分表
表记录 ID: {refno}

简化字段:
- id: refno                      # PE 完整参考号
- noun: String                   # PE 类型名称
- children: [{child_id}, ...]   # 子节点
- name: String                   # 元素名称
- owner: refno                   # 所有者
```

**与 dbnum_info_table 的关系**:
- 表名中的 dbnum 来自 dbnum_info_table.dbnum
- 提供快速查询特定数据库的数据

---

## 数据一致性设计

### 1. 原子性保障

- **UPSERT 操作**: 确保 INSERT 和 UPDATE 原子性
- **EVENT 触发**: 自动同步更新，不存在时间窗口

### 2. 冪等性设计

- **count 字段**: 使用 `count?:0 + n` 支持重复执行
- **sesno 字段**: 使用 `math::max()` 保证单调性
- **max_ref1 字段**: 使用 `math::max()` 保证正确性

### 3. 数据监测

```sql
-- 检查数据一致性
SELECT 
  COUNT(*) as total_records,
  SUM(count) as total_elements,
  MAX(sesno) as max_sesno,
  MIN(sesno) as min_sesno
FROM dbnum_info_table;

-- 找出异常记录
SELECT * FROM dbnum_info_table 
WHERE count = 0 OR count < 0;
```

---

## 最佳实践

### 1. 查询优化

✅ **推荐**: 按 ref_0 直接查询
```sql
SELECT * FROM dbnum_info_table WHERE ref_0 = 17496;
```

❌ **避免**: 全表扫描
```sql
SELECT * FROM dbnum_info_table;  -- 避免用于大规模数据集
```

### 2. 更新策略

✅ **使用 UPSERT**: 自动处理插入和更新
```sql
UPSERT dbnum_info_table:17496 SET ...;
```

❌ **避免**: 先查询再分别 INSERT/UPDATE
```sql
-- 容易出现竞态条件
```

### 3. 事件管理

✅ **定义事件**: 自动维护统计数据
```sql
DEFINE EVENT update_dbnum_event ON pe WHEN ... THEN ...;
```

❌ **手动维护**: 在应用代码中更新统计数据
```rust
// 容易出现同步问题
```

### 4. 版本控制

✅ **使用 sesno 比较**: 判断更新需求
```sql
SELECT * FROM dbnum_info_table 
WHERE sesno < file_sesno;
```

❌ **使用时间戳**: 不精确
```sql
-- sesno 更准确
```

---

## 故障排查

### 问题: count 不准确

**原因**: EVENT 可能未正确触发或中断

**解决**:
1. 检查 EVENT 定义状态
2. 重新扫描数据库
3. 重建统计信息

```sql
-- 重新计算 count
SELECT ref_0, COUNT(*) as calculated_count 
FROM pe 
GROUP BY ref_0;
```

### 问题: 更新无法触发

**原因**: auto_update 未启用或 auto_update_type 未设置

**解决**:
```sql
UPDATE dbnum_info_table 
SET auto_update = true, 
    auto_update_type = 'ParseAndModel'
WHERE dbnum = 1112;
```

### 问题: 数据库号映射错误

**原因**: dbnum 和 ref_0 不匹配

**解决**:
```sql
-- 验证一致性
SELECT DISTINCT dbnum, ref_0 
FROM dbnum_info_table
ORDER BY ref_0;
```

---

## 扩展性考虑

### 1. 多项目支持

- 通过 `project` 字段隔离不同项目
- 支持按项目的统计查询

### 2. 版本管理

- 通过 `sesno` 和 `file_sesno` 进行版本追踪
- 支持增量更新机制

### 3. 性能扩展

- 分表设计 (pe_{dbnum}) 分散查询负载
- 按 ref_0 分组实现高效统计

### 4. 自动化更新

- 灵活的 `auto_update_type` 配置
- 支持定时任务和事件触发

---

## 演进计划

### 短期 (当前)
- ✅ 核心表结构稳定
- ✅ EVENT 自动维护
- ✅ 基础版本管理

### 中期
- 🔄 增强数据一致性检查
- 🔄 优化大规模数据集查询
- 🔄 完善故障恢复机制

### 长期
- 📋 支持更多后端数据库
- 📋 分布式数据管理
- 📋 高级分析和报表功能

---

## 相关文档

- [dbnum_info_table 详细架构](./dbnum_info_table_schema.md)
- [SurrealDB 适配分析](../database/SurrealDB适配gen_geos分析报告.md)
- [增量数据管理方案](./基于element_changes表的增量数据管理方案.md)
- [sesno 增量更新设计](./基于sesno的增量更新接口设计.md)

---

## 版本历史

| 版本 | 日期 | 作者 | 变更 |
|------|------|------|------|
| 1.0 | 2024-10-17 | Droid | 初始创建，包含 dbnum_info_table 完整设计 |
| | | | 包含参考号分解、保存流程、查询示例 |
| | | | 包含故障排查和扩展性考虑 |
