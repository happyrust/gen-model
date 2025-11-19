# 模型生成错误码说明

本文档说明模型生成过程中使用的错误码分类体系和统计方法。

## 功能概述

使用 `--log-model-error` 参数可以记录模型生成过程中的所有问题，输出标准化的错误日志用于后续统计分析。

### 使用方法

```bash
# 记录模型生成错误（自动开启 debug-model + errors-only 模式）
cargo run -- --config DbOption --log-model-error

# 重定向错误日志到文件
cargo run -- --config DbOption --log-model-error 2>&1 | grep "\[MODEL_ERROR\]" > logs/model_errors.log

# 结合其他参数使用
cargo run -- --config DbOption --log-model-error --debug-model 14207/545
```

## 错误码分类体系

### 数据/引用问题 (E-REF / E-DATA)

#### E-REF-001: 获取元件库引用失败
- **类型**: InvalidReference
- **阶段**: get_cat_refno
- **说明**: 设计元件无法找到对应的元件库引用（CATA）
- **可能原因**:
  - 元件库引用缺失
  - 数据库关系损坏
  - CATA 表数据不完整

#### E-REF-002: GMSE 查询失败 / Owner 无效
- **类型**: DbInconsistent / InvalidReference
- **阶段**: query_gmse / validate_owner
- **说明**: 无法查询 GMSE 或元件 owner 无效
- **可能原因**:
  - 元件库几何数据缺失
  - 数据库关系不一致
  - owner 字段为空或无效

#### E-DATA-001: GMSE 和 NGMR 都无效
- **类型**: DataMissing
- **阶段**: validate_gmse_ngmr
- **说明**: 元件既没有正实体（GMSE）也没有负实体（NGMR）
- **可能原因**:
  - 元件库数据不完整
  - 几何定义缺失
  - 数据导入问题

### 表达式计算问题 (E-EXPR)

#### E-EXPR-001: 表达式计算失败
- **类型**: InvalidGeometry
- **阶段**: resolve_cata_comp / gen_cata_single_geoms
- **说明**: PDMS 表达式解析或计算失败
- **可能原因**:
  - 表达式语法错误
  - ATTRIB 关键字处理失败
  - 参数未定义或类型错误
  - MIN/MAX 函数参数不正确
  - PARA 数组索引越界
  - 变量在上下文中未定义
  - 括号不匹配

### 几何/拓扑问题 (E-GEO)

#### E-GEO-002: 几何数据无效
- **类型**: InvalidGeometry
- **阶段**: gen_cata_single_geoms
- **说明**: 几何解析或转换过程中出错
- **可能原因**:
  - 多边形不闭合
  - 截面自交
  - 几何参数非法
  - profile 定义错误

#### E-GEO-003: 元件没有生成任何几何
- **类型**: UnsupportedGeometry
- **阶段**: convert_to_csg_shapes
- **说明**: 元件的所有几何都转换失败
- **可能原因**:
  - 几何类型不支持
  - 所有几何都退化
  - 参数组合超出实现范围

### 流水线问题 (E-PIPE)

#### E-PIPE-001: 流水线错误
- **类型**: PipelineError
- **阶段**: gen_cata_single_geoms
- **说明**: 模型生成流程中的其他错误
- **可能原因**:
  - 属性解析失败
  - 坐标转换错误
  - 内部逻辑错误

## 日志格式

所有模型错误日志遵循以下格式（适合解析为表格）：

```
[MODEL_ERROR] code=<错误码> kind=<错误类型> stage=<阶段> refno=<元件号> desc="<简短描述>" msg=<详细信息>
```

### 字段说明

| 字段 | 说明 | 用途 |
|------|------|------|
| code | 错误码（如 E-REF-001） | 错误分类统计 |
| kind | 错误类型枚举 | 技术层面分类 |
| stage | 发生阶段 | 定位错误位置 |
| refno | 元件引用号 | 追踪具体模型 |
| desc | 简短描述（带引号） | 表格展示用 |
| msg | 详细信息 | 调试用详细数据 |

### 示例

```
[MODEL_ERROR] code=E-REF-001 kind=InvalidReference stage=get_cat_refno refno=14207/545 desc="获取元件库引用失败" msg=ele_refno=14207/545, result=Ok(None)

[MODEL_ERROR] code=E-GEO-003 kind=UnsupportedGeometry stage=convert_to_csg_shapes refno=21491/16521 desc="元件未生成任何几何" msg=design_refno=21491/16521, type_name=ELBO, geometries_len=2, n_geometries_len=0

[MODEL_ERROR] code=E-EXPR-001 kind=InvalidGeometry stage=resolve_cata_comp refno=14207/545 desc="表达式计算失败" msg=design_refno=14207/545, scom_ref=21491/16521, err=变量PARA5未定义
```

## 统计分析

### 手动统计

```bash
# 统计各类错误数量
grep "\[MODEL_ERROR\]" logs/model_errors.log | grep -oP "code=\K[^ ]+" | sort | uniq -c

# 统计受影响的模型数
grep "\[MODEL_ERROR\]" logs/model_errors.log | grep -oP "refno=\K[^ ]+" | sort -u | wc -l

# 按错误码分组查看详情
grep "\[MODEL_ERROR\]" logs/model_errors.log | grep "code=E-GEO-003"

# 提取描述字段（用于表格）
grep "\[MODEL_ERROR\]" logs/model_errors.log | grep -oP 'desc="\K[^"]+' | sort | uniq -c
```

### 解析为表格示例

使用简单脚本可将日志转换为 Markdown 表格：

```bash
# 按错误码分组统计
echo "| 错误码 | 描述 | 出现次数 | 示例 Refno |"
echo "|--------|------|----------|------------|"
grep "\[MODEL_ERROR\]" logs/model_errors.log | \
  awk -F' ' '{
    match($0, /code=([^ ]+)/, code);
    match($0, /desc="([^"]+)"/, desc);
    match($0, /refno=([^ ]+)/, refno);
    key = code[1] "|" desc[1];
    count[key]++;
    if (example[key] == "") example[key] = refno[1];
  }
  END {
    for (k in count) {
      split(k, parts, "|");
      printf "| %s | %s | %d | %s |\n", parts[1], parts[2], count[k], example[k];
    }
  }' | sort
```

预期输出类似：

| 错误码 | 描述 | 出现次数 | 示例 Refno |
|--------|------|----------|------------|
| E-REF-001 | 获取元件库引用失败 | 45 | 14207/545 |
| E-EXPR-001 | 表达式计算失败 | 23 | 21491/16521 |
| E-GEO-003 | 元件未生成任何几何 | 12 | 14207/1024 |

### 自动化工具（待开发）

计划开发一个 Rust/Python/Node 小工具，从日志文件自动生成错误统计报告：

```bash
# 未来使用方式（示例）
cargo run --bin analyze_model_errors -- logs/model_errors.log --output docs/model_gen_errors.md
```

生成的报告将包含：
- 按错误码分类的统计
- 每类错误的出现次数
- 受影响的模型列表
- 典型错误信息示例

## 最佳实践

1. **定期收集错误日志**: 在大规模模型生成后，使用 `--log-model-error` 收集错误统计。

2. **分析高频错误**: 优先修复出现次数最多的错误类型。

3. **保留历史记录**: 将错误日志按日期归档，追踪问题修复进度。

4. **结合具体模型调试**: 对于典型错误，使用 `--debug-model <refno>` 深入分析。

## 未来扩展

- [ ] 添加更多错误码（如导出错误、性能超时等）
- [ ] 支持错误日志写入 SurrealDB
- [ ] 开发自动化错误分析工具
- [ ] 添加错误趋势分析
- [ ] 支持错误日志可视化
