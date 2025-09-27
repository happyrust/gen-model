#!/usr/bin/env node

const http = require('http');

const colors = {
    reset: '\x1b[0m',
    red: '\x1b[31m',
    green: '\x1b[32m',
    blue: '\x1b[34m',
    yellow: '\x1b[33m',
    cyan: '\x1b[36m',
    magenta: '\x1b[35m',
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

async function analyzeGeometryData(dbnum = 1112, refno = null) {
    const scope = refno ? `参考号 ${refno}` : `整个数据库 ${dbnum}`;
    log(`🔍 开始分析 ${scope} 的几何体数据`, 'blue');

    try {
        // 1. 查询 pe 表中的记录数
        log('📊 查询 pe 表记录数...', 'cyan');
        const peQuery = refno
            ? `SELECT COUNT() FROM pe WHERE dbno = ${dbnum} AND refno = "${refno}" GROUP ALL`
            : `SELECT COUNT() FROM pe WHERE dbno = ${dbnum} GROUP ALL`;
        const peCount = await queryDatabase(peQuery);
        console.log('PE 记录数:', JSON.stringify(peCount, null, 2));

        // 2. 查询有几何体类型的 pe 记录
        log('📊 查询几何体类型的 pe 记录...', 'cyan');
        const geoTypes = ['GENSEC', 'CYSEC', 'NZSEC', 'DISEC', 'CYLI', 'BOX', 'SNOUT', 'DISH', 'CTORUS', 'RTORUS', 'PYRAMID'];
        const geoTypeQuery = refno
            ? `SELECT type, COUNT() FROM pe WHERE dbno = ${dbnum} AND refno = "${refno}" AND type IN [${geoTypes.map(t => `"${t}"`).join(', ')}] GROUP type`
            : `SELECT type, COUNT() FROM pe WHERE dbno = ${dbnum} AND type IN [${geoTypes.map(t => `"${t}"`).join(', ')}] GROUP type`;
        const geoTypeCount = await queryDatabase(geoTypeQuery);
        console.log('几何体类型统计:', JSON.stringify(geoTypeCount, null, 2));

        // 3. 查询 inst_geos 表
        log('📊 查询 inst_geos 表记录数...', 'cyan');
        const instGeosQuery = refno
            ? `SELECT COUNT() FROM inst_geos WHERE dbno = ${dbnum} AND refno = "${refno}" GROUP ALL`
            : `SELECT COUNT() FROM inst_geos WHERE dbno = ${dbnum} GROUP ALL`;
        const instGeosCount = await queryDatabase(instGeosQuery);
        console.log('inst_geos 记录数:', JSON.stringify(instGeosCount, null, 2));

        // 4. 查询 meshes 表
        log('📊 查询 meshes 表记录数...', 'cyan');
        const meshesQuery = refno
            ? `SELECT COUNT() FROM meshes WHERE dbno = ${dbnum} AND refno = "${refno}" GROUP ALL`
            : `SELECT COUNT() FROM meshes WHERE dbno = ${dbnum} GROUP ALL`;
        const meshesCount = await queryDatabase(meshesQuery);
        console.log('meshes 记录数:', JSON.stringify(meshesCount, null, 2));

        // 5. 查询具体的几何体记录样本
        if (refno) {
            log(`📊 查询具体的 ${refno} 记录详情...`, 'cyan');
            const specificPe = await queryDatabase(`SELECT * FROM pe WHERE dbno = ${dbnum} AND refno = "${refno}" LIMIT 5`);
            console.log('具体 PE 记录:', JSON.stringify(specificPe, null, 2));

            const specificInstGeos = await queryDatabase(`SELECT * FROM inst_geos WHERE dbno = ${dbnum} AND refno = "${refno}" LIMIT 5`);
            console.log('具体 inst_geos 记录:', JSON.stringify(specificInstGeos, null, 2));

            const specificMeshes = await queryDatabase(`SELECT * FROM meshes WHERE dbno = ${dbnum} AND refno = "${refno}" LIMIT 5`);
            console.log('具体 meshes 记录:', JSON.stringify(specificMeshes, null, 2));
        } else {
            // 对于整个数据库，查询一些样本
            log('📊 查询样本记录...', 'cyan');
            const samplePe = await queryDatabase(`SELECT * FROM pe WHERE dbno = ${dbnum} AND type IN ["GENSEC", "CYSEC", "BOX", "CYLI"] LIMIT 10`);
            console.log('样本 PE 记录:', JSON.stringify(samplePe, null, 2));

            const sampleInstGeos = await queryDatabase(`SELECT * FROM inst_geos WHERE dbno = ${dbnum} LIMIT 5`);
            console.log('样本 inst_geos 记录:', JSON.stringify(sampleInstGeos, null, 2));

            const sampleMeshes = await queryDatabase(`SELECT * FROM meshes WHERE dbno = ${dbnum} LIMIT 5`);
            console.log('样本 meshes 记录:', JSON.stringify(sampleMeshes, null, 2));
        }

        // 6. 分析mesh的几何数据质量
        log('📊 分析 mesh 几何数据质量...', 'cyan');
        const meshDataQuery = refno
            ? `SELECT refno, vertices, indices, position, array::len(vertices) as vertex_count, array::len(indices) as index_count FROM meshes WHERE dbno = ${dbnum} AND refno = "${refno}" LIMIT 3`
            : `SELECT refno, vertices, indices, position, array::len(vertices) as vertex_count, array::len(indices) as index_count FROM meshes WHERE dbno = ${dbnum} AND vertices IS NOT NULL LIMIT 10`;

        const meshData = await queryDatabase(meshDataQuery);
        console.log('Mesh 几何数据分析:', JSON.stringify(meshData, null, 2));

        // 7. 检查是否有转换错误的数据
        log('📊 检查数据转换问题...', 'cyan');
        const problemQuery = refno
            ? `SELECT refno, type, "issue" FROM (
                (SELECT refno, type, "empty_vertices" as issue FROM meshes WHERE dbno = ${dbnum} AND refno = "${refno}" AND (vertices IS NULL OR array::len(vertices) = 0))
                UNION
                (SELECT refno, type, "empty_indices" as issue FROM meshes WHERE dbno = ${dbnum} AND refno = "${refno}" AND (indices IS NULL OR array::len(indices) = 0))
                UNION
                (SELECT refno, type, "missing_position" as issue FROM meshes WHERE dbno = ${dbnum} AND refno = "${refno}" AND position IS NULL)
            )`
            : `SELECT refno, type, "issue" FROM (
                (SELECT refno, type, "empty_vertices" as issue FROM meshes WHERE dbno = ${dbnum} AND (vertices IS NULL OR array::len(vertices) = 0) LIMIT 5)
                UNION
                (SELECT refno, type, "empty_indices" as issue FROM meshes WHERE dbno = ${dbnum} AND (indices IS NULL OR array::len(indices) = 0) LIMIT 5)
                UNION
                (SELECT refno, type, "missing_position" as issue FROM meshes WHERE dbno = ${dbnum} AND position IS NULL LIMIT 5)
            )`;

        const problems = await queryDatabase(problemQuery);
        console.log('数据问题分析:', JSON.stringify(problems, null, 2));

        // 8. 生成摘要报告
        console.log('\n' + '='.repeat(60));
        log('📊 几何数据分析摘要', 'blue');
        console.log('='.repeat(60));
        log(`🎯 分析范围: ${scope}`, 'cyan');

        // 提取数字进行汇总
        const peTotal = peCount[0]?.count || 0;
        const instGeosTotal = instGeosCount[0]?.count || 0;
        const meshesTotal = meshesCount[0]?.count || 0;

        log(`📊 数据量统计:`, 'cyan');
        log(`  - PE 记录: ${peTotal}`, 'cyan');
        log(`  - inst_geos 记录: ${instGeosTotal}`, 'cyan');
        log(`  - meshes 记录: ${meshesTotal}`, 'cyan');

        // 计算转换率
        if (peTotal > 0) {
            const geoConversionRate = ((instGeosTotal / peTotal) * 100).toFixed(1);
            const meshConversionRate = ((meshesTotal / peTotal) * 100).toFixed(1);

            log(`🔄 转换率分析:`, 'yellow');
            log(`  - PE → inst_geos: ${geoConversionRate}%`, 'yellow');
            log(`  - PE → meshes: ${meshConversionRate}%`, 'yellow');

            if (instGeosTotal === 0) {
                log(`❌ 问题: inst_geos 表为空，几何体未生成`, 'red');
            }
            if (meshesTotal === 0) {
                log(`❌ 问题: meshes 表为空，mesh 未生成`, 'red');
            }
        }

        console.log('='.repeat(60));
        log(`✅ 几何数据分析完成`, 'green');

    } catch (error) {
        log(`❌ 分析失败: ${error.message}`, 'red');
        console.error(error);
    }
}

// 解析命令行参数
function parseArgs() {
    const args = process.argv.slice(2);
    let dbnum = 1112;
    let refno = null;

    for (let i = 0; i < args.length; i++) {
        if (args[i] === '--dbnum' || args[i] === '-d') {
            dbnum = parseInt(args[i + 1]);
            i++;
        } else if (args[i] === '--refno' || args[i] === '-r') {
            refno = args[i + 1];
            i++;
        } else if (args[i] === '--help' || args[i] === '-h') {
            console.log(`
几何数据分析工具

用法: node query_geometry_data.js [选项]

选项:
  --dbnum, -d <数字>    指定数据库号 (默认: 1112)
  --refno, -r <refno>   指定参考号 (可选)
  --help, -h            显示帮助信息

示例:
  node query_geometry_data.js                     # 分析整个数据库 1112
  node query_geometry_data.js -r "17496/256215"   # 分析特定参考号
  node query_geometry_data.js -d 1113             # 分析数据库 1113
            `);
            process.exit(0);
        }
    }

    return { dbnum, refno };
}

// 主函数
async function main() {
    const { dbnum, refno } = parseArgs();
    await analyzeGeometryData(dbnum, refno);
}

if (require.main === module) {
    main().catch(console.error);
}

module.exports = { analyzeGeometryData };