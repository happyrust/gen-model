#!/usr/bin/env node

import puppeteer from 'puppeteer';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

class XKTTestRunner {
    constructor() {
        this.browser = null;
        this.page = null;
        this.testResults = [];
    }

    async initialize() {
        console.log('🚀 启动XKT自动化测试...');

        try {
            this.browser = await puppeteer.launch({
                headless: "new", // 使用新的headless模式
                args: [
                    '--no-sandbox',
                    '--disable-setuid-sandbox',
                    '--disable-dev-shm-usage',
                    '--disable-web-security',
                    '--allow-running-insecure-content'
                ]
            });
            console.log('✅ Puppeteer浏览器启动成功');
        } catch (error) {
            console.log('❌ Puppeteer浏览器启动失败:', error.message);
            throw error;
        }

        this.page = await this.browser.newPage();

        // 设置控制台监听
        this.page.on('console', (msg) => {
            const text = msg.text();
            if (text.includes('RangeError') || text.includes('Error') || text.includes('Failed')) {
                console.log('❌ 浏览器错误:', text);
                this.testResults.push({
                    type: 'error',
                    message: text,
                    timestamp: new Date().toISOString()
                });
            } else if (text.includes('✅') || text.includes('成功')) {
                console.log('✅ 浏览器成功:', text);
                this.testResults.push({
                    type: 'success',
                    message: text,
                    timestamp: new Date().toISOString()
                });
            }
        });

        // 监听页面错误
        this.page.on('pageerror', (error) => {
            console.log('❌ 页面错误:', error.message);
            this.testResults.push({
                type: 'page_error',
                message: error.message,
                timestamp: new Date().toISOString()
            });
        });
    }

    async testXKTLoading() {
        console.log('📄 加载测试页面...');

        // 加载本地测试页面
        const testPageUrl = 'http://localhost:8082/xkt_v10_test.html';

        try {
            await this.page.goto(testPageUrl, {
                waitUntil: 'networkidle0',
                timeout: 30000
            });

            console.log('✅ 页面加载成功');

            // 等待页面初始化
            await this.page.waitForTimeout(2000);

            // 检查是否有初始化错误
            const statusText = await this.page.$eval('#status', el => el.textContent);
            console.log('📊 初始状态:', statusText);

            // 点击测试按钮
            console.log('🔘 点击测试按钮...');
            await this.page.click('button[onclick="testXKTv10()"]');

            // 等待加载完成或失败
            let attempts = 0;
            const maxAttempts = 20; // 20秒超时
            let loadResult = null;

            while (attempts < maxAttempts) {
                await this.page.waitForTimeout(1000);
                attempts++;

                const currentStatus = await this.page.$eval('#status', el => el.textContent);

                if (currentStatus.includes('✅') && currentStatus.includes('加载成功')) {
                    loadResult = 'success';
                    console.log('🎉 XKT文件加载成功！');
                    break;
                } else if (currentStatus.includes('❌') || currentStatus.includes('失败')) {
                    loadResult = 'failed';
                    console.log('💥 XKT文件加载失败:', currentStatus);
                    break;
                }

                if (attempts % 5 === 0) {
                    console.log(`⏳ 等待中... (${attempts}/${maxAttempts})`);
                }
            }

            if (!loadResult) {
                console.log('⚠️ 测试超时');
                loadResult = 'timeout';
            }

            // 获取最终状态
            const finalStatus = await this.page.$eval('#status', el => el.innerHTML);
            console.log('📋 最终状态:', finalStatus);

            // 获取页面中的任何错误
            const errors = await this.page.evaluate(() => {
                const errorElements = document.querySelectorAll('.error');
                return Array.from(errorElements).map(el => el.textContent);
            });

            if (errors.length > 0) {
                console.log('❌ 页面中发现的错误:');
                errors.forEach(error => console.log('  -', error));
            }

            return {
                result: loadResult,
                status: finalStatus,
                errors: errors,
                testResults: this.testResults
            };

        } catch (error) {
            console.log('❌ 测试页面加载失败:', error.message);
            return {
                result: 'page_load_failed',
                error: error.message,
                testResults: this.testResults
            };
        }
    }

    async checkServerStatus() {
        console.log('🌐 检查HTTP服务器状态...');

        try {
            const response = await this.page.goto('http://localhost:8082/', {
                waitUntil: 'domcontentloaded',
                timeout: 5000
            });

            if (response.ok()) {
                console.log('✅ HTTP服务器运行正常');
                return true;
            } else {
                console.log('❌ HTTP服务器响应异常:', response.status());
                return false;
            }
        } catch (error) {
            console.log('❌ 无法连接到HTTP服务器:', error.message);
            console.log('💡 请确保运行: python3 -m http.server 8082');
            return false;
        }
    }

    async testFileAccess() {
        console.log('📁 测试文件访问...');

        const filesToTest = [
            'xkt_v10_test.html',
            '../output/cube_v10_fixed.xkt',
            '../js/xeokit/xeokit-sdk.es.js'
        ];

        const results = {};

        for (const file of filesToTest) {
            try {
                const response = await this.page.goto(`http://localhost:8082/${file}`, {
                    timeout: 5000
                });

                if (response.ok()) {
                    const size = response.headers()['content-length'] || 'unknown';
                    console.log(`✅ ${file} - ${size} bytes`);
                    results[file] = { status: 'ok', size };
                } else {
                    console.log(`❌ ${file} - ${response.status()}`);
                    results[file] = { status: response.status() };
                }
            } catch (error) {
                console.log(`❌ ${file} - ${error.message}`);
                results[file] = { status: 'error', error: error.message };
            }
        }

        return results;
    }

    async generateReport(testResult) {
        console.log('\n' + '='.repeat(60));
        console.log('📊 XKT测试报告');
        console.log('='.repeat(60));

        const timestamp = new Date().toLocaleString('zh-CN');
        console.log(`⏰ 测试时间: ${timestamp}`);

        if (testResult.result === 'success') {
            console.log('🎉 结果: 测试通过 - XKT文件加载成功！');
        } else if (testResult.result === 'failed') {
            console.log('💥 结果: 测试失败 - XKT文件加载失败');
        } else if (testResult.result === 'timeout') {
            console.log('⚠️ 结果: 测试超时');
        } else {
            console.log('❌ 结果: 页面加载失败');
        }

        if (testResult.errors && testResult.errors.length > 0) {
            console.log('\n❌ 发现的错误:');
            testResult.errors.forEach((error, index) => {
                console.log(`  ${index + 1}. ${error}`);
            });
        }

        if (testResult.testResults && testResult.testResults.length > 0) {
            console.log('\n📝 详细日志:');
            testResult.testResults.forEach((result, index) => {
                const icon = result.type === 'success' ? '✅' : '❌';
                console.log(`  ${icon} ${result.message}`);
            });
        }

        console.log('='.repeat(60));
    }

    async cleanup() {
        if (this.browser) {
            await this.browser.close();
        }
    }

    async run() {
        try {
            await this.initialize();

            // 1. 检查服务器状态
            const serverOk = await this.checkServerStatus();
            if (!serverOk) {
                process.exit(1);
            }

            // 2. 测试文件访问
            const fileResults = await this.testFileAccess();

            // 3. 测试XKT加载
            const testResult = await this.testXKTLoading();

            // 4. 生成报告
            await this.generateReport(testResult);

            // 5. 退出码
            if (testResult.result === 'success') {
                process.exit(0);
            } else {
                process.exit(1);
            }

        } catch (error) {
            console.log('💥 测试运行失败:', error.message);
            process.exit(1);
        } finally {
            await this.cleanup();
        }
    }
}

// 运行测试
const runner = new XKTTestRunner();
runner.run().catch(console.error);