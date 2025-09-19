//! GRPC服务单元测试

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc_service::auth::{AuthConfig, AuthService};
    use crate::grpc_service::managers::{ProgressManager, TaskManager};
    use crate::grpc_service::types::{
        ProgressUpdate, TaskOptions, TaskPriority, TaskRequest, TaskStatus, TaskType,
    };
    use chrono::Utc;
    use std::sync::Arc;
    use tokio::time::{Duration, sleep};

    /// 测试ProgressManager
    #[tokio::test]
    async fn test_progress_manager_create_task() {
        let manager = ProgressManager::new();
        let task_id = "test_task_1".to_string();

        // 创建任务
        let receiver = manager.create_task(task_id.clone()).await.unwrap();

        // 验证任务是否存在
        assert!(manager.has_task(&task_id));

        // 验证任务进度
        let progress = manager.get_task_progress(&task_id).await;
        assert!(progress.is_some());
        assert_eq!(progress.unwrap().task_id, task_id);
    }

    #[tokio::test]
    async fn test_progress_manager_update_progress() {
        let manager = ProgressManager::new();
        let task_id = "test_task_2".to_string();

        // 创建任务
        let mut receiver = manager.create_task(task_id.clone()).await.unwrap();

        // 更新进度
        let update = ProgressUpdate {
            task_id: task_id.clone(),
            progress: 50.0,
            status: TaskStatus::Running,
            message: "Half way done".to_string(),
            timestamp: Utc::now(),
            details: None,
        };

        manager.update_progress(update.clone()).await.unwrap();

        // 验证进度更新
        let progress = manager.get_task_progress(&task_id).await.unwrap();
        assert_eq!(progress.progress, 50.0);
        assert_eq!(progress.status, TaskStatus::Running);
        assert_eq!(progress.message, "Half way done");

        // 验证广播
        let received_update = receiver.recv().await.unwrap();
        assert_eq!(received_update.task_id, task_id);
        assert_eq!(received_update.progress, 50.0);
    }

    #[tokio::test]
    async fn test_progress_manager_remove_task() {
        let manager = ProgressManager::new();
        let task_id = "test_task_3".to_string();

        // 创建任务
        manager.create_task(task_id.clone()).await.unwrap();
        assert!(manager.has_task(&task_id));

        // 移除任务
        manager.remove_task(&task_id).await.unwrap();
        assert!(!manager.has_task(&task_id));
    }

    /// 测试TaskManager
    #[tokio::test]
    async fn test_task_manager_submit_task() {
        let manager = TaskManager::new(2);

        let task_request = TaskRequest {
            id: "test_task_submit".to_string(),
            task_type: TaskType::FullSync,
            mdb_name: "test_mdb".to_string(),
            db_files: vec![1, 2, 3],
            options: TaskOptions::default(),
            priority: TaskPriority::Normal,
        };

        // 提交任务
        let task_id = manager.submit_task(task_request).await.unwrap();
        assert_eq!(task_id, "test_task_submit");

        // 验证任务状态
        let status = manager.get_task_status(&task_id).await;
        assert!(status.is_some());
        assert_eq!(status.unwrap(), TaskStatus::Running);
    }

    #[tokio::test]
    async fn test_task_manager_concurrent_limit() {
        let manager = TaskManager::new(1); // 限制为1个并发任务

        // 提交第一个任务
        let task1 = TaskRequest {
            id: "concurrent_task_1".to_string(),
            task_type: TaskType::FullSync,
            mdb_name: "test_mdb".to_string(),
            db_files: vec![1],
            options: TaskOptions::default(),
            priority: TaskPriority::Normal,
        };

        let task_id1 = manager.submit_task(task1).await.unwrap();
        assert_eq!(manager.active_task_count(), 1);

        // 提交第二个任务（应该进入队列）
        let task2 = TaskRequest {
            id: "concurrent_task_2".to_string(),
            task_type: TaskType::IncrementalSync,
            mdb_name: "test_mdb".to_string(),
            db_files: vec![2],
            options: TaskOptions::default(),
            priority: TaskPriority::Normal,
        };

        let task_id2 = manager.submit_task(task2).await.unwrap();
        assert_eq!(manager.active_task_count(), 1);
        assert_eq!(manager.queued_task_count().await, 1);

        // 停止第一个任务
        manager.stop_task(&task_id1).await.unwrap();

        // 等待一小段时间让队列中的任务启动
        sleep(Duration::from_millis(100)).await;

        // 验证第二个任务已经启动
        assert_eq!(manager.active_task_count(), 1);
        assert_eq!(manager.queued_task_count().await, 0);
    }

    #[tokio::test]
    async fn test_task_manager_stop_task() {
        let manager = TaskManager::new(2);

        let task_request = TaskRequest {
            id: "test_task_stop".to_string(),
            task_type: TaskType::ModelGeneration,
            mdb_name: "test_mdb".to_string(),
            db_files: vec![1],
            options: TaskOptions::default(),
            priority: TaskPriority::High,
        };

        // 提交任务
        let task_id = manager.submit_task(task_request).await.unwrap();
        assert_eq!(manager.active_task_count(), 1);

        // 停止任务
        manager.stop_task(&task_id).await.unwrap();
        assert_eq!(manager.active_task_count(), 0);

        // 验证任务状态
        let status = manager.get_task_status(&task_id).await;
        assert!(status.is_none());
    }

    /// 测试AuthService
    #[tokio::test]
    async fn test_auth_service_generate_and_validate_token() {
        let config = AuthConfig {
            jwt_secret: "test_secret_key".to_string(),
            token_expiry_hours: 1,
            enable_auth: true,
        };

        let auth_service = AuthService::new(config);

        // 生成token
        let token = auth_service
            .generate_token("user123", "testuser", vec!["user".to_string()])
            .unwrap();

        assert!(!token.is_empty());

        // 验证token
        let claims = auth_service.validate_token(&token).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.name, "testuser");
        assert_eq!(claims.roles, vec!["user".to_string()]);
    }

    #[tokio::test]
    async fn test_auth_service_invalid_token() {
        let config = AuthConfig::default();
        let auth_service = AuthService::new(config);

        // 尝试验证无效token
        let result = auth_service.validate_token("invalid_token");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_auth_service_permissions() {
        use crate::grpc_service::auth::{Claims, Permission};

        let config = AuthConfig::default();
        let auth_service = AuthService::new(config);

        // 测试用户权限
        let user_claims = Claims {
            sub: "user123".to_string(),
            name: "Test User".to_string(),
            roles: vec!["user".to_string()],
            exp: 0,
            iat: 0,
        };

        assert!(auth_service.check_permission(&user_claims, Permission::ReadMdb));
        assert!(auth_service.check_permission(&user_claims, Permission::ViewProgress));
        assert!(!auth_service.check_permission(&user_claims, Permission::StartTask));
        assert!(!auth_service.check_permission(&user_claims, Permission::AdminAccess));

        // 测试管理员权限
        let admin_claims = Claims {
            sub: "admin123".to_string(),
            name: "Test Admin".to_string(),
            roles: vec!["admin".to_string()],
            exp: 0,
            iat: 0,
        };

        assert!(auth_service.check_permission(&admin_claims, Permission::ReadMdb));
        assert!(auth_service.check_permission(&admin_claims, Permission::ViewProgress));
        assert!(auth_service.check_permission(&admin_claims, Permission::StartTask));
        assert!(auth_service.check_permission(&admin_claims, Permission::AdminAccess));
    }

    /// 测试RateLimiter
    #[tokio::test]
    async fn test_rate_limiter() {
        use crate::grpc_service::auth::RateLimiter;
        use std::time::Duration;

        let rate_limiter = RateLimiter::new(3, Duration::from_secs(1));
        let client_id = "test_client";

        // 前3个请求应该被允许
        assert!(rate_limiter.allow_request(client_id));
        assert!(rate_limiter.allow_request(client_id));
        assert!(rate_limiter.allow_request(client_id));

        // 第4个请求应该被拒绝
        assert!(!rate_limiter.allow_request(client_id));

        // 等待窗口期过去
        sleep(Duration::from_secs(2)).await;

        // 现在应该又可以发送请求了
        assert!(rate_limiter.allow_request(client_id));
    }

    /// 测试InputValidator
    #[tokio::test]
    async fn test_input_validator() {
        use crate::grpc_service::auth::InputValidator;

        // 测试有效的任务ID
        assert!(InputValidator::validate_task_id("valid_task_123").is_ok());

        // 测试无效的任务ID
        assert!(InputValidator::validate_task_id("").is_err());
        assert!(InputValidator::validate_task_id(&"x".repeat(101)).is_err());
        assert!(InputValidator::validate_task_id("task<script>").is_err());

        // 测试有效的MDB名称
        assert!(InputValidator::validate_mdb_name("valid_mdb").is_ok());

        // 测试无效的MDB名称
        assert!(InputValidator::validate_mdb_name("").is_err());
        assert!(InputValidator::validate_mdb_name("../etc/passwd").is_err());
        assert!(InputValidator::validate_mdb_name("mdb/with/slash").is_err());

        // 测试字符串清理
        let dirty_string = "<script>alert('xss')</script>";
        let clean_string = InputValidator::sanitize_string(dirty_string);
        assert!(!clean_string.contains("<script>"));
        assert!(clean_string.contains("&lt;script&gt;"));
    }

    /// 集成测试辅助函数
    pub async fn create_test_progress_manager() -> ProgressManager {
        ProgressManager::new()
    }

    pub async fn create_test_task_manager() -> TaskManager {
        TaskManager::new(2)
    }

    pub fn create_test_auth_service() -> AuthService {
        let config = AuthConfig {
            jwt_secret: "test_secret_for_testing_only".to_string(),
            token_expiry_hours: 1,
            enable_auth: true,
        };
        AuthService::new(config)
    }
}
