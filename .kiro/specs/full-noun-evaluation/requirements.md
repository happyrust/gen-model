# Requirements Document

## Introduction

本文档评估 Full Noun 模型生成系统的当前状态，识别已完成的优化工作和待完善的功能。Full Noun 模式是一种通过遍历所有 Noun 类型来生成完整 3D 模型的方法，用于工厂设计数据的可视化。

## Glossary

- **Full Noun Mode**: 全 Noun 模式，通过遍历所有 Noun 类型（CATE、LOOP、PRIM）生成完整模型的方法
- **Noun**: PDMS 数据库中的元素类型标识符（如 PIPE、ELBO、VALVE 等）
- **Refno**: 元素的唯一引用编号，用于标识数据库中的具体实例
- **CATE**: 使用元件库的 Noun 类别（Catalogue-based elements）
- **LOOP**: Loop owner Noun 类别，管道回路相关元素
- **PRIM**: 基本体 Noun 类别，基础几何图元
- **SJUS Map**: 存储 Loop 元素的空间位置和尺寸信息的映射表
- **Batch Processing**: 批量处理，将大量数据分批次处理以提高效率
- **DbOption**: 数据库配置选项结构体
- **gen_model Module**: 模型生成模块，负责将数据库数据转换为 3D 几何体
- **Legacy Implementation**: 旧版实现，指 gen_model_old.rs 中的原始代码
- **Refactored Implementation**: 重构实现，指 gen_model/ 目录下的模块化代码

## Requirements

### Requirement 1: 代码质量评估

**User Story:** 作为开发者，我希望了解当前代码的质量状况，以便制定改进计划

#### Acceptance Criteria

1. THE System SHALL 识别出旧版实现（gen_model_old.rs）的代码规模为 2,095 行
2. THE System SHALL 识别出重构版本已将代码拆分为 14 个模块文件
3. THE System SHALL 验证每个重构模块文件的行数不超过 250 行
4. THE System SHALL 识别出旧版实现中存在 90% 代码重复的三个函数（process_cate_nouns、process_loop_nouns、process_prim_nouns）
5. THE System SHALL 确认重构版本通过 NounProcessor 通用处理器消除了代码重复

### Requirement 2: 功能完整性评估

**User Story:** 作为系统架构师，我希望了解重构版本的功能完整性，以便判断是否可以切换到新实现

#### Acceptance Criteria

1. THE System SHALL 识别出当前生产环境仍在使用旧版实现（gen_model_old.rs）
2. THE System SHALL 确认 mod.rs 中存在 `pub use gen_model_old::gen_all_geos_data` 的临时兼容代码
3. THE System SHALL 验证重构版本的 legacy.rs 中 gen_all_geos_data 函数包含 TODO 标记
4. THE System SHALL 识别出重构版本缺少几何体数据接收和处理逻辑
5. THE System SHALL 识别出重构版本缺少 mesh 生成和布尔运算集成

### Requirement 3: 配置管理评估

**User Story:** 作为运维人员，我希望了解配置管理的改进情况，以便正确配置系统

#### Acceptance Criteria

1. THE System SHALL 确认重构版本实现了类型安全的 Concurrency 配置（范围 2-8）
2. THE System SHALL 确认重构版本实现了类型安全的 BatchSize 配置（范围 10-1000）
3. THE System SHALL 验证 FullNounConfig 结构体整合了所有 Full Noun 相关配置
4. THE System SHALL 识别出配置验证逻辑会自动限制超出范围的值并发出警告
5. THE System SHALL 确认配置可以从 DbOptionExt 结构体创建

### Requirement 4: 性能优化评估

**User Story:** 作为性能工程师，我希望了解性能优化的实施情况，以便评估性能提升

#### Acceptance Criteria

1. THE System SHALL 确认重构版本实现了顺序执行策略（LOOP -> PRIM -> CATE）
2. THE System SHALL 验证每个类别内部使用批量并发处理
3. THE System SHALL 识别出重构版本使用 CategorizedRefnos 替代三个独立 HashSet 以优化内存
4. THE System SHALL 确认重构版本实现了 SJUS map 验证功能
5. WHEN validate_sjus_map 配置启用时，THE System SHALL 在 SJUS map 为空时发出警告或报错

### Requirement 5: 错误处理评估

**User Story:** 作为开发者，我希望了解错误处理的改进情况，以便更好地诊断问题

#### Acceptance Criteria

1. THE System SHALL 确认重构版本定义了 FullNounError 枚举类型
2. THE System SHALL 验证 FullNounError 包含 EmptySjusMap、InvalidConcurrency、InvalidBatchSize 等错误类型
3. THE System SHALL 确认错误类型使用 thiserror 库提供清晰的错误消息
4. THE System SHALL 验证 NounProcessor 在处理失败时返回带上下文的错误信息
5. THE System SHALL 确认配置验证失败时提供明确的错误范围提示

### Requirement 6: 测试覆盖评估

**User Story:** 作为质量保证工程师，我希望了解测试覆盖情况，以便评估代码质量

#### Acceptance Criteria

1. THE System SHALL 识别出重构版本包含单元测试模块
2. THE System SHALL 验证 config.rs 包含 Concurrency 和 BatchSize 的测试用例
3. THE System SHALL 验证 full_noun_mode.rs 包含 SJUS map 验证的测试用例
4. THE System SHALL 识别出旧版实现（gen_model_old.rs）缺少单元测试
5. THE System SHALL 确认测试覆盖了边界条件（如并发数为 0、超出范围等）

### Requirement 7: 迁移路径评估

**User Story:** 作为项目经理，我希望了解从旧版到新版的迁移路径，以便制定实施计划

#### Acceptance Criteria

1. THE System SHALL 识别出 legacy.rs 提供了兼容层以保持 API 向后兼容
2. THE System SHALL 确认 gen_all_geos_data 函数可以根据配置选择使用优化版本
3. THE System SHALL 识别出迁移过程中需要实现的 TODO 项
4. THE System SHALL 验证旧版实现被重命名为 gen_model_old.rs 并保留作为参考
5. THE System SHALL 确认文档（MIGRATION_GUIDE.md）提供了迁移指南

### Requirement 8: 依赖关系评估

**User Story:** 作为架构师，我希望了解模块间的依赖关系，以便评估架构合理性

#### Acceptance Criteria

1. THE System SHALL 确认 full_noun_mode.rs 依赖 processor.rs 提供的通用处理器
2. THE System SHALL 验证 processor.rs 依赖 context.rs 提供的处理上下文
3. THE System SHALL 确认各处理器（cate_processor、loop_processor、prim_processor）独立实现具体逻辑
4. THE System SHALL 验证 legacy.rs 作为适配层连接新旧实现
5. THE System SHALL 确认模块间依赖关系清晰，无循环依赖

### Requirement 9: 数据完整性评估

**User Story:** 作为数据工程师，我希望了解数据完整性保障措施，以便确保生成结果正确

#### Acceptance Criteria

1. THE System SHALL 识别出旧版实现使用空的 SJUS map 存在数据完整性风险
2. THE System SHALL 确认重构版本实现了 validate_sjus_map 函数检查数据完整性
3. WHEN strict_validation 启用时，THE System SHALL 在 SJUS map 为空时返回错误
4. WHEN strict_validation 禁用时，THE System SHALL 在 SJUS map 为空时仅发出警告
5. THE System SHALL 确认 CategorizedRefnos 提供统计信息以验证处理结果

### Requirement 10: 文档完整性评估

**User Story:** 作为新加入的开发者，我希望有完整的文档，以便快速理解系统

#### Acceptance Criteria

1. THE System SHALL 确认存在 FULL_NOUN_OPTIMIZATION_PLAN.md 详细说明优化方案
2. THE System SHALL 确认存在 FULL_NOUN_README.md 提供快速开始指南
3. THE System SHALL 验证文档包含架构对比图和执行流程说明
4. THE System SHALL 确认文档说明了已完成的工作和待完成的任务
5. THE System SHALL 验证代码注释清晰说明了模块职责和关键逻辑
