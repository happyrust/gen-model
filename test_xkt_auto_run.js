#!/usr/bin/env node

import http from 'http';
import fs from 'fs';

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

async function makeRequest(options, postData = null) {
    return new Promise((resolve, reject) => {
        const req = http.request(options, (res) => {
            let data = '';
            res.on('data', (chunk) => data += chunk);
            res.on('end', () => {
                if (res.statusCode === 200) {
                    try {
                        resolve(JSON.parse(data));
                    } catch (e) {
                        resolve(data);
                    }
                } else {
                    reject(new Error(`API 错误 ${res.statusCode}: ${data}`));
                }
            });
        });
        req.on('error', reject);
        if (postData) {
            req.write(postData);
        }
        req.end();
    });
}

async function runXKTTest(dbnum = 1112, refno = "17496/266203", compress = true) {
    log(`🚀 开始测试 DBNUM ${dbnum}, REFNO ${refno}`, 'blue');

    const testResults = {
        timestamp: new Date().toISOString(),
        dbnum,
        refno,
        compress,
        tests: {},
        errors: []
    };

    try {
        // Step 1: 生成 XKT 文件
        log('📦 生成 XKT 文件...', 'cyan');

        const postData = JSON.stringify({
            dbno: dbnum,
            refno: refno,
            compress: compress
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

        const generateResult = await makeRequest(options, postData);
        log(`✅ XKT 生成成功: ${generateResult.filename}`, 'green');

        testResults.filename = generateResult.filename;
        testResults.tests.generation = true;

        // Step 2: 验证文件存在
        const filePath = `output/web_ui/${generateResult.filename}`;

        if (!fs.existsSync(filePath)) {
            throw new Error(`文件不存在: ${filePath}`);
        }

        testResults.tests.fileExists = true;

        // Step 3: 验证文件格式
        const fileBuffer = fs.readFileSync(filePath);
        const fileSize = fileBuffer.length;

        log(`📁 文件大小: ${fileSize} 字节`, 'cyan');
        testResults.fileSize = fileSize;

        if (fileSize < 4) {
            throw new Error('文件太小，不是有效的 XKT 文件');
        }

        const version = fileBuffer.readUInt32LE(0);
        log(`🔍 XKT 版本: ${version}`, 'cyan');
        testResults.version = version;

        if (version >= 1 && version <= 11) {
            log(`✅ XKT 格式验证通过`, 'green');
            testResults.tests.formatValidation = true;
        } else {
            throw new Error(`不支持的 XKT 版本: ${version}`);
        }

        // Step 4: 分析文件结构
        if (fileSize >= 84) {
            log(`📊 分析文件结构:`, 'cyan');
            const sections = [];
            for (let i = 0; i < Math.min(5, 20); i++) {
                const offset = fileBuffer.readUInt32LE(4 + i * 4);
                sections.push({ index: i, offset });
                log(`  - Section ${i} 偏移: ${offset}`, 'cyan');
            }
            testResults.sections = sections;
            testResults.tests.structureAnalysis = true;
        }

        testResults.success = true;

    } catch (error) {
        log(`❌ 测试失败: ${error.message}`, 'red');
        testResults.success = false;
        testResults.errors.push(error.message);
    }

    return testResults;
}

async function runBatchTests() {
    log('🚀 开始批量自动化测试', 'blue');
    console.log('='.repeat(60));

    const allResults = [];
    const testCases = [
        { dbnum: 1112, refno: "17496/266203", compress: true },
        { dbnum: 1112, refno: "17496/266203", compress: false },
        { dbnum: 1112, refno: "17497/256215", compress: true },
    ];

    for (const testCase of testCases) {
        log(`\n🔄 测试用例 ${testCases.indexOf(testCase) + 1}/${testCases.length}`, 'yellow');
        const result = await runXKTTest(testCase.dbnum, testCase.refno, testCase.compress);
        allResults.push(result);

        // 短暂延迟避免服务器过载
        await new Promise(resolve => setTimeout(resolve, 1000));
    }

    // 生成总结报告
    const summary = {
        timestamp: new Date().toISOString(),
        totalTests: allResults.length,
        successful: allResults.filter(r => r.success).length,
        failed: allResults.filter(r => !r.success).length,
        results: allResults
    };

    // 保存报告
    const reportFile = `xkt_auto_test_report_${Date.now()}.json`;
    fs.writeFileSync(reportFile, JSON.stringify(summary, null, 2));

    console.log('\n' + '='.repeat(60));
    log('📊 测试总结', 'blue');
    console.log('='.repeat(60));
    log(`✅ 成功: ${summary.successful}/${summary.totalTests}`, 'green');
    log(`❌ 失败: ${summary.failed}/${summary.totalTests}`, summary.failed > 0 ? 'red' : 'green');
    log(`📄 详细报告: ${reportFile}`, 'cyan');

    // 显示每个测试的状态
    console.log('\n📋 测试详情:');
    allResults.forEach((result, index) => {
        const status = result.success ? '✅' : '❌';
        const color = result.success ? 'green' : 'red';
        log(`${status} 测试 ${index + 1}: DBNUM=${result.dbnum}, REFNO=${result.refno}, 压缩=${result.compress}`, color);
        if (result.filename) {
            log(`   文件: ${result.filename} (${result.fileSize} 字节)`, 'cyan');
        }
        if (!result.success && result.errors.length > 0) {
            log(`   错误: ${result.errors[0]}`, 'red');
        }
    });

    return summary;
}

// 主程序
async function main() {
    try {
        await runBatchTests();
        process.exit(0);
    } catch (error) {
        console.error('\n❌ 自动化测试异常:', error);
        process.exit(1);
    }
}

main();