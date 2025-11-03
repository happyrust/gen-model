#!/usr/bin/env node

/**
 * XKT 模型验证脚本
 * 
 * 功能：
 * 1. 解析 XKT 文件结构
 * 2. 验证文件完整性
 * 3. 统计几何数据
 * 4. 生成详细的验证报告
 */

import fs from 'fs';
import path from 'path';
import pako from 'pako';

class XKTValidator {
    constructor(filePath) {
        this.filePath = filePath;
        this.buffer = null;
        this.report = {
            valid: false,
            filePath: filePath,
            fileName: path.basename(filePath),
            fileSize: 0,
            version: 0,
            compressed: false,
            sectionCount: 0,
            errors: [],
            warnings: [],
            statistics: {
                geometries: 0,
                meshes: 0,
                entities: 0,
                vertices: 0,
                triangles: 0,
                normals: 0,
                colors: 0,
                uvs: 0
            },
            sections: {},
            metadata: {}
        };
    }

    /**
     * 执行完整验证
     */
    async validate() {
        try {
            // 1. 读取文件
            this.readFile();
            
            // 2. 验证文件头
            this.validateHeader();
            
            // 3. 解析段偏移
            this.parseSectionOffsets();
            
            // 4. 验证各个段
            this.validateSections();
            
            // 5. 解析元数据
            this.parseMetadata();
            
            // 6. 统计几何数据
            this.calculateStatistics();
            
            // 7. 最终验证
            this.finalValidation();
            
            return this.report;
        } catch (error) {
            this.report.errors.push(`验证失败: ${error.message}`);
            this.report.valid = false;
            return this.report;
        }
    }

    /**
     * 读取文件
     */
    readFile() {
        if (!fs.existsSync(this.filePath)) {
            throw new Error(`文件不存在: ${this.filePath}`);
        }

        this.buffer = fs.readFileSync(this.filePath);
        this.report.fileSize = this.buffer.length;

        if (this.buffer.length < 120) {
            throw new Error(`文件太小 (${this.buffer.length} bytes)，不是有效的 XKT 文件`);
        }
    }

    /**
     * 验证文件头
     */
    validateHeader() {
        // 读取版本和压缩标志 (4 bytes)
        const versionAndFlags = this.buffer.readUInt32LE(0);
        this.report.version = versionAndFlags & 0x7FFFFFFF;
        this.report.compressed = (versionAndFlags & 0x80000000) !== 0;

        // 验证版本号
        if (this.report.version !== 10) {
            this.report.errors.push(`不支持的 XKT 版本: ${this.report.version} (期望: 10)`);
        }

        console.log(`📄 文件: ${this.report.fileName}`);
        console.log(`📦 大小: ${this.formatFileSize(this.report.fileSize)}`);
        console.log(`🔢 版本: v${this.report.version}`);
        console.log(`🗜️  压缩: ${this.report.compressed ? '是' : '否'}`);
    }

    /**
     * 解析段偏移
     */
    parseSectionOffsets() {
        const sectionNames = [
            'metadata', 'positions', 'normals', 'colors', 'uvs',
            'indices', 'edgeIndices', 'matrices', 'eachMeshGeometriesPortion',
            'eachMeshMaterialAttributes', 'eachMeshMaterial', 'eachGeometryPrimitiveType',
            'eachGeometryPositionsPortion', 'eachGeometryNormalsPortion', 'eachGeometryColorsPortion',
            'eachGeometryUVsPortion', 'eachGeometryIndicesPortion', 'eachGeometryEdgeIndicesPortion',
            'eachMeshId', 'eachEntityId', 'eachEntityMeshIds', 'eachTileAABB',
            'eachTileEntitiesIndex', 'eachEntityMatricesIndex', 'eachMeshMatricesIndex',
            'eachGeometryCompressedPositionsPortion', 'eachGeometryCompressedNormalsPortion',
            'eachGeometryCompressedColorsPortion', 'eachGeometryCompressedUVsPortion'
        ];

        this.report.sectionCount = 29;
        const offsets = [];

        for (let i = 0; i < 29; i++) {
            const offset = this.buffer.readUInt32LE(4 + i * 4);
            offsets.push(offset);
            
            if (i < sectionNames.length) {
                this.report.sections[sectionNames[i]] = {
                    offset: offset,
                    hasData: offset > 120,
                    size: 0
                };
            }
        }

        // 计算每个段的大小
        for (let i = 0; i < offsets.length - 1; i++) {
            if (offsets[i] > 120) {
                const sectionName = sectionNames[i];
                if (this.report.sections[sectionName]) {
                    this.report.sections[sectionName].size = offsets[i + 1] - offsets[i];
                }
            }
        }

        // 验证偏移量
        for (let i = 0; i < offsets.length; i++) {
            if (offsets[i] > this.buffer.length) {
                this.report.errors.push(`段 ${i} (${sectionNames[i]}) 偏移超出文件大小: ${offsets[i]} > ${this.buffer.length}`);
            }
        }
    }

    /**
     * 验证各个段
     */
    validateSections() {
        const requiredSections = ['positions', 'indices', 'eachMeshId', 'eachEntityId'];
        
        for (const sectionName of requiredSections) {
            if (!this.report.sections[sectionName] || !this.report.sections[sectionName].hasData) {
                this.report.warnings.push(`缺少必需的段: ${sectionName}`);
            }
        }

        // 统计有数据的段
        const sectionsWithData = Object.keys(this.report.sections).filter(
            name => this.report.sections[name].hasData
        );

        console.log(`\n📊 数据段统计:`);
        console.log(`   - 总段数: ${this.report.sectionCount}`);
        console.log(`   - 有数据的段: ${sectionsWithData.length}`);
    }

    /**
     * 解析元数据
     */
    parseMetadata() {
        const metadataSection = this.report.sections['metadata'];
        
        if (!metadataSection || !metadataSection.hasData) {
            this.report.warnings.push('metadata 段为空');
            return;
        }

        try {
            const offset = metadataSection.offset;
            const size = metadataSection.size;
            
            if (size > 0 && offset + size <= this.buffer.length) {
                let metadataBuffer = this.buffer.slice(offset, offset + size);
                
                // 如果压缩，先解压
                if (this.report.compressed) {
                    try {
                        metadataBuffer = pako.inflate(metadataBuffer);
                    } catch (e) {
                        this.report.warnings.push(`metadata 解压失败: ${e.message}`);
                        return;
                    }
                }
                
                // 解析 JSON
                try {
                    const metadataStr = metadataBuffer.toString('utf8');
                    this.report.metadata = JSON.parse(metadataStr);
                    
                    console.log(`\n📋 元数据:`);
                    if (this.report.metadata.title) {
                        console.log(`   - 标题: ${this.report.metadata.title}`);
                    }
                    if (this.report.metadata.author) {
                        console.log(`   - 作者: ${this.report.metadata.author}`);
                    }
                    if (this.report.metadata.created) {
                        console.log(`   - 创建时间: ${this.report.metadata.created}`);
                    }
                } catch (e) {
                    this.report.warnings.push(`metadata JSON 解析失败: ${e.message}`);
                }
            }
        } catch (error) {
            this.report.warnings.push(`解析 metadata 失败: ${error.message}`);
        }
    }

    /**
     * 统计几何数据
     */
    calculateStatistics() {
        try {
            // 统计几何体数量
            const geometryPortionSection = this.report.sections['eachGeometryPositionsPortion'];
            if (geometryPortionSection && geometryPortionSection.hasData && geometryPortionSection.size > 0) {
                // 每个几何体有 2 个 uint32 (start, count)
                this.report.statistics.geometries = Math.floor(geometryPortionSection.size / 8);
            }

            // 统计网格数量
            const meshIdSection = this.report.sections['eachMeshId'];
            if (meshIdSection && meshIdSection.hasData && meshIdSection.size > 0) {
                // 需要解析实际的字符串数据来计数
                // 这里简化处理，假设平均每个 ID 占用一定字节
                this.report.statistics.meshes = Math.floor(meshIdSection.size / 20);
            }

            // 统计实体数量
            const entityIdSection = this.report.sections['eachEntityId'];
            if (entityIdSection && entityIdSection.hasData && entityIdSection.size > 0) {
                this.report.statistics.entities = Math.floor(entityIdSection.size / 20);
            }

            // 统计顶点数量
            const positionsSection = this.report.sections['positions'];
            if (positionsSection && positionsSection.hasData && positionsSection.size > 0) {
                // 每个顶点 3 个 float32 (x, y, z) = 12 bytes
                this.report.statistics.vertices = Math.floor(positionsSection.size / 12);
            }

            // 统计三角形数量
            const indicesSection = this.report.sections['indices'];
            if (indicesSection && indicesSection.hasData && indicesSection.size > 0) {
                // 每个索引 1 个 uint32 = 4 bytes，每个三角形 3 个索引
                const indexCount = Math.floor(indicesSection.size / 4);
                this.report.statistics.triangles = Math.floor(indexCount / 3);
            }

            console.log(`\n🔢 几何统计:`);
            console.log(`   - 几何体: ${this.report.statistics.geometries}`);
            console.log(`   - 网格: ${this.report.statistics.meshes}`);
            console.log(`   - 实体: ${this.report.statistics.entities}`);
            console.log(`   - 顶点: ${this.report.statistics.vertices.toLocaleString()}`);
            console.log(`   - 三角形: ${this.report.statistics.triangles.toLocaleString()}`);

        } catch (error) {
            this.report.warnings.push(`统计几何数据失败: ${error.message}`);
        }
    }

    /**
     * 最终验证
     */
    finalValidation() {
        // 检查是否有致命错误
        if (this.report.errors.length > 0) {
            this.report.valid = false;
            console.log(`\n❌ 验证失败 (${this.report.errors.length} 个错误)`);
            this.report.errors.forEach(err => console.log(`   - ${err}`));
        } else {
            this.report.valid = true;
            console.log(`\n✅ 验证通过`);
        }

        // 显示警告
        if (this.report.warnings.length > 0) {
            console.log(`\n⚠️  警告 (${this.report.warnings.length} 个):`);
            this.report.warnings.forEach(warn => console.log(`   - ${warn}`));
        }

        // 检查是否有几何数据
        if (this.report.statistics.geometries === 0 || this.report.statistics.vertices === 0) {
            this.report.warnings.push('文件中没有几何数据');
            console.log(`\n⚠️  文件中没有几何数据`);
        }
    }

    /**
     * 格式化文件大小
     */
    formatFileSize(bytes) {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(2)} KB`;
        return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
    }

    /**
     * 保存报告为 JSON
     */
    saveReport(outputPath) {
        const reportJson = JSON.stringify(this.report, null, 2);
        fs.writeFileSync(outputPath, reportJson);
        console.log(`\n💾 报告已保存: ${outputPath}`);
    }
}

// 主函数
async function main() {
    const args = process.argv.slice(2);
    
    if (args.length === 0) {
        console.log('用法: node validate_xkt_model.js <xkt文件路径> [报告输出路径]');
        console.log('');
        console.log('示例:');
        console.log('  node validate_xkt_model.js output/test/21491_18946.xkt');
        console.log('  node validate_xkt_model.js output/test/21491_18946.xkt report.json');
        process.exit(1);
    }

    const xktFilePath = args[0];
    const reportPath = args[1] || xktFilePath.replace('.xkt', '_validation_report.json');

    console.log('🔍 XKT 模型验证工具\n');
    console.log('═'.repeat(60));

    const validator = new XKTValidator(xktFilePath);
    const report = await validator.validate();

    console.log('═'.repeat(60));

    // 保存报告
    validator.saveReport(reportPath);

    // 返回退出码
    process.exit(report.valid ? 0 : 1);
}

main().catch(error => {
    console.error('❌ 验证过程出错:', error);
    process.exit(1);
});

