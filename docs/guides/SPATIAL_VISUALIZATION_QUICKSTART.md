# 空间查询可视化 - 快速开始指南

## 🚀 快速开始

### 1. 后端编译和运行

```bash
# 编译项目(调试模式)
cargo build

# 运行项目
cargo run
```

后端服务将在 `http://localhost:8080` 启动。

### 2. 前端开发

```bash
# 进入前端目录
cd frontend/v0-aios-database-management

# 安装依赖(如果还未安装)
pnpm install

# 启动开发服务器
pnpm dev
```

前端应用将在 `http://localhost:3000` 启动。

### 3. 访问功能

打开浏览器访问:
```
http://localhost:3000/spatial-visualization
```

或从侧边栏导航:
- 点击"空间查询" → "可视化查询"

## 📝 使用示例

### 查询空间

1. 在参考号输入框中输入: `24381`
2. 点击"查询"按钮
3. 系统会显示该空间的信息和所有房间

### 查询房间

1. 在参考号输入框中输入: `24382`
2. 点击"查询"按钮
3. 系统会显示该房间的信息和所有构件

### 使用高级树形视图

1. 查询后,点击"高级树形"按钮
2. 在搜索框中输入关键词(例如: "PIPE")
3. 点击"房间"按钮只显示房间节点
4. 查看统计信息

### 使用流程图视图

1. 查询后,点击"流程图视图"按钮
2. 拖拽节点调整位置
3. 使用鼠标滚轮缩放
4. 点击小地图快速导航

## 🔍 API端点

### 查询节点
```bash
curl http://localhost:8080/api/spatial/query/24381
```

### 查询子节点
```bash
curl http://localhost:8080/api/spatial/children/24381
```

### 获取节点信息
```bash
curl http://localhost:8080/api/spatial/node-info/24381
```

## 📊 节点类型

| 类型 | 图标 | 颜色 | 示例 |
|------|------|------|------|
| SPACE | 🏢 | 蓝色 | FRMW, SBFR |
| ROOM | 🚪 | 绿色 | PANE |
| COMPONENT | ⚙️ | 紫色 | PIPE, ELBO, EQUI |

## 🛠️ 开发指南

### 修改节点样式

编辑 `components/spatial-query/nodes/` 中的组件文件:
- `SpaceNode.tsx` - 空间节点
- `RoomNode.tsx` - 房间节点
- `ComponentNode.tsx` - 构件节点

### 添加新的过滤类型

在 `AdvancedSpatialVisualization.tsx` 中:
1. 添加新的过滤按钮
2. 更新 `filterType` 状态
3. 修改过滤逻辑

### 自定义API端点

在 `src/web_api/spatial_query_api.rs` 中修改:
- 路由定义
- 查询逻辑
- 响应格式

## 🧪 测试

### 运行前端测试
```bash
cd frontend/v0-aios-database-management
pnpm test
```

### 运行后端测试
```bash
cargo test web_api::tests
```

## 📁 项目结构

```
后端:
├── src/
│   ├── web_api/
│   │   ├── spatial_query_api.rs    # API实现
│   │   ├── mod.rs                  # 模块导出
│   │   └── tests.rs                # 单元测试
│   ├── web_server/
│   │   ├── handlers.rs             # 页面处理器
│   │   └── mod.rs                  # 路由集成
│   └── lib.rs                      # 模块导出

前端:
├── app/
│   └── spatial-visualization/
│       └── page.tsx                # 主页面
├── components/
│   └── spatial-query/
│       ├── SpatialVisualization.tsx
│       ├── AdvancedSpatialVisualization.tsx
│       ├── ReactFlowVisualization.tsx
│       └── nodes/
│           ├── SpaceNode.tsx
│           ├── RoomNode.tsx
│           └── ComponentNode.tsx
├── __tests__/
│   └── spatial-visualization.test.tsx
└── docs/
    └── SPATIAL_VISUALIZATION.md
```

## 🐛 常见问题

### Q: 查询返回"查询失败"
**A:** 
- 检查参考号是否正确
- 确保后端服务正在运行
- 查看浏览器控制台的错误信息

### Q: 节点不显示子节点
**A:**
- 该节点可能没有子节点
- 尝试点击展开按钮
- 检查数据库中是否有相关数据

### Q: 流程图视图加载缓慢
**A:**
- 减少查询的数据量
- 使用过滤功能
- 切换到简单树形视图

### Q: 如何清除缓存
**A:**
- 刷新页面
- 清除浏览器缓存
- 重新启动开发服务器

## 📚 相关文档

- [完整功能文档](frontend/v0-aios-database-management/docs/SPATIAL_VISUALIZATION.md)
- [实现总结](SPATIAL_VISUALIZATION_IMPLEMENTATION.md)

## 🔗 相关链接

- [React Flow文档](https://reactflow.dev/)
- [Next.js文档](https://nextjs.org/)
- [Tailwind CSS文档](https://tailwindcss.com/)

## 💡 提示

1. **性能优化**: 使用高级树形视图的搜索功能来减少显示的节点数量
2. **大数据处理**: 流程图视图最适合处理大规模数据
3. **快速导航**: 使用小地图快速定位节点
4. **批量操作**: 可以逐个查询多个参考号

## 📞 支持

如有问题或建议,请联系开发团队。

---

**最后更新**: 2025-10-29
**版本**: 1.0.0

