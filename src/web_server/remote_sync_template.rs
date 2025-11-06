/// 远程增量环境配置页面

pub fn render_remote_sync_page_with_sidebar() -> String {
    let content = r#"
<div x-data="remoteSyncApp()" x-init="init()" x-cloak class="space-y-6 relative">
  <div x-show="toast.show" x-transition class="fixed top-20 right-6 z-50" style="display:none;">
    <div :class="toastClass()" class="flex items-center space-x-2 px-4 py-2 rounded-lg shadow-lg">
      <i :class="toastIcon()"></i>
      <span class="text-sm" x-text="toast.text"></span>
    </div>
  </div>
  <div class="flex items-center justify-between">
    <h1 class="text-2xl font-bold text-gray-800"><i class='fas fa-project-diagram mr-2 text-blue-600'></i>异地增量环境配置</h1>
    <div class="space-x-2">
      <button @click="openCreateEnv()" class="px-3 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700"><i class='fas fa-plus mr-1'></i> 新建环境</button>
      <button @click="loadEnvs()" class="px-3 py-1.5 bg-gray-100 rounded hover:bg-gray-200"><i class='fas fa-sync mr-1'></i> 刷新</button>
    </div>
  </div>

  <div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
    <!-- 运行时状态 -->
    <div class="lg:col-span-3 bg-white rounded-lg shadow p-4">
      <div class="flex items-center justify-between">
        <div>
          <h2 class="font-semibold text-gray-700">运行时状态</h2>
          <div class="text-sm text-gray-600 mt-1">
            <span>当前激活环境：</span>
            <span class="font-mono" x-text="runtimeStatus.env_id || '-' "></span>
            <span class="ml-4">MQTT：</span>
            <span :class="runtimeStatus.mqtt_connected===true?'text-green-600':(runtimeStatus.mqtt_connected===false?'text-red-600':'text-gray-500')"
                  x-text="runtimeStatus.mqtt_connected===true?'已连接':(runtimeStatus.mqtt_connected===false?'未连接':'未知')"></span>
            <span class="ml-4">QoS：</span>
            <span class="text-gray-700">ExactlyOnce</span>
          </div>
        </div>
        <button @click="refreshRuntime()" class="px-3 py-1.5 bg-gray-100 rounded hover:bg-gray-200"><i class="fas fa-sync mr-1"></i> 刷新</button>
      </div>
    </div>
    <!-- 环境列表 -->
    <div class="lg:col-span-1 bg-white rounded-lg shadow p-4">
      <h2 class="font-semibold text-gray-700 mb-3">环境列表</h2>
      <template x-if="envs.length===0">
        <div class="text-gray-500 text-sm">暂无环境，点击右上角“新建环境”</div>
      </template>
      <ul class="divide-y" x-show="envs.length>0">
        <template x-for="e in envs" :key="e.id">
          <li @click="selectEnv(e)" class="py-2 cursor-pointer" :class="selectedEnv && selectedEnv.id===e.id ? 'text-blue-700' : 'text-gray-700'">
            <div class="flex items-center justify-between">
              <div>
                <div class="font-medium" x-text="e.name"></div>
                <div class="text-xs text-gray-500" x-text="(e.location||'-') + ' · MQTT: ' + (e.mqtt_host||'-')"></div>
              </div>
              <div class="space-x-2">
                <button @click.stop="editEnv(e)" class="text-blue-600 hover:text-blue-800 text-sm"><i class='fas fa-edit'></i></button>
                <button @click.stop="deleteEnv(e)" class="text-red-600 hover:text-red-800 text-sm"><i class='fas fa-trash'></i></button>
              </div>
            </div>
          </li>
        </template>
      </ul>
    </div>

    <!-- 环境详情与站点 -->
    <div class="lg:col-span-2 bg-white rounded-lg shadow p-4">
      <template x-if="!selectedEnv">
        <div class="text-gray-500 text-sm">请选择左侧环境查看详情或新建一个环境</div>
      </template>

      <div x-show="selectedEnv">
        <!-- 环境配置 -->
        <h2 class="font-semibold text-gray-700 mb-3">环境配置</h2>
        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div>
            <label class="block text-sm text-gray-600 mb-1">环境名称</label>
            <input class="w-full border rounded px-3 py-2" x-model="envForm.name" />
          </div>
          <div>
            <label class="block text-sm text-gray-600 mb-1">地区标识 location</label>
            <input class="w-full border rounded px-3 py-2" x-model="envForm.location" placeholder="如: bj / sjz / zz" />
          </div>
          <div>
            <label class="block text-sm text-gray-600 mb-1">MQTT 主机</label>
            <input class="w-full border rounded px-3 py-2" x-model="envForm.mqtt_host" placeholder="mqtt.example.com" />
          </div>
          <div>
            <label class="block text-sm text-gray-600 mb-1">MQTT 端口</label>
            <input type="number" class="w-full border rounded px-3 py-2" x-model.number="envForm.mqtt_port" placeholder="1883" />
          </div>
          <div class="md:col-span-2">
            <label class="block text-sm text-gray-600 mb-1">当前站点的文件服务地址（用于分发 .cba）</label>
            <input class="w-full border rounded px-3 py-2" x-model="envForm.file_server_host" placeholder="http://host:port/assets/archives" />
          </div>
          <div class="md:col-span-2">
            <label class="block text-sm text-gray-600 mb-1">本地区负责的 dbnums（逗号分隔）</label>
            <input class="w-full border rounded px-3 py-2" x-model="envForm.location_dbs" placeholder="7999,8001,8002" />
          </div>
          <div>
            <label class="block text-sm text-gray-600 mb-1">重连初始间隔(ms)</label>
            <input type="number" class="w-full border rounded px-3 py-2" x-model.number="envForm.reconnect_initial_ms" placeholder="1000" />
          </div>
          <div>
            <label class="block text-sm text-gray-600 mb-1">重连最大间隔(ms)</label>
            <input type="number" class="w-full border rounded px-3 py-2" x-model.number="envForm.reconnect_max_ms" placeholder="30000" />
          </div>
        </div>
        <div class="mt-3 space-x-2">
          <button @click="saveEnv()" class="px-3 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700"><i class='fas fa-save mr-1'></i> 保存环境</button>
          <button @click="applyEnv()" :disabled="!selectedEnv" class="px-3 py-1.5 bg-purple-600 text-white rounded hover:bg-purple-700 disabled:bg-gray-400"><i class='fas fa-bolt mr-1'></i> 写入配置</button>
          <button @click="activateEnv()" :disabled="!selectedEnv" class="px-3 py-1.5 bg-indigo-600 text-white rounded hover:bg-indigo-700 disabled:bg-gray-400"><i class='fas fa-play mr-1'></i> 应用即生效</button>
          <button @click="stopRuntime()" class="px-3 py-1.5 bg-gray-600 text-white rounded hover:bg-gray-700"><i class='fas fa-stop mr-1'></i> 停止运行时</button>
          <button @click="testMqtt()" :disabled="!selectedEnv" class="px-3 py-1.5 bg-amber-500 text-white rounded hover:bg-amber-600 disabled:bg-gray-400"><i class='fas fa-satellite-dish mr-1'></i> 测试 MQTT</button>
          <button @click="testHttp()" :disabled="!selectedEnv" class="px-3 py-1.5 bg-amber-600 text-white rounded hover:bg-amber-700 disabled:bg-gray-400"><i class='fas fa-link mr-1'></i> 测试文件服务</button>
        </div>

        <!-- 外部站点 -->
        <div class="mt-6">
          <div class="flex items-center justify-between mb-2">
            <h2 class="font-semibold text-gray-700">外部站点</h2>
            <button @click="openCreateSite()" class="px-3 py-1.5 bg-green-600 text-white rounded hover:bg-green-700"><i class='fas fa-plus mr-1'></i> 新增站点</button>
          </div>
          <div class="overflow-x-auto">
            <table class="min-w-full text-sm">
              <thead>
                <tr class="text-left text-gray-600 border-b">
                  <th class="py-2 pr-4">名称</th>
                  <th class="py-2 pr-4">Location</th>
                  <th class="py-2 pr-4">HTTP Host</th>
                  <th class="py-2 pr-4">DBNums</th>
                  <th class="py-2 pr-4">操作</th>
                </tr>
              </thead>
              <tbody>
                <template x-for="s in sites" :key="s.id">
                  <tr class="border-b">
                    <td class="py-2 pr-4" x-text="s.name"></td>
                    <td class="py-2 pr-4" x-text="s.location||'-'"></td>
                    <td class="py-2 pr-4" x-text="s.http_host||'-'"></td>
                    <td class="py-2 pr-4" x-text="s.dbnums||'-'"></td>
                    <td class="py-2 pr-4 space-x-2">
                      <button @click="editSite(s)" class="text-blue-600 hover:text-blue-800"><i class='fas fa-edit'></i></button>
                      <button @click="deleteSite(s)" class="text-red-600 hover:text-red-800"><i class='fas fa-trash'></i></button>
                      <button @click="testSiteHttp(s)" class="text-amber-600 hover:text-amber-800"><i class='fas fa-link'></i></button>
                    </td>
                  </tr>
                </template>
                <tr x-show="sites.length===0"><td class="py-3 text-gray-500" colspan="5">暂无外部站点</td></tr>
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  </div>

  <!-- 环境创建/编辑模态框 -->
  <div x-show="showEnvModal" class="fixed inset-0 bg-black bg-opacity-30 flex items-center justify-center" style="display:none" @click.self="showEnvModal=false">
    <div class="bg-white rounded-lg shadow p-4 w-full max-w-xl">
      <h3 class="font-semibold mb-3" x-text="envForm.id ? '编辑环境' : '新建环境'"></h3>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div>
          <label class="block text-sm text-gray-600 mb-1">环境名称</label>
          <input class="w-full border rounded px-3 py-2" x-model="envForm.name" />
        </div>
        <div>
          <label class="block text-sm text-gray-600 mb-1">Location</label>
          <input class="w-full border rounded px-3 py-2" x-model="envForm.location" />
        </div>
        <div>
          <label class="block text-sm text-gray-600 mb-1">MQTT 主机</label>
          <input class="w-full border rounded px-3 py-2" x-model="envForm.mqtt_host" />
        </div>
        <div>
          <label class="block text-sm text-gray-600 mb-1">MQTT 端口</label>
          <input type="number" class="w-full border rounded px-3 py-2" x-model.number="envForm.mqtt_port" />
        </div>
        <div class="md:col-span-2">
          <label class="block text-sm text-gray-600 mb-1">文件服务地址</label>
          <input class="w-full border rounded px-3 py-2" x-model="envForm.file_server_host" />
        </div>
        <div class="md:col-span-2">
          <label class="block text-sm text-gray-600 mb-1">本地 DBNums（逗号分隔）</label>
          <input class="w-full border rounded px-3 py-2" x-model="envForm.location_dbs" />
        </div>
      </div>
      <div class="mt-4 text-right space-x-2">
        <button @click="showEnvModal=false" class="px-3 py-1.5 bg-gray-100 rounded hover:bg-gray-200">取消</button>
        <button @click="saveEnv()" class="px-3 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700">保存</button>
      </div>
    </div>
  </div>

  <!-- 站点创建/编辑模态框 -->
  <div x-show="showSiteModal" class="fixed inset-0 bg-black bg-opacity-30 flex items-center justify-center" style="display:none" @click.self="showSiteModal=false">
    <div class="bg-white rounded-lg shadow p-4 w-full max-w-xl">
      <h3 class="font-semibold mb-3" x-text="siteForm.id ? '编辑站点' : '新增站点'"></h3>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div class="md:col-span-2">
          <label class="block text-sm text-gray-600 mb-1">站点名称</label>
          <input class="w-full border rounded px-3 py-2" x-model="siteForm.name" />
        </div>
        <div>
          <label class="block text-sm text-gray-600 mb-1">Location</label>
          <input class="w-full border rounded px-3 py-2" x-model="siteForm.location" />
        </div>
        <div>
          <label class="block text-sm text-gray-600 mb-1">HTTP Host</label>
          <input class="w-full border rounded px-3 py-2" x-model="siteForm.http_host" placeholder="http://host:port/assets/archives" />
        </div>
        <div class="md:col-span-2">
          <label class="block text-sm text-gray-600 mb-1">DBNums（逗号分隔，可选）</label>
          <input class="w-full border rounded px-3 py-2" x-model="siteForm.dbnums" />
        </div>
        <div class="md:col-span-2">
          <label class="block text-sm text-gray-600 mb-1">备注</label>
          <textarea class="w-full border rounded px-3 py-2" x-model="siteForm.notes"></textarea>
        </div>
      </div>
      <div class="mt-4 text-right space-x-2">
        <button @click="showSiteModal=false" class="px-3 py-1.5 bg-gray-100 rounded hover:bg-gray-200">取消</button>
        <button @click="saveSite()" class="px-3 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700">保存</button>
      </div>
    </div>
  </div>
</div>
"#;

    let extra_head = Some(
        r#"
        <script src="/static/alpine.min.js" defer></script>
        <style>[x-cloak] { display: none !important; }</style>
    "#,
    );
    let extra_scripts = Some(REMOTE_SYNC_JS);
    crate::web_server::layout::render_layout_with_sidebar(
        "异地增量环境配置",
        Some("remote-sync"),
        content,
        extra_head,
        extra_scripts,
    )
}

const REMOTE_SYNC_JS: &str = r#"
function remoteSyncApp(){
  return {
    envs: [],
    runtimeStatus: { env_id: null, mqtt_connected: null },
    selectedEnv: null,
    sites: [],
    showEnvModal: false,
    showSiteModal: false,
    envForm: { id: null, name: '', mqtt_host: '', mqtt_port: 1883, file_server_host: '', location: '', location_dbs: '', reconnect_initial_ms: 1000, reconnect_max_ms: 30000 },
    siteForm: { id: null, name: '', location: '', http_host: '', dbnums: '', notes: '' },
    desiredEnvId: null,
    toast: { show: false, type: 'info', text: '' },
    toastTimer: null,
    notify(type, text){
      this.toast.type = type || 'info';
      this.toast.text = text || '';
      this.toast.show = true;
      if(this.toastTimer){ clearTimeout(this.toastTimer); }
      this.toastTimer = setTimeout(()=>{ this.toast.show = false; }, 3500);
    },
    toastClass(){
      return {
        info: 'bg-blue-600 text-white',
        success: 'bg-green-600 text-white',
        warning: 'bg-amber-500 text-white',
        error: 'bg-red-600 text-white'
      }[this.toast.type || 'info'];
    },
    toastIcon(){
      return {
        info: 'fas fa-info-circle',
        success: 'fas fa-check-circle',
        warning: 'fas fa-exclamation-triangle',
        error: 'fas fa-times-circle'
      }[this.toast.type || 'info'];
    },
    handleError(err, fallback){
      console.error(err);
      const msg = err?.message || fallback || '操作失败';
      this.notify('error', msg);
    },
    normalizeEnv(env){
      env = env || {};
      return {
        id: env.id || null,
        name: env.name || '',
        mqtt_host: env.mqtt_host || '',
        mqtt_port: env.mqtt_port ?? 1883,
        file_server_host: env.file_server_host || '',
        location: env.location || '',
        location_dbs: env.location_dbs || '',
        reconnect_initial_ms: env.reconnect_initial_ms ?? 1000,
        reconnect_max_ms: env.reconnect_max_ms ?? 30000,
      };
    },
    normalizeSite(site){
      site = site || {};
      return {
        id: site.id || null,
        name: site.name || '',
        location: site.location || '',
        http_host: site.http_host || '',
        dbnums: site.dbnums || '',
        notes: site.notes || '',
      };
    },
    sanitizeEnvPayload(){
      const numOrNull = (v) => {
        const n = Number(v);
        return Number.isFinite(n) ? n : null;
      };
      const trimOrNull = (v) => {
        if(v === undefined || v === null) return null;
        const s = String(v).trim();
        return s.length ? s : null;
      };
      return {
        name: (this.envForm.name || '').trim(),
        mqtt_host: trimOrNull(this.envForm.mqtt_host),
        mqtt_port: numOrNull(this.envForm.mqtt_port),
        file_server_host: trimOrNull(this.envForm.file_server_host),
        location: trimOrNull(this.envForm.location),
        location_dbs: trimOrNull(this.envForm.location_dbs),
        reconnect_initial_ms: numOrNull(this.envForm.reconnect_initial_ms),
        reconnect_max_ms: numOrNull(this.envForm.reconnect_max_ms),
      };
    },
    sanitizeSitePayload(){
      const trimOrNull = (v) => {
        if(v === undefined || v === null) return null;
        const s = String(v).trim();
        return s.length ? s : null;
      };
      const compactList = (v) => {
        if(!v) return null;
        const arr = String(v).split(',').map(s => s.trim()).filter(Boolean);
        return arr.length ? arr.join(',') : null;
      };
      return {
        name: (this.siteForm.name || '').trim(),
        location: trimOrNull(this.siteForm.location),
        http_host: trimOrNull(this.siteForm.http_host),
        dbnums: compactList(this.siteForm.dbnums),
        notes: trimOrNull(this.siteForm.notes),
      };
    },
    async fetchJson(url, options = {}, errorHint){
      try {
        const resp = await fetch(url, options);
        const text = await resp.text();
        let data = {};
        if(text){
          try { data = JSON.parse(text); } catch(_) { data = { message: text }; }
        }
        if(!resp.ok){
          const detail = data && typeof data === 'object' ? (data.message || data.error || JSON.stringify(data)) : `HTTP ${resp.status}`;
          throw new Error(errorHint ? `${errorHint}: ${detail}` : detail);
        }
        return data;
      } catch (err) {
        throw err instanceof Error ? err : new Error(errorHint || '网络异常');
      }
    },
    async loadEnvs(){
      try {
        const d = await this.fetchJson('/api/remote-sync/envs', {}, '加载环境失败');
        this.envs = d.items || [];
        if(this.desiredEnvId){
          const found = this.envs.find(x => x.id === this.desiredEnvId);
          if(found){ this.selectEnv(found); this.desiredEnvId = null; }
        }
        if(!this.selectedEnv && this.envs.length > 0){ this.selectEnv(this.envs[0]); }
      } catch (err) {
        this.handleError(err, '加载环境失败');
      }
    },
    async refreshRuntime(){
      try {
        const d = await this.fetchJson('/api/remote-sync/runtime/status', {}, '查询运行状态失败');
        if(d && d.status === 'success'){
          this.runtimeStatus.env_id = d.env_id || null;
          this.runtimeStatus.mqtt_connected = d.mqtt_connected;
        }
      } catch (err) {
        this.handleError(err, '查询运行状态失败');
      }
    },
    async selectEnv(e){
      this.selectedEnv = e;
      this.envForm = Object.assign({ id: e.id }, this.normalizeEnv(e));
      await this.loadSites();
    },
    openCreateEnv(){
      console.log('打开新建环境对话框');
      this.envForm = this.normalizeEnv();
      console.log('初始化表单:', this.envForm);
      this.showEnvModal = true;
    },
    editEnv(e){
      this.envForm = Object.assign({ id: e.id }, this.normalizeEnv(e));
      this.showEnvModal = true;
    },
    async saveEnv(){
      if(!this.envForm.name || !this.envForm.name.trim()){
        this.notify('warning','环境名称不能为空');
        return;
      }
      const payload = this.sanitizeEnvPayload();
      console.log('保存环境 - 请求数据:', payload);
      try {
        if(this.envForm.id){
          await this.fetchJson(`/api/remote-sync/envs/${this.envForm.id}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
          }, '更新环境失败');
          this.notify('success','环境已更新');
        } else {
          const d = await this.fetchJson('/api/remote-sync/envs', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
          }, '创建环境失败');
          console.log('创建环境 - 响应数据:', d);
          if(d && d.id){
            this.desiredEnvId = d.id;
            console.log('设置 desiredEnvId:', this.desiredEnvId);
          }
          this.notify('success','环境已创建');
        }
        this.showEnvModal = false;
        await this.loadEnvs();
        console.log('环境列表已刷新:', this.envs);
      } catch (err) {
        console.error('保存环境失败:', err);
        this.handleError(err, '保存环境失败');
      }
    },
    async activateEnv(){
      if(!this.selectedEnv){ this.notify('warning','请选择环境'); return; }
      try {
        const d = await this.fetchJson(`/api/remote-sync/envs/${this.selectedEnv.id}/activate`, { method: 'POST' }, '启用环境失败');
        this.notify('success', d.message || '已写入并启动运行时');
        await this.refreshRuntime();
      } catch (err) {
        this.handleError(err, '启用环境失败');
      }
    },
    async applyEnv(){
      if(!this.selectedEnv){ this.notify('warning','请选择环境'); return; }
      try {
        const d = await this.fetchJson(`/api/remote-sync/envs/${this.selectedEnv.id}/apply`, { method: 'POST' }, '写入配置失败');
        this.notify('success', d.message || '已写入 DbOption.toml');
      } catch (err) {
        this.handleError(err, '写入配置失败');
      }
    },
    async deleteEnv(e){
      if(!confirm(`删除环境 ${e.name}?`)) return;
      try {
        await this.fetchJson(`/api/remote-sync/envs/${e.id}`, { method: 'DELETE' }, '删除环境失败');
        this.notify('success','环境已删除');
        if(this.selectedEnv && this.selectedEnv.id === e.id){ this.selectedEnv = null; this.sites = []; }
        await this.loadEnvs();
      } catch (err) {
        this.handleError(err, '删除环境失败');
      }
    },
    async loadSites(){
      if(!this.selectedEnv){ this.sites = []; return; }
      try {
        const d = await this.fetchJson(`/api/remote-sync/envs/${this.selectedEnv.id}/sites`, {}, '加载站点失败');
        this.sites = d.items || [];
      } catch (err) {
        this.handleError(err, '加载站点失败');
      }
    },
    openCreateSite(){
      if(!this.selectedEnv){ this.notify('warning','请先选择环境'); return; }
      this.siteForm = this.normalizeSite();
      this.showSiteModal = true;
    },
    editSite(s){
      this.siteForm = Object.assign({ id: s.id }, this.normalizeSite(s));
      this.showSiteModal = true;
    },
    async saveSite(){
      if(!this.siteForm.name || !this.siteForm.name.trim()){
        this.notify('warning','站点名称不能为空');
        return;
      }
      const payload = this.sanitizeSitePayload();
      try {
        if(this.siteForm.id){
          await this.fetchJson(`/api/remote-sync/sites/${this.siteForm.id}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
          }, '更新站点失败');
          this.notify('success','站点已更新');
        } else {
          await this.fetchJson(`/api/remote-sync/envs/${this.selectedEnv.id}/sites`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
          }, '创建站点失败');
          this.notify('success','站点已创建');
        }
        this.showSiteModal = false;
        await this.loadSites();
      } catch (err) {
        this.handleError(err, '保存站点失败');
      }
    },
    async deleteSite(s){
      if(!confirm(`删除站点 ${s.name}?`)) return;
      try {
        await this.fetchJson(`/api/remote-sync/sites/${s.id}`, { method: 'DELETE' }, '删除站点失败');
        this.notify('success','站点已删除');
        await this.loadSites();
      } catch (err) {
        this.handleError(err, '删除站点失败');
      }
    },
    async stopRuntime(){
      try {
        const d = await this.fetchJson('/api/remote-sync/runtime/stop', { method: 'POST' }, '停止运行时失败');
        this.notify('success', d.message || '已停止运行时');
        await this.refreshRuntime();
      } catch (err) {
        this.handleError(err, '停止运行时失败');
      }
    },
    async testMqtt(){
      if(!this.selectedEnv){ this.notify('warning','请选择环境'); return; }
      try {
        const d = await this.fetchJson(`/api/remote-sync/envs/${this.selectedEnv.id}/test-mqtt`, { method: 'POST' }, '测试 MQTT 失败');
        const prefix = d.status === 'success' ? 'MQTT 可达' : 'MQTT 失败';
        const detail = [d.addr, d.message].filter(Boolean).join(' - ');
        this.notify(d.status === 'success' ? 'success' : 'warning', detail ? `${prefix}: ${detail}` : prefix);
      } catch (err) {
        this.handleError(err, '测试 MQTT 失败');
      }
    },
    async testHttp(){
      if(!this.selectedEnv){ this.notify('warning','请选择环境'); return; }
      try {
        const d = await this.fetchJson(`/api/remote-sync/envs/${this.selectedEnv.id}/test-http`, { method: 'POST' }, '测试文件服务失败');
        const prefix = d.status === 'success' ? '文件服务可达' : '文件服务失败';
        const detail = [d.url, d.code ? `code=${d.code}` : null, d.message].filter(Boolean).join(' · ');
        this.notify(d.status === 'success' ? 'success' : 'warning', detail ? `${prefix}: ${detail}` : prefix);
      } catch (err) {
        this.handleError(err, '测试文件服务失败');
      }
    },
    async testSiteHttp(s){
      try {
        const d = await this.fetchJson(`/api/remote-sync/sites/${s.id}/test-http`, { method: 'POST' }, '测试站点失败');
        const prefix = d.status === 'success' ? '站点可达' : '站点失败';
        const detail = [d.url, d.code ? `code=${d.code}` : null, d.message].filter(Boolean).join(' · ');
        this.notify(d.status === 'success' ? 'success' : 'warning', detail ? `${prefix}: ${detail}` : prefix);
      } catch (err) {
        this.handleError(err, '测试站点失败');
      }
    },
    async init(){
      try {
        const usp = new URLSearchParams(window.location.search);
        const env = usp.get('env');
        if(env){ this.desiredEnvId = env; }
        await this.loadEnvs();
        await this.refreshRuntime();
      } catch (err) {
        this.handleError(err, '初始化失败');
      }
    }
  }
}
document.addEventListener('alpine:init', ()=>{
  // 未来可在这里注册全局组件
});
"#;
