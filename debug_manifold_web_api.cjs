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

async function debugManifoldIssue() {
    log('=== 调试布尔运算失败问题 ===', 'blue');
    
    // 问题refno列表
    const problemRefnos = ['14207_545', '14207_856', '14207_858', '14207_1357', '14207_185'];
    
    for (const refnoStr of problemRefnos) {
        log(`\n--- 分析 refno: ${refnoStr} ---`, 'cyan');
        
        try {
            // 1. 检查这个refno的基本信息
            const basicInfoSql = `SELECT * FROM pe WHERE refno = ${refnoStr}`;
            log(`执行查询: ${basicInfoSql}`, 'yellow');
            const basicInfo = await queryDatabase(basicInfoSql);
            
            if (!basicInfo.result || basicInfo.result.length === 0) {
                log(`❌ refno ${refnoStr} 在pe表中不存在`, 'red');
                continue;
            }
            
            const peData = basicInfo.result[0];
            log(`✅ 找到pe记录: ${peData.id}, noun: ${peData.noun || 'N/A'}`, 'green');
            
            // 2. 检查inst_relate记录
            const instRelateSql = `SELECT * FROM ${peData.id}->inst_relate`;
            log(`执行查询: ${instRelateSql}`, 'yellow');
            const instRelate = await queryDatabase(instRelateSql);
            
            if (!instRelate.result || instRelate.result.length === 0) {
                log(`❌ refno ${refnoStr} 没有inst_relate记录`, 'red');
                continue;
            }
            
            const inst = instRelate.result[0];
            log(`✅ 找到inst_relate记录: ${inst.id}`, 'green');
            log(`   bad_bool: ${inst.bad_bool || false}`, 'blue');
            log(`   booled: ${inst.booled || false}`, 'blue');
            
            // 3. 检查几何数据关系
            const geoRelateSql = `SELECT * FROM ${inst.id}->geo_relate`;
            log(`执行查询: ${geoRelateSql}`, 'yellow');
            const geoRelates = await queryDatabase(geoRelateSql);
            
            if (!geoRelates.result || geoRelates.result.length === 0) {
                log(`❌ refno ${refnoStr} 没有geo_relate记录`, 'red');
                continue;
            }
            
            log(`✅ 找到 ${geoRelates.result.length} 个geo_relate记录`, 'green');
            
            // 4. 检查每个几何记录的详细信息
            for (let idx = 0; idx < geoRelates.result.length; idx++) {
                const geo = geoRelates.result[idx];
                log(`   几何记录[${idx}]: ${geo.id}`, 'blue');
                log(`     geom_refno: ${geo.geom_refno || 'N/A'}`, 'blue');
                log(`     geo_type: ${geo.geo_type || 'N/A'}`, 'blue');
                log(`     visible: ${geo.visible || false}`, 'blue');
                log(`     bad: ${geo.bad || false}`, 'blue');
                
                // 5. 检查inst_info和几何数据
                if (geo.out && geo.out.id) {
                    const instInfoSql = `SELECT * FROM ${geo.out.id}`;
                    log(`执行查询: ${instInfoSql}`, 'yellow');
                    const instInfo = await queryDatabase(instInfoSql);
                    
                    if (instInfo.result && instInfo.result.length > 0) {
                        const info = instInfo.result[0];
                        log(`     inst_info: ${geo.out.id}`, 'blue');
                        log(`       meshed: ${info.meshed || false}`, 'blue');
                        log(`       aabb: ${info.aabb ? '存在' : '不存在'}`, 'blue');
                        log(`       param: ${info.param ? '存在' : '不存在'}`, 'blue');
                        
                        // 6. 检查实际的mesh文件
                        if (info.meshed) {
                            const meshId = geo.out.id.toString().replace('inst_geo:<', '').replace('>', '');
                            log(`       预期mesh文件: ${meshId}.mesh`, 'yellow');
                        }
                    }
                }
            }
            
            // 7. 检查布尔运算组
            const booleanGroupSql = `SELECT * FROM cata_neg_boolean_group WHERE refno = ${refnoStr}`;
            log(`执行查询: ${booleanGroupSql}`, 'yellow');
            const booleanGroups = await queryDatabase(booleanGroupSql);
            
            if (booleanGroups.result && booleanGroups.result.length > 0) {
                const group = booleanGroups.result[0];
                log(`✅ 找到布尔运算组: ${group.id}`, 'green');
                
                if (group.boolean_group && Array.isArray(group.boolean_group)) {
                    log(`   布尔组数量: ${group.boolean_group.length}`, 'blue');
                    
                    for (let i = 0; i < group.boolean_group.length; i++) {
                        const bg = group.boolean_group[i];
                        if (Array.isArray(bg)) {
                            log(`   组${i}: [${bg.join(', ')}]`, 'blue');
                        }
                    }
                }
            } else {
                log(`ℹ️  refno ${refnoStr} 没有布尔运算组记录`, 'yellow');
            }
            
        } catch (error) {
            log(`❌ 查询 refno ${refnoStr} 时发生错误: ${error.message}`, 'red');
        }
    }
    
    // 8. 统计分析
    log('\n=== 统计分析 ===', 'blue');
    
    try {
        // 检查所有14207开头的refno
        const all14207Sql = "SELECT refno, noun, COUNT() as count FROM pe WHERE refno LIKE '14207_%' GROUP BY refno, noun ORDER BY refno";
        log(`执行查询: ${all14207Sql}`, 'yellow');
        const all14207 = await queryDatabase(all14207Sql);
        
        if (all14207.result) {
            log('所有14207开头的refno统计:', 'cyan');
            for (const record of all14207.result) {
                log(`  ${record.refno}: ${record.noun || 'N/A'} (${record.count}条记录)`, 'blue');
            }
        }
        
        // 9. 检查mesh文件目录
        log('\n=== 检查mesh文件目录 ===', 'blue');
        const fs = require('fs');
        if (fs.existsSync('assets/meshes')) {
            const files = fs.readdirSync('assets/meshes');
            const meshFiles = files.filter(file => file.endsWith('.mesh') && file.startsWith('14207_'));
            
            log(`找到 ${meshFiles.length} 个14207开头的mesh文件:`, 'cyan');
            for (const file of meshFiles) {
                log(`  ${file}`, 'blue');
            }
            
            // 检查问题refno对应的mesh文件是否存在
            log('\n检查问题refno的mesh文件:', 'cyan');
            for (const refnoStr of problemRefnos) {
                const meshFile = `${refnoStr}.mesh`;
                const exists = meshFiles.includes(meshFile);
                log(`  ${meshFile}: ${exists ? '✅ 存在' : '❌ 不存在'}`, exists ? 'green' : 'red');
            }
        } else {
            log('❌ 无法读取assets/meshes目录', 'red');
        }
        
    } catch (error) {
        log(`❌ 统计分析时发生错误: ${error.message}`, 'red');
    }
    
    log('\n=== 调试完成 ===', 'green');
}

// 运行调试
debugManifoldIssue().catch(console.error);
