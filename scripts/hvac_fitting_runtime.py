"""Verify one native-matched SCTN/FITT assembly per HVAC scope on the UI backend."""
from pathlib import Path
import json
import requests

root = Path(__file__).resolve().parents[1]
out = root/'output/hvac-flex-gril-fix-20260905'
results = []
http = requests.Session()
for scope in ['24381_46775', '24381_48631', '24381_57476', '24381_79977', '24381_100675']:
    folder = out/scope
    mapping = json.loads((folder/'owned-fittings-mapping.json').read_text())
    ref = mapping[0]['host_refno']
    assert mapping[0]['host_bounds_status'] == 'bounds_pass'
    generated = next(e for e in json.loads((folder/'generated/elements.json').read_text()) if e['refno'] == ref)
    response = http.post('http://127.0.0.1:18206/api/v1/model/ensure', json={'refno':ref}, timeout=150)
    response.raise_for_status()
    response = http.post('http://127.0.0.1:18206/api/v1/model/records',
                         json={'generation_root':ref,'limit':100}, timeout=60)
    response.raise_for_status()
    page = response.json()
    (out/f'runtime-fitt-{ref.replace("/","_")}.json').write_text(json.dumps(page,indent=2))
    item = next(e for e in page['items'] if e['refno'].replace('_','/') == ref)
    bounds = item['world_aabb']['mins'] + item['world_aabb']['maxs']
    expected = generated['aabb_min'] + generated['aabb_max']
    error = max(abs(a-b) for a,b in zip(bounds,expected))
    results.append(dict(scope=scope,refno=ref,error_mm=error,passed=error<0.05))
(out/'runtime-fittings.json').write_text(json.dumps(results,indent=2))
print(json.dumps(results,indent=2))
assert all(r['passed'] for r in results)
