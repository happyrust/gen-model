"""Resolve every RVM audit difference to the live database tree identity."""
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from collections import Counter, defaultdict
import json
import csv
import requests
import threading
from cpv_site_audit import name_key

OUT=Path(__file__).resolve().parents[1]/'output/hvac-sites-audit-20260905'
BASE='http://127.0.0.1:18206'
LOCAL=threading.local()

def children(parent):
    if not hasattr(LOCAL,'http'):
        LOCAL.http=requests.Session()
    r=LOCAL.http.get(BASE+'/api/v1/tree/children',params={'refno':parent['refno']},timeout=30)
    r.raise_for_status()
    nodes=r.json()['nodes']; ordinals=Counter()
    for node in nodes:
        ordinals[node['noun']]+=1
        node['label']=node['name'] if node['name'].startswith('/') else f"{node['noun']} {ordinals[node['noun']]} of {parent['label']}"
    return nodes

all_differences=[]; summaries=[]
for target in json.loads((OUT/'targets.json').read_text(encoding='utf-8')):
    folder=OUT/target['refno'].replace('/','_')
    cache=folder/'tree-nodes.json'
    if cache.exists():
        nodes=json.loads(cache.read_text(encoding='utf-8'))
    else:
        root={**target,'label':target['name'] if target['name'].startswith('/') else 'SITE of /*','children_count':1}
        nodes=[];frontier=[root]
        with ThreadPoolExecutor(12) as pool:
            while frontier:
                level=[n for page in pool.map(children,frontier) for n in page]
                nodes.extend(level);frontier=[n for n in level if n['children_count']]
        cache.write_text(json.dumps(nodes,indent=2,ensure_ascii=False),encoding='utf-8')
    by_name=defaultdict(list)
    for n in nodes: by_name[name_key(n['label'])].append(n)
    report=json.loads((folder/'audit.json').read_text())
    for r in report['results']:
        matches=by_name[r['name']]
        if r['status']=='missing' and len(matches)==1:
            r['noun']=matches[0]['noun'];r['refno']=matches[0]['refno'].replace('_','/')
        if r['status']!='bounds_pass':
            all_differences.append(dict(site=target['site'],scope=target['name'],scope_refno=target['refno'],
                 status=r['status'],refno=r.get('refno',''),noun=r.get('noun',r['name'].split()[0] if not r['name'].startswith('/') else 'unresolved'),
                 name=r['name'],error_mm=r.get('error_mm','')))
    (folder/'audit.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
    summary={**target,'database_nodes':len(nodes),'missing_by_noun':dict(Counter(r.get('noun',r['name'].split()[0]) for r in report['results'] if r['status']=='missing'))}
    summaries.append(summary);print(json.dumps(summary,ensure_ascii=False),flush=True)
(OUT/'identity-summary.json').write_text(json.dumps(summaries,indent=2,ensure_ascii=False),encoding='utf-8')
with (OUT/'differences.csv').open('w',newline='',encoding='utf-8-sig') as f:
    writer=csv.DictWriter(f,fieldnames=list(all_differences[0]));writer.writeheader();writer.writerows(all_differences)
