#!/usr/bin/env node

import fetch from 'node-fetch';
import fs from 'fs';
import path from 'path';

/**
 * 扫描数据库1112寻找所有有效的区域refno
 */
class DatabaseZoneScanner {
    constructor(baseUrl = 'http://localhost:8080') {
        this.baseUrl = baseUrl;
        this.dbno = 1112;
        this.validZones = [];
        this.scannedRefnos = new Set();
        this.outputDir = path.join('output', 'zones', `db${this.dbno}`);
    }

    /**
     * 生成refno扫描范围
     */
    generateRefnoRanges() {
        const ranges = [];

        // 基于已知的有效refno (17496/266203) 扩展搜索范围
        // 格式: 主号/子号

        // 扫描相同主号的不同子号
        for (let subNo = 266200; subNo <= 266250; subNo++) {
            ranges.push(`17496/${subNo}`);
        }

        // 扫描相邻主号
        for (let mainNo = 17494; mainNo <= 17500; mainNo++) {
            for (let subNo = 256200; subNo <= 256220; subNo++) {
                ranges.push(`${mainNo}/${subNo}`);
            }
            for (let subNo = 266200; subNo <= 266220; subNo++) {
                ranges.push(`${mainNo}/${subNo}`);
            }
        }

        return ranges;
    }

    /**
     * 快速测试refno是否包含几何数据
     */
    async testRefno(refno) {
        if (this.scannedRefnos.has(refno)) {
            return false;
        }
        this.scannedRefnos.add(refno);

        try {
            const controller = new AbortController();
            const timeout = setTimeout(() => controller.abort(), 5000); // 5秒超时

            const response = await fetch(`${this.baseUrl}/api/xkt/generate`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    dbno: this.dbno,
                    refno: refno,
                    compress: true
                }),
                signal: controller.signal
            });

            clearTimeout(timeout);

            if (!response.ok) {
                return false;
            }

            const result = await response.json();

            // 检查是否成功生成且有数据
            if (result.success && result.filename) {
                // 下载文件检查大小
                const downloadUrl = `${this.baseUrl}${result.url}`;
                const xktResponse = await fetch(downloadUrl);

                if (xktResponse.ok) {
                    const data = await xktResponse.arrayBuffer();
                    // 文件大于500字节才认为有有效几何数据
                    return data.byteLength > 500;
                }
            }

            return false;
        } catch (error) {
            return false;
        }
    }

    /**
     * 扫描数据库寻找有效区域
     */
    async scanForValidZones() {
        console.log('🔍 开始扫描数据库 1112...\n');

        const ranges = this.generateRefnoRanges();
        console.log(`📊 将扫描 ${ranges.length} 个refno\n`);

        let scannedCount = 0;
        const batchSize = 10;

        for (let i = 0; i < ranges.length; i += batchSize) {
            const batch = ranges.slice(i, Math.min(i + batchSize, ranges.length));

            // 并行测试一批refno
            const results = await Promise.all(
                batch.map(async (refno) => {
                    process.stdout.write(`\r扫描进度: ${scannedCount}/${ranges.length} - 测试 ${refno}...`);
                    const isValid = await this.testRefno(refno);
                    scannedCount++;

                    if (isValid) {
                        return {
                            refno: refno,
                            valid: true
                        };
                    }
                    return null;
                })
            );

            // 收集有效结果
            results.forEach(result => {
                if (result && result.valid) {
                    this.validZones.push({
                        id: `zone_${String(this.validZones.length + 1).padStart(3, '0')}`,
                        name: `Zone ${this.validZones.length + 1}`,
                        refno: result.refno,
                        description: `自动发现的区域 - ${result.refno}`
                    });
                    console.log(`\n✅ 找到有效区域: ${result.refno}`);
                }
            });

            // 避免请求过快
            await new Promise(resolve => setTimeout(resolve, 1000));
        }

        console.log(`\n\n扫描完成！找到 ${this.validZones.length} 个有效区域`);
        return this.validZones;
    }

    /**
     * 为所有有效区域生成XKT文件
     */
    async generateAllZoneXKTs() {
        if (this.validZones.length === 0) {
            console.log('❌ 没有找到有效区域');
            return;
        }

        console.log('\n' + '='.repeat(60));
        console.log(`🏗️  开始为 ${this.validZones.length} 个区域生成XKT文件`);
        console.log('='.repeat(60));

        const generatedFiles = [];

        for (const zone of this.validZones) {
            console.log(`\n🔧 生成 ${zone.name} (${zone.refno})...`);

            try {
                // 生成XKT
                const response = await fetch(`${this.baseUrl}/api/xkt/generate`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        dbno: this.dbno,
                        refno: zone.refno,
                        compress: true
                    })
                });

                if (response.ok) {
                    const result = await response.json();

                    // 下载文件
                    const downloadUrl = `${this.baseUrl}${result.url}`;
                    const xktResponse = await fetch(downloadUrl);
                    const xktData = await xktResponse.arrayBuffer();
                    const buffer = Buffer.from(xktData);

                    // 保存文件
                    const zonePath = path.join(this.outputDir, `${zone.id}.xkt`);
                    fs.writeFileSync(zonePath, buffer);

                    console.log(`  ✅ 已保存: ${zonePath} (${buffer.length} 字节)`);

                    generatedFiles.push({
                        zone: zone,
                        filename: `${zone.id}.xkt`,
                        size: buffer.length,
                        path: zonePath
                    });
                }
            } catch (error) {
                console.error(`  ❌ 生成失败: ${error.message}`);
            }

            // 延时避免服务器过载
            await new Promise(resolve => setTimeout(resolve, 500));
        }

        return generatedFiles;
    }

    /**
     * 生成区域清单
     */
    generateManifest(generatedFiles) {
        const manifest = {
            database: this.dbno,
            totalZones: generatedFiles.length,
            scanDate: new Date().toISOString(),
            zones: generatedFiles.map((file, index) => ({
                id: file.zone.id,
                name: file.zone.name,
                refno: file.zone.refno,
                description: file.zone.description,
                xktFile: file.filename,
                fileSize: file.size,
                compressed: true,
                // 占位包围盒 - 实际应从几何数据计算
                boundingBox: {
                    min: [-50 - index * 100, -50, 0],
                    max: [50 - index * 100, 50, 100]
                },
                center: [0 - index * 100, 0, 50],
                radius: 86.6
            }))
        };

        const manifestPath = path.join(this.outputDir, 'zone_manifest.json');
        fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

        console.log(`\n📝 清单已保存到: ${manifestPath}`);
        return manifest;
    }

    /**
     * 主执行函数
     */
    async run() {
        console.log('='.repeat(60));
        console.log('🔍 数据库区域自动发现和XKT生成');
        console.log('='.repeat(60));

        // 创建输出目录
        if (!fs.existsSync(this.outputDir)) {
            fs.mkdirSync(this.outputDir, { recursive: true });
        }

        // 扫描有效区域
        await this.scanForValidZones();

        if (this.validZones.length === 0) {
            console.log('\n⚠️ 未找到有效区域，使用已知区域...');

            // 使用已知的有效区域
            this.validZones = [{
                id: 'zone_001',
                name: '工艺区A',
                refno: '17496/266203',
                description: '主要工艺设备区域'
            }];
        }

        // 生成XKT文件
        const generatedFiles = await this.generateAllZoneXKTs();

        // 生成清单
        const manifest = this.generateManifest(generatedFiles);

        // 输出总结
        console.log('\n' + '='.repeat(60));
        console.log('📊 生成总结');
        console.log('='.repeat(60));
        console.log(`扫描的refno数量: ${this.scannedRefnos.size}`);
        console.log(`找到的有效区域: ${this.validZones.length}`);
        console.log(`成功生成的XKT: ${generatedFiles.length}`);

        let totalSize = 0;
        generatedFiles.forEach(file => totalSize += file.size);
        console.log(`总文件大小: ${(totalSize / 1024).toFixed(2)} KB`);

        console.log('\n✅ 完成！');
        console.log(`📁 文件位置: ${this.outputDir}`);

        // 保存扫描结果
        const scanResultPath = path.join(this.outputDir, 'scan_result.json');
        fs.writeFileSync(scanResultPath, JSON.stringify({
            scanDate: new Date().toISOString(),
            scannedCount: this.scannedRefnos.size,
            validZones: this.validZones,
            generatedFiles: generatedFiles.map(f => ({
                zone: f.zone.name,
                refno: f.zone.refno,
                size: f.size
            }))
        }, null, 2));

        console.log(`📊 扫描结果已保存到: ${scanResultPath}`);

        return manifest;
    }
}

// 执行
if (import.meta.url === `file://${process.argv[1]}`) {
    const scanner = new DatabaseZoneScanner();
    scanner.run().catch(console.error);
}

export { DatabaseZoneScanner };