# Issues 管理文档

## 📋 概述

这个文件夹用于管理gen-model项目的所有问题、bug报告、功能请求和改进建议。每个issue都有独立的markdown文件进行详细记录。

## 📁 文件命名规范

```
ISSUE-{编号}-{简短描述}.md
```

**示例**:
- `ISSUE-001-TUBI-inst-relate-missing.md`
- `ISSUE-002-performance-optimization.md`
- `ISSUE-003-memory-leak-fix.md`

## 🏷️ Issue 分类

### 按类型分类
- 🐛 **Bug**: 系统缺陷和错误
- ✨ **Feature**: 新功能请求
- 🔧 **Enhancement**: 现有功能改进
- 📚 **Documentation**: 文档相关
- 🚀 **Performance**: 性能优化
- 🔒 **Security**: 安全问题

### 按优先级分类
- 🔴 **Critical**: 严重问题，需要立即处理
- 🟠 **High**: 高优先级，尽快处理
- 🟡 **Medium**: 中等优先级
- 🟢 **Low**: 低优先级
- 🔵 **Nice to have**: 可选功能

### 按状态分类
- 📝 **Open**: 新建，待处理
- 🔄 **In Progress**: 处理中
- 🧪 **Testing**: 测试验证中
- ✅ **Fixed**: 已解决
- ❌ **Closed**: 已关闭
- 🚫 **Rejected**: 已拒绝

## 📝 Issue 模板

### 标准Issue模板

```markdown
# Issue #{编号}: {标题}

## 📋 Issue 信息
- **Issue ID**: #{编号}
- **标题**: {问题标题}
- **类型**: {Bug/Feature/Enhancement/etc} {emoji}
- **优先级**: {Critical/High/Medium/Low} {emoji}
- **状态**: {Open/In Progress/Fixed/etc} {emoji}
- **创建日期**: YYYY-MM-DD
- **解决日期**: YYYY-MM-DD (如果已解决)
- **负责人**: {负责人姓名}
- **相关模块**: {相关的代码模块}

## 🔍 问题描述
{详细描述问题}

## 🔬 问题分析
{问题的根本原因分析}

## 🛠️ 解决方案
{解决方案的详细描述}

## 🧪 测试验证
{如何测试和验证修复}

## 📊 修复效果
{修复前后的对比}

## 📚 相关文档
{相关的文档链接}

## 🔄 后续行动
{需要采取的后续行动}

## 🏷️ 标签
{相关标签}
```

## 📊 当前Issue统计

### 总体统计
- **总数**: 1
- **已解决**: 1
- **进行中**: 0
- **待处理**: 0

### 按类型统计
- 🐛 Bug: 1
- ✨ Feature: 0
- 🔧 Enhancement: 0
- 📚 Documentation: 0
- 🚀 Performance: 0

### 按优先级统计
- 🔴 Critical: 0
- 🟠 High: 1
- 🟡 Medium: 0
- 🟢 Low: 0

## 📋 Issue 列表

| ID | 标题 | 类型 | 优先级 | 状态 | 创建日期 |
|----|------|------|--------|------|----------|
| #001 | BRAN的TUBI对应的aabb和world_trans没有保存成功 | 🐛 Bug | 🟠 High | ✅ Fixed | 2025-01-01 |
| #021 | `insts_flat = NONE` 的读者可见残留（库 A 快照 40 行） | 🐛 Bug | 🟡 Medium | 📝 Open | 2026-08-25 |

> 上表长期没跟上：`issues/` 下另有 `ISSUE-019-cross-session-parent-child-delete.md`
> 与 `ISSUE-020-db8000-model-increment-ci-suite.md`，它们的状态本文件没有维护过，
> 补进表里等于替它们编一个状态，所以留白，以各自文件为准。
> 上面「当前 Issue 统计」那几个计数同样是 2025 年的旧值，未随本条更新。

## 🔄 工作流程

### 1. 创建Issue
1. 使用标准模板创建新的issue文件
2. 分配唯一的编号
3. 设置适当的类型、优先级和状态
4. 详细描述问题和分析

### 2. 处理Issue
1. 更新状态为"In Progress"
2. 分析问题根本原因
3. 设计和实施解决方案
4. 进行测试验证

### 3. 解决Issue
1. 更新状态为"Fixed"
2. 记录解决日期
3. 添加修复效果和验证结果
4. 更新相关文档

### 4. 关闭Issue
1. 确认问题已完全解决
2. 更新状态为"Closed"
3. 归档相关文档

## 🔍 搜索和查找

### 按状态查找
```bash
# 查找所有开放的issues
grep -l "状态.*Open" issues/ISSUE-*.md

# 查找所有已解决的issues
grep -l "状态.*Fixed" issues/ISSUE-*.md
```

### 按类型查找
```bash
# 查找所有Bug类型的issues
grep -l "类型.*Bug" issues/ISSUE-*.md

# 查找所有性能相关的issues
grep -l "Performance" issues/ISSUE-*.md
```

### 按优先级查找
```bash
# 查找高优先级issues
grep -l "优先级.*High" issues/ISSUE-*.md
```

## 📈 质量指标

### 响应时间目标
- 🔴 Critical: 2小时内响应
- 🟠 High: 24小时内响应
- 🟡 Medium: 3天内响应
- 🟢 Low: 1周内响应

### 解决时间目标
- 🔴 Critical: 24小时内解决
- 🟠 High: 1周内解决
- 🟡 Medium: 2周内解决
- 🟢 Low: 1个月内解决

## 📞 联系方式

如有问题或建议，请联系项目维护者。

---

**最后更新**: 2025-01-01  
**维护者**: AI Assistant
