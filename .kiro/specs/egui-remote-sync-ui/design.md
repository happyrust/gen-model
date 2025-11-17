# Design Document

## Overview

本设计文档描述了基于 egui 0.33 + glow 的异地协同运维原生界面的技术架构、组件设计和实现方案。该应用将提供独立的桌面 GUI，复刻现有 Next.js Web UI 的核心功能，并与后端 Rust API 无缝集成。

### 设计目标

1. **原生性能** - 使用 egui 即时模式 GUI，提供流畅的 60fps 渲染
2. **跨平台** - 支持 Windows、macOS、Linux 三大平台
3. **独立部署** - 单一可执行文件，无需浏览器和 Node.js 环境
4. **API 集成** - 复用现有的 Axum REST API，无需修改后端
5. **可视化** - 提供拓扑画布编辑器和实时监控图表

### 技术栈

**GUI 框架**
- egui 0.33 (即时模式 GUI)
- eframe (egui 的应用框架)
- glow (OpenGL 渲染后端)
- egui_extras (表格、图表等扩展组件)
- egui_plot (性能图表)

**网络和数据**
- reqwest (HTTP 客户端，调用 REST API)
- tokio (异步运行时)
- serde_json (JSON 序列化)
- rusqlite (SQLite 数据库访问)

**其他依赖**
- chrono (时间处理)
- anyhow (错误处理)
- tracing (日志记录)
- rfd (原生文件对话框)

## Architecture

### 应用架构

```
┌─────────────────────────────────────────────────────────────────┐
│                      EguiRemoteSyncApp                          │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ AppState (全局状态)                                       │  │
│  │ - current_page: Page                                     │  │
│  │ - environments: Vec<RemoteSyncEnv>                       │  │
│  │ - sites: Vec<RemoteSyncSite>                             │  │
│  │ - sync_tasks: Vec<SyncTask>                              │  │
│  │ - web_server_status: ServerStatus                        │  │
│  │ - api_client: ApiClient                                  │  │
│  └──────────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ UI 组件层                                                 │  │
│  │ - MainWindow (主窗口)                                     │  │
│  │ - NavigationPanel (导航栏)                                │  │
│  │ - EnvironmentListPage (环境列表)                          │  │
│  │ - TopologyCanvasPage (拓扑画布)                           │  │
│  │ - MonitorDashboardPage (监控面板)                         │  │
│  │ - LogQueryPage (日志查询)                                 │  │
│  │ - WebServerPage (服务器管理)                              │  │
│  │ - DeploymentPage (部署页面)                               │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ HTTP REST API
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Axum Web Server                            │
│  /api/remote-sync/envs                                          │
│  /api/remote-sync/sites                                         │
│  /api/sync/start, /api/sync/stop                                │
│  /api/topology/save, /api/topology/load                         │
└─────────────────────────────────────────────────────────────────┘
```

### 目录结构

```
src/
├── gui/                          # GUI 模块根目录
│   ├── mod.rs                    # 模块导出
│   ├── app.rs                    # 主应用结构
│   ├── state.rs                  # 全局状态管理
│   ├── api_client.rs             # API 客户端
│   ├── pages/                    # 页面组件
│   │   ├── mod.rs
│   │   ├── environment_list.rs   # 环境列表页
│   │   ├── topology_canvas.rs    # 拓扑画布页
│   │   ├── monitor_dashboard.rs  # 监控面板页
│   │   ├── log_query.rs          # 日志查询页
│   │   ├── web_server.rs         # 服务器管理页
│   │   └── deployment.rs         # 部署页面
│   ├── components/               # 可复用组件
│   │   ├── mod.rs
│   │   ├── env_form.rs           # 环境配置表单
│   │   ├── site_form.rs          # 站点配置表单
│   │   ├── task_list.rs          # 任务列表
│   │   ├── status_card.rs        # 状态卡片
│   │   ├── confirm_dialog.rs     # 确认对话框
│   │   └── toast.rs              # Toast 提示
│   ├── canvas/                   # 拓扑画布相关
│   │   ├── mod.rs
│   │   ├── node.rs               # 节点定义
│   │   ├── edge.rs               # 连线定义
│   │   ├── layout.rs             # 布局算法
│   │   └── renderer.rs           # 画布渲染
│   └── theme.rs                  # 主题和样式
└── bin/
    └── egui_remote_sync.rs       # 应用入口
```


## Components and Interfaces

### 1. 主应用结构 (EguiRemoteSyncApp)

```rust
pub struct EguiRemoteSyncApp {
    state: AppState,
    api_client: ApiClient,
    current_page: Page,
    navigation_panel: NavigationPanel,
    toast_manager: ToastManager,
    theme: Theme,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Page {
    EnvironmentList,
    TopologyCanvas,
    MonitorDashboard,
    LogQuery,
    WebServer,
    Deployment,
    Settings,
}

impl eframe::App for EguiRemoteSyncApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 顶部菜单栏
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            self.render_menu_bar(ui);
        });
        
        // 左侧导航栏
        egui::SidePanel::left("navigation").show(ctx, |ui| {
            self.navigation_panel.render(ui, &mut self.current_page);
        });
        
        // 主内容区域
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.current_page {
                Page::EnvironmentList => self.render_environment_list(ui),
                Page::TopologyCanvas => self.render_topology_canvas(ui),
                Page::MonitorDashboard => self.render_monitor_dashboard(ui),
                Page::LogQuery => self.render_log_query(ui),
                Page::WebServer => self.render_web_server(ui),
                Page::Deployment => self.render_deployment(ui),
                Page::Settings => self.render_settings(ui),
            }
        });
        
        // Toast 提示
        self.toast_manager.render(ctx);
        
        // 定时刷新（监控面板）
        if self.current_page == Page::MonitorDashboard {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }
    
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // 保存窗口布局和当前页面
        eframe::set_value(storage, "current_page", &self.current_page);
        eframe::set_value(storage, "theme", &self.theme);
    }
}
```

### 2. 全局状态管理 (AppState)

```rust
pub struct AppState {
    pub environments: Vec<RemoteSyncEnv>,
    pub sites: Vec<RemoteSyncSite>,
    pub sync_tasks: Vec<SyncTask>,
    pub sync_logs: Vec<SyncLog>,
    pub web_server_status: ServerStatus,
    pub topology: TopologyData,
    pub loading: bool,
    pub error: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            environments: Vec::new(),
            sites: Vec::new(),
            sync_tasks: Vec::new(),
            sync_logs: Vec::new(),
            web_server_status: ServerStatus::Stopped,
            topology: TopologyData::default(),
            loading: false,
            error: None,
        }
    }
    
    pub async fn load_from_api(&mut self, api_client: &ApiClient) -> anyhow::Result<()> {
        self.loading = true;
        self.error = None;
        
        match api_client.get_environments().await {
            Ok(envs) => self.environments = envs,
            Err(e) => self.error = Some(format!("加载环境失败: {}", e)),
        }
        
        match api_client.get_sites().await {
            Ok(sites) => self.sites = sites,
            Err(e) => self.error = Some(format!("加载站点失败: {}", e)),
        }
        
        self.loading = false;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Stopped,
    Starting,
    Running { address: String },
    Stopping,
    Error(String),
}
```

### 3. API 客户端 (ApiClient)

```rust
pub struct ApiClient {
    base_url: String,
    client: reqwest::Client,
}

impl ApiClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        }
    }
    
    // 环境管理 API
    pub async fn get_environments(&self) -> anyhow::Result<Vec<RemoteSyncEnv>> {
        let url = format!("{}/api/remote-sync/envs", self.base_url);
        let response = self.client.get(&url).send().await?;
        let data: Vec<RemoteSyncEnv> = response.json().await?;
        Ok(data)
    }
    
    pub async fn create_environment(&self, env: &RemoteSyncEnv) -> anyhow::Result<String> {
        let url = format!("{}/api/remote-sync/envs", self.base_url);
        let response = self.client.post(&url).json(env).send().await?;
        let result: serde_json::Value = response.json().await?;
        Ok(result["id"].as_str().unwrap().to_string())
    }
    
    pub async fn update_environment(&self, id: &str, env: &RemoteSyncEnv) -> anyhow::Result<()> {
        let url = format!("{}/api/remote-sync/envs/{}", self.base_url, id);
        self.client.put(&url).json(env).send().await?;
        Ok(())
    }
    
    pub async fn delete_environment(&self, id: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/remote-sync/envs/{}", self.base_url, id);
        self.client.delete(&url).send().await?;
        Ok(())
    }
    
    pub async fn activate_environment(&self, id: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/remote-sync/envs/{}/activate", self.base_url, id);
        self.client.post(&url).send().await?;
        Ok(())
    }
    
    // 站点管理 API
    pub async fn get_sites(&self) -> anyhow::Result<Vec<RemoteSyncSite>> {
        let url = format!("{}/api/remote-sync/sites", self.base_url);
        let response = self.client.get(&url).send().await?;
        let data: Vec<RemoteSyncSite> = response.json().await?;
        Ok(data)
    }
    
    pub async fn create_site(&self, site: &RemoteSyncSite) -> anyhow::Result<String> {
        let url = format!("{}/api/remote-sync/sites", self.base_url);
        let response = self.client.post(&url).json(site).send().await?;
        let result: serde_json::Value = response.json().await?;
        Ok(result["id"].as_str().unwrap().to_string())
    }
    
    pub async fn test_site_connection(&self, id: &str) -> anyhow::Result<TestResult> {
        let url = format!("{}/api/remote-sync/sites/{}/test", self.base_url, id);
        let response = self.client.post(&url).send().await?;
        let result: TestResult = response.json().await?;
        Ok(result)
    }
    
    // 同步控制 API
    pub async fn start_sync(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/sync/start", self.base_url);
        self.client.post(&url).send().await?;
        Ok(())
    }
    
    pub async fn stop_sync(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/sync/stop", self.base_url);
        self.client.post(&url).send().await?;
        Ok(())
    }
    
    pub async fn pause_sync(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/sync/pause", self.base_url);
        self.client.post(&url).send().await?;
        Ok(())
    }
    
    pub async fn resume_sync(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/sync/resume", self.base_url);
        self.client.post(&url).send().await?;
        Ok(())
    }
    
    pub async fn clear_queue(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/sync/queue/clear", self.base_url);
        self.client.post(&url).send().await?;
        Ok(())
    }
    
    pub async fn get_sync_status(&self) -> anyhow::Result<SyncStatus> {
        let url = format!("{}/api/sync/status", self.base_url);
        let response = self.client.get(&url).send().await?;
        let status: SyncStatus = response.json().await?;
        Ok(status)
    }
    
    // 日志查询 API
    pub async fn query_logs(&self, filters: &LogFilters) -> anyhow::Result<Vec<SyncLog>> {
        let url = format!("{}/api/remote-sync/logs", self.base_url);
        let response = self.client.get(&url).query(filters).send().await?;
        let logs: Vec<SyncLog> = response.json().await?;
        Ok(logs)
    }
    
    // 拓扑配置 API
    pub async fn save_topology(&self, topology: &TopologyData) -> anyhow::Result<()> {
        let url = format!("{}/api/topology/save", self.base_url);
        self.client.post(&url).json(topology).send().await?;
        Ok(())
    }
    
    pub async fn load_topology(&self) -> anyhow::Result<TopologyData> {
        let url = format!("{}/api/topology/load", self.base_url);
        let response = self.client.get(&url).send().await?;
        let topology: TopologyData = response.json().await?;
        Ok(topology)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    pub running: bool,
    pub paused: bool,
    pub queue_size: usize,
    pub active_tasks: usize,
    pub mqtt_connected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogFilters {
    pub env_id: Option<String>,
    pub site_id: Option<String>,
    pub status: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub limit: Option<usize>,
}
```


### 4. 环境列表页面 (EnvironmentListPage)

```rust
pub struct EnvironmentListPage {
    selected_env: Option<String>,
    show_env_form: bool,
    env_form: EnvironmentForm,
    show_confirm_delete: bool,
    delete_target: Option<String>,
}

impl EnvironmentListPage {
    pub fn render(&mut self, ui: &mut egui::Ui, state: &mut AppState, api_client: &ApiClient) {
        ui.heading("异地协同环境");
        
        ui.horizontal(|ui| {
            if ui.button("➕ 添加环境").clicked() {
                self.show_env_form = true;
                self.env_form = EnvironmentForm::new();
            }
            
            if ui.button("🔄 刷新").clicked() {
                // 异步加载环境列表
                let api_client = api_client.clone();
                tokio::spawn(async move {
                    // 刷新逻辑
                });
            }
        });
        
        ui.separator();
        
        // 环境列表表格
        use egui_extras::{TableBuilder, Column};
        
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::auto().at_least(200.0)) // 名称
            .column(Column::auto().at_least(150.0)) // MQTT 地址
            .column(Column::auto().at_least(150.0)) // 文件服务器
            .column(Column::auto().at_least(100.0)) // 地区
            .column(Column::auto().at_least(100.0)) // 状态
            .column(Column::remainder())            // 操作
            .header(20.0, |mut header| {
                header.col(|ui| { ui.strong("环境名称"); });
                header.col(|ui| { ui.strong("MQTT 地址"); });
                header.col(|ui| { ui.strong("文件服务器"); });
                header.col(|ui| { ui.strong("地区"); });
                header.col(|ui| { ui.strong("状态"); });
                header.col(|ui| { ui.strong("操作"); });
            })
            .body(|mut body| {
                for env in &state.environments {
                    body.row(30.0, |mut row| {
                        row.col(|ui| { ui.label(&env.name); });
                        row.col(|ui| {
                            if let Some(host) = &env.mqtt_host {
                                ui.label(format!("{}:{}", host, env.mqtt_port.unwrap_or(1883)));
                            }
                        });
                        row.col(|ui| {
                            if let Some(host) = &env.file_server_host {
                                ui.label(host);
                            }
                        });
                        row.col(|ui| { ui.label(&env.location); });
                        row.col(|ui| {
                            // 状态指示器
                            let (color, text) = if env.id == "active" {
                                (egui::Color32::GREEN, "🟢 运行中")
                            } else {
                                (egui::Color32::GRAY, "⚪ 未激活")
                            };
                            ui.colored_label(color, text);
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                if ui.small_button("编辑").clicked() {
                                    self.env_form = EnvironmentForm::from_env(env);
                                    self.show_env_form = true;
                                }
                                if ui.small_button("激活").clicked() {
                                    // 激活环境
                                }
                                if ui.small_button("删除").clicked() {
                                    self.delete_target = Some(env.id.clone());
                                    self.show_confirm_delete = true;
                                }
                            });
                        });
                    });
                }
            });
        
        // 环境配置表单对话框
        if self.show_env_form {
            egui::Window::new("环境配置")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    self.env_form.render(ui);
                    
                    ui.horizontal(|ui| {
                        if ui.button("保存").clicked() {
                            // 保存环境
                            self.show_env_form = false;
                        }
                        if ui.button("取消").clicked() {
                            self.show_env_form = false;
                        }
                    });
                });
        }
        
        // 删除确认对话框
        if self.show_confirm_delete {
            egui::Window::new("确认删除")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("确定要删除这个环境吗？");
                    ui.label("⚠️ 关联的站点也将被删除");
                    
                    ui.horizontal(|ui| {
                        if ui.button("确认删除").clicked() {
                            // 删除环境
                            self.show_confirm_delete = false;
                            self.delete_target = None;
                        }
                        if ui.button("取消").clicked() {
                            self.show_confirm_delete = false;
                            self.delete_target = None;
                        }
                    });
                });
        }
    }
}

pub struct EnvironmentForm {
    pub id: Option<String>,
    pub name: String,
    pub mqtt_host: String,
    pub mqtt_port: String,
    pub file_server_host: String,
    pub location: String,
    pub location_dbs: String,
    pub errors: HashMap<String, String>,
}

impl EnvironmentForm {
    pub fn new() -> Self {
        Self {
            id: None,
            name: String::new(),
            mqtt_host: String::new(),
            mqtt_port: "1883".to_string(),
            file_server_host: String::new(),
            location: String::new(),
            location_dbs: String::new(),
            errors: HashMap::new(),
        }
    }
    
    pub fn from_env(env: &RemoteSyncEnv) -> Self {
        Self {
            id: Some(env.id.clone()),
            name: env.name.clone(),
            mqtt_host: env.mqtt_host.clone().unwrap_or_default(),
            mqtt_port: env.mqtt_port.map(|p| p.to_string()).unwrap_or_else(|| "1883".to_string()),
            file_server_host: env.file_server_host.clone().unwrap_or_default(),
            location: env.location.clone(),
            location_dbs: env.location_dbs.clone().unwrap_or_default(),
            errors: HashMap::new(),
        }
    }
    
    pub fn render(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("env_form_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("环境名称 *");
                ui.text_edit_singleline(&mut self.name);
                ui.end_row();
                
                if let Some(error) = self.errors.get("name") {
                    ui.label("");
                    ui.colored_label(egui::Color32::RED, error);
                    ui.end_row();
                }
                
                ui.label("MQTT 主机");
                ui.text_edit_singleline(&mut self.mqtt_host);
                ui.end_row();
                
                ui.label("MQTT 端口");
                ui.text_edit_singleline(&mut self.mqtt_port);
                ui.end_row();
                
                ui.label("文件服务器地址");
                ui.text_edit_singleline(&mut self.file_server_host);
                ui.end_row();
                
                ui.label("地区标识");
                ui.text_edit_singleline(&mut self.location);
                ui.end_row();
                
                ui.label("数据库编号");
                ui.text_edit_singleline(&mut self.location_dbs);
                ui.end_row();
                
                ui.label("");
                ui.label("(逗号分隔，如: 7999,8001,8002)");
                ui.end_row();
            });
    }
    
    pub fn validate(&mut self) -> bool {
        self.errors.clear();
        
        if self.name.trim().is_empty() {
            self.errors.insert("name".to_string(), "环境名称不能为空".to_string());
        }
        
        if !self.mqtt_port.is_empty() {
            if let Ok(port) = self.mqtt_port.parse::<u16>() {
                if port == 0 {
                    self.errors.insert("mqtt_port".to_string(), "端口号必须大于 0".to_string());
                }
            } else {
                self.errors.insert("mqtt_port".to_string(), "端口号格式不正确".to_string());
            }
        }
        
        self.errors.is_empty()
    }
    
    pub fn to_env(&self) -> RemoteSyncEnv {
        RemoteSyncEnv {
            id: self.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name: self.name.clone(),
            mqtt_host: if self.mqtt_host.is_empty() { None } else { Some(self.mqtt_host.clone()) },
            mqtt_port: self.mqtt_port.parse().ok(),
            file_server_host: if self.file_server_host.is_empty() { None } else { Some(self.file_server_host.clone()) },
            location: self.location.clone(),
            location_dbs: if self.location_dbs.is_empty() { None } else { Some(self.location_dbs.clone()) },
            reconnect_initial_ms: Some(1000),
            reconnect_max_ms: Some(30000),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}
```


### 5. 拓扑画布页面 (TopologyCanvasPage)

```rust
pub struct TopologyCanvasPage {
    canvas: TopologyCanvas,
    selected_node: Option<NodeId>,
    show_node_config: bool,
    node_config_panel: NodeConfigPanel,
    mode: CanvasMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CanvasMode {
    Select,
    AddEnvironment,
    AddSite,
    Connect,
}

impl TopologyCanvasPage {
    pub fn render(&mut self, ui: &mut egui::Ui, state: &mut AppState, api_client: &ApiClient) {
        ui.heading("拓扑配置");
        
        // 工具栏
        ui.horizontal(|ui| {
            if ui.selectable_label(self.mode == CanvasMode::Select, "🖱️ 选择").clicked() {
                self.mode = CanvasMode::Select;
            }
            if ui.selectable_label(self.mode == CanvasMode::AddEnvironment, "➕ 添加环境").clicked() {
                self.mode = CanvasMode::AddEnvironment;
            }
            if ui.selectable_label(self.mode == CanvasMode::AddSite, "➕ 添加站点").clicked() {
                self.mode = CanvasMode::AddSite;
            }
            if ui.selectable_label(self.mode == CanvasMode::Connect, "🔗 连接").clicked() {
                self.mode = CanvasMode::Connect;
            }
            
            ui.separator();
            
            if ui.button("🔄 自动布局").clicked() {
                self.canvas.auto_layout();
            }
            if ui.button("💾 保存拓扑").clicked() {
                // 保存拓扑到后端
                let topology = self.canvas.to_topology_data();
                let api_client = api_client.clone();
                tokio::spawn(async move {
                    let _ = api_client.save_topology(&topology).await;
                });
            }
            if ui.button("📥 导入 JSON").clicked() {
                // 打开文件对话框
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .pick_file()
                {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        if let Ok(topology) = serde_json::from_str::<TopologyData>(&content) {
                            self.canvas.load_from_topology(&topology);
                        }
                    }
                }
            }
            if ui.button("📤 导出 JSON").clicked() {
                // 保存文件对话框
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .save_file()
                {
                    let topology = self.canvas.to_topology_data();
                    if let Ok(json) = serde_json::to_string_pretty(&topology) {
                        let _ = std::fs::write(path, json);
                    }
                }
            }
        });
        
        ui.separator();
        
        // 画布区域
        let canvas_response = egui::Frame::canvas(ui.style())
            .show(ui, |ui| {
                self.canvas.render(ui, self.mode);
            });
        
        // 处理画布交互
        if let Some(node_id) = self.canvas.get_selected_node() {
            self.selected_node = Some(node_id);
            self.show_node_config = true;
        }
        
        // 节点配置面板（右侧）
        if self.show_node_config {
            egui::SidePanel::right("node_config")
                .default_width(300.0)
                .show(ui.ctx(), |ui| {
                    if let Some(node_id) = self.selected_node {
                        self.node_config_panel.render(ui, &mut self.canvas, node_id);
                    }
                });
        }
    }
}

pub struct TopologyCanvas {
    nodes: HashMap<NodeId, TopologyNode>,
    edges: Vec<TopologyEdge>,
    next_node_id: usize,
    zoom: f32,
    pan: egui::Vec2,
    dragging_node: Option<NodeId>,
    connecting_from: Option<NodeId>,
}

pub type NodeId = usize;

#[derive(Debug, Clone)]
pub struct TopologyNode {
    pub id: NodeId,
    pub node_type: NodeType,
    pub position: egui::Pos2,
    pub data: NodeData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeType {
    Environment,
    Site,
}

#[derive(Debug, Clone)]
pub enum NodeData {
    Environment(RemoteSyncEnv),
    Site(RemoteSyncSite),
}

#[derive(Debug, Clone)]
pub struct TopologyEdge {
    pub source: NodeId,
    pub target: NodeId,
}

impl TopologyCanvas {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            next_node_id: 0,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            dragging_node: None,
            connecting_from: None,
        }
    }
    
    pub fn render(&mut self, ui: &mut egui::Ui, mode: CanvasMode) {
        let (response, painter) = ui.allocate_painter(
            ui.available_size(),
            egui::Sense::click_and_drag(),
        );
        
        let to_screen = |pos: egui::Pos2| -> egui::Pos2 {
            response.rect.min + (pos.to_vec2() * self.zoom + self.pan)
        };
        
        // 绘制网格背景
        self.draw_grid(&painter, &response.rect);
        
        // 绘制连线
        for edge in &self.edges {
            if let (Some(source), Some(target)) = (self.nodes.get(&edge.source), self.nodes.get(&edge.target)) {
                let start = to_screen(source.position);
                let end = to_screen(target.position);
                
                painter.arrow(
                    start,
                    end - start,
                    egui::Stroke::new(2.0, egui::Color32::GRAY),
                );
            }
        }
        
        // 绘制节点
        for (node_id, node) in &self.nodes {
            let screen_pos = to_screen(node.position);
            
            match node.node_type {
                NodeType::Environment => {
                    self.draw_environment_node(&painter, screen_pos, node);
                }
                NodeType::Site => {
                    self.draw_site_node(&painter, screen_pos, node);
                }
            }
        }
        
        // 处理交互
        if response.clicked() {
            let click_pos = response.interact_pointer_pos().unwrap();
            let canvas_pos = (click_pos - response.rect.min - self.pan) / self.zoom;
            
            match mode {
                CanvasMode::AddEnvironment => {
                    self.add_environment_node(canvas_pos.to_pos2());
                }
                CanvasMode::AddSite => {
                    self.add_site_node(canvas_pos.to_pos2());
                }
                CanvasMode::Select => {
                    // 选择节点
                }
                CanvasMode::Connect => {
                    // 连接节点
                }
            }
        }
        
        // 处理拖拽
        if response.dragged() {
            if let Some(node_id) = self.dragging_node {
                let delta = response.drag_delta() / self.zoom;
                if let Some(node) = self.nodes.get_mut(&node_id) {
                    node.position += delta;
                }
            } else {
                // 平移画布
                self.pan += response.drag_delta();
            }
        }
        
        // 处理缩放
        if let Some(hover_pos) = response.hover_pos() {
            let scroll_delta = ui.input(|i| i.scroll_delta.y);
            if scroll_delta != 0.0 {
                let zoom_delta = 1.0 + scroll_delta * 0.001;
                self.zoom = (self.zoom * zoom_delta).clamp(0.1, 5.0);
            }
        }
    }
    
    fn draw_grid(&self, painter: &egui::Painter, rect: &egui::Rect) {
        let grid_size = 50.0 * self.zoom;
        let color = egui::Color32::from_gray(230);
        
        // 绘制垂直线
        let mut x = rect.min.x + (self.pan.x % grid_size);
        while x < rect.max.x {
            painter.line_segment(
                [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
                egui::Stroke::new(1.0, color),
            );
            x += grid_size;
        }
        
        // 绘制水平线
        let mut y = rect.min.y + (self.pan.y % grid_size);
        while y < rect.max.y {
            painter.line_segment(
                [egui::pos2(rect.min.x, y), egui::pos2(rect.max.x, y)],
                egui::Stroke::new(1.0, color),
            );
            y += grid_size;
        }
    }
    
    fn draw_environment_node(&self, painter: &egui::Painter, pos: egui::Pos2, node: &TopologyNode) {
        let size = egui::vec2(150.0, 80.0) * self.zoom;
        let rect = egui::Rect::from_center_size(pos, size);
        
        // 绘制矩形
        painter.rect(
            rect,
            5.0,
            egui::Color32::from_rgb(200, 220, 255),
            egui::Stroke::new(2.0, egui::Color32::BLUE),
        );
        
        // 绘制文本
        if let NodeData::Environment(env) = &node.data {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &env.name,
                egui::FontId::proportional(14.0 * self.zoom),
                egui::Color32::BLACK,
            );
        }
    }
    
    fn draw_site_node(&self, painter: &egui::Painter, pos: egui::Pos2, node: &TopologyNode) {
        let radius = 40.0 * self.zoom;
        
        // 绘制圆形
        painter.circle(
            pos,
            radius,
            egui::Color32::from_rgb(200, 255, 200),
            egui::Stroke::new(2.0, egui::Color32::GREEN),
        );
        
        // 绘制文本
        if let NodeData::Site(site) = &node.data {
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                &site.name,
                egui::FontId::proportional(14.0 * self.zoom),
                egui::Color32::BLACK,
            );
        }
    }
    
    pub fn add_environment_node(&mut self, position: egui::Pos2) {
        let node_id = self.next_node_id;
        self.next_node_id += 1;
        
        let env = RemoteSyncEnv {
            id: format!("env_{}", node_id),
            name: format!("环境 {}", node_id),
            mqtt_host: None,
            mqtt_port: None,
            file_server_host: None,
            location: String::new(),
            location_dbs: None,
            reconnect_initial_ms: Some(1000),
            reconnect_max_ms: Some(30000),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        
        self.nodes.insert(node_id, TopologyNode {
            id: node_id,
            node_type: NodeType::Environment,
            position,
            data: NodeData::Environment(env),
        });
    }
    
    pub fn add_site_node(&mut self, position: egui::Pos2) {
        let node_id = self.next_node_id;
        self.next_node_id += 1;
        
        let site = RemoteSyncSite {
            id: format!("site_{}", node_id),
            env_id: String::new(),
            name: format!("站点 {}", node_id),
            location: String::new(),
            http_host: None,
            dbnums: None,
            notes: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        
        self.nodes.insert(node_id, TopologyNode {
            id: node_id,
            node_type: NodeType::Site,
            position,
            data: NodeData::Site(site),
        });
    }
    
    pub fn auto_layout(&mut self) {
        // 简单的层次布局算法
        let mut env_nodes = Vec::new();
        let mut site_nodes = Vec::new();
        
        for (id, node) in &self.nodes {
            match node.node_type {
                NodeType::Environment => env_nodes.push(*id),
                NodeType::Site => site_nodes.push(*id),
            }
        }
        
        // 环境节点排列在上层
        let env_y = 100.0;
        let env_spacing = 200.0;
        for (i, node_id) in env_nodes.iter().enumerate() {
            if let Some(node) = self.nodes.get_mut(node_id) {
                node.position = egui::pos2(100.0 + i as f32 * env_spacing, env_y);
            }
        }
        
        // 站点节点排列在下层
        let site_y = 300.0;
        let site_spacing = 150.0;
        for (i, node_id) in site_nodes.iter().enumerate() {
            if let Some(node) = self.nodes.get_mut(node_id) {
                node.position = egui::pos2(100.0 + i as f32 * site_spacing, site_y);
            }
        }
    }
    
    pub fn to_topology_data(&self) -> TopologyData {
        let mut environments = Vec::new();
        let mut sites = Vec::new();
        let mut connections = Vec::new();
        
        for node in self.nodes.values() {
            match &node.data {
                NodeData::Environment(env) => environments.push(env.clone()),
                NodeData::Site(site) => sites.push(site.clone()),
            }
        }
        
        for edge in &self.edges {
            if let (Some(source), Some(target)) = (self.nodes.get(&edge.source), self.nodes.get(&edge.target)) {
                if let (NodeData::Environment(env), NodeData::Site(site)) = (&source.data, &target.data) {
                    connections.push(TopologyConnection {
                        env_id: env.id.clone(),
                        site_id: site.id.clone(),
                    });
                }
            }
        }
        
        TopologyData {
            environments,
            sites,
            connections,
        }
    }
    
    pub fn load_from_topology(&mut self, topology: &TopologyData) {
        self.nodes.clear();
        self.edges.clear();
        self.next_node_id = 0;
        
        // 加载环境节点
        for env in &topology.environments {
            let node_id = self.next_node_id;
            self.next_node_id += 1;
            
            self.nodes.insert(node_id, TopologyNode {
                id: node_id,
                node_type: NodeType::Environment,
                position: egui::pos2(100.0, 100.0), // 临时位置
                data: NodeData::Environment(env.clone()),
            });
        }
        
        // 加载站点节点
        for site in &topology.sites {
            let node_id = self.next_node_id;
            self.next_node_id += 1;
            
            self.nodes.insert(node_id, TopologyNode {
                id: node_id,
                node_type: NodeType::Site,
                position: egui::pos2(100.0, 300.0), // 临时位置
                data: NodeData::Site(site.clone()),
            });
        }
        
        // 应用自动布局
        self.auto_layout();
    }
    
    pub fn get_selected_node(&self) -> Option<NodeId> {
        // 返回当前选中的节点 ID
        None
    }
}
```


### 6. 监控面板页面 (MonitorDashboardPage)

```rust
pub struct MonitorDashboardPage {
    sync_status: Option<SyncStatus>,
    tasks: Vec<SyncTask>,
    selected_task: Option<String>,
    last_refresh: std::time::Instant,
    auto_refresh: bool,
}

impl MonitorDashboardPage {
    pub fn render(&mut self, ui: &mut egui::Ui, state: &mut AppState, api_client: &ApiClient) {
        ui.heading("实时监控");
        
        ui.horizontal(|ui| {
            if ui.button("🔄 刷新").clicked() {
                self.refresh_status(api_client);
            }
            
            ui.checkbox(&mut self.auto_refresh, "自动刷新 (5秒)");
            
            ui.label(format!(
                "上次刷新: {} 秒前",
                self.last_refresh.elapsed().as_secs()
            ));
        });
        
        ui.separator();
        
        // 状态卡片
        ui.horizontal(|ui| {
            if let Some(status) = &self.sync_status {
                self.render_status_card(ui, "运行状态", if status.running {
                    if status.paused { "⏸️ 暂停中" } else { "🟢 运行中" }
                } else {
                    "⚪ 已停止"
                });
                
                self.render_status_card(ui, "MQTT 连接", if status.mqtt_connected {
                    "🟢 已连接"
                } else {
                    "🔴 断开"
                });
                
                self.render_status_card(ui, "队列大小", &status.queue_size.to_string());
                self.render_status_card(ui, "活跃任务", &status.active_tasks.to_string());
            }
        });
        
        ui.separator();
        
        // 任务列表
        ui.heading("同步任务");
        
        use egui_extras::{TableBuilder, Column};
        
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::auto().at_least(150.0)) // 文件名
            .column(Column::auto().at_least(100.0)) // 源环境
            .column(Column::auto().at_least(100.0)) // 目标站点
            .column(Column::auto().at_least(80.0))  // 状态
            .column(Column::remainder())            // 进度
            .header(20.0, |mut header| {
                header.col(|ui| { ui.strong("文件名"); });
                header.col(|ui| { ui.strong("源环境"); });
                header.col(|ui| { ui.strong("目标站点"); });
                header.col(|ui| { ui.strong("状态"); });
                header.col(|ui| { ui.strong("进度"); });
            })
            .body(|mut body| {
                for task in &self.tasks {
                    body.row(30.0, |mut row| {
                        row.col(|ui| { ui.label(&task.file_name); });
                        row.col(|ui| { ui.label(&task.source_env); });
                        row.col(|ui| { ui.label(&task.target_site); });
                        row.col(|ui| {
                            let (color, text) = match task.status.as_str() {
                                "pending" => (egui::Color32::GRAY, "⏸️ 等待中"),
                                "running" => (egui::Color32::BLUE, "🟢 运行中"),
                                "completed" => (egui::Color32::GREEN, "✅ 完成"),
                                "failed" => (egui::Color32::RED, "❌ 失败"),
                                _ => (egui::Color32::GRAY, "未知"),
                            };
                            ui.colored_label(color, text);
                        });
                        row.col(|ui| {
                            let progress = task.progress as f32 / 100.0;
                            ui.add(egui::ProgressBar::new(progress).text(format!("{}%", task.progress)));
                        });
                    });
                }
            });
        
        // 自动刷新
        if self.auto_refresh && self.last_refresh.elapsed().as_secs() >= 5 {
            self.refresh_status(api_client);
        }
    }
    
    fn render_status_card(&self, ui: &mut egui::Ui, title: &str, value: &str) {
        egui::Frame::group(ui.style())
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(title);
                    ui.heading(value);
                });
            });
    }
    
    fn refresh_status(&mut self, api_client: &ApiClient) {
        let api_client = api_client.clone();
        let sender = self.status_sender.clone();
        
        tokio::spawn(async move {
            if let Ok(status) = api_client.get_sync_status().await {
                let _ = sender.send(status);
            }
        });
        
        self.last_refresh = std::time::Instant::now();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTask {
    pub id: String,
    pub file_name: String,
    pub source_env: String,
    pub target_site: String,
    pub status: String,
    pub progress: u8,
}
```

### 7. 日志查询页面 (LogQueryPage)

```rust
pub struct LogQueryPage {
    filters: LogFilters,
    logs: Vec<SyncLog>,
    selected_log: Option<String>,
    show_log_detail: bool,
    page: usize,
    page_size: usize,
    total: usize,
}

impl LogQueryPage {
    pub fn render(&mut self, ui: &mut egui::Ui, state: &mut AppState, api_client: &ApiClient) {
        ui.heading("日志查询");
        
        // 筛选表单
        egui::Grid::new("log_filters")
            .num_columns(4)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("环境");
                egui::ComboBox::from_id_source("env_filter")
                    .selected_text(self.filters.env_id.as_deref().unwrap_or("全部"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.filters.env_id, None, "全部");
                        for env in &state.environments {
                            ui.selectable_value(&mut self.filters.env_id, Some(env.id.clone()), &env.name);
                        }
                    });
                
                ui.label("站点");
                egui::ComboBox::from_id_source("site_filter")
                    .selected_text(self.filters.site_id.as_deref().unwrap_or("全部"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.filters.site_id, None, "全部");
                        for site in &state.sites {
                            ui.selectable_value(&mut self.filters.site_id, Some(site.id.clone()), &site.name);
                        }
                    });
                
                ui.label("状态");
                egui::ComboBox::from_id_source("status_filter")
                    .selected_text(self.filters.status.as_deref().unwrap_or("全部"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.filters.status, None, "全部");
                        ui.selectable_value(&mut self.filters.status, Some("pending".to_string()), "待处理");
                        ui.selectable_value(&mut self.filters.status, Some("running".to_string()), "运行中");
                        ui.selectable_value(&mut self.filters.status, Some("completed".to_string()), "完成");
                        ui.selectable_value(&mut self.filters.status, Some("failed".to_string()), "失败");
                    });
                
                if ui.button("🔍 查询").clicked() {
                    self.query_logs(api_client);
                }
                
                ui.end_row();
            });
        
        ui.separator();
        
        // 日志列表
        use egui_extras::{TableBuilder, Column};
        
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::auto().at_least(150.0)) // 时间
            .column(Column::auto().at_least(150.0)) // 文件路径
            .column(Column::auto().at_least(100.0)) // 源环境
            .column(Column::auto().at_least(100.0)) // 目标站点
            .column(Column::auto().at_least(80.0))  // 状态
            .column(Column::remainder())            // 操作
            .header(20.0, |mut header| {
                header.col(|ui| { ui.strong("时间"); });
                header.col(|ui| { ui.strong("文件路径"); });
                header.col(|ui| { ui.strong("源环境"); });
                header.col(|ui| { ui.strong("目标站点"); });
                header.col(|ui| { ui.strong("状态"); });
                header.col(|ui| { ui.strong("操作"); });
            })
            .body(|mut body| {
                for log in &self.logs {
                    body.row(30.0, |mut row| {
                        row.col(|ui| { ui.label(&log.created_at); });
                        row.col(|ui| { ui.label(&log.file_path); });
                        row.col(|ui| { ui.label(&log.source_env); });
                        row.col(|ui| { ui.label(&log.target_site); });
                        row.col(|ui| {
                            let (color, text) = match log.status.as_str() {
                                "completed" => (egui::Color32::GREEN, "✅ 完成"),
                                "failed" => (egui::Color32::RED, "❌ 失败"),
                                _ => (egui::Color32::GRAY, "⏸️ 其他"),
                            };
                            ui.colored_label(color, text);
                        });
                        row.col(|ui| {
                            if ui.small_button("详情").clicked() {
                                self.selected_log = Some(log.id.clone());
                                self.show_log_detail = true;
                            }
                        });
                    });
                }
            });
        
        // 分页控件
        ui.horizontal(|ui| {
            if ui.button("◀ 上一页").clicked() && self.page > 0 {
                self.page -= 1;
                self.query_logs(api_client);
            }
            
            ui.label(format!("第 {} 页 / 共 {} 条", self.page + 1, self.total));
            
            if ui.button("下一页 ▶").clicked() && (self.page + 1) * self.page_size < self.total {
                self.page += 1;
                self.query_logs(api_client);
            }
            
            if ui.button("📤 导出 CSV").clicked() {
                self.export_csv();
            }
        });
        
        // 日志详情对话框
        if self.show_log_detail {
            egui::Window::new("日志详情")
                .collapsible(false)
                .resizable(true)
                .show(ui.ctx(), |ui| {
                    if let Some(log_id) = &self.selected_log {
                        if let Some(log) = self.logs.iter().find(|l| &l.id == log_id) {
                            egui::Grid::new("log_detail")
                                .num_columns(2)
                                .spacing([10.0, 10.0])
                                .show(ui, |ui| {
                                    ui.label("任务 ID:");
                                    ui.label(&log.task_id);
                                    ui.end_row();
                                    
                                    ui.label("文件路径:");
                                    ui.label(&log.file_path);
                                    ui.end_row();
                                    
                                    ui.label("文件大小:");
                                    ui.label(format!("{} bytes", log.file_size));
                                    ui.end_row();
                                    
                                    ui.label("记录数:");
                                    ui.label(format!("{}", log.record_count));
                                    ui.end_row();
                                    
                                    ui.label("开始时间:");
                                    ui.label(&log.started_at);
                                    ui.end_row();
                                    
                                    if let Some(completed_at) = &log.completed_at {
                                        ui.label("完成时间:");
                                        ui.label(completed_at);
                                        ui.end_row();
                                    }
                                    
                                    if let Some(error) = &log.error_message {
                                        ui.label("错误信息:");
                                        ui.colored_label(egui::Color32::RED, error);
                                        ui.end_row();
                                    }
                                });
                        }
                    }
                    
                    if ui.button("关闭").clicked() {
                        self.show_log_detail = false;
                    }
                });
        }
    }
    
    fn query_logs(&mut self, api_client: &ApiClient) {
        let filters = self.filters.clone();
        let api_client = api_client.clone();
        
        tokio::spawn(async move {
            if let Ok(logs) = api_client.query_logs(&filters).await {
                // 更新日志列表
            }
        });
    }
    
    fn export_csv(&self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .save_file()
        {
            let mut writer = csv::Writer::from_path(path).unwrap();
            
            writer.write_record(&["时间", "文件路径", "源环境", "目标站点", "状态"]).unwrap();
            
            for log in &self.logs {
                writer.write_record(&[
                    &log.created_at,
                    &log.file_path,
                    &log.source_env,
                    &log.target_site,
                    &log.status,
                ]).unwrap();
            }
            
            writer.flush().unwrap();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncLog {
    pub id: String,
    pub task_id: String,
    pub file_path: String,
    pub file_size: u64,
    pub record_count: u32,
    pub source_env: String,
    pub target_site: String,
    pub status: String,
    pub error_message: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub created_at: String,
}
```


### 8. Web Server 管理页面 (WebServerPage)

```rust
pub struct WebServerPage {
    config: WebServerConfig,
    status: ServerStatus,
    logs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebServerConfig {
    pub host: String,
    pub port: u16,
    pub db_path: String,
    pub static_dir: String,
}

impl WebServerPage {
    pub fn render(&mut self, ui: &mut egui::Ui) {
        ui.heading("Web Server 管理");
        
        // 状态显示
        ui.horizontal(|ui| {
            ui.label("服务器状态:");
            match &self.status {
                ServerStatus::Stopped => {
                    ui.colored_label(egui::Color32::GRAY, "⚪ 已停止");
                }
                ServerStatus::Starting => {
                    ui.colored_label(egui::Color32::YELLOW, "🟡 启动中...");
                }
                ServerStatus::Running { address } => {
                    ui.colored_label(egui::Color32::GREEN, format!("🟢 运行中 ({})", address));
                }
                ServerStatus::Stopping => {
                    ui.colored_label(egui::Color32::YELLOW, "🟡 停止中...");
                }
                ServerStatus::Error(msg) => {
                    ui.colored_label(egui::Color32::RED, format!("🔴 错误: {}", msg));
                }
            }
        });
        
        ui.separator();
        
        // 配置表单
        egui::Grid::new("server_config")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("监听地址:");
                ui.text_edit_singleline(&mut self.config.host);
                ui.end_row();
                
                ui.label("监听端口:");
                ui.add(egui::DragValue::new(&mut self.config.port).clamp_range(1..=65535));
                ui.end_row();
                
                ui.label("数据库路径:");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.config.db_path);
                    if ui.button("📁").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("SQLite", &["db", "sqlite"])
                            .pick_file()
                        {
                            self.config.db_path = path.to_string_lossy().to_string();
                        }
                    }
                });
                ui.end_row();
                
                ui.label("静态文件目录:");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.config.static_dir);
                    if ui.button("📁").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.config.static_dir = path.to_string_lossy().to_string();
                        }
                    }
                });
                ui.end_row();
            });
        
        ui.separator();
        
        // 操作按钮
        ui.horizontal(|ui| {
            match &self.status {
                ServerStatus::Stopped | ServerStatus::Error(_) => {
                    if ui.button("▶️ 启动服务器").clicked() {
                        self.start_server();
                    }
                }
                ServerStatus::Running { .. } => {
                    if ui.button("⏹️ 停止服务器").clicked() {
                        self.stop_server();
                    }
                }
                _ => {
                    ui.add_enabled(false, egui::Button::new("处理中..."));
                }
            }
            
            if ui.button("💾 保存配置").clicked() {
                self.save_config();
            }
        });
        
        ui.separator();
        
        // 日志输出
        ui.heading("服务器日志");
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                for log in &self.logs {
                    ui.label(log);
                }
            });
    }
    
    fn start_server(&mut self) {
        self.status = ServerStatus::Starting;
        
        let config = self.config.clone();
        
        tokio::spawn(async move {
            // 启动 Axum Web Server
            // 这里需要与现有的 web_server 模块集成
            match start_web_server(config).await {
                Ok(address) => {
                    // 更新状态为 Running
                }
                Err(e) => {
                    // 更新状态为 Error
                }
            }
        });
    }
    
    fn stop_server(&mut self) {
        self.status = ServerStatus::Stopping;
        
        tokio::spawn(async move {
            // 停止 Web Server
            stop_web_server().await;
        });
    }
    
    fn save_config(&self) {
        // 保存配置到 DbOption.toml
        let toml_content = toml::to_string(&self.config).unwrap();
        std::fs::write("DbOption.toml", toml_content).unwrap();
    }
}

// Web Server 启动函数（需要与现有代码集成）
async fn start_web_server(config: WebServerConfig) -> anyhow::Result<String> {
    // 这里调用现有的 web_server 模块
    // 返回监听地址
    Ok(format!("http://{}:{}", config.host, config.port))
}

async fn stop_web_server() {
    // 停止 Web Server
}
```

## Data Models

### RemoteSyncEnv (复用现有定义)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSyncEnv {
    pub id: String,
    pub name: String,
    pub mqtt_host: Option<String>,
    pub mqtt_port: Option<u16>,
    pub file_server_host: Option<String>,
    pub location: String,
    pub location_dbs: Option<String>,
    pub reconnect_initial_ms: Option<u64>,
    pub reconnect_max_ms: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}
```

### RemoteSyncSite (复用现有定义)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSyncSite {
    pub id: String,
    pub env_id: String,
    pub name: String,
    pub location: String,
    pub http_host: Option<String>,
    pub dbnums: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
```

### TopologyData

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyData {
    pub environments: Vec<RemoteSyncEnv>,
    pub sites: Vec<RemoteSyncSite>,
    pub connections: Vec<TopologyConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyConnection {
    pub env_id: String,
    pub site_id: String,
}
```

## Error Handling

### 错误类型定义

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("API 调用失败: {0}")]
    ApiError(String),
    
    #[error("网络连接失败: {0}")]
    NetworkError(#[from] reqwest::Error),
    
    #[error("数据库错误: {0}")]
    DatabaseError(#[from] rusqlite::Error),
    
    #[error("配置文件错误: {0}")]
    ConfigError(String),
    
    #[error("验证失败: {0}")]
    ValidationError(String),
}
```

### Toast 提示管理器

```rust
pub struct ToastManager {
    toasts: Vec<Toast>,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: usize,
    pub message: String,
    pub toast_type: ToastType,
    pub created_at: std::time::Instant,
    pub duration: std::time::Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToastType {
    Success,
    Error,
    Warning,
    Info,
}

impl ToastManager {
    pub fn new() -> Self {
        Self {
            toasts: Vec::new(),
        }
    }
    
    pub fn success(&mut self, message: impl Into<String>) {
        self.add_toast(message.into(), ToastType::Success);
    }
    
    pub fn error(&mut self, message: impl Into<String>) {
        self.add_toast(message.into(), ToastType::Error);
    }
    
    pub fn warning(&mut self, message: impl Into<String>) {
        self.add_toast(message.into(), ToastType::Warning);
    }
    
    pub fn info(&mut self, message: impl Into<String>) {
        self.add_toast(message.into(), ToastType::Info);
    }
    
    fn add_toast(&mut self, message: String, toast_type: ToastType) {
        let id = self.toasts.len();
        self.toasts.push(Toast {
            id,
            message,
            toast_type,
            created_at: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(3),
        });
    }
    
    pub fn render(&mut self, ctx: &egui::Context) {
        let mut to_remove = Vec::new();
        
        egui::Area::new("toasts")
            .fixed_pos(egui::pos2(ctx.screen_rect().width() - 320.0, 10.0))
            .show(ctx, |ui| {
                for toast in &self.toasts {
                    if toast.created_at.elapsed() > toast.duration {
                        to_remove.push(toast.id);
                        continue;
                    }
                    
                    let (bg_color, icon) = match toast.toast_type {
                        ToastType::Success => (egui::Color32::from_rgb(200, 255, 200), "✅"),
                        ToastType::Error => (egui::Color32::from_rgb(255, 200, 200), "❌"),
                        ToastType::Warning => (egui::Color32::from_rgb(255, 255, 200), "⚠️"),
                        ToastType::Info => (egui::Color32::from_rgb(200, 220, 255), "ℹ️"),
                    };
                    
                    egui::Frame::none()
                        .fill(bg_color)
                        .rounding(5.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(icon);
                                ui.label(&toast.message);
                            });
                        });
                    
                    ui.add_space(5.0);
                }
            });
        
        self.toasts.retain(|t| !to_remove.contains(&t.id));
    }
}
```

## Testing Strategy

### 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_environment_form_validation() {
        let mut form = EnvironmentForm::new();
        assert!(!form.validate()); // 名称为空应该失败
        
        form.name = "测试环境".to_string();
        assert!(form.validate());
        
        form.mqtt_port = "invalid".to_string();
        assert!(!form.validate()); // 端口格式错误应该失败
    }
    
    #[test]
    fn test_topology_canvas_add_node() {
        let mut canvas = TopologyCanvas::new();
        canvas.add_environment_node(egui::pos2(100.0, 100.0));
        
        assert_eq!(canvas.nodes.len(), 1);
        assert_eq!(canvas.next_node_id, 1);
    }
    
    #[test]
    fn test_topology_data_serialization() {
        let topology = TopologyData {
            environments: vec![],
            sites: vec![],
            connections: vec![],
        };
        
        let json = serde_json::to_string(&topology).unwrap();
        let deserialized: TopologyData = serde_json::from_str(&json).unwrap();
        
        assert_eq!(topology.environments.len(), deserialized.environments.len());
    }
}
```

### 集成测试

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_api_client_get_environments() {
        let client = ApiClient::new("http://localhost:3000");
        
        // 需要启动测试服务器
        let result = client.get_environments().await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_create_and_delete_environment() {
        let client = ApiClient::new("http://localhost:3000");
        
        let env = RemoteSyncEnv {
            id: "test-env".to_string(),
            name: "测试环境".to_string(),
            // ...
        };
        
        let env_id = client.create_environment(&env).await.unwrap();
        assert!(!env_id.is_empty());
        
        client.delete_environment(&env_id).await.unwrap();
    }
}
```

## Performance Optimization

### 异步操作

所有 API 调用都使用 tokio 异步运行时，避免阻塞 UI 线程：

```rust
// 在 UI 线程中发起异步请求
let api_client = self.api_client.clone();
tokio::spawn(async move {
    match api_client.get_environments().await {
        Ok(envs) => {
            // 通过 channel 发送结果到 UI 线程
        }
        Err(e) => {
            // 处理错误
        }
    }
});
```

### 渲染优化

- 使用 `egui::Context::request_repaint_after()` 控制刷新频率
- 仅在数据变化时重新渲染
- 使用 `egui::ScrollArea` 和虚拟滚动优化大列表

### 内存优化

- 使用 `Arc` 和 `Mutex` 共享状态
- 及时清理不再使用的数据
- 限制日志和任务列表的最大长度

## Deployment

### 构建配置

```toml
# Cargo.toml
[package]
name = "egui-remote-sync"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "egui_remote_sync"
path = "src/bin/egui_remote_sync.rs"

[dependencies]
egui = "0.33"
eframe = { version = "0.33", features = ["glow"] }
egui_extras = { version = "0.33", features = ["all"] }
egui_plot = "0.33"
reqwest = { version = "0.11", features = ["json"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = "0.29"
chrono = "0.4"
anyhow = "1"
thiserror = "1"
tracing = "0.1"
rfd = "0.12"
csv = "1"
toml = "0.8"
uuid = { version = "1", features = ["v4"] }

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

### 构建命令

```bash
# 开发构建
cargo build --bin egui_remote_sync

# 发布构建
cargo build --bin egui_remote_sync --release

# 跨平台构建
cargo build --bin egui_remote_sync --release --target x86_64-pc-windows-gnu
cargo build --bin egui_remote_sync --release --target x86_64-apple-darwin
cargo build --bin egui_remote_sync --release --target x86_64-unknown-linux-gnu
```

### 打包分发

- Windows: 生成 `.exe` 可执行文件
- macOS: 打包为 `.app` 应用包
- Linux: 生成 ELF 可执行文件或 AppImage

## Integration with Existing Backend

### API 端点映射

egui 应用将调用现有的 REST API：

- `GET /api/remote-sync/envs` - 获取环境列表
- `POST /api/remote-sync/envs` - 创建环境
- `PUT /api/remote-sync/envs/{id}` - 更新环境
- `DELETE /api/remote-sync/envs/{id}` - 删除环境
- `POST /api/remote-sync/envs/{id}/activate` - 激活环境
- `GET /api/remote-sync/sites` - 获取站点列表
- `POST /api/remote-sync/sites` - 创建站点
- `POST /api/remote-sync/sites/{id}/test` - 测试站点连接
- `GET /api/remote-sync/sites/{id}/metadata` - 获取站点元数据
- `POST /api/sync/start` - 启动同步服务
- `POST /api/sync/stop` - 停止同步服务
- `POST /api/sync/pause` - 暂停同步服务
- `POST /api/sync/resume` - 恢复同步服务
- `GET /api/sync/status` - 获取同步状态
- `POST /api/sync/queue/clear` - 清空队列
- `GET /api/remote-sync/logs` - 查询日志
- `POST /api/topology/save` - 保存拓扑配置
- `GET /api/topology/load` - 加载拓扑配置

### 数据库共享

egui 应用和 Web Server 共享同一个 SQLite 数据库（`deployment_sites.sqlite`），确保数据一致性。

### 配置文件共享

egui 应用读写 `DbOption.toml` 配置文件，与现有系统保持一致。
