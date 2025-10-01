# XKT生成器自动化测试

## 📋 概述

本测试套件提供了XKT生成器功能的全面自动化测试，包括：
- **API测试**: 测试后端XKT生成接口
- **组件测试**: 测试前端React组件
- **E2E测试**: 测试完整的用户交互流程

## 🚀 快速开始

### 1. 环境准备

#### 启动后端服务（端口8080）
```bash
cargo run --features="web_ui" --bin web_ui
```

#### 启动前端服务（端口3001）
```bash
cd frontend/v0-aios-database-management
pnpm dev
```

### 2. 安装测试依赖
```bash
cd tests/xkt-generator
npm install
```

### 3. 运行测试

#### 运行所有测试
```bash
npm test
# 或
./run-tests.sh
```

#### 单独运行API测试
```bash
npm run test:api
# 或
node api.test.js
```

#### 单独运行E2E测试
```bash
npm run test:e2e
# 或
node e2e.test.js
```

#### 运行前端组件测试
```bash
npm run test:frontend
```

## 📝 测试用例

### API测试 (api.test.js)

| 测试用例 | 描述 | 预期结果 |
|---------|------|---------|
| 生成完整数据库XKT | 测试生成整个数据库的XKT文件 | 返回成功，文件名包含"compressed" |
| 生成指定参考号XKT | 测试生成特定区域的XKT文件 | 返回成功，文件名包含"refno" |
| 生成未压缩XKT | 测试生成未压缩的XKT文件 | 返回成功，文件名包含"raw" |
| 下载XKT文件 | 测试下载生成的文件 | 成功下载，文件大于500字节 |
| 无效数据库号 | 测试错误处理 | 返回错误或空模型 |
| 缺少必填参数 | 测试参数验证 | 返回错误状态 |
| 并发生成测试 | 测试并发请求处理 | 所有请求成功 |
| 文件大小比较 | 测试压缩效果 | 压缩率小于50% |

### 组件测试 (xkt-generator.test.tsx)

| 测试用例 | 描述 | 预期结果 |
|---------|------|---------|
| 组件渲染 | 测试所有UI元素正确渲染 | 显示输入框、按钮、标签页 |
| 默认值设置 | 测试默认参数 | dbno=1112, compress=true |
| 输入验证 | 测试必填字段验证 | 无数据库号时显示错误 |
| API调用 | 测试与后端交互 | 正确发送请求，处理响应 |
| 历史记录 | 测试生成历史管理 | 成功生成后添加到历史 |
| 下载功能 | 测试文件下载 | 创建下载链接 |
| UI交互 | 测试用户交互 | 加载状态、开关切换正常 |

### E2E测试 (e2e.test.js)

| 测试用例 | 描述 | 预期结果 |
|---------|------|---------|
| 页面导航 | 测试访问XKT生成器页面 | 成功导航到/xkt-generator |
| 生成基本XKT | 测试完整生成流程 | 文件生成并显示在历史中 |
| 带参考号生成 | 测试指定区域生成 | 历史记录包含参考号 |
| 压缩选项 | 测试压缩开关 | 状态正确切换 |
| 下载功能 | 测试文件下载 | 文件成功下载到本地 |
| 输入验证 | 测试错误处理 | 显示验证错误 |
| 响应式设计 | 测试不同屏幕尺寸 | 各尺寸下正常显示 |

## 🔧 测试配置

### 测试数据
```javascript
{
  validDbno: 1112,              // 有效的数据库号
  validRefno: '17496/266203',   // 有效的参考号
  invalidDbno: 99999,           // 无效的数据库号
  timeout: 30000                // 超时时间（毫秒）
}
```

### 服务端点
- 后端API: http://localhost:8080
- 前端应用: http://localhost:3001

## 📊 测试报告

测试完成后会生成详细报告，包括：
- ✅ 通过的测试数量
- ❌ 失败的测试数量
- 📈 通过率
- 🔍 失败测试的详细信息

### 示例报告
```
==============================================================
📊 测试报告
==============================================================
✅ 通过: 8
❌ 失败: 0
📈 通过率: 100.0%
```

## 🐛 调试模式

### API测试调试
```javascript
// 修改 api.test.js 中的配置
const TEST_CONFIG = {
  timeout: 60000  // 增加超时时间
};
```

### E2E测试调试
```javascript
// 修改 e2e.test.js 中的配置
const TEST_CONFIG = {
  headless: false,  // 显示浏览器窗口
  slowMo: 100      // 减慢操作速度
};
```

## 📸 错误截图

E2E测试失败时会自动截图保存在：
```
tests/xkt-generator/error-[timestamp].png
```

## 🔄 持续集成

### GitHub Actions配置
```yaml
name: XKT Generator Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions/setup-node@v2
        with:
          node-version: '18'
      - name: Install dependencies
        run: |
          cd tests/xkt-generator
          npm install
      - name: Run tests
        run: |
          cd tests/xkt-generator
          npm test
```

## 📝 注意事项

1. **服务依赖**: 测试需要后端和前端服务都在运行
2. **端口占用**: 确保8080和3001端口未被占用
3. **网络连接**: E2E测试需要稳定的网络环境
4. **浏览器**: E2E测试需要Chrome浏览器（Puppeteer会自动下载）

## 🤝 贡献指南

1. 添加新测试用例时，请遵循现有的命名规范
2. 每个测试应该是独立的，不依赖其他测试
3. 使用有意义的断言消息
4. 失败的测试应该提供足够的调试信息

## 📚 相关文档

- [XKT生成器功能文档](../../frontend/XKT_GENERATOR_README.md)
- [API接口文档](../../src/web_ui/handlers.rs)
- [前端组件文档](../../frontend/v0-aios-database-management/components/xkt-generator.tsx)

---

*测试套件版本: 1.0.0*
*最后更新: 2025-09-30*