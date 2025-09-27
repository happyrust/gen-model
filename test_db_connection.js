#!/usr/bin/env node

const http = require('http');

const colors = {
    reset: '\x1b[0m',
    red: '\x1b[31m',
    green: '\x1b[32m',
    blue: '\x1b[34m',
    yellow: '\x1b[33m',
};

function log(message, color = 'reset') {
    const timestamp = new Date().toISOString();
    console.log(`${colors[color]}[${timestamp}] ${message}${colors.reset}`);
}

async function testDatabaseConnection() {
    log('🔍 测试数据库连接状态', 'blue');

    try {
        // 测试数据库连接检查 API
        const result = await new Promise((resolve, reject) => {
            const req = http.request({
                hostname: 'localhost',
                port: 8080,
                path: '/api/database/connection/check',
                method: 'GET'
            }, (res) => {
                let data = '';
                res.on('data', (chunk) => data += chunk);
                res.on('end', () => {
                    try {
                        const json = JSON.parse(data);
                        resolve(json);
                    } catch (e) {
                        resolve({ raw: data, statusCode: res.statusCode });
                    }
                });
            });
            req.on('error', reject);
            req.end();
        });

        console.log('📊 数据库连接检查结果:');
        console.log(JSON.stringify(result, null, 2));

        if (result.status === 'connected' || result.connected === true) {
            log('✅ 数据库连接正常', 'green');

            // 测试获取系统状态
            await testSystemStatus();

        } else {
            log('❌ 数据库连接异常', 'red');
            log('建议检查数据库服务状态', 'yellow');
        }

    } catch (error) {
        log(`❌ 数据库连接测试失败: ${error.message}`, 'red');
    }
}

async function testSystemStatus() {
    log('🔍 获取系统状态', 'blue');

    try {
        const result = await new Promise((resolve, reject) => {
            const req = http.request({
                hostname: 'localhost',
                port: 8080,
                path: '/api/status',
                method: 'GET'
            }, (res) => {
                let data = '';
                res.on('data', (chunk) => data += chunk);
                res.on('end', () => {
                    try {
                        const json = JSON.parse(data);
                        resolve(json);
                    } catch (e) {
                        resolve({ raw: data, statusCode: res.statusCode });
                    }
                });
            });
            req.on('error', reject);
            req.end();
        });

        console.log('📊 系统状态:');
        console.log(JSON.stringify(result, null, 2));

    } catch (error) {
        log(`❌ 系统状态获取失败: ${error.message}`, 'red');
    }
}

// 运行测试
testDatabaseConnection();