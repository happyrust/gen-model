#!/usr/bin/env node

// 直接连接SurrealDB 8009端口检查数据

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

async function querySurrealDirect(port, sql, ns = '1500', db = 'SLYK') {
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

async function checkAllPorts() {
    log('=== 直接检查所有SurrealDB端口 ===', 'blue');
    
    const ports = [8009, 8010, 8020];
    const problemRefnos = ['14207_545', '14207_856', '14207_858', '14207_1357', '14207_185'];
    
    for (const port of ports) {
        log(`\n--- 检查端口 ${port} ---`, 'cyan');
        
        try {
            // 1. 检查数据库基本信息
            const dbInfo = await querySurrealDirect(port, 'INFO FOR DB');
            log(`数据库信息: ${JSON.stringify(dbInfo, null, 2)}`, 'blue');
            
            // 2. 检查表
            const tables = await querySurrealDirect(port, 'INFO FOR TABLES');
            if (tables.result && tables.result.length > 0) {
                log(`找到表: ${tables.result.join(', ')}`, 'green');
                
                // 3. 检查pe表
                const peCount = await querySurrealDirect(port, 'SELECT COUNT() as count FROM pe GROUP ALL');
                if (peCount.result && peCount.result.length > 0) {
                    log(`pe表记录数: ${peCount.result[0].count}`, 'green');
                    
                    // 4. 检查问题refno
                    for (const refno of problemRefnos) {
                        const checkSql = `SELECT * FROM pe WHERE refno = ${refno}`;
                        const result = await querySurrealDirect(port, checkSql);
                        if (result.result && result.result.length > 0) {
                            log(`  ✅ ${refno}: 找到`, 'green');
                        } else {
                            log(`  ❌ ${refno}: 未找到`, 'red');
                        }
                    }
                    
                    // 5. 检查一些示例refno
                    const samples = await querySurrealDirect(port, 'SELECT refno, noun FROM pe LIMIT 10');
                    if (samples.result && samples.result.length > 0) {
                        log('示例refno:', 'blue');
                        for (const record of samples.result) {
                            log(`  ${record.refno}: ${record.noun || 'N/A'}`, 'blue');
                        }
                    }
                } else {
                    log('pe表没有记录', 'red');
                }
            } else {
                log('没有找到表', 'red');
            }
            
        } catch (error) {
            log(`端口 ${port} 连接失败: ${error.message}`, 'red');
        }
    }
    
    log('\n=== 检查完成 ===', 'green');
}

checkAllPorts().catch(console.error);
