/// 数据解析向导页面模板

pub fn wizard_page() -> String {
    format!(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>部署站点创建向导 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js" defer></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
    <style>
        /* 自定义卡片样式增强 */
        .db-connection-card {{
            background: linear-gradient(135deg, #ffffff 0%, #f8fafc 100%);
            border: 1px solid #e2e8f0;
            box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
            transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
        }}

        .db-connection-card:hover {{
            box-shadow: 0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05);
            transform: translateY(-1px);
        }}

        .info-section {{
            background: rgba(248, 250, 252, 0.8);
            backdrop-filter: blur(10px);
            border: 1px solid rgba(226, 232, 240, 0.5);
        }}

        .auth-section {{
            background: rgba(254, 252, 232, 0.8);
            backdrop-filter: blur(10px);
            border: 1px solid rgba(251, 191, 36, 0.2);
        }}

        .ssh-section {{
            background: rgba(254, 252, 232, 0.6);
            backdrop-filter: blur(10px);
            border: 1px solid rgba(251, 191, 36, 0.3);
        }}

        /* 输入框增强效果 */
        .enhanced-input {{
            transition: all 0.2s ease-in-out;
        }}

        .enhanced-input:focus {{
            transform: scale(1.02);
            box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.1);
        }}

        /* 按钮增强效果 */
        .enhanced-button {{
            transition: all 0.2s ease-in-out;
            position: relative;
            overflow: hidden;
        }}

        .enhanced-button:hover {{
            transform: translateY(-1px);
        }}

        .enhanced-button:active {{
            transform: translateY(0);
        }}
    </style>
</head>
<body class="bg-gray-50" x-data="wizardManager()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-magic text-2xl"></i>
                        <h1 class="text-xl font-bold">部署站点创建向导</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/db-status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-database mr-2"></i>数据库状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
            {}
            {}
        </div>
    </div>

    {}
</body>
</html>
"#, 
    wizard_steps_indicator(),
    wizard_step_content(),
    wizard_javascript()
    )
}

/// 步骤指示器
fn wizard_steps_indicator() -> String {
    r#"
    <!-- 步骤指示器 -->
    <div class="mb-8">
        <div class="flex items-center justify-center">
            <div class="flex items-center space-x-4">
                <!-- 步骤1: 选择目录 -->
                <div class="flex items-center">
                    <div class="flex items-center justify-center w-10 h-10 rounded-full"
                         :class="currentStep >= 1 ? 'bg-blue-600 text-white' : 'bg-gray-300 text-gray-600'">
                        <i class="fas fa-folder"></i>
                    </div>
                    <span class="ml-2 text-sm font-medium" 
                          :class="currentStep >= 1 ? 'text-blue-600' : 'text-gray-500'">选择目录</span>
                </div>
                
                <div class="w-16 h-1 bg-gray-300" :class="currentStep >= 2 ? 'bg-blue-600' : 'bg-gray-300'"></div>
                
                <!-- 步骤2: 选择项目 -->
                <div class="flex items-center">
                    <div class="flex items-center justify-center w-10 h-10 rounded-full"
                         :class="currentStep >= 2 ? 'bg-blue-600 text-white' : 'bg-gray-300 text-gray-600'">
                        <i class="fas fa-project-diagram"></i>
                    </div>
                    <span class="ml-2 text-sm font-medium"
                          :class="currentStep >= 2 ? 'text-blue-600' : 'text-gray-500'">选择项目</span>
                </div>
                
                <div class="w-16 h-1 bg-gray-300" :class="currentStep >= 3 ? 'bg-blue-600' : 'bg-gray-300'"></div>
                
                <!-- 步骤3: 配置参数 -->
                <div class="flex items-center">
                    <div class="flex items-center justify-center w-10 h-10 rounded-full"
                         :class="currentStep >= 3 ? 'bg-blue-600 text-white' : 'bg-gray-300 text-gray-600'">
                        <i class="fas fa-cogs"></i>
                    </div>
                    <span class="ml-2 text-sm font-medium"
                          :class="currentStep >= 3 ? 'text-blue-600' : 'text-gray-500'">配置参数</span>
                </div>
                
                <div class="w-16 h-1 bg-gray-300" :class="currentStep >= 4 ? 'bg-blue-600' : 'bg-gray-300'"></div>
                
                <!-- 步骤4: 执行任务 -->
                <div class="flex items-center">
                    <div class="flex items-center justify-center w-10 h-10 rounded-full"
                         :class="currentStep >= 4 ? 'bg-blue-600 text-white' : 'bg-gray-300 text-gray-600'">
                        <i class="fas fa-play"></i>
                    </div>
                    <span class="ml-2 text-sm font-medium"
                          :class="currentStep >= 4 ? 'text-blue-600' : 'text-gray-500'">执行任务</span>
                </div>
            </div>
        </div>
    </div>
    "#.to_string()
}

/// 步骤内容
fn wizard_step_content() -> String {
    format!(r#"
    <!-- 步骤内容 -->
    <div class="bg-white rounded-lg shadow-lg p-6">
        {}
        {}
        {}
        {}
    </div>
    "#,
    step1_directory_selection(),
    step2_project_selection(),
    step3_parameter_configuration(),
    step4_task_execution()
    )
}

/// 步骤1: 目录选择
fn step1_directory_selection() -> String {
    r#"
    <!-- 步骤1: 选择目录 -->
    <div x-show="currentStep === 1" x-transition>
        <h2 class="text-2xl font-bold mb-6 text-gray-800">
            <i class="fas fa-folder mr-2 text-blue-600"></i>选择项目目录
        </h2>
        
        <div class="space-y-4">
            <div>
                <label class="block text-sm font-medium text-gray-700 mb-2">
                    项目根目录路径
                </label>
                <div class="flex space-x-2">
                    <input type="text" 
                           x-model="directoryPath"
                           class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                           placeholder="例如: /Volumes/DPC/work/e3d_models">
                    <button @click="scanDirectory()" 
                            :disabled="!directoryPath || scanning"
                            class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-400">
                        <i class="fas fa-search mr-2"></i>
                        <span x-text="scanning ? '扫描中...' : '扫描'"></span>
                    </button>
                </div>
            </div>
            
            <div>
                <label class="flex items-center">
                    <input type="checkbox" x-model="scanRecursive" class="mr-2">
                    <span class="text-sm text-gray-700">递归扫描子目录</span>
                </label>
                <p class="text-xs text-gray-500 mt-1">默认扫描深度：1层</p>
            </div>
            
            <!-- 扫描结果 -->
            <div x-show="scanResult" class="mt-6">
                <div class="bg-green-50 border border-green-200 rounded-md p-4">
                    <h3 class="text-lg font-medium text-green-800 mb-2">
                        <i class="fas fa-check-circle mr-2"></i>扫描完成
                    </h3>
                    <div class="text-sm text-green-700">
                        <p>扫描目录: <span x-text="scanResult?.root_directory"></span></p>
                        <p>找到项目: <span x-text="scanResult?.projects?.length || 0"></span> 个</p>
                        <p>扫描耗时: <span x-text="scanResult?.scan_duration_ms"></span> 毫秒</p>
                        <p>扫描目录数: <span x-text="scanResult?.scanned_directories"></span> 个</p>
                    </div>
                </div>
            </div>
            
            <!-- 错误信息 -->
            <div x-show="scanResult?.errors?.length > 0" class="mt-4">
                <div class="bg-yellow-50 border border-yellow-200 rounded-md p-4">
                    <h4 class="text-sm font-medium text-yellow-800 mb-2">扫描警告:</h4>
                    <ul class="text-sm text-yellow-700 space-y-1">
                        <template x-for="error in (scanResult?.errors || [])">
                            <li x-text="error"></li>
                        </template>
                    </ul>
                </div>
            </div>
        </div>
        
        <div class="flex justify-end mt-8">
            <button @click="nextStep()" 
                    :disabled="!scanResult || scanResult.projects.length === 0"
                    class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-400">
                下一步 <i class="fas fa-arrow-right ml-2"></i>
            </button>
        </div>
    </div>
    "#.to_string()
}

/// 步骤2: 项目选择
fn step2_project_selection() -> String {
    r#"
    <!-- 步骤2: 选择项目 -->
    <div x-show="currentStep === 2" x-transition>
        <h2 class="text-2xl font-bold mb-6 text-gray-800">
            <i class="fas fa-project-diagram mr-2 text-blue-600"></i>选择要解析的项目
        </h2>
        
        <div class="space-y-4">
            <div class="flex justify-between items-center">
                <p class="text-gray-600">
                    找到 <span x-text="scanResult?.projects?.length || 0"></span> 个项目，请选择要解析的项目：
                </p>
                <div class="space-x-2">
                    <button @click="selectAllProjects()" 
                            class="px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded hover:bg-blue-200">
                        全选
                    </button>
                    <button @click="clearAllProjects()" 
                            class="px-3 py-1 text-sm bg-gray-100 text-gray-700 rounded hover:bg-gray-200">
                        清空
                    </button>
                </div>
            </div>
            
            <!-- 项目列表 -->
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 max-h-96 overflow-y-auto">
                <template x-for="project in scanResult?.projects || []" :key="project.name">
                    <div class="border border-gray-200 rounded-lg p-4 hover:border-blue-300 transition-colors"
                         :class="selectedProjects.includes(project.name) ? 'border-blue-500 bg-blue-50' : ''">
                        <label class="flex items-start space-x-3 cursor-pointer">
                            <input type="checkbox"
                                   :value="project.name"
                                   x-model="selectedProjects"
                                   class="mt-1 rounded border-gray-300 text-blue-600">
                            <div class="flex-1 min-w-0">
                                <h3 class="font-semibold text-lg text-gray-900 truncate" x-text="project.name"></h3>
                                <p class="text-sm text-gray-500 mb-2 truncate" x-text="project.description"></p>
                                <div class="space-y-1">
                                    <div class="flex items-center text-xs text-gray-600">
                                        <i class="fas fa-database mr-1 w-3"></i>
                                        <span x-text="project.db_file_count"></span> 个文件
                                    </div>
                                    <div class="flex items-center text-xs text-gray-600">
                                        <i class="fas fa-hdd mr-1 w-3"></i>
                                        <span x-text="formatFileSize(project.size_bytes)"></span>
                                    </div>
                                    <div class="flex items-center text-xs text-gray-600" x-show="project.project_code">
                                        <i class="fas fa-code mr-1 w-3"></i>
                                        代码: <span x-text="project.project_code"></span>
                                    </div>
                                </div>
                            </div>
                        </label>
                    </div>
                </template>
            </div>
            
            <!-- 选择统计 -->
            <div x-show="selectedProjects.length > 0" class="bg-blue-50 border border-blue-200 rounded-md p-4">
                <h4 class="text-sm font-medium text-blue-800 mb-2">
                    已选择 <span x-text="selectedProjects.length"></span> 个项目:
                </h4>
                <div class="flex flex-wrap gap-2">
                    <template x-for="projectName in selectedProjects" :key="projectName">
                        <span class="inline-block bg-blue-100 text-blue-800 text-xs px-2 py-1 rounded"
                              x-text="projectName"></span>
                    </template>
                </div>
            </div>
        </div>
        
        <div class="flex justify-between mt-8">
            <button @click="prevStep()" 
                    class="px-6 py-2 bg-gray-300 text-gray-700 rounded-md hover:bg-gray-400">
                <i class="fas fa-arrow-left mr-2"></i>上一步
            </button>
            <button @click="nextStep()" 
                    :disabled="selectedProjects.length === 0"
                    class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-400">
                下一步 <i class="fas fa-arrow-right ml-2"></i>
            </button>
        </div>
    </div>
    "#.to_string()
}

/// 步骤3: 参数配置
fn step3_parameter_configuration() -> String {
    r#"
    <!-- 步骤3: 配置参数 -->
    <div x-show="currentStep === 3" x-transition>
        <h2 class="text-2xl font-bold mb-6 text-gray-800">
            <i class="fas fa-cogs mr-2 text-blue-600"></i>配置解析参数
        </h2>

        <div class="space-y-6">
            <!-- 项目信息配置 -->
            <div>
                <h3 class="text-lg font-medium text-gray-900 mb-4">项目信息</h3>
                <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">任务名称</label>
                        <input type="text"
                               x-model="taskName"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                               placeholder="部署站点解析任务">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">项目名称</label>
                        <input type="text"
                               x-model="config.project_name"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                               placeholder="AvevaMarineSample">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">项目代码</label>
                        <input type="number"
                               x-model="config.project_code"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                               placeholder="1516">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">MDB名称</label>
                        <input type="text"
                               x-model="config.mdb_name"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                               placeholder="ALL">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">模块类型</label>
                        <select x-model="config.module"
                                class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                            <option value="DESI">DESI - 设计模块</option>
                            <option value="DRAFT">DRAFT - 制图模块</option>
                            <option value="ADMIN">ADMIN - 管理模块</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">数据库编号</label>
                        <div class="flex gap-2">
                            <input type="text"
                                   x-model="manualDbNums"
                                   class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                                   placeholder="7999,1112 (可选，逗号分隔)">
                            <button type="button"
                                    @click="showDbSelector = true; loadDatabaseFiles()"
                                    class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-blue-500">
                                <i class="fas fa-search mr-1"></i>扫描
                            </button>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 数据库连接配置卡片 -->
            <div class="db-connection-card rounded-lg p-6">
                <!-- 卡片标题和状态区 -->
                <div class="flex items-center justify-between mb-6">
                    <div class="flex items-center space-x-3">
                        <div class="flex items-center justify-center w-10 h-10 bg-blue-100 rounded-lg">
                            <i class="fas fa-database text-blue-600 text-lg"></i>
                        </div>
                        <div>
                            <h3 class="text-lg font-semibold text-gray-900">数据库连接</h3>
                            <div class="flex items-center text-sm text-gray-600 mt-1">
                                <span :class="surrealStatus.listening ? 'bg-green-500' : 'bg-gray-400'"
                                      class="inline-block w-2 h-2 rounded-full mr-2"></span>
                                <span x-text="surrealStatusText()"></span>
                            </div>
                        </div>
                    </div>

                    <!-- 控制模式和操作按钮 -->
                    <div class="flex items-center space-x-3">
                        <div class="hidden md:flex items-center space-x-2 text-sm text-gray-700 bg-gray-50 rounded-lg px-3 py-2">
                            <label class="flex items-center space-x-1 cursor-pointer">
                                <input type="radio" name="wizardCtrlMode" value="local" x-model="controlMode" class="text-blue-600">
                                <span>本机</span>
                            </label>
                            <label class="flex items-center space-x-1 cursor-pointer">
                                <input type="radio" name="wizardCtrlMode" value="ssh" x-model="controlMode" class="text-blue-600">
                                <span>远程(SSH)</span>
                            </label>
                        </div>
                        <button @click="toggleSurreal()"
                                :class="surrealStatus.listening ? 'bg-red-600 hover:bg-red-700' : 'bg-blue-600 hover:bg-blue-700'"
                                class="enhanced-button px-4 py-2 text-white text-sm rounded-lg font-medium transition-colors duration-200">
                            <i :class="surrealStatus.listening ? 'fas fa-stop mr-2' : 'fas fa-play mr-2'"></i>
                            <span x-text="surrealStatus.listening ? '停止' : '启动'"></span>
                        </button>
                        <button @click="restartSurreal()" x-show="surrealStatus.listening"
                                class="enhanced-button px-4 py-2 bg-purple-600 text-white rounded-lg hover:bg-purple-700 text-sm font-medium transition-colors duration-200">
                            <i class="fas fa-rotate mr-2"></i>
                            重启
                        </button>
                        <button @click="testConnection()"
                                class="enhanced-button px-4 py-2 bg-gray-600 text-white rounded-lg hover:bg-gray-700 text-sm font-medium transition-colors duration-200"
                                :disabled="testing">
                            <i class="fas fa-plug mr-2"></i>
                            <span x-text="testing ? '测试中…' : '连接测试'"></span>
                        </button>
                    </div>
                </div>
                <!-- 基本连接信息区 -->
                <div class="info-section rounded-lg p-4 mb-6">
                    <h4 class="text-sm font-semibold text-gray-800 mb-4 flex items-center">
                        <i class="fas fa-server text-gray-600 mr-2"></i>
                        基本连接信息
                    </h4>
                    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">数据库类型</label>
                            <select x-model="config.db_type"
                                    class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent bg-white">
                                <option value="surrealdb">SurrealDB</option>
                                <option value="tidb">TiDB</option>
                                <option value="arangodb">ArangoDB</option>
                            </select>
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">数据库IP</label>
                            <input type="text"
                                   x-model="config.db_ip"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                                   placeholder="localhost">
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">端口</label>
                            <input type="text"
                                   x-model="config.db_port"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                                   placeholder="8009">
                        </div>
                    </div>
                </div>

                <!-- 认证信息区 -->
                <div class="auth-section rounded-lg p-4 mb-6">
                    <h4 class="text-sm font-semibold text-gray-800 mb-4 flex items-center">
                        <i class="fas fa-key text-amber-600 mr-2"></i>
                        认证信息
                    </h4>
                    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">用户名</label>
                            <input type="text"
                                   x-model="config.db_user"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                                   placeholder="root">
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">密码</label>
                            <input type="password"
                                   x-model="config.db_password"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                                   placeholder="••••••••">
                        </div>
                        <div x-show="config.db_type === 'surrealdb'">
                            <label class="block text-sm font-medium text-gray-700 mb-2">命名空间</label>
                            <input type="number"
                                   x-model="config.surreal_ns"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                                   placeholder="1516">
                        </div>
                    </div>
                </div>

                <!-- 远程SSH参数 -->
                <div x-show="controlMode==='ssh'" class="ssh-section rounded-lg p-4">
                    <h4 class="text-sm font-semibold text-gray-800 mb-4 flex items-center">
                        <i class="fas fa-network-wired text-yellow-600 mr-2"></i>
                        SSH连接参数
                    </h4>
                    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">主机</label>
                            <input x-model="ssh.host" type="text" placeholder="192.168.1.10"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent">
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">端口</label>
                            <input x-model.number="ssh.port" type="number" placeholder="22"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent">
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">用户</label>
                            <input x-model="ssh.user" type="text" placeholder="root"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent">
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">密码（可选）</label>
                            <input x-model="ssh.password" type="password" placeholder="建议使用密钥"
                                   class="enhanced-input w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent">
                            <p class="text-xs text-yellow-700 mt-1">如未安装 sshpass，请使用密钥/agent</p>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 生成选项 -->
            <div>
                <h3 class="text-lg font-medium text-gray-900 mb-4">生成选项</h3>
                <template x-if="opMsg">
                    <div :class="opOk === null ? 'bg-gray-50 border-gray-200' : (opOk ? 'bg-green-50 border-green-200' : 'bg-red-50 border-red-200')"
                         class="mb-4 border rounded-md p-3 text-sm">
                        <span :class="opOk ? 'text-green-800' : 'text-red-800'" x-text="opMsg"></span>
                    </div>
                </template>
                <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                    <label class="flex items-center">
                        <input type="checkbox" x-model="config.gen_model" class="mr-2 rounded border-gray-300 text-blue-600">
                        <span class="text-sm text-gray-700">生成几何模型</span>
                    </label>
                    <label class="flex items-center">
                        <input type="checkbox" x-model="config.gen_mesh" class="mr-2 rounded border-gray-300 text-blue-600">
                        <span class="text-sm text-gray-700">生成网格数据</span>
                    </label>
                    <label class="flex items-center">
                        <input type="checkbox" x-model="config.gen_spatial_tree" class="mr-2 rounded border-gray-300 text-blue-600">
                        <span class="text-sm text-gray-700">生成空间树</span>
                    </label>
                    <label class="flex items-center">
                        <input type="checkbox" x-model="config.apply_boolean_operation" class="mr-2 rounded border-gray-300 text-blue-600">
                        <span class="text-sm text-gray-700">应用布尔运算</span>
                    </label>
                </div>
            </div>

            <!-- 高级选项 -->
            <div>
                <h3 class="text-lg font-medium text-gray-900 mb-4">高级选项</h3>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">网格容差比率</label>
                        <input type="number"
                               x-model="config.mesh_tol_ratio"
                               step="0.1"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">房间关键字</label>
                        <input type="text"
                               x-model="config.room_keyword"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                               placeholder="-RM">
                    </div>
                </div>
            </div>

            <!-- 执行选项 -->
            <div>
                <h3 class="text-lg font-medium text-gray-900 mb-4">执行选项</h3>
                <div class="space-y-4">
                    <label class="flex items-center">
                        <input type="checkbox" x-model="parallelProcessing" class="mr-2 rounded border-gray-300 text-blue-600">
                        <span class="text-sm text-gray-700">并行处理项目</span>
                    </label>
                    <div x-show="parallelProcessing" class="ml-6">
                        <label class="block text-sm font-medium text-gray-700 mb-2">最大并发数</label>
                        <input type="number"
                               x-model="maxConcurrent"
                               min="1"
                               max="8"
                               class="w-32 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                    <label class="flex items-center">
                        <input type="checkbox" x-model="continueOnFailure" class="mr-2 rounded border-gray-300 text-blue-600">
                        <span class="text-sm text-gray-700">失败时继续处理其他项目</span>
                    </label>
                </div>
            </div>

            <!-- 配置预览 -->
            <div class="bg-gray-50 border border-gray-200 rounded-md p-4">
                <h4 class="text-sm font-medium text-gray-800 mb-2">配置预览</h4>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs text-gray-600">
                    <div class="space-y-1">
                        <p><strong>项目信息:</strong></p>
                        <p>• 任务名称: <span x-text="taskName"></span></p>
                        <p>• 项目名称: <span x-text="config.project_name"></span></p>
                        <p>• 项目代码: <span x-text="config.project_code"></span></p>
                        <p>• MDB名称: <span x-text="config.mdb_name"></span></p>
                        <p>• 选中项目: <span x-text="selectedProjects.length"></span> 个</p>
                    </div>
                    <div class="space-y-1">
                        <p><strong>数据库连接:</strong></p>
                        <p>• 类型: <span x-text="config.db_type"></span></p>
                        <p>• 地址: <span x-text="config.db_ip + ':' + config.db_port"></span></p>
                        <p>• 用户: <span x-text="config.db_user"></span></p>
                        <p x-show="config.db_type === 'surrealdb'">• 命名空间: <span x-text="config.surreal_ns"></span></p>
                    </div>
                    <div class="space-y-1">
                        <p><strong>生成选项:</strong></p>
                        <p>• 生成模型: <span x-text="config.gen_model ? '是' : '否'"></span></p>
                        <p>• 生成网格: <span x-text="config.gen_mesh ? '是' : '否'"></span></p>
                        <p>• 生成空间树: <span x-text="config.gen_spatial_tree ? '是' : '否'"></span></p>
                        <p>• 并行处理: <span x-text="parallelProcessing ? '是' : '否'"></span></p>
                    </div>
                </div>
            </div>
        </div>

        <div class="flex justify-between mt-8">
            <button @click="prevStep()"
                    class="px-6 py-2 bg-gray-300 text-gray-700 rounded-md hover:bg-gray-400">
                <i class="fas fa-arrow-left mr-2"></i>上一步
            </button>
            <button @click="nextStep()"
                    :disabled="!taskName"
                    class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-400">
                下一步 <i class="fas fa-arrow-right ml-2"></i>
            </button>
        </div>
    </div>
    "#.to_string()
}

/// 步骤4: 任务执行
fn step4_task_execution() -> String {
    r#"
    <!-- 步骤4: 执行任务 -->
    <div x-show="currentStep === 4" x-transition>
        <h2 class="text-2xl font-bold mb-6 text-gray-800">
            <i class="fas fa-play mr-2 text-blue-600"></i>执行解析任务
        </h2>

        <div class="space-y-6">
            <!-- 任务摘要 -->
            <div class="bg-blue-50 border border-blue-200 rounded-md p-6">
                <h3 class="text-lg font-medium text-blue-800 mb-4">任务摘要</h3>
                <div class="grid grid-cols-1 md:grid-cols-3 gap-4 text-sm text-blue-700">
                    <div>
                        <p><strong>任务信息:</strong></p>
                        <p>• 任务名称: <span x-text="taskName"></span></p>
                        <p>• 项目数量: <span x-text="selectedProjects.length"></span> 个</p>
                        <p>• 根目录: <span x-text="directoryPath"></span></p>
                    </div>
                    <div>
                        <p><strong>项目配置:</strong></p>
                        <p>• 项目名称: <span x-text="config.project_name"></span></p>
                        <p>• 项目代码: <span x-text="config.project_code"></span></p>
                        <p>• MDB名称: <span x-text="config.mdb_name"></span></p>
                        <p>• 数据库类型: <span x-text="config.db_type"></span></p>
                    </div>
                    <div>
                        <p><strong>生成选项:</strong></p>
                        <p>• 生成模型: <span x-text="config.gen_model ? '是' : '否'"></span></p>
                        <p>• 生成网格: <span x-text="config.gen_mesh ? '是' : '否'"></span></p>
                        <p>• 生成空间树: <span x-text="config.gen_spatial_tree ? '是' : '否'"></span></p>
                        <p>• 并行处理: <span x-text="parallelProcessing ? '是' : '否'"></span></p>
                    </div>
                </div>
            </div>

            <!-- 项目列表 -->
            <div>
                <h4 class="text-md font-medium text-gray-800 mb-3">将要处理的项目:</h4>
                <div class="bg-white border border-gray-200 rounded-md max-h-48 overflow-y-auto">
                    <template x-for="(projectName, index) in selectedProjects" :key="projectName">
                        <div class="flex items-center justify-between px-4 py-2 border-b border-gray-100 last:border-b-0">
                            <div class="flex items-center">
                                <span class="text-sm font-medium text-gray-600 mr-2" x-text="index + 1 + '.'"></span>
                                <span class="text-sm text-gray-900" x-text="projectName"></span>
                            </div>
                            <div class="text-xs text-gray-500">
                                <template x-for="project in scanResult?.projects || []" :key="project.name">
                                    <span x-show="project.name === projectName" x-text="project.db_file_count + ' 个文件'"></span>
                                </template>
                            </div>
                        </div>
                    </template>
                </div>
            </div>

            <!-- 任务状态 -->
            <div x-show="taskCreated" class="bg-green-50 border border-green-200 rounded-md p-4">
                <div class="flex items-center">
                    <i class="fas fa-check-circle text-green-600 mr-2"></i>
                    <div>
                        <h4 class="text-sm font-medium text-green-800">任务创建成功！</h4>
                        <p class="text-sm text-green-700 mt-1">
                            任务ID: <span x-text="createdTaskId"></span>
                        </p>
                    </div>
                </div>
            </div>

            <!-- 错误信息 -->
            <div x-show="taskError" class="bg-red-50 border border-red-200 rounded-md p-4">
                <div class="flex items-center">
                    <i class="fas fa-exclamation-triangle text-red-600 mr-2"></i>
                    <div>
                        <h4 class="text-sm font-medium text-red-800">任务创建失败</h4>
                        <p class="text-sm text-red-700 mt-1" x-text="taskError"></p>
                    </div>
                </div>
            </div>
        </div>

        <div class="flex justify-between mt-8">
            <button @click="prevStep()"
                    :disabled="creatingTask"
                    class="px-6 py-2 bg-gray-300 text-gray-700 rounded-md hover:bg-gray-400 disabled:bg-gray-200">
                <i class="fas fa-arrow-left mr-2"></i>上一步
            </button>
            <div class="space-x-3">
                <button @click="createTask()"
                        :disabled="creatingTask || taskCreated"
                        class="px-6 py-2 bg-green-600 text-white rounded-md hover:bg-green-700 disabled:bg-gray-400">
                    <i class="fas fa-rocket mr-2"></i>
                    <span x-text="creatingTask ? '创建中...' : (taskCreated ? '已创建' : '创建任务')"></span>
                </button>
                <button x-show="taskCreated"
                        @click="goToTasks()"
                        class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700">
                    <i class="fas fa-tasks mr-2"></i>查看任务
                </button>
            </div>
        </div>

        <!-- 数据库选择器弹窗 -->
        <div x-show="showDbSelector"
             class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center"
             style="z-index: 9999;"
             @click.self="showDbSelector = false">
            <div class="bg-white rounded-lg shadow-xl max-w-2xl w-full mx-4 max-h-[80vh] overflow-hidden">
                <div class="px-6 py-4 border-b border-gray-200">
                    <div class="flex items-center justify-between">
                        <h3 class="text-lg font-medium text-gray-900">选择数据库文件</h3>
                        <button @click="showDbSelector = false" class="text-gray-400 hover:text-gray-600">
                            <i class="fas fa-times"></i>
                        </button>
                    </div>
                    <p class="text-sm text-gray-600 mt-2" x-show="selectedProjects.length > 0">
                        扫描项目: <span x-text="selectedProjects.join(', ')"></span>
                    </p>
                </div>

                <div class="p-6 overflow-y-auto max-h-96">
                    <div x-show="loadingDatabases" class="text-center py-8">
                        <i class="fas fa-spinner fa-spin text-2xl text-blue-600 mb-2"></i>
                        <p class="text-gray-600">正在扫描数据库文件...</p>
                    </div>

                    <div x-show="!loadingDatabases && databaseFiles.length === 0" class="text-center py-8">
                        <i class="fas fa-database text-4xl text-gray-400 mb-4"></i>
                        <p class="text-gray-600">未找到数据库文件</p>
                        <p class="text-sm text-gray-500 mt-2">请确保选择了正确的项目目录</p>
                    </div>

                    <div x-show="!loadingDatabases && databaseFiles.length > 0" class="space-y-2">
                        <div class="flex items-center justify-between mb-4">
                            <span class="text-sm text-gray-600">
                                找到 <span x-text="databaseFiles.length"></span> 个数据库文件
                            </span>
                            <div class="space-x-2">
                                <button @click="selectAllDatabases()"
                                        class="text-sm text-blue-600 hover:text-blue-800">全选</button>
                                <button @click="clearAllDatabases()"
                                        class="text-sm text-gray-600 hover:text-gray-800">清空</button>
                            </div>
                        </div>

                        <template x-for="dbFile in databaseFiles" :key="dbFile.db_num">
                            <label class="flex items-center p-3 border border-gray-200 rounded-md hover:bg-gray-50 cursor-pointer">
                                <input type="checkbox"
                                       :value="dbFile.db_num"
                                       x-model="selectedDbNums"
                                       class="mr-3 rounded border-gray-300 text-blue-600">
                                <div class="flex-1">
                                    <div class="flex items-center justify-between">
                                        <span class="font-medium text-gray-900" x-text="'DB ' + dbFile.db_num"></span>
                                        <span class="text-sm font-mono text-gray-600" x-text="dbFile.db_type"></span>
                                    </div>
                                    <div class="text-sm text-gray-600 mt-1" x-text="dbFile.file_name"></div>
                                    <div class="text-xs text-gray-500 mt-1">
                                        <span x-text="formatFileSize(dbFile.file_size)"></span>
                                        <span class="mx-2">•</span>
                                        <span x-text="formatDate(dbFile.modified_time)"></span>
                                    </div>
                                </div>
                            </label>
                        </template>
                    </div>
                </div>

                <div class="px-6 py-4 border-t border-gray-200 flex justify-between">
                    <div class="text-sm text-gray-600">
                        已选择 <span x-text="selectedDbNums.length"></span> 个数据库
                    </div>
                    <div class="space-x-3">
                        <button @click="showDbSelector = false"
                                class="px-4 py-2 text-gray-700 bg-gray-200 rounded-md hover:bg-gray-300">
                            取消
                        </button>
                        <button @click="applySelectedDatabases()"
                                class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700">
                            确定
                        </button>
                    </div>
                </div>
            </div>
        </div>
    </div>
    "#.to_string()
}

/// JavaScript 代码
fn wizard_javascript() -> String {
    r#"
    <script>
        function wizardManager() {
            return {
                // 当前步骤
                currentStep: 1,

                // 步骤1: 目录扫描
                directoryPath: '/Volumes/DPC/work/e3d_models',
                scanRecursive: true,
                maxDepth: 1,
                scanning: false,
                scanResult: null,

                // 步骤2: 项目选择
                selectedProjects: [],

                // 步骤3: 参数配置
                taskName: '部署站点解析任务',
                manualDbNums: '',
                config: {
                    project_name: 'AvevaMarineSample',
                    project_code: 1516,
                    mdb_name: 'ALL',
                    module: 'DESI',
                    db_type: 'surrealdb',
                    surreal_ns: 1516,
                    db_ip: 'localhost',
                    db_port: '8009',
                    db_user: 'root',
                    db_password: 'root',
                    gen_model: true,
                    gen_mesh: false,
                    gen_spatial_tree: true,
                    apply_boolean_operation: true,
                    mesh_tol_ratio: 3.0,
                    room_keyword: '-RM'
                },
                parallelProcessing: false,
                maxConcurrent: 2,
                continueOnFailure: true,

                // SurrealDB 控制与状态
                surrealStatus: { status: 'unknown', listening: false, connected: false, address: '' },
                statusTimer: null,
                controlMode: 'local',
                ssh: { host: '', port: 22, user: '', password: '' },
                opMsg: '',
                opOk: null,
                testing: false,

                // 数据库选择器
                showDbSelector: false,
                loadingDatabases: false,
                databaseFiles: [],
                selectedDbNums: [],
                scanErrors: [],

                // 步骤4: 任务执行
                creatingTask: false,
                taskCreated: false,
                createdTaskId: null,
                taskError: null,

                // 扫描目录
                async scanDirectory() {
                    if (!this.directoryPath) return;

                    this.scanning = true;
                    this.scanResult = null;

                    try {
                        const params = new URLSearchParams({
                            directory_path: this.directoryPath,
                            recursive: this.scanRecursive,
                            max_depth: this.maxDepth
                        });

                        const response = await fetch(`/api/wizard/scan-directory?${params}`);
                        if (response.ok) {
                            this.scanResult = await response.json();
                        } else {
                            throw new Error('扫描失败');
                        }
                    } catch (error) {
                        console.error('扫描目录失败:', error);
                        this.scanResult = {
                            root_directory: this.directoryPath,
                            projects: [],
                            scan_duration_ms: 0,
                            scanned_directories: 0,
                            errors: ['扫描失败: ' + error.message]
                        };
                    } finally {
                        this.scanning = false;
                    }
                },

                // SurrealDB: 刷新状态
                async refreshSurrealStatus() {
                    try {
                        const ip = this.config.db_ip || '';
                        const port = parseInt(this.config.db_port || '0') || 0;
                        const qs = new URLSearchParams({ ip, port });
                        const res = await fetch(`/api/surreal/status?${qs}`);
                        if (res.ok) {
                            const data = await res.json();
                            this.surrealStatus = {
                                status: data.status,
                                listening: !!data.listening,
                                connected: !!data.connected,
                                address: data.address || ''
                            };
                        }
                    } catch (_) { /* 忽略 */ }
                },

                surrealStatusText() {
                    const addr = this.surrealStatus.address ? `@ ${this.surrealStatus.address}` : '';
                    return this.surrealStatus.listening ? `已连接 ${addr}` : `未连接 ${addr}`;
                },

                async startSurreal() {
                    const body = {
                        mode: this.controlMode,
                        ssh: this.controlMode === 'ssh' ? { ...this.ssh } : undefined,
                        bind_ip: this.config.db_ip,
                        bind_port: parseInt(this.config.db_port || '0') || undefined,
                        db_user: this.config.db_user,
                        db_password: this.config.db_password,
                        project_name: this.config.project_name,
                    };
                    try {
                        const res = await fetch('/api/surreal/start', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: JSON.stringify(body)
                        });
                        const data = await res.json();
                        this.opMsg = data.message || (data.success ? '启动命令已发送' : '启动失败');
                        this.opOk = !!data.success;
                        await this.refreshSurrealStatus();
                    } catch (e) {
                        this.opMsg = '网络错误，无法启动';
                        this.opOk = false;
                    }
                },

                async stopSurreal() {
                    const body = {
                        mode: this.controlMode,
                        ssh: this.controlMode === 'ssh' ? { ...this.ssh } : undefined,
                        bind_ip: this.config.db_ip,
                        bind_port: parseInt(this.config.db_port || '0') || undefined,
                    };
                    try {
                        const res = await fetch('/api/surreal/stop', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: JSON.stringify(body)
                        });
                        const data = await res.json();
                        this.opMsg = data.message || (data.success ? '停止命令已发送' : '停止失败');
                        this.opOk = !!data.success;
                        await this.refreshSurrealStatus();
                    } catch (e) {
                        this.opMsg = '网络错误，无法停止';
                        this.opOk = false;
                    }
                },

                async restartSurreal() {
                    const body = {
                        mode: this.controlMode,
                        ssh: this.controlMode === 'ssh' ? { ...this.ssh } : undefined,
                        bind_ip: this.config.db_ip,
                        bind_port: parseInt(this.config.db_port || '0') || undefined,
                        db_user: this.config.db_user,
                        db_password: this.config.db_password,
                        project_name: this.config.project_name,
                    };
                    try {
                        const res = await fetch('/api/surreal/restart', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: JSON.stringify(body)
                        });
                        const data = await res.json();
                        this.opMsg = data.message || (data.success ? '重启命令已发送' : '重启失败');
                        this.opOk = !!data.success;
                        await this.refreshSurrealStatus();
                    } catch (e) {
                        this.opMsg = '网络错误，无法重启';
                        this.opOk = false;
                    }
                },

                async toggleSurreal() {
                    if (this.surrealStatus.listening) {
                        await this.stopSurreal();
                    } else {
                        await this.startSurreal();
                    }
                },

                async testConnection() {
                    this.testing = true;
                    try {
                        const body = {
                            ip: this.config.db_ip,
                            port: parseInt(this.config.db_port || '0') || 0,
                            user: this.config.db_user,
                            password: this.config.db_password,
                            namespace: String(this.config.surreal_ns || ''),
                            database: this.config.project_name,
                        };
                        const res = await fetch('/api/surreal/test', {
                            method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body)
                        });
                        const data = await res.json();
                        this.opMsg = data.message || (data.success ? '连接成功' : '连接失败');
                        this.opOk = !!data.success;
                    } catch (e) {
                        this.opMsg = '网络错误，连接测试失败';
                        this.opOk = false;
                    } finally {
                        this.testing = false;
                        await this.refreshSurrealStatus();
                    }
                },

                // 选择所有项目
                selectAllProjects() {
                    this.selectedProjects = (this.scanResult?.projects || []).map(p => p.name);
                },

                // 清空项目选择
                clearAllProjects() {
                    this.selectedProjects = [];
                },

                // 加载数据库文件列表
                async loadDatabaseFiles() {
                    if (this.selectedProjects.length === 0) {
                        alert('请先选择项目');
                        this.showDbSelector = false;
                        return;
                    }

                    this.loadingDatabases = true;
                    this.databaseFiles = [];
                    this.scanErrors = [];

                    try {
                        // 扫描每个选中的项目
                        for (const projectName of this.selectedProjects) {
                            const projectPath = `${this.directoryPath}/${projectName}`;
                            const apiUrl = `/api/wizard/scan-database-files?project_path=${encodeURIComponent(projectPath)}&project_name=${encodeURIComponent(projectName)}`;

                            const response = await fetch(apiUrl);

                            if (response.ok) {
                                const result = await response.json();
                                this.databaseFiles.push(...(result.database_files || []));
                                if (result.errors && result.errors.length > 0) {
                                    this.scanErrors.push(...result.errors);
                                }
                            } else {
                                this.scanErrors.push(`扫描项目 ${projectName} 失败: ${response.statusText}`);
                            }
                        }

                        // 去重并排序
                        const uniqueFiles = new Map();
                        this.databaseFiles.forEach(file => {
                            if (!uniqueFiles.has(file.db_num) || uniqueFiles.get(file.db_num).file_size < file.file_size) {
                                uniqueFiles.set(file.db_num, file);
                            }
                        });
                        this.databaseFiles = Array.from(uniqueFiles.values()).sort((a, b) => a.db_num - b.db_num);

                        // 解析当前已选择的数据库编号
                        if (this.manualDbNums) {
                            this.selectedDbNums = this.manualDbNums
                                .split(',')
                                .map(n => parseInt(n.trim()))
                                .filter(n => !isNaN(n));
                        }
                    } catch (error) {
                        this.scanErrors.push(`扫描失败: ${error.message}`);
                        alert(`扫描失败: ${error.message}`);
                    } finally {
                        this.loadingDatabases = false;
                    }
                },

                // 选择所有数据库
                selectAllDatabases() {
                    this.selectedDbNums = this.databaseFiles.map(dbFile => dbFile.db_num);
                },

                // 清空数据库选择
                clearAllDatabases() {
                    this.selectedDbNums = [];
                },

                // 应用选择的数据库
                applySelectedDatabases() {
                    this.manualDbNums = this.selectedDbNums.join(',');
                    this.showDbSelector = false;
                },

                // 创建任务
                async createTask() {
                    this.creatingTask = true;
                    this.taskError = null;

                    try {
                        // 处理手动数据库编号
                        const manualDbNums = this.manualDbNums ?
                            this.manualDbNums.split(',').map(n => parseInt(n.trim())).filter(n => !isNaN(n)) :
                            [];

                        const wizardConfig = {
                            base_config: {
                                name: this.taskName,
                                manual_db_nums: manualDbNums,
                                project_name: this.config.project_name,
                                project_code: this.config.project_code,
                                mdb_name: this.config.mdb_name,
                                module: this.config.module,
                                db_type: this.config.db_type,
                                surreal_ns: this.config.surreal_ns,
                                db_ip: this.config.db_ip,
                                db_port: this.config.db_port,
                                db_user: this.config.db_user,
                                db_password: this.config.db_password,
                                gen_model: this.config.gen_model,
                                gen_mesh: this.config.gen_mesh,
                                gen_spatial_tree: this.config.gen_spatial_tree,
                                apply_boolean_operation: this.config.apply_boolean_operation,
                                mesh_tol_ratio: this.config.mesh_tol_ratio,
                                room_keyword: this.config.room_keyword
                            },
                            selected_projects: this.selectedProjects,
                            root_directory: this.directoryPath,
                            parallel_processing: this.parallelProcessing,
                            max_concurrent: this.parallelProcessing ? this.maxConcurrent : null,
                            continue_on_failure: this.continueOnFailure
                        };

                        const response = await fetch('/api/wizard/create-task', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: JSON.stringify({
                                task_name: this.taskName,
                                wizard_config: wizardConfig,
                                priority: 'Normal'
                            })
                        });

                        if (response.ok) {
                            const result = await response.json();
                            this.createdTaskId = result.id;
                            this.taskCreated = true;
                        } else {
                            throw new Error('任务创建失败');
                        }
                    } catch (error) {
                        console.error('创建任务失败:', error);
                        this.taskError = error.message;
                    } finally {
                        this.creatingTask = false;
                    }
                },

                // 跳转到任务页面
                goToTasks() {
                    window.location.href = '/tasks';
                },

                // 步骤导航
                nextStep() {
                    if (this.currentStep < 4) {
                        this.currentStep++;

                        // 如果从项目选择步骤进入配置步骤，自动更新项目名称
                        if (this.currentStep === 3 && this.selectedProjects.length > 0) {
                            // 创建新的config对象来触发响应式更新
                            this.config = {
                                ...this.config,
                                project_name: this.selectedProjects[0]
                            };
                            // 进入第3步后开始轮询状态
                            this.refreshSurrealStatus();
                            if (!this.statusTimer) {
                                this.statusTimer = setInterval(() => this.refreshSurrealStatus(), 3000);
                            }
                        }
                    }
                },

                prevStep() {
                    if (this.currentStep > 1) {
                        this.currentStep--;
                        if (this.currentStep !== 3 && this.statusTimer) {
                            clearInterval(this.statusTimer);
                            this.statusTimer = null;
                        }
                    }
                },

                // 格式化文件大小
                formatFileSize(bytes) {
                    if (bytes === 0) return '0 B';
                    const k = 1024;
                    const sizes = ['B', 'KB', 'MB', 'GB'];
                    const i = Math.floor(Math.log(bytes) / Math.log(k));
                    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
                },

                // 格式化日期
                formatDate(timestamp) {
                    if (!timestamp) return '未知';
                    try {
                        // 处理不同的时间戳格式
                        let date;
                        if (typeof timestamp === 'object' && timestamp.secs_since_epoch) {
                            // Rust SystemTime 格式
                            date = new Date(timestamp.secs_since_epoch * 1000);
                        } else if (typeof timestamp === 'number') {
                            // Unix 时间戳
                            date = new Date(timestamp * 1000);
                        } else {
                            // 字符串格式
                            date = new Date(timestamp);
                        }
                        return date.toLocaleDateString('zh-CN') + ' ' + date.toLocaleTimeString('zh-CN', {hour12: false});
                    } catch (e) {
                        return '格式错误';
                    }
                }
            }
        }
    </script>
    "#.to_string()
}
