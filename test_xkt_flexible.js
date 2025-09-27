#!/usr/bin/env node

const http = require('http');
const fs = require('fs');

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

// 解析命令行参数
function parseArgs() {
    const args = process.argv.slice(2);
    const config = {
        dbnum: 1112,  // 默认数据库号
        refno: null,  // 默认为 null，生成整个数据库
        compress: true
    };

    for (let i = 0; i < args.length; i++) {
        if (args[i] === '--dbnum' || args[i] === '-d') {
            config.dbnum = parseInt(args[i + 1]);
            i++;
        } else if (args[i] === '--refno' || args[i] === '-r') {
            config.refno = args[i + 1];
            i++;
        } else if (args[i] === '--no-compress') {
            config.compress = false;
        } else if (args[i] === '--help' || args[i] === '-h') {
            console.log(`
用法: node test_xkt_flexible.js [选项]

选项:
  --dbnum, -d <数字>    指定数据库号 (默认: 1112)
  --refno, -r <refno>   指定参考号 (可选，不指定则生成整个数据库)
  --no-compress         禁用压缩
  --help, -h            显示帮助信息

示例:
  node test_xkt_flexible.js                        # 生成整个 DBNUM 1112
  node test_xkt_flexible.js -r "17496/256215"      # 生成指定参考号
  node test_xkt_flexible.js -d 1113 -r "12345/67890"  # 指定数据库和参考号
            `);
            process.exit(0);
        }
    }

    return config;
}

async function testXKTGeneration(config) {
    const scope = config.refno ? `参考号 ${config.refno}` : `整个数据库 ${config.dbnum}`;
    log(`🚀 开始测试 ${scope} 的 XKT 生成`, 'blue');

    // Step 1: 准备请求数据
    const requestData = {
        dbno: config.dbnum,
        compress: config.compress
    };

    // 只有当指定了 refno 时才添加到请求中
    if (config.refno) {
        requestData.refno = config.refno;
    }

    try {
        const postData = JSON.stringify(requestData);
        log(`📋 请求参数: ${JSON.stringify(requestData, null, 2)}`, 'cyan');

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
        log(`📁 文件大小: ${generateResult.file_size} 字节`, 'cyan');

        // Step 2: 验证生成的文件
        const filePath = `output/web_ui/${generateResult.filename}`;
        if (fs.existsSync(filePath)) {
            const buffer = fs.readFileSync(filePath);

            // 验证 XKT 版本
            if (buffer.length >= 4) {
                const version = buffer.readUInt32LE(0);
                log(`🔍 XKT 版本: ${version}`, 'cyan');

                if (version === 10) {
                    log(`✅ XKT 格式验证通过`, 'green');
                } else {
                    log(`⚠️ XKT 版本异常: ${version}`, 'yellow');
                }
            }

            // 分析文件结构（简单版本）
            if (buffer.length >= 84) { // 头部 + 20个section偏移
                log(`📊 分析文件结构:`, 'cyan');
                log(`  - 版本: ${buffer.readUInt32LE(0)}`, 'cyan');
                log(`  - Section 数量: 20`, 'cyan');

                // 显示前几个 section 的偏移
                for (let i = 0; i < Math.min(5, 20); i++) {
                    const offset = buffer.readUInt32LE(4 + i * 4);
                    log(`  - Section ${i} 偏移: ${offset}`, 'cyan');
                }
            }

            // 生成测试报告
            const reportData = {
                timestamp: new Date().toISOString(),
                dbnum: config.dbnum,
                refno: config.refno,
                filename: generateResult.filename,
                fileSize: generateResult.file_size,
                version: buffer.length >= 4 ? buffer.readUInt32LE(0) : null,
                success: true,
                tests: {
                    generation: true,
                    fileExists: true,
                    formatValidation: buffer.length >= 4 && buffer.readUInt32LE(0) === 10,
                    versionCheck: buffer.length >= 4 && buffer.readUInt32LE(0) === 10
                }
            };

            const reportFilename = `xkt_test_report_${config.dbnum}_${Date.now()}.json`;
            fs.writeFileSync(reportFilename, JSON.stringify(reportData, null, 2));
            log(`📄 测试报告已保存: ${reportFilename}`, 'cyan');

        } else {
            throw new Error(`生成的文件不存在: ${filePath}`);
        }

        // 显示测试结果摘要
        console.log('\n' + '='.repeat(60));
        log(`✅ 测试完成！`, 'green');
        console.log('='.repeat(60));
        log(`📊 测试摘要:`, 'cyan');
        log(`  - DBNUM: ${config.dbnum}${config.refno ? '' : ' (完整数据库)'}`, 'cyan');
        if (config.refno) {
            log(`  - REFNO: ${config.refno}`, 'cyan');
        }
        log(`  - 文件: ${generateResult.filename}`, 'cyan');
        log(`  - 大小: ${generateResult.file_size} 字节`, 'cyan');
        if (fs.existsSync(`output/web_ui/${generateResult.filename}`)) {
            const buffer = fs.readFileSync(`output/web_ui/${generateResult.filename}`);
            if (buffer.length >= 4) {
                log(`  - 版本: ${buffer.readUInt32LE(0)}`, 'cyan');
            }
        }

        console.log('\n' + '-'.repeat(40));
        log(`🌐 浏览器测试建议:`, 'blue');
        log(`访问: http://localhost:8080/xeokit-viewer`, 'blue');
        log(`加载文件: ${generateResult.filename}`, 'blue');
        console.log('-'.repeat(40));

    } catch (error) {
        log(`❌ 测试失败: ${error.message}`, 'red');
        console.error(error);
        process.exit(1);
    }
}

// 主函数
async function main() {
    const config = parseArgs();
    await testXKTGeneration(config);
}

// 如果直接运行此脚本
if (require.main === module) {
    main().catch(console.error);
}

module.exports = { testXKTGeneration, parseArgs };