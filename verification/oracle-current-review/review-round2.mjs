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
files.push('C:/Users/dpc/.oracle/sessions/refno-current-gpt6-review/artifacts/transcript.md');
const args={engine:'browser',model:'gpt-6',browserThinkingTime:'pro',browserModelStrategy:'select',files,prompt:'第二轮纠错复审。附件含上一轮报告，它不是事实来源。已发现上一轮把 direct_tree.rs 43-47 行过时注释当成执行路径；实际 open_with_store 调用 registry.get_blocking，没有 ensure 骨架调用。请撤回不成立的扫描结论，并复查所有问题。要求每项必须有真实源码文件与准确行号、可达调用链、最小触发时序，区分证实缺陷/待验证风险/误报。优先审查 registry 同dbnum集变化后只invalidate却保留旧route_directory是否导致新路径永远不生效；attrs pin/session/index并发是否有generation fence；provider is_current/generation检查实际覆盖什么；DICT独立刷新是否存在；health字段实际缺什么。输出最小修复方案和能先失败后通过的确定性测试，不要给泛泛建议。不把单元测试绿色当现场SAVEWORK证据。审核 gen-model 当前 RefNo 原生路由与 lazy MDB 实现，Windows Rust。必须以附件源码为准，不信任以前所有测试通过的叙述。目标：RefNo::dbno 解码，MDB上下文路由，零启动元素扫描，共享惰性存储，跨库属性/owner/tree 按需读取，按 path/dbnum/generation 定向失效，single-flight，DICT独立刷新，HTTP bind先于预热，health真实反映 stale/failed/refresh。特别审查 registry zip fingerprint 比较是否漏 path/identity/type 变化、失效后并发旧读是否回填、provider 是否每请求检查 registry、schema/UDA 随DICT是否刷新、AMS8000增删改SAVEWORK后缓存与session是否收敛。列出P0/P1/P2问题、文件行号、触发过程、最小修复及测试；逐条区分已实现/缺失/仅弱测试；不要仅以green tests推导验收。不修改代码。中文输出。',slug:'refno-gpt6-corrective-review',waitForCompletion:false,dryRun:process.argv[2]!=='run'};
try {const result=await client.callTool({name:'consult',arguments:args},undefined,{timeout:180000}); fs.writeFileSync(`verification/oracle-current-review/round2-${args.dryRun?'preview':'dispatch'}.json`,JSON.stringify(result,null,2)); console.log(JSON.stringify(result)); if(!args.dryRun && result.structuredContent?.sessionId){const waited=await client.callTool({name:'wait',arguments:{id:result.structuredContent.sessionId,timeoutMs:1200000}});fs.writeFileSync('verification/oracle-current-review/round2-result.json',JSON.stringify(waited,null,2));console.log(JSON.stringify(waited));}}finally{child.kill();}


