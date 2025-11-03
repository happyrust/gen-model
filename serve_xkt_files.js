#!/usr/bin/env node

import express from 'express';
import path from 'path';
import fs from 'fs';
import cors from 'cors';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const app = express();
const PORT = 3001;

// 启用 CORS
app.use(cors());

// 静态文件服务
app.use('/output', express.static(path.join(__dirname, 'output')));

// 获取 XKT 文件列表
app.get('/api/xkt/files', (req, res) => {
  try {
    const outputDir = path.join(__dirname, 'output');
    const files = fs.readdirSync(outputDir)
      .filter(file => file.endsWith('.xkt'))
      .map(file => {
        const filePath = path.join(outputDir, file);
        const stats = fs.statSync(filePath);
        return {
          filename: file,
          size: stats.size,
          url: `/output/${file}`,
          lastModified: stats.mtime
        };
      });
    
    res.json({ success: true, files });
  } catch (error) {
    console.error('获取文件列表失败:', error);
    res.status(500).json({ success: false, error: error.message });
  }
});

// 分析 XKT 文件
app.get('/api/xkt/analyze/:filename', (req, res) => {
  try {
    const filename = req.params.filename;
    const filePath = path.join(__dirname, 'output', filename);
    
    if (!fs.existsSync(filePath)) {
      return res.status(404).json({ success: false, error: '文件不存在' });
    }
    
    const buffer = fs.readFileSync(filePath);
    const dataView = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
    
    // 读取版本号 (前4字节，小端序)
    const version = dataView.getUint32(0, true);
    
    // 读取段数量 (第5-8字节)
    const sections = dataView.getUint32(4, true);
    
    // 读取段偏移表
    const offsets = [];
    for (let i = 0; i < sections; i++) {
      offsets.push(dataView.getUint32(8 + i * 4, true));
    }
    
    // 验证文件结构
    const errors = [];
    let valid = true;
    
    // 检查版本号
    if (version !== 10) {
      errors.push(`版本号错误: 期望 10，实际 ${version}`);
      valid = false;
    }
    
    // 检查段数量
    if (sections !== 29) {
      errors.push(`段数量错误: 期望 29，实际 ${sections}`);
      valid = false;
    }
    
    // 检查段偏移
    for (let i = 0; i < offsets.length; i++) {
      if (offsets[i] > buffer.length) {
        errors.push(`段 ${i} 偏移超出文件大小: ${offsets[i]} > ${buffer.length}`);
        valid = false;
      }
    }
    
    // 尝试解析元数据
    let metadata = null;
    if (offsets[0] > 0 && offsets[0] < buffer.length) {
      try {
        const metadataBytes = buffer.slice(offsets[0], Math.min(offsets[0] + 1000, buffer.length));
        const metadataText = metadataBytes.toString('utf8');
        if (metadataText.includes('{')) {
          const jsonStart = metadataText.indexOf('{');
          const jsonEnd = metadataText.lastIndexOf('}') + 1;
          if (jsonEnd > jsonStart) {
            metadata = JSON.parse(metadataText.substring(jsonStart, jsonEnd));
          }
        }
      } catch (error) {
        console.warn('元数据解析失败:', error);
      }
    }
    
    // 检查压缩状态
    const compressed = offsets.some(offset => offset > buffer.length);
    
    res.json({
      success: true,
      analysis: {
        filename,
        size: buffer.length,
        version,
        sections,
        compressed,
        valid,
        errors,
        metadata,
        offsets: offsets.slice(0, 10) // 只返回前10个偏移
      }
    });
    
  } catch (error) {
    console.error('分析文件失败:', error);
    res.status(500).json({ success: false, error: error.message });
  }
});

// 健康检查
app.get('/health', (req, res) => {
  res.json({ status: 'ok', timestamp: new Date().toISOString() });
});

// 启动服务器
app.listen(PORT, () => {
  console.log(`🚀 XKT 文件服务器已启动`);
  console.log(`📁 静态文件目录: ${path.join(__dirname, 'output')}`);
  console.log(`🌐 服务地址: http://localhost:${PORT}`);
  console.log(`📋 API 端点:`);
  console.log(`   GET /api/xkt/files - 获取文件列表`);
  console.log(`   GET /api/xkt/analyze/:filename - 分析 XKT 文件`);
  console.log(`   GET /output/:filename - 下载 XKT 文件`);
  console.log(`   GET /health - 健康检查`);
});

// 优雅关闭
process.on('SIGINT', () => {
  console.log('\n🛑 正在关闭服务器...');
  process.exit(0);
});
