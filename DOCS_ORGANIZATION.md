# 文档整理说明

## 目录结构

项目文档已按类型分类整理到 `docs/` 目录下：

```
docs/
├── database/          # 数据库相关 (17 个文件)
│   ├── 数据库比选报告
│   ├── SurrealDB 迁移文档
│   ├── Helix 查询文档
│   ├── 缓存实现文档
│   └── 性能分析报告
│
├── architecture/      # 架构设计 (20 个文件)
│   ├── 系统重构文档
│   ├── E3D 增量系统
│   ├── 数据接口层设计
│   └── IDA Pro 分析文档
│
├── xkt-generator/     # XKT 生成器 (11 个文件)
│   ├── XKT 格式规范
│   ├── 生成器架构
│   ├── 测试报告
│   └── Zone 分块方案
│
├── deployment/        # 部署与协作 (15 个文件)
│   ├── 部署站点文档
│   ├── LiteFS 同步方案
│   └── 协作功能设计
│
├── guides/            # 开发指南 (7 个文件)
│   ├── API 集成指南
│   ├── 错误处理指南
│   └── Web UI 开发文档
│
├── api/               # API 文档 (2 个文件)
│   └── 元数据接口
│
├── reports/           # 报告与日志 (2 个文件)
│   └── changelog
│
└── legacy/            # 历史遗留 (2 个文件)
    └── 旧版文档

development/           # 开发子目录 (独立保留)
```

## 保留在根目录的文档

- `readme.md` - 项目主要说明文档
- `README_WEB_UI.md` - Web UI 专项说明

## .gitignore 更新

已添加以下规则：
- 临时文件：`*.tmp`, `*.bak`, `*.swp`, `*.swo`
- 备份文件：`*~`
- 系统文件：`.DS_Store`
- 崩溃报告：`rustc-ice-*.txt`
- 日志文件：`*.log`, `*.db-shm`, `*.db-wal`
- Node 模块：`node_modules/`, `**/node_modules/`

## 统计信息

- 总计整理文档：75+ 个 Markdown 文件
- 文档分类：8 个主要类别
- 根目录保留：2 个 README 文件
