# 修复 has_tubi 字段反序列化错误

## 问题描述

运行 `cargo run --bin aios-database` 时出现反序列化错误：

```
Failed to deserialize field 'has_tubi' on type 'SPdmsElement': Expected bool, got none
```

## 根本原因

数据库中某些 `SPdmsElement` 记录的 `has_tubi` 字段为 null，而不是期望的 bool 类型，导致反序列化失败。

## 解决方案

1. **移除 has_tubi 字段**：
   - 从 `rs-core/src/types/pe.rs` 中移除 `has_tubi` 字段定义
   - 从 `rs-core/src/rs_surreal/inst_structs.rs` 中移除 `TubiRelate::to_surql` 方法中对 `has_tubi = true` 的设置

2. **更新相关代码逻辑**：
   - 修改 `src/fast_model/cata_model.rs` 中的代码，移除对 `has_tubi` 字段的更新逻辑
   - 修复 `src/dblist_parser/db_loader.rs` 中的导入路径问题

3. **保持查询逻辑不变**：
   - `rs-core/src/rs_surreal/inst.rs` 中的 `query_tubi_insts_by_brans` 函数已经直接使用 `tubi_relate` 表的 `in` 字段来查询 tubi 数据，无需修改

## 测试结果

- ✅ 代码编译成功
- ✅ 程序运行成功，能够正常处理 BRAN/HANG 元素的 tubi 生成
- ✅ 没有出现反序列化错误

## 相关文件

- `rs-core/src/types/pe.rs`
- `rs-core/src/rs_surreal/inst_structs.rs`
- `src/fast_model/cata_model.rs`
- `src/dblist_parser/db_loader.rs`

## 提交信息

- 类型：fix
- 影响范围：数据库查询、模型生成
- 测试环境：本地开发环境
- 验证步骤：运行 `cargo run --bin aios-database` 确认无反序列化错误