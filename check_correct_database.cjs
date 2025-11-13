#!/usr/bin/env node

// 检查正确的数据库配置

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

async function querySurrealDirect(port, sql, ns, db) {
    return new Promise((resolve, reject) => {
        const postData = JSON.stringify({ sql });
        
        const req = http.request({
            hostname: '127.0.0.1',
            port: port,
            path: '/sql',
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Accept': 'application/json',
                'NS': ns,
                'DB': db,
                'Authorization': 'Basic ' + Buffer.from('root:root').toString('base64')
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

async function checkCorrectDatabase() {
    log('=== 检查正确的数据库配置 ===', 'blue');
    
    // 从run_model_gen_test.sh看到的配置
    const combinations = [
        { ns: '1516', db: '1112' },
        { ns: '1500', db: 'SLYK' },
        { ns: '1500', db: '1112' },
        { ns: '1516', db: 'SLYK' },
    ];
    
    const problemRefnos = ['14207_545', '14207_856', '14207_858', '14207_1357', '14207_185'];
    
    for (const combo of combinations) {
        log(`\n--- 检查命名空间: ${combo.ns}, 数据库: ${combo.db} ---`, 'cyan');
        
        try {
            // 1. 检查pe表记录总数
            const peCount = await querySurrealDirect(8009, 'SELECT COUNT() as count FROM pe GROUP ALL', combo.ns, combo.db);
            if (peCount.result && peCount.result.length > 0) {
                log(`   ✅ pe表记录数: ${peCount.result[0].count}`, 'green');
                
                // 2. 检查inst_relate表
                const instCount = await querySurrealDirect(8009, 'SELECT COUNT() as count FROM inst_relate GROUP ALL', combo.ns, combo.db);
                if (instCount.result && instCount.result.length > 0) {
                    log(`   ✅ inst_relate表记录数: ${instCount.result[0].count}`, 'green');
                }
                
                // 3. 检查问题refno
                log('   检查问题refno:', 'yellow');
                let foundCount = 0;
                for (const refno of problemRefnos) {
                    const checkSql = `SELECT * FROM pe WHERE refno = ${refno}`;
                    const result = await querySurrealDirect(8009, checkSql, combo.ns, combo.db);
                    if (result.result && result.result.length > 0) {
                        log(`     ✅ ${refno}: 找到`, 'green');
                        foundCount++;
                    } else {
                        log(`     ❌ ${refno}: 未找到`, 'red');
                    }
                }
                
                if (foundCount > 0) {
                    log(`   🎯 找到 ${foundCount} 个问题refno！`, 'green');
                    
                    // 4. 检查这些refno的详细信息
                    for (const refno of problemRefnos) {
                        const detailSql = `SELECT * FROM pe WHERE refno = ${refno}`;
                        const result = await querySurrealDirect(8009, detailSql, combo.ns, combo.db);
                        if (result.result && result.result.length > 0) {
                            const record = result.result[0];
                            log(`     详细信息 ${refno}:`, 'blue');
                            log(`       id: ${record.id}`, 'blue');
                            log(`       noun: ${record.noun || 'N/A'}`, 'blue');
                            log(`       type: ${record.type || 'N/A'}`, 'blue');
                            
                            // 检查相关的inst_relate
                            const instSql = `SELECT * FROM ${record.id}->inst_relate`;
                            const instResult = await querySurrealDirect(8009, instSql, combo.ns, combo.db);
                            if (instResult.result && instResult.result.length > 0) {
                                log(`       inst_relate: ${instResult.result.length}条记录`, 'blue');
                            }
                        }
                    }
                }
                
                // 5. 如果有数据，检查一些示例
                if (peCount.result[0].count > 0) {
                    const samples = await querySurrealDirect(8009, 'SELECT refno, noun FROM pe LIMIT 5', combo.ns, combo.db);
                    if (samples.result && samples.result.length > 0) {
                        log('   示例refno:', 'blue');
                        for (const record of samples.result) {
                            log(`     ${record.refno}: ${record.noun || 'N/A'}`, 'blue');
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
    
    log('\n=== 检查完成 ===', 'green');
}

checkCorrectDatabase().catch(console.error);
