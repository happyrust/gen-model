#!/usr/bin/env node

/**
 * 房间计算API测试脚本
 * 测试 gen-model-fork 项目的房间计算功能
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

// 延迟函数
function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

// 测试用例类
class RoomApiTester {
    constructor() {
        this.testResults = [];
        this.taskIds = [];
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

    // 1. 测试系统状态
    async testSystemStatus() {
        return this.runTest('系统状态查询', async () => {
            const response = await axios.get(`${API_BASE}/status`);
            
            if (response.status !== 200) {
                throw new Error(`状态码错误: ${response.status}`);
            }
            
            const data = response.data;
            logInfo(`系统健康状态: ${data.system_health}`);
            logInfo(`活跃任务数: ${data.active_tasks}`);
            logInfo(`缓存命中率: ${data.cache_status.hit_rate}`);
            
            return data;
        });
    }

    // 2. 测试房间点查询
    async testRoomPointQuery() {
        return this.runTest('房间点查询', async () => {
            const testPoints = [
                { point: [10271.33, -140.43, 14275.37], name: '测试点1' },
                { point: [5000.0, 0.0, 3000.0], name: '测试点2' },
                { point: [-1000.0, 500.0, 2000.0], name: '测试点3' }
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
                logInfo(`  房间号: ${result.room_number || '未找到'}`);
                logInfo(`  面板REFNO: ${result.panel_refno || '无'}`);
                logInfo(`  置信度: ${result.confidence || 0}`);
                logInfo(`  查询时间: ${result.query_time_ms}ms`);
                
                results.push({ ...testPoint, result });
            }

            return results;
        });
    }

    // 3. 测试批量房间查询
    async testBatchRoomQuery() {
        return this.runTest('批量房间查询', async () => {
            const batchRequest = {
                points: [
                    [10271.33, -140.43, 14275.37],
                    [5000.0, 0.0, 3000.0],
                    [-1000.0, 500.0, 2000.0],
                    [0.0, 0.0, 0.0],
                    [15000.0, 2000.0, 8000.0]
                ],
                tolerance: 15.0
            };

            const response = await axios.post(`${API_BASE}/batch-query`, batchRequest);
            
            if (response.status !== 200) {
                throw new Error(`批量查询失败: ${response.status}`);
            }

            const data = response.data;
            logInfo(`批量查询成功: ${data.results.length} 个结果`);
            logInfo(`总查询时间: ${data.total_query_time_ms}ms`);
            
            data.results.forEach((result, index) => {
                logInfo(`  点 ${index + 1}: 房间号=${result.room_number}, 时间=${result.query_time_ms}ms`);
            });

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
            logInfo(`处理时间: ${data.processing_time_ms}ms`);
            
            data.results.forEach((result, index) => {
                logInfo(`  代码 "${result.input}": ${result.success ? '✅' : '❌'} -> ${result.standardized_code || '无'}`);
            });

            return data;
        });
    }

    // 5. 测试房间任务创建 - 重建关系
    async testCreateRebuildTask() {
        return this.runTest('创建重建关系任务', async () => {
            const taskRequest = {
                task_type: 'RebuildRelations',
                config: {
                    project_code: 'TEST_PROJECT',
                    room_keywords: ['R610', 'R661'],
                    database_numbers: [1, 2],
                    force_rebuild: true,
                    batch_size: 100,
                    validation_options: {
                        check_room_codes: true,
                        check_spatial_consistency: true,
                        check_reference_integrity: true
                    },
                    model_generation: {
                        generate_model: false,
                        generate_mesh: false,
                        generate_spatial_tree: false,
                        apply_boolean_operation: false,
                        mesh_tolerance_ratio: 0.01,
                        output_formats: ['Xkt'],
                        quality_level: 'Medium'
                    }
                }
            };

            const response = await axios.post(`${API_BASE}/tasks`, taskRequest);
            
            if (response.status !== 200) {
                throw new Error(`任务创建失败: ${response.status}`);
            }

            const task = response.data;
            this.taskIds.push(task.id);
            
            logInfo(`任务创建成功: ${task.id}`);
            logInfo(`任务类型: ${task.task_type}`);
            logInfo(`任务状态: ${task.status}`);
            logInfo(`创建时间: ${task.created_at}`);

            return task;
        });
    }

    // 6. 测试房间任务创建 - 数据验证
    async testCreateValidationTask() {
        return this.runTest('创建数据验证任务', async () => {
            const taskRequest = {
                task_type: 'DataValidation',
                config: {
                    project_code: null,
                    room_keywords: [],
                    database_numbers: [],
                    force_rebuild: false,
                    batch_size: 500,
                    validation_options: {
                        check_room_codes: true,
                        check_spatial_consistency: true,
                        check_reference_integrity: true
                    },
                    model_generation: {
                        generate_model: false,
                        generate_mesh: false,
                        generate_spatial_tree: false,
                        apply_boolean_operation: false,
                        mesh_tolerance_ratio: 0.01,
                        output_formats: [],
                        quality_level: 'Low'
                    }
                }
            };

            const response = await axios.post(`${API_BASE}/tasks`, taskRequest);
            
            if (response.status !== 200) {
                throw new Error(`验证任务创建失败: ${response.status}`);
            }

            const task = response.data;
            this.taskIds.push(task.id);
            
            logInfo(`验证任务创建成功: ${task.id}`);
            logInfo(`任务类型: ${task.task_type}`);
            logInfo(`任务状态: ${task.status}`);

            return task;
        });
    }

    // 7. 测试任务状态查询
    async testTaskStatusQuery() {
        return this.runTest('任务状态查询', async () => {
            if (this.taskIds.length === 0) {
                throw new Error('没有可查询的任务ID');
            }

            const results = [];
            for (const taskId of this.taskIds) {
                const response = await axios.get(`${API_BASE}/tasks/${taskId}`);
                
                if (response.status !== 200) {
                    throw new Error(`任务状态查询失败: ${response.status}`);
                }

                const task = response.data;
                logInfo(`任务 ${taskId}:`);
                logInfo(`  状态: ${task.status}`);
                logInfo(`  进度: ${task.progress}%`);
                logInfo(`  消息: ${task.message}`);
                logInfo(`  更新时间: ${task.updated_at}`);
                
                if (task.result) {
                    logInfo(`  结果: 成功=${task.result.success}, 处理数=${task.result.processed_count}, 错误数=${task.result.error_count}`);
                    logInfo(`  耗时: ${task.result.duration_ms}ms`);
                }

                results.push(task);
            }

            return results;
        });
    }

    // 8. 测试快照创建
    async testCreateSnapshot() {
        return this.runTest('创建数据快照', async () => {
            const description = `测试快照_${new Date().toISOString()}`;
            
            const response = await axios.post(`${API_BASE}/snapshot`, JSON.stringify(description), {
                headers: {
                    'Content-Type': 'application/json'
                }
            });
            
            if (response.status !== 200) {
                throw new Error(`快照创建失败: ${response.status}`);
            }

            const result = response.data;
            logInfo(`快照创建成功: ${result.operation_id}`);
            logInfo(`消息: ${result.message}`);
            logInfo(`时间戳: ${result.timestamp}`);

            return result;
        });
    }

    // 等待任务完成
    async waitForTasksCompletion(maxWaitTime = 30000) {
        logInfo(`等待任务完成 (最多 ${maxWaitTime/1000} 秒)...`);
        
        const startTime = Date.now();
        while (Date.now() - startTime < maxWaitTime) {
            let allCompleted = true;
            
            for (const taskId of this.taskIds) {
                try {
                    const response = await axios.get(`${API_BASE}/tasks/${taskId}`);
                    const task = response.data;
                    
                    if (task.status === 'Running' || task.status === 'Pending') {
                        allCompleted = false;
                        logInfo(`任务 ${taskId} 仍在运行: ${task.status} (${task.progress}%)`);
                    }
                } catch (error) {
                    logWarning(`查询任务 ${taskId} 状态失败: ${error.message}`);
                }
            }
            
            if (allCompleted) {
                logSuccess('所有任务已完成');
                return;
            }
            
            await sleep(2000); // 等待2秒后再次检查
        }
        
        logWarning('等待超时，部分任务可能仍在运行');
    }

    // 运行所有测试
    async runAllTests() {
        log('\n🚀 开始房间计算API测试', 'bright');
        log('=' * 50, 'cyan');

        try {
            // 基础功能测试
            await this.testSystemStatus();
            await this.testRoomPointQuery();
            await this.testBatchRoomQuery();
            await this.testRoomCodeProcessing();
            
            // 任务管理测试
            await this.testCreateRebuildTask();
            await this.testCreateValidationTask();
            await this.testCreateSnapshot();
            
            // 等待任务完成
            await this.waitForTasksCompletion();
            
            // 最终状态查询
            await this.testTaskStatusQuery();
            
        } catch (error) {
            logError(`测试过程中发生错误: ${error.message}`);
        }

        // 输出测试总结
        this.printTestSummary();
    }

    // 打印测试总结
    printTestSummary() {
        log('\n📊 测试总结', 'bright');
        log('=' * 50, 'cyan');
        
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
        } else {
            logError(`服务器连接检查失败: ${error.message}`);
        }
        return false;
    }
}

// 主函数
async function main() {
    log('🏠 房间计算API测试工具', 'bright');
    log(`服务器地址: ${BASE_URL}`, 'blue');
    
    // 检查服务器连接
    const serverOk = await checkServerConnection();
    if (!serverOk) {
        process.exit(1);
    }
    
    // 运行测试
    const tester = new RoomApiTester();
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

module.exports = { RoomApiTester };
