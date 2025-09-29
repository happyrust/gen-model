#!/usr/bin/env node

import fs from 'fs';
import zlib from 'zlib';

console.log('📦 立方体包围盒验证测试');
console.log('='.repeat(60));

function analyzeBoundingBox(filepath) {
    console.log(`\n📁 分析文件: ${filepath}`);
    console.log('-'.repeat(50));

    try {
        const buffer = fs.readFileSync(filepath);

        // 解析版本
        const versionAndFlags = buffer.readUInt32LE(0);
        const version = versionAndFlags & 0x7FFFFFFF;
        const compressed = (versionAndFlags & 0x80000000) !== 0;

        console.log(`📌 版本: ${version}`);
        console.log(`🗜️  压缩: ${compressed ? '是' : '否'}`);

        // XKT v10 段偏移位置
        const numSections = 29;
        const offsets = [];
        const sizes = [];

        // 读取所有段偏移
        for (let i = 0; i < numSections; i++) {
            offsets.push(buffer.readUInt32LE(4 + i * 4));
        }

        // 计算段大小
        for (let i = 0; i < numSections - 1; i++) {
            sizes.push(offsets[i + 1] - offsets[i]);
        }
        sizes.push(buffer.length - offsets[numSections - 1]);

        console.log('\n🔍 段信息:');
        console.log(`  段[27] (包围盒): 偏移=${offsets[27]}, 大小=${sizes[27]} 字节`);

        // 段27是包围盒数据 (Float64Array, 6个double值: minX,minY,minZ,maxX,maxY,maxZ)
        const bboxIndex = 27;
        const bboxOffset = offsets[bboxIndex];
        const bboxSize = sizes[bboxIndex];

        if (bboxSize > 0 && bboxOffset > 0) {
            const bboxData = buffer.slice(bboxOffset, bboxOffset + bboxSize);

            if (compressed) {
                try {
                    const decompressed = zlib.inflateSync(bboxData);
                    console.log(`  解压后大小: ${decompressed.length} 字节`);

                    // Float64Array需要48字节 (6 * 8)
                    if (decompressed.length === 48) {
                        const bbox = new Float64Array(decompressed.buffer, decompressed.byteOffset, 6);

                        console.log('\n📊 包围盒坐标:');
                        console.log(`  最小点: (${bbox[0].toFixed(3)}, ${bbox[1].toFixed(3)}, ${bbox[2].toFixed(3)})`);
                        console.log(`  最大点: (${bbox[3].toFixed(3)}, ${bbox[4].toFixed(3)}, ${bbox[5].toFixed(3)})`);

                        // 计算尺寸
                        const width = bbox[3] - bbox[0];
                        const height = bbox[4] - bbox[1];
                        const depth = bbox[5] - bbox[2];

                        console.log('\n📐 包围盒尺寸:');
                        console.log(`  宽度(X): ${width.toFixed(3)}`);
                        console.log(`  高度(Y): ${height.toFixed(3)}`);
                        console.log(`  深度(Z): ${depth.toFixed(3)}`);
                        console.log(`  对角线: ${Math.sqrt(width*width + height*height + depth*depth).toFixed(3)}`);

                        // 中心点
                        const centerX = (bbox[0] + bbox[3]) / 2;
                        const centerY = (bbox[1] + bbox[4]) / 2;
                        const centerZ = (bbox[2] + bbox[5]) / 2;

                        console.log('\n🎯 中心点:');
                        console.log(`  (${centerX.toFixed(3)}, ${centerY.toFixed(3)}, ${centerZ.toFixed(3)})`);

                        // 验证立方体的合理性
                        console.log('\n✅ 验证结果:');

                        // 检查是否是立方体（三个维度应该相等或接近）
                        const tolerance = 0.01;
                        const isCube = Math.abs(width - height) < tolerance &&
                                      Math.abs(height - depth) < tolerance;

                        if (isCube) {
                            console.log(`  ✅ 是立方体 (边长约 ${width.toFixed(3)})`);
                        } else {
                            console.log(`  ⚠️  不是标准立方体 (尺寸: ${width.toFixed(3)} x ${height.toFixed(3)} x ${depth.toFixed(3)})`);
                        }

                        // 检查包围盒是否有效
                        const isValid = bbox[0] <= bbox[3] &&
                                       bbox[1] <= bbox[4] &&
                                       bbox[2] <= bbox[5];

                        console.log(`  ${isValid ? '✅' : '❌'} 包围盒有效性: ${isValid ? '正确' : '错误(min > max)'}`);

                        // 检查是否包含原点附近
                        const nearOrigin = Math.abs(centerX) < 10 &&
                                          Math.abs(centerY) < 10 &&
                                          Math.abs(centerZ) < 10;

                        console.log(`  ${nearOrigin ? '✅' : '⚠️'} 位置合理性: ${nearOrigin ? '在原点附近' : '远离原点'}`);

                        return {
                            min: [bbox[0], bbox[1], bbox[2]],
                            max: [bbox[3], bbox[4], bbox[5]],
                            center: [centerX, centerY, centerZ],
                            size: [width, height, depth],
                            isCube,
                            isValid
                        };
                    } else {
                        console.log(`  ❌ 包围盒数据大小错误: ${decompressed.length} 字节 (期望48字节)`);
                    }
                } catch (e) {
                    console.log(`  ❌ 解压失败: ${e.message}`);
                }
            } else {
                // 未压缩的情况
                if (bboxData.length === 48) {
                    const bbox = new Float64Array(bboxData.buffer, bboxData.byteOffset, 6);
                    console.log('\n📊 包围盒坐标 (未压缩):');
                    console.log(`  最小点: (${bbox[0].toFixed(3)}, ${bbox[1].toFixed(3)}, ${bbox[2].toFixed(3)})`);
                    console.log(`  最大点: (${bbox[3].toFixed(3)}, ${bbox[4].toFixed(3)}, ${bbox[5].toFixed(3)})`);
                }
            }
        } else {
            console.log('  ⚠️  没有包围盒数据');
        }

        // 分析顶点数据来验证包围盒
        console.log('\n🔍 从几何数据验证包围盒:');

        // 段8是位置数据 (压缩的Uint32Array)
        if (sizes[8] > 0) {
            const posData = buffer.slice(offsets[8], offsets[8] + sizes[8]);

            if (compressed) {
                try {
                    const decompressed = zlib.inflateSync(posData);
                    const positions = new Uint32Array(decompressed.buffer, decompressed.byteOffset, decompressed.length / 4);

                    console.log(`  顶点数量: ${positions.length / 3}`);

                    // 如果有量化矩阵(段11)，需要反量化
                    if (sizes[11] > 0) {
                        const matrixData = buffer.slice(offsets[11], offsets[11] + sizes[11]);
                        const matrixDecompressed = zlib.inflateSync(matrixData);
                        const matrix = new Float32Array(matrixDecompressed.buffer, matrixDecompressed.byteOffset, 16);

                        console.log(`  找到量化矩阵 (4x4)`);

                        // 显示前几个顶点的量化值
                        console.log('\n  前3个顶点的量化值:');
                        for (let i = 0; i < Math.min(3, positions.length / 3); i++) {
                            console.log(`    顶点${i}: (${positions[i*3]}, ${positions[i*3+1]}, ${positions[i*3+2]})`);
                        }
                    }
                } catch (e) {
                    console.log(`  ❌ 位置数据解压失败: ${e.message}`);
                }
            }
        }

    } catch (error) {
        console.log(`❌ 错误: ${error.message}`);
        return null;
    }
}

// 主程序
const testFiles = [
    'tests/cube_v10_fixed.xkt',
    'tests/cube_v10_final.xkt'
];

console.log('📋 开始验证立方体包围盒...\n');

const results = [];
for (const file of testFiles) {
    if (fs.existsSync(file)) {
        const result = analyzeBoundingBox(file);
        if (result) {
            results.push({ file, ...result });
        }
    } else {
        console.log(`⚠️  文件不存在: ${file}`);
    }
}

// 比较两个文件的包围盒
if (results.length === 2) {
    console.log('\n' + '='.repeat(60));
    console.log('📊 包围盒对比:');
    console.log('='.repeat(60));

    const [r1, r2] = results;

    console.log('\n文件1: ' + r1.file.split('/').pop());
    console.log(`  包围盒: (${r1.min.map(v => v.toFixed(3)).join(', ')}) 到 (${r1.max.map(v => v.toFixed(3)).join(', ')})`);

    console.log('\n文件2: ' + r2.file.split('/').pop());
    console.log(`  包围盒: (${r2.min.map(v => v.toFixed(3)).join(', ')}) 到 (${r2.max.map(v => v.toFixed(3)).join(', ')})`);

    // 检查是否一致
    const tolerance = 0.001;
    let allMatch = true;
    for (let i = 0; i < 3; i++) {
        if (Math.abs(r1.min[i] - r2.min[i]) > tolerance ||
            Math.abs(r1.max[i] - r2.max[i]) > tolerance) {
            allMatch = false;
            break;
        }
    }

    console.log(`\n${allMatch ? '✅' : '⚠️'} 两个文件的包围盒${allMatch ? '一致' : '不一致'}`);
}

console.log('\n' + '='.repeat(60));
console.log('🎯 测试完成');
console.log('='.repeat(60));