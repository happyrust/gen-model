#!/usr/bin/env node

const http = require('http');
const fs = require('fs');

const colors = {
    reset: '\x1b[0m',
    red: '\x1b[31m',
    green: '\x1b[32m',
    blue: '\x1b[34m',
    cyan: '\x1b[36m',
};

function log(message, color = 'reset') {
    const timestamp = new Date().toISOString();
    console.log(`${colors[color]}[${timestamp}] ${message}${colors.reset}`);
}

async function testXKT1112() {
    log('🚀 开始测试 DBNUM 1112 XKT 生成', 'blue');

    // Step 1: 生成 XKT 文件
    try {
        const postData = JSON.stringify({
            dbno: 1112,
            // 移除 refno，让系统生成整个 DBNUM 1112 的完整数据
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

        const generateResult = await new Promise((resolve, reject) => {
            const req = http.request(options, (res) => {
                let data = '';
                res.on('data', (chunk) => data += chunk);
                res.on('end', () => {
                    if (res.statusCode === 200) {
                        resolve(JSON.parse(data));
                    } else {
                        reject(new Error(`API 错误 ${res.statusCode}: ${data}`));
                    }
                });
            });
            req.on('error', reject);
            req.write(postData);
            req.end();
        });

        log(`✅ XKT 生成成功: ${generateResult.filename}`, 'green');

        // Step 2: 直接读取本地文件验证
        const filePath = `output/web_ui/${generateResult.filename}`;

        if (!fs.existsSync(filePath)) {
            throw new Error(`文件不存在: ${filePath}`);
        }

        const fileBuffer = fs.readFileSync(filePath);
        const fileSize = fileBuffer.length;

        log(`📁 文件大小: ${fileSize} 字节`, 'cyan');

        // 验证 XKT 格式
        if (fileSize < 4) {
            throw new Error('文件太小，不是有效的 XKT 文件');
        }

        const version = fileBuffer.readUInt32LE(0);
        log(`🔍 XKT 版本: ${version}`, 'cyan');

        if (version >= 1 && version <= 10) {
            log(`✅ XKT 格式验证通过`, 'green');
        } else {
            throw new Error(`不支持的 XKT 版本: ${version}`);
        }

        // Step 3: 分析文件结构
        if (fileSize >= 84) { // 至少有版本号 + 20个section偏移
            const sectionCount = 20;
            log(`📊 分析文件结构:`, 'cyan');
            log(`  - 版本: ${version}`, 'cyan');
            log(`  - Section 数量: ${sectionCount}`, 'cyan');

            // 读取前几个 section 偏移
            for (let i = 0; i < Math.min(5, sectionCount); i++) {
                const offset = fileBuffer.readUInt32LE(4 + i * 4);
                log(`  - Section ${i} 偏移: ${offset}`, 'cyan');
            }
        }

        // Step 4: 创建测试报告
        const report = {
            timestamp: new Date().toISOString(),
            dbnum: 1112,
            refno: "17496/266203",
            filename: generateResult.filename,
            fileSize: fileSize,
            version: version,
            success: true,
            tests: {
                generation: true,
                fileExists: true,
                formatValidation: true,
                versionCheck: version >= 1 && version <= 10
            }
        };

        const reportFile = `xkt_test_report_1112_${Date.now()}.json`;
        fs.writeFileSync(reportFile, JSON.stringify(report, null, 2));

        console.log('\n' + '='.repeat(60));
        log('✅ 测试完成！', 'green');
        console.log('='.repeat(60));
        log(`📊 测试摘要:`, 'cyan');
        log(`  - DBNUM: 1112`, 'cyan');
        log(`  - REFNO: 17496/266203`, 'cyan');
        log(`  - 文件: ${generateResult.filename}`, 'cyan');
        log(`  - 大小: ${fileSize} 字节`, 'cyan');
        log(`  - 版本: ${version}`, 'cyan');
        log(`  - 报告: ${reportFile}`, 'cyan');

        // Step 5: Xeokit 加载测试提示
        console.log('\n' + '-'.repeat(40));
        log('🌐 浏览器测试建议:', 'blue');
        log(`访问: http://localhost:8080/xeokit-viewer`, 'blue');
        log(`加载文件: ${generateResult.filename}`, 'blue');
        console.log('-'.repeat(40));

    } catch (error) {
        console.log('\n' + '='.repeat(60));
        log('❌ 测试失败！', 'red');
        console.log('='.repeat(60));
        log(`错误: ${error.message}`, 'red');
        process.exit(1);
    }
}

// 运行测试
testXKT1112();