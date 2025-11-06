# 异地协同同步压缩包 HTTP 访问方案梳理

## 1. 背景与目标
- 异地协同基于 `dpcsync` 生成增量压缩包（数据库、模型等），由中心节点管理并推送至各站点。
- 现有系统以 `web_server` Feature 提供监控与 API，但需统一为 `web_server` 并补全直接 HTTP 目录访问能力，便于远端站点或第三方服务按需拉取压缩包。
- 新需求：提供指定协同环境/站点的压缩包目录浏览与文件下载，确保权限、路径及缓存策略安全。

## 2. 现有异步流程概述
1. **压缩包生成**  
   - `increment_manager` 中的增量任务调用 `execute_compress` / 导出逻辑，生成 `dpcsync` 压缩文件及关联元数据（记录条数、哈希等）。
   - 生成结束后调用 `enqueue_generated_sync_tasks`，将 `GeneratedSyncArtifact` 入队，全局同步中心 `SYNC_CONTROL_CENTER` 开始调度。

2. **任务调度与分发**  
   - `SYNC_CONTROL_CENTER`（`sync_control_center.rs`）维护任务队列、历史、状态统计，后台协程 `process_sync_task` 串行消费：
     - 目标为本地路径（`ResolvedTarget::Local`）：`fs::copy` 将压缩包写入 `<base>/<env>/<site>/<direction>/`，随后 `update_site_metadata` 写入 `metadata.json` 并落缓存。
     - 目标为 HTTP（`ResolvedTarget::Http`）：PUT 上传至远端 URL，成功后 `refresh_remote_site_metadata` 反向拉取 `metadata.json` 缓存到本地。

3. **元数据服务**  
   - `site_metadata.rs` 负责读取/写入本地元数据文件、缓存、HTTP 拉取等操作。
   - Axum 端点 `get_site_metadata` / `download_site_metadata_file` 提供 JSON 元数据和文件下载（本地存在则直接读取，缺少则重定向至远端 `download_url`）。
   - 前端协同详情页根据上述 API 展示同步日志、流向图和元数据条目，方便手动下载文件。

## 3. 亟需补全的 HTTP 目录访问能力
目标：在重命名后的 `web_server` 中开放 REST 或静态形式的列表/下载接口，允许按项目（协同环境）查看已有的 dpcsync 压缩包，并直接下载。

需要考虑的关键点：
1. **目录定位**  
   - 基于站点配置（`remote_sync_envs.file_server_host`、`remote_sync_sites.http_host`）或默认输出目录 `output/remote_sync`，为每个环境/站点确定白名单根路径。
   - 支持按元数据指向的具体文件（`metadata.entries` 中的 `file_path`）和路径结构列目录。

2. **权限与安全**  
   - 禁止路径穿越，仅允许访问配置允许的目录树。
   - 若未来存在访问控制/鉴权需求，应在 Handler 层扩展 token 校验或网关限制。

3. **带宽与缓存**  
   - 对大文件提供流式响应（`tokio_util::io::ReaderStream` + `Body::from_stream`），避免一次性读入内存。
   - 可选择实现 Range/断点续传（可先留 TODO），同时考虑 Nginx/LiteFS 方案时的兼容。

4. **与元数据联动**  
   - 列表接口最好直接复用 `metadata.json`，保证展示顺序（更新时间/方向等）与 UI 一致。
   - 下载接口在返回文件前再次校验元数据一致性（文件存在、大小/哈希匹配），以防读到已被替换/删除的旧文件。

## 4. 模块重命名影响（`web_server` → `web_server`）
1. **Feature 与模块名**  
   - `Cargo.toml` 中的 `web_server` Feature 需要改为 `web_server`，所有 `#[cfg(feature = "web_server")]` 宏同步修改。
   - 目录 `src/web_server/` 及引用 `crate::web_server::...` 需转换为 `src/web_server/`，包含：同步中心、Axum handlers、模板与前端依赖。

2. **初始化与路由**  
   - `src/web_api` 或主入口需调整 Feature 判断逻辑，确保启用 `--features web_server` 时注册路由。
   - 前端如果仍位于同一仓库，需要同步更新 API 路径引用（目前都走 `/api/remote-sync/...`，命名更改对接口协议无影响）。

3. **测试与构建链路**  
   - 调整 CI/文档：`cargo build --all-features`、`cargo run --bin web_server` 等命令改为新的 Feature/二进制名。
   - 注意已有的 `cargo fmt` / `cargo check` 兼容旧路径；重命名后需一次性全量跑通。

## 5. TODO 列表
1. **命名迁移**
   - [ ] 重命名 Feature 与模块目录：`web_server` → `web_server`，批量替换所有 `cfg` 与 `use`。
   - [ ] 更新构建脚本、文档、启动脚本（如 `README_web_server.md`）以匹配新名称。

2. **HTTP 目录访问实现**
   - [ ] 定义新的 Axum Handler：`GET /api/remote-sync/sites/{id}/files`（返回文件列表）与 `GET /api/remote-sync/sites/{id}/files/{file}`（流式下载）。
   - [ ] 基于 `site_metadata::read_local_metadata` 汇总条目，校验合法路径并返回结构化 JSON。
   - [ ] 文件下载 Handler 使用 `tokio::fs::File` + `ReaderStream`，并设置合适的 MIME/响应头。
   - [ ] 考虑错误处理（文件缺失、后端不存在元数据）与安全校验（阻止路径注入）。

3. **元数据增强（可选）**
   - [ ] 在 `metadata.json` 中记录文件保留策略（是否已被替换/过期），供目录接口过滤。
   - [ ] 视需要添加哈希校验 API，供远端 clone 后验证。

4. **前端/文档同步**
   - [ ] 前端协同详情页增加“HTTP 目录访问”入口或文件列表对接新接口。
   - [ ] 更新操作手册/部署文档，说明如何通过新接口下载 dpcsync 压缩包。

5. **验证与部署**
   - [ ] `cargo check --features web_server`、`cargo test` 验证新模块编译。
   - [ ] 预留本地/测试环境对大文件下载的压测，评估带宽及超时设置。

> 注：本文件旨在梳理现状与拆解任务，具体实现时需根据最新代码结构调整路径与接口命名。***
