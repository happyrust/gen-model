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

async function main() {
    log('🔍 开始查询 DBNUM 1112 数据库状态', 'blue');

    try {
        // 1. 查询 pe 表中的总记录数
        log('📊 查询 pe 表中 DBNUM 1112 的记录数...', 'cyan');
        const peCount = await queryDatabase('SELECT COUNT() FROM pe WHERE dbno = 1112 GROUP ALL');
        console.log('PE 记录数:', JSON.stringify(peCount, null, 2));

        // 2. 查询 inst_geos 表
        log('📊 查询 inst_geos 表中 DBNUM 1112 的记录数...', 'cyan');
        const instGeosCount = await queryDatabase('SELECT COUNT() FROM inst_geos WHERE dbno = 1112 GROUP ALL');
        console.log('inst_geos 记录数:', JSON.stringify(instGeosCount, null, 2));

        // 3. 查询 meshes 表
        log('📊 查询 meshes 表中 DBNUM 1112 的记录数...', 'cyan');
        const meshesCount = await queryDatabase('SELECT COUNT() FROM meshes WHERE dbno = 1112 GROUP ALL');
        console.log('meshes 记录数:', JSON.stringify(meshesCount, null, 2));

        // 4. 查询具体的 17496/266203 相关记录
        log('📊 查询 17496/266203 相关的 pe 记录...', 'cyan');
        const specificPe = await queryDatabase('SELECT * FROM pe WHERE refno CONTAINS "17496" AND refno CONTAINS "266203"');
        console.log('17496/266203 PE 记录:', JSON.stringify(specificPe, null, 2));

        // 5. 查询层级关系
        log('📊 查询 17496/266203 的子节点...', 'cyan');
        const children = await queryDatabase('SELECT refno FROM pe WHERE parent_refno = "17496/266203" OR parent_refno = "17496_266203"');
        console.log('子节点:', JSON.stringify(children, null, 2));

        // 6. 查询有几何数据的记录
        log('📊 查询有几何数据的记录...', 'cyan');
        const geoRecords = await queryDatabase('SELECT refno, type FROM pe WHERE dbno = 1112 AND type IN ["GENSEC", "CYSEC", "NZSEC", "DISEC", "CYLI", "BOX", "SNOUT", "DISH", "CTORUS", "RTORUS", "PYRAMID"] LIMIT 10');
        console.log('有几何数据的记录:', JSON.stringify(geoRecords, null, 2));

        log('✅ 查询完成', 'green');
    } catch (error) {
        log(`❌ 查询失败: ${error.message}`, 'red');
    }
}

main();