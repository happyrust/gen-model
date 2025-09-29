#!/usr/bin/env node

// 简化测试：使用curl验证服务器和文件访问
import { exec } from 'child_process';
import { promisify } from 'util';

const execAsync = promisify(exec);

class SimpleTestRunner {
    async testHttpServer() {
        console.log('🌐 测试HTTP服务器...');

        try {
            const { stdout, stderr } = await execAsync('curl -s -o /dev/null -w "%{http_code}" http://localhost:8083/');
            if (stdout.trim() === '200') {
                console.log('✅ HTTP服务器运行正常');
                return true;
            } else {
                console.log('❌ HTTP服务器响应异常:', stdout.trim());
                return false;
            }
        } catch (error) {
            console.log('❌ 无法连接到HTTP服务器:', error.message);
            return false;
        }
    }

    async testFileAccess() {
        console.log('📁 测试文件访问...');

        const files = [
            'xkt_v10_test.html',
            'cube_v10_fixed.xkt',
            'js/xeokit/xeokit-sdk.es.js'
        ];

        for (const file of files) {
            try {
                const { stdout } = await execAsync(`curl -s -o /dev/null -w "%{http_code}" http://localhost:8082/${file}`);
                if (stdout.trim() === '200') {
                    console.log(`✅ ${file} - 可访问`);
                } else {
                    console.log(`❌ ${file} - HTTP ${stdout.trim()}`);
                }
            } catch (error) {
                console.log(`❌ ${file} - 错误: ${error.message}`);
            }
        }
    }

    async testXKTFileStructure() {
        console.log('🔍 分析XKT文件结构...');

        try {
            const { stdout } = await execAsync('cargo run --bin xkt_detailed_validator -- -f output/cube_v10_fixed.xkt');
            console.log('📊 XKT文件验证结果:');

            if (stdout.includes('✅ 大小匹配')) {
                console.log('✅ XKT文件格式正确');
                return true;
            } else {
                console.log('❌ XKT文件格式有问题');
                return false;
            }
        } catch (error) {
            console.log('❌ XKT文件验证失败:', error.message);
            return false;
        }
    }

    async run() {
        console.log('='.repeat(50));
        console.log('🧪 XKT简化测试');
        console.log('='.repeat(50));

        const serverOk = await this.testHttpServer();
        if (!serverOk) {
            console.log('💡 请启动HTTP服务器: python3 -m http.server 8082');
            process.exit(1);
        }

        await this.testFileAccess();

        const xktOk = await this.testXKTFileStructure();

        console.log('\n' + '='.repeat(50));
        console.log('📋 测试总结');
        console.log('='.repeat(50));

        if (serverOk && xktOk) {
            console.log('🎉 基础测试通过！XKT文件可以尝试在浏览器中加载');
            console.log('🌐 访问: http://localhost:8082/xkt_v10_test.html');
        } else {
            console.log('❌ 基础测试失败，需要修复问题');
        }
    }
}

const runner = new SimpleTestRunner();
runner.run().catch(console.error);