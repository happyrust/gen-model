#!/usr/bin/env node

/**
 * 真实房间计算API测试脚本
 * 测试使用aios-core真实查询方法的房间计算功能
 */

const axios = require('axios');

// 配置
const BASE_URL = 'http://localhost:8080';
const API_BASE = `${BASE_URL}/api/room`;

// 颜色输出
const colors = {
    reset: '\x1b[0m',
    bright: '\x1b[1m',
    red: '\x1b[31m',
    green: '\x1b[32m',
    yellow: '\x1b[33m',
    blue: '\x1b[34m',
    magenta: '\x1b[35m',
    cyan: '\x1b[36m'
};

function log(message, color = 'reset') {
    console.log(`${colors[color]}${message}${colors.reset}`);
}

function logSuccess(message) {
    log(`✅ ${message}`, 'green');
}

function logError(message) {
    log(`❌ ${message}`, 'red');
}

function logInfo(message) {
    log(`ℹ️  ${message}`, 'blue');
}

function logWarning(message) {
    log(`⚠️  ${message}`, 'yellow');
}

// 测试用例类
class RealRoomApiTester {
    constructor() {
        this.testResults = [];
    }

    async runTest(name, testFn) {
        log(`\n🧪 开始测试: ${name}`, 'cyan');
        try {
            const startTime = Date.now();
            const result = await testFn();
            const duration = Date.now() - startTime;
            
            this.testResults.push({
                name,
                success: true,
                duration,
                result
            });
            
            logSuccess(`测试通过: ${name} (${duration}ms)`);
            return result;
        } catch (error) {
            this.testResults.push({
                name,
                success: false,
                error: error.message,
                stack: error.stack
            });
            
            logError(`测试失败: ${name} - ${error.message}`);
            throw error;
        }
    }

    // 1. 测试真实房间点查询
    async testRealRoomPointQuery() {
        return this.runTest('真实房间点查询', async () => {
            // 使用一些真实的测试坐标点
            const testPoints = [
                { point: [10271.33, -140.43, 14275.37], name: 'AMS项目测试点1' },
                { point: [5000.0, 0.0, 3000.0], name: '标准测试点2' },
                { point: [0.0, 0.0, 0.0], name: '原点测试' },
                { point: [15000.0, 2000.0, 8000.0], name: '高坐标测试点' }
            ];

            const results = [];
            for (const testPoint of testPoints) {
                const params = new URLSearchParams({
                    'point[0]': testPoint.point[0],
                    'point[1]': testPoint.point[1], 
                    'point[2]': testPoint.point[2],
                    tolerance: '10.0',
                    max_results: '5'
                });

                const response = await axios.get(`${API_BASE}/query?${params}`);
                
                if (response.status !== 200) {
                    throw new Error(`查询失败: ${response.status}`);
                }

                const result = response.data;
                logInfo(`${testPoint.name} [${testPoint.point.join(', ')}]:`);
                
                if (result.success) {
                    if (result.room_number) {
                        logSuccess(`  ✅ 找到房间: ${result.room_number}`);
                        logInfo(`  面板REFNO: ${result.panel_refno || '无'}`);
                        logInfo(`  置信度: ${(result.confidence * 100).toFixed(1)}%`);
                    } else {
                        logWarning(`  ⚠️  未找到房间`);
                    }
                    logInfo(`  查询时间: ${result.query_time_ms.toFixed(2)}ms`);
                } else {
                    logError(`  ❌ 查询失败`);
                }
                
                results.push({ ...testPoint, result });
            }

            return results;
        });
    }

    // 2. 测试真实批量房间查询
    async testRealBatchRoomQuery() {
        return this.runTest('真实批量房间查询', async () => {
            const batchRequest = {
                points: [
                    [10271.33, -140.43, 14275.37], // AMS项目已知点
                    [5000.0, 0.0, 3000.0],
                    [-1000.0, 500.0, 2000.0],
                    [0.0, 0.0, 0.0],
                    [15000.0, 2000.0, 8000.0],
                    [8000.0, 1000.0, 5000.0]
                ],
                tolerance: 15.0
            };

            const response = await axios.post(`${API_BASE}/batch-query`, batchRequest);
            
            if (response.status !== 200) {
                throw new Error(`批量查询失败: ${response.status}`);
            }

            const data = response.data;
            logInfo(`批量查询结果: ${data.results.length} 个结果`);
            logInfo(`总查询时间: ${data.total_query_time_ms.toFixed(2)}ms`);
            
            let foundRooms = 0;
            data.results.forEach((result, index) => {
                const point = batchRequest.points[index];
                if (result.success && result.room_number) {
                    foundRooms++;
                    logSuccess(`  点 ${index + 1} [${point.join(', ')}]: 房间=${result.room_number}, 置信度=${(result.confidence * 100).toFixed(1)}%`);
                } else {
                    logWarning(`  点 ${index + 1} [${point.join(', ')}]: 未找到房间`);
                }
            });

            logInfo(`成功找到房间: ${foundRooms}/${data.results.length}`);
            return data;
        });
    }

    // 3. 测试系统状态（真实缓存统计）
    async testRealSystemStatus() {
        return this.runTest('真实系统状态查询', async () => {
            const response = await axios.get(`${API_BASE}/status`);
            
            if (response.status !== 200) {
                throw new Error(`状态查询失败: ${response.status}`);
            }
            
            const data = response.data;
            logInfo(`系统健康状态: ${data.system_health}`);
            logInfo(`活跃任务数: ${data.active_tasks}`);
            
            // 显示真实的缓存统计
            const cache = data.cache_status;
            logInfo(`几何缓存大小: ${cache.geometry_cache_size}`);
            logInfo(`查询缓存大小: ${cache.query_cache_size}`);
            logInfo(`缓存命中率: ${(cache.hit_rate * 100).toFixed(2)}%`);
            
            // 显示系统指标
            const metrics = data.metrics;
            logInfo(`总操作数: ${metrics.total_operations}`);
            logInfo(`成功率: ${(metrics.success_rate * 100).toFixed(2)}%`);
            logInfo(`平均响应时间: ${metrics.avg_response_time_ms.toFixed(2)}ms`);
            
            return data;
        });
    }

    // 4. 测试房间代码处理
    async testRoomCodeProcessing() {
        return this.runTest('房间代码处理', async () => {
            const codeRequest = {
                codes: [
                    'R610',
                    'r661', 
                    '/123AB-RM03-R310',
                    'AE-AC01-R',
                    'ROOM_001',
                    'invalid_code',
                    ''
                ],
                project_type: 'PDMS'
            };

            const response = await axios.post(`${API_BASE}/process-codes`, codeRequest);
            
            if (response.status !== 200) {
                throw new Error(`代码处理失败: ${response.status}`);
            }

            const data = response.data;
            logInfo(`代码处理完成: ${data.results.length} 个结果`);
            logInfo(`处理时间: ${data.processing_time_ms.toFixed(2)}ms`);
            
            data.results.forEach((result, index) => {
                const status = result.success ? '✅' : '❌';
                logInfo(`  ${status} "${result.input}" -> ${result.standardized_code || '无效'}`);
            });

            return data;
        });
    }

    // 5. 性能压力测试
    async testPerformanceStress() {
        return this.runTest('性能压力测试', async () => {
            logInfo('开始性能压力测试...');
            
            // 生成100个随机测试点
            const randomPoints = [];
            for (let i = 0; i < 100; i++) {
                randomPoints.push([
                    Math.random() * 20000 - 10000, // X: -10000 到 10000
                    Math.random() * 4000 - 2000,   // Y: -2000 到 2000  
                    Math.random() * 16000           // Z: 0 到 16000
                ]);
            }

            const batchRequest = {
                points: randomPoints,
                tolerance: 10.0
            };

            const startTime = Date.now();
            const response = await axios.post(`${API_BASE}/batch-query`, batchRequest);
            const totalTime = Date.now() - startTime;
            
            if (response.status !== 200) {
                throw new Error(`压力测试失败: ${response.status}`);
            }

            const data = response.data;
            const foundRooms = data.results.filter(r => r.success && r.room_number).length;
            
            logInfo(`压力测试结果:`);
            logInfo(`  测试点数: ${randomPoints.length}`);
            logInfo(`  找到房间: ${foundRooms}`);
            logInfo(`  总耗时: ${totalTime}ms`);
            logInfo(`  平均每点: ${(totalTime / randomPoints.length).toFixed(2)}ms`);
            logInfo(`  服务器处理时间: ${data.total_query_time_ms.toFixed(2)}ms`);
            
            return {
                total_points: randomPoints.length,
                found_rooms: foundRooms,
                total_time_ms: totalTime,
                avg_per_point_ms: totalTime / randomPoints.length,
                server_time_ms: data.total_query_time_ms
            };
        });
    }

    // 运行所有测试
    async runAllTests() {
        log('\n🚀 开始真实房间计算API测试', 'bright');
        log('=' * 60, 'cyan');

        try {
            // 基础功能测试
            await this.testRealSystemStatus();
            await this.testRealRoomPointQuery();
            await this.testRealBatchRoomQuery();
            await this.testRoomCodeProcessing();
            
            // 性能测试
            await this.testPerformanceStress();
            
        } catch (error) {
            logError(`测试过程中发生错误: ${error.message}`);
        }

        // 输出测试总结
        this.printTestSummary();
    }

    // 打印测试总结
    printTestSummary() {
        log('\n📊 测试总结', 'bright');
        log('=' * 60, 'cyan');
        
        const totalTests = this.testResults.length;
        const passedTests = this.testResults.filter(r => r.success).length;
        const failedTests = totalTests - passedTests;
        
        log(`总测试数: ${totalTests}`, 'blue');
        logSuccess(`通过: ${passedTests}`);
        
        if (failedTests > 0) {
            logError(`失败: ${failedTests}`);
            
            log('\n失败的测试:', 'red');
            this.testResults
                .filter(r => !r.success)
                .forEach(r => {
                    log(`  ❌ ${r.name}: ${r.error}`, 'red');
                });
        }
        
        if (passedTests > 0) {
            log('\n通过的测试:', 'green');
            this.testResults
                .filter(r => r.success)
                .forEach(r => {
                    log(`  ✅ ${r.name} (${r.duration}ms)`, 'green');
                });
        }
        
        const successRate = ((passedTests / totalTests) * 100).toFixed(1);
        log(`\n成功率: ${successRate}%`, successRate >= 80 ? 'green' : 'red');
        
        // 性能总结
        const avgTime = this.testResults
            .filter(r => r.success)
            .reduce((sum, r) => sum + r.duration, 0) / passedTests;
        log(`平均测试时间: ${avgTime.toFixed(2)}ms`, 'blue');
    }
}

// 检查服务器连接
async function checkServerConnection() {
    try {
        logInfo('检查服务器连接...');
        const response = await axios.get(`${BASE_URL}/health`, { timeout: 5000 });
        logSuccess(`服务器连接正常 (${response.status})`);
        return true;
    } catch (error) {
        if (error.code === 'ECONNREFUSED') {
            logError('无法连接到服务器，请确保Web服务器正在运行');
            logInfo('启动命令: cargo run --bin web_server --features web_server');
            logInfo('确保启用了sqlite特性以使用真实查询功能');
        } else {
            logError(`服务器连接检查失败: ${error.message}`);
        }
        return false;
    }
}

// 主函数
async function main() {
    log('🏠 真实房间计算API测试工具', 'bright');
    log(`服务器地址: ${BASE_URL}`, 'blue');
    log('测试使用aios-core真实查询方法', 'magenta');
    
    // 检查服务器连接
    const serverOk = await checkServerConnection();
    if (!serverOk) {
        process.exit(1);
    }
    
    // 运行测试
    const tester = new RealRoomApiTester();
    await tester.runAllTests();
    
    // 根据测试结果设置退出码
    const failedTests = tester.testResults.filter(r => !r.success).length;
    process.exit(failedTests > 0 ? 1 : 0);
}

// 错误处理
process.on('unhandledRejection', (reason, promise) => {
    logError(`未处理的Promise拒绝: ${reason}`);
    process.exit(1);
});

process.on('uncaughtException', (error) => {
    logError(`未捕获的异常: ${error.message}`);
    process.exit(1);
});

// 运行主函数
if (require.main === module) {
    main();
}

module.exports = { RealRoomApiTester };
