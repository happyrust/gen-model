#!/usr/bin/env node

import fs from 'fs';
import zlib from 'zlib';

console.log('🔍 验证数据库1112 ZONE的XKT文件');
console.log('='.repeat(60));

// 验证XKT文件详细信息
function validateXKTDetailed(filepath) {
    console.log(`\n📁 分析文件: ${filepath}`);
    console.log('-'.repeat(50));

    if (!fs.existsSync(filepath)) {
        console.log('❌ 文件不存在');
        return null;
    }

    const buffer = fs.readFileSync(filepath);
    const fileSize = buffer.length;
    const result = {
        filepath,
        fileSize,
        valid: false,
        version: 0,
        compressed: false,
        hasGeometry: false,
        hasEntities: false,
        bbox: null,
        stats: {}
    };

    console.log(`✅ 文件大小: ${fileSize} 字节`);

    // 解析版本
    const versionAndFlags = buffer.readUInt32LE(0);
    result.version = versionAndFlags & 0x7FFFFFFF;
    result.compressed = (versionAndFlags & 0x80000000) !== 0;

    console.log(`📌 版本: ${result.version}`);
    console.log(`🗜️  压缩: ${result.compressed ? '是' : '否'}`);

    if (result.version !== 10) {
        console.log(`❌ 不支持的版本: ${result.version}`);
        return result;
    }

    // 读取段信息
    const numSections = 29;
    const offsets = [];
    const sizes = [];

    for (let i = 0; i < numSections; i++) {
        offsets.push(buffer.readUInt32LE(4 + i * 4));
    }

    for (let i = 0; i < numSections - 1; i++) {
        sizes.push(offsets[i + 1] - offsets[i]);
    }
    sizes.push(buffer.length - offsets[numSections - 1]);

    // 分析关键段
    console.log('\n📊 段分析:');

    // 元数据 (段0)
    if (sizes[0] > 8) {
        try {
            const compressed = buffer.slice(offsets[0], offsets[0] + sizes[0]);
            const decompressed = zlib.inflateSync(compressed);
            const metadata = decompressed.toString('utf-8');
            const metaObj = JSON.parse(metadata);
            console.log(`  元数据: ${metaObj.id || 'N/A'}`);
            result.stats.metadata = metaObj;
        } catch (e) {
            console.log(`  元数据: 解析失败`);
        }
    }

    // 几何数据 (段8: 位置, 段9: 法线)
    if (sizes[8] > 0 && sizes[9] > 0) {
        result.hasGeometry = true;
        console.log(`  ✅ 几何数据: 位置=${sizes[8]}字节, 法线=${sizes[9]}字节`);
        result.stats.geometrySize = sizes[8] + sizes[9];
    } else {
        console.log(`  ❌ 无几何数据`);
    }

    // 索引数据 (段4)
    if (sizes[4] > 0) {
        console.log(`  ✅ 索引数据: ${sizes[4]}字节`);
        result.stats.indexSize = sizes[4];

        // 尝试解析索引数量
        try {
            const indexData = buffer.slice(offsets[4], offsets[4] + sizes[4]);
            const decompressed = zlib.inflateSync(indexData);
            const indexCount = decompressed.length / 2; // Uint16Array
            console.log(`    三角形数量: ${indexCount / 3}`);
            result.stats.triangles = Math.floor(indexCount / 3);
        } catch (e) {
            // 忽略解压错误
        }
    }

    // 实体数据 (段25)
    if (sizes[25] > 8) {
        try {
            const compressed = buffer.slice(offsets[25], offsets[25] + sizes[25]);
            const decompressed = zlib.inflateSync(compressed);
            const entities = decompressed.toString('utf-8');
            const entityList = JSON.parse(entities);
            result.hasEntities = entityList.length > 0;
            console.log(`  ✅ 实体数量: ${entityList.length}`);
            result.stats.entities = entityList.length;
            console.log(`    实体ID: ${entityList.join(', ')}`);
        } catch (e) {
            console.log(`  实体数据: 解析失败`);
        }
    }

    // 包围盒 (段27)
    if (offsets[27] === 820 && sizes[27] === 20) {
        try {
            const bboxCompressed = buffer.slice(820, 840);
            const decompressed = zlib.inflateSync(bboxCompressed);
            const bbox = new Float64Array(decompressed.buffer, decompressed.byteOffset, 6);

            result.bbox = {
                min: [bbox[0], bbox[1], bbox[2]],
                max: [bbox[3], bbox[4], bbox[5]]
            };

            console.log(`\n📦 包围盒:`);
            console.log(`  最小点: (${bbox[0].toFixed(2)}, ${bbox[1].toFixed(2)}, ${bbox[2].toFixed(2)})`);
            console.log(`  最大点: (${bbox[3].toFixed(2)}, ${bbox[4].toFixed(2)}, ${bbox[5].toFixed(2)})`);

            const width = bbox[3] - bbox[0];
            const height = bbox[4] - bbox[1];
            const depth = bbox[5] - bbox[2];
            console.log(`  尺寸: ${width.toFixed(2)} x ${height.toFixed(2)} x ${depth.toFixed(2)}`);

            // 中心点
            const centerX = (bbox[0] + bbox[3]) / 2;
            const centerY = (bbox[1] + bbox[4]) / 2;
            const centerZ = (bbox[2] + bbox[5]) / 2;
            console.log(`  中心: (${centerX.toFixed(2)}, ${centerY.toFixed(2)}, ${centerZ.toFixed(2)})`);

            // 验证包围盒有效性
            const isValidBBox = bbox[0] <= bbox[3] && bbox[1] <= bbox[4] && bbox[2] <= bbox[5];
            console.log(`  有效性: ${isValidBBox ? '✅' : '❌'}`);
        } catch (e) {
            console.log(`  ⚠️  无法解析包围盒: ${e.message}`);
        }
    }

    // 总体验证
    result.valid = result.version === 10 && result.hasGeometry && result.hasEntities;

    console.log(`\n✅ 验证结果:`);
    console.log(`  版本正确: ${result.version === 10 ? '✅' : '❌'}`);
    console.log(`  包含几何数据: ${result.hasGeometry ? '✅' : '❌'}`);
    console.log(`  包含实体数据: ${result.hasEntities ? '✅' : '❌'}`);
    console.log(`  总体: ${result.valid ? '✅ 通过' : '❌ 失败'}`);

    return result;
}

// 主程序
function main() {
    const files = [
        'output/web_ui/db1112_compressed_refno_17496_266203.xkt',
        'output/web_ui/db1112_raw_refno_17496_266203.xkt'
    ];

    const results = [];

    console.log('📋 测试文件列表:');
    files.forEach(f => {
        if (fs.existsSync(f)) {
            const size = fs.statSync(f).size;
            console.log(`  ✅ ${f} (${size} 字节)`);
        } else {
            console.log(`  ❌ ${f} (不存在)`);
        }
    });

    // 验证每个文件
    for (const file of files) {
        const result = validateXKTDetailed(file);
        if (result) {
            results.push(result);
        }
    }

    // 对比分析
    if (results.length === 2) {
        console.log('\n' + '='.repeat(60));
        console.log('📈 对比分析');
        console.log('='.repeat(60));

        const [compressed, uncompressed] = results;

        if (compressed.fileSize && uncompressed.fileSize) {
            const compressionRatio = ((1 - compressed.fileSize / uncompressed.fileSize) * 100).toFixed(1);
            console.log(`\n压缩效果:`);
            console.log(`  压缩版本: ${compressed.fileSize} 字节`);
            console.log(`  非压缩版本: ${uncompressed.fileSize} 字节`);
            console.log(`  压缩率: ${compressionRatio}%`);
        }

        // 比较包围盒
        if (compressed.bbox && uncompressed.bbox) {
            const tolerance = 0.001;
            let bboxMatch = true;

            for (let i = 0; i < 3; i++) {
                if (Math.abs(compressed.bbox.min[i] - uncompressed.bbox.min[i]) > tolerance ||
                    Math.abs(compressed.bbox.max[i] - uncompressed.bbox.max[i]) > tolerance) {
                    bboxMatch = false;
                    break;
                }
            }

            console.log(`\n包围盒一致性: ${bboxMatch ? '✅ 一致' : '❌ 不一致'}`);
        }

        // 比较统计数据
        if (compressed.stats && uncompressed.stats) {
            console.log(`\n数据一致性:`);
            console.log(`  三角形数量: ${compressed.stats.triangles === uncompressed.stats.triangles ? '✅' : '❌'}`);
            console.log(`  实体数量: ${compressed.stats.entities === uncompressed.stats.entities ? '✅' : '❌'}`);
        }
    }

    // 总结
    console.log('\n' + '='.repeat(60));
    console.log('🎯 测试总结');
    console.log('='.repeat(60));

    let allValid = results.every(r => r.valid);
    console.log(`\n数据库: 1112`);
    console.log(`参考号: 17496/266203 (ZONE)`);
    console.log(`测试文件数: ${results.length}`);
    console.log(`全部通过: ${allValid ? '✅ 是' : '❌ 否'}`);

    if (allValid && results.length > 0) {
        const r = results[0];
        console.log(`\n模型统计:`);
        if (r.stats.triangles) console.log(`  三角形: ${r.stats.triangles}`);
        if (r.stats.entities) console.log(`  实体: ${r.stats.entities}`);
        if (r.bbox) {
            const width = r.bbox.max[0] - r.bbox.min[0];
            const height = r.bbox.max[1] - r.bbox.min[1];
            const depth = r.bbox.max[2] - r.bbox.min[2];
            console.log(`  模型尺寸: ${width.toFixed(2)} x ${height.toFixed(2)} x ${depth.toFixed(2)}`);
        }

        console.log('\n✅ ZONE XKT文件生成和验证成功！');
        console.log('\n💡 可以通过以下方式查看3D模型:');
        console.log('  1. 访问: http://localhost:8080/xeokit-viewer');
        console.log('  2. 选择加载文件: db1112_compressed_refno_17496_266203.xkt');
    } else {
        console.log('\n⚠️  测试未完全通过，请检查日志');
    }
}

// 运行主程序
main();