#!/usr/bin/env node

import fs from 'fs';
import zlib from 'zlib';

// 直接分析立方体文件的包围盒
const filepath = 'tests/cube_v10_fixed.xkt';
const buffer = fs.readFileSync(filepath);

console.log('📦 立方体包围盒提取');
console.log('='.repeat(50));

// 段27在偏移820，大小20字节
const bboxCompressed = buffer.slice(820, 840);
console.log('压缩数据 (hex):', bboxCompressed.toString('hex'));

try {
    // 解压数据
    const decompressed = zlib.inflateSync(bboxCompressed);
    console.log('解压后大小:', decompressed.length, '字节');

    // 转换为Float64Array (6个double: minX,minY,minZ,maxX,maxY,maxZ)
    const bbox = new Float64Array(decompressed.buffer, decompressed.byteOffset, 6);

    console.log('\n✅ 包围盒坐标:');
    console.log(`  最小点: (${bbox[0]}, ${bbox[1]}, ${bbox[2]})`);
    console.log(`  最大点: (${bbox[3]}, ${bbox[4]}, ${bbox[5]})`);

    // 计算尺寸
    const width = bbox[3] - bbox[0];
    const height = bbox[4] - bbox[1];
    const depth = bbox[5] - bbox[2];

    console.log('\n📐 尺寸:');
    console.log(`  X轴 (宽度): ${width.toFixed(6)}`);
    console.log(`  Y轴 (高度): ${height.toFixed(6)}`);
    console.log(`  Z轴 (深度): ${depth.toFixed(6)}`);

    // 中心点
    const centerX = (bbox[0] + bbox[3]) / 2;
    const centerY = (bbox[1] + bbox[4]) / 2;
    const centerZ = (bbox[2] + bbox[5]) / 2;
    console.log(`\n🎯 中心点: (${centerX.toFixed(6)}, ${centerY.toFixed(6)}, ${centerZ.toFixed(6)})`);

    // 验证
    console.log('\n✅ 验证结果:');

    // 检查是否是单位立方体
    const expectedSize = 1.0; // 单位立方体
    const tolerance = 0.000001;

    const isUnitCube = Math.abs(width - expectedSize) < tolerance &&
                       Math.abs(height - expectedSize) < tolerance &&
                       Math.abs(depth - expectedSize) < tolerance;

    if (isUnitCube) {
        console.log('  ✅ 是单位立方体 (边长 1.0)');
    } else {
        console.log(`  ⚠️  不是单位立方体 (尺寸: ${width.toFixed(3)} x ${height.toFixed(3)} x ${depth.toFixed(3)})`);
    }

    // 检查中心是否在原点
    const centerAtOrigin = Math.abs(centerX) < tolerance &&
                          Math.abs(centerY) < tolerance &&
                          Math.abs(centerZ) < tolerance;

    if (centerAtOrigin) {
        console.log('  ✅ 中心在原点');
    } else {
        console.log(`  ℹ️  中心偏离原点: (${centerX.toFixed(6)}, ${centerY.toFixed(6)}, ${centerZ.toFixed(6)})`);
    }

    // 检查包围盒有效性
    const isValid = bbox[0] <= bbox[3] &&
                   bbox[1] <= bbox[4] &&
                   bbox[2] <= bbox[5];

    console.log(`  ${isValid ? '✅' : '❌'} 包围盒有效性: ${isValid ? '正确' : '错误'}`);

} catch (e) {
    console.error('❌ 解压失败:', e.message);
}