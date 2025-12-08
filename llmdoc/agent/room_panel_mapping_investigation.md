<!-- 房间面板映射构建流程调查报告 -->

### 代码证据部分

#### 代码位置和符号引用

- `src/fast_model/room_model.rs` (lines 272-350) - `build_room_panels_relate()` 主函数及其通用实现
- `src/fast_model/room_model.rs` (lines 227-269) - `build_room_panel_query_sql()` SQL构建函数
- `src/fast_model/room_model.rs` (lines 352-373) - `create_room_panel_relations_batch()` 批量关系创建
- `src/fast_model/room_model.rs` (lines 286-350) - `build_room_panels_relate_common()` 通用处理逻辑
- `src/fast_model/room_model.rs` (lines 753-761) - `match_room_name_hd()` 和 `match_room_name_hh()` 房间名称匹配函数
- `src/fast_model/room_model.rs` (lines 715-734) - `collect_geometry_hashes()` 几何哈希收集函数
- `src/fast_model/room_model.rs` (lines 134-171) - `build_room_relations()` 主入口函数
- `src/fast_model/room_model.rs` (lines 174-225) - `compute_room_relations()` 并发计算函数

#### 外部依赖函数调用

- `aios_core::RefnoEnum` - 引用号枚举类型，用于表示房间和面板的唯一标识
- `aios_core::SUL_DB.query()` - SurrealDB 异步查询接口
- `aios_core::options::DbOption` - 数据库配置选项对象
- `aios_core::RecordId` - SurrealDB 记录ID类型
- `DbOption::get_room_key_word()` - 获取房间关键词配置列表
- `DbOption::get_meshes_path()` - 获取网格文件所在目录
- `RefnoEnum::from(RecordId)` - 从 SurrealDB RecordId 转换为 RefnoEnum
- `RefnoEnum::is_valid()` - 验证 RefnoEnum 有效性
- `RefnoEnum::to_pe_key()` - 将 RefnoEnum 转换为 PE Key 字符串格式

---

### 调查报告

#### result

##### 1. `build_room_panels_relate()` 函数完整实现逻辑

**函数签名**：
```rust
async fn build_room_panels_relate(
    room_key_word: &Vec<String>,
) -> anyhow::Result<Vec<(RefnoEnum, String, Vec<RefnoEnum>)>>
```

**执行流程**：

1. **条件编译分支**（第 275-282 行）：
   - 如果启用 `project_hd` 特性：使用 `match_room_name_hd` 函数进行房间名称过滤（HD项目规则：单字母+3位数字，如 A123）
   - 如果启用 `project_hh` 特性：使用 `match_room_name_hh` 函数进行房间名称过滤（HH项目规则：接受所有房间名称）
   - 默认情况：不进行名称过滤（接受所有）

2. **调用通用函数**（第 276-282 行）：
   - 实际处理逻辑在 `build_room_panels_relate_common()` 中，这三个分支只是传入不同的房间名称匹配函数

##### 2. `build_room_panel_query_sql()` SQL查询构建

**功能**：根据房间关键词生成 SurrealDB 递归查询 SQL

**SQL查询逻辑**（第 228-269 行）：

```sql
select value [  id,
                array::last(string::split(NAME, '-')),
                array::flatten(@.{1..2+collect}.children)[?noun='PANE'].id
            ] from FRMW where {filter}
```

**关键要素**：
- 查询源表：
  - `project_hd` 特性：从 FRMW 表查询（框架/Frame）
  - `project_hh` 特性：从 SBFR 表查询（船/Ship Frame）
  - 默认：从 FRMW 表查询
- 过滤条件：`room_key_word` 中任意一个关键词包含在房间的 NAME 字段中（`'KEYWORD' in NAME`）
- 递归查询：`@.{1..2+collect}.children` 表示递归查询 1-2 层深度的子元素
- 面板过滤：`[?noun='PANE']` 只选择 noun 字段值为 'PANE' 的子元素

**返回结果结构**：`Vec<(RecordId, String, Vec<RecordId>)>`
- 第 1 元素：房间 ID（RecordId）
- 第 2 元素：房间号字符串（从 NAME 字段中提取，用 '-' 分割后取最后一个）
- 第 3 元素：该房间下所有面板的 ID 列表

##### 3. `build_room_panels_relate_common()` 核心处理流程

**执行阶段**（第 286-350 行）：

**阶段 1：SQL执行和原始结果转换**（第 293-298 行）
```
构建SQL → 执行查询 → 获取原始结果 Vec<(RecordId, String, Vec<RecordId>)>
```

**阶段 2：结果转换和过滤**（第 301-336 行）
- 逐条处理原始结果：
  - 使用传入的 `match_room_fn` 验证房间号格式（第 305 行）
    - 房间号不匹配时记录 debug 日志并跳过
  - 将房间 RecordId 转换为 RefnoEnum（第 311 行）
    - 若转换失败或无效，记录 warn 日志并跳过（第 312-314 行）
  - 将面板 RecordId 列表转换为 RefnoEnum 列表（第 317-327 行）
    - 同时过滤无效的面板引用（第 321-325 行）
  - 若转换后面板列表为空，记录 debug 日志并跳过（第 329-332 行）
  - 成功处理的元组被添加到 `room_groups` 向量（第 334 行）

**阶段 3：批量数据库操作**（第 338-341 行）
- 若 `room_groups` 非空，调用 `create_room_panel_relations_batch()` 批量创建关系

**阶段 4：日志记录和返回**（第 343-349 行）
- 记录完成信息：处理的关系数和耗时
- 返回转换后的 `room_groups` 数据

##### 4. `create_room_panel_relations_batch()` 批量关系创建

**执行流程**（第 352-373 行）：

1. **SQL语句生成**（第 356-366 行）：
   - 对每个 `(room_refno, room_num_str, panel_refnos)` 三元组生成一条 RELATE SQL
   ```sql
   relate {room_refno_pe_key}->room_panel_relate->[{panel_refno_list}] set room_num='{room_num_str}';
   ```
   - 其中 `room_refno_pe_key` 是房间的 PE Key 字符串
   - `panel_refno_list` 是逗号分隔的面板 PE Key 字符串列表
   - `room_num_str` 是房间号字符串

2. **批量执行**（第 369-370 行）：
   - 将所有 SQL 语句用换行符连接成单个批量SQL
   - 一次性提交给 SUL_DB 执行

##### 5. 房间关键词查询和匹配机制

**房间关键词来源**：`DbOption::get_room_key_word()` （第 138 行）
- 从配置文件（DbOption.toml）读取
- 返回值为 `Vec<String>` 列表

**房间名称匹配规则**（第 753-761 行）：

- **HD项目规则** (`match_room_name_hd`)：
  ```
  正则表达式: ^[A-Z]\d{3}$
  格式：单个大写字母 + 3位数字
  示例：A123、B456、Z999
  ```

- **HH项目规则** (`match_room_name_hh`)：
  ```
  接受所有房间号（无格式限制）
  ```

- **默认规则**：
  ```
  无限制（第 282 行 `|_| true`）
  ```

##### 6. 返回数据结构详解

**返回类型**：`Vec<(RefnoEnum, String, Vec<RefnoEnum>)>`

每个三元组表示：
- **第 1 元素 (RefnoEnum)**：房间对象的引用号
  - 类型：从 SurrealDB RecordId 转换而来
  - 用途：数据库中唯一标识房间记录

- **第 2 元素 (String)**：房间号
  - 内容：从房间 NAME 字段中提取，使用 '-' 分割后取最后一个部分
  - 用途：供人类可读的房间标识符

- **第 3 元素 (Vec<RefnoEnum>)**：该房间下所有面板的引用号列表
  - 内容：通过递归查询 FRMW/SBFR 表的 1-2 层子元素获得
  - 过滤：仅包含 noun='PANE' 的元素
  - 用途：建立房间与面板的直接映射关系

---

#### conclusions

**关键架构发现**：

1. **面板数据来源**：面板数据不是单独查询的，而是通过递归查询房间记录的子层级（1-2层深度）获取，这种设计利用了 PDMS 的层级结构。

2. **房间关键词匹配机制**：通过在 SQL 中使用 `'KEYWORD' in NAME` 条件进行模糊匹配，支持多个关键词的 OR 组合，灵活性高。

3. **两步转换策略**：RecordId → RefnoEnum 的转换分两个阶段：
   - 先转换后验证有效性（`is_valid()`）
   - 这样可以过滤掉数据库中的脏数据或无效记录

4. **批量操作优化**：建立房间面板关系时采用批量 RELATE 语句，一次性提交所有关系到数据库，减少往返次数。

5. **项目特定配置**：使用条件编译特性标志 (`project_hd`, `project_hh`)，允许在编译时选择目标项目的规则，避免运行时多余的判断。

6. **房间号提取规则**：房间号是从 NAME 字段动态提取的，使用 `-` 作为分隔符，这假设房间名称的最后一个部分是房间号。

7. **边界条件处理**：函数在以下情况下会跳过处理：
   - 房间号不符合项目规则
   - RecordId 转换失败或无效
   - 房间下没有面板（面板列表为空）
   - 面板 RecordId 无效

8. **性能特性**：
   - 使用递归 SQL 在单次查询中获得房间和面板的完整映射，避免 N+1 查询问题
   - 批量数据库操作减少往返延迟
   - 使用异步 await 避免阻塞

---

#### relations

**函数调用关系链**：

```
build_room_relations() (主入口，第 134 行)
  ↓
  ├─ get_room_key_word() (获取配置)
  ├─ build_room_panels_relate() (第 144 行，条件编译分支)
  │   ↓
  │   ├─ [feature: project_hd] match_room_name_hd() (房间验证)
  │   ├─ [feature: project_hh] match_room_name_hh() (房间验证)
  │   └─ build_room_panels_relate_common() (第 276-282 行，核心逻辑)
  │       ↓
  │       ├─ build_room_panel_query_sql() (第 295 行，SQL构建)
  │       ├─ SUL_DB.query() (第 297 行，执行查询)
  │       ├─ RefnoEnum::from() (第 311, 320 行，类型转换)
  │       ├─ RefnoEnum::is_valid() (第 312, 321 行，有效性验证)
  │       └─ create_room_panel_relations_batch() (第 340 行，批量写入)
  │           ↓
  │           ├─ RefnoEnum::to_pe_key() (第 361-362 行，格式转换)
  │           └─ SUL_DB.query() (第 370 行，执行写入)
  │
  ├─ collect_geometry_hashes() (从面板收集几何哈希，第 715 行)
  │   ↓
  │   └─ aios_core::query_insts() (查询几何实例)
  │
  └─ compute_room_relations() (第 153 行，计算房间内构件)
      ↓
      └─ cal_room_refnos() (对每个面板计算房间内的构件)

```

**数据流转关系**：

```
DbOption (配置)
  ├─ get_room_key_word() → Vec<String> (房间关键词)
  └─ get_meshes_path() → PathBuf (网格目录)
           ↓
    build_room_panels_relate()
           ↓
    SurrealDB (SQL查询)
           ↓
    Vec<(RecordId, String, Vec<RecordId>)> (原始结果)
           ↓
    转换与过滤 (RefnoEnum + 验证)
           ↓
    Vec<(RefnoEnum, String, Vec<RefnoEnum>)> (最终映射)
           ↓
    批量RELATE语句
           ↓
    SurrealDB (房间面板关系存储)
```

**与其他模块的关系**：

- `room_model.rs` 中的 `build_room_panels_relate()` 是 `build_room_relations()` 的第一阶段
- 返回的 `room_panel_map` 随后被 `compute_room_relations()` 使用，进行第二阶段的房间内构件计算
- 最终的房间面板关系被存储在 SurrealDB 的 `room_panel_relate` 关系表中，供后续的房间查询使用（如 `query_room_by_point()` API）

**异常处理和错误传播**：

```
build_room_panels_relate()
  └─ Result::Err? (数据库查询失败)
       ↓
       返回给 build_room_relations()
       ↓
       propagate 给调用方 (room_api.rs 中的 execute_rebuild_relations())
```

**关键字段映射**：

- `room_key_word` (输入参数) → SQL FILTER → 房间 NAME 字段匹配
- `room_thing` (RecordId) → RefnoEnum::from() → room_refno (使用)
- `room_num` (String) → 直接传递 → room_num_str (保存)
- `panel_things` (Vec<RecordId>) → RefnoEnum::from() → panel_refnos (使用)

---

### 补充信息

#### 环境特性依赖

该函数在以下情况下使用：

- **编译特性**：`sqlite-index` 特性启用时可用（非 WASM 目标）
- **项目配置**：
  - `project_hd`：HD项目（推荐房间号格式 ^[A-Z]\d{3}$）
  - `project_hh`：HH项目（接受任意房间号）

#### 可测试的代码单元

测试用例位于：
- `src/fast_model/room_model.rs` (lines 764-799，测试模块)
- `src/test/test_room_v2_verification.rs` - V2 验证测试
- `src/test/test_room_integration.rs` - 集成测试

#### 监控和调试信息

函数执行时输出的关键日志：

```
- info!("开始构建房间关系 (改进版本)")
- debug!("跳过不匹配的房间号: {}")
- warn!("无效的房间引用号: {:?}")
- debug!("房间 {} 没有关联的面板")
- info!("房间面板关系构建完成: {} 个关系, 耗时 {:?}")
```

