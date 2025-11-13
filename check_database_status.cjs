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

async function checkDatabaseStatus() {
    log('=== 检查数据库状态 ===', 'blue');
    
    try {
        // 1. 检查数据库基本信息
        log('1. 检查pe表记录总数...', 'cyan');
        const peCount = await queryDatabase('SELECT COUNT() as count FROM pe GROUP ALL');
        if (peCount.result && peCount.result.length > 0) {
            log(`   pe表总记录数: ${peCount.result[0].count}`, 'green');
        } else {
            log('   ❌ pe表没有记录', 'red');
        }
        
        // 2. 检查表结构
        log('\n2. 检查pe表结构...', 'cyan');
        const peSchema = await queryDatabase('SELECT * FROM pe LIMIT 1');
        if (peSchema.result && peSchema.result.length > 0) {
            log('   pe表结构示例:', 'green');
            const record = peSchema.result[0];
            Object.keys(record).forEach(key => {
                log(`     ${key}: ${typeof record[key]} (${record[key] !== null ? '有值' : 'null'})`, 'blue');
            });
        } else {
            log('   ❌ pe表没有数据', 'red');
        }
        
        // 3. 检查所有表
        log('\n3. 检查所有表...', 'cyan');
        const tables = await queryDatabase('SELECT * FROM information_schema.tables WHERE table_schema = $current_schema()');
        if (tables.result && tables.result.length > 0) {
            log('   数据库中的表:', 'green');
            for (const table of tables.result) {
                log(`     ${table.table_name}`, 'blue');
            }
        } else {
            log('   ❌ 无法获取表信息', 'red');
        }
        
        // 4. 检查refno字段的一些示例
        log('\n4. 检查refno示例...', 'cyan');
        const refnoSamples = await queryDatabase('SELECT refno FROM pe LIMIT 10');
        if (refnoSamples.result && refnoSamples.result.length > 0) {
            log('   refno示例:', 'green');
            for (const record of refnoSamples.result) {
                log(`     ${record.refno}`, 'blue');
            }
        } else {
            log('   ❌ 没有refno示例', 'red');
        }
        
        // 5. 检查数字开头的refno
        log('\n5. 检查数字开头的refno...', 'cyan');
        const numericRefnos = await queryDatabase("SELECT refno FROM pe WHERE refno LIKE '1%' LIMIT 10");
        if (numericRefnos.result && numericRefnos.result.length > 0) {
            log('   数字开头的refno示例:', 'green');
            for (const record of numericRefnos.result) {
                log(`     ${record.refno}`, 'blue');
            }
        } else {
            log('   ❌ 没有数字开头的refno', 'red');
        }
        
        // 6. 检查布尔运算相关表
        log('\n6. 检查布尔运算相关表...', 'cyan');
        const booleanTables = ['cata_neg_boolean_group', 'inst_relate', 'geo_relate'];
        for (const tableName of booleanTables) {
            const countSql = `SELECT COUNT() as count FROM ${tableName} GROUP ALL`;
            const count = await queryDatabase(countSql);
            if (count.result && count.result.length > 0) {
                log(`   ${tableName}表: ${count.result[0].count}条记录`, 'green');
            } else {
                log(`   ${tableName}表: 没有记录`, 'yellow');
            }
        }
        
        // 7. 检查数据库连接信息
        log('\n7. 检查数据库连接信息...', 'cyan');
        const dbInfo = await queryDatabase('SELECT * FROM $auth');
        if (dbInfo.result) {
            log('   认证信息:', 'green');
            log(`     ${JSON.stringify(dbInfo.result, null, 6)}`, 'blue');
        }
        
    } catch (error) {
        log(`❌ 检查过程中发生错误: ${error.message}`, 'red');
    }
    
    log('\n=== 检查完成 ===', 'green');
}

// 运行检查
checkDatabaseStatus().catch(console.error);
