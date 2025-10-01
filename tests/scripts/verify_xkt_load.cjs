#!/usr/bin/env node
/**
 * 简化版 XKT v10 校验脚本。
 *
 * 我们复用 xeokit SDK 对 XKT v10 的 section 设计，逐段解压并做基础一致性检查，
 * 以确认 Rust 生成的文件能够被官方解析流程接受。
 */

const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const SECTION_ORDER = [
  "metadata",
  "textureData",
  "eachTextureDataPortion",
  "eachTextureAttributes",
  "positions",
  "normals",
  "colors",
  "uvs",
  "indices",
  "edgeIndices",
  "eachTextureSetTextures",
  "matrices",
  "reusedGeometriesDecodeMatrix",
  "eachGeometryPrimitiveType",
  "eachGeometryAxisLabel",
  "eachGeometryPositionsPortion",
  "eachGeometryNormalsPortion",
  "eachGeometryColorsPortion",
  "eachGeometryUVsPortion",
  "eachGeometryIndicesPortion",
  "eachGeometryEdgeIndicesPortion",
  "eachMeshGeometriesPortion",
  "eachMeshMatricesPortion",
  "eachMeshTextureSet",
  "eachMeshMaterialAttributes",
  "eachEntityId",
  "eachEntityMeshesPortion",
  "eachTileAABB",
  "eachTileEntitiesPortion",
];

function usage() {
  console.error("用法: node verify_xkt_load.js <path/to/file.xkt>");
  process.exit(1);
}

function inflateSection(name, buffer) {
  if (!buffer || buffer.length === 0) {
    return Buffer.alloc(0);
  }
  try {
    return zlib.inflateSync(buffer);
  } catch (err) {
    throw new Error(`section '${name}' 解压失败: ${err.message}`);
  }
}

function checkAlignment(name, buffer, bytesPerElement, required = false) {
  if (buffer.length === 0) {
    if (required) {
      throw new Error(`section '${name}' 为空`);
    }
    return 0;
  }
  if (buffer.length % bytesPerElement !== 0) {
    throw new Error(
      `section '${name}' 长度 ${buffer.length} 不是 ${bytesPerElement} 的整数倍`
    );
  }
  return buffer.length / bytesPerElement;
}

function toArrayBuffer(buffer) {
  const copy = new Uint8Array(buffer.length);
  copy.set(buffer);
  return copy.buffer;
}

function readUint32(buffer, offset = 0) {
  return new DataView(buffer, offset, 4).getUint32(0, true);
}

function main() {
  const input = process.argv[2];
  if (!input) {
    usage();
  }

  const filePath = path.resolve(process.cwd(), input);
  if (!fs.existsSync(filePath)) {
    console.error(`文件不存在: ${filePath}`);
    process.exit(1);
  }

  const raw = fs.readFileSync(filePath);
  if (raw.length < 4) {
    throw new Error("文件过小，无法识别为 XKT");
  }

  const view = new DataView(toArrayBuffer(raw));
  const versionWithFlags = view.getUint32(0, true);
  const version = versionWithFlags & 0x7FFFFFFF; // 移除压缩标志位
  const compressed = (versionWithFlags & 0x80000000) !== 0;
  if (version !== 10) {
    throw new Error(`仅支持 XKT v10，检测到版本: ${version} (原始值: ${versionWithFlags})`);
  }

  const SECTION_COUNT = SECTION_ORDER.length;
  const offsets = [];
  for (let i = 0; i < SECTION_COUNT; i++) {
    offsets.push(view.getUint32(4 + i * 4, true));
  }
  offsets.push(raw.length);

  const sections = new Map();
  SECTION_ORDER.forEach((name, idx) => {
    const start = offsets[idx];
    const end = offsets[idx + 1];
    const slice = raw.slice(start, end);
    sections.set(name, inflateSection(name, slice));
  });

  // 元数据
  const metadataBuf = sections.get("metadata");
  const metadataStr = metadataBuf.toString("utf8");
  let metadata;
  try {
    metadata = JSON.parse(metadataStr);
  } catch (err) {
    throw new Error(`元数据 JSON 解析失败: ${err.message}`);
  }

  if (!metadata || typeof metadata !== "object") {
    throw new Error("元数据结构非法");
  }

  // 顶点与法线
  const positionsBuf = sections.get("positions");
  const vertexCount = checkAlignment("positions", positionsBuf, 2, true) / 3;

  const normalsBuf = sections.get("normals");
  const normalCount = checkAlignment("normals", normalsBuf, 1);
  if (normalCount > 0 && normalCount % 3 != 0) {
    throw new Error("法线数组长度非法");
  }

  const colorsBuf = sections.get("colors");
  checkAlignment("colors", colorsBuf, 4);

  // 索引
  const indicesBuf = sections.get("indices");
  const indexCount = checkAlignment("indices", indicesBuf, 4, true);
  if (indexCount < 1) {
    throw new Error("索引数量为 0");
  }

  // 几何体摘要
  const geometriesBuf = sections.get("eachGeometryPositionsPortion");
  const geometryCount = checkAlignment("eachGeometryPositionsPortion", geometriesBuf, 4, true);

  const meshGeomBuf = sections.get("eachMeshGeometriesPortion");
  const meshCount = checkAlignment("eachMeshGeometriesPortion", meshGeomBuf, 4, true);

  // 实体列表
  const entityIdBuf = sections.get("eachEntityId");
  const entityIds = JSON.parse(entityIdBuf.toString("utf8"));
  if (!Array.isArray(entityIds) || entityIds.length === 0) {
    throw new Error("实体列表为空");
  }

  const entityMeshesPortionBuf = sections.get("eachEntityMeshesPortion");
  checkAlignment("eachEntityMeshesPortion", entityMeshesPortionBuf, 4, true);

  process.stdout.write(
    JSON.stringify(
      {
        version,
        vertexCount,
        indexCount,
        geometryCount,
        meshCount,
        entityCount: entityIds.length,
      },
      null,
      2
    ) + "\n"
  );
}

try {
  main();
} catch (err) {
  console.error(`验证失败: ${err.message}`);
  process.exit(1);
}
