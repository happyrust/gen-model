use axum::{
    response::Html,
    routing::get,
    Router,
};

/// 独立的Web UI服务器
/// 
/// 这个示例展示了一个完全独立的Web UI界面，不依赖任何复杂的库
/// 
/// 使用方法：
/// ```bash
/// cargo run --example standalone_web_ui
/// ```
/// 
/// 然后在浏览器中访问: http://localhost:8080
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    env_logger::init();
    
    println!("🚀 正在启动独立 AIOS Web UI...");
    println!("📋 这是一个独立版本，不依赖复杂的库");
    println!();
    
    let app = Router::new()
        .route("/", get(index_page))
        .route("/dashboard", get(dashboard_page))
        .route("/tasks", get(tasks_page))
        .route("/config", get(config_page))
        .route("/status", get(status_page));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    println!("🚀 Web UI服务器启动成功！");
    println!("📱 访问地址: http://localhost:8080");
    println!("🎯 功能包括:");
    println!("   - 基础页面导航");
    println!("   - 简单的界面展示");
    println!("   - 系统状态查看");
    
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index_page() -> Html<String> {
    Html(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-info-circle mr-2"></i>系统状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <main class="max-w-7xl mx-auto py-6 px-4">
            <!-- 欢迎区域 -->
            <div class="text-center mb-12">
                <h1 class="text-4xl font-bold text-gray-900 mb-4">
                    <i class="fas fa-database text-blue-600 mr-3"></i>
                    AIOS 数据库管理平台
                </h1>
                <p class="text-xl text-gray-600 mb-8">
                    专业的数据库生成和空间树管理系统
                </p>
                <div class="bg-green-100 border border-green-400 text-green-700 px-4 py-3 rounded mb-4">
                    <strong>✅ 系统运行正常</strong> - 独立Web UI已成功启动
                </div>
            </div>

            <!-- 功能卡片 -->
            <div class="grid md:grid-cols-3 gap-8 mb-12">
                <!-- 数据生成卡片 -->
                <div class="bg-white rounded-lg shadow-lg p-6 hover:shadow-xl transition-shadow">
                    <div class="text-center">
                        <div class="bg-blue-100 w-16 h-16 rounded-full flex items-center justify-center mx-auto mb-4">
                            <i class="fas fa-database text-2xl text-blue-600"></i>
                        </div>
                        <h3 class="text-xl font-semibold text-gray-900 mb-2">数据库生成</h3>
                        <p class="text-gray-600 mb-4">生成和管理数据库编号7999的数据</p>
                        <button class="bg-blue-600 text-white px-6 py-2 rounded hover:bg-blue-700 transition" onclick="alert('功能开发中...')">
                            立即执行
                        </button>
                    </div>
                </div>

                <!-- 空间树生成卡片 -->
                <div class="bg-white rounded-lg shadow-lg p-6 hover:shadow-xl transition-shadow">
                    <div class="text-center">
                        <div class="bg-green-100 w-16 h-16 rounded-full flex items-center justify-center mx-auto mb-4">
                            <i class="fas fa-sitemap text-2xl text-green-600"></i>
                        </div>
                        <h3 class="text-xl font-semibold text-gray-900 mb-2">空间树生成</h3>
                        <p class="text-gray-600 mb-4">构建和优化空间关系树结构</p>
                        <a href="/tasks" class="bg-green-600 text-white px-6 py-2 rounded hover:bg-green-700 transition inline-block">
                            查看任务
                        </a>
                    </div>
                </div>

                <!-- 配置管理卡片 -->
                <div class="bg-white rounded-lg shadow-lg p-6 hover:shadow-xl transition-shadow">
                    <div class="text-center">
                        <div class="bg-purple-100 w-16 h-16 rounded-full flex items-center justify-center mx-auto mb-4">
                            <i class="fas fa-cog text-2xl text-purple-600"></i>
                        </div>
                        <h3 class="text-xl font-semibold text-gray-900 mb-2">配置管理</h3>
                        <p class="text-gray-600 mb-4">管理系统配置和参数设置</p>
                        <a href="/config" class="bg-purple-600 text-white px-6 py-2 rounded hover:bg-purple-700 transition inline-block">
                            配置设置
                        </a>
                    </div>
                </div>
            </div>

            <!-- 系统状态 -->
            <div class="bg-white rounded-lg shadow-lg p-6">
                <h2 class="text-2xl font-semibold text-gray-900 mb-4">
                    <i class="fas fa-chart-line text-blue-600 mr-2"></i>
                    系统状态
                </h2>
                <div class="grid md:grid-cols-4 gap-4">
                    <div class="text-center">
                        <div class="text-2xl font-bold text-blue-600">运行中</div>
                        <div class="text-gray-600">系统状态</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-green-600">正常</div>
                        <div class="text-gray-600">Web服务</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-purple-600">0</div>
                        <div class="text-gray-600">活跃任务</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-orange-600">0</div>
                        <div class="text-gray-600">队列任务</div>
                    </div>
                </div>
            </div>

            <!-- 快速操作 -->
            <div class="bg-white rounded-lg shadow-lg p-6 mt-8">
                <h2 class="text-2xl font-semibold text-gray-900 mb-4">
                    <i class="fas fa-bolt text-yellow-600 mr-2"></i>
                    快速操作
                </h2>
                <div class="grid md:grid-cols-3 gap-4">
                    <button onclick="alert('创建任务功能开发中...')" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition text-center">
                        <i class="fas fa-plus mr-2"></i>创建新任务
                    </button>
                    <a href="/config" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700 transition text-center">
                        <i class="fas fa-cog mr-2"></i>配置管理
                    </a>
                    <a href="/status" class="bg-purple-600 text-white px-4 py-2 rounded hover:bg-purple-700 transition text-center">
                        <i class="fas fa-info-circle mr-2"></i>系统状态
                    </a>
                </div>
            </div>
        </main>
    </div>

    <script>
        // 简单的交互功能
        console.log('AIOS Web UI 已加载');
        
        // 定期更新时间
        function updateTime() {
            const now = new Date();
            console.log('当前时间:', now.toLocaleString());
        }
        
        setInterval(updateTime, 60000); // 每分钟更新一次
        updateTime(); // 立即执行一次
    </script>
</body>
</html>
    "#.to_string())
}

async fn dashboard_page() -> Html<String> {
    Html(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>仪表板 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-info-circle mr-2"></i>系统状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-tachometer-alt text-blue-600 mr-3"></i>
                系统仪表板
            </h1>
            
            <!-- 状态卡片 -->
            <div class="grid md:grid-cols-4 gap-6 mb-8">
                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-server text-2xl text-blue-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">系统状态</p>
                            <p class="text-2xl font-semibold text-gray-900">运行中</p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-database text-2xl text-green-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">数据库连接</p>
                            <p class="text-2xl font-semibold text-gray-900">正常</p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-tasks text-2xl text-purple-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">活跃任务</p>
                            <p class="text-2xl font-semibold text-gray-900">0</p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-clock text-2xl text-orange-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">队列任务</p>
                            <p class="text-2xl font-semibold text-gray-900">0</p>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 快速操作 -->
            <div class="bg-white rounded-lg shadow p-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">快速操作</h2>
                <div class="grid md:grid-cols-3 gap-4">
                    <button onclick="alert('创建任务功能开发中...')" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition text-center">
                        <i class="fas fa-plus mr-2"></i>创建新任务
                    </button>
                    <a href="/config" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700 transition text-center">
                        <i class="fas fa-cog mr-2"></i>配置管理
                    </a>
                    <a href="/status" class="bg-purple-600 text-white px-4 py-2 rounded hover:bg-purple-700 transition text-center">
                        <i class="fas fa-info-circle mr-2"></i>系统状态
                    </a>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#.to_string())
}

async fn tasks_page() -> Html<String> {
    Html(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>任务管理 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-info-circle mr-2"></i>系统状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-tasks text-blue-600 mr-3"></i>
                任务管理
            </h1>

            <!-- 任务创建 -->
            <div class="bg-white rounded-lg shadow p-6 mb-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">创建新任务</h2>
                <div class="grid md:grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">任务类型</label>
                        <select class="w-full border border-gray-300 rounded-md px-3 py-2">
                            <option>数据库生成</option>
                            <option>空间树生成</option>
                            <option>网格生成</option>
                            <option>数据解析</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">数据库编号</label>
                        <input type="text" placeholder="例如: 7999" class="w-full border border-gray-300 rounded-md px-3 py-2">
                    </div>
                </div>
                <div class="mt-4">
                    <button onclick="alert('任务创建功能开发中...')" class="bg-blue-600 text-white px-6 py-2 rounded hover:bg-blue-700 transition">
                        <i class="fas fa-plus mr-2"></i>创建任务
                    </button>
                </div>
            </div>

            <!-- 任务列表 -->
            <div class="bg-white rounded-lg shadow p-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">任务列表</h2>
                <div class="text-center py-8">
                    <i class="fas fa-inbox text-4xl text-gray-400 mb-4"></i>
                    <p class="text-gray-600">暂无任务</p>
                    <p class="text-sm text-gray-500 mt-2">创建新任务开始使用系统</p>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#.to_string())
}

async fn config_page() -> Html<String> {
    Html(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>配置管理 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
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
                        <a href="/config" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-info-circle mr-2"></i>系统状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-cog text-blue-600 mr-3"></i>
                配置管理
            </h1>

            <!-- 数据库配置 -->
            <div class="bg-white rounded-lg shadow p-6 mb-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">数据库配置</h2>
                <div class="grid md:grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">SurrealDB 地址</label>
                        <input type="text" value="ws://localhost:8009" class="w-full border border-gray-300 rounded-md px-3 py-2">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">数据库名称</label>
                        <input type="text" value="AvevaMarineSample" class="w-full border border-gray-300 rounded-md px-3 py-2">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">用户名</label>
                        <input type="text" value="root" class="w-full border border-gray-300 rounded-md px-3 py-2">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">密码</label>
                        <input type="password" value="root" class="w-full border border-gray-300 rounded-md px-3 py-2">
                    </div>
                </div>
                <div class="mt-4">
                    <button onclick="alert('配置保存功能开发中...')" class="bg-blue-600 text-white px-6 py-2 rounded hover:bg-blue-700 transition">
                        <i class="fas fa-save mr-2"></i>保存配置
                    </button>
                </div>
            </div>

            <!-- 系统参数 -->
            <div class="bg-white rounded-lg shadow p-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">系统参数</h2>
                <div class="grid md:grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">网格容差</label>
                        <input type="number" value="0.001" step="0.001" class="w-full border border-gray-300 rounded-md px-3 py-2">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">最大并发任务</label>
                        <input type="number" value="4" class="w-full border border-gray-300 rounded-md px-3 py-2">
                    </div>
                </div>
                <div class="mt-4">
                    <button onclick="alert('参数保存功能开发中...')" class="bg-green-600 text-white px-6 py-2 rounded hover:bg-green-700 transition">
                        <i class="fas fa-save mr-2"></i>保存参数
                    </button>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#.to_string())
}

async fn status_page() -> Html<String> {
    Html(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>系统状态 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
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
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/status" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-info-circle mr-2"></i>系统状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-info-circle text-blue-600 mr-3"></i>
                系统状态
            </h1>

            <!-- 系统信息 -->
            <div class="bg-white rounded-lg shadow p-6 mb-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">系统信息</h2>
                <div class="grid md:grid-cols-2 gap-4">
                    <div class="border-l-4 border-blue-500 pl-4">
                        <p class="text-sm text-gray-600">系统版本</p>
                        <p class="text-lg font-semibold">AIOS v0.1.3</p>
                    </div>
                    <div class="border-l-4 border-green-500 pl-4">
                        <p class="text-sm text-gray-600">运行时间</p>
                        <p class="text-lg font-semibold">刚刚启动</p>
                    </div>
                    <div class="border-l-4 border-purple-500 pl-4">
                        <p class="text-sm text-gray-600">Web UI 端口</p>
                        <p class="text-lg font-semibold">8080</p>
                    </div>
                    <div class="border-l-4 border-orange-500 pl-4">
                        <p class="text-sm text-gray-600">数据库状态</p>
                        <p class="text-lg font-semibold text-red-600">未连接</p>
                    </div>
                </div>
            </div>

            <!-- 服务状态 -->
            <div class="bg-white rounded-lg shadow p-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">服务状态</h2>
                <div class="space-y-4">
                    <div class="flex items-center justify-between p-4 border rounded-lg">
                        <div class="flex items-center">
                            <i class="fas fa-globe text-blue-600 mr-3"></i>
                            <span class="font-medium">Web UI 服务</span>
                        </div>
                        <span class="bg-green-100 text-green-800 px-3 py-1 rounded-full text-sm">运行中</span>
                    </div>
                    <div class="flex items-center justify-between p-4 border rounded-lg">
                        <div class="flex items-center">
                            <i class="fas fa-database text-purple-600 mr-3"></i>
                            <span class="font-medium">SurrealDB 连接</span>
                        </div>
                        <span class="bg-red-100 text-red-800 px-3 py-1 rounded-full text-sm">未连接</span>
                    </div>
                    <div class="flex items-center justify-between p-4 border rounded-lg">
                        <div class="flex items-center">
                            <i class="fas fa-tasks text-green-600 mr-3"></i>
                            <span class="font-medium">任务管理器</span>
                        </div>
                        <span class="bg-yellow-100 text-yellow-800 px-3 py-1 rounded-full text-sm">待启动</span>
                    </div>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#.to_string())
}
