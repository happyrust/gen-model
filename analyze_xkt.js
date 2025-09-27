#!/usr/bin/env node

const fs = require('fs');
const zlib = require('zlib');

function analyzeXKT(filename) {
    console.log(`🔍 分析 XKT 文件: ${filename}`);
    console.log('=' .repeat(50));

    const buffer = fs.readFileSync(filename);
    console.log(`📁 文件大小: ${buffer.length} 字节`);

    if (buffer.length < 4) {
        console.log('❌ 文件太小');
        return;
    }

    // 读取版本号
    const version = buffer.readUInt32LE(0);
    console.log(`🔢 XKT 版本: ${version}`);

    if (buffer.length < 84) { // 4 + 20*4
        console.log('❌ 文件结构不完整');
        return;
    }

    // 读取 section 偏移表
    const sectionCount = 20;
    const offsets = [];

    for (let i = 0; i < sectionCount; i++) {
        const offset = buffer.readUInt32LE(4 + i * 4);
        offsets.push(offset);
    }

    console.log('\n📊 Section 偏移表:');
    for (let i = 0; i < sectionCount; i++) {
        const offset = offsets[i];
        const nextOffset = i < sectionCount - 1 ? offsets[i + 1] : buffer.length;
        const size = nextOffset - offset;
        console.log(`  Section ${i.toString().padStart(2)}: 偏移=${offset.toString().padStart(3)}, 大小=${size.toString().padStart(3)} 字节`);
    }

    console.log('\n🔍 Section 内容分析:');

    // 分析各个 section
    const sectionNames = [
        'Metadata (JSON)',          // 0
        'Positions',               // 1
        'Normals',                 // 2
        'Colors',                  // 3
        'Indices',                 // 4
        'Edge Indices',            // 5
        'Matrices',                // 6
        'Reused Geometries',       // 7
        'Geometries',              // 8
        'Geometry Instances',      // 9
        'Entities',                // 10
        'Tiles',                   // 11
        'Property Sets',           // 12
        'Meta Objects',            // 13
        'Reserved 14',             // 14
        'Reserved 15',             // 15
        'Reserved 16',             // 16
        'Reserved 17',             // 17
        'Reserved 18',             // 18
        'Reserved 19'              // 19
    ];

    for (let i = 0; i < sectionCount; i++) {
        const offset = offsets[i];
        const nextOffset = i < sectionCount - 1 ? offsets[i + 1] : buffer.length;
        const size = nextOffset - offset;
        const sectionName = sectionNames[i] || `Unknown ${i}`;

        if (size > 0) {
            console.log(`\n  📦 Section ${i} (${sectionName}):`);
            console.log(`     偏移: ${offset}, 大小: ${size} 字节`);

            const sectionData = buffer.slice(offset, nextOffset);

            // 尝试解压缩（如果是压缩数据）
            try {
                const decompressed = zlib.inflateSync(sectionData);
                console.log(`     ✅ 解压缩成功，原始大小: ${decompressed.length} 字节`);

                // 如果是 JSON 数据（通常是 metadata 或 entities）
                if (i === 0 || i === 10 || i === 12 || i === 13) {
                    try {
                        const jsonStr = decompressed.toString('utf8');
                        const jsonData = JSON.parse(jsonStr);
                        console.log(`     📄 JSON 内容:`, JSON.stringify(jsonData, null, 2).slice(0, 200) + '...');
                    } catch (e) {
                        console.log(`     📄 非 JSON 格式或解析失败`);
                    }
                }

                // 分析几何数据
                if (i === 1 && decompressed.length > 0) { // Positions
                    const vertexCount = decompressed.length / 6; // 每个顶点 3*2 bytes (i16)
                    console.log(`     🔺 顶点数量: ${Math.floor(vertexCount)}`);
                }

                if (i === 4 && decompressed.length > 0) { // Indices
                    const indexCount = decompressed.length / 4; // 每个索引 4 bytes (u32)
                    const triangleCount = indexCount / 3;
                    console.log(`     🔺 索引数量: ${Math.floor(indexCount)}, 三角形: ${Math.floor(triangleCount)}`);
                }

                if (i === 8 && decompressed.length > 0) { // Geometries
                    const geometryCount = decompressed.readUInt32LE(0);
                    console.log(`     📐 几何体数量: ${geometryCount}`);
                }

                if (i === 9 && decompressed.length > 0) { // Geometry Instances
                    const instanceCount = decompressed.readUInt32LE(0);
                    console.log(`     📋 实例数量: ${instanceCount}`);
                }

            } catch (e) {
                console.log(`     ❌ 解压缩失败，可能是未压缩数据`);

                // 显示原始数据的前几个字节
                const preview = Array.from(sectionData.slice(0, Math.min(16, sectionData.length)))
                    .map(b => b.toString(16).padStart(2, '0')).join(' ');
                console.log(`     📄 原始数据 (前16字节): ${preview}`);
            }
        } else {
            console.log(`  📦 Section ${i} (${sectionName}): 空`);
        }
    }

    console.log('\n📈 总结:');
    const nonEmptySections = offsets.filter((offset, i) => {
        const nextOffset = i < sectionCount - 1 ? offsets[i + 1] : buffer.length;
        return nextOffset - offset > 0;
    }).length;

    console.log(`  - 非空 sections: ${nonEmptySections}/${sectionCount}`);
    console.log(`  - 总文件大小: ${buffer.length} 字节`);
    console.log(`  - 数据密度: ${((buffer.length - 84) / buffer.length * 100).toFixed(1)}% (除去头部)`);
}

// 分析最新的文件
const latestFile = 'output/web_ui/db1112_compressed_20250926063844.xkt';

if (fs.existsSync(latestFile)) {
    analyzeXKT(latestFile);
} else {
    console.log('❌ 文件不存在:', latestFile);

    // 查找其他 XKT 文件
    const files = fs.readdirSync('output/web_ui/')
        .filter(f => f.endsWith('.xkt'))
        .sort()
        .reverse();

    if (files.length > 0) {
        console.log('📁 找到其他 XKT 文件:');
        files.forEach(f => console.log(`  - ${f}`));
        console.log(`\n分析最新文件: ${files[0]}`);
        analyzeXKT(`output/web_ui/${files[0]}`);
    }
}