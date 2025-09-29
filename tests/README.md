# XKT文件测试套件

这个目录包含了用于测试XKT文件生成和加载的自动化测试工具。

## 文件结构

```
tests/
├── README.md              # 本说明文件
├── package.json           # Node.js依赖配置
├── test-xkt.js            # 主要的自动化测试脚本
├── xkt_v10_test.html      # XKT v10测试页面
├── test_xkt_viewer.html   # 完整的XKT查看器测试页面
└── run-test.sh            # 一键运行测试的shell脚本
```

## 快速开始

### 方法1: 使用一键脚本 (推荐)

```bash
# 在项目根目录运行
./tests/run-test.sh
```

这个脚本会自动：
- 安装Node.js依赖
- 下载Chrome浏览器
- 启动HTTP服务器
- 运行自动化测试
- 清理环境

### 方法2: 手动运行

```bash
# 1. 进入tests目录
cd tests

# 2. 安装依赖
npm install

# 3. 安装Chrome (如果需要)
npx puppeteer browsers install chrome

# 4. 启动HTTP服务器 (另一个终端)
python3 -m http.server 8082

# 5. 运行测试
npm test
```

## 测试内容

自动化测试会验证以下内容：

1. **HTTP服务器状态** - 确保服务器正常运行
2. **文件访问测试** - 验证所有必需文件可访问：
   - `xkt_v10_test.html` - 测试页面
   - `../output/cube_v10_fixed.xkt` - XKT文件
   - `../js/xeokit/xeokit-sdk.es.js` - xeokit SDK
3. **XKT文件加载测试** - 使用Puppeteer自动化浏览器：
   - 加载测试页面
   - 点击测试按钮
   - 监控加载状态
   - 捕获错误信息
   - 验证成功加载

## 测试输出

测试脚本会提供详细的报告，包括：

- ✅ 成功步骤
- ❌ 错误信息和堆栈跟踪
- 📊 最终测试报告
- ⏰ 时间戳

## 故障排除

### 常见问题

1. **Chrome未安装**
   ```bash
   npx puppeteer browsers install chrome
   ```

2. **端口被占用**
   - 修改`package.json`中的端口号
   - 或停止占用8082端口的进程

3. **XKT文件不存在**
   - 确保运行了Rust构建生成XKT文件
   ```bash
   cargo run --bin xkt_v10_cube_test -- -o output/cube_v10_fixed.xkt
   ```

4. **路径问题**
   - 确保从项目根目录运行测试
   - 检查相对路径设置

## 扩展测试

要添加新的测试：

1. 修改`test-xkt.js`中的测试逻辑
2. 添加新的测试HTML页面
3. 更新`filesToTest`数组
4. 添加相应的验证步骤

## 技术细节

- **测试框架**: Puppeteer (Chrome自动化)
- **HTTP服务器**: Python内置服务器
- **端口**: 8082 (避免与开发服务器冲突)
- **浏览器**: Headless Chrome
- **JavaScript模块**: ES6 modules