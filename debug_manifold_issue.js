// 调试布尔运算失败问题的脚本
// 分析没有找到正实体 manifold 的 refno: 14207_545, 14207_856, 14207_858, 14207_1357, 14207_185

import { Surreal } from 'surrealdb';

async function debugManifoldIssue() {
    try {
        // 连接数据库
        const db = new Surreal();
        await db.connect('ws://127.0.0.1:8009');
        await db.use({ namespace: '1500', database: 'SLYK' });

        console.log('=== 调试布尔运算失败问题 ===');
        
        // 问题refno列表
        const problemRefnos = ['14207_545', '14207_856', '14207_858', '14207_1357', '14207_185'];
        
        for (const refnoStr of problemRefnos) {
            console.log(`\n--- 分析 refno: ${refnoStr} ---`);
            
            // 1. 检查这个refno的基本信息
            const basicInfo = await db.query(`
                SELECT * FROM pe WHERE refno = ${refnoStr}
            `);
            
            if (basicInfo[0].result.length === 0) {
                console.log(`❌ refno ${refnoStr} 在pe表中不存在`);
                continue;
            }
            
            const peData = basicInfo[0].result[0];
            console.log(`✅ 找到pe记录: ${peData.id}, noun: ${peData.noun || 'N/A'}`);
            
            // 2. 检查inst_relate记录
            const instRelateQuery = await db.query(`
                SELECT * FROM ${peData.id}->inst_relate
            `);
            
            if (instRelateQuery[0].result.length === 0) {
                console.log(`❌ refno ${refnoStr} 没有inst_relate记录`);
                continue;
            }
            
            const instRelate = instRelateQuery[0].result[0];
            console.log(`✅ 找到inst_relate记录: ${instRelate.id}`);
            console.log(`   bad_bool: ${instRelate.bad_bool || false}`);
            console.log(`   booled: ${instRelate.booled || false}`);
            
            // 3. 检查几何数据关系
            const geoRelateQuery = await db.query(`
                SELECT * FROM ${instRelate.id}->geo_relate
            `);
            
            if (geoRelateQuery[0].result.length === 0) {
                console.log(`❌ refno ${refnoStr} 没有geo_relate记录`);
                continue;
            }
            
            const geoRelates = geoRelateQuery[0].result;
            console.log(`✅ 找到 ${geoRelates.length} 个geo_relate记录`);
            
            // 4. 检查每个几何记录的详细信息
            for (const geo of geoRelates) {
                console.log(`   几何记录: ${geo.id}`);
                console.log(`     geom_refno: ${geo.geom_refno || 'N/A'}`);
                console.log(`     geo_type: ${geo.geo_type || 'N/A'}`);
                console.log(`     visible: ${geo.visible || false}`);
                console.log(`     bad: ${geo.bad || false}`);
                
                // 5. 检查inst_info和几何数据
                if (geo.out) {
                    const instInfoQuery = await db.query(`
                        SELECT * FROM ${geo.out.id}
                    `);
                    
                    if (instInfoQuery[0].result.length > 0) {
                        const instInfo = instInfoQuery[0].result[0];
                        console.log(`     inst_info: ${instInfo.id}`);
                        console.log(`       meshed: ${instInfo.meshed || false}`);
                        console.log(`       aabb: ${instInfo.aabb ? '存在' : '不存在'}`);
                        console.log(`       param: ${instInfo.param ? '存在' : '不存在'}`);
                        
                        // 6. 检查实际的mesh文件是否存在
                        if (instInfo.meshed) {
                            const meshId = instInfo.id.toString().replace(':', '');
                            console.log(`       预期mesh文件: ${meshId}.mesh`);
                        }
                    }
                }
            }
            
            // 7. 检查布尔运算组
            const booleanGroupQuery = await db.query(`
                SELECT * FROM cata_neg_boolean_group WHERE refno = ${refnoStr}
            `);
            
            if (booleanGroupQuery[0].result.length > 0) {
                const booleanGroup = booleanGroupQuery[0].result[0];
                console.log(`✅ 找到布尔运算组: ${booleanGroup.id}`);
                console.log(`   布尔组数量: ${booleanGroup.boolean_group ? booleanGroup.boolean_group.length : 0}`);
                
                if (booleanGroup.boolean_group) {
                    for (let i = 0; i < booleanGroup.boolean_group.length; i++) {
                        const group = booleanGroup.boolean_group[i];
                        console.log(`   组${i}: [${group.join(', ')}]`);
                    }
                }
            } else {
                console.log(`ℹ️  refno ${refnoStr} 没有布尔运算组记录`);
            }
        }
        
        // 8. 统计分析
        console.log('\n=== 统计分析 ===');
        
        // 检查所有14207开头的refno
        const all14207Query = await db.query(`
            SELECT refno, noun, COUNT(*) as count 
            FROM pe 
            WHERE refno LIKE '14207_%' 
            GROUP BY refno, noun
            ORDER BY refno
        `);
        
        console.log('所有14207开头的refno统计:');
        for (const record of all14207Query[0].result) {
            console.log(`  ${record.refno}: ${record.noun || 'N/A'} (${record.count}条记录)`);
        }
        
        await db.close();
        
    } catch (error) {
        console.error('调试过程中发生错误:', error);
    }
}

// 运行调试
debugManifoldIssue();
