#!/usr/bin/env node

const http = require('http');

function log(message, color = 'reset') {
    const colors = {
        reset: '\x1b[0m',
        red: '\x1b[31m',
        green: '\x1b[32m',
        blue: '\x1b[34m',
        yellow: '\x1b[33m',
        cyan: '\x1b[36m',
    };
    const timestamp = new Date().toISOString();
    console.log(`${colors[color]}[${timestamp}] ${message}${colors.reset}`);
}

async function queryDatabase(query, ns = '1500', db = 'SLYK') {
    return new Promise((resolve, reject) => {
        const postData = JSON.stringify({ query });

        const req = http.request({
            hostname: 'localhost',
            port: 8080,
            path: '/api/database/query',
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Content-Length': postData.length,
                'NS': ns,
                'DB': db
            }
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
        req.write(postData);
        req.end();
    });
}

async function checkAllDatabases() {
    log('=== 检查所有数据库和命名空间 ===', 'blue');
    
    // 检查不同的命名空间和数据库组合
    const combinations = [
        { ns: '1500', db: 'SLYK' },
        { ns: 'YCYK', db: 'E3D' },
        { ns: 'test', db: 'test' },
        { ns: 'root', db: 'root' },
    ];
    
    for (const combo of combinations) {
        log(`\n--- 检查命名空间: ${combo.ns}, 数据库: ${combo.db} ---`, 'cyan');
        
        try {
            // 1. 检查pe表记录总数
            const peCount = await queryDatabase('SELECT COUNT() as count FROM pe GROUP ALL', combo.ns, combo.db);
            if (peCount.result && peCount.result.length > 0) {
                log(`   ✅ pe表记录数: ${peCount.result[0].count}`, 'green');
                
                // 如果有数据，检查一些示例
                const samples = await queryDatabase('SELECT refno, noun FROM pe LIMIT 5', combo.ns, combo.db);
                if (samples.result && samples.result.length > 0) {
                    log('   示例记录:', 'blue');
                    for (const record of samples.result) {
                        log(`     ${record.refno}: ${record.noun || 'N/A'}`, 'blue');
                    }
                    
                    // 检查问题refno
                    const problemRefnos = ['14207_545', '14207_856', '14207_858', '14207_1357', '14207_185'];
                    log('   检查问题refno:', 'yellow');
                    for (const refno of problemRefnos) {
                        const checkSql = `SELECT * FROM pe WHERE refno = ${refno}`;
                        const result = await queryDatabase(checkSql, combo.ns, combo.db);
                        if (result.result && result.result.length > 0) {
                            log(`     ✅ ${refno}: 找到`, 'green');
                        } else {
                            log(`     ❌ ${refno}: 未找到`, 'red');
                        }
                    }
                }
            } else {
                log(`   ❌ pe表没有记录`, 'red');
            }
            
        } catch (error) {
            log(`   ❌ 查询错误: ${error.message}`, 'red');
        }
    }
    
    // 检查数据库配置
    log('\n=== 检查数据库配置 ===', 'blue');
    try {
        // 查询数据库信息
        const dbInfo = await queryDatabase('SELECT * FROM $auth', '1500', 'SLYK');
        log('认证信息: ' + JSON.stringify(dbInfo, null, 2), 'blue');
    } catch (error) {
        log(`❌ 获取数据库信息失败: ${error.message}`, 'red');
    }
    
    log('\n=== 检查完成 ===', 'green');
}

// 运行检查
checkAllDatabases().catch(console.error);
