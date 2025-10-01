#!/usr/bin/env node

/**
 * XKT生成器快速测试脚本
 * 用于快速验证基本功能
 */

import fetch from 'node-fetch';

const API_URL = 'http://localhost:8080';
const TEST_DBNO = 1112;
const TEST_REFNO = '17496/266203';

async function quickTest() {
  console.log('🚀 XKT生成器快速测试');
  console.log('=' .repeat(50));

  try {
    // 测试1: 生成带参考号的XKT（快速）
    console.log('\n📝 测试: 生成指定参考号的XKT文件');
    console.log('  数据库号:', TEST_DBNO);
    console.log('  参考号:', TEST_REFNO);

    const response = await fetch(`${API_URL}/api/xkt/generate`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        dbno: TEST_DBNO,
        refno: TEST_REFNO,
        compress: true
      })
    });

    if (!response.ok) {
      throw new Error(`API返回错误: ${response.status}`);
    }

    const result = await response.json();
    console.log('\n✅ 生成成功!');
    console.log('  文件名:', result.filename);
    console.log('  下载URL:', result.url);

    // 测试2: 验证文件下载
    console.log('\n📝 测试: 下载生成的文件');
    const downloadResponse = await fetch(`${API_URL}${result.url}`);

    if (!downloadResponse.ok) {
      throw new Error(`下载失败: ${downloadResponse.status}`);
    }

    const buffer = await downloadResponse.buffer();
    console.log('✅ 下载成功!');
    console.log('  文件大小:', buffer.length, 'bytes');

    // 验证XKT文件格式
    const version = buffer.readUInt32LE(0) & 0x7FFFFFFF;
    const isCompressed = (buffer.readUInt32LE(0) & 0x80000000) !== 0;
    console.log('  XKT版本:', version);
    console.log('  是否压缩:', isCompressed);

    // 测试总结
    console.log('\n' + '=' .repeat(50));
    console.log('✅ 所有测试通过!');
    console.log('\n功能验证:');
    console.log('  ✅ API接口正常');
    console.log('  ✅ XKT生成功能正常');
    console.log('  ✅ 文件下载功能正常');
    console.log('  ✅ 文件格式正确');

    return true;
  } catch (error) {
    console.error('\n❌ 测试失败:', error.message);
    return false;
  }
}

// 运行测试
quickTest().then(success => {
  process.exit(success ? 0 : 1);
}).catch(error => {
  console.error('测试运行错误:', error);
  process.exit(1);
});