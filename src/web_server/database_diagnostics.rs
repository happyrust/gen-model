use aios_core::{get_db_option, SUL_DB};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

/// 数据库诊断结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseDiagnosticResult {
    pub overall_status: DiagnosticStatus,
    pub checks: Vec<DiagnosticCheck>,
    pub recommendations: Vec<String>,
    pub connection_info: ConnectionInfo,
    pub timestamp: SystemTime,
}

/// 诊断状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiagnosticStatus {
    Healthy,
    Warning,
    Critical,
}

/// 单项诊断检查
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    pub name: String,
    pub status: DiagnosticStatus,
    pub message: String,
    pub details: Option<String>,
    pub duration_ms: Option<u64>,
}

/// 连接信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub project_name: String,
    pub project_code: String,
    pub connection_string: String,
}

impl DatabaseDiagnosticResult {
    pub fn new() -> Self {
        let db_option = get_db_option();
        Self {
            overall_status: DiagnosticStatus::Critical,
            checks: Vec::new(),
            recommendations: Vec::new(),
            connection_info: ConnectionInfo {
                host: db_option.v_ip.clone(),
                port: db_option.v_port,
                user: db_option.v_user.clone(),
                project_name: db_option.project_name.clone(),
                project_code: db_option.project_code.to_string(),
                connection_string: db_option.get_version_db_conn_str(),
            },
            timestamp: SystemTime::now(),
        }
    }

    pub fn add_check(&mut self, check: DiagnosticCheck) {
        self.checks.push(check);
        self.update_overall_status();
    }

    pub fn add_recommendation(&mut self, recommendation: String) {
        self.recommendations.push(recommendation);
    }

    fn update_overall_status(&mut self) {
        let has_critical = self
            .checks
            .iter()
            .any(|c| matches!(c.status, DiagnosticStatus::Critical));
        let has_warning = self
            .checks
            .iter()
            .any(|c| matches!(c.status, DiagnosticStatus::Warning));

        self.overall_status = if has_critical {
            DiagnosticStatus::Critical
        } else if has_warning {
            DiagnosticStatus::Warning
        } else {
            DiagnosticStatus::Healthy
        };
    }
}

/// 执行完整的数据库诊断
pub async fn run_database_diagnostics() -> DatabaseDiagnosticResult {
    let mut result = DatabaseDiagnosticResult::new();

    // 1. 检查配置有效性
    check_configuration(&mut result).await;

    // 2. 检查 SurrealDB CLI
    check_surreal_cli(&mut result).await;

    // 3. 检查端口监听
    check_port_listening(&mut result).await;

    // 4. 检查 TCP 连接
    check_tcp_connection(&mut result).await;

    // 5. 检查数据库功能
    check_database_functionality(&mut result).await;

    // 6. 检查进程状态
    check_process_status(&mut result).await;

    // 7. 生成建议
    generate_recommendations(&mut result);

    result
}

async fn check_configuration(result: &mut DatabaseDiagnosticResult) {
    let start_time = std::time::Instant::now();
    let db_option = get_db_option();

    let mut issues = Vec::new();

    if db_option.v_ip.is_empty() {
        issues.push("数据库IP为空");
    }

    if db_option.v_port == 0 || db_option.v_port > 65535 {
        issues.push("数据库端口无效");
    }

    if db_option.v_user.is_empty() {
        issues.push("数据库用户名为空");
    }

    if db_option.project_name.is_empty() {
        issues.push("项目名称为空");
    }

    let status = if issues.is_empty() {
        DiagnosticStatus::Healthy
    } else {
        DiagnosticStatus::Critical
    };

    let message = if issues.is_empty() {
        "配置验证通过".to_string()
    } else {
        format!("配置问题: {}", issues.join(", "))
    };

    result.add_check(DiagnosticCheck {
        name: "配置验证".to_string(),
        status,
        message,
        details: Some(format!(
            "连接字符串: {}",
            db_option.get_version_db_conn_str()
        )),
        duration_ms: Some(start_time.elapsed().as_millis() as u64),
    });
}

async fn check_surreal_cli(result: &mut DatabaseDiagnosticResult) {
    let start_time = std::time::Instant::now();

    let cli_exists = match TokioCommand::new("which").arg("surreal").output().await {
        Ok(output) => output.status.success(),
        Err(_) => false,
    };

    let (status, message) = if cli_exists {
        // 尝试获取版本信息
        let version_info = match timeout(
            Duration::from_secs(5),
            TokioCommand::new("surreal").arg("version").output(),
        )
        .await
        {
            Ok(Ok(output)) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
            _ => "版本信息获取失败".to_string(),
        };

        (
            DiagnosticStatus::Healthy,
            format!("SurrealDB CLI 可用: {}", version_info),
        )
    } else {
        (
            DiagnosticStatus::Critical,
            "SurrealDB CLI 未安装或不在 PATH 中".to_string(),
        )
    };

    result.add_check(DiagnosticCheck {
        name: "SurrealDB CLI".to_string(),
        status,
        message,
        details: None,
        duration_ms: Some(start_time.elapsed().as_millis() as u64),
    });

    if !cli_exists {
        result.add_recommendation(
            "安装 SurrealDB CLI: curl -sSf https://install.surrealdb.com | sh".to_string(),
        );
    }
}

async fn check_port_listening(result: &mut DatabaseDiagnosticResult) {
    let start_time = std::time::Instant::now();
    let db_option = get_db_option();
    let addr = format!("{}:{}", db_option.v_ip, db_option.v_port);

    let listening = super::handlers::is_addr_listening(&addr);

    let (status, message) = if listening {
        (DiagnosticStatus::Healthy, format!("端口 {} 正在监听", addr))
    } else {
        (DiagnosticStatus::Critical, format!("端口 {} 未监听", addr))
    };

    result.add_check(DiagnosticCheck {
        name: "端口监听".to_string(),
        status,
        message,
        details: Some(format!("检查地址: {}", addr)),
        duration_ms: Some(start_time.elapsed().as_millis() as u64),
    });

    if !listening {
        result.add_recommendation("启动 SurrealDB 服务".to_string());
    }
}

async fn check_tcp_connection(result: &mut DatabaseDiagnosticResult) {
    let start_time = std::time::Instant::now();
    let db_option = get_db_option();
    let addr = format!("{}:{}", db_option.v_ip, db_option.v_port);

    let connected = super::handlers::test_tcp_connection(&addr).await;

    let (status, message) = if connected {
        (DiagnosticStatus::Healthy, "TCP 连接成功".to_string())
    } else {
        (DiagnosticStatus::Critical, "TCP 连接失败".to_string())
    };

    result.add_check(DiagnosticCheck {
        name: "TCP 连接".to_string(),
        status,
        message,
        details: Some(format!("目标地址: {}", addr)),
        duration_ms: Some(start_time.elapsed().as_millis() as u64),
    });

    if !connected {
        result.add_recommendation("检查网络连接和防火墙设置".to_string());
    }
}

async fn check_database_functionality(result: &mut DatabaseDiagnosticResult) {
    let start_time = std::time::Instant::now();

    let (functional, error_msg) = super::handlers::test_database_functionality().await;

    let (status, message) = if functional {
        (DiagnosticStatus::Healthy, "数据库功能正常".to_string())
    } else {
        (
            DiagnosticStatus::Critical,
            format!(
                "数据库功能异常: {}",
                error_msg.as_ref().unwrap_or(&"未知错误".to_string())
            ),
        )
    };

    result.add_check(DiagnosticCheck {
        name: "数据库功能".to_string(),
        status,
        message,
        details: error_msg,
        duration_ms: Some(start_time.elapsed().as_millis() as u64),
    });

    if !functional {
        result.add_recommendation("检查数据库用户权限和密码".to_string());
        result.add_recommendation("验证命名空间和数据库名称".to_string());
    }
}

async fn check_process_status(result: &mut DatabaseDiagnosticResult) {
    let start_time = std::time::Instant::now();

    // 检查 PID 文件
    let pid_exists = std::fs::read_to_string(".surreal.pid").is_ok();

    let (status, message) = if pid_exists {
        (
            DiagnosticStatus::Healthy,
            "找到 SurrealDB 进程 PID 文件".to_string(),
        )
    } else {
        (
            DiagnosticStatus::Warning,
            "未找到 SurrealDB 进程 PID 文件".to_string(),
        )
    };

    result.add_check(DiagnosticCheck {
        name: "进程状态".to_string(),
        status,
        message,
        details: Some("PID 文件: .surreal.pid".to_string()),
        duration_ms: Some(start_time.elapsed().as_millis() as u64),
    });
}

fn generate_recommendations(result: &mut DatabaseDiagnosticResult) {
    match result.overall_status {
        DiagnosticStatus::Healthy => {
            result.add_recommendation("数据库连接正常，无需额外操作".to_string());
        }
        DiagnosticStatus::Warning => {
            result.add_recommendation("存在一些警告，建议检查相关配置".to_string());
        }
        DiagnosticStatus::Critical => {
            result.add_recommendation("存在严重问题，请按照检查结果进行修复".to_string());
            result.add_recommendation("可以尝试重启 SurrealDB 服务".to_string());
        }
    }

    // 添加通用建议
    result.add_recommendation("查看详细日志: tail -f surreal.log".to_string());
    result.add_recommendation(
        "手动测试连接: surreal sql --conn ws://localhost:8009 --user root --pass root".to_string(),
    );
}
