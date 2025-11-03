import fs from 'fs';
import pako from 'pako';

function decompressData(buffer) {
    try {
        const result = pako.inflate(new Uint8Array(buffer));
        return result;
    } catch (e) {
        console.error('解压失败:', e.message, buffer);
        return null;
    }
}

function parseXKTV12(buffer) {
    const dataView = new DataView(buffer);
    let offset = 0;

    // 读取版本号
    const version = dataView.getUint32(offset, true);
    offset += 4;
    console.log(`版本: 0x${version.toString(16)} (${version & 0xFF})`);
    
    const isCompressed = (version & 0x80000000) !== 0;
    console.log(`压缩: ${isCompressed}`);

    // 读取段数量
    const segmentCount = dataView.getUint32(offset, true);
    offset += 4;
    console.log(`段数量: ${segmentCount}`);

    // 读取 size 表
    const sizes = [];
    for (let i = 0; i < segmentCount; i++) {
        sizes.push(dataView.getUint32(offset, true));
        offset += 4;
    }

    console.log(`\n各段大小:`);
    sizes.forEach((size, i) => {
        console.log(`  段 ${i}: ${size} 字节`);
    });

    // 定义段名称
    const segmentNames = [
        'metadata',
        'textureData',
        'eachTextureDataPortion',
        'eachTextureAttributes',
        'positions (u16)',
        'normals (i8)',
        'colors',
        'uvs',
        'indices (u32)',
        'edgeIndices (u32)',
        'eachTextureSetTextures',
        'matrices',
        'reusedGeometriesDecodeMatrix',
        'eachGeometryPrimitiveType',
        'eachGeometryAxisLabel',
        'eachGeometryPositionsPortion',
        'eachGeometryNormalsPortion',
        'eachGeometryColorsPortion',
        'eachGeometryUvsPortion',
        'eachGeometryIndicesPortion',
        'eachGeometryEdgeIndicesPortion',
        'eachMeshGeometriesPortion',
        'eachMeshMatricesPortion',
        'eachMeshTextureSet',
        'eachMeshMaterialAttributes',
        'eachEntityId',
        'eachEntityMeshesPortion',
        'eachTileAabb',
        'eachTileEntitiesPortion'
    ];

    // 解压并解析关键段
    const positionsData = decompressData(new Uint8Array(buffer, offset, sizes[4]));
    let positionsOffset = offset;
    offset += sizes[4];
    
    const indicesData = decompressData(new Uint8Array(buffer, offset, sizes[8]));
    let indicesOffset = offset;
    offset += sizes[8];
    
    const geometryPositionsPortionData = decompressData(new Uint8Array(buffer, offset, sizes[15]));
    offset += sizes[15];
    
    const geometryIndicesPortionData = decompressData(new Uint8Array(buffer, offset, sizes[19]));
    offset += sizes[19];

    console.log(`\n=== POSITIONS ===`);
    console.log(`压缩后大小: ${sizes[4]} 字节`);
    console.log(`解压后大小: ${positionsData.length} 字节`);
    
    // POSITIONS 是 u16 数组
    const positionsArray = new Uint16Array(positionsData.buffer, positionsData.byteOffset, positionsData.byteLength / 2);
    console.log(`实际元素数: ${positionsArray.length}`);
    console.log(`顶点数: ${positionsArray.length / 3}`);
    console.log(`前 12 个值:`, Array.from(positionsArray.slice(0, 12)));

    console.log(`\n=== INDICES ===`);
    console.log(`压缩后大小: ${sizes[8]} 字节`);
    console.log(`解压后大小: ${indicesData.length} 字节`);
    
    // INDICES 是 u32 数组
    const indicesArray = new Uint32Array(indicesData.buffer, indicesData.byteOffset, indicesData.byteLength / 4);
    console.log(`实际元素数: ${indicesArray.length}`);
    console.log(`三角形数: ${indicesArray.length / 3}`);
    console.log(`索引范围: [${Math.min(...indicesArray)}..=${Math.max(...indicesArray)}]`);
    console.log(`前 12 个索引:`, Array.from(indicesArray.slice(0, 12)));

    console.log(`\n=== EACH_GEOMETRY_POSITIONS_PORTION ===`);
    console.log(`压缩后大小: ${sizes[15]} 字节`);
    console.log(`解压后大小: ${geometryPositionsPortionData.length} 字节`);
    
    const geometryPositionsPortion = new Uint32Array(geometryPositionsPortionData.buffer, geometryPositionsPortionData.byteOffset, geometryPositionsPortionData.byteLength / 4);
    console.log(`几何体数量: ${geometryPositionsPortion.length}`);
    geometryPositionsPortion.forEach((offset, idx) => {
        console.log(`  几何体 ${idx}: offset=${offset}`);
    });

    console.log(`\n=== EACH_GEOMETRY_INDICES_PORTION ===`);
    console.log(`压缩后大小: ${sizes[19]} 字节`);
    console.log(`解压后大小: ${geometryIndicesPortionData.length} 字节`);
    
    const geometryIndicesPortion = new Uint32Array(geometryIndicesPortionData.buffer, geometryIndicesPortionData.byteOffset, geometryIndicesPortionData.byteLength / 4);
    console.log(`几何体数量: ${geometryIndicesPortion.length}`);
    geometryIndicesPortion.forEach((offset, idx) => {
        console.log(`  几何体 ${idx}: offset=${offset}`);
    });

    // 验证索引
    console.log(`\n=== 索引验证 ===`);
    for (let i = 0; i < geometryPositionsPortion.length; i++) {
        const startPos = geometryPositionsPortion[i];
        const endPos = i + 1 < geometryPositionsPortion.length ? geometryPositionsPortion[i + 1] : positionsArray.length;
        const startVertex = startPos / 3;
        const endVertex = endPos / 3;
        
        console.log(`\n几何体 ${i}:`);
        console.log(`  顶点范围: [${startVertex}, ${endVertex}) (${endVertex - startVertex} 个顶点)`);
        
        if (i < geometryIndicesPortion.length) {
            const startIdx = geometryIndicesPortion[i];
            const endIdx = i + 1 < geometryIndicesPortion.length ? geometryIndicesPortion[i + 1] : indicesArray.length;
            const geometryIndices = indicesArray.slice(startIdx, endIdx);
            
            if (geometryIndices.length > 0) {
                const minIdx = Math.min(...geometryIndices);
                const maxIdx = Math.max(...geometryIndices);
                
                console.log(`  索引范围: [${minIdx}..=${maxIdx}]`);
                console.log(`  索引计数: ${geometryIndices.length}`);
                
                if (minIdx < startVertex || maxIdx >= endVertex) {
                    console.log(`  ❌ 错误: 索引超出顶点范围!`);
                    console.log(`     期望范围: [0, ${endVertex - startVertex})`);
                    console.log(`     实际索引: [${minIdx}, ${maxIdx}]`);
                } else {
                    console.log(`  ✅ 索引在有效范围内`);
                }
            }
        }
    }
}

// 主程序
const filename = process.argv[2] || '_火车卸车鹤管_B1.xkt';
const buffer = fs.readFileSync(filename);
parseXKTV12(buffer.buffer);
