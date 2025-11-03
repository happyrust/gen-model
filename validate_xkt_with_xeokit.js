#!/usr/bin/env node

/**
 * XKT 模型验证脚本 - 使用 xeokit 解析逻辑
 * 
 * 功能：
 * 1. 解析 XKT 文件（支持 v10, v11, v12）
 * 2. 提取几何统计信息
 * 3. 验证文件完整性
 * 4. 生成详细的验证报告
 * 5. 与 OBJ 格式进行对比
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
            errors: [],
            warnings: [],
            statistics: {
                geometries: 0,
                meshes: 0,
                entities: 0,
                vertices: 0,
                triangles: 0,
                edges: 0
            },
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
            
            // 2. 解析文件头
            this.parseHeader();
            
            // 3. 解压缩数据（如果需要）
            this.decompressData();
            
            // 4. 解析 XKT 数据
            this.parseXKTData();
            
            // 5. 计算统计信息
            this.calculateStatistics();
            
            // 6. 最终验证
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

        if (this.buffer.length < 8) {
            throw new Error(`文件太小 (${this.buffer.length} bytes)，不是有效的 XKT 文件`);
        }
    }

    /**
     * 解析文件头
     * 参考 xeokit-sdk 的 _parseModel 方法
     */
    parseHeader() {
        const dataView = new DataView(this.buffer.buffer, this.buffer.byteOffset, this.buffer.byteLength);
        
        // 读取第一个 uint32
        const firstUint = dataView.getUint32(0, true);
        
        // 提取版本号（低 31 位）
        this.report.version = firstUint & 0x7fffffff;
        
        // 提取压缩标志（最高位）
        // v11 及以下：总是压缩
        // v12 及以上：由最高位决定
        if (this.report.version < 11) {
            this.report.compressed = true;
        } else if (this.report.version >= 12) {
            this.report.compressed = (firstUint >>> 31) === 1;
        } else {
            this.report.compressed = true;
        }

        console.log(`📦 XKT 版本: v${this.report.version}`);
        console.log(`🗜️  压缩状态: ${this.report.compressed ? '是' : '否'}`);
    }

    /**
     * 解压缩数据
     */
    decompressData() {
        if (!this.report.compressed) {
            // 未压缩，直接使用原始数据
            this.dataArray = new Uint8Array(this.buffer.buffer, this.buffer.byteOffset, this.buffer.byteLength);
            this.elements = [this.dataArray];
            return;
        }

        // 压缩格式：解析数据段
        const dataView = new DataView(this.buffer.buffer, this.buffer.byteOffset, this.buffer.byteLength);
        const dataArray = new Uint8Array(this.buffer.buffer, this.buffer.byteOffset, this.buffer.byteLength);

        // 读取段数量
        const numElements = dataView.getUint32(4, true);
        console.log(`📊 数据段数量: ${numElements}`);

        this.elements = [];
        let byteOffset = (numElements + 2) * 4;

        // 解析每个段
        for (let i = 0; i < numElements; i++) {
            const elementSize = dataView.getUint32((i + 2) * 4, true);
            const elementData = dataArray.subarray(byteOffset, byteOffset + elementSize);

            // 尝试解压缩
            try {
                const decompressed = pako.inflate(elementData);
                this.elements.push(decompressed);
            } catch (e) {
                // 如果解压失败，可能是未压缩的数据
                this.elements.push(elementData);
            }

            byteOffset += elementSize;
        }
    }

    /**
     * 解析 XKT 数据
     * 根据版本选择不同的解析逻辑
     */
    parseXKTData() {
        const version = this.report.version;
        
        if (version === 10) {
            this.parseXKTV10();
        } else if (version === 11) {
            this.parseXKTV11();
        } else if (version === 12) {
            this.parseXKTV12();
        } else {
            this.report.warnings.push(`未知的 XKT 版本: ${version}，尝试使用 v12 解析器`);
            this.parseXKTV12();
        }
    }

    /**
     * 解析 XKT v10 格式
     */
    parseXKTV10() {
        // XKT v10 的数据段索引
        const METADATA_INDEX = 0;
        const POSITIONS_INDEX = 1;
        const NORMALS_INDEX = 2;
        const INDICES_INDEX = 3;
        const EDGE_INDICES_INDEX = 4;
        const MATRICES_INDEX = 5;
        const EACH_MESH_GEOMETRIES_PORTION_INDEX = 6;
        const EACH_MESH_MATERIAL_ATTRIBUTES_INDEX = 7;
        const EACH_MESH_MATERIAL_INDEX = 8;
        
        this.xktData = {
            metadata: this.elements[METADATA_INDEX] || new Uint8Array(0),
            positions: this.elements[POSITIONS_INDEX] || new Uint8Array(0),
            normals: this.elements[NORMALS_INDEX] || new Uint8Array(0),
            indices: this.elements[INDICES_INDEX] || new Uint8Array(0),
            edgeIndices: this.elements[EDGE_INDICES_INDEX] || new Uint8Array(0),
            matrices: this.elements[MATRICES_INDEX] || new Uint8Array(0),
            eachMeshGeometriesPortion: this.elements[EACH_MESH_GEOMETRIES_PORTION_INDEX] || new Uint8Array(0),
            eachMeshMaterialAttributes: this.elements[EACH_MESH_MATERIAL_ATTRIBUTES_INDEX] || new Uint8Array(0),
            eachMeshMaterial: this.elements[EACH_MESH_MATERIAL_INDEX] || new Uint8Array(0)
        };
    }

    /**
     * 解析 XKT v11 格式
     */
    parseXKTV11() {
        // v11 与 v10 类似，但有一些额外的段
        this.parseXKTV10(); // 先使用 v10 的解析逻辑
    }

    /**
     * 解析 XKT v12 格式
     * 根据 gen-xkt/docs/xkt_v12.md 的段顺序
     */
    parseXKTV12() {
        // XKT v12 的数据段索引（29 个段，索引从 0 开始）
        const indices = {
            METADATA: 0,                                    // 1) metadata
            TEXTURE_DATA: 1,                                // 2) textureData
            EACH_TEXTURE_DATA_PORTION: 2,                   // 3) eachTextureDataPortion
            EACH_TEXTURE_ATTRIBUTES: 3,                     // 4) eachTextureAttributes
            POSITIONS: 4,                                   // 5) positions
            NORMALS: 5,                                     // 6) normals
            COLORS: 6,                                      // 7) colors
            UVS: 7,                                         // 8) uvs
            INDICES: 8,                                     // 9) indices
            EDGE_INDICES: 9,                                // 10) edgeIndices
            EACH_TEXTURE_SET_TEXTURES: 10,                  // 11) eachTextureSetTextures
            MATRICES: 11,                                   // 12) matrices
            REUSED_GEOMETRIES_DECODE_MATRIX: 12,            // 13) reusedGeometriesDecodeMatrix
            EACH_GEOMETRY_PRIMITIVE_TYPE: 13,               // 14) eachGeometryPrimitiveType
            EACH_GEOMETRY_AXIS_LABEL: 14,                   // 15) eachGeometryAxisLabel
            EACH_GEOMETRY_POSITIONS_PORTION: 15,            // 16) eachGeometryPositionsPortion
            EACH_GEOMETRY_NORMALS_PORTION: 16,              // 17) eachGeometryNormalsPortion
            EACH_GEOMETRY_COLORS_PORTION: 17,               // 18) eachGeometryColorsPortion
            EACH_GEOMETRY_UVS_PORTION: 18,                  // 19) eachGeometryUVsPortion
            EACH_GEOMETRY_INDICES_PORTION: 19,              // 20) eachGeometryIndicesPortion
            EACH_GEOMETRY_EDGE_INDICES_PORTION: 20,         // 21) eachGeometryEdgeIndicesPortion
            EACH_MESH_GEOMETRIES_PORTION: 21,               // 22) eachMeshGeometriesPortion
            EACH_MESH_MATRICES_PORTION: 22,                 // 23) eachMeshMatricesPortion
            EACH_MESH_TEXTURE_SET: 23,                      // 24) eachMeshTextureSet
            EACH_MESH_MATERIAL_ATTRIBUTES: 24,              // 25) eachMeshMaterialAttributes
            EACH_ENTITY_ID: 25,                             // 26) eachEntityId
            EACH_ENTITY_MESHES_PORTION: 26,                 // 27) eachEntityMeshesPortion
            EACH_TILE_AABB: 27,                             // 28) eachTileAABB
            EACH_TILE_ENTITIES_PORTION: 28                  // 29) eachTileEntitiesPortion
        };

        this.xktData = {};
        for (const [name, index] of Object.entries(indices)) {
            this.xktData[name] = this.elements[index] || new Uint8Array(0);
        }
    }

    /**
     * 计算统计信息
     */
    calculateStatistics() {
        const stats = this.report.statistics;

        // 计算几何体数量（从 EACH_GEOMETRY_POSITIONS_PORTION）
        // 这个段存储的是 Uint32Array，每个几何体只有 1 个 uint32（起始偏移量）
        if (this.xktData.EACH_GEOMETRY_POSITIONS_PORTION && this.xktData.EACH_GEOMETRY_POSITIONS_PORTION.length > 0) {
            // 创建 Uint32Array 视图来读取数据
            const uint32Array = new Uint32Array(
                this.xktData.EACH_GEOMETRY_POSITIONS_PORTION.buffer,
                this.xktData.EACH_GEOMETRY_POSITIONS_PORTION.byteOffset,
                this.xktData.EACH_GEOMETRY_POSITIONS_PORTION.byteLength / 4
            );
            stats.geometries = uint32Array.length; // 每个几何体 1 个 uint32（偏移量）
        }

        // 计算网格数量（从 EACH_MESH_GEOMETRIES_PORTION）
        // 每个 mesh 有 1 个 uint32：指向几何体的索引
        if (this.xktData.EACH_MESH_GEOMETRIES_PORTION && this.xktData.EACH_MESH_GEOMETRIES_PORTION.length > 0) {
            stats.meshes = this.xktData.EACH_MESH_GEOMETRIES_PORTION.length / 4; // 1 * 4 bytes
        }

        // 计算实体数量（从 EACH_ENTITY_ID）
        // EACH_ENTITY_ID 是 JSON 数组（ASCII 转义后压缩）
        if (this.xktData.EACH_ENTITY_ID && this.xktData.EACH_ENTITY_ID.length > 0) {
            try {
                const decoder = new TextDecoder('utf-8');
                const entityIdsStr = decoder.decode(this.xktData.EACH_ENTITY_ID);
                const entityIds = JSON.parse(entityIdsStr);
                if (Array.isArray(entityIds)) {
                    stats.entities = entityIds.length;
                }
            } catch (e) {
                console.warn(`解析 EACH_ENTITY_ID 失败: ${e.message}`);
            }
        }

        // 计算顶点数（从 POSITIONS 段）
        // POSITIONS 是 Uint16Array（量化后的顶点），每个顶点 3 个 uint16
        if (this.xktData.POSITIONS && this.xktData.POSITIONS.length > 0) {
            stats.vertices = Math.floor(this.xktData.POSITIONS.length / 6); // 3 * 2 bytes
        }

        // 计算三角形数（从 INDICES 段）
        // INDICES 是 Uint32Array，每个三角形 3 个索引
        if (this.xktData.INDICES && this.xktData.INDICES.length > 0) {
            stats.triangles = Math.floor(this.xktData.INDICES.length / 12); // 3 indices * 4 bytes
        }

        // 计算边数（从 EDGE_INDICES 段）
        // EDGE_INDICES 是 Uint32Array，每条边 2 个索引
        if (this.xktData.EDGE_INDICES && this.xktData.EDGE_INDICES.length > 0) {
            stats.edges = Math.floor(this.xktData.EDGE_INDICES.length / 8); // 2 indices * 4 bytes
        }
    }

    /**
     * 最终验证
     */
    finalValidation() {
        const stats = this.report.statistics;
        
        // 检查是否有几何数据
        if (stats.vertices === 0 && stats.triangles === 0) {
            this.report.errors.push('文件中没有几何数据（顶点和三角形均为0）');
        }
        
        // 详细验证：检查每个实体是否都有mesh
        this.validateEntityMeshMapping();
        
        // 验证成功
        if (this.report.errors.length === 0) {
            this.report.valid = true;
        }
    }
    
    /**
     * 验证实体和网格的映射关系
     */
    validateEntityMeshMapping() {
        try {
            // 解析 EACH_ENTITY_MESHES_PORTION
            // 这个段存储的是每个实体的mesh索引范围
            // 格式：每个实体有1个uint32表示mesh数量，然后是uint32数组表示mesh索引
            if (this.xktData.EACH_ENTITY_MESHES_PORTION && 
                this.xktData.EACH_ENTITY_MESHES_PORTION.length > 0 &&
                this.xktData.EACH_ENTITY_ID && 
                this.xktData.EACH_ENTITY_ID.length > 0) {
                
                // 解析实体ID
                const decoder = new TextDecoder('utf-8');
                const entityIdsStr = decoder.decode(this.xktData.EACH_ENTITY_ID);
                const entityIds = JSON.parse(entityIdsStr);
                
                if (!Array.isArray(entityIds)) {
                    this.report.errors.push('EACH_ENTITY_ID 格式错误：不是数组');
                    return;
                }
                
                // 解析 EACH_ENTITY_MESHES_PORTION
                const entityMeshesBuffer = this.xktData.EACH_ENTITY_MESHES_PORTION;
                const entityMeshesArray = new Uint32Array(
                    entityMeshesBuffer.buffer,
                    entityMeshesBuffer.byteOffset,
                    entityMeshesBuffer.byteLength / 4
                );
                
                let entityIndex = 0;
                let bufferIndex = 0;
                const emptyEntities = [];
                
                while (bufferIndex < entityMeshesArray.length && entityIndex < entityIds.length) {
                    const numMeshes = entityMeshesArray[bufferIndex++];
                    
                    if (numMeshes === 0) {
                        emptyEntities.push({
                            entityId: entityIds[entityIndex],
                            index: entityIndex
                        });
                    }
                    
                    // 跳过mesh索引
                    bufferIndex += numMeshes;
                    entityIndex++;
                }
                
                if (emptyEntities.length > 0) {
                    const emptyCount = emptyEntities.length;
                    const totalCount = entityIds.length;
                    const emptyPercentage = ((emptyCount / totalCount) * 100).toFixed(2);
                    
                    this.report.warnings.push(
                        `发现 ${emptyCount} 个空实体（无mesh），占总数的 ${emptyPercentage}%`
                    );
                    
                    // 记录前10个空实体的ID
                    const sampleEmptyIds = emptyEntities.slice(0, 10).map(e => e.entityId);
                    this.report.metadata.emptyEntities = {
                        count: emptyCount,
                        total: totalCount,
                        percentage: emptyPercentage,
                        samples: sampleEmptyIds
                    };
                }
                
                // 验证mesh是否都有几何体
                this.validateMeshGeometryMapping();
            } else {
                this.report.warnings.push('无法验证实体-mesh映射：缺少必要的XKT数据段');
            }
        } catch (error) {
            this.report.warnings.push(`验证实体-mesh映射时出错: ${error.message}`);
        }
    }
    
    /**
     * 验证mesh和几何体的映射关系
     */
    validateMeshGeometryMapping() {
        try {
            // 解析 EACH_MESH_GEOMETRIES_PORTION
            // 每个mesh有1个uint32表示几何体索引
            if (this.xktData.EACH_MESH_GEOMETRIES_PORTION && 
                this.xktData.EACH_MESH_GEOMETRIES_PORTION.length > 0) {
                
                const meshGeomBuffer = this.xktData.EACH_MESH_GEOMETRIES_PORTION;
                const meshGeomArray = new Uint32Array(
                    meshGeomBuffer.buffer,
                    meshGeomBuffer.byteOffset,
                    meshGeomBuffer.byteLength / 4
                );
                
                // 获取几何体总数
                const totalGeometries = this.report.statistics.geometries;
                
                if (totalGeometries > 0) {
                    const invalidMeshes = [];
                    
                    for (let i = 0; i < meshGeomArray.length; i++) {
                        const geomIndex = meshGeomArray[i];
                        if (geomIndex >= totalGeometries) {
                            invalidMeshes.push({
                                meshIndex: i,
                                geometryIndex: geomIndex
                            });
                        }
                    }
                    
                    if (invalidMeshes.length > 0) {
                        this.report.errors.push(
                            `发现 ${invalidMeshes.length} 个mesh引用了无效的几何体索引`
                        );
                    }
                }
            }
        } catch (error) {
            this.report.warnings.push(`验证mesh-几何体映射时出错: ${error.message}`);
        }
    }

    /**
     * 生成报告
     */
    generateReport() {
        console.log('\n════════════════════════════════════════════════════════════');
        console.log(`📄 文件: ${this.report.fileName}`);
        console.log(`📦 大小: ${(this.report.fileSize / 1024).toFixed(2)} KB`);
        console.log(`🔢 版本: v${this.report.version}`);
        console.log(`🗜️  压缩: ${this.report.compressed ? '是' : '否'}`);
        console.log('\n📊 几何统计:');
        console.log(`   - 几何体: ${this.report.statistics.geometries}`);
        console.log(`   - 网格: ${this.report.statistics.meshes}`);
        console.log(`   - 实体: ${this.report.statistics.entities}`);
        console.log(`   - 顶点: ${this.report.statistics.vertices}`);
        console.log(`   - 三角形: ${this.report.statistics.triangles}`);
        console.log(`   - 边: ${this.report.statistics.edges}`);
        
        if (this.report.errors.length > 0) {
            console.log(`\n❌ 验证失败 (${this.report.errors.length} 个错误)`);
            this.report.errors.forEach(err => console.log(`   - ${err}`));
        } else {
            console.log('\n✅ 验证成功');
        }
        
        if (this.report.warnings.length > 0) {
            console.log(`\n⚠️  警告 (${this.report.warnings.length} 个):`);
            this.report.warnings.forEach(warn => console.log(`   - ${warn}`));
        }
        
        // 显示空实体信息
        if (this.report.metadata && this.report.metadata.emptyEntities) {
            const info = this.report.metadata.emptyEntities;
            console.log(`\n📋 空实体统计:`);
            console.log(`   - 空实体数量: ${info.count} / ${info.total}`);
            console.log(`   - 空实体比例: ${info.percentage}%`);
            if (info.samples && info.samples.length > 0) {
                console.log(`   - 示例（前10个）:`);
                info.samples.forEach((id, i) => console.log(`     ${i + 1}. ${id}`));
            }
        }
        
        console.log('════════════════════════════════════════════════════════════\n');
    }
}

// 主函数
async function main() {
    const args = process.argv.slice(2);
    
    if (args.length === 0) {
        console.error('用法: node validate_xkt_with_xeokit.js <xkt文件路径> [输出JSON路径]');
        process.exit(1);
    }
    
    const xktFile = args[0];
    const outputJson = args[1];
    
    console.log('🔍 XKT 模型验证工具\n');
    
    const validator = new XKTValidator(xktFile);
    const report = await validator.validate();
    
    validator.generateReport();
    
    // 保存 JSON 报告
    if (outputJson) {
        fs.writeFileSync(outputJson, JSON.stringify(report, null, 2));
        console.log(`💾 报告已保存: ${outputJson}`);
    }
    
    process.exit(report.valid ? 0 : 1);
}

main().catch(err => {
    console.error('错误:', err);
    process.exit(1);
});

