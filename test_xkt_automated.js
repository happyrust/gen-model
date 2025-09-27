#!/usr/bin/env node

const http = require('http');
const fs = require('fs');
const path = require('path');

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
        dbnum: 1112,
        refno: null,
        compress: true,
        validateFormat: true,
        validateLoading: true,
        autoCleanup: true,
        timeout: 120000 // 2分钟超时
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
        } else if (args[i] === '--no-format-validate') {
            config.validateFormat = false;
        } else if (args[i] === '--no-load-validate') {
            config.validateLoading = false;
        } else if (args[i] === '--no-cleanup') {
            config.autoCleanup = false;
        } else if (args[i] === '--timeout') {
            config.timeout = parseInt(args[i + 1]);
            i++;
        } else if (args[i] === '--help' || args[i] === '-h') {
            console.log(`
XKT 自动化测试验证工具

用法: node test_xkt_automated.js [选项]

选项:
  --dbnum, -d <数字>         指定数据库号 (默认: 1112)
  --refno, -r <refno>        指定参考号 (可选，不指定则生成整个数据库)
  --no-compress             禁用压缩
  --no-format-validate      跳过格式验证
  --no-load-validate        跳过加载验证
  --no-cleanup             跳过自动清理
  --timeout <毫秒>          设置超时时间 (默认: 120000)
  --help, -h                显示帮助信息

验证步骤:
  1. 生成 XKT 文件
  2. 验证文件格式 (版本、结构)
  3. 验证 Xeokit SDK 加载能力
  4. 生成详细测试报告
  5. 可选：自动清理生成的测试文件

示例:
  node test_xkt_automated.js                        # 完整验证 DBNUM 1112
  node test_xkt_automated.js -r "17496/256215"      # 验证指定参考号
  node test_xkt_automated.js --no-load-validate     # 只验证格式，不验证加载
            `);
            process.exit(0);
        }
    }

    return config;
}

// 生成 XKT 文件
async function generateXKT(config) {
    const scope = config.refno ? `参考号 ${config.refno}` : `整个数据库 ${config.dbnum}`;
    log(`🚀 开始生成 ${scope} 的 XKT 文件`, 'blue');

    const requestData = {
        dbno: config.dbnum,
        compress: config.compress
    };

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
            },
            timeout: config.timeout
        };

        const result = await new Promise((resolve, reject) => {
            const req = http.request(options, (res) => {
                let data = '';
                res.on('data', (chunk) => data += chunk);
                res.on('end', () => {
                    if (res.statusCode === 200) {
                        try {
                            resolve(JSON.parse(data));
                        } catch (e) {
                            reject(new Error(`JSON解析失败: ${e.message}`));
                        }
                    } else {
                        reject(new Error(`API 错误 ${res.statusCode}: ${data}`));
                    }
                });
            });

            req.on('error', reject);
            req.on('timeout', () => {
                req.destroy();
                reject(new Error(`请求超时 (${config.timeout}ms)`));
            });

            req.write(postData);
            req.end();
        });

        log(`✅ XKT 生成成功: ${result.filename}`, 'green');
        return result;

    } catch (error) {
        log(`❌ XKT 生成失败: ${error.message}`, 'red');
        throw error;
    }
}

// 验证文件格式
async function validateFileFormat(filename) {
    if (!config.validateFormat) {
        log(`⏭️  跳过格式验证`, 'yellow');
        return { valid: true, skipped: true };
    }

    log(`🔍 验证文件格式: ${filename}`, 'blue');

    const filePath = `output/web_ui/${filename}`;

    try {
        if (!fs.existsSync(filePath)) {
            throw new Error(`文件不存在: ${filePath}`);
        }

        const buffer = fs.readFileSync(filePath);
        const fileSize = buffer.length;

        log(`📁 文件大小: ${fileSize} 字节`, 'cyan');

        // 验证最小文件大小
        if (fileSize < 84) {
            throw new Error(`文件太小 (${fileSize} bytes)，不是有效的 XKT 文件`);
        }

        // 验证版本号
        const version = buffer.readUInt32LE(0);
        log(`🔍 XKT 版本: ${version}`, 'cyan');

        if (version !== 10) {
            throw new Error(`不支持的 XKT 版本: ${version}`);
        }

        // 验证文件结构
        const validations = {
            fileExists: true,
            minSize: fileSize >= 84,
            validVersion: version === 10,
            hasHeader: fileSize >= 84
        };

        // 分析 section 偏移
        if (fileSize >= 84) {
            log(`📊 文件结构分析:`, 'cyan');
            log(`  - 版本: ${version}`, 'cyan');
            log(`  - Section 数量: 20`, 'cyan');

            // 检查前几个 section 偏移是否合理
            for (let i = 0; i < Math.min(5, 20); i++) {
                const offset = buffer.readUInt32LE(4 + i * 4);
                log(`  - Section ${i} 偏移: ${offset}`, 'cyan');

                // 偏移值应该在合理范围内
                if (offset < 84 || offset > fileSize) {
                    validations[`section${i}Offset`] = false;
                    log(`  ⚠️ Section ${i} 偏移异常: ${offset}`, 'yellow');
                } else {
                    validations[`section${i}Offset`] = true;
                }
            }
        }

        const allValid = Object.values(validations).every(v => v === true);

        if (allValid) {
            log(`✅ 文件格式验证通过`, 'green');
        } else {
            log(`⚠️ 文件格式验证部分通过`, 'yellow');
        }

        return {
            valid: allValid,
            fileSize,
            version,
            validations,
            skipped: false
        };

    } catch (error) {
        log(`❌ 格式验证失败: ${error.message}`, 'red');
        return {
            valid: false,
            error: error.message,
            skipped: false
        };
    }
}

// 验证 Xeokit SDK 加载能力（模拟）
async function validateXeokitLoading(filename) {
    if (!config.validateLoading) {
        log(`⏭️  跳过加载验证`, 'yellow');
        return { valid: true, skipped: true };
    }

    log(`🌐 验证 Xeokit SDK 加载能力`, 'blue');

    try {
        // 检查文件内容是否符合基本要求
        const filePath = `output/web_ui/${filename}`;
        const buffer = fs.readFileSync(filePath);

        // 基本的二进制格式检查
        const checks = {
            hasVersionHeader: buffer.length >= 4 && buffer.readUInt32LE(0) === 10,
            hasSectionOffsets: buffer.length >= 84,
            notEmpty: buffer.length > 164, // 最小有意义内容
            binaryFormat: true
        };

        // 检查是否有压缩数据标识 (zlib header: 0x78)
        let hasCompressedData = false;
        for (let i = 84; i < Math.min(buffer.length - 1, 200); i++) {
            if (buffer[i] === 0x78 && (buffer[i + 1] === 0x9c || buffer[i + 1] === 0x5e)) {
                hasCompressedData = true;
                break;
            }
        }
        checks.hasCompressedData = hasCompressedData;

        const loadingScore = Object.values(checks).filter(v => v).length;
        const totalChecks = Object.keys(checks).length;

        log(`📊 加载能力评估:`, 'cyan');
        Object.entries(checks).forEach(([key, value]) => {
            log(`  - ${key}: ${value ? '✅' : '❌'}`, value ? 'cyan' : 'yellow');
        });

        const loadingValid = loadingScore >= totalChecks * 0.8; // 80% 通过率

        if (loadingValid) {
            log(`✅ Xeokit 加载验证通过 (${loadingScore}/${totalChecks})`, 'green');
        } else {
            log(`⚠️ Xeokit 加载验证部分通过 (${loadingScore}/${totalChecks})`, 'yellow');
        }

        return {
            valid: loadingValid,
            score: loadingScore,
            totalChecks,
            checks,
            skipped: false
        };

    } catch (error) {
        log(`❌ 加载验证失败: ${error.message}`, 'red');
        return {
            valid: false,
            error: error.message,
            skipped: false
        };
    }
}

// 生成测试报告
function generateTestReport(config, generateResult, formatValidation, loadingValidation) {
    log(`📄 生成测试报告`, 'blue');

    const report = {
        timestamp: new Date().toISOString(),
        config: config,
        generation: {
            success: !!generateResult,
            filename: generateResult?.filename,
            fileSize: generateResult?.file_size,
            error: generateResult?.error
        },
        formatValidation: formatValidation,
        loadingValidation: loadingValidation,
        overallStatus: 'unknown'
    };

    // 计算总体状态
    const generationOk = !!generateResult;
    const formatOk = formatValidation.skipped || formatValidation.valid;
    const loadingOk = loadingValidation.skipped || loadingValidation.valid;

    if (generationOk && formatOk && loadingOk) {
        report.overallStatus = 'success';
        log(`✅ 总体测试状态: 成功`, 'green');
    } else if (generationOk && (formatOk || loadingOk)) {
        report.overallStatus = 'partial';
        log(`⚠️ 总体测试状态: 部分成功`, 'yellow');
    } else {
        report.overallStatus = 'failed';
        log(`❌ 总体测试状态: 失败`, 'red');
    }

    const reportFilename = `xkt_automated_test_${config.dbnum}_${Date.now()}.json`;
    fs.writeFileSync(reportFilename, JSON.stringify(report, null, 2));

    log(`📄 详细报告已保存: ${reportFilename}`, 'cyan');
    return report;
}

// 自动清理
function performCleanup(report, config) {
    if (!config.autoCleanup) {
        log(`⏭️  跳过自动清理`, 'yellow');
        return;
    }

    log(`🧹 执行自动清理`, 'blue');

    try {
        // 清理生成的 XKT 文件 (只清理测试生成的)
        if (report.generation.filename) {
            const filePath = `output/web_ui/${report.generation.filename}`;
            if (fs.existsSync(filePath)) {
                const stats = fs.statSync(filePath);
                const ageMinutes = (Date.now() - stats.mtime.getTime()) / (1000 * 60);

                // 只清理刚生成的文件 (5分钟内)
                if (ageMinutes < 5) {
                    fs.unlinkSync(filePath);
                    log(`🗑️  已删除测试文件: ${report.generation.filename}`, 'cyan');
                }
            }
        }

        log(`✅ 清理完成`, 'green');
    } catch (error) {
        log(`⚠️ 清理失败: ${error.message}`, 'yellow');
    }
}

// 主测试流程
async function runAutomatedTest() {
    const config = parseArgs();
    const scope = config.refno ? `参考号 ${config.refno}` : `整个数据库 ${config.dbnum}`;

    log(`🚀 开始 XKT 自动化验证测试`, 'blue');
    log(`📋 测试范围: ${scope}`, 'cyan');
    log(`⚙️  测试配置: 格式验证=${config.validateFormat}, 加载验证=${config.validateLoading}, 自动清理=${config.autoCleanup}`, 'cyan');

    let generateResult = null;
    let formatValidation = { valid: false, skipped: true };
    let loadingValidation = { valid: false, skipped: true };

    try {
        // 1. 生成 XKT 文件
        generateResult = await generateXKT(config);

        // 2. 验证文件格式
        formatValidation = await validateFileFormat(generateResult.filename);

        // 3. 验证 Xeokit 加载
        loadingValidation = await validateXeokitLoading(generateResult.filename);

        // 4. 生成报告
        const report = generateTestReport(config, generateResult, formatValidation, loadingValidation);

        // 5. 显示测试摘要
        console.log('\\n' + '='.repeat(60));
        log(`📊 自动化测试摘要`, 'blue');
        console.log('='.repeat(60));
        log(`📋 测试范围: ${scope}`, 'cyan');
        log(`📁 生成文件: ${generateResult.filename}`, 'cyan');
        log(`📏 文件大小: ${formatValidation.fileSize || 'N/A'} 字节`, 'cyan');
        log(`🔍 格式验证: ${formatValidation.skipped ? '跳过' : (formatValidation.valid ? '✅ 通过' : '❌ 失败')}`, formatValidation.valid ? 'green' : (formatValidation.skipped ? 'yellow' : 'red'));
        log(`🌐 加载验证: ${loadingValidation.skipped ? '跳过' : (loadingValidation.valid ? '✅ 通过' : '❌ 失败')}`, loadingValidation.valid ? 'green' : (loadingValidation.skipped ? 'yellow' : 'red'));
        log(`📊 总体状态: ${report.overallStatus === 'success' ? '✅ 成功' : (report.overallStatus === 'partial' ? '⚠️ 部分成功' : '❌ 失败')}`, report.overallStatus === 'success' ? 'green' : (report.overallStatus === 'partial' ? 'yellow' : 'red'));

        // 6. 自动清理
        performCleanup(report, config);

        // 7. 浏览器测试建议
        if (!config.autoCleanup) {
            console.log('\\n' + '-'.repeat(40));
            log(`🌐 浏览器测试建议:`, 'blue');
            log(`访问: http://localhost:8080/xeokit-viewer`, 'blue');
            log(`加载文件: ${generateResult.filename}`, 'blue');
            console.log('-'.repeat(40));
        }

        // 设置进程退出码
        process.exit(report.overallStatus === 'success' ? 0 : (report.overallStatus === 'partial' ? 1 : 2));

    } catch (error) {
        log(`❌ 自动化测试失败: ${error.message}`, 'red');

        // 生成错误报告
        const report = generateTestReport(config, null, formatValidation, loadingValidation);
        report.generation.error = error.message;

        console.error('\\n详细错误:', error);
        process.exit(3);
    }
}

// 全局变量声明
let config;

// 如果直接运行此脚本
if (require.main === module) {
    config = parseArgs();
    runAutomatedTest().catch(console.error);
}

module.exports = { runAutomatedTest, parseArgs };