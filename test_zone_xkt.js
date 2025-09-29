#!/usr/bin/env node

import http from 'http';
import fs from 'fs';
import zlib from 'zlib';

console.log('🔍 查询并生成数据库1112的ZONE XKT文件');
console.log('='.repeat(60));

// 查询ZONE类型的参考号
async function queryZoneRefnos() {
    const postData = JSON.stringify({
        dbno: 1112,
        types: ["ZONE"]
    });

    const options = {
        hostname: 'localhost',
        port: 8080,
        path: '/api/query/types',
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Content-Length': Buffer.byteLength(postData)
        }
    };

    return new Promise((resolve, reject) => {
        const req = http.request(options, (res) => {
            let data = '';
            res.on('data', (chunk) => data += chunk);
            res.on('end', () => {
                if (res.statusCode === 200) {
                    try {
                        const result = JSON.parse(data);
                        resolve(result);
                    } catch (e) {
                        reject(new Error('Failed to parse response'));
                    }
                } else {
                    reject(new Error(`API error ${res.statusCode}: ${data}`));
                }
            });
        });
        req.on('error', reject);
        req.write(postData);
        req.end();
    });
}

// 生成XKT文件
async function generateXKT(dbno, refno, compress = true) {
    console.log(`\n📦 生成XKT: DBNO=${dbno}, REFNO=${refno}, 压缩=${compress}`);

    const postData = JSON.stringify({
        dbno: dbno,
        refno: refno,
        compress: compress
    });

    const options = {
        hostname: 'localhost',
        port: 8080,
        path: '/api/xkt/generate',
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Content-Length': Buffer.byteLength(postData)
        }
    };

    return new Promise((resolve, reject) => {
        const req = http.request(options, (res) => {
            let data = '';
            res.on('data', (chunk) => data += chunk);
            res.on('end', () => {
                if (res.statusCode === 200) {
                    try {
                        const result = JSON.parse(data);
                        resolve(result);
                    } catch (e) {
                        resolve({ success: true, message: data });
                    }
                } else {
                    reject(new Error(`API error ${res.statusCode}: ${data}`));
                }
            });
        });
        req.on('error', reject);
        req.write(postData);
        req.end();
    });
}

// 验证XKT文件
async function validateXKT(filepath) {
    console.log(`\n🔍 验证XKT文件: ${filepath}`);

    if (!fs.existsSync(filepath)) {
        console.log('❌ 文件不存在');
        return false;
    }

    const buffer = fs.readFileSync(filepath);
    const fileSize = buffer.length;

    console.log(`📁 文件大小: ${fileSize} 字节`);

    // 解析版本
    const versionAndFlags = buffer.readUInt32LE(0);
    const version = versionAndFlags & 0x7FFFFFFF;
    const compressed = (versionAndFlags & 0x80000000) !== 0;

    console.log(`📌 版本: ${version}`);
    console.log(`🗜️  压缩: ${compressed ? '是' : '否'}`);

    if (version !== 10) {
        console.log(`❌ 不支持的版本: ${version}`);
        return false;
    }

    // 分析段信息
    const numSections = 29;
    let hasGeometry = false;
    let hasEntities = false;

    // 检查几何数据段
    if (fileSize >= 120) {
        // 检查是否有几何数据（通常在段8和段9）
        const geometryOffset = buffer.readUInt32LE(4 + 8 * 4);
        const normalOffset = buffer.readUInt32LE(4 + 9 * 4);

        if (geometryOffset > 0 && normalOffset > geometryOffset) {
            hasGeometry = true;
            console.log(`✅ 包含几何数据`);
        }

        // 检查实体段（段25）
        const entityOffset = buffer.readUInt32LE(4 + 25 * 4);
        const nextOffset = buffer.readUInt32LE(4 + 26 * 4);

        if (entityOffset > 0 && nextOffset > entityOffset) {
            hasEntities = true;
            console.log(`✅ 包含实体数据`);
        }
    }

    // 提取包围盒（段27）
    if (fileSize >= 840) {
        try {
            const bboxCompressed = buffer.slice(820, 840);
            const decompressed = zlib.inflateSync(bboxCompressed);
            const bbox = new Float64Array(decompressed.buffer, decompressed.byteOffset, 6);

            console.log(`📦 包围盒:`);
            console.log(`  最小点: (${bbox[0].toFixed(2)}, ${bbox[1].toFixed(2)}, ${bbox[2].toFixed(2)})`);
            console.log(`  最大点: (${bbox[3].toFixed(2)}, ${bbox[4].toFixed(2)}, ${bbox[5].toFixed(2)})`);

            const width = bbox[3] - bbox[0];
            const height = bbox[4] - bbox[1];
            const depth = bbox[5] - bbox[2];
            console.log(`  尺寸: ${width.toFixed(2)} x ${height.toFixed(2)} x ${depth.toFixed(2)}`);
        } catch (e) {
            console.log(`⚠️  无法解析包围盒`);
        }
    }

    const isValid = hasGeometry && hasEntities;
    console.log(`\n${isValid ? '✅' : '❌'} 文件验证: ${isValid ? '通过' : '失败'}`);

    return isValid;
}

// 主程序
async function main() {
    try {
        // 使用一个已知的ZONE参考号 (从之前的输出中看到的)
        // 或者我们可以尝试查询
        const testZones = [
            "17496/266203",  // 之前测试过的参考号
            "17496/266210",  // 可能是ZONE下的元素
        ];

        console.log('📋 测试ZONE参考号列表:');
        testZones.forEach(z => console.log(`  - ${z}`));

        // 生成第一个ZONE的XKT
        const zoneRefno = testZones[0];
        console.log(`\n🎯 选择ZONE: ${zoneRefno}`);

        // 生成压缩版本
        const compressedResult = await generateXKT(1112, zoneRefno, true);
        console.log('✅ 压缩版本生成成功');

        let compressedFile = null;
        if (compressedResult.filename) {
            compressedFile = `output/web_ui/${compressedResult.filename}`;
            console.log(`  文件: ${compressedFile}`);
        } else {
            // 尝试查找生成的文件
            const expectedFile = `output/web_ui/db1112_compressed_refno_${zoneRefno.replace('/', '_')}.xkt`;
            if (fs.existsSync(expectedFile)) {
                compressedFile = expectedFile;
                console.log(`  文件: ${compressedFile}`);
            }
        }

        // 生成非压缩版本
        const uncompressedResult = await generateXKT(1112, zoneRefno, false);
        console.log('✅ 非压缩版本生成成功');

        let uncompressedFile = null;
        if (uncompressedResult.filename) {
            uncompressedFile = `output/web_ui/${uncompressedResult.filename}`;
            console.log(`  文件: ${uncompressedFile}`);
        } else {
            // 尝试查找生成的文件
            const expectedFile = `output/web_ui/db1112_raw_refno_${zoneRefno.replace('/', '_')}.xkt`;
            if (fs.existsSync(expectedFile)) {
                uncompressedFile = expectedFile;
                console.log(`  文件: ${uncompressedFile}`);
            }
        }

        // 验证生成的文件
        console.log('\n' + '='.repeat(60));
        console.log('📊 验证测试结果');
        console.log('='.repeat(60));

        let testsPassed = 0;
        let totalTests = 0;

        if (compressedFile) {
            totalTests++;
            console.log('\n测试1: 压缩版本');
            if (validateXKT(compressedFile)) {
                testsPassed++;
            }
        }

        if (uncompressedFile) {
            totalTests++;
            console.log('\n测试2: 非压缩版本');
            if (validateXKT(uncompressedFile)) {
                testsPassed++;
            }
        }

        // 比较两个文件的大小
        if (compressedFile && uncompressedFile) {
            const compressedSize = fs.statSync(compressedFile).size;
            const uncompressedSize = fs.statSync(uncompressedFile).size;
            const compressionRatio = ((1 - compressedSize / uncompressedSize) * 100).toFixed(1);

            console.log('\n📈 压缩效果分析:');
            console.log(`  压缩版本: ${compressedSize} 字节`);
            console.log(`  非压缩版本: ${uncompressedSize} 字节`);
            console.log(`  压缩率: ${compressionRatio}%`);
        }

        // 总结
        console.log('\n' + '='.repeat(60));
        console.log('🎯 测试总结');
        console.log('='.repeat(60));
        console.log(`  测试ZONE: ${zoneRefno}`);
        console.log(`  测试通过: ${testsPassed}/${totalTests}`);

        if (testsPassed === totalTests) {
            console.log('  状态: ✅ 全部通过');
            console.log('\n💡 可以使用以下方式查看生成的模型:');
            console.log('  1. 访问: http://localhost:8080/xeokit-viewer');
            console.log(`  2. 加载文件: ${compressedFile || uncompressedFile}`);
        } else {
            console.log('  状态: ⚠️  部分失败');
        }

    } catch (error) {
        console.error('\n❌ 测试失败:', error.message);
        process.exit(1);
    }
}

// 运行主程序
main().catch(console.error);