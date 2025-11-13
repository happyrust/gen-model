#!/usr/bin/env node

/**
 * 验证单位 mesh GLB 文件中的节点矩阵
 * 检查所有节点是否使用单位矩阵，原始变换是否保存在 extras 中
 */

const fs = require('fs');
const path = require('path');

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
            nodesWithOriginalTransform: 0
        };
    }
    
    let identityMatrixNodes = 0;
    let nonIdentityMatrixNodes = 0;
    let nodesWithOriginalTransform = 0;
    let meshNodes = 0;
    
    gltf.nodes.forEach((node, index) => {
        const hasMesh = node.mesh !== undefined;
        if (hasMesh) meshNodes++;
        
        const hasMatrix = node.matrix !== undefined;
        const hasOriginalTransform = node.extras && node.extras.originalTransform;
        
        if (hasOriginalTransform) {
            nodesWithOriginalTransform++;
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
            console.log(`      - 有原始变换: ${hasOriginalTransform ? '是 ✅' : '否 ⚠️'}`);
            if (hasOriginalTransform) {
                console.log(`      - 原始变换保存位置: extras.originalTransform`);
            }
        }
    });
    
    return {
        totalNodes: gltf.nodes.length,
        meshNodes,
        identityMatrixNodes,
        nonIdentityMatrixNodes,
        nodesWithOriginalTransform
    };
}

function main() {
    const args = process.argv.slice(2);
    
    if (args.length === 0) {
        console.log('用法: node test_unit_mesh_verification.js <glb文件...>');
        console.log('示例: node test_unit_mesh_verification.js output/test-unit-mesh-*/geometry_*.glb');
        process.exit(1);
    }
    
    console.log('🧪 单位 Mesh GLB 验证工具');
    console.log('========================');
    console.log('检查目标:');
    console.log('   1. 所有 mesh 节点应使用单位矩阵');
    console.log('   2. 原始变换应保存在 extras.originalTransform 中');
    console.log('');
    
    let totalStats = {
        totalNodes: 0,
        meshNodes: 0,
        identityMatrixNodes: 0,
        nonIdentityMatrixNodes: 0,
        nodesWithOriginalTransform: 0
    };
    
    const files = args.flatMap(arg => {
        if (arg.includes('*')) {
            // 简单的 glob 展开
            const dir = path.dirname(arg);
            const pattern = path.basename(arg);
            const regex = new RegExp('^' + pattern.replace(/\*/g, '.*') + '$');
            
            try {
                return fs.readdirSync(dir)
                    .filter(file => regex.test(file))
                    .map(file => path.join(dir, file));
            } catch (e) {
                console.warn(`⚠️  无法读取目录 ${dir}: ${e.message}`);
                return [];
            }
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
            console.log(`   - 保存原始变换的节点: ${stats.nodesWithOriginalTransform}`);
            
            if (stats.nonIdentityMatrixNodes === 0 && stats.meshNodes > 0) {
                console.log('   ✅ 所有 mesh 节点都使用单位矩阵！');
            } else if (stats.meshNodes > 0) {
                console.log(`   ⚠️  发现 ${stats.nonIdentityMatrixNodes} 个非单位矩阵节点`);
            }
            
            if (stats.nodesWithOriginalTransform === stats.meshNodes && stats.meshNodes > 0) {
                console.log('   ✅ 所有 mesh 节点都保存了原始变换！');
            } else if (stats.meshNodes > 0) {
                console.log(`   ⚠️  只有 ${stats.nodesWithOriginalTransform}/${stats.meshNodes} 个节点保存了原始变换`);
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
    console.log(`   - 保存原始变换的节点: ${totalStats.nodesWithOriginalTransform}`);
    
    console.log('\n🎉 验证结果:');
    if (totalStats.nonIdentityMatrixNodes === 0 && totalStats.meshNodes > 0) {
        console.log('✅ 成功！所有 mesh 节点都使用单位矩阵');
    } else {
        console.log(`❌ 失败！发现 ${totalStats.nonIdentityMatrixNodes} 个非单位矩阵节点`);
    }
    
    if (totalStats.nodesWithOriginalTransform === totalStats.meshNodes && totalStats.meshNodes > 0) {
        console.log('✅ 成功！所有 mesh 节点都保存了原始变换');
    } else if (totalStats.meshNodes > 0) {
        console.log(`⚠️  部分成功！${totalStats.nodesWithOriginalTransform}/${totalStats.meshNodes} 个节点保存了原始变换`);
    }
    
    if (totalStats.nonIdentityMatrixNodes === 0 && 
        totalStats.nodesWithOriginalTransform === totalStats.meshNodes && 
        totalStats.meshNodes > 0) {
        console.log('\n🏆 完美！单位 mesh 导出功能工作正常！');
        process.exit(0);
    } else {
        console.log('\n❌ 单位 mesh 导出功能存在问题');
        process.exit(1);
    }
}

if (require.main === module) {
    main();
}
