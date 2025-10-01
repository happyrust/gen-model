/**
 * XKT生成器端到端(E2E)测试
 * 使用Puppeteer测试完整的用户交互流程
 */

import puppeteer from 'puppeteer';
import { fileURLToPath } from 'url';
import path from 'path';
import fs from 'fs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// 测试配置
const TEST_CONFIG = {
  frontendUrl: 'http://localhost:3001',
  apiUrl: 'http://localhost:8080',
  timeout: 60000,
  headless: true, // 设为false可以看到浏览器操作
  slowMo: 50 // 减慢操作速度，便于观察
};

// 测试数据
const TEST_DATA = {
  validDbno: '1112',
  validRefno: '17496/266203',
  invalidDbno: '99999'
};

class E2ETestRunner {
  constructor() {
    this.browser = null;
    this.page = null;
    this.results = {
      passed: 0,
      failed: 0,
      errors: []
    };
  }

  async setup() {
    console.log('🚀 启动浏览器...');

    this.browser = await puppeteer.launch({
      headless: TEST_CONFIG.headless,
      slowMo: TEST_CONFIG.slowMo,
      args: ['--no-sandbox', '--disable-setuid-sandbox']
    });

    this.page = await this.browser.newPage();
    this.page.setDefaultTimeout(TEST_CONFIG.timeout);

    // 设置视口大小
    await this.page.setViewport({ width: 1366, height: 768 });

    // 监听控制台消息
    this.page.on('console', msg => {
      if (msg.type() === 'error') {
        console.log('浏览器错误:', msg.text());
      }
    });
  }

  async teardown() {
    if (this.browser) {
      await this.browser.close();
    }
  }

  async test(name, testFunction) {
    console.log(`\n🧪 测试: ${name}`);

    try {
      await testFunction.call(this);
      console.log(`  ✅ 通过`);
      this.results.passed++;
    } catch (error) {
      console.error(`  ❌ 失败: ${error.message}`);
      this.results.failed++;
      this.results.errors.push({ test: name, error: error.message });

      // 截图保存失败情况
      const screenshotPath = path.join(__dirname, `error-${Date.now()}.png`);
      await this.page.screenshot({ path: screenshotPath, fullPage: true });
      console.log(`  📸 错误截图: ${screenshotPath}`);
    }
  }

  /**
   * 测试1: 访问XKT生成器页面
   */
  async testNavigateToXKTGenerator() {
    // 访问主页
    await this.page.goto(TEST_CONFIG.frontendUrl);

    // 等待页面加载
    await this.page.waitForSelector('.ml-64'); // 主内容区域

    // 点击侧边栏的XKT生成器链接
    const xktLink = await this.page.waitForSelector('button:has-text("XKT生成器")', {
      timeout: 5000
    });

    if (!xktLink) {
      // 备用选择器
      await this.page.click('text=XKT生成器');
    } else {
      await xktLink.click();
    }

    // 等待页面跳转
    await this.page.waitForFunction(
      () => window.location.pathname === '/xkt-generator',
      { timeout: 5000 }
    );

    // 验证页面标题
    const title = await this.page.$eval('h1', el => el.textContent);
    if (!title.includes('XKT 模型生成工具')) {
      throw new Error(`页面标题不正确: ${title}`);
    }
  }

  /**
   * 测试2: 生成基本XKT文件
   */
  async testGenerateBasicXKT() {
    await this.page.goto(`${TEST_CONFIG.frontendUrl}/xkt-generator`);

    // 等待组件加载
    await this.page.waitForSelector('input[type="number"]', { timeout: 5000 });

    // 清空并输入数据库号
    const dbnoInput = await this.page.$('input[type="number"]');
    await dbnoInput.click({ clickCount: 3 }); // 全选
    await dbnoInput.type(TEST_DATA.validDbno);

    // 点击生成按钮
    const generateButton = await this.page.waitForSelector(
      'button:has-text("生成XKT文件")',
      { timeout: 5000 }
    );
    await generateButton.click();

    // 等待生成完成（按钮文本变化或Toast提示）
    await this.page.waitForFunction(
      () => {
        const button = document.querySelector('button');
        return button && !button.textContent.includes('正在生成');
      },
      { timeout: 30000 }
    );

    // 检查是否有成功提示
    // 由于toast可能很快消失，我们检查历史记录
    await this.page.click('button:has-text("历史记录")');

    // 等待历史记录更新
    await this.page.waitForFunction(
      () => {
        const content = document.body.textContent;
        return !content.includes('暂无生成记录');
      },
      { timeout: 5000 }
    );
  }

  /**
   * 测试3: 生成带参考号的XKT文件
   */
  async testGenerateWithRefno() {
    await this.page.goto(`${TEST_CONFIG.frontendUrl}/xkt-generator`);

    // 等待组件加载
    await this.page.waitForSelector('input[type="number"]');

    // 输入数据库号
    const dbnoInput = await this.page.$('input[type="number"]');
    await dbnoInput.click({ clickCount: 3 });
    await dbnoInput.type(TEST_DATA.validDbno);

    // 输入参考号
    const refnoInput = await this.page.$('input[placeholder*="17496"]');
    await refnoInput.type(TEST_DATA.validRefno);

    // 生成文件
    await this.page.click('button:has-text("生成XKT文件")');

    // 等待生成完成
    await this.page.waitForFunction(
      () => {
        const button = document.querySelector('button');
        return button && !button.textContent.includes('正在生成');
      },
      { timeout: 30000 }
    );

    // 验证历史记录中包含参考号
    await this.page.click('button:has-text("历史记录")');

    await this.page.waitForFunction(
      () => document.body.textContent.includes(TEST_DATA.validRefno.replace('/', '_')),
      { timeout: 5000 }
    );
  }

  /**
   * 测试4: 压缩选项切换
   */
  async testCompressionToggle() {
    await this.page.goto(`${TEST_CONFIG.frontendUrl}/xkt-generator`);

    // 找到压缩开关
    const switchButton = await this.page.$('[role="switch"]');

    // 获取初始状态
    const initialState = await this.page.$eval(
      '[role="switch"]',
      el => el.getAttribute('data-state')
    );

    // 点击切换
    await switchButton.click();

    // 验证状态改变
    const newState = await this.page.$eval(
      '[role="switch"]',
      el => el.getAttribute('data-state')
    );

    if (initialState === newState) {
      throw new Error('压缩开关状态未改变');
    }
  }

  /**
   * 测试5: 下载功能
   */
  async testDownloadXKT() {
    // 先生成一个文件
    await this.page.goto(`${TEST_CONFIG.frontendUrl}/xkt-generator`);

    const dbnoInput = await this.page.$('input[type="number"]');
    await dbnoInput.click({ clickCount: 3 });
    await dbnoInput.type(TEST_DATA.validDbno);

    const refnoInput = await this.page.$('input[placeholder*="17496"]');
    await refnoInput.type(TEST_DATA.validRefno);

    await this.page.click('button:has-text("生成XKT文件")');

    // 等待生成完成
    await this.page.waitForFunction(
      () => {
        const button = document.querySelector('button');
        return button && !button.textContent.includes('正在生成');
      },
      { timeout: 30000 }
    );

    // 切换到历史记录
    await this.page.click('button:has-text("历史记录")');

    // 设置下载路径
    const downloadPath = path.join(__dirname, 'downloads');
    if (!fs.existsSync(downloadPath)) {
      fs.mkdirSync(downloadPath, { recursive: true });
    }

    // 监听下载
    const client = await this.page.target().createCDPSession();
    await client.send('Page.setDownloadBehavior', {
      behavior: 'allow',
      downloadPath: downloadPath
    });

    // 点击下载按钮
    const downloadButton = await this.page.$('button svg[class*="h-4 w-4"]');
    if (downloadButton) {
      const button = await downloadButton.$('xpath=..');
      await button.click();

      // 等待下载开始
      await this.page.waitForTimeout(2000);

      // 检查下载文件
      const files = fs.readdirSync(downloadPath);
      const xktFile = files.find(f => f.endsWith('.xkt'));

      if (!xktFile) {
        throw new Error('未找到下载的XKT文件');
      }
    }
  }

  /**
   * 测试6: 输入验证
   */
  async testInputValidation() {
    await this.page.goto(`${TEST_CONFIG.frontendUrl}/xkt-generator`);

    // 清空数据库号
    const dbnoInput = await this.page.$('input[type="number"]');
    await dbnoInput.click({ clickCount: 3 });
    await this.page.keyboard.press('Delete');

    // 尝试生成
    await this.page.click('button:has-text("生成XKT文件")');

    // 应该有错误提示（通过toast）
    // 由于toast可能很快消失，我们检查按钮是否仍然可用
    await this.page.waitForTimeout(1000);

    const buttonDisabled = await this.page.$eval(
      'button:has-text("生成XKT文件")',
      btn => btn.disabled
    );

    // 按钮应该不会被禁用（因为验证失败，没有开始生成）
    if (buttonDisabled) {
      throw new Error('验证失败时按钮不应该被禁用');
    }
  }

  /**
   * 测试7: 响应式设计
   */
  async testResponsiveDesign() {
    await this.page.goto(`${TEST_CONFIG.frontendUrl}/xkt-generator`);

    // 测试不同屏幕尺寸
    const viewports = [
      { width: 375, height: 667, name: 'iPhone SE' },
      { width: 768, height: 1024, name: 'iPad' },
      { width: 1920, height: 1080, name: 'Desktop' }
    ];

    for (const viewport of viewports) {
      await this.page.setViewport(viewport);
      await this.page.waitForTimeout(500);

      // 检查主要元素是否可见
      const isVisible = await this.page.$eval(
        'h1',
        el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        }
      );

      if (!isVisible) {
        throw new Error(`在${viewport.name}视图下页面元素不可见`);
      }
    }
  }

  async runAllTests() {
    console.log('=' .repeat(60));
    console.log('🚀 XKT生成器端到端测试');
    console.log('=' .repeat(60));
    console.log(`前端URL: ${TEST_CONFIG.frontendUrl}`);
    console.log(`API URL: ${TEST_CONFIG.apiUrl}`);
    console.log(`无头模式: ${TEST_CONFIG.headless}`);

    await this.setup();

    // 运行所有测试
    await this.test('访问XKT生成器页面', this.testNavigateToXKTGenerator);
    await this.test('生成基本XKT文件', this.testGenerateBasicXKT);
    await this.test('生成带参考号的XKT文件', this.testGenerateWithRefno);
    await this.test('压缩选项切换', this.testCompressionToggle);
    await this.test('下载功能', this.testDownloadXKT);
    await this.test('输入验证', this.testInputValidation);
    await this.test('响应式设计', this.testResponsiveDesign);

    await this.teardown();

    // 输出测试报告
    console.log('\n' + '=' .repeat(60));
    console.log('📊 测试报告');
    console.log('=' .repeat(60));
    console.log(`✅ 通过: ${this.results.passed}`);
    console.log(`❌ 失败: ${this.results.failed}`);
    console.log(`📈 通过率: ${(this.results.passed / (this.results.passed + this.results.failed) * 100).toFixed(1)}%`);

    if (this.results.errors.length > 0) {
      console.log('\n失败的测试:');
      this.results.errors.forEach(error => {
        console.log(`  - ${error.test}: ${error.error}`);
      });
    }

    return this.results.failed === 0;
  }
}

// 主函数
async function main() {
  const runner = new E2ETestRunner();

  try {
    const success = await runner.runAllTests();
    process.exit(success ? 0 : 1);
  } catch (error) {
    console.error('测试运行器错误:', error);
    process.exit(1);
  }
}

// 如果直接运行此文件
if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}

export { E2ETestRunner };