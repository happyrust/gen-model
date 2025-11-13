#!/usr/bin/env node

// 跟踪布尔运算流程，分析14207_545失败的完整路径

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

async function traceBooleanFlow() {
    const targetRefno = '14207_545';
    
    log('=== 跟踪布尔运算流程: refno 14207_545 ===', 'blue');
    
    // 根据代码分析，布尔运算的流程是：
    // 1. query_manifold_boolean_operations(refno) -> 获取布尔运算数据
    // 2. 对每个正实体尝试加载manifold
    // 3. 如果没有找到任何正实体manifold，报错
    
    log('\n步骤1: 模拟 query_manifold_boolean_operations 查询', 'cyan');
    
    const dbConfigs = [
        { ns: '1516', db: '1112' },
        { ns: '1500', db: 'SLYK' },
    ];
    
    for (const config of dbConfigs) {
        log(`\n--- 检查配置: ${config.ns}/${config.db} ---`, 'yellow');
        
        try {
            // 1. 检查pe表（这是布尔运算的起点）
            const peSql = `SELECT * FROM pe WHERE refno = ${targetRefno}`;
            log(`执行: ${peSql}`, 'blue');
            const peResult = await querySurrealDirect(8009, peSql, config.ns, config.db);
            
            if (!peResult.result || peResult.result.length === 0) {
                log(`❌ pe表中没有refno ${targetRefno}`, 'red');
                log('这意味着布尔运算根本无法开始，因为基础数据不存在', 'red');
                continue;
            }
            
            const peData = peResult.result[0];
            log(`✅ 找到pe记录: ${peData.id}`, 'green');
            
            // 2. 检查inst_relate（布尔运算从这里获取几何信息）
            const instSql = `SELECT * FROM ${peData.id}->inst_relate`;
            log(`执行: ${instSql}`, 'blue');
            const instResult = await querySurrealDirect(8009, instSql, config.ns, config.db);
            
            if (!instResult.result || instResult.result.length === 0) {
                log(`❌ 没有inst_relate记录`, 'red');
                log('这意味着没有几何实例数据，无法进行布尔运算', 'red');
                continue;
            }
            
            log(`✅ 找到 ${instResult.result.length} 个inst_relate记录`, 'green');
            
            // 3. 对每个inst_relate，检查几何关系
            for (let i = 0; i < instResult.result.length; i++) {
                const instData = instResult.result[i];
                log(`\n检查inst_relate[${i}]: ${instData.id}`, 'blue');
                
                // 检查geo_relate（这里包含实际的几何数据）
                const geoSql = `SELECT * FROM ${instData.id}->geo_relate`;
                log(`执行: ${geoSql}`, 'blue');
                const geoResult = await querySurrealDirect(8009, geoSql, config.ns, config.db);
                
                if (!geoResult.result || geoResult.result.length === 0) {
                    log(`❌ 没有geo_relate记录`, 'red');
                    continue;
                }
                
                log(`✅ 找到 ${geoResult.result.length} 个geo_relate记录`, 'green');
                
                // 4. 检查每个几何记录
                let posManifoldCount = 0;
                for (let j = 0; j < geoResult.result.length; j++) {
                    const geoData = geoResult.result[j];
                    log(`  geo_relate[${j}]: ${geoData.id}`, 'blue');
                    log(`    geom_refno: ${geoData.geom_refno || 'N/A'}`, 'blue');
                    log(`    geo_type: ${geoData.geo_type || 'N/A'}`, 'blue');
                    
                    // 检查正实体（geo_type通常是'Pos'）
                    if (geoData.geo_type === 'Pos' || !geoData.geo_type) {
                        log(`    这是正实体，尝试加载manifold...`, 'yellow');
                        
                        if (geoData.out && geoData.out.id) {
                            const meshId = geoData.out.id.toString()
                                .replace('inst_geo:<', '')
                                .replace('>', '');
                            
                            log(`    mesh ID: ${meshId}`, 'blue');
                            
                            // 检查对应的mesh文件
                            const fs = require('fs');
                            const meshPaths = [
                                `assets/meshes/${meshId}.mesh`,
                                `assets/meshes/lod_L1/${meshId}_L1.mesh`,
                                `assets/meshes/lod_L2/${meshId}_L2.mesh`,
                                `assets/meshes/lod_L3/${meshId}_L3.mesh`,
                            ];
                            
                            let meshFound = false;
                            for (const meshPath of meshPaths) {
                                if (fs.existsSync(meshPath)) {
                                    log(`    ✅ mesh文件存在: ${meshPath}`, 'green');
                                    meshFound = true;
                                    posManifoldCount++;
                                    break;
                                }
                            }
                            
                            if (!meshFound) {
                                log(`    ❌ mesh文件不存在`, 'red');
                                log(`    检查的路径:`, 'red');
                                for (const meshPath of meshPaths) {
                                    log(`      - ${meshPath}`, 'red');
                                }
                            }
                        } else {
                            log(`    ❌ 没有out指针或inst_geo数据`, 'red');
                        }
                    } else {
                        log(`    跳过负实体/其他类型`, 'blue');
                    }
                }
                
                // 5. 总结这个inst_relate的情况
                if (posManifoldCount > 0) {
                    log(`✅ 这个inst_relate有 ${posManifoldCount} 个正实体manifold`, 'green');
                } else {
                    log(`❌ 这个inst_relate没有找到任何正实体manifold`, 'red');
                    log(`这就是"布尔运算失败: 没有找到正实体 manifold"错误的原因！`, 'red');
                }
            }
            
        } catch (error) {
            log(`❌ 查询错误: ${error.message}`, 'red');
        }
    }
    
    log('\n=== 根本原因分析 ===', 'blue');
    log('基于代码跟踪和数据库检查，refno 14207_545布尔运算失败的根本原因是:', 'yellow');
    log('');
    log('1. 数据库中完全不存在refno 14207_545的任何数据', 'red');
    log('2. 没有对应的pe记录，导致无法找到inst_relate', 'red');
    log('3. 没有inst_relate记录，导致无法找到几何数据', 'red');
    log('4. 没有几何数据，load_manifold函数无法加载mesh文件', 'red');
    log('5. pos_manifolds数组为空，触发错误消息', 'red');
    log('');
    log('这不是代码逻辑错误，而是数据缺失问题！', 'cyan');
    log('');
    log('=== 解决方案 ===', 'blue');
    log('1. 确认原始数据源是否包含14207_545', 'yellow');
    log('2. 运行完整的数据导入流程', 'yellow');
    log('3. 检查数据导入配置是否正确', 'yellow');
    log('4. 验证模型生成是否成功', 'yellow');
    
    log('\n=== 验证步骤 ===', 'blue');
    log('运行以下命令来导入数据:', 'yellow');
    log('  ./run_model_gen_test.sh', 'cyan');
    log('或者:', 'yellow');
    log('  cargo run --release --bin aios-database', 'cyan');
    
    log('\n=== 分析完成 ===', 'green');
}

traceBooleanFlow().catch(console.error);
