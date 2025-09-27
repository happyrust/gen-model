#!/usr/bin/env node

/**
 * 完整的 XKT 浏览器加载测试
 *
 * 安装依赖:
 * npm install puppeteer
 *
 * 使用:
 * node test_xkt_browser.js
 */

const http = require('http');

const colors = {
    reset: '\x1b[0m',
    red: '\x1b[31m',
    green: '\x1b[32m',
    blue: '\x1b[34m',
    cyan: '\x1b[36m',
    yellow: '\x1b[33m',
};

function log(message, color = 'reset') {
    const timestamp = new Date().toISOString();
    console.log(`${colors[color]}[${timestamp}] ${message}${colors.reset}`);
}

// 检查 puppeteer 是否安装
function checkPuppeteer() {
    try {
        require('puppeteer');
        return true;
    } catch (e) {
        return false;
    }
}

// 生成 XKT 文件
async function generateXKT() {
    log('正在生成 XKT 文件...', 'blue');

    const postData = JSON.stringify({
        dbno: 1112,
        refno: "17496/266203",
        compress: true
    });

    const options = {
        hostname: 'localhost',
        port: 8080,
        path: '/api/xkt/generate',
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Content-Length': Buffer.byteLength(postData)
        }
    };

    return new Promise((resolve, reject) => {
        const req = http.request(options, (res) => {
            let data = '';
            res.on('data', (chunk) => data += chunk);
            res.on('end', () => {
                if (res.statusCode === 200) {
                    const result = JSON.parse(data);
                    log(`✅ 生成成功: ${result.filename}`, 'green');
                    resolve(result.filename);
                } else {
                    reject(new Error(`生成失败 ${res.statusCode}: ${data}`));
                }
            });
        });
        req.on('error', reject);
        req.write(postData);
        req.end();
    });
}

// 使用 Puppeteer 测试浏览器加载
async function testBrowserLoading(filename) {
    const puppeteer = require('puppeteer');

    log('启动浏览器进行 Xeokit 加载测试...', 'blue');

    const browser = await puppeteer.launch({
        headless: true,
        args: ['--no-sandbox', '--disable-setuid-sandbox']
    });

    try {
        const page = await browser.newPage();

        // 监听控制台消息
        const consoleMessages = [];
        page.on('console', msg => {
            consoleMessages.push({
                type: msg.type(),
                text: msg.text(),
                timestamp: new Date().toISOString()
            });
        });

        // 监听页面错误
        const pageErrors = [];
        page.on('pageerror', error => {
            pageErrors.push({
                message: error.message,
                stack: error.stack,
                timestamp: new Date().toISOString()
            });
        });

        // 创建测试页面
        const testHTML = `
<!DOCTYPE html>
<html>
<head>
    <title>XKT 加载测试</title>
    <script src="https://cdn.jsdelivr.net/npm/xeokit-sdk@2.6.8/dist/xeokit-sdk.es.js"></script>
</head>
<body>
    <canvas id="myCanvas" width="800" height="600"></canvas>
    <div id="status">初始化中...</div>
    <script>
        window.testResult = { success: false, error: null, info: {} };

        try {
            // 初始化 viewer
            const viewer = new xeokit.Viewer({
                canvasId: "myCanvas"
            });

            // XKT 加载器
            const xktLoader = new xeokit.XKTLoaderPlugin(viewer);

            const startTime = Date.now();

            const model = xktLoader.load({
                id: "testModel",
                src: "http://localhost:8080/output/web_ui/${filename}",
                edges: true,
                performance: true
            });

            model.on("loaded", () => {
                const loadTime = Date.now() - startTime;
                const entities = Object.keys(model.entities);
                const entityCount = entities.length;
                const triangles = model.numTriangles || 0;

                window.testResult = {
                    success: true,
                    info: {
                        loadTime: loadTime,
                        entityCount: entityCount,
                        triangleCount: triangles,
                        modelId: model.id,
                        entities: entities.slice(0, 5) // 只记录前5个实体
                    }
                };

                document.getElementById('status').innerHTML =
                    \`✅ 加载成功！实体: \${entityCount}, 三角形: \${triangles}, 耗时: \${loadTime}ms\`;

                console.log("XKT_LOAD_SUCCESS", window.testResult);
            });

            model.on("error", (error) => {
                window.testResult = {
                    success: false,
                    error: error.message || "未知加载错误"
                };

                document.getElementById('status').innerHTML =
                    \`❌ 加载失败: \${error.message}\`;

                console.error("XKT_LOAD_ERROR", error);
            });

            // 超时处理
            setTimeout(() => {
                if (!window.testResult.success && !window.testResult.error) {
                    window.testResult = {
                        success: false,
                        error: "加载超时 (10秒)"
                    };
                    console.error("XKT_LOAD_TIMEOUT");
                }
            }, 10000);

        } catch (error) {
            window.testResult = {
                success: false,
                error: error.message
            };
            console.error("XKT_INIT_ERROR", error);
        }
    </script>
</body>
</html>`;

        await page.setContent(testHTML);

        // 等待加载完成或超时
        await page.waitForTimeout(12000);

        // 获取测试结果
        const testResult = await page.evaluate(() => window.testResult);

        log(`浏览器测试完成`, 'cyan');

        // 分析控制台消息
        const errors = consoleMessages.filter(msg =>
            msg.type === 'error' || msg.text.includes('XKT_LOAD_ERROR'));
        const successes = consoleMessages.filter(msg =>
            msg.text.includes('XKT_LOAD_SUCCESS'));

        if (testResult.success) {
            log(`✅ Xeokit 加载成功!`, 'green');
            log(`  - 加载时间: ${testResult.info.loadTime}ms`, 'cyan');
            log(`  - 实体数量: ${testResult.info.entityCount}`, 'cyan');
            log(`  - 三角形数量: ${testResult.info.triangleCount}`, 'cyan');
        } else {
            log(`❌ Xeokit 加载失败: ${testResult.error}`, 'red');
        }

        return {
            success: testResult.success,
            result: testResult,
            consoleMessages: consoleMessages,
            pageErrors: pageErrors
        };

    } finally {
        await browser.close();
    }
}

// 主测试函数
async function runBrowserTest() {
    console.log('\n' + '='.repeat(60));
    log('🌐 XKT 浏览器加载测试', 'blue');
    console.log('='.repeat(60) + '\n');

    try {
        if (!checkPuppeteer()) {
            log('❌ 未安装 puppeteer, 运行: npm install puppeteer', 'red');
            log('使用简化测试模式...', 'yellow');

            const filename = await generateXKT();
            log(`生成的文件: ${filename}`, 'cyan');
            log(`手动测试: http://localhost:8080/xeokit-viewer`, 'blue');
            return;
        }

        // 生成文件
        const filename = await generateXKT();

        // 浏览器测试
        const browserResult = await testBrowserLoading(filename);

        // 生成完整报告
        const report = {
            timestamp: new Date().toISOString(),
            filename: filename,
            browserTest: browserResult,
            summary: {
                fileGeneration: true,
                browserLoading: browserResult.success,
                overall: browserResult.success
            }
        };

        // 保存报告
        const fs = require('fs');
        const reportFile = `browser_test_report_${Date.now()}.json`;
        fs.writeFileSync(reportFile, JSON.stringify(report, null, 2));

        console.log('\n' + '='.repeat(60));
        if (browserResult.success) {
            log('✅ 完整测试通过！', 'green');
        } else {
            log('❌ 浏览器加载测试失败', 'red');
        }
        console.log('='.repeat(60));
        log(`📊 详细报告已保存: ${reportFile}`, 'cyan');

    } catch (error) {
        log(`❌ 测试失败: ${error.message}`, 'red');
        process.exit(1);
    }
}

// 运行测试
runBrowserTest();