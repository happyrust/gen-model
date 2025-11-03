// 检查参考号 21491/19209 的数据
import { Surreal } from 'surrealdb';

async function checkRefno() {
    const db = new Surreal();
    
    try {
        await db.connect('ws://127.0.0.1:8009');
        await db.use({ namespace: 'YCYK', database: 'E3D' });
        
        const refno = '21491_19209';
        console.log(`\n🔍 检查参考号: ${refno}\n`);
        
        // 1. 检查 pdms_eles 表
        console.log('1️⃣ 检查 pdms_eles 表:');
        const eles = await db.query(`SELECT * FROM pdms_eles:⟨${refno}⟩`);
        if (eles && eles[0] && eles[0].length > 0) {
            console.log('   ✅ 找到元素:', JSON.stringify(eles[0][0], null, 2));
        } else {
            console.log('   ❌ 未找到元素');
        }
        
        // 2. 检查 inst_relate 表
        console.log('\n2️⃣ 检查 inst_relate 表:');
        const instRelate = await db.query(`SELECT * FROM inst_relate WHERE refno = '${refno}'`);
        if (instRelate && instRelate[0] && instRelate[0].length > 0) {
            console.log('   ✅ 找到 inst_relate 记录:', instRelate[0].length, '条');
            instRelate[0].forEach((record, idx) => {
                console.log(`   [${idx}] geo_hash: ${record.geo_hash}, geo_type: ${record.geo_type}`);
            });
        } else {
            console.log('   ❌ 未找到 inst_relate 记录');
        }
        
        // 3. 检查 inst_geo 表
        console.log('\n3️⃣ 检查 inst_geo 表:');
        if (instRelate && instRelate[0] && instRelate[0].length > 0) {
            for (const record of instRelate[0]) {
                if (record.geo_hash) {
                    const geoId = `inst_geo:⟨${record.geo_hash}⟩`;
                    const instGeo = await db.query(`SELECT * FROM ${geoId}`);
                    if (instGeo && instGeo[0] && instGeo[0].length > 0) {
                        const geo = instGeo[0][0];
                        console.log(`   ✅ geo_hash: ${record.geo_hash}`);
                        console.log(`      - param: ${geo.param ? 'exists' : 'null'}`);
                        console.log(`      - aabb: ${geo.aabb ? 'exists' : 'null'}`);
                        console.log(`      - meshed: ${geo.meshed || false}`);
                        console.log(`      - bad: ${geo.bad || false}`);
                    } else {
                        console.log(`   ❌ geo_hash: ${record.geo_hash} - 未找到 inst_geo 记录`);
                    }
                }
            }
        }
        
        // 4. 查询子节点
        console.log('\n4️⃣ 查询子节点:');
        const children = await db.query(`
            SELECT id, refno, noun, geo_type 
            FROM pdms_eles 
            WHERE owner = pdms_eles:⟨${refno}⟩ 
            LIMIT 10
        `);
        if (children && children[0] && children[0].length > 0) {
            console.log(`   ✅ 找到 ${children[0].length} 个子节点:`);
            children[0].forEach((child, idx) => {
                console.log(`   [${idx}] refno: ${child.refno}, noun: ${child.noun}, geo_type: ${child.geo_type}`);
            });
        } else {
            console.log('   ❌ 未找到子节点');
        }
        
        // 5. 查询父节点
        console.log('\n5️⃣ 查询父节点:');
        if (eles && eles[0] && eles[0].length > 0 && eles[0][0].owner) {
            const owner = eles[0][0].owner;
            const ownerData = await db.query(`SELECT * FROM ${owner}`);
            if (ownerData && ownerData[0] && ownerData[0].length > 0) {
                console.log('   ✅ 父节点:', JSON.stringify(ownerData[0][0], null, 2));
            }
        } else {
            console.log('   ❌ 无父节点');
        }
        
    } catch (error) {
        console.error('❌ 错误:', error);
    } finally {
        await db.close();
    }
}

checkRefno();

