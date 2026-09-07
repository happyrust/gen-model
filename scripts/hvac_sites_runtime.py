"""Generate all audited scopes on the backend used by plant-ui."""
from pathlib import Path
import json
import requests
import argparse
OUT=Path(__file__).resolve().parents[1]/'output/hvac-sites-audit-20260905'
parser=argparse.ArgumentParser()
parser.add_argument('--only',nargs='+')
args=parser.parse_args()
http=requests.Session()
summary_path=OUT/'runtime-summary-current.json'
summary=json.loads(summary_path.read_text(encoding='utf-8')) if summary_path.exists() else []
for target in json.loads((OUT/'targets.json').read_text(encoding='utf-8')):
    if args.only and target['refno'] not in args.only: continue
    summary=[s for s in summary if s['refno']!=target['refno']]
    folder=OUT/target['refno'].replace('/','_')
    cache=folder/'runtime-ensure-current.json'
    if cache.exists():
        data=json.loads(cache.read_text())
    else:
        print('Generating runtime',target['refno'],target['name'],flush=True)
        # A 504 leaves generation running. Record the bounded test result;
        # do not start another generation request to poll the same scope.
        r=http.post('http://127.0.0.1:18206/api/v1/model/ensure',json={'refno':target['refno']},timeout=150)
        (folder/'runtime-current-response.json').write_text(json.dumps({'status':r.status_code,'body':r.text},indent=2),encoding='utf-8')
        if not r.ok:
            summary.append({**target,'status':'runtime_failed','http_status':r.status_code,'error':r.text})
            (OUT/'runtime-summary-current.json').write_text(json.dumps(summary,indent=2,ensure_ascii=False),encoding='utf-8')
            print(target['refno'],'HTTP',r.status_code,r.text[:250],flush=True)
            continue
        data=r.json();cache.write_text(json.dumps(data,indent=2))
    expected=len(json.loads((folder/'generated/elements.json').read_text()))
    summary.append({**target,'runtime_instances':data['model_instance_count'],'offline_instances':expected,
                    'counts_match':data['model_instance_count']==expected,'model_available':data['model_available']})
    (OUT/'runtime-summary-current.json').write_text(json.dumps(summary,indent=2,ensure_ascii=False),encoding='utf-8')
    print(summary[-1],flush=True)
if any(s.get('status')=='runtime_failed' or not s.get('counts_match',False) for s in summary):
    raise SystemExit(1)
