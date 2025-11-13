// 简化的调试查询脚本 - 使用curl直接查询SurrealDB

const problemRefnos = ['14207_545', '14207_856', '14207_858', '14207_1357', '14207_185'];

async function querySurrealDB(sql) {
    const response = await fetch('http://127.0.0.1:8009/sql', {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Accept': 'application/json',
            'NS': '1500',
            'DB': 'SLYK'
        },
        body: JSON.stringify({ sql })
    });
    
    if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`);
    }
    
    return await response.json();
}

async function debugManifoldIssue() {
    console.log('=== 调试布尔运算失败问题 ===');
    
    for (const refnoStr of problemRefnos) {
        console.log(`\n--- 分析 refno: ${refnoStr} ---`);
        
        try {
            // 1. 检查这个refno的基本信息
            const basicInfoSql = `SELECT * FROM pe WHERE refno = ${refnoStr}`;
            const basicInfo = await querySurrealDB(basicInfoSql);
            
            if (basicInfo.length === 0) {
                console.log(`❌ refno ${refnoStr} 在pe表中不存在`);
                continue;
            }
            
            const peData = basicInfo[0];
            console.log(`✅ 找到pe记录: ${peData.id}, noun: ${peData.noun || 'N/A'}`);
            
            // 2. 检查inst_relate记录
            const instRelateSql = `SELECT * FROM ${peData.id}->inst_relate`;
            const instRelate = await querySurrealDB(instRelateSql);
            
            if (instRelate.length === 0) {
                console.log(`❌ refno ${refnoStr} 没有inst_relate记录`);
                continue;
            }
            
            const inst = instRelate[0];
            console.log(`✅ 找到inst_relate记录: ${inst.id}`);
            console.log(`   bad_bool: ${inst.bad_bool || false}`);
            console.log(`   booled: ${inst.booled || false}`);
            
            // 3. 检查几何数据关系
            const geoRelateSql = `SELECT * FROM ${inst.id}->geo_relate`;
            const geoRelates = await querySurrealDB(geoRelateSql);
            
            if (geoRelates.length === 0) {
                console.log(`❌ refno ${refnoStr} 没有geo_relate记录`);
                continue;
            }
            
            console.log(`✅ 找到 ${geoRelates.length} 个geo_relate记录`);
            
            // 4. 检查每个几何记录的详细信息
            for (let idx = 0; idx < geoRelates.length; idx++) {
                const geo = geoRelates[idx];
                console.log(`   几何记录[${idx}]: ${geo.id}`);
                console.log(`     geom_refno: ${geo.geom_refno || 'N/A'}`);
                console.log(`     geo_type: ${geo.geo_type || 'N/A'}`);
                console.log(`     visible: ${geo.visible || false}`);
                console.log(`     bad: ${geo.bad || false}`);
                
                // 5. 检查inst_info和几何数据
                if (geo.out && geo.out.id) {
                    const instInfoSql = `SELECT * FROM ${geo.out.id}`;
                    const instInfo = await querySurrealDB(instInfoSql);
                    
                    if (instInfo.length > 0) {
                        const info = instInfo[0];
                        console.log(`     inst_info: ${geo.out.id}`);
                        console.log(`       meshed: ${info.meshed || false}`);
                        console.log(`       aabb: ${info.aabb ? '存在' : '不存在'}`);
                        console.log(`       param: ${info.param ? '存在' : '不存在'}`);
                        
                        // 6. 检查实际的mesh文件
                        if (info.meshed) {
                            const meshId = geo.out.id.toString().replace('inst_geo:<', '').replace('>', '');
                            console.log(`       预期mesh文件: ${meshId}.mesh`);
                        }
                    }
                }
            }
            
            // 7. 检查布尔运算组
            const booleanGroupSql = `SELECT * FROM cata_neg_boolean_group WHERE refno = ${refnoStr}`;
            const booleanGroups = await querySurrealDB(booleanGroupSql);
            
            if (booleanGroups.length > 0) {
                const group = booleanGroups[0];
                console.log(`✅ 找到布尔运算组: ${group.id}`);
                
                if (group.boolean_group && Array.isArray(group.boolean_group)) {
                    console.log(`   布尔组数量: ${group.boolean_group.length}`);
                    
                    for (let i = 0; i < group.boolean_group.length; i++) {
                        const bg = group.boolean_group[i];
                        if (Array.isArray(bg)) {
                            console.log(`   组${i}: [${bg.join(', ')}]`);
                        }
                    }
                }
            } else {
                console.log(`ℹ️  refno ${refnoStr} 没有布尔运算组记录`);
            }
            
        } catch (error) {
            console.error(`❌ 查询 refno ${refnoStr} 时发生错误:`, error.message);
        }
    }
    
    // 8. 统计分析
    console.log('\n=== 统计分析 ===');
    
    try {
        // 检查所有14207开头的refno
        const all14207Sql = "SELECT refno, noun, COUNT() as count FROM pe WHERE refno LIKE '14207_%' GROUP BY refno, noun ORDER BY refno";
        const all14207 = await querySurrealDB(all14207Sql);
        
        console.log('所有14207开头的refno统计:');
        for (const record of all14207) {
            console.log(`  ${record.refno}: ${record.noun || 'N/A'} (${record.count}条记录)`);
        }
    } catch (error) {
        console.error('❌ 统计分析时发生错误:', error.message);
    }
}

// 运行调试
debugManifoldIssue().catch(console.error);
