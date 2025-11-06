// 部署站点管理 JavaScript

function deploymentSitesApp() {
    return {
        // 数据状态
        sites: [],
        currentPage: 1,
        totalPages: 1,
        totalItems: 0,
        perPage: 10,
        loading: false,
        
        // 过滤和搜索
        searchQuery: '',
        statusFilter: '',
        envFilter: '',
        
        // 模态框状态
        showCreateModal: false,
        showTaskModal: false,
        showDetailModal: false,
        
        // 新站点表单数据
        newSite: {
            name: '',
            description: '',
            env: 'dev',
            owner: '',
            selectedProjectsText: '',
            config: {
                name: '默认配置',
                manual_db_nums: [],
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
                room_keyword: '-RM',
                target_sesno: null
            }
        },
        
        // 任务请求数据
        taskRequest: {
            site_id: '',
            task_type: 'ParsePdmsData',
            priority: 'Normal'
        },
        
        // 当前选中的站点
        selectedSite: null,
        
        // 初始化
        init() {
            this.loadSites();
        },
        
        // 加载站点列表
        async loadSites() {
            this.loading = true;
            try {
                const params = new URLSearchParams({
                    page: this.currentPage,
                    per_page: this.perPage
                });
                
                if (this.searchQuery) params.append('q', this.searchQuery);
                if (this.statusFilter) params.append('status', this.statusFilter);
                if (this.envFilter) params.append('env', this.envFilter);
                
                const response = await fetch(`/api/deployment-sites?${params}`);
                const data = await response.json();
                
                this.sites = data.items || [];
                this.totalPages = data.pages || 1;
                this.totalItems = data.total || 0;
                this.currentPage = data.page || 1;
            } catch (error) {
                console.error('加载站点列表失败:', error);
                this.showError('加载站点列表失败');
            } finally {
                this.loading = false;
            }
        },
        
        // 搜索站点
        searchSites() {
            this.currentPage = 1;
            this.loadSites();
        },
        
        // 过滤站点
        filterSites() {
            this.currentPage = 1;
            this.loadSites();
        },
        
        // 刷新站点列表
        refreshSites() {
            this.loadSites();
        },
        
        // 换页
        changePage(page) {
            if (page >= 1 && page <= this.totalPages) {
                this.currentPage = page;
                this.loadSites();
            }
        },
        
        // 获取分页页码数组
        get pageNumbers() {
            const pages = [];
            const start = Math.max(1, this.currentPage - 2);
            const end = Math.min(this.totalPages, this.currentPage + 2);
            
            for (let i = start; i <= end; i++) {
                pages.push(i);
            }
            return pages;
        },
        
        // 创建站点
        async createSite() {
            try {
                // 解析E3D项目路径
                const selectedProjects = this.newSite.selectedProjectsText
                    .split('\n')
                    .map(path => path.trim())
                    .filter(path => path.length > 0);
                
                const siteData = {
                    name: this.newSite.name,
                    description: this.newSite.description || null,
                    env: this.newSite.env,
                    owner: this.newSite.owner || null,
                    selected_projects: selectedProjects,
                    config: this.newSite.config
                };
                
                const response = await fetch('/api/deployment-sites', {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json'
                    },
                    body: JSON.stringify(siteData)
                });
                
                if (response.ok) {
                    this.showCreateModal = false;
                    this.resetNewSiteForm();
                    this.loadSites();
                    this.showSuccess('站点创建成功');
                } else {
                    const error = await response.json();
                    throw new Error(error.error || '创建失败');
                }
            } catch (error) {
                console.error('创建站点失败:', error);
                this.showError('创建站点失败: ' + error.message);
            }
        },
        
        // 重置新站点表单
        resetNewSiteForm() {
            this.newSite = {
                name: '',
                description: '',
                env: 'dev',
                owner: '',
                selectedProjectsText: '',
                config: {
                    name: '默认配置',
                    manual_db_nums: [],
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
                    room_keyword: '-RM',
                    target_sesno: null
                }
            };
        },
        
        // 查看站点详情
        viewSiteDetail(site) {
            this.selectedSite = site;
            this.showDetailModal = true;
        },
        
        // 编辑站点
        editSite(site) {
            // TODO: 实现编辑功能
            this.showInfo('编辑功能开发中...');
        },
        
        // 删除站点
        async deleteSite(site) {
            if (!confirm(`确定要删除站点 "${site.name}" 吗？`)) {
                return;
            }
            
            try {
                const response = await fetch(`/api/deployment-sites/${site.id}`, {
                    method: 'DELETE'
                });
                
                if (response.ok) {
                    this.loadSites();
                    this.showSuccess('站点删除成功');
                } else {
                    throw new Error('删除失败');
                }
            } catch (error) {
                console.error('删除站点失败:', error);
                this.showError('删除站点失败');
            }
        },
        
        // 为站点创建任务
        createSiteTask(site) {
            this.selectedSite = site;
            this.taskRequest.site_id = site.id;
            this.showTaskModal = true;
        },
        
        // 提交创建任务
        async submitCreateTask() {
            try {
                const response = await fetch(`/api/deployment-sites/${this.taskRequest.site_id}/tasks`, {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json'
                    },
                    body: JSON.stringify(this.taskRequest)
                });
                
                if (response.ok) {
                    const result = await response.json();
                    this.showTaskModal = false;
                    this.showSuccess(`任务创建成功，任务ID: ${result.task_id}`);
                    // 自动启动任务
                    try {
                        await fetch(`/api/tasks/${encodeURIComponent(result.task_id)}/start`, { method: 'POST' });
                        this.showSuccess('任务已自动启动');
                    } catch (e) {
                        console.error('自动启动任务失败:', e);
                        this.showError('任务已创建，但启动失败，请在任务页面手动启动');
                    }
                } else {
                    const error = await response.json();
                    throw new Error(error.error || '创建任务失败');
                }
            } catch (error) {
                console.error('创建任务失败:', error);
                this.showError('创建任务失败: ' + error.message);
            }
        },
        
        // 获取状态颜色样式
        getStatusColor(status) {
            const colors = {
                'Configuring': 'bg-yellow-100 text-yellow-800',
                'Deploying': 'bg-blue-100 text-blue-800',
                'Running': 'bg-green-100 text-green-800',
                'Failed': 'bg-red-100 text-red-800',
                'Stopped': 'bg-gray-100 text-gray-800'
            };
            return colors[status] || 'bg-gray-100 text-gray-800';
        },
        
        // 获取状态文本
        getStatusText(status) {
            const texts = {
                'Configuring': '配置中',
                'Deploying': '部署中',
                'Running': '运行中',
                'Failed': '失败',
                'Stopped': '已停止'
            };
            return texts[status] || status;
        },
        
        // 格式化日期
        formatDate(dateString) {
            if (!dateString) return '-';
            try {
                const date = new Date(dateString);
                return date.toLocaleString('zh-CN');
            } catch {
                return dateString;
            }
        },
        
        // 显示成功消息
        showSuccess(message) {
            this.showNotification(message, 'success');
        },
        
        // 显示错误消息
        showError(message) {
            this.showNotification(message, 'error');
        },
        
        // 显示信息消息
        showInfo(message) {
            this.showNotification(message, 'info');
        },
        
        // 显示通知
        showNotification(message, type = 'info') {
            // 创建通知元素
            const notification = document.createElement('div');
            notification.className = `fixed top-4 right-4 p-4 rounded-md shadow-lg z-50 ${
                type === 'success' ? 'bg-green-100 text-green-800 border border-green-200' :
                type === 'error' ? 'bg-red-100 text-red-800 border border-red-200' :
                'bg-blue-100 text-blue-800 border border-blue-200'
            }`;
            notification.innerHTML = `
                <div class="flex items-center">
                    <i class="fas ${
                        type === 'success' ? 'fa-check-circle' :
                        type === 'error' ? 'fa-exclamation-circle' :
                        'fa-info-circle'
                    } mr-2"></i>
                    <span>${message}</span>
                    <button onclick="this.parentElement.parentElement.remove()" class="ml-2 text-gray-400 hover:text-gray-600">
                        <i class="fas fa-times"></i>
                    </button>
                </div>
            `;
            
            document.body.appendChild(notification);
            
            // 3秒后自动移除
            setTimeout(() => {
                if (notification.parentElement) {
                    notification.remove();
                }
            }, 3000);
        }
    };
}
