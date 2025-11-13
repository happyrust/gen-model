#!/usr/bin/env node

// 专门分析 refno 14207_545 布尔运算失败的原因

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

async function analyzeRefno14207_545() {
    const targetRefno = '14207_545';
    
    log('=== 深入分析 refno 14207_545 布尔运算失败原因 ===', 'blue');
    log(`目标refno: ${targetRefno}`, 'cyan');
    
    // 检查所有可能的数据库配置
    const dbConfigs = [
        { ns: '1516', db: '1112', name: '配置1 (从run_model_gen_test.sh)' },
        { ns: '1500', db: 'SLYK', name: '配置2 (从DbOption.toml)' },
        { ns: '1500', db: '1112', name: '配置3 (混合)' },
        { ns: '1516', db: 'SLYK', name: '配置4 (混合)' },
    ];
    
    for (const config of dbConfigs) {
        log(`\n--- 检查数据库配置: ${config.name} ---`, 'cyan');
        log(`命名空间: ${config.ns}, 数据库: ${config.db}`, 'blue');
        
        try {
            // 步骤1: 检查基础数据
            log('\n步骤1: 检查pe表中的基础数据', 'yellow');
            const peSql = `SELECT * FROM pe WHERE refno = ${targetRefno}`;
            const peResult = await querySurrealDirect(8009, peSql, config.ns, config.db);
            
            if (peResult.result && peResult.result.length > 0) {
                const peData = peResult.result[0];
                log(`✅ 找到pe记录: ${peData.id}`, 'green');
                log(`  - noun: ${peData.noun || 'N/A'}`, 'blue');
                log(`  - type: ${peData.type || 'N/A'}`, 'blue');
                log(`  - dbno: ${peData.dbno || 'N/A'}`, 'blue');
                
                // 步骤2: 检查inst_relate关系
                log('\n步骤2: 检查inst_relate关系', 'yellow');
                const instSql = `SELECT * FROM ${peData.id}->inst_relate`;
                const instResult = await querySurrealDirect(8009, instSql, config.ns, config.db);
                
                if (instResult.result && instResult.result.length > 0) {
                    log(`✅ 找到 ${instResult.result.length} 个inst_relate记录`, 'green');
                    
                    for (let i = 0; i < instResult.result.length; i++) {
                        const instData = instResult.result[i];
                        log(`  inst_relate[${i}]: ${instData.id}`, 'blue');
                        log(`    - bad_bool: ${instData.bad_bool || false}`, 'blue');
                        log(`    - booled: ${instData.booled || false}`, 'blue');
                        
                        // 步骤3: 检查geo_relate关系
                        log(`\n步骤3: 检查inst_relate[${i}]的geo_relate关系`, 'yellow');
                        const geoSql = `SELECT * FROM ${instData.id}->geo_relate`;
                        const geoResult = await querySurrealDirect(8009, geoSql, config.ns, config.db);
                        
                        if (geoResult.result && geoResult.result.length > 0) {
                            log(`✅ 找到 ${geoResult.result.length} 个geo_relate记录`, 'green');
                            
                            for (let j = 0; j < geoResult.result.length; j++) {
                                const geoData = geoResult.result[j];
                                log(`  geo_relate[${j}]: ${geoData.id}`, 'blue');
                                log(`    - geom_refno: ${geoData.geom_refno || 'N/A'}`, 'blue');
                                log(`    - geo_type: ${geoData.geo_type || 'N/A'}`, 'blue');
                                log(`    - visible: ${geoData.visible || false}`, 'blue');
                                log(`    - bad: ${geoData.bad || false}`, 'blue');
                                
                                // 步骤4: 检查inst_info几何数据
                                if (geoData.out && geoData.out.id) {
                                    log(`\n步骤4: 检查几何数据inst_info`, 'yellow');
                                    const infoSql = `SELECT * FROM ${geoData.out.id}`;
                                    const infoResult = await querySurrealDirect(8009, infoSql, config.ns, config.db);
                                    
                                    if (infoResult.result && infoResult.result.length > 0) {
                                        const infoData = infoResult.result[0];
                                        log(`✅ 找到inst_info: ${geoData.out.id}`, 'green');
                                        log(`    - meshed: ${infoData.meshed || false}`, 'blue');
                                        log(`    - aabb: ${infoData.aabb ? '存在' : '不存在'}`, 'blue');
                                        log(`    - param: ${infoData.param ? '存在' : '不存在'}`, 'blue');
                                        
                                        // 步骤5: 检查对应的mesh文件
                                        if (infoData.meshed) {
                                            const meshId = geoData.out.id.toString().replace('inst_geo:<', '').replace('>', '');
                                            log(`    - 预期mesh文件: ${meshId}.mesh`, 'blue');
                                            
                                            // 检查文件是否存在
                                            const fs = require('fs');
                                            const meshPath = `assets/meshes/${meshId}.mesh`;
                                            if (fs.existsSync(meshPath)) {
                                                log(`    - mesh文件: ✅ 存在`, 'green');
                                            } else {
                                                log(`    - mesh文件: ❌ 不存在`, 'red');
                                            }
                                        }
                                    } else {
                                        log(`❌ 未找到inst_info: ${geoData.out.id}`, 'red');
                                    }
                                }
                            }
                        } else {
                            log(`❌ 未找到geo_relate记录`, 'red');
                        }
                        
                        // 步骤6: 检查布尔运算组
                        log(`\n步骤6: 检查布尔运算组`, 'yellow');
                        const boolSql = `SELECT * FROM cata_neg_boolean_group WHERE refno = ${targetRefno}`;
                        const boolResult = await querySurrealDirect(8009, boolSql, config.ns, config.db);
                        
                        if (boolResult.result && boolResult.result.length > 0) {
                            log(`✅ 找到布尔运算组: ${boolResult.result[0].id}`, 'green');
                            const boolGroup = boolResult.result[0];
                            if (boolGroup.boolean_group) {
                                log(`  布尔组数量: ${boolGroup.boolean_group.length}`, 'blue');
                                for (let k = 0; k < boolGroup.boolean_group.length; k++) {
                                    const group = boolGroup.boolean_group[k];
                                    log(`    组${k}: [${group.join(', ')}]`, 'blue');
                                }
                            }
                        } else {
                            log(`❌ 未找到布尔运算组`, 'red');
                        }
                    }
                } else {
                    log(`❌ 未找到inst_relate记录`, 'red');
                }
            } else {
                log(`❌ 未找到pe记录`, 'red');
            }
            
        } catch (error) {
            log(`❌ 查询错误: ${error.message}`, 'red');
        }
    }
    
    // 总结分析
    log('\n=== 失败原因总结 ===', 'blue');
    log('基于以上分析，布尔运算失败的可能原因:', 'yellow');
    log('1. 数据库中不存在refno 14207_545的基础数据', 'red');
    log('2. 缺少对应的inst_relate关系', 'red');
    log('3. 缺少geo_relate几何关系', 'red');
    log('4. 几何数据未正确生成或导入', 'red');
    log('5. mesh文件缺失', 'red');
    log('6. 布尔运算组配置错误', 'red');
    
    log('\n=== 解决建议 ===', 'blue');
    log('1. 确认数据源是否包含14207_545', 'yellow');
    log('2. 重新运行数据导入流程', 'yellow');
    log('3. 检查模型生成配置', 'yellow');
    log('4. 验证mesh文件生成', 'yellow');
    
    log('\n=== 分析完成 ===', 'green');
}

analyzeRefno14207_545().catch(console.error);
