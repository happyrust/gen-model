#!/usr/bin/env node

const fs = require('fs');

function analyzeOldXKT(filename) {
    console.log(`🔍 分析旧格式 XKT 文件: ${filename}`);
    console.log('=' .repeat(60));

    const buffer = fs.readFileSync(filename);
    console.log(`📁 文件大小: ${buffer.length} 字节`);

    // 旧格式：XKT\0 + version + data
    const magic = buffer.toString('ascii', 0, 4);
    console.log(`🔮 Magic bytes: "${magic}"`);

    if (magic === 'XKT\0') {
        const version = buffer.readUInt32LE(4);
        console.log(`🔢 XKT 版本: ${version}`);

        // 尝试找到 JSON 元数据（通常在文件开始的某个位置）
        let jsonStart = -1;
        let jsonEnd = -1;

        // 查找第一个 '{' 和最后一个 '}'
        for (let i = 8; i < Math.min(buffer.length, 1000); i++) {
            if (buffer[i] === 0x7B && jsonStart === -1) { // '{'
                jsonStart = i;
                break;
            }
        }

        if (jsonStart !== -1) {
            // 找到对应的 '}'
            let braceCount = 0;
            for (let i = jsonStart; i < Math.min(buffer.length, jsonStart + 2000); i++) {
                if (buffer[i] === 0x7B) braceCount++; // '{'
                if (buffer[i] === 0x7D) braceCount--; // '}'
                if (braceCount === 0) {
                    jsonEnd = i;
                    break;
                }
            }

            if (jsonEnd !== -1) {
                try {
                    const jsonStr = buffer.toString('utf8', jsonStart, jsonEnd + 1);
                    const metadata = JSON.parse(jsonStr);

                    console.log('\n📄 元数据分析:');
                    console.log(`  🏗️  几何体数量: ${metadata.geometries ? metadata.geometries.length : 'N/A'}`);
                    console.log(`  📦 网格数量: ${metadata.meshes ? metadata.meshes.length : 'N/A'}`);
                    console.log(`  🏢 实体数量: ${metadata.entities ? metadata.entities.length : 'N/A'}`);
                    console.log(`  🎨 材质数量: ${metadata.materials ? metadata.materials.length : 'N/A'}`);

                    if (metadata.entities && metadata.entities.length > 0) {
                        console.log('\n🔍 前5个实体:');
                        metadata.entities.slice(0, 5).forEach((entity, i) => {
                            console.log(`  ${i + 1}. ID: ${entity.id || 'N/A'}, Type: ${entity.type || 'N/A'}`);
                        });
                    }

                    if (metadata.geometries && metadata.geometries.length > 0) {
                        console.log('\n📐 几何体统计:');
                        let totalVertices = 0;
                        let totalIndices = 0;

                        metadata.geometries.forEach(geo => {
                            if (geo.positions) totalVertices += geo.positions.length / 3;
                            if (geo.indices) totalIndices += geo.indices.length;
                        });

                        console.log(`  🔺 总顶点数: ${totalVertices}`);
                        console.log(`  🔗 总索引数: ${totalIndices}`);
                        console.log(`  🔺 估计三角形数: ${Math.floor(totalIndices / 3)}`);
                    }

                    console.log('\n📈 文件密度分析:');
                    const jsonSize = jsonEnd - jsonStart + 1;
                    const dataSize = buffer.length - jsonSize - 8; // 减去头部
                    console.log(`  📄 JSON 大小: ${jsonSize} 字节 (${((jsonSize / buffer.length) * 100).toFixed(1)}%)`);
                    console.log(`  💾 二进制数据: ${dataSize} 字节 (${((dataSize / buffer.length) * 100).toFixed(1)}%)`);

                } catch (e) {
                    console.log('❌ JSON 解析失败:', e.message);
                }
            } else {
                console.log('❌ 找不到完整的 JSON 结构');
            }
        } else {
            console.log('❌ 找不到 JSON 元数据起始位置');
        }

    } else {
        console.log('❌ 不是有效的旧格式 XKT 文件');
    }

    console.log('\n' + '=' .repeat(60));
}

// 分析一个大文件
const filename = 'output/web_ui/db1112_compressed_20250926033141.xkt';
if (fs.existsSync(filename)) {
    analyzeOldXKT(filename);
} else {
    console.log('文件不存在:', filename);
}