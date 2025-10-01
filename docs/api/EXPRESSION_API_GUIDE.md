# 表达式解析API使用指南

## 🎯 **核心API函数**

### **1. eval_pdms_expression() - 推荐使用** ⭐⭐⭐

**用途**: 评估PDMS表达式，支持ATTRIB关键字和数组索引语法

```rust
use aios_database::expression_fix::eval_pdms_expression;

// 基本使用
let result = eval_pdms_expression(
    "( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )",
    &context
)?;
```

**特点**:
- ✅ 自动处理ATTRIB关键字
- ✅ 支持数组索引语法 `PARA[n]`
- ✅ 使用默认单位 "DIST"
- ✅ 最简单的使用方式

---

### **2. eval_attrib_expression() - 高级使用** ⭐⭐

**用途**: 评估ATTRIB表达式，可指定单位

```rust
use aios_database::expression_fix::eval_attrib_expression;

// 指定单位
let result = eval_attrib_expression(
    "ATTRIB HEIG + ATTRIB WIDT",
    &context,
    "MM"  // 指定单位
)?;
```

**特点**:
- ✅ 支持自定义单位
- ✅ 完整的ATTRIB语法支持
- ✅ 适合需要精确控制的场景

---

### **3. eval_enhanced_expression() - 通用增强** ⭐

**用途**: 通用的增强表达式评估器

```rust
use aios_database::expression_fix::eval_enhanced_expression;

let result = eval_enhanced_expression(
    "MAX(ATTRIB PARA[0], ATTRIB PARA[1])",
    &context,
    "DIST"
)?;
```

**特点**:
- ✅ 通用增强功能
- ✅ 支持所有表达式类型
- ✅ 灵活的参数配置

---

## 🔧 **实用工具函数**

### **ExpressionFixer::preprocess_attrib_expression()**

**用途**: 仅预处理表达式，不求值

```rust
use aios_database::expression_fix::ExpressionFixer;

let processed = ExpressionFixer::preprocess_attrib_expression(
    "( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )"
);
// 结果: "(MIN (HEIG,PARA3))"
```

### **ExpressionFixer::create_test_context()**

**用途**: 创建测试上下文

```rust
let context = ExpressionFixer::create_test_context();
// 包含预定义的测试属性: HEIG, PARA0-5, WIDT, LENG, RADI
```

---

## 📋 **使用场景对照表**

| 场景 | 推荐函数 | 示例 |
|------|----------|------|
| **日常PDMS表达式** | `eval_pdms_expression` | `"ATTRIB HEIG"` |
| **需要指定单位** | `eval_attrib_expression` | `"ATTRIB WIDT", "MM"` |
| **复杂表达式** | `eval_enhanced_expression` | `"MAX(A, B) + C"` |
| **仅预处理** | `preprocess_attrib_expression` | 调试和分析 |
| **测试开发** | `create_test_context` | 单元测试 |

---

## 🚀 **快速开始示例**

### **基础使用**
```rust
use aios_database::expression_fix::eval_pdms_expression;
use aios_core::CataContext;
use dashmap::DashMap;

// 创建上下文
let mut context_map = DashMap::new();
context_map.insert("HEIG".to_string(), "100.0".to_string());
context_map.insert("PARA3".to_string(), "40.0".to_string());

let context = CataContext {
    context: context_map,
    is_tubi: false,
};

// 评估表达式
let result = eval_pdms_expression(
    "( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )",
    &context
)?;

println!("结果: {}", result); // 输出: 结果: 40
```

### **批量处理**
```rust
let expressions = vec![
    "ATTRIB HEIG",
    "ATTRIB PARA[0]",
    "MAX(ATTRIB WIDT, ATTRIB LENG)",
    "ATTRIB HEIG + ATTRIB PARA[1] * 2",
];

for expr in expressions {
    match eval_pdms_expression(expr, &context) {
        Ok(result) => println!("{} = {}", expr, result),
        Err(e) => println!("{} 错误: {}", expr, e),
    }
}
```

---

## ⚠️ **注意事项**

1. **上下文准备**: 确保所有用到的属性都在 `CataContext` 中定义
2. **错误处理**: 使用 `Result` 类型，记得处理错误情况
3. **单位一致性**: 注意表达式中各属性的单位要一致
4. **性能考虑**: 对于大量表达式，考虑复用上下文对象

---

## 🔄 **迁移指南**

### **从 eval_str_to_f64 迁移**

**旧代码**:
```rust
let result = eval_str_to_f64(expr, &context, "DIST")?;
```

**新代码**:
```rust
let result = eval_pdms_expression(expr, &context)?;
```

**优势**:
- ✅ 自动处理ATTRIB关键字
- ✅ 更好的错误提示
- ✅ 支持更多表达式格式

---

## 🎉 **总结**

- **日常使用**: `eval_pdms_expression()` 
- **高级控制**: `eval_attrib_expression()`
- **通用场景**: `eval_enhanced_expression()`
- **调试分析**: `preprocess_attrib_expression()`

选择合适的函数，让表达式解析更简单、更可靠！
