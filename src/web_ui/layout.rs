/// 统一页面布局（顶部导航 + 左侧栏 + 主内容）
/// 使用 simple-tailwind.css + ui.css + simple-icons.css
/// active_nav 取值建议："home"|"dashboard"|"tasks"|"batch"|"config"|"db-status"|"sqlite-spatial"|"wizard"|"db-conn"
pub fn render_layout_with_sidebar(
    title: &str,
    active_nav: Option<&str>,
    content_html: &str,
    extra_head: Option<&str>,
    extra_scripts: Option<&str>,
) -> String {
    let is_active = |key: &str| -> &'static str {
        match active_nav {
            Some(k) if k == key => "active",
            _ => "",
        }
    };

    let tpl = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{{title}}</title>
  <link href="/static/simple-tailwind.css" rel="stylesheet">
  <link href="/static/simple-icons.css" rel="stylesheet">
  <link href="/static/ui.css" rel="stylesheet">
  {{extra_head}}
</head>
<body class="bg-gray-50">
  <!-- 顶部导航 -->
  <header class="nav-gradient app-topnav">
    <div class="app-container flex justify-between items-center" style="padding-top:0;padding-bottom:0;">
      <div class="app-brand">
        <i class="fas fa-database"></i>
        <span>AIOS 数据库管理平台</span>
      </div>
    </div>
  </header>

  <div class="app-shell">
    <!-- 左侧栏 -->
    <aside class="app-sidebar">
      <div class="sidebar-header">
        <span class="text-sm text-gray-300">导航</span>
      </div>
      <div class="nav-group">常用</div>
      <nav class="nav-list">
        <a class="nav-item {home_active}" href="/"><span class="icon fas fa-home app-icon"></span> 首页</a>
        <a class="nav-item {dashboard_active}" href="/dashboard"><span class="icon fas fa-tachometer-alt app-icon"></span> 仪表板</a>
        <a class="nav-item {tasks_active}" href="/tasks"><span class="icon fas fa-tasks app-icon"></span> 任务管理</a>
        <a class="nav-item {batch_active}" href="/batch-tasks"><span class="icon fas fa-layer-group app-icon"></span> 批量任务</a>
        <a class="nav-item {deploy_active}" href="/deployment-sites"><span class="icon fas fa-server app-icon"></span> 部署站点</a>
      </nav>
      <div class="nav-group">系统</div>
      <nav class="nav-list">
        <a class="nav-item {config_active}" href="/config"><span class="icon fas fa-cog app-icon"></span> 配置管理</a>
        <a class="nav-item {dbstatus_active}" href="/db-status"><span class="icon fas fa-database app-icon"></span> 系统状态</a>
        <a class="nav-item {spatial_active}" href="/sqlite-spatial"><span class="icon fas fa-vector-square app-icon"></span> 空间查询</a>
        <a class="nav-item {wizard_active}" href="/wizard"><span class="icon fas fa-magic app-icon"></span> 解析向导</a>
        <a class="nav-item {remote_active}" href="/remote-sync"><span class="icon fas fa-project-diagram app-icon"></span> 异地环境</a>
      </nav>
      <div class="nav-group">工具</div>
      <nav class="nav-list">
        <a class="nav-item {dbconn_active}" href="/database-connection"><span class="icon fas fa-plug app-icon"></span> 数据库连接</a>
        <a class="nav-item {xkt_test_active}" href="/xkt-test"><span class="icon fas fa-cube app-icon"></span> XKT 模型测试</a>
      </nav>
    </aside>

    <!-- 主内容区域 -->
    <main class="app-main">
      <div class="app-container">
        {content}
      </div>
    </main>
  </div>

  {extra_scripts}
</body>
</html>
"#;

    tpl.replace("{{title}}", title)
        .replace("{{extra_head}}", extra_head.unwrap_or(""))
        .replace("{home_active}", is_active("home"))
        .replace("{dashboard_active}", is_active("dashboard"))
        .replace("{tasks_active}", is_active("tasks"))
        .replace("{batch_active}", is_active("batch"))
        .replace("{config_active}", is_active("config"))
        .replace("{deploy_active}", is_active("deploy-sites"))
        .replace("{dbstatus_active}", is_active("db-status"))
        .replace("{spatial_active}", is_active("sqlite-spatial"))
        .replace("{wizard_active}", is_active("wizard"))
        .replace("{remote_active}", is_active("remote-sync"))
        .replace("{dbconn_active}", is_active("db-conn"))
        .replace("{xkt_test_active}", is_active("xkt-test"))
        .replace("{content}", content_html)
        .replace("{extra_scripts}", extra_scripts.unwrap_or(""))
}

/// 简单从完整 HTML 中提取 <style>...</style> 片段（全部拼接）
pub fn extract_inline_styles(full_html: &str) -> String {
    let mut styles = String::new();
    let mut rest = full_html;
    while let Some(open_idx) = rest.find("<style") {
        let after_open = &rest[open_idx..];
        if let Some(gt_idx) = after_open.find('>') {
            let content_start = open_idx + gt_idx + 1;
            if let Some(close_rel) = rest[content_start..].find("</style>") {
                let close_abs = content_start + close_rel + "</style>".len();
                styles.push_str(&rest[open_idx..close_abs]);
                rest = &rest[close_abs..];
                continue;
            }
        }
        break;
    }
    styles
}

/// 提取 <body> 内部 HTML 内容
pub fn extract_body_inner(full_html: &str) -> String {
    if let Some(body_open) = full_html.find("<body") {
        if let Some(gt) = full_html[body_open..].find('>') {
            let inner_start = body_open + gt + 1;
            if let Some(close) = full_html[inner_start..].rfind("</body>") {
                let inner_end = inner_start + close;
                return full_html[inner_start..inner_end].to_string();
            }
        }
    }
    full_html.to_string()
}

/// 将内联样式做简单作用域收敛，避免污染全局：
/// - 把 `body {` 或 `body{` 替换为 `.wrapped-page {`
pub fn scope_inline_styles(inline_styles: &str) -> String {
    inline_styles
        .replace("body {", ".wrapped-page {")
        .replace("body{", ".wrapped-page{")
}

/// 从内容中移除第一个指定标签块（如 <nav>...</nav>）
pub fn strip_first_tag_block(content: &str, tag: &str) -> String {
    let open_pat = format!("<{}", tag);
    let close_pat = format!("</{}>", tag);
    if let Some(open) = content.find(&open_pat) {
        if let Some(close_rel) = content[open..].find(&close_pat) {
            let close_abs = open + close_rel + close_pat.len();
            let mut out = String::with_capacity(content.len());
            out.push_str(&content[..open]);
            out.push_str(&content[close_abs..]);
            return out;
        }
    }
    content.to_string()
}

/// 包装外部完整 HTML（文件或字符串）到统一布局：
/// - 提取并注入 <style> 片段到 <head>
/// - 提取 <body> 内的内容，并移除第一个 <nav> 区块
pub fn wrap_external_html_in_layout(
    title: &str,
    active_nav: Option<&str>,
    full_html: &str,
) -> String {
    let extra_head = scope_inline_styles(&extract_inline_styles(full_html));
    let mut content = extract_body_inner(full_html);
    content = strip_first_tag_block(&content, "nav");
    let wrapped_content = format!("<div class=\"wrapped-page\">{}</div>", content);
    render_layout_with_sidebar(title, active_nav, &wrapped_content, Some(&extra_head), None)
}
