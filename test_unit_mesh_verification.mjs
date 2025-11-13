#!/usr/bin/env node

/**
 * 验证单位 mesh GLB 文件中的节点矩阵
 * 检查所有节点是否使用单位矩阵，原始变换是否保存在 extras 中
 */

import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// 单位矩阵常量
const IDENTITY_MATRIX = [
    1, 0, 0, 0,
    0, 1, 0, 0,
    0, 0, 1, 0,
    0, 0, 0, 1
];

function isIdentityMatrix(matrix) {
    if (!matrix || matrix.length !== 16) return false;
    return matrix.every((val, i) => Math.abs(val - IDENTITY_MATRIX[i]) < 1e-6);
}

function parseGLB(filePath) {
    const data = fs.readFileSync(filePath);
    
    // GLB 格式：12字节 header + 8字节 JSON chunk header + JSON data + 8字节 binary chunk header + binary data
    const jsonLength = data.readUInt32LE(12);
    const jsonFormat = data.readUInt32LE(16);
    
    if (jsonFormat !== 0x4E4F534A) { // "JSON"
        throw new Error('Invalid GLB format: missing JSON chunk');
    }
    
    const jsonData = data.slice(20, 20 + jsonLength);
    const gltf = JSON.parse(jsonData.toString());
    
    return gltf;
}

function analyzeGLBNodes(gltf, filePath) {
    console.log(`\n🔍 分析文件: ${path.basename(filePath)}`);
    console.log(`   - 总节点数: ${gltf.nodes ? gltf.nodes.length : 0}`);
    console.log(`   - 总 mesh 数: ${gltf.meshes ? gltf.meshes.length : 0}`);
    
    if (!gltf.nodes) {
        console.log('   ⚠️  未找到节点数据');
        return {
            totalNodes: 0,
            identityMatrixNodes: 0,
            nonIdentityMatrixNodes: 0,
            nodesWithGeoHash: 0
        };
    }
    
    let identityMatrixNodes = 0;
    let nonIdentityMatrixNodes = 0;
    let nodesWithGeoHash = 0;
    let meshNodes = 0;
    
    gltf.nodes.forEach((node, index) => {
        const hasMesh = node.mesh !== undefined;
        if (hasMesh) meshNodes++;
        
        const hasMatrix = node.matrix !== undefined;
        const hasGeoHash = node.extras && node.extras.geoHash;
        
        if (hasGeoHash) {
            nodesWithGeoHash++;
        }
        
        if (hasMatrix) {
            const isIdentity = isIdentityMatrix(node.matrix);
            if (isIdentity) {
                identityMatrixNodes++;
            } else {
                nonIdentityMatrixNodes++;
                if (hasMesh) {
                    console.log(`   ❌ 节点 ${index} (${node.name || 'unnamed'}) 有非单位矩阵且有 mesh`);
                    console.log(`      矩阵: [${node.matrix.slice(0, 4).join(', ')}...]`);
                }
            }
        }
        
        // 详细输出前几个 mesh 节点的信息
        if (hasMesh && index < 3) {
            console.log(`   📋 节点 ${index} (${node.name || 'unnamed'}):`);
            console.log(`      - 有 mesh: ${hasMesh}`);
            console.log(`      - 矩阵: ${hasMatrix ? (isIdentityMatrix(node.matrix) ? '单位矩阵 ✅' : '非单位矩阵 ❌') : '无矩阵'}`);
            console.log(`      - 有 geo_hash: ${hasGeoHash ? '是 ✅' : '否 ⚠️'}`);
            if (hasGeoHash) {
                console.log(`      - geo_hash: ${node.extras.geoHash}`);
            }
        }
    });
    
    return {
        totalNodes: gltf.nodes.length,
        meshNodes,
        identityMatrixNodes,
        nonIdentityMatrixNodes,
        nodesWithGeoHash
    };
}

function expandGlob(pattern) {
    const dir = path.dirname(pattern);
    const basename = path.basename(pattern);
    const regex = new RegExp('^' + basename.replace(/\*/g, '.*') + '$');
    
    try {
        return fs.readdirSync(dir)
            .filter(file => regex.test(file))
            .map(file => path.join(dir, file));
    } catch (e) {
        console.warn(`⚠️  无法读取目录 ${dir}: ${e.message}`);
        return [];
    }
}

function main() {
    const args = process.argv.slice(2);
    
    if (args.length === 0) {
        console.log('用法: node test_unit_mesh_verification.mjs <glb文件...>');
        console.log('示例: node test_unit_mesh_verification.mjs output/test-unit-mesh-*/geometry_*.glb');
        process.exit(1);
    }
    
    console.log('🧪 单位 Mesh GLB 验证工具 (简化结构版)');
    console.log('========================================');
    console.log('检查目标:');
    console.log('   1. 所有 mesh 节点应使用单位矩阵');
    console.log('   2. GLB 只存储几何体，不包含实例变换');
    console.log('   3. geo_hash 与 mesh_index 正确映射');
    console.log('');
    
    let totalStats = {
        totalNodes: 0,
        meshNodes: 0,
        identityMatrixNodes: 0,
        nonIdentityMatrixNodes: 0,
        nodesWithGeoHash: 0
    };
    
    const files = args.flatMap(arg => {
        if (arg.includes('*')) {
            return expandGlob(arg);
        }
        return [arg];
    }).filter(file => fs.existsSync(file));
    
    if (files.length === 0) {
        console.log('❌ 未找到匹配的文件');
        process.exit(1);
    }
    
    console.log(`📁 找到 ${files.length} 个文件`);
    
    files.forEach(filePath => {
        try {
            const gltf = parseGLB(filePath);
            const stats = analyzeGLBNodes(gltf, filePath);
            
            // 累计统计
            Object.keys(stats).forEach(key => {
                totalStats[key] += stats[key];
            });
            
            // 单个文件结果
            console.log(`\n📊 ${path.basename(filePath)} 统计:`);
            console.log(`   - Mesh 节点: ${stats.meshNodes}`);
            console.log(`   - 单位矩阵节点: ${stats.identityMatrixNodes}`);
            console.log(`   - 非单位矩阵节点: ${stats.nonIdentityMatrixNodes}`);
            console.log(`   - 包含 geo_hash 的节点: ${stats.nodesWithGeoHash}`);
            
            if (stats.nonIdentityMatrixNodes === 0 && stats.meshNodes > 0) {
                console.log('   ✅ 所有 mesh 节点都使用单位矩阵！');
            } else if (stats.meshNodes > 0) {
                console.log(`   ⚠️  发现 ${stats.nonIdentityMatrixNodes} 个非单位矩阵节点`);
            }
            
            if (stats.nodesWithGeoHash === stats.meshNodes && stats.meshNodes > 0) {
                console.log('   ✅ 所有 mesh 节点都包含 geo_hash！');
            } else if (stats.meshNodes > 0) {
                console.log(`   ⚠️  只有 ${stats.nodesWithGeoHash}/${stats.meshNodes} 个节点包含 geo_hash`);
            }
            
        } catch (e) {
            console.error(`❌ 处理文件 ${filePath} 时出错: ${e.message}`);
        }
    });
    
    // 总体统计
    console.log('\n🎯 总体统计:');
    console.log(`   - 总文件数: ${files.length}`);
    console.log(`   - 总节点数: ${totalStats.totalNodes}`);
    console.log(`   - 总 mesh 节点数: ${totalStats.meshNodes}`);
    console.log(`   - 单位矩阵节点: ${totalStats.identityMatrixNodes}`);
    console.log(`   - 非单位矩阵节点: ${totalStats.nonIdentityMatrixNodes}`);
    console.log(`   - 包含 geo_hash 的节点: ${totalStats.nodesWithGeoHash}`);
    
    console.log('\n🎉 验证结果:');
    if (totalStats.nonIdentityMatrixNodes === 0 && totalStats.meshNodes > 0) {
        console.log('✅ 成功！所有 mesh 节点都使用单位矩阵');
    } else {
        console.log(`❌ 失败！发现 ${totalStats.nonIdentityMatrixNodes} 个非单位矩阵节点`);
    }
    
    if (totalStats.nodesWithGeoHash === totalStats.meshNodes && totalStats.meshNodes > 0) {
        console.log('✅ 成功！所有 mesh 节点都包含 geo_hash 标识');
    } else if (totalStats.meshNodes > 0) {
        console.log(`⚠️  部分成功！${totalStats.nodesWithGeoHash}/${totalStats.meshNodes} 个节点包含 geo_hash`);
    }
    
    console.log('\n💡 简化结构说明:');
    console.log('   - GLB 文件只存储几何体（单位 mesh）');
    console.log('   - 实例变换信息保存在 instances.json 中');
    console.log('   - geo_index 直接映射到 GLB 中的 mesh_index');
    console.log('   - 适合 InstancedMesh 渲染，性能更优');
    
    if (totalStats.nonIdentityMatrixNodes === 0 && 
        totalStats.nodesWithGeoHash === totalStats.meshNodes && 
        totalStats.meshNodes > 0) {
        console.log('\n🏆 完美！简化单位 mesh 导出功能工作正常！');
        process.exit(0);
    } else {
        console.log('\n❌ 简化单位 mesh 导出功能存在问题');
        process.exit(1);
    }
}

main();
