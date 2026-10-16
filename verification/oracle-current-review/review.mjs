import fs from 'node:fs';
import {spawn} from 'node:child_process';
import readline from 'node:readline';
const child=spawn('node',['C:/Users/dpc/AppData/Roaming/npm/node_modules/@steipete/oracle/dist/bin/oracle-mcp.js'],{stdio:['pipe','pipe','inherit']});
let id=0; const pending=new Map();
readline.createInterface({input:child.stdout}).on('line',line=>{try{const r=JSON.parse(line); if(pending.has(r.id)){pending.get(r.id)(r.result??r.error);pending.delete(r.id);}}catch{}});
function rpc(method,params){return new Promise(resolve=>{const n=++id; pending.set(n,resolve);child.stdin.write(JSON.stringify({jsonrpc:'2.0',id:n,method,params})+'\n');});}
await rpc('initialize',{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'review',version:'1'}});
child.stdin.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})+'\n');
const client={callTool:args=>rpc('tools/call',args)};
const files=['direct_store_registry','mdb_route_directory','direct_store','direct_tree','element_attributes','mdb_membership','mdb_memory_store','model_source','read_through','on_demand_db'].map(n=>`src/data_interface/${n}.rs`).concat(['src/web_service/mod.rs','src/web_service/handlers.rs','src/lib.rs','verification/http-runtime/verify_ams_metrics.py','verification/http-runtime/acceptance-gaps.md']);
const args={engine:'browser',model:'gpt-6',browserThinkingTime:'pro',browserModelStrategy:'select',files,prompt:'审核 gen-model 当前 RefNo 原生路由与 lazy MDB 实现，Windows Rust。必须以附件源码为准，不信任以前所有测试通过的叙述。目标：RefNo::dbno 解码，MDB上下文路由，零启动元素扫描，共享惰性存储，跨库属性/owner/tree 按需读取，按 path/dbnum/generation 定向失效，single-flight，DICT独立刷新，HTTP bind先于预热，health真实反映 stale/failed/refresh。特别审查 registry zip fingerprint 比较是否漏 path/identity/type 变化、失效后并发旧读是否回填、provider 是否每请求检查 registry、schema/UDA 随DICT是否刷新、AMS8000增删改SAVEWORK后缓存与session是否收敛。列出P0/P1/P2问题、文件行号、触发过程、最小修复及测试；逐条区分已实现/缺失/仅弱测试；不要仅以green tests推导验收。不修改代码。中文输出。',slug:'refno-current-gpt6-review',waitForCompletion:false,dryRun:process.argv[2]!=='run'};
try {const result=await client.callTool({name:'consult',arguments:args},undefined,{timeout:180000}); fs.writeFileSync(`verification/oracle-current-review/${args.dryRun?'preview':'dispatch'}.json`,JSON.stringify(result,null,2)); console.log(JSON.stringify(result));}finally{child.kill();}

