# Requirements Document

## Introduction

本需求文档定义了模型生成系统的深度模块化重构需求。当前系统已完成初步重构（将 2,095 行的 gen_model_old.rs 拆分为 14 个模块），但仍存在模块职责不清晰、耦合度高、可测试性差等问题。本次重构将进一步优化模块结构，建立清晰的分层架构，提升代码的可维护性、可测试性和可扩展性。

## 当前状态

已完成的工作：
- 将单文件 gen_model_old.rs (2,095 行) 拆分为 gen_model/ 目录下的 14 个模块
- 实现了 NounProcessor 通用处理器消除代码重复
- 引入了类型安全的配置管理（Concurrency、BatchSize、FullNounConfig）
- 实现了 CategorizedRefnos 优化内存使用
- 添加了 FullNounError 错误类型和 SJUS map 验证

待改进的问题：
- 模块间职责边界不清晰，存在交叉依赖
- 缺少清晰的分层架构（数据层、业务层、接口层）
- 处理器模块（cate_processor、loop_processor、prim_processor）与具体实现耦合
- 缺少统一的几何体生成接口抽象
- 测试覆盖不足，难以进行单元测试
- 配置和上下文传递方式不统一
- 缺少清晰的数据流和控制流文档

## Glossary

- **Model Generation System（模型生成系统）**: 将 PDMS 数据库数据转换为 3D 几何体的核心系统
- **Noun**: PDMS 数据库中的元素类型标识符（如 PIPE、ELBO、VALVE 等）
- **Refno**: 元素的唯一引用编号，用于标识数据库中的具体实例
- **CATE Processor（元件库处理器）**: 处理使用元件库的 Noun 类型（Catalogue-based elements）
- **LOOP Processor（回路处理器）**: 处理管道回路相关的 Loop owner Noun 类型
- **PRIM Processor（基本体处理器）**: 处理基础几何图元的 Prim Noun 类型
- **NounProcessContext（处理上下文）**: 包含处理所需的数据库连接、配置和状态信息
- **ShapeInstancesData（形状实例数据）**: 生成的几何体数据结构
- **Geometry Generator（几何体生成器）**: 负责将数据库数据转换为具体几何体的组件
- **Data Layer（数据层）**: 负责数据库访问和数据查询的底层模块
- **Business Layer（业务层）**: 负责业务逻辑和几何体生成的中间层模块
- **Interface Layer（接口层）**: 对外提供 API 的顶层模块
- **Dependency Injection（依赖注入）**: 通过构造函数或参数传递依赖，而非硬编码依赖关系
- **Trait Abstraction（特征抽象）**: 使用 Rust trait 定义接口契约，实现多态和解耦

## Requirements

### Requirement 1: 建立清晰的分层架构

**User Story:** 作为系统架构师，我希望建立清晰的三层架构（数据层、业务层、接口层），以便降低模块间耦合度并提升可维护性

#### Acceptance Criteria

1. THE System SHALL 定义数据层（Data Layer）模块，负责所有数据库查询和数据访问操作
2. THE System SHALL 定义业务层（Business Layer）模块，负责几何体生成的核心业务逻辑
3. THE System SHALL 定义接口层（Interface Layer）模块，提供对外的公共 API
4. THE System SHALL 确保数据层不依赖业务层，业务层不依赖接口层
5. THE System SHALL 通过依赖注入方式在各层之间传递依赖，避免硬编码依赖关系

### Requirement 2: 抽象几何体生成接口

**User Story:** 作为开发者，我希望定义统一的几何体生成接口，以便不同类型的 Noun 处理器遵循相同的契约

#### Acceptance Criteria

1. THE System SHALL 定义 GeometryGenerator trait，包含 generate 方法用于生成几何体
2. THE System SHALL 为 CATE、LOOP、PRIM 三种类型分别实现 GeometryGenerator trait
3. THE System SHALL 在 GeometryGenerator trait 中定义 validate 方法用于验证输入数据
4. THE System SHALL 在 GeometryGenerator trait 中定义 estimate_complexity 方法用于评估生成复杂度
5. THE System SHALL 确保所有几何体生成器可以通过 trait object 或泛型方式统一调用

### Requirement 3: 重构处理器模块职责

**User Story:** 作为开发者，我希望明确各处理器模块的职责边界，以便降低模块间耦合并提升代码可读性

#### Acceptance Criteria

1. THE System SHALL 将 cate_processor 重构为仅负责协调 CATE 类型的处理流程
2. THE System SHALL 将具体的 CATE 几何体生成逻辑移至独立的 cate_geometry 模块
3. THE System SHALL 将 loop_processor 重构为仅负责协调 LOOP 类型的处理流程
4. THE System SHALL 将具体的 LOOP 几何体生成逻辑移至独立的 loop_geometry 模块
5. THE System SHALL 将 prim_processor 重构为仅负责协调 PRIM 类型的处理流程
6. THE System SHALL 将具体的 PRIM 几何体生成逻辑移至独立的 prim_geometry 模块

### Requirement 4: 统一配置和上下文管理

**User Story:** 作为开发者，我希望统一配置和上下文的传递方式，以便简化函数签名并提升代码一致性

#### Acceptance Criteria

1. THE System SHALL 定义 GenerationConfig 结构体，整合所有模型生成相关的配置参数
2. THE System SHALL 定义 GenerationContext 结构体，包含数据库连接、配置、缓存和状态信息
3. THE System SHALL 确保 GenerationContext 实现 Clone trait 以支持并发场景
4. THE System SHALL 通过 Arc<GenerationContext> 在异步任务间共享上下文
5. THE System SHALL 移除函数签名中的冗余参数，统一使用 context 参数传递

### Requirement 5: 实现数据访问层抽象

**User Story:** 作为开发者，我希望抽象数据访问层，以便支持不同的数据源并提升可测试性

#### Acceptance Criteria

1. THE System SHALL 定义 DataRepository trait，包含查询 Noun、查询 Refno、批量查询等方法
2. THE System SHALL 实现 PdmsDataRepository 结构体，封装对 PDMS 数据库的访问
3. THE System SHALL 实现 MockDataRepository 结构体，用于单元测试
4. THE System SHALL 确保所有数据查询操作通过 DataRepository trait 进行
5. THE System SHALL 在 GenerationContext 中持有 DataRepository trait object

### Requirement 6: 优化并发处理策略

**User Story:** 作为性能工程师，我希望优化并发处理策略，以便提升模型生成的吞吐量和资源利用率

#### Acceptance Criteria

1. THE System SHALL 实现动态并发度调整，根据系统负载自动调整并发任务数
2. THE System SHALL 实现任务优先级队列，优先处理高优先级的几何体生成任务
3. THE System SHALL 实现批量处理优化，自动合并小批次以减少调度开销
4. THE System SHALL 实现背压机制，当下游处理速度慢时自动限制上游生成速度
5. THE System SHALL 提供并发性能监控指标，包括任务队列长度、活跃任务数、吞吐量等

### Requirement 7: 增强错误处理和恢复

**User Story:** 作为开发者，我希望增强错误处理和恢复机制，以便提升系统的健壮性和可诊断性

#### Acceptance Criteria

1. THE System SHALL 为每种错误类型定义明确的错误码和错误消息
2. THE System SHALL 实现错误上下文链，记录错误发生的完整调用栈和上下文信息
3. THE System SHALL 实现自动重试机制，对于临时性错误（如网络超时）自动重试
4. THE System SHALL 实现错误降级策略，当部分几何体生成失败时继续处理其他几何体
5. THE System SHALL 提供错误统计和报告功能，汇总生成过程中的所有错误信息

### Requirement 8: 完善单元测试覆盖

**User Story:** 作为质量保证工程师，我希望完善单元测试覆盖，以便确保代码质量和防止回归

#### Acceptance Criteria

1. THE System SHALL 为所有公共 API 编写单元测试，覆盖率达到 80% 以上
2. THE System SHALL 为关键业务逻辑编写集成测试，验证端到端流程
3. THE System SHALL 使用 MockDataRepository 编写隔离的单元测试，不依赖真实数据库
4. THE System SHALL 编写性能基准测试，监控关键路径的性能指标
5. THE System SHALL 编写并发安全测试，验证多线程场景下的数据一致性

### Requirement 9: 建立模块文档规范

**User Story:** 作为新加入的开发者，我希望每个模块都有清晰的文档，以便快速理解模块职责和使用方式

#### Acceptance Criteria

1. THE System SHALL 为每个模块文件添加模块级文档注释，说明模块职责和主要功能
2. THE System SHALL 为每个公共函数添加文档注释，包含参数说明、返回值说明和示例代码
3. THE System SHALL 为每个公共结构体和枚举添加文档注释，说明字段含义和使用场景
4. THE System SHALL 在 gen_model/README.md 中提供架构概览和模块关系图
5. THE System SHALL 在 gen_model/DESIGN.md 中详细说明设计决策和实现细节

### Requirement 10: 实现插件化扩展机制

**User Story:** 作为系统架构师，我希望实现插件化扩展机制，以便在不修改核心代码的情况下添加新的 Noun 类型支持

#### Acceptance Criteria

1. THE System SHALL 定义 NounTypePlugin trait，包含注册、查询和生成方法
2. THE System SHALL 实现 PluginRegistry 结构体，管理所有已注册的插件
3. THE System SHALL 支持在运行时动态注册和卸载插件
4. THE System SHALL 为 CATE、LOOP、PRIM 三种内置类型实现插件
5. THE System SHALL 提供插件开发指南和示例代码，说明如何添加新的 Noun 类型支持
