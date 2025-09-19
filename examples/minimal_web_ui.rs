use axum::{Router, response::Html, routing::get};

/// 最小化的Web UI服务器
///
/// 这个示例展示了一个最基本的Web UI界面，用于测试基础功能
///
/// 使用方法：
/// ```bash
/// cargo run --example minimal_web_ui --features "web_ui"
/// ```
///
/// 然后在浏览器中访问: http://localhost:8080
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    env_logger::init();

    println!("🚀 正在启动最小化 AIOS Web UI...");
    println!("📋 这是一个基础版本，用于测试连接");
    println!();

    let app = Router::new()
        .route("/", get(index_page))
        .route("/dashboard", get(dashboard_page))
        .route("/tasks", get(tasks_page))
        .route("/config", get(config_page));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    println!("🚀 Web UI服务器启动成功！");
    println!("📱 访问地址: http://localhost:8080");
    println!("🎯 功能包括:");
    println!("   - 基础页面导航");
    println!("   - 简单的界面展示");

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
                    <strong>✅ 系统运行正常</strong> - 最小化Web UI已成功启动
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
        </main>
    </div>
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
                    </div>
                </div>
            </div>
        </nav>
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-tachometer-alt text-blue-600 mr-3"></i>
                系统仪表板
            </h1>
            <div class="bg-white rounded-lg shadow p-6">
                <p class="text-gray-600">仪表板功能正在开发中...</p>
                <div class="mt-4">
                    <a href="/" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition">
                        返回首页
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
                    </div>
                </div>
            </div>
        </nav>
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-tasks text-blue-600 mr-3"></i>
                任务管理
            </h1>
            <div class="bg-white rounded-lg shadow p-6">
                <p class="text-gray-600">任务管理功能正在开发中...</p>
                <div class="mt-4">
                    <a href="/" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition">
                        返回首页
                    </a>
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
                    </div>
                </div>
            </div>
        </nav>
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-cog text-blue-600 mr-3"></i>
                配置管理
            </h1>
            <div class="bg-white rounded-lg shadow p-6">
                <p class="text-gray-600">配置管理功能正在开发中...</p>
                <div class="mt-4">
                    <a href="/" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition">
                        返回首页
                    </a>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#.to_string())
}
