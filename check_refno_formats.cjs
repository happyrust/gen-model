#!/usr/bin/env node

const http = require('http');

const colors = {
    reset: '\x1b[0m',
    red: '\x1b[31m',
    green: '\x1b[32m',
    blue: '\x1b[34m',
    yellow: '\x1b[33m',
    cyan: '\x1b[36m',
};

function log(message, color = 'reset') {
    const timestamp = new Date().toISOString();
    console.log(`${colors[color]}[${timestamp}] ${message}${colors.reset}`);
}

async function queryDatabase(query) {
    return new Promise((resolve, reject) => {
        const postData = JSON.stringify({ query });

        const req = http.request({
            hostname: 'localhost',
            port: 8080,
            path: '/api/database/query',
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Content-Length': postData.length
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

async function checkRefnoFormats() {
    log('=== 检查refno格式问题 ===', 'blue');
    
    // 问题refno列表
    const problemRefnos = ['14207_545', '14207_856', '14207_858', '14207_1357', '14207_185'];
    
    // 检查不同的格式
    const formats = [
        { name: '下划线格式', template: (r) => `SELECT * FROM pe WHERE refno = ${r}` },
        { name: '斜杠格式', template: (r) => `SELECT * FROM pe WHERE refno = ${r.replace('_', '/')}` },
        { name: '包含查询', template: (r) => `SELECT * FROM pe WHERE refno CONTAINS '${r.split('_')[0]}' AND refno CONTAINS '${r.split('_')[1]}'` },
        { name: '前缀匹配', template: (r) => `SELECT * FROM pe WHERE refno LIKE '${r.split('_')[0]}%'` }
    ];
    
    for (const refnoStr of problemRefnos) {
        log(`\n--- 检查 refno: ${refnoStr} ---`, 'cyan');
        
        for (const format of formats) {
            try {
                const sql = format.template(refnoStr);
                log(`  ${format.name}: ${sql}`, 'yellow');
                const result = await queryDatabase(sql);
                
                if (result.result && result.result.length > 0) {
                    log(`    ✅ 找到 ${result.result.length} 条记录`, 'green');
                    for (let i = 0; i < Math.min(result.result.length, 3); i++) {
                        const record = result.result[i];
                        log(`      [${i}] id: ${record.id}, refno: ${record.refno}, noun: ${record.noun || 'N/A'}`, 'blue');
                    }
                    if (result.result.length > 3) {
                        log(`      ... 还有 ${result.result.length - 3} 条记录`, 'blue');
                    }
                } else {
                    log(`    ❌ 没有找到记录`, 'red');
                }
            } catch (error) {
                log(`    ❌ 查询错误: ${error.message}`, 'red');
            }
        }
    }
    
    // 检查所有包含14207的refno
    log('\n=== 检查所有包含14207的refno ===', 'blue');
    try {
        const sql = "SELECT refno, noun FROM pe WHERE refno CONTAINS '14207' ORDER BY refno LIMIT 20";
        log(`执行查询: ${sql}`, 'yellow');
        const result = await queryDatabase(sql);
        
        if (result.result && result.result.length > 0) {
            log(`找到 ${result.result.length} 条包含14207的记录:`, 'green');
            for (const record of result.result) {
                log(`  ${record.refno}: ${record.noun || 'N/A'}`, 'blue');
            }
        } else {
            log('❌ 没有找到包含14207的记录', 'red');
        }
    } catch (error) {
        log(`❌ 查询错误: ${error.message}`, 'red');
    }
    
    // 检查数据库中所有的refno前缀
    log('\n=== 检查refno前缀统计 ===', 'blue');
    try {
        const sql = "SELECT refno, COUNT() as count FROM pe GROUP BY refno ORDER BY count DESC LIMIT 20";
        log(`执行查询: ${sql}`, 'yellow');
        const result = await queryDatabase(sql);
        
        if (result.result && result.result.length > 0) {
            log('refno出现频率统计(前20):', 'green');
            for (const record of result.result) {
                log(`  ${record.refno}: ${record.count}次`, 'blue');
            }
        } else {
            log('❌ 没有找到refno统计', 'red');
        }
    } catch (error) {
        log(`❌ 查询错误: ${error.message}`, 'red');
    }
    
    log('\n=== 检查完成 ===', 'green');
}

// 运行检查
checkRefnoFormats().catch(console.error);
