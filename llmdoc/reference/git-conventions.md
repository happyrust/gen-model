# Git 约定

## 提交信息格式

### 类型前缀
```
feat:     新功能
fix:      Bug 修复
refactor: 代码重构
chore:    构建/工具变更
docs:     文档更新
test:     测试相关
perf:     性能优化
```

### 示例
```
feat: 添加 --name-config 选项支持 Excel 名称映射导出
fix: 修复 TUBI 模型无法显示的问题
refactor: 完成 gen_model 模块化重构和测试文件优化
chore: 移除 BRAN 补充查询重复逻辑
```

### 中英文混合
项目允许中英文混合的提交信息，技术术语保持英文：
```
feat: CSG 优化与调试工具完善
fix: 修复布尔运算时 LOD mesh 文件路径查找问题
```

## 分支策略

### 分支命名
- `feature/*` - 功能开发分支
- `fix/*` - Bug 修复分支
- `refactor/*` - 重构分支

### 当前分支
```
feature/csg-optimization-20251206
```

## 工作流程

1. 从主分支创建功能分支
2. 开发完成后提交代码
3. 运行测试确保通过
4. 合并到主分支

## 注意事项
- 提交前运行 `cargo build` 确保编译通过
- 大型重构分多次小提交
- 保持提交信息简洁明了
