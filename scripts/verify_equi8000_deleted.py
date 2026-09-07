"""Verify deletion hides current data while retaining documented tombstones."""
import json,urllib.error
from concurrent.futures import ThreadPoolExecutor
import equi8000_live as e
before=json.loads((e.OUT/'profiles-final-moved'/'all-attributes.json').read_text())
pe=e.sql("SELECT * FROM pe WHERE string::starts_with(name,'/CODEX_TYPES_');")[0]
remaining=[]
pe_by_ref={r['id'].split(':',1)[1]:r for r in pe}
for entry in before:
    for row in entry['durable']:
        records=e.sql('SELECT * FROM '+row['id']+';')[0]
        if records:remaining.append(dict(id=row['id'],tombstoned=pe_by_ref.get(entry['refno'].replace('/','_'),{}).get('deleted') is True))
def check_api(entry):
    try:
        payload=e.api('element/attributes',{'refno':entry['refno']})
        return dict(refno=entry['refno'],passed=False,payload=payload)
    except urllib.error.HTTPError as ex:
        return dict(refno=entry['refno'],status=ex.code,passed=ex.code==404)
with ThreadPoolExecutor(max_workers=8) as pool:api_checks=list(pool.map(check_api,before))
models=e.api('model/records',{'generation_root':e.ROOT})['items']
tree=e.api('tree/children?refno='+e.ROOT)['nodes']
live_pe=[r for r in pe if r.get('deleted') is not True]
result=dict(passed=not live_pe and all(r['tombstoned'] for r in remaining) and all(r['passed'] for r in api_checks) and len(models)==1 and len(tree)==1,
    live_pe=live_pe,retained_attributes=remaining,api_checks=api_checks,models=len(models),tree_nodes=len(tree),checked_attribute_records=len(before))
e.save(e.OUT/'types-final-restored'/'deletion-softdelete-check.json',result)
print(json.dumps({k:v for k,v in result.items() if k not in ['retained_attributes','api_checks']},ensure_ascii=False))
assert result['passed']
