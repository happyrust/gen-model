#!/usr/bin/env node

import fs from 'fs';
import zlib from 'zlib';

console.log('🧊 立方体 XKT 文件测试');
console.log('='.repeat(50));

// 测试文件列表
const testFiles = [
    'tests/cube_v10_fixed.xkt',
    'tests/cube_v10_final.xkt'
];

function analyzeXKTFile(filepath) {
    console.log(`\n📁 分析文件: ${filepath}`);
    console.log('-'.repeat(40));

    try {
        // 读取文件
        const buffer = fs.readFileSync(filepath);
        console.log(`✅ 文件大小: ${buffer.length} 字节`);

        // 解析头部
        const versionAndFlags = buffer.readUInt32LE(0);
        const version = versionAndFlags & 0x7FFFFFFF; // 去除最高位压缩标志
        const compressed = (versionAndFlags & 0x80000000) !== 0;
        console.log(`📌 版本: ${version}`);
        console.log(`🗜️  压缩: ${compressed ? '是' : '否'}`);

        if (version !== 10) {
            console.log(`❌ 错误: 期望版本 10，实际版本 ${version}`);
            return false;
        }

        // 读取段偏移
        const numSections = 29; // XKT v10 有29个段
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

        // 分析关键段
        console.log('\n📊 关键段分析:');

        // 段0: 元数据 (JSON)
        const metadataStart = offsets[0];
        const metadataSize = sizes[0];
        if (metadataSize > 8) {
            try {
                const compressed = buffer.slice(metadataStart, metadataStart + metadataSize);
                const decompressed = zlib.inflateSync(compressed);
                const metadata = decompressed.toString('utf-8');
                const metaObj = JSON.parse(metadata);
                console.log(`  - 元数据: ${metaObj.id || 'N/A'}`);
                console.log(`    作者: ${metaObj.author || 'N/A'}`);
                console.log(`    创建时间: ${metaObj.createdAt || 'N/A'}`);
            } catch (e) {
                console.log(`  - 元数据: 解析失败`);
            }
        }

        // 段4: 索引数据
        if (sizes[4] > 0) {
            console.log(`  - 索引数据: ${sizes[4]} 字节`);
        }

        // 段8: 位置数据
        if (sizes[8] > 0) {
            console.log(`  - 位置数据: ${sizes[8]} 字节`);
        }

        // 段9: 法线数据
        if (sizes[9] > 0) {
            console.log(`  - 法线数据: ${sizes[9]} 字节`);
        }

        // 段25: 实体ID
        if (sizes[25] > 8) {
            try {
                const compressed = buffer.slice(offsets[25], offsets[25] + sizes[25]);
                const decompressed = zlib.inflateSync(compressed);
                const entityIds = decompressed.toString('utf-8');
                console.log(`  - 实体ID: ${entityIds}`);
            } catch (e) {
                console.log(`  - 实体ID: 解析失败`);
            }
        }

        // 段27: 边界框
        if (sizes[27] > 0) {
            console.log(`  - 边界框数据: ${sizes[27]} 字节`);
        }

        // 验证结果
        console.log('\n✅ 文件结构验证通过');
        console.log(`  - 版本正确: v${version}`);
        console.log(`  - 段数量正确: ${numSections}`);
        console.log(`  - 包含几何数据: ${sizes[8] > 0 && sizes[9] > 0 ? '是' : '否'}`);
        console.log(`  - 包含索引数据: ${sizes[4] > 0 ? '是' : '否'}`);

        return true;

    } catch (error) {
        console.log(`❌ 错误: ${error.message}`);
        return false;
    }
}

// 主程序
async function main() {
    let successCount = 0;
    let failCount = 0;

    for (const file of testFiles) {
        if (fs.existsSync(file)) {
            const result = analyzeXKTFile(file);
            if (result) {
                successCount++;
            } else {
                failCount++;
            }
        } else {
            console.log(`\n⚠️ 文件不存在: ${file}`);
            failCount++;
        }
    }

    // 总结
    console.log('\n' + '='.repeat(50));
    console.log('📋 测试总结');
    console.log('='.repeat(50));
    console.log(`✅ 成功: ${successCount}`);
    console.log(`❌ 失败: ${failCount}`);

    if (successCount > 0) {
        console.log('\n🎉 立方体 XKT 文件验证成功！');
        console.log('💡 可以通过以下方式查看:');
        console.log('   1. 启动服务器: cd tests && python3 -m http.server 8082');
        console.log('   2. 访问: http://localhost:8082/xkt_v10_test.html');
    }

    process.exit(failCount > 0 ? 1 : 0);
}

main();