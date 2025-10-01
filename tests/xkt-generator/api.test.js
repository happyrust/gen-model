/**
 * XKT生成器API自动化测试
 * 测试后端XKT生成接口的功能
 */

import fetch from 'node-fetch';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const API_BASE_URL = 'http://localhost:8080';

// 测试配置
const TEST_CONFIG = {
  validDbno: 1112,
  validRefno: '17496/266203',
  invalidDbno: 99999,
  invalidRefno: 'invalid/refno',
  timeout: 30000
};

// 测试结果收集
const testResults = {
  passed: 0,
  failed: 0,
  errors: []
};

/**
 * 断言函数
 */
function assert(condition, message) {
  if (!condition) {
    throw new Error(`Assertion failed: ${message}`);
  }
}

/**
 * 测试用例包装器
 */
async function test(name, testFunction) {
  console.log(`\n🧪 测试: ${name}`);

  try {
    await testFunction();
    console.log(`  ✅ 通过`);
    testResults.passed++;
  } catch (error) {
    console.error(`  ❌ 失败: ${error.message}`);
    testResults.failed++;
    testResults.errors.push({ test: name, error: error.message });
  }
}

/**
 * 测试1: 生成完整数据库的XKT（压缩）
 */
async function testGenerateFullDatabaseCompressed() {
  const response = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      dbno: TEST_CONFIG.validDbno,
      compress: true
    })
  });

  assert(response.ok, `响应状态码应为200，实际为${response.status}`);

  const result = await response.json();
  assert(result.success === true, '应返回success: true');
  assert(result.dbno === TEST_CONFIG.validDbno, `数据库号应为${TEST_CONFIG.validDbno}`);
  assert(result.filename, '应返回文件名');
  assert(result.filename.includes('compressed'), '文件名应包含compressed');
  assert(result.url, '应返回下载URL');
}

/**
 * 测试2: 生成特定参考号的XKT（压缩）
 */
async function testGenerateWithRefnoCompressed() {
  const response = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      dbno: TEST_CONFIG.validDbno,
      refno: TEST_CONFIG.validRefno,
      compress: true
    })
  });

  assert(response.ok, `响应状态码应为200，实际为${response.status}`);

  const result = await response.json();
  assert(result.success === true, '应返回success: true');
  assert(result.dbno === TEST_CONFIG.validDbno, `数据库号应为${TEST_CONFIG.validDbno}`);
  assert(result.refno === TEST_CONFIG.validRefno, `参考号应为${TEST_CONFIG.validRefno}`);
  assert(result.filename, '应返回文件名');
  assert(result.filename.includes('refno'), '文件名应包含refno');
  assert(result.url, '应返回下载URL');
}

/**
 * 测试3: 生成未压缩的XKT文件
 */
async function testGenerateUncompressed() {
  const response = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      dbno: TEST_CONFIG.validDbno,
      refno: TEST_CONFIG.validRefno,
      compress: false
    })
  });

  assert(response.ok, `响应状态码应为200，实际为${response.status}`);

  const result = await response.json();
  assert(result.success === true, '应返回success: true');
  assert(result.filename, '应返回文件名');
  assert(result.filename.includes('raw'), '文件名应包含raw');
}

/**
 * 测试4: 下载生成的XKT文件
 */
async function testDownloadXKT() {
  // 先生成一个文件
  const generateResponse = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      dbno: TEST_CONFIG.validDbno,
      refno: TEST_CONFIG.validRefno,
      compress: true
    })
  });

  const result = await generateResponse.json();
  assert(result.success, '生成文件应成功');

  // 下载文件
  const downloadResponse = await fetch(`${API_BASE_URL}${result.url}`);
  assert(downloadResponse.ok, `下载响应应为200，实际为${downloadResponse.status}`);

  const buffer = await downloadResponse.buffer();
  assert(buffer.length > 0, '下载的文件不应为空');
  assert(buffer.length > 500, '文件大小应大于500字节');

  // 验证XKT文件格式（检查文件头）
  const version = buffer.readUInt32LE(0) & 0x7FFFFFFF;
  assert(version === 10, `XKT版本应为10，实际为${version}`);
}

/**
 * 测试5: 无效数据库号
 */
async function testInvalidDbno() {
  const response = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      dbno: TEST_CONFIG.invalidDbno,
      compress: true
    })
  });

  // 对于无效数据库，可能返回500或空模型
  assert(response.status === 500 || response.status === 200,
    '无效数据库应返回错误或空模型');
}

/**
 * 测试6: 缺少必填参数
 */
async function testMissingRequiredParam() {
  const response = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      compress: true
      // 缺少dbno
    })
  });

  assert(!response.ok, '缺少必填参数应返回错误');
}

/**
 * 测试7: 并发生成测试
 */
async function testConcurrentGeneration() {
  const promises = [];
  const concurrentCount = 3;

  for (let i = 0; i < concurrentCount; i++) {
    const promise = fetch(`${API_BASE_URL}/api/xkt/generate`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        dbno: TEST_CONFIG.validDbno,
        refno: TEST_CONFIG.validRefno,
        compress: true
      })
    });
    promises.push(promise);
  }

  const responses = await Promise.all(promises);
  const results = await Promise.all(responses.map(r => r.json()));

  responses.forEach((response, index) => {
    assert(response.ok, `并发请求${index + 1}应成功`);
  });

  results.forEach((result, index) => {
    assert(result.success === true, `并发请求${index + 1}应返回success: true`);
  });
}

/**
 * 测试8: 文件大小验证
 */
async function testFileSizeComparison() {
  // 生成压缩版本
  const compressedResponse = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      dbno: TEST_CONFIG.validDbno,
      refno: TEST_CONFIG.validRefno,
      compress: true
    })
  });

  const compressedResult = await compressedResponse.json();
  const compressedFile = await fetch(`${API_BASE_URL}${compressedResult.url}`);
  const compressedBuffer = await compressedFile.buffer();

  // 生成未压缩版本
  const uncompressedResponse = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      dbno: TEST_CONFIG.validDbno,
      refno: TEST_CONFIG.validRefno,
      compress: false
    })
  });

  const uncompressedResult = await uncompressedResponse.json();
  const uncompressedFile = await fetch(`${API_BASE_URL}${uncompressedResult.url}`);
  const uncompressedBuffer = await uncompressedFile.buffer();

  const compressionRatio = compressedBuffer.length / uncompressedBuffer.length;
  console.log(`  压缩率: ${(compressionRatio * 100).toFixed(1)}%`);
  console.log(`  压缩大小: ${compressedBuffer.length} bytes`);
  console.log(`  未压缩大小: ${uncompressedBuffer.length} bytes`);

  assert(compressedBuffer.length < uncompressedBuffer.length,
    '压缩文件应小于未压缩文件');
  assert(compressionRatio < 0.5,
    '压缩率应小于50%');
}

/**
 * 主测试运行器
 */
async function runTests() {
  console.log('=' .repeat(60));
  console.log('🚀 XKT生成器API自动化测试');
  console.log('=' .repeat(60));
  console.log(`API地址: ${API_BASE_URL}`);
  console.log(`测试数据库: ${TEST_CONFIG.validDbno}`);
  console.log(`测试参考号: ${TEST_CONFIG.validRefno}`);

  // 检查服务器是否运行
  try {
    const healthCheck = await fetch(`${API_BASE_URL}/api/xkt/generate`, {
      method: 'GET'
    }).catch(() => null);

    if (!healthCheck) {
      console.error('\n❌ 错误: 无法连接到API服务器');
      console.error('请确保后端服务在8080端口运行');
      process.exit(1);
    }
  } catch (error) {
    // GET请求会失败，但这证明服务器在运行
  }

  // 运行测试用例
  await test('生成完整数据库XKT（压缩）', testGenerateFullDatabaseCompressed);
  await test('生成指定参考号XKT（压缩）', testGenerateWithRefnoCompressed);
  await test('生成未压缩XKT', testGenerateUncompressed);
  await test('下载XKT文件', testDownloadXKT);
  await test('无效数据库号处理', testInvalidDbno);
  await test('缺少必填参数处理', testMissingRequiredParam);
  await test('并发生成测试', testConcurrentGeneration);
  await test('文件大小比较', testFileSizeComparison);

  // 输出测试报告
  console.log('\n' + '=' .repeat(60));
  console.log('📊 测试报告');
  console.log('=' .repeat(60));
  console.log(`✅ 通过: ${testResults.passed}`);
  console.log(`❌ 失败: ${testResults.failed}`);
  console.log(`📈 通过率: ${(testResults.passed / (testResults.passed + testResults.failed) * 100).toFixed(1)}%`);

  if (testResults.errors.length > 0) {
    console.log('\n失败的测试:');
    testResults.errors.forEach(error => {
      console.log(`  - ${error.test}: ${error.error}`);
    });
  }

  // 返回退出码
  process.exit(testResults.failed > 0 ? 1 : 0);
}

// 运行测试
runTests().catch(console.error);