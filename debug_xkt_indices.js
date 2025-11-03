#!/usr/bin/env node

/**
 * 调试 XKT 索引问题
 * 检查索引是否超出顶点缓冲区
 */

import fs from 'fs';
import pako from 'pako';

const filePath = '_火车卸车鹤管_B1.xkt';

console.log('🔍 分析 XKT 索引数据...\n');

// 读取文件
const buffer = fs.readFileSync(filePath);
const dataView = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);

// 解析文件头
const firstUint = dataView.getUint32(0, true);
const version = firstUint & 0x7fffffff;

console.log(`📦 XKT v${version}\n`);

// 解压缩
const dataArray = new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
const numElements = dataView.getUint32(4, true);
const elements = [];
let byteOffset = (numElements + 2) * 4;

for (let i = 0; i < numElements; i++) {
    const elementSize = dataView.getUint32((i + 2) * 4, true);
    const elementData = dataArray.subarray(byteOffset, byteOffset + elementSize);
    try {
        const decompressed = pako.inflate(elementData);
        elements.push(decompressed);
    } catch (e) {
        elements.push(elementData);
    }
    byteOffset += elementSize;
}

// 获取关键数据段
const POSITIONS = elements[4];  // uint16 array (量化)
const NORMALS = elements[5];   // uint16 array
const INDICES = elements[8];    // uint32 array
const EACH_GEOMETRY_POSITIONS_PORTION = elements[15];  // uint32 array (字节偏移)
const EACH_GEOMETRY_INDICES_PORTION = elements[19];     // uint32 array (索引偏移)

// POSITIONS 是 uint16 数组，每个顶点 3 个组件，每个组件 2 bytes
// 所以字节偏移 / 6 = 顶点索引
const positionsByteOffsets = new Uint32Array(
    EACH_GEOMETRY_POSITIONS_PORTION.buffer,
    EACH_GEOMETRY_POSITIONS_PORTION.byteOffset,
    EACH_GEOMETRY_POSITIONS_PORTION.byteLength / 4
);

const indicesOffsets = new Uint32Array(
    EACH_GEOMETRY_INDICES_PORTION.buffer,
    EACH_GEOMETRY_INDICES_PORTION.byteOffset,
    EACH_GEOMETRY_INDICES_PORTION.byteLength / 4
);

const indicesArray = new Uint32Array(
    INDICES.buffer,
    INDICES.byteOffset,
    INDICES.byteLength / 4
);

console.log(`📊 几何体总数: ${positionsByteOffsets.length}`);
console.log(`📊 POSITIONS 字节偏移: ${Array.from(positionsByteOffsets).join(', ')}`);
console.log(`📊 INDICES 偏移: ${Array.from(indicesOffsets).join(', ')}\n`);

// 检查每个几何体
const totalVertices = POSITIONS.length / 6; // 每个顶点 6 bytes (3 * 2)
console.log(`📊 总顶点数 (字节/6): ${totalVertices}\n`);

// POSITIONS 字节偏移转换为顶点索引
const vertexOffsets = positionsByteOffsets.map(byteOffset => byteOffset / 6);
console.log(`📊 顶点偏移: ${vertexOffsets.map(v => Math.floor(v)).join(', ')}\n`);

let maxViolation = 0;
let violations = [];

for (let i = 0; i < vertexOffsets.length; i++) {
    const startVertex = Math.floor(vertexOffsets[i]);
    const endVertex = i < vertexOffsets.length - 1 ? Math.floor(vertexOffsets[i + 1]) : totalVertices;
    const vertexCount = endVertex - startVertex;
    
    // 获取该几何体的索引
    const startIndexPos = indicesOffsets[i];
    const endIndexPos = i < indicesOffsets.length - 1 ? indicesOffsets[i + 1] : indicesArray.length;
    const indices = Array.from(indicesArray.subarray(startIndexPos, endIndexPos));
    
    if (indices.length > 0) {
        const minIndex = Math.min(...indices);
        const maxIndex = Math.max(...indices);
        
        // 检查索引是否在几何体的顶点范围内
        const inRange = maxIndex < endVertex && minIndex >= startVertex;
        
        if (!inRange) {
            violations.push({
                geom: i,
                maxIndex,
                vertexRangeEnd: endVertex,
                violation: maxIndex >= endVertex ? maxIndex - endVertex + 1 : 0
            });
            maxViolation = Math.max(maxViolation, maxIndex >= endVertex ? maxIndex - endVertex + 1 : 0);
        }
        
        if (i < 5) {
            console.log(`Geometry ${i}: ${vertexCount} vertices [${startVertex}, ${endVertex}), ${indices.length} indices, index range [${minIndex}, ${maxIndex}]`);
            if (!inRange) {
                console.log(`  ❌ VIOLATION: max index ${maxIndex} >= vertex range end ${endVertex}`);
            }
        }
    }
}

console.log(`\n❌ 发现 ${violations.length} 个几何体有索引违规`);
if (violations.length > 0) {
    console.log(`   最大违规值: ${maxViolation}`);
    violations.forEach(v => {
        console.log(`   Geometry ${v.geom}: index ${v.maxIndex} >= ${v.vertexCount} vertices (超出 ${v.violation})`);
    });
} else {
    console.log(`✅ 所有索引都在有效范围内`);
}
